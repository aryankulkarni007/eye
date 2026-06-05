//! MIR -> C emitter: the dumb printer.
//!
//! This is the Track 2 codegen, the only path since the Segment 5 cutover (it
//! replaced the HIR-walking emitter). It walks a [`MirBody`] and prints C, one
//! construct to one C form, making no semantic decisions (control-flow
//! flattening and temp generation already happened in `mir::lower`). The driver
//! seam is `src/backend.rs`; the oracle is program output, not C text.

use super::arrays::{array_wrapper_name, fn_typedef_name};
use super::types::{CDeclarator, CType, spec_for_type};
use hir::core::{
    ConstValue, Enum, Expr, FieldId, Function, HIR, Literal, Resolution, Text, TypeKind, TypeNode,
    TypeRef, topo_order,
};
use mir::core::{ArmTest, MirBlock, MirBody, MirStmt, Operand, Place, RValue, SwitchArm, Type};
use mir::lower::lower_function;
use rustc_hash::{FxBuildHasher, FxHashMap};
use std::fmt::Write as _;

/// Generate a complete C translation unit from the HIR via MIR.
pub fn gen_mir(hir: &HIR) -> String {
    MirGen::new(hir).gen_all()
}

struct MirGen<'a> {
    hir: &'a HIR,
    output: String,
    indent: usize,
    local_names: Vec<Text>,
    /// Unique string-literal contents in discovery order; the index is the
    /// literal's id. A string literal is `&[uint8; N]` (HORIZON0 C3): its bytes
    /// live in a file-scope static (`__eye_str{id}`) and the value is a wrapper
    /// pointer into it. See [`MirGen::collect_strings`].
    strings: Vec<Text>,
    /// Maps each unique string to its index in `strings` for O(1) lookup in
    /// `string_id`, replacing the former linear scan.
    string_index: FxHashMap<Text, usize>,
    /// Cached error-sentinel handle so error-type returns don't need a mutable
    /// borrow just to interp the error kind.
    error_ty: TypeRef,
    /// Monotonic counter for guarded-switch fall-through flags (`_g0`, `_g1`,
    /// ...). Never reset within the translation unit so sibling guarded matches
    /// in the same C block never collide.
    guard_flag: usize,
    /// EXPERIMENTAL(A2): Memoized place_type results. Keyed by the full Place so
    /// that repeated calls (index_access, place_is_pointer_like, specifier
    /// resolution) avoid re-walking deep projection chains.
    place_types: FxHashMap<Place, Type>,
}

/// Collect the unique string-literal contents across every function body, in a
/// deterministic (arena) order, so each gets one shared file-scope static.
fn collect_strings(hir: &HIR) -> (Vec<Text>, FxHashMap<Text, usize>) {
    let mut seen: FxHashMap<&Text, usize> =
        FxHashMap::with_capacity_and_hasher(hir.bodies.len() * 2, FxBuildHasher);
    let mut out: Vec<Text> = Vec::new();
    for (_, body) in hir.bodies.iter() {
        for (_, expr) in body.exprs.iter() {
            if let Expr::Literal(Literal::String(s)) = expr
                && !seen.contains_key(s)
            {
                let idx = out.len();
                seen.insert(s, idx);
                out.push(s.clone());
            }
        }
    }
    let string_index: FxHashMap<Text, usize> = out
        .iter()
        .enumerate()
        .map(|(i, s)| (s.clone(), i))
        .collect();
    (out, string_index)
}

enum IndexAccess {
    ArrayValue,
    ArrayPointer,
    Direct,
}

/// Whether `ty` is a string value - a reference to a `uint8` array
/// (`&[uint8; N]`). Such a value prints with `%s` over its byte array, not as a
/// pointer address.
fn is_byte_string(ty: TypeRef, types: &TypeInterner) -> bool {
    matches!(
        types.lookup(ty),
        TypeKind::Ref(inner)
            if matches!(
                types.lookup(*inner),
                TypeKind::Array { elem, .. } if matches!(types.lookup(*elem), TypeKind::Path(n) if n == "uint8")
            )
    )
}
use hir::core::TypeInterner;

/// The C identifier for a function. A user-defined `main` is emitted as
/// `__eye_main` so the generated `int main(void)` entry shim owns the reserved
/// `main` symbol; every other function (and any `extern`) keeps its name.
fn c_fn_name(f: &Function) -> &str {
    if !f.is_extern && f.name == "main" {
        "__eye_main"
    } else {
        &f.name
    }
}

/// Whether `main`'s return type is an integer, and so forwards to the process
/// exit code in the entry shim. Only the integer scalars qualify; every other
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

/// Render a `char` as a valid C character literal, re-escaping the control
/// characters and the quote/backslash. HIR stores the decoded char (`'\n'` ->
/// the newline byte), so codegen must put the escape back or the emitted `'<x>'`
/// is invalid C.
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
    fn new(hir: &'a HIR) -> Self {
        let (strings, string_index) = collect_strings(hir);
        let error_ty = hir.types.borrow_mut().error_type();
        Self {
            hir,
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

        // No `<stdio.h>`: an `extern` block is the sole prototype for any libc
        // function the program declares (the Rust FFI model), and a header
        // prototype would conflict with a user declaration (e.g. `fopen`
        // returning an opaque `FILE*` vs the header's `struct FILE`). The one
        // libc symbol Eye itself emits is `printf` (the `println` intrinsic),
        // so a program that uses `println` without declaring `printf` gets
        // this ABI-identical prototype.
        // A1: needs_printf_prototype scans all body exprs via
        // self.hir.bodies.iter() any(exprs any(matches!(println))).
        // This is O(N*E) but called only once. Acceptable for v1.
        if self.needs_printf_prototype() {
            self.output.push_str("int printf(const char *, ...);\n\n");
        }

        // Type declarations in dependency order (docs: object topology). Enums
        // have no dependencies, so emit them first. Then forward-declare every
        // struct, union, and array wrapper with a named tag, so pointer fields,
        // self-references, and `&[Self; N]` all resolve. Then emit the full
        // definitions in topological order of value embedding, so every
        // value-embedded type is complete first. The order is the shared
        // `typegraph` topo sort, so it agrees with the HIR value-recursion check
        // on which programs are legal.
        for (_, e) in self.hir.enums.iter() {
            self.gen_enum(e);
        }
        let nodes = topo_order(self.hir);
        let mut any_fwd = false;
        // Opaque FFI types (`extern { type FILE; }`): forward typedef only,
        // never a definition - the C side owns the layout, Eye only passes
        // pointers. The tag spelling matches the struct/union typedefs below.
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
            let types = self.hir.types.borrow();
            for node in &nodes {
                if let TypeNode::Array { elem, len } = node {
                    let name = array_wrapper_name(*elem, *len, &types);
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

        // Module-level statics: top-level `let`/`mut` globals (HORIZON0 C3),
        // emitted at file scope before the functions that reference them.
        self.gen_globals();
        // String-literal backing storage (HORIZON0 C3): one NUL-terminated byte
        // array per unique literal, also file-scope before the functions.
        self.gen_string_statics();

        let extern_fns = self.hir.functions.iter().filter(|(_, f)| f.is_extern);
        for (_, f) in extern_fns {
            self.gen_function(f);
        }
        let defined_fns = self.hir.functions.iter().filter(|(_, f)| !f.is_extern);
        for (_, f) in defined_fns {
            self.gen_function(f);
        }

        // C entry shim. The runtime requires `int main(void)`; the user's `main`
        // (emitted as `__eye_main`) is adapted to it here, which is what lets
        // `main` declare any return type. An integer return forwards as the
        // process exit code (cast to `int` so a wider integer is well-defined);
        // every other return type - void included - runs `main` for its effect
        // and exits 0, so a `bool`/`float`/struct/array return still produces
        // valid C. This is the sole place the C entry convention lives; a non-C
        // backend omits it entirely.
        let main_ret = self
            .hir
            .functions
            .iter()
            .find(|(_, f)| !f.is_extern && f.name == "main")
            .map(|(_, m)| m.ret);
        if let Some(ret) = main_ret {
            self.output.push_str("int main(void) {\n");
            let types = self.hir.types.borrow();
            if ret.is_some_and(|r| main_ret_is_integer(r, &types)) {
                self.output.push_str("\treturn (int)__eye_main();\n");
            } else {
                self.output.push_str("\t__eye_main();\n\treturn 0;\n");
            }
            self.output.push_str("}\n");
        }

        self.output
    }

    /// Whether the unit needs the emitter's own `printf` prototype: some body
    /// calls the `println` intrinsic (the only call that survives to MIR with
    /// an unresolved callee) and no user `printf` declaration exists to serve
    /// as the prototype. A user `extern printf(string fmt, ...) -> int32`
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

    /// Emit an indented block body: bump the indent, run `body`, restore the
    /// indent, then write `close` at the restored indentation. The caller
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

    fn gen_function(&mut self, r#fn: &Function) {
        // `main` is an ordinary Eye function. The C runtime reserves the symbol
        // `main` for the `int main(void)` entry point, so the user's `main` is
        // emitted under an internal name and a shim is generated in `gen_all`.
        let name = c_fn_name(r#fn);
        if r#fn.is_extern {
            let types = self.hir.types.borrow();
            match r#fn.ret {
                Some(ret) => self.output
                    .write_fmt(format_args!("{} {}(", CType::new(ret, &types), name))
                    .expect("writing to String cannot fail"),
                None => self.output
                    .write_fmt(format_args!("void {}(", name))
                    .expect("writing to String cannot fail"),
            }
            drop(types);
            self.comma_params(r#fn, false);
            self.output.push_str(");\n");
            return;
        }

        {
            let types = self.hir.types.borrow();
            match r#fn.ret {
                Some(ret) => self.output
                    .write_fmt(format_args!("{} {}(", CType::new(ret, &types), name))
                    .expect("writing to String cannot fail"),
                None => self.output
                    .write_fmt(format_args!("void {}(", name))
                    .expect("writing to String cannot fail"),
            }
        }
        self.comma_params(r#fn, true);
        self.output.push_str(") {\n");
        self.block("}\n\n", |s| {
            if let Some(body_id) = r#fn.body {
                let body = &s.hir.bodies[body_id];
                let mir = lower_function(s.hir, body, r#fn.params.len(), r#fn.ret);
                // EXPERIMENTAL(A2): per-function cache. LocalId indices are
                // per-function, so the cache must not leak across functions.
                s.place_types.clear();
                s.local_names = Self::local_names(&mir);
                s.gen_block(&mir, &mir.body);
                s.local_names.clear();
            }
        });
    }

    fn comma_params(&mut self, r#fn: &Function, with_names: bool) {
        // C distinguishes `f()` (an unprototyped declaration, deprecated) from
        // `f(void)` (a zero-parameter prototype); only the latter is a valid
        // prototype under `-Wstrict-prototypes` / C23.
        if r#fn.params.is_empty() && !r#fn.variadic {
            self.output.push_str("void");
            return;
        }
        let types = self.hir.types.borrow();
        for (i, param) in r#fn.params.iter().enumerate() {
            if i > 0 {
                self.output.push_str(", ");
            }
            if with_names {
                self.output
                    .write_fmt(format_args!("{} {}", CType::new(param.ty, &types), param.name))
                    .expect("writing to String cannot fail");
            } else {
                self.output
                    .write_fmt(format_args!("{}", CType::new(param.ty, &types)))
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
                    let types = self.hir.types.borrow();
                    self.output
                        .write_fmt(format_args!("{}", CDeclarator::new(l.ty, name, &types)))
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

    /// Render a [`MirStmt::Switch`] as an `if`/`else if` chain comparing the
    /// scrutinee tag against each variant, not as a C `switch`. A C `switch`
    /// would capture a `break` that a match arm intends for an enclosing loop;
    /// an `if` chain leaves `break`/`continue` bound to the loop. The scrutinee
    /// is a trivial operand, so re-evaluating it per arm has no side effect.
    fn gen_switch(
        &mut self,
        mir: &MirBody,
        scrut: &Operand,
        arms: &[SwitchArm],
        default: &Option<MirBlock>,
    ) {
        // A guard can fail, so its arm must fall through to the next. With guard
        // temp statements that an `&&` cannot hold, an `if`/`else-if` chain
        // cannot express that, so a guarded switch uses a flag-gated chain. The
        // guard-free common case keeps the clean `if`/`else-if`.
        if arms.iter().any(|a| a.guard.is_some()) {
            self.gen_guarded_switch(mir, scrut, arms, default);
            return;
        }
        // A switch with no `default` is a match HIR proved exhaustive (a
        // non-exhaustive match is diagnosed and never reaches codegen), so the
        // last arm's test is tautological. Emit it as the chain's `else`: C
        // cannot see the exhaustiveness, so a tested last arm draws
        // `-Wsometimes-uninitialized` on a value-match's temp and leaves a
        // genuinely uninitialized read if the scrutinee holds a rogue value
        // (e.g. an enum from a bad FFI cast).
        let (chain, last_as_else) = match (default, arms) {
            (None, [chain @ .., last]) => (chain, Some(last)),
            _ => (arms, None),
        };
        let mut first = true;
        for arm in chain {
            self.push_indent();
            self.output
                .push_str(if first { "if (" } else { "else if (" });
            first = false;
            self.gen_arm_test(mir, scrut, &arm.test);
            self.output.push_str(") {\n");
            self.block("}\n", |s| s.gen_block(mir, &arm.body));
        }
        let tail = last_as_else.map(|a| &a.body).or(default.as_ref());
        if let Some(body) = tail {
            self.push_indent();
            self.output.push_str(if first { "{\n" } else { "else {\n" });
            self.block("}\n", |s| s.gen_block(mir, body));
        }
    }

    /// A guarded switch as a flag-gated chain. Each arm fires only while no
    /// earlier arm has both matched and passed its guard (`!flag`); a matched arm
    /// whose guard is false leaves `flag` unset, so the next arm's test is
    /// re-evaluated - the fall-through a plain `if`/`else-if` cannot give once a
    /// guard needs temp statements. No C `switch`/`break`, so a `break` /
    /// `continue` in an arm body still binds to the enclosing loop.
    ///
    /// ```c
    /// bool _gN = false;
    /// if (!_gN && <test>) { <guard.stmts> if (<guard.cond>) { <body> _gN = true; } }
    /// ...
    /// if (!_gN) { <default> }
    /// ```
    fn gen_guarded_switch(
        &mut self,
        mir: &MirBody,
        scrut: &Operand,
        arms: &[SwitchArm],
        default: &Option<MirBlock>,
    ) {
        let flag = format!("_g{}", self.guard_flag);
        self.guard_flag += 1;
        self.push_indent();
        self.w(format_args!("bool {flag} = false;\n"));
        for arm in arms {
            self.push_indent();
            // An `Always` arm (guarded catch-all) has no scrutinee test - it
            // matches anything, gated only by the flag and its own guard.
            match &arm.test {
                ArmTest::Always => self.w(format_args!("if (!{flag}) {{\n")),
                _ => {
                    self.w(format_args!("if (!{flag} && "));
                    self.gen_arm_test(mir, scrut, &arm.test);
                    self.output.push_str(") {\n");
                }
            }
            self.block("}\n", |s| match &arm.guard {
                Some(guard) => {
                    for stmt in &guard.stmts {
                        s.gen_stmt(mir, stmt);
                    }
                    s.push_indent();
                    s.output.push_str("if (");
                    s.gen_operand(mir, &guard.cond);
                    s.output.push_str(") {\n");
                    s.block("}\n", |s| {
                        s.gen_block(mir, &arm.body);
                        s.push_indent();
                        s.w(format_args!("{flag} = true;\n"));
                    });
                }
                None => {
                    s.gen_block(mir, &arm.body);
                    s.push_indent();
                    s.w(format_args!("{flag} = true;\n"));
                }
            });
        }
        if let Some(default) = default {
            self.push_indent();
            self.w(format_args!("if (!{flag}) {{\n"));
            self.block("}\n", |s| s.gen_block(mir, default));
        }
    }

    /// Render an [`ArmTest`] as a C boolean expression over `scrut` (no
    /// surrounding parens; the caller wraps the chain's `if (...)`). One arm per
    /// `ArmTest` kind: S1 adds `Const`, S4 adds `Range`/`Or`.
    fn gen_arm_test(&mut self, mir: &MirBody, scrut: &Operand, test: &ArmTest) {
        match test {
            ArmTest::Variant(v) => {
                self.gen_operand(mir, scrut);
                let label = &self.hir.enums[v.enum_id].variants[v.idx as usize].name;
                self.w(format_args!(" == {}", label));
            }
            ArmTest::Const(lit) => {
                self.gen_operand(mir, scrut);
                self.output.push_str(" == ");
                self.gen_literal(lit);
            }
            // `Always` carries no scrutinee test and only appears guarded, so
            // `gen_guarded_switch` renders it without a test condition; this branch
            // is unreachable. Emit `true` rather than panic.
            ArmTest::Always => self.output.push_str("true"),
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

    /// Emit one type-declaration node's full definition, in topological order.
    /// Nominal types were already forward-declared (named tag), so a definition
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
                let types = self.hir.types.borrow();
                // `len` is 0 only for the empty string literal (`""` types as
                // `&[uint8; 0]`; a `[T; 0]` array type is rejected upstream). A
                // zero-length array member is a GCC/clang extension, so the
                // storage is padded to 1; the type-level length stays 0, and the
                // backing static already carries the NUL byte this slot aliases.
                self.w(format_args!(
                    "struct {} {{ {} data[{}]; }};\n",
                    array_wrapper_name(*elem, *len, &types),
                    CType::new(*elem, &types),
                    (*len).max(1)
                ));
            }
            TypeNode::Fn { params, ret } => {
                let types = self.hir.types.borrow();
                let name = fn_typedef_name(params, *ret, &types);
                match ret {
                    Some(r) => self.output
                        .write_fmt(format_args!("typedef {} (*{})(", CType::new(*r, &types), name))
                        .expect("writing to String cannot fail"),
                    None => self.output
                        .write_fmt(format_args!("typedef void (*{})(", name))
                        .expect("writing to String cannot fail"),
                }
                drop(types);
                if params.is_empty() {
                    self.output.push_str("void");
                } else {
                    let types = self.hir.types.borrow();
                    for (i, p) in params.iter().enumerate() {
                        if i > 0 {
                            self.output.push_str(", ");
                        }
                        self.output
                            .write_fmt(format_args!("{}", CType::new(*p, &types)))
                            .expect("writing to String cannot fail");
                    }
                }
                self.output.push_str(");\n");
            }
        }
    }

    /// Emit a struct or union definition body for an already-forward-declared
    /// named tag: `struct Name { <fields> };`.
    fn gen_record_def(&mut self, kw: &str, name: &Text, fields: &[FieldId]) {
        self.w(format_args!("{kw} {name} {{\n"));
        self.block("};\n\n", |s| {
            let types = s.hir.types.borrow();
            for &field_id in fields {
                let field = &s.hir.fields[field_id];
                s.push_indent();
                s.output
                    .write_fmt(format_args!("{} {};\n", CType::new(field.ty, &types), field.name))
                    .expect("writing to String cannot fail");
            }
        });
    }

    fn gen_rvalue(&mut self, mir: &MirBody, rv: &RValue) {
        match rv {
            RValue::Use(op) => self.gen_operand(mir, op),
            RValue::Binary(op, a, b) => {
                self.output.push('(');
                self.gen_operand(mir, a);
                self.w(format_args!(" {} ", op));
                self.gen_operand(mir, b);
                self.output.push(')');
            }
            RValue::Unary(op, operand) => {
                self.w(format_args!("{}", op));
                self.gen_operand(mir, operand);
            }
            RValue::Cast(operand, ty) => {
                let types = self.hir.types.borrow();
                self.output
                    .write_fmt(format_args!("({})", CType::new(*ty, &types)))
                    .expect("writing to String cannot fail");
                drop(types);
                self.gen_operand(mir, operand);
            }
            // `sizeof(T)`: the C backend is the layout authority.
            RValue::SizeOf(ty) => {
                let types = self.hir.types.borrow();
                self.output
                    .write_fmt(format_args!("sizeof({})", CType::new(*ty, &types)))
                    .expect("writing to String cannot fail");
            }
            RValue::Ref(place) => {
                self.output.push('&');
                self.gen_place(mir, place);
            }
            RValue::Deref(operand) => {
                self.output.push_str("(*");
                self.gen_operand(mir, operand);
                self.output.push(')');
            }
            RValue::Println { args } => self.gen_println(mir, args),
            RValue::Variant(v) => {
                let label = &self.hir.enums[v.enum_id].variants[v.idx as usize].name;
                self.w(format_args!("{}", label));
            }
            RValue::Call { func, args } => {
                let name = c_fn_name(&self.hir.functions[*func]);
                self.w(format_args!("{}(", name));
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.gen_operand(mir, a);
                }
                self.output.push(')');
            }
            // An indirect call: call through the function-pointer operand. C
            // calls through a pointer value with ordinary call syntax.
            RValue::CallIndirect { callee, args } => {
                self.gen_operand(mir, callee);
                self.output.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.gen_operand(mir, a);
                }
                self.output.push(')');
            }
            // A function used as a value: its bare C name, which decays to a
            // function pointer in value context.
            RValue::Func(func) => {
                let name = c_fn_name(&self.hir.functions[*func]);
                self.w(format_args!("{}", name));
            }
            // A value array is a compound literal of its wrapper struct:
            // `(__eye_arr_T_N){{ a, b, c }}` - the outer brace is the struct,
            // the inner initializes its `data[N]`. The type is carried on the
            // node, so unlike the HIR path there is no type-recovery fallback:
            // the emitter trusts the lowering (R2 / I2).
            RValue::ArrayLit { ty, elems } => {
                let types = self.hir.types.borrow();
                let TypeKind::Array { elem, len } = types.lookup(*ty) else {
                    unreachable!("ArrayLit rvalue carries a non-array type: {ty:?}");
                };
                self.output
                    .write_fmt(format_args!("({}){{{{ ", array_wrapper_name(*elem, *len, &types)))
                    .expect("writing to String cannot fail");
                drop(types);
                for (i, el) in elems.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.gen_operand(mir, el);
                }
                self.output.push_str(" }}");
            }
            // `[value; count]`: emit the wrapper with `count` copies of the
            // (already evaluated-once) operand. A future native backend can lower
            // this to a fill loop / memset instead.
            RValue::ArrayRepeat { ty, value, count } => {
                let types = self.hir.types.borrow();
                let TypeKind::Array { elem, len } = types.lookup(*ty) else {
                    unreachable!("ArrayRepeat rvalue carries a non-array type: {ty:?}");
                };
                self.output
                    .write_fmt(format_args!("({}){{{{ ", array_wrapper_name(*elem, *len, &types)))
                    .expect("writing to String cannot fail");
                drop(types);
                for i in 0..*count {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.gen_operand(mir, value);
                }
                self.output.push_str(" }}");
            }
            RValue::StructLit { ty, fields } => {
                let types = self.hir.types.borrow();
                self.output
                    .write_fmt(format_args!("({}){{ ", CType::new(*ty, &types)))
                    .expect("writing to String cannot fail");
                drop(types);
                for (i, (name, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.w(format_args!(".{} = ", name));
                    self.gen_operand(mir, value);
                }
                self.output.push_str(" }");
            }
        }
    }

    fn gen_operand(&mut self, mir: &MirBody, op: &Operand) {
        match op {
            Operand::Const(lit) => self.gen_literal(lit),
            Operand::Copy(place) => self.gen_place(mir, place),
        }
    }

    fn gen_place(&mut self, mir: &MirBody, place: &Place) {
        match place {
            Place::Local(id) => {
                self.output.push_str(local_name(&self.local_names, *id));
            }
            // A global is referenced by its bare C symbol name.
            Place::Global(name) => self.output.push_str(name),
            // Arrays are wrapper structs, so indexing reaches through `data`. A
            // value array uses `.data[i]`; a reference or pointer to an array
            // uses `->data[i]`; a raw pointer indexes directly. The decision is
            // driven by the base place's type (`place_type`).
            Place::Index(base, index) => {
                self.gen_place(mir, base);
                match self.index_access(mir, base) {
                    IndexAccess::ArrayValue => self.output.push_str(".data["),
                    IndexAccess::ArrayPointer => self.output.push_str("->data["),
                    IndexAccess::Direct => self.output.push('['),
                }
                self.gen_operand(mir, index);
                self.output.push(']');
            }
            // A field through a reference or pointer auto-derefs to `->`; a
            // field on a value uses `.`.
            Place::Field(base, name) => {
                self.gen_place(mir, base);
                if self.place_is_pointer_like(mir, base) {
                    self.w(format_args!("->{}", name));
                } else {
                    self.w(format_args!(".{}", name));
                }
            }
            Place::Deref(base) => {
                self.output.push_str("(*");
                self.gen_place(mir, base);
                self.output.push(')');
            }
        }
    }

    /// Emit each top-level global as a file-scope C static. A `let` global is
    /// `static const` (read-only); a `mut` global is `static` (mutable). The
    /// initializer is the const-folded scalar value (C requires a constant static
    /// initializer at the floor). A poisoned global (failed fold) was already
    /// diagnosed and halts compilation, so it is skipped here.
    fn gen_globals(&mut self) {
        let mut any = false;
        for (_, g) in self.hir.globals.iter() {
            let Some(value) = &g.value else { continue };
            let qual = if g.mutable { "static" } else { "static const" };
            let types = self.hir.types.borrow();
            self.output
                .write_fmt(format_args!("{} {} {} = ", qual, CType::new(g.ty, &types), g.name))
                .expect("writing to String cannot fail");
            drop(types);
            match value {
                ConstValue::Int(v) => {
                    self.output.push_str(itoa::Buffer::new().format(*v));
                }
                // ryu formatting: always emits a decimal point so C reads it as
                // `double`, faster than Rust's Debug formatter.
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

    /// Emit one NUL-terminated `uint8_t[]` static per unique string literal.
    /// The literal's source text is decoded first (`decode_string_literal`:
    /// escapes expanded to their real bytes), so `N` = decoded byte count,
    /// matching the `&[uint8; N]` type the literal carries. The NUL at index
    /// `N` lives in the static but outside the wrapper's `data[N]`, so a byte
    /// pointer (`->data`) read to the NUL is in-bounds (the storage is `N + 1`).
    fn gen_string_statics(&mut self) {
        if self.strings.is_empty() {
            return;
        }
        let string_data: Vec<(usize, Vec<u8>)> = self
            .strings
            .iter()
            .enumerate()
            .map(|(id, s)| (id, hir::core::decode_string_literal(s)))
            .collect();
        for (id, bytes) in &string_data {
            self.w(format_args!(
                "static const uint8_t __eye_str{}[{}] = {{",
                id,
                bytes.len() + 1
            ));
            for b in bytes {
                self.w(format_args!("{},", b));
            }
            self.output.push_str("0};\n");
        }
        self.output.push('\n');
    }

    /// The C id of a string literal's backing static (its index in the pool).
    // A4: unwrap_or(0) silently returns wrong static if a string is absent
    // from string_index. collect_strings runs first and always populates all
    // strings, so this never fires on correct data. Use expect() for defense.
    fn string_id(&self, s: &Text) -> usize {
        self.string_index.get(s).copied().expect("string literal in string_index")
    }

    fn gen_literal(&mut self, lit: &Literal) {
        match lit {
            Literal::Int(v) => self.output.push_str(itoa::Buffer::new().format(*v)),
            Literal::Float(s) => self.output.push_str(s.as_str()),
            // A string literal is `&[uint8; N]`: a wrapper pointer into its
            // file-scope byte static. The static is `uint8_t[]`, layout-identical
            // to the wrapper's first member, so the cast lets `s->data[i]`/`len`
            // work. The print intrinsic emits the byte form (`%s`) separately.
            Literal::String(s) => {
                let n = hir::core::decode_string_literal(s).len() as u64;
                let mut types = self.hir.types.borrow_mut();
                let uint8_ty = types.intern(TypeKind::Path(Text::from("uint8")));
                let wrapper = array_wrapper_name(uint8_ty, n, &types);
                drop(types);
                self.w(format_args!("({}*)__eye_str{}", wrapper, self.string_id(s)));
            }
            Literal::Bool(b) => self.output.push_str(if *b { "true" } else { "false" }),
            Literal::Char(c) => write_c_char_literal(*c, &mut self.output),
        }
    }

    /// The `println` intrinsic, lowered to `printf` with a per-argument specifier
    /// substituted for each `{}` and a trailing `\n` appended automatically.
    fn gen_println(&mut self, mir: &MirBody, args: &[Operand]) {
        self.output.push_str("printf(");
        let Some((fmt, values)) = args.split_first() else {
            self.output.push(')');
            return;
        };

        let Operand::Const(Literal::String(s)) = fmt else {
            // Non-literal format string: forward operands unchanged.
            self.gen_operand(mir, fmt);
            for v in values {
                self.output.push_str(", ");
                self.gen_operand(mir, v);
            }
            self.output.push(')');
            return;
        };

        let mut value_iter = values.iter();
        let mut rendered = String::with_capacity(s.len() + values.len() * 2);
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' && chars.peek() == Some(&'}') {
                // U5: placeholder count vs argument count unchecked.
                // Exhausted value_iter silently emits %d, extra values
                // are forwarded to printf. Fix independently of type
                // inference.
                chars.next();
                let spec = value_iter
                    .next()
                    .map(|op| self.operand_spec(mir, op))
                    .unwrap_or("%d");
                rendered.push_str(spec);
            } else if c == '%' {
                rendered.push_str("%%");
            } else {
                rendered.push(c);
            }
        }
        self.w(format_args!("\"{}\\n\"", rendered));
        for v in values {
            self.output.push_str(", ");
            self.gen_println_value(mir, v);
        }
        self.output.push(')');
    }

    /// Emit a `println` value argument. A string (`&[uint8; N]`) prints with `%s`,
    /// which needs a `char*` byte pointer, not the wrapper pointer (`%p`): a
    /// string literal is its raw C string (NUL-terminated by C); a string place
    /// dereferences to its byte array (`->data`, NUL-terminated by the static).
    /// Every other value emits normally.
    fn gen_println_value(&mut self, mir: &MirBody, op: &Operand) {
        match op {
            Operand::Const(Literal::String(s)) => self.w(format_args!("\"{}\"", s)),
            Operand::Copy(place) => {
                let (is_str, is_pointer) = {
                    let types = self.hir.types.borrow();
                    let ty = self.place_type(mir, place);
                    // Everything `spec_for_type` formats as `%p` except `ptr`
                    // itself, which is already `void*` and needs no cast.
                    (
                        is_byte_string(ty, &types),
                        matches!(
                            types.lookup(ty),
                            TypeKind::Ref(_) | TypeKind::Ptr(_) | TypeKind::Fn { .. }
                        ),
                    )
                };
                if is_str {
                    self.gen_place(mir, place);
                    self.output.push_str("->data");
                } else if is_pointer {
                    // `%p` requires a `void*` argument; any other pointer type
                    // passed through varargs is formally undefined.
                    self.output.push_str("(void*)");
                    self.gen_operand(mir, op);
                } else {
                    self.gen_operand(mir, op);
                }
            }
            _ => self.gen_operand(mir, op),
        }
    }

    fn operand_spec(&mut self, mir: &MirBody, op: &Operand) -> &'static str {
        match op {
            Operand::Const(Literal::Int(_)) => "%d",
            Operand::Const(Literal::Float(_)) => "%f",
            Operand::Const(Literal::String(_)) => "%s",
            Operand::Const(Literal::Bool(_)) => "%d",
            Operand::Const(Literal::Char(_)) => "%c",
            Operand::Copy(place) => {
                let types = self.hir.types.borrow();
                spec_for_type(self.place_type(mir, place), &types)
            }
        }
    }

    /// Recover the type of a place from `MirLocal.ty` plus the HIR struct/union
    /// definitions. Total (REDESIGN I2): it always returns a [`Type`], never
    /// rejects. A projection whose type cannot be resolved (only reachable on a
    /// malformed input the front end would already have diagnosed) falls back to
    /// [`TypeRef::Error`], which the callers (the `.`/`->` and `.data[]`
    /// decisions, the printf specifier) handle without panicking.
    // EXPERIMENTAL(A2): Memoized. The cache is checked on entry and populated
    // on each return. Repeated calls (index_access, place_is_pointer_like,
    // specifier resolution) for the same place are O(1) after the first walk.
    fn place_type(&mut self, mir: &MirBody, place: &Place) -> Type {
        if let Some(&ty) = self.place_types.get(place) {
            return ty;
        }
        let ty = match place {
            Place::Local(id) => mir.locals[*id].ty,
            // A global's type comes from its HIR declaration.
            Place::Global(name) => match self.hir.items.globals.get(name) {
                Some(&id) => self.hir.globals[id].ty,
                None => self.error_ty,
            },
            Place::Field(base, name) => self.field_type(mir, base, name),
            // `a[i]` has the element type of an array base, or the pointee of a
            // reference/raw-pointer base.
            Place::Index(base, _) => {
                let base_ty = self.place_type(mir, base);
                let types = self.hir.types.borrow();
                match types.lookup(base_ty) {
                    TypeKind::Array { elem, .. } => *elem,
                    TypeKind::Ref(inner) | TypeKind::Ptr(inner) => match types.lookup(*inner) {
                        TypeKind::Array { elem, .. } => *elem,
                        _ => *inner,
                    },
                    _ => base_ty,
                }
            }
            // `*p` has the pointee type.
            Place::Deref(base) => {
                let base_ty = self.place_type(mir, base);
                let types = self.hir.types.borrow();
                match types.lookup(base_ty) {
                    TypeKind::Ref(inner) | TypeKind::Ptr(inner) => *inner,
                    _ => base_ty,
                }
            }
        };
        self.place_types.insert(place.clone(), ty);
        ty
    }

    fn index_access(&mut self, mir: &MirBody, place: &Place) -> IndexAccess {
        let ty = self.place_type(mir, place);
        let types = self.hir.types.borrow();
        match types.lookup(ty) {
            TypeKind::Array { .. } => IndexAccess::ArrayValue,
            TypeKind::Ref(inner) | TypeKind::Ptr(inner)
                if matches!(types.lookup(*inner), TypeKind::Array { .. }) =>
            {
                IndexAccess::ArrayPointer
            }
            _ => IndexAccess::Direct,
        }
    }

    fn place_is_pointer_like(&mut self, mir: &MirBody, place: &Place) -> bool {
        let ty = self.place_type(mir, place);
        let types = self.hir.types.borrow();
        matches!(types.lookup(ty), TypeKind::Ref(_) | TypeKind::Ptr(_))
    }

    /// The declared type of field `name` on the (possibly reference/pointer)
    /// struct or union that `base` resolves to. Structs and unions share the
    /// field arena, so a union member resolves the same way.
    fn field_type(&mut self, mir: &MirBody, base: &Place, name: &hir::core::Text) -> Type {
        let base_ty = self.place_type(mir, base);
        let types = self.hir.types.borrow();
        let struct_name = match types.lookup(base_ty) {
            TypeKind::Path(n) => n.clone(),
            TypeKind::Ref(inner) | TypeKind::Ptr(inner) => match types.lookup(*inner) {
                TypeKind::Path(n) => n.clone(),
                _ => return self.error_ty,
            },
            _ => return self.error_ty,
        };
        drop(types);
        let field_id = self
            .hir
            .items
            .structs
            .get(&struct_name)
            .and_then(|&id| self.hir.structs[id].field_index.get(name).copied())
            .or_else(|| {
                self.hir
                    .items
                    .unions
                    .get(&struct_name)
                    .and_then(|&id| self.hir.unions[id].field_index.get(name).copied())
            });
        match field_id {
            Some(id) => self.hir.fields[id].ty,
            None => self.error_ty,
        }
    }

    /// The C name for a local. Parameters keep their bare source name (the
    /// function signature declares them by that name). Every other local - a
    /// `let` binding or a generated temp - is suffixed with its [`LocalId`], so
    /// two same-named `let`s in one C scope (same-block shadowing) cannot
    /// collide into a redeclaration. MIR's locals arena gives each a unique id;
    /// suffixing surfaces it. This is output-invariant (it only renames), so it
    /// is safe to do now and closes the totality hole before cutover.
    fn local_names(mir: &MirBody) -> Vec<Text> {
        let mut names: Vec<Text> = Vec::with_capacity(mir.locals.len());
        for (id, local) in mir.locals.iter() {
            let raw = u32::from(id.into_raw());
            let idx = raw as usize;
            if names.len() <= idx {
                names.resize(idx + 1, Text::from(""));
            }
            names[idx] = if mir.params.contains(&id) {
                // A parameter always has a source name; the signature uses it bare.
                local
                    .name
                    .as_ref()
                    .expect("parameter has a source name")
                    .clone()
            } else {
                match &local.name {
                    Some(name) => Text::from(format!("{}_{}", name, raw)),
                    None => Text::from(format!("_t{}", raw)),
                }
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
    let raw = u32::from(id.into_raw()) as usize;
    names.get(raw).expect("MIR local name was precomputed")
}
