//! pass 1: collect top-level items into [`HIR::items`].

use ast::AstNode;
use diagnostics::Span;
use rustc_hash::{FxBuildHasher, FxHashMap};
use smallvec::SmallVec;
use smol_str::SmolStr;
use syntax::{StringTable, SyntaxKind, SyntaxNode, SyntaxNodePtr, SyntaxToken};

use super::const_eval::ConstEnv;
use super::types::lower_type_ref;
use crate::core::{
    Const, ConstId, ConstValue, Enum, Field, FnId, Function, Global, GlobalId, HIR, HirError,
    OpaqueType, Param, ResolveError, Struct, Text, TypeError, TypeKind, TypeRef, Union, Variant,
};

/// lower a declared type and record it - with the type node's span - into
/// `typed_decls` for [`validate_type_names`]. item signatures may forward-
/// reference items collected later (`structure A { B b, }` before `B`, an
/// extern fn before its `type FILE;`), so name validation cannot run inline;
/// it runs once after every item is collected.
fn lower_recorded_type(
    hir: &mut HIR,
    t: &ast::TypeRef,
    consts: &dyn ConstEnv,
    typed_decls: &mut Vec<(Span, TypeRef)>,
) -> TypeRef {
    let ty = lower_type_ref(t, &mut hir.diagnostics, consts, &mut hir.types);
    typed_decls.push((Span::from(SyntaxNodePtr::new(t.syntax())), ty));
    ty
}

/// pass 1.6 (CLEAK L6, R012): every `Path` name in a collected item
/// signature - field, parameter, return, global, and const types - must name
/// a declared type (primitive, struct, union, enum, or opaque extern type).
/// without this an undeclared name is emitted verbatim into c and clang
/// reports "unknown type name". runs after all items are collected so forward
/// references resolve.
pub(super) fn validate_type_names(hir: &mut HIR, typed_decls: &[(Span, TypeRef)]) {
    for (span, ty) in typed_decls {
        for name in super::types::unknown_type_names(*ty, &hir.types, hir) {
            hir.diagnostics.emit(
                span.clone(),
                HirError::Resolve(ResolveError::UnknownTypeName { name }),
            );
        }
    }
}

fn text(token: Option<SyntaxToken>, interner: &dyn StringTable) -> Text {
    token
        .map(|t| {
            interner
                .get(t.text())
                .unwrap_or_else(|| SmolStr::from(t.text()))
        })
        .unwrap_or_default()
}

/// pass 1a: collect top-level `const` items into the const arena and item scope,
/// *before* [`collect_items`], so a later item's array length (`[T; N]`) can
/// resolve `N` to a const value once pass 1.5 ([`super::const_eval`]) folds it.
/// values are filled in by the evaluator; this pass only records names, types,
/// and the AST body to fold. returns the AST nodes so the evaluator can walk
/// each initializer without re-traversing the file.
pub(super) fn collect_consts(
    hir: &mut HIR,
    file: &ast::SourceFile,
    interner: &dyn StringTable,
    typed_decls: &mut Vec<(Span, TypeRef)>,
) -> Vec<(ConstId, ast::ConstDef)> {
    let mut const_asts = Vec::new();
    for item in file.items() {
        let ast::Item::ConstDef(c) = item else {
            continue;
        };
        let name: Text = text(c.name(), interner);
        // const values are not folded yet, so a const-as-array-length in a
        // const's *own* type cannot be resolved here. aggregate const values
        // are deferred anyway (scalar-only floor), so an empty map is correct.
        let ty = match c.ty() {
            Some(t) => lower_recorded_type(
                hir,
                &t,
                &FxHashMap::with_capacity_and_hasher(0, FxBuildHasher),
                typed_decls,
            ),
            None => hir.types.error_type(),
        };
        let const_id = hir.consts.alloc(Const {
            name: name.clone(),
            ty,
            value: None,
        });
        // only other consts exist in scope at this point (this pass runs first);
        // a clash with a struct/fn/enum is caught when that item is collected.
        if hir.items.consts.contains_key(&name) {
            hir.diagnostics.emit(
                name_span(c.name(), c.syntax()),
                HirError::Resolve(ResolveError::DuplicateItem { name: name.clone() }),
            );
        }
        hir.items.consts.insert(name, const_id);
        const_asts.push((const_id, c));
    }
    const_asts
}

/// pass 1b: collect top-level `let`/`mut` globals (addressable static storage).
/// runs after consts are folded, so a global's type (`[T; N]`) may reference a
/// const length. values are folded by [`super::const_eval::eval_globals`]; this
/// pass records name, type, and mutability, and returns the AST initializers.
pub(super) fn collect_globals(
    hir: &mut HIR,
    file: &ast::SourceFile,
    const_values: &FxHashMap<Text, ConstValue>,
    interner: &dyn StringTable,
    typed_decls: &mut Vec<(Span, TypeRef)>,
) -> Vec<(GlobalId, ast::GlobalDef)> {
    let mut global_asts = Vec::new();
    for item in file.items() {
        let ast::Item::GlobalDef(g) = item else {
            continue;
        };
        let name: Text = text(g.name(), interner);
        check_c_keyword(hir, &name, "global", name_span(g.name(), g.syntax()));
        check_reserved_file_scope(hir, &name, "global", name_span(g.name(), g.syntax()));
        let ty = match g.ty() {
            Some(t) => lower_recorded_type(hir, &t, const_values, typed_decls),
            None => hir.types.error_type(),
        };
        let mutable = matches!(g.kind(), Some(ast::LetKind::Mut));
        let global_id = hir.globals.alloc(Global {
            name: name.clone(),
            ty,
            mutable,
            value: None,
        });
        // a global sharing a name with an already-collected const/global is a
        // duplicate item (other namespaces are checked in `collect_items`).
        if hir.items.consts.contains_key(&name) || hir.items.globals.contains_key(&name) {
            hir.diagnostics.emit(
                name_span(g.name(), g.syntax()),
                HirError::Resolve(ResolveError::DuplicateItem { name: name.clone() }),
            );
        }
        hir.items.globals.insert(name, global_id);
        global_asts.push((global_id, g));
    }
    global_asts
}

/// whether `name` is a c keyword (C11 plus the C23 additions). the c backend
/// emits item names, field names, parameter names, and enum variants verbatim,
/// so any of them being a c keyword produces illegal c (`.struct = ...`).
/// names that are also eye keywords (`if`, `return`, `union`, ...) never get
/// here - the parser rejects them - but they are kept in the list as defense
/// in depth. the `_X`-style spellings (`_Bool`, `_Atomic`, ...) are omitted:
/// a leading underscore followed by an uppercase letter is reserved c the
/// user would have to spell deliberately.
fn is_c_keyword(name: &str) -> bool {
    matches!(
        name,
        "auto"
            | "bool"
            | "break"
            | "case"
            | "char"
            | "const"
            | "constexpr"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "inline"
            | "int"
            | "long"
            | "nullptr"
            | "register"
            | "restrict"
            | "return"
            | "short"
            | "signed"
            | "sizeof"
            | "static"
            | "static_assert"
            | "struct"
            | "switch"
            | "thread_local"
            | "true"
            | "typedef"
            | "typeof"
            | "typeof_unqual"
            | "union"
            | "unsigned"
            | "void"
            | "volatile"
            | "while"
    )
}

/// reject a declared name the backend will emit verbatim when it is a c
/// keyword, or when it starts with `__eye` - the backend's own symbol
/// namespace (string statics, array-wrapper typedefs, the `main` shim).
/// `what` names the declaration kind for the message.
fn check_c_keyword(hir: &mut HIR, name: &Text, what: &'static str, span: Span) {
    if is_c_keyword(name) {
        hir.diagnostics.emit(
            span,
            HirError::Resolve(ResolveError::NameIsCKeyword {
                name: name.clone(),
                what,
            }),
        );
    } else if name.starts_with("__eye") {
        hir.diagnostics.emit(
            span,
            HirError::Resolve(ResolveError::NameIsReserved {
                name: name.clone(),
                what,
            }),
        );
    }
}

/// reject `printf` for a name emitted at c file scope in the ordinary
/// namespace (function, global, type typedef, enum variant): the `println`
/// intrinsic lowers to libc `printf`, so a user definition collides with the
/// emitted prototype and shadows the libc symbol with an incompatible
/// signature. an `extern` declaration of `printf` stays legal - it names the
/// same libc symbol (and suppresses the emitted prototype in its favor).
fn check_reserved_file_scope(hir: &mut HIR, name: &Text, what: &'static str, span: Span) {
    if name == "printf" {
        hir.diagnostics.emit(
            span,
            HirError::Resolve(ResolveError::NameIsReserved {
                name: name.clone(),
                what,
            }),
        );
    }
}

/// anchor a diagnostic on an item's name token when present, falling back to the
/// whole item node. a name conflict is about the name, so the underline should
/// sit on it rather than the entire declaration.
fn name_span(name: Option<SyntaxToken>, fallback: &SyntaxNode) -> Span {
    name.map(|t| Span::from(t.text_range()))
        .unwrap_or_else(|| Span::from(SyntaxNodePtr::new(fallback)))
}

/// the contextual effect annotations preceding a fn name (`io render(...)`),
/// as `(name, span)` in source order. the parser nests them in an `EffectList`
/// node; each ident token is one effect. validation against the atom set is the
/// EFFECT crate's job (EFFECT.md), so this only interns names and their spans.
fn collect_effect_annotations(
    f: &ast::FnDef,
    interner: &dyn StringTable,
) -> Vec<(Text, Span)> {
    let Some(list) = f
        .syntax()
        .children()
        .find(|n| n.kind() == SyntaxKind::EffectList)
    else {
        return Vec::new();
    };
    list.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::Ident)
        .map(|t| (text(Some(t.clone()), interner), Span::from(t.text_range())))
        .collect()
}

/// walk top-level items, allocate signatures, populate [`ItemScope`].
/// returns the AST nodes for each function so pass 3 can lower their bodies
/// without re-traversing the file. emits a diagnostic on duplicate names
/// (later definitions still take effect; the original slot stays allocated
/// but is shadowed in the scope map).
pub(super) fn collect_items(
    hir: &mut HIR,
    file: &ast::SourceFile,
    const_values: &FxHashMap<Text, ConstValue>,
    interner: &dyn StringTable,
    typed_decls: &mut Vec<(Span, TypeRef)>,
) -> Vec<(FnId, ast::FnDef)> {
    let mut fn_asts = Vec::new();
    for item in file.items() {
        match item {
            // collected in pass 1a (`collect_consts`), before this pass, so an
            // array length here can already reference a const value.
            ast::Item::ConstDef(_) => {}
            // collected in pass 1b (`collect_globals`), after const folding.
            ast::Item::GlobalDef(_) => {}
            ast::Item::StructDef(s) => {
                let name: Text = text(s.name(), interner);
                check_c_keyword(hir, &name, "struct", name_span(s.name(), s.syntax()));
                check_reserved_file_scope(hir, &name, "struct", name_span(s.name(), s.syntax()));
                let field_count = s.field_list().map(|fl| fl.fields().count()).unwrap_or(0);
                let mut fields = SmallVec::new();
                let mut field_index =
                    FxHashMap::with_capacity_and_hasher(field_count, FxBuildHasher);
                if let Some(fl) = s.field_list() {
                    for f in fl.fields() {
                        let fname: Text = text(f.name(), interner);
                        check_c_keyword(hir, &fname, "field", name_span(f.name(), f.syntax()));
                        let ty = match f.ty() {
                            Some(t) => lower_recorded_type(hir, &t, const_values, typed_decls),
                            None => hir.types.error_type(),
                        };
                        let field_id = hir.fields.alloc(Field { name: fname, ty });
                        if !field_index.contains_key(&hir.fields[field_id].name) {
                            field_index.insert(hir.fields[field_id].name.clone(), field_id);
                        }
                        fields.push(field_id);
                    }
                }
                let struct_id = hir.structs.alloc(Struct {
                    name: name.clone(),
                    fields,
                    field_index,
                });
                if hir.items.structs.contains_key(&name)
                    || hir.items.functions.contains_key(&name)
                    || hir.items.consts.contains_key(&name)
                {
                    hir.diagnostics.emit(
                        name_span(s.name(), s.syntax()),
                        HirError::Resolve(ResolveError::DuplicateItem { name: name.clone() }),
                    );
                }
                hir.items.structs.insert(name, struct_id);
            }
            ast::Item::FnDef(f) => {
                let name: Text = text(f.name(), interner);
                check_c_keyword(hir, &name, "function", name_span(f.name(), f.syntax()));
                check_reserved_file_scope(hir, &name, "function", name_span(f.name(), f.syntax()));
                let mut params = SmallVec::new();
                if let Some(pl) = f.param_list() {
                    for param_ast in pl.params() {
                        let pname = text(param_ast.name(), interner);
                        check_c_keyword(
                            hir,
                            &pname,
                            "parameter",
                            name_span(param_ast.name(), param_ast.syntax()),
                        );
                        // a definition emits the names into the c signature,
                        // where a duplicate is a redefinition error (extern
                        // prototypes are types-only and skip this).
                        if params.iter().any(|p: &Param| p.name == pname) {
                            hir.diagnostics.emit(
                                name_span(param_ast.name(), param_ast.syntax()),
                                HirError::Resolve(ResolveError::DuplicateParam {
                                    name: pname.clone(),
                                    function: name.clone(),
                                }),
                            );
                        }
                        let pty = match param_ast.ty() {
                            Some(t) => lower_recorded_type(hir, &t, const_values, typed_decls),
                            None => hir.types.error_type(),
                        };
                        params.push(Param {
                            name: pname,
                            ty: pty,
                        });
                    }
                }
                let ret = f
                    .ret_type()
                    .map(|t| lower_recorded_type(hir, &t, const_values, typed_decls));
                // `main` is the program entry point. the c backend wraps it in
                // an `int main(void)` shim that calls it with no arguments and
                // adapts whatever it returns to the process exit code. any
                // return type is fine, but the shim has nothing to pass for a
                // parameter, so a parameterized `main` is rejected here rather
                // than emitting c that clang rejects (a call with too few args).
                if name == "main" && !params.is_empty() {
                    hir.diagnostics.emit(
                        name_span(f.name(), f.syntax()),
                        HirError::Type(TypeError::MainHasParams),
                    );
                }
                let fn_type = {
                    let p: &[Param] = &params;
                    let param_tys: Vec<TypeRef> = p.iter().map(|p| p.ty).collect();
                    Some(hir.types.intern(TypeKind::Fn {
                        params: param_tys,
                        ret,
                        variadic: false,
                    }))
                };
                let declared_effects = collect_effect_annotations(&f, interner);
                let fn_id = hir.functions.alloc(Function {
                    name: name.clone(),
                    params,
                    ret,
                    body: None,
                    is_extern: false,
                    variadic: false,
                    fn_type,
                    declared_effects,
                });
                if hir.items.functions.contains_key(&name)
                    || hir.items.structs.contains_key(&name)
                    || hir.items.consts.contains_key(&name)
                {
                    hir.diagnostics.emit(
                        name_span(f.name(), f.syntax()),
                        HirError::Resolve(ResolveError::DuplicateItem { name: name.clone() }),
                    );
                }
                hir.items.functions.insert(name, fn_id);
                fn_asts.push((fn_id, f));
            }
            // a union mirrors struct collection exactly - same field list,
            // separate arena + scope namespace.
            ast::Item::UnionDef(u) => {
                let name: Text = text(u.name(), interner);
                check_c_keyword(hir, &name, "union", name_span(u.name(), u.syntax()));
                check_reserved_file_scope(hir, &name, "union", name_span(u.name(), u.syntax()));
                let field_count = u.field_list().map(|fl| fl.fields().count()).unwrap_or(0);
                let mut fields = SmallVec::new();
                let mut field_index =
                    FxHashMap::with_capacity_and_hasher(field_count, FxBuildHasher);
                if let Some(fl) = u.field_list() {
                    for f in fl.fields() {
                        let fname: Text = text(f.name(), interner);
                        check_c_keyword(hir, &fname, "field", name_span(f.name(), f.syntax()));
                        let ty = match f.ty() {
                            Some(t) => lower_recorded_type(hir, &t, const_values, typed_decls),
                            None => hir.types.error_type(),
                        };
                        let field_id = hir.fields.alloc(Field { name: fname, ty });
                        if !field_index.contains_key(&hir.fields[field_id].name) {
                            field_index.insert(hir.fields[field_id].name.clone(), field_id);
                        }
                        fields.push(field_id);
                    }
                }
                let union_id = hir.unions.alloc(Union {
                    name: name.clone(),
                    fields,
                    field_index,
                });
                if hir.items.structs.contains_key(&name)
                    || hir.items.unions.contains_key(&name)
                    || hir.items.functions.contains_key(&name)
                    || hir.items.consts.contains_key(&name)
                {
                    hir.diagnostics.emit(
                        name_span(u.name(), u.syntax()),
                        HirError::Resolve(ResolveError::DuplicateItem { name: name.clone() }),
                    );
                }
                hir.items.unions.insert(name, union_id);
            }
            // each signature in an `extern` block becomes a bodyless
            // [`Function`] flagged `is_extern`, registered in the fn namespace
            // so calls resolve. no AST body, so nothing is pushed to `fn_asts`.
            // a `type Name;` declaration becomes an [`OpaqueType`], registered
            // in its own namespace so `Name*` type refs name a real item.
            ast::Item::ExternBlock(eb) => {
                for item in eb.items() {
                    let ef = match item {
                        ast::ExternItem::ExternFn(ef) => ef,
                        ast::ExternItem::ExternTypeDef(et) => {
                            let name: Text = text(et.name(), interner);
                            check_c_keyword(hir, &name, "type", name_span(et.name(), et.syntax()));
                            let opaque_id = hir.opaques.alloc(OpaqueType { name: name.clone() });
                            if hir.items.opaques.contains_key(&name)
                                || hir.items.structs.contains_key(&name)
                                || hir.items.unions.contains_key(&name)
                                || hir.items.enums.contains_key(&name)
                            {
                                hir.diagnostics.emit(
                                    name_span(et.name(), et.syntax()),
                                    HirError::Resolve(ResolveError::DuplicateItem {
                                        name: name.clone(),
                                    }),
                                );
                            }
                            hir.items.opaques.insert(name, opaque_id);
                            continue;
                        }
                    };
                    let name: Text = text(ef.name(), interner);
                    // no c symbol can be a keyword, so an extern keyword name
                    // could never link; reject it like a defined function's.
                    // extern *parameter* names are not checked: the emitted
                    // prototype is types-only, so they never reach the c.
                    check_c_keyword(hir, &name, "function", name_span(ef.name(), ef.syntax()));
                    let mut params = SmallVec::new();
                    let mut variadic = false;
                    if let Some(pl) = ef.param_list() {
                        for param_ast in pl.params() {
                            let pname = text(param_ast.name(), interner);
                            let pty = match param_ast.ty() {
                                Some(t) => lower_recorded_type(hir, &t, const_values, typed_decls),
                                None => hir.types.error_type(),
                            };
                            params.push(Param {
                                name: pname,
                                ty: pty,
                            });
                        }
                        variadic = pl.variadic().is_some();
                    }
                    let ret = ef
                        .ret_type()
                        .map(|t| lower_recorded_type(hir, &t, const_values, typed_decls));
                    let fn_type = {
                        let p: &[Param] = &params;
                        let param_tys: Vec<TypeRef> = p.iter().map(|p| p.ty).collect();
                        Some(hir.types.intern(TypeKind::Fn {
                            params: param_tys,
                            ret,
                            variadic,
                        }))
                    };
                    let fn_id = hir.functions.alloc(Function {
                        name: name.clone(),
                        params,
                        ret,
                        body: None,
                        is_extern: true,
                        variadic,
                        fn_type,
                        // extern signatures carry no effect annotations (an
                        // extern call is always `ffi` by construction).
                        declared_effects: Vec::new(),
                    });
                    if hir.items.functions.contains_key(&name)
                        || hir.items.structs.contains_key(&name)
                        || hir.items.consts.contains_key(&name)
                    {
                        hir.diagnostics.emit(
                            name_span(ef.name(), ef.syntax()),
                            HirError::Resolve(ResolveError::DuplicateItem { name: name.clone() }),
                        );
                    }
                    hir.items.functions.insert(name, fn_id);
                }
            }
            ast::Item::EnumDef(e) => {
                let name: Text = text(e.name(), interner);
                check_c_keyword(hir, &name, "enum", name_span(e.name(), e.syntax()));
                check_reserved_file_scope(hir, &name, "enum", name_span(e.name(), e.syntax()));
                let variant_count = e.variants().count();
                let mut variants = SmallVec::new();
                let mut variant_index =
                    FxHashMap::with_capacity_and_hasher(variant_count, FxBuildHasher);
                for v in e.variants() {
                    let vname = text(v.name(), interner);
                    check_c_keyword(hir, &vname, "enum variant", name_span(v.name(), v.syntax()));
                    check_reserved_file_scope(
                        hir,
                        &vname,
                        "enum variant",
                        name_span(v.name(), v.syntax()),
                    );
                    if !variant_index.contains_key(&vname) {
                        variant_index.insert(vname.clone(), variants.len() as u32);
                    }
                    variants.push(Variant { name: vname });
                }
                let enum_id = hir.enums.alloc(Enum {
                    name: name.clone(),
                    variants,
                    variant_index,
                });
                if hir.items.structs.contains_key(&name)
                    || hir.items.functions.contains_key(&name)
                    || hir.items.consts.contains_key(&name)
                {
                    hir.diagnostics.emit(
                        name_span(e.name(), e.syntax()),
                        HirError::Resolve(ResolveError::DuplicateItem { name: name.clone() }),
                    );
                }
                // register each variant in the flat index. a second enum
                // claiming the same variant name conflicts with the first
                // and is a hard error (the lookup would otherwise be
                // ambiguous, and the c backend would emit two enum
                // constants with the same name).
                // parallel to the lowered variants (built in source order above),
                // so index `idx` recovers the ast variant to anchor a conflict
                // on its name rather than the whole enum.
                let variant_asts: Vec<_> = e.variants().collect();
                let enum_def = &hir.enums[enum_id];
                for (idx, v) in enum_def.variants.iter().enumerate() {
                    let vname = v.name.clone();
                    if let Some(&(other_enum, _)) = hir.items.variants.get(&vname) {
                        let other_name = hir.enums[other_enum].name.clone();
                        let anchor =
                            name_span(variant_asts.get(idx).and_then(|va| va.name()), e.syntax());
                        hir.diagnostics.emit(
                            anchor,
                            HirError::Resolve(ResolveError::DuplicateVariantDecl {
                                variant: vname.clone(),
                                enum_name: other_name.clone(),
                            }),
                        );
                    } else {
                        hir.items.variants.insert(vname, (enum_id, idx as u32));
                    }
                }
                hir.items.enums.insert(name, enum_id);
            }
        }
    }
    fn_asts
}
