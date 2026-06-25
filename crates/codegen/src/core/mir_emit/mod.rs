//! MIR -> C emitter: the direct printer.
//!
//! this is the track 2 codegen, the only path since the segment 5 cutover (it
//! replaced the HIR-walking emitter). it walks a [`MirBody`] and prints c, one
//! construct to one c form, making no semantic decisions (control-flow
//! flattening and temp generation already happened in `mir::lower`). the driver
//! seam is `src/backend.rs`; the oracle is program output, not c text.
//!
//! split by concern (same layout as `mir::lower` and `typeck::infer`):
//! - this module: the [`gen_mir`] entry, the [`MirGen`] context, the top-level
//!   driver (`gen_all`), function / type-declaration / global emission, the
//!   statement printer (`gen_stmt`), and the shared free helpers (`c_fn_name`,
//!   `write_c_char_literal`, `local_name`).
//! - [`expr`]: value emission - `gen_rvalue` / `gen_operand` / `gen_place`,
//!   place-type recovery, the `println` rendering, and literals.
//! - [`switch`]: a `MirStmt::Switch` as an `if`/`else-if` (or flag-gated) chain
//!   plus its arm tests.
//! - [`strings`]: the string-literal pool - collection, the file-scope byte
//!   statics, and id lookup.

use super::arrays::{array_wrapper_name, fn_typedef_name};
use super::types::{CDeclarator, CType};
use hir::core::{
    ConstValue, Enum, Expr, FieldId, FnId, Function, HIR, Resolution, Text, TypeInterner, TypeKind,
    TypeNode, TypeRef, topo_order,
};
use mir::core::{MirBlock, MirBody, MirStmt, Place, Type};
use rustc_hash::{FxBuildHasher, FxHashMap};
use std::fmt::Write as _;

mod expr;
mod strings;
mod switch;

use strings::collect_strings;

/// generate a complete c translation unit from the HIR plus each defined
/// function's pre-lowered MIR (keyed by [`FnId`]). MIR lowering happens
/// upstream - in the database's `mir_map` query (memoized, shared with
/// `--dump-mir`) or in [`mir::lower_all`] for direct callers - so the emitter
/// never lowers a body twice.
pub fn gen_mir(hir: &HIR, mirs: &FxHashMap<FnId, MirBody>, expr_type_seed: &[TypeRef]) -> String {
    MirGen::new(hir, mirs, expr_type_seed).gen_all()
}

struct MirGen<'a> {
    hir: &'a HIR,
    /// whole-file typeck expression types, seeding the type-declaration topology
    /// for wrapper typedefs of intermediate values (S2C C5). see [`gen_mir`].
    expr_type_seed: &'a [TypeRef],
    /// pre-lowered MIR per defined function. a function absent here (an
    /// `extern`, or a body MIR the caller chose not to lower) emits as a
    /// prototype / empty body.
    mirs: &'a FxHashMap<FnId, MirBody>,
    output: String,
    indent: usize,
    local_names: Vec<Text>,
    /// unique string-literal contents in discovery order; the index is the
    /// literal's id. a string literal is `&[uint8; N]` (HORIZON0 C3): its bytes
    /// live in a file-scope static (`__eye_str{id}`) and the value is a wrapper
    /// pointer into it. see [`MirGen::collect_strings`].
    strings: Vec<Text>,
    /// maps each unique string to its index in `strings` for o(1) lookup in
    /// `string_id`, replacing the former linear scan.
    string_index: FxHashMap<Text, usize>,
    /// cached error-sentinel handle so error-type returns don't need a mutable
    /// borrow just to interp the error kind.
    error_ty: TypeRef,
    /// monotonic counter for guarded-switch fall-through flags (`_g0`, `_g1`,
    /// ...). never reset within the translation unit so sibling guarded matches
    /// in the same c block never collide.
    guard_flag: usize,
    /// A2: memoized place_type results. keyed by the full place so
    /// that repeated calls (index_access, place_is_pointer_like, specifier
    /// resolution) avoid re-walking deep projection chains.
    place_types: FxHashMap<Place, Type>,
}

/// the c identifier for a function. a user-defined `main` is emitted as
/// `__eye_main` so the generated `int main(void)` entry shim owns the reserved
/// `main` symbol; every other function (and any `extern`) keeps its name.
fn c_fn_name(f: &Function) -> &str {
    if !f.is_extern && f.name == "main" {
        "__eye_main"
    } else {
        &f.name
    }
}

/// whether `main`'s return type is an integer, and so forwards to the process
/// exit code in the entry shim. only the integer scalars qualify; every other
/// type (float, bool, char, pointer, aggregate, enum) makes `main` run for its
/// effect and exit 0.
fn main_ret_is_integer(ty: TypeRef, types: &TypeInterner) -> bool {
    matches!(
        types.lookup(ty),
        TypeKind::Path(name) if matches!(
            name.as_str(),
            "int8" | "int16" | "int32" | "int64"
                | "uint8" | "uint16" | "uint32" | "uint64"
                | "usize" | "isize"
        )
    )
}

/// render a `char` as a valid c character literal, re-escaping the control
/// characters and the quote/backslash. HIR stores the decoded char (`'\n'` ->
/// the newline byte), so codegen must put the escape back or the emitted `'<x>'`
/// is invalid c.
fn write_c_char_literal(c: char, out: &mut String) {
    match c {
        '\n' => out.push_str("'\\n'"),
        '\t' => out.push_str("'\\t'"),
        '\r' => out.push_str("'\\r'"),
        '\0' => out.push_str("'\\0'"),
        '\\' => out.push_str("'\\\\'"),
        '\'' => out.push_str("'\\''"),
        other => {
            out.push('\'');
            out.push(other);
            out.push('\'');
        }
    }
}

impl<'a> MirGen<'a> {
    fn new(
        hir: &'a HIR,
        mirs: &'a FxHashMap<FnId, MirBody>,
        expr_type_seed: &'a [TypeRef],
    ) -> Self {
        let (strings, string_index) = collect_strings(hir, mirs);
        let error_ty = hir.types.error_type();
        Self {
            hir,
            mirs,
            expr_type_seed,
            output: String::new(),
            indent: 0,
            local_names: Vec::new(),
            string_index,
            strings,
            error_ty,
            guard_flag: 0,
            place_types: FxHashMap::with_capacity_and_hasher(64, FxBuildHasher),
        }
    }

    fn gen_all(mut self) -> String {
        self.output
            .push_str("// generated by the Eye Compiler v0.7\n");
        self.output.push_str("#include <stdint.h>\n");
        self.output.push_str("#include <stddef.h>\n");
        self.output.push_str("#include <stdbool.h>\n\n");

        // no `<stdio.h>`: an `extern` block is the sole prototype for any libc
        // function the program declares (the rust FFI model), and a header
        // prototype would conflict with a user declaration (e.g. `fopen`
        // returning an opaque `FILE*` vs the header's `struct FILE`). the one
        // libc symbol eye itself emits is `printf` (the `println` intrinsic),
        // so a program that uses `println` without declaring `printf` gets
        // this ABI-identical prototype.
        // A1: needs_printf_prototype scans all body exprs via
        // self.hir.bodies.iter() any(exprs any(matches!(println))).
        // this is o(n*e) but called only once. acceptable for v1.
        if self.needs_printf_prototype() {
            self.output.push_str("int printf(const char *, ...);\n\n");
        }

        // type declarations in dependency order (docs: object topology). enums
        // have no dependencies, so emit them first. then forward-declare every
        // struct, union, and array wrapper with a named tag, so pointer fields,
        // self-references, and `&[Self; N]` all resolve. then emit the full
        // definitions in topological order of value embedding, so every
        // value-embedded type is complete first. the order is the shared
        // `typegraph` topo sort, so it agrees with the HIR value-recursion check
        // on which programs are legal.
        for (_, e) in self.hir.enums.iter() {
            self.gen_enum(e);
        }
        let nodes = topo_order(self.hir, self.expr_type_seed);
        let mut any_fwd = false;
        // opaque FFI types (`extern { type FILE; }`): forward typedef only,
        // never a definition - the c side owns the layout, eye only passes
        // pointers. the tag spelling matches the struct/union typedefs below.
        for (_, o) in self.hir.opaques.iter() {
            self.w(format_args!("typedef struct {0} {0};\n", o.name));
            any_fwd = true;
        }
        for (_, s) in self.hir.structs.iter() {
            self.w(format_args!("typedef struct {0} {0};\n", s.name));
            any_fwd = true;
        }
        for (_, u) in self.hir.unions.iter() {
            self.w(format_args!("typedef union {0} {0};\n", u.name));
            any_fwd = true;
        }
        {
            let types = &self.hir.types;
            for node in &nodes {
                if let TypeNode::Array { elem, len } = node {
                    let name = array_wrapper_name(*elem, *len, types);
                    self.w(format_args!("typedef struct {0} {0};\n", name));
                    any_fwd = true;
                }
            }
        }
        if any_fwd {
            self.output.push('\n');
        }
        for node in &nodes {
            self.gen_type_def(node);
        }

        // module-level statics: top-level `let`/`mut` globals (HORIZON0 C3),
        // emitted at file scope before the functions that reference them.
        self.gen_globals();
        // string-literal backing storage (HORIZON0 C3): one NUL-terminated byte
        // array per unique *referenced* literal (P2: a literal println inlines
        // gets no static), also file-scope before the functions.
        self.gen_string_statics();

        let extern_fns: Vec<FnId> = self
            .hir
            .functions
            .iter()
            .filter(|(_, f)| f.is_extern)
            .map(|(id, _)| id)
            .collect();
        for id in extern_fns {
            self.gen_function(id);
        }
        let defined_fns: Vec<FnId> = self
            .hir
            .functions
            .iter()
            .filter(|(_, f)| !f.is_extern)
            .map(|(id, _)| id)
            .collect();
        for id in defined_fns {
            self.gen_function(id);
        }

        // c entry shim. the runtime requires `int main(void)`; the user's `main`
        // (emitted as `__eye_main`) is adapted to it here, which is what lets
        // `main` declare any return type. an integer return forwards as the
        // process exit code (cast to `int` so a wider integer is well-defined);
        // every other return type - void included - runs `main` for its effect
        // and exits 0, so a `bool`/`float`/struct/array return still produces
        // valid c. this is the sole place the c entry convention lives; a non-c
        // backend omits it entirely.
        let main_ret = self
            .hir
            .functions
            .iter()
            .find(|(_, f)| !f.is_extern && f.name == "main")
            .map(|(_, m)| m.ret);
        if let Some(ret) = main_ret {
            self.output.push_str("int main(void) {\n");
            let types = &self.hir.types;
            if ret.is_some_and(|r| main_ret_is_integer(r, types)) {
                self.output.push_str("\treturn (int)__eye_main();\n");
            } else {
                self.output.push_str("\t__eye_main();\n\treturn 0;\n");
            }
            self.output.push_str("}\n");
        }

        self.output
    }

    /// whether the unit needs the emitter's own `printf` prototype: some body
    /// calls the `println` intrinsic (the only call that survives to MIR with
    /// an unresolved callee) and no user `printf` declaration exists to serve
    /// as the prototype. a user `extern printf(string fmt, ...) -> int32`
    /// emits `int32_t printf(const char* fmt, ...)`, which is the same ABI.
    fn needs_printf_prototype(&self) -> bool {
        if self.hir.items.functions.contains_key("printf") {
            return false;
        }
        self.hir.bodies.iter().any(|(_, body)| {
            body.exprs.iter().any(|(_, expr)| {
                matches!(expr, Expr::Path(Resolution::Unresolved(name)) if name == "println")
            })
        })
    }

    /// emit an indented block body: bump the indent, run `body`, restore the
    /// indent, then write `close` at the restored indentation. the caller
    /// writes the opening line, which varies too much (condition, `else`,
    /// typedef tail) to fold in here.
    fn block(&mut self, close: &str, body: impl FnOnce(&mut Self)) {
        self.indent += 1;
        body(self);
        self.indent -= 1;
        self.push_indent();
        self.output.push_str(close);
    }

    fn push_indent(&mut self) {
        for _ in 0..self.indent {
            self.output.push('\t');
        }
    }

    fn gen_function(&mut self, fn_id: FnId) {
        let r#fn = &self.hir.functions[fn_id];
        // `main` is an ordinary eye function. the c runtime reserves the symbol
        // `main` for the `int main(void)` entry point, so the user's `main` is
        // emitted under an internal name and a shim is generated in `gen_all`.
        let name = c_fn_name(r#fn);
        if r#fn.is_extern {
            let types = &self.hir.types;
            match r#fn.ret {
                Some(ret) => self
                    .output
                    .write_fmt(format_args!("{} {}(", CType::new(ret, types), name))
                    .expect("writing to String cannot fail"),
                None => self
                    .output
                    .write_fmt(format_args!("void {}(", name))
                    .expect("writing to String cannot fail"),
            }
            self.comma_params(r#fn, false);
            self.output.push_str(");\n");
            return;
        }

        {
            let types = &self.hir.types;
            match r#fn.ret {
                Some(ret) => self
                    .output
                    .write_fmt(format_args!("{} {}(", CType::new(ret, types), name))
                    .expect("writing to String cannot fail"),
                None => self
                    .output
                    .write_fmt(format_args!("void {}(", name))
                    .expect("writing to String cannot fail"),
            }
        }
        self.comma_params(r#fn, true);
        self.output.push_str(") {\n");
        self.block("}\n\n", |s| {
            if let Some(mir) = s.mirs.get(&fn_id) {
                s.place_types.clear();
                s.local_names = Self::local_names(mir);
                s.gen_block(mir, &mir.body);
                s.local_names.clear();
            }
        });
    }

    fn comma_params(&mut self, r#fn: &Function, with_names: bool) {
        // c distinguishes `f()` (an unprototyped declaration, deprecated) from
        // `f(void)` (a zero-parameter prototype); only the latter is a valid
        // prototype under `-Wstrict-prototypes` / C23.
        if r#fn.params.is_empty() && !r#fn.variadic {
            self.output.push_str("void");
            return;
        }
        let types = &self.hir.types;
        for (i, param) in r#fn.params.iter().enumerate() {
            if i > 0 {
                self.output.push_str(", ");
            }
            if with_names {
                self.output
                    .write_fmt(format_args!(
                        "{} {}",
                        CType::new(param.ty, types),
                        param.name
                    ))
                    .expect("writing to String cannot fail");
            } else {
                self.output
                    .write_fmt(format_args!("{}", CType::new(param.ty, types)))
                    .expect("writing to String cannot fail");
            }
        }
        // the parser guarantees `...` follows at least one named parameter
        if r#fn.variadic {
            self.output.push_str(", ...");
        }
    }

    fn gen_block(&mut self, mir: &MirBody, block: &MirBlock) {
        for stmt in &block.stmts {
            self.gen_stmt(mir, stmt);
        }
    }

    fn gen_stmt(&mut self, mir: &MirBody, stmt: &MirStmt) {
        match stmt {
            MirStmt::Let { local, init } => {
                self.push_indent();
                let l = &mir.locals[*local];
                let name = local_name(&self.local_names, *local);
                {
                    let types = &self.hir.types;
                    self.output
                        .write_fmt(format_args!("{}", CDeclarator::new(l.ty, name, types)))
                        .expect("writing to String cannot fail");
                }
                if let Some(rv) = init {
                    self.output.push_str(" = ");
                    self.gen_rvalue(mir, rv);
                }
                self.output.push_str(";\n");
            }
            MirStmt::Assign { place, value } => {
                self.push_indent();
                self.gen_place(mir, place);
                self.output.push_str(" = ");
                self.gen_rvalue(mir, value);
                self.output.push_str(";\n");
            }
            MirStmt::Eval(rv) => {
                self.push_indent();
                self.gen_rvalue(mir, rv);
                self.output.push_str(";\n");
            }
            MirStmt::Return(op) => {
                self.push_indent();
                self.output.push_str("return");
                if let Some(op) = op {
                    self.output.push(' ');
                    self.gen_operand(mir, op);
                }
                self.output.push_str(";\n");
            }
            MirStmt::If {
                cond,
                then_block,
                else_block,
            } => {
                self.push_indent();
                self.output.push_str("if (");
                self.gen_operand(mir, cond);
                self.output.push_str(") {\n");
                self.block("}", |s| s.gen_block(mir, then_block));
                if let Some(else_block) = else_block {
                    self.output.push_str(" else {\n");
                    self.block("}", |s| s.gen_block(mir, else_block));
                }
                self.output.push('\n');
            }
            MirStmt::Loop { body } => {
                self.push_indent();
                self.output.push_str("while (true) {\n");
                self.block("}\n", |s| s.gen_block(mir, body));
            }
            MirStmt::Switch {
                scrut,
                arms,
                default,
            } => self.gen_switch(mir, scrut, arms, default),
            MirStmt::Break => {
                self.push_indent();
                self.output.push_str("break;\n");
            }
            MirStmt::Continue => {
                self.push_indent();
                self.output.push_str("continue;\n");
            }
        }
    }

    fn gen_enum(&mut self, e: &Enum) {
        self.output.push_str("typedef enum {\n");
        self.block("", |s| {
            for variant in &e.variants {
                s.push_indent();
                s.w(format_args!("{},\n", variant.name));
            }
        });
        self.w(format_args!("}} {};\n\n", e.name));
    }

    /// emit one type-declaration node's full definition, in topological order.
    /// nominal types were already forward-declared (named tag), so a definition
    /// here is `struct Name { ... };` (no `typedef`); an array wrapper is its
    /// value-wrapper struct typedef.
    fn gen_type_def(&mut self, node: &TypeNode) {
        match node {
            TypeNode::Nominal(name) => {
                if let Some(&id) = self.hir.items.structs.get(name) {
                    let s = &self.hir.structs[id];
                    self.gen_record_def("struct", &s.name, &s.fields);
                } else if let Some(&id) = self.hir.items.unions.get(name) {
                    let u = &self.hir.unions[id];
                    self.gen_record_def("union", &u.name, &u.fields);
                }
            }
            TypeNode::Array { elem, len } => {
                let types = &self.hir.types;
                // `len` is 0 only for the empty string literal (`""` types as
                // `&[uint8; 0]`; a `[T; 0]` array type is rejected upstream). a
                // zero-length array member is a GCC/clang extension, so the
                // storage is padded to 1; the type-level length stays 0, and the
                // backing static already carries the NUL byte this slot aliases.
                self.w(format_args!(
                    "struct {} {{ {} data[{}]; }};\n",
                    array_wrapper_name(*elem, *len, types),
                    CType::new(*elem, types),
                    (*len).max(1)
                ));
            }
            TypeNode::Fn {
                params,
                ret,
                variadic,
            } => {
                let types = &self.hir.types;
                let name = fn_typedef_name(params, *ret, *variadic, types);
                match ret {
                    Some(r) => self
                        .output
                        .write_fmt(format_args!(
                            "typedef {} (*{})(",
                            CType::new(*r, types),
                            name
                        ))
                        .expect("writing to String cannot fail"),
                    None => self
                        .output
                        .write_fmt(format_args!("typedef void (*{})(", name))
                        .expect("writing to String cannot fail"),
                }
                if params.is_empty() {
                    // a variadic with no fixed params is unreachable from the
                    // grammar (`...` follows at least one param), but emit the
                    // honest c form rather than `void` if it ever arrives.
                    self.output.push_str(if *variadic { "..." } else { "void" });
                } else {
                    let types = &self.hir.types;
                    for (i, p) in params.iter().enumerate() {
                        if i > 0 {
                            self.output.push_str(", ");
                        }
                        self.output
                            .write_fmt(format_args!("{}", CType::new(*p, types)))
                            .expect("writing to String cannot fail");
                    }
                    if *variadic {
                        self.output.push_str(", ...");
                    }
                }
                self.output.push_str(");\n");
            }
        }
    }

    /// emit a struct or union definition body for an already-forward-declared
    /// named tag: `struct Name { <fields> };`.
    fn gen_record_def(&mut self, kw: &str, name: &Text, fields: &[FieldId]) {
        self.w(format_args!("{kw} {name} {{\n"));
        self.block("};\n\n", |s| {
            let types = &s.hir.types;
            for &field_id in fields {
                let field = &s.hir.fields[field_id];
                s.push_indent();
                s.output
                    .write_fmt(format_args!(
                        "{} {};\n",
                        CType::new(field.ty, types),
                        field.name
                    ))
                    .expect("writing to String cannot fail");
            }
        });
    }

    /// emit each top-level global as a file-scope c static. a `let` global is
    /// `static const` (read-only); a `mut` global is `static` (mutable). the
    /// initializer is the const-folded scalar value (c requires a constant static
    /// initializer at the floor). a poisoned global (failed fold) was already
    /// diagnosed and halts compilation, so it is skipped here.
    fn gen_globals(&mut self) {
        let mut any = false;
        for (_, g) in self.hir.globals.iter() {
            let Some(value) = &g.value else { continue };
            let qual = if g.mutable { "static" } else { "static const" };
            let types = &self.hir.types;
            self.output
                .write_fmt(format_args!(
                    "{} {} {} = ",
                    qual,
                    CType::new(g.ty, types),
                    g.name
                ))
                .expect("writing to String cannot fail");
            match value {
                ConstValue::Int(v) => {
                    self.output.push_str(itoa::Buffer::new().format(*v));
                }
                // ryu formatting: always emits a decimal point so c reads it as
                // `double`, faster than rust's debug formatter.
                ConstValue::Float(f) => {
                    self.output.push_str(ryu::Buffer::new().format_finite(*f));
                }
                ConstValue::Bool(b) => self.output.push_str(if *b { "true" } else { "false" }),
                ConstValue::Char(c) => write_c_char_literal(*c, &mut self.output),
            }
            self.output.push_str(";\n");
            any = true;
        }
        if any {
            self.output.push('\n');
        }
    }

    /// the c name for a local. parameters keep their bare source name (the
    /// function signature declares them by that name). every other local - a
    /// `let` binding or a generated temp - is suffixed with its [`LocalId`], so
    /// two same-named `let`s in one c scope (same-block shadowing) cannot
    /// collide into a redeclaration. MIR's locals arena gives each a unique id;
    /// suffixing surfaces it. this is output-invariant (it only renames), so it
    /// is safe to do now and closes the totality hole before cutover.
    fn local_names(mir: &MirBody) -> Vec<Text> {
        // the bare parameter names are the only names a generated local name
        // can collide with: raw ids make generated names unique among
        // themselves, but a parameter literally named like one (`x_3`, `_t2`)
        // lands in the same c scope - a redefinition error, or a silent
        // shadow miscompile from a nested block. generated names never end in
        // `_`, so appending `_` until free stays injective.
        let param_names: rustc_hash::FxHashSet<&str> = mir
            .params
            .iter()
            .filter_map(|&p| mir.locals[p].name.as_deref())
            .collect();
        let mut names: Vec<Text> = Vec::with_capacity(mir.locals.len());
        for (id, local) in mir.locals.iter() {
            let raw = u32::from(id.raw_idx());
            let idx = raw as usize;
            if names.len() <= idx {
                names.resize(idx + 1, Text::from(""));
            }
            names[idx] = if mir.params.contains(&id) {
                // a parameter always has a source name; the signature uses it bare.
                local
                    .name
                    .as_ref()
                    .expect("parameter has a source name")
                    .clone()
            } else {
                let mut name = match &local.name {
                    Some(name) => format!("{}_{}", name, raw),
                    None => format!("_t{}", raw),
                };
                while param_names.contains(name.as_str()) {
                    name.push('_');
                }
                Text::from(name)
            };
        }
        names
    }

    fn w(&mut self, args: std::fmt::Arguments<'_>) {
        self.output
            .write_fmt(args)
            .expect("writing to String cannot fail");
    }
}

fn local_name(names: &[Text], id: mir::core::LocalId) -> &str {
    let raw = u32::from(id.raw_idx()) as usize;
    names.get(raw).expect("MIR local name was precomputed")
}
