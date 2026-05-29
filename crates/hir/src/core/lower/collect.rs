//! Pass 1: collect top-level items into [`HIR::items`].

use ast::AstNode;
use diagnostics::{Sink, Span};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use smol_str::SmolStr;
use syntax::{SyntaxNode, SyntaxNodePtr, SyntaxToken};

use super::types::lower_type_ref;
use crate::core::{
    Enum, Field, FnId, Function, HIR, HirError, Param, ResolveError, Struct, Text, TypeRef, Union,
    UnsupportedError, Variant,
};

fn text(token: Option<SyntaxToken>) -> Text {
    token.map(|t| SmolStr::from(t.text())).unwrap_or_default()
}

/// Anchor a diagnostic on an item's name token when present, falling back to the
/// whole item node. A name conflict is about the name, so the underline should
/// sit on it rather than the entire declaration.
fn name_span(name: Option<SyntaxToken>, fallback: &SyntaxNode) -> Span {
    name.map(|t| Span::from(t.text_range()))
        .unwrap_or_else(|| Span::from(SyntaxNodePtr::new(fallback)))
}

/// Arrays as struct/union fields need the array's wrapper typedef emitted
/// before the containing struct, which the current codegen ordering (arrays
/// after nominal types) does not provide. Reject them with a clear diagnostic
/// until codegen gains a type-dependency topo-sort, and degrade the field type
/// to `Error` so the unbuilt C never references an undeclared wrapper. The
/// diagnostic anchors on the field's type node (the offending `[T; N]`), or the
/// whole field when no type node is present.
fn reject_array_field(
    ty: TypeRef,
    ty_node: Option<&SyntaxNode>,
    field: &SyntaxNode,
    diagnostics: &mut Sink<HirError>,
) -> TypeRef {
    if matches!(ty, TypeRef::Array { .. }) {
        let anchor = ty_node
            .map(|n| Span::from(SyntaxNodePtr::new(n)))
            .unwrap_or_else(|| Span::from(SyntaxNodePtr::new(field)));
        diagnostics.emit(anchor, HirError::Unsupported(UnsupportedError::ArrayField));
        return TypeRef::Error;
    }
    ty
}

/// Walk top-level items, allocate signatures, populate [`ItemScope`].
/// Returns the AST nodes for each function so pass 3 can lower their bodies
/// without re-traversing the file. Emits a diagnostic on duplicate names
/// (later definitions still take effect; the original slot stays allocated
/// but is shadowed in the scope map).
pub(super) fn collect_items(hir: &mut HIR, file: &ast::SourceFile) -> Vec<(FnId, ast::FnDef)> {
    let mut fn_asts = Vec::new();
    for item in file.items() {
        match item {
            ast::Item::StructDef(s) => {
                let name: Text = text(s.name());
                let mut fields = SmallVec::new();
                let mut field_index = FxHashMap::default();
                if let Some(fl) = s.field_list() {
                    for f in fl.fields() {
                        let fname: Text = text(f.name());
                        let ty_ast = f.ty();
                        let ty = match &ty_ast {
                            Some(t) => lower_type_ref(t, &mut hir.diagnostics),
                            None => TypeRef::Error,
                        };
                        let ty = reject_array_field(
                            ty,
                            ty_ast.as_ref().map(|t| t.syntax()),
                            f.syntax(),
                            &mut hir.diagnostics,
                        );
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
                if hir.items.structs.contains_key(&name) || hir.items.functions.contains_key(&name)
                {
                    hir.diagnostics.emit(
                        name_span(s.name(), s.syntax()),
                        HirError::Resolve(ResolveError::DuplicateItem { name: name.clone() }),
                    );
                }
                hir.items.structs.insert(name, struct_id);
            }
            ast::Item::FnDef(f) => {
                let name: Text = text(f.name());
                let mut params = SmallVec::new();
                if let Some(pl) = f.param_list() {
                    for param_ast in pl.params() {
                        let pname = text(param_ast.name());
                        let pty = match param_ast.ty() {
                            Some(t) => lower_type_ref(&t, &mut hir.diagnostics),
                            None => TypeRef::Error,
                        };
                        params.push(Param {
                            name: pname,
                            ty: pty,
                        });
                    }
                }
                let ret = f
                    .ret_type()
                    .map(|t| lower_type_ref(&t, &mut hir.diagnostics));
                let fn_id = hir.functions.alloc(Function {
                    name: name.clone(),
                    params,
                    ret,
                    body: None,
                    is_extern: false,
                });
                if hir.items.functions.contains_key(&name) || hir.items.structs.contains_key(&name)
                {
                    hir.diagnostics.emit(
                        name_span(f.name(), f.syntax()),
                        HirError::Resolve(ResolveError::DuplicateItem { name: name.clone() }),
                    );
                }
                hir.items.functions.insert(name, fn_id);
                fn_asts.push((fn_id, f));
            }
            // A union mirrors struct collection exactly - same field list,
            // separate arena + scope namespace.
            ast::Item::UnionDef(u) => {
                let name: Text = text(u.name());
                let mut fields = SmallVec::new();
                let mut field_index = FxHashMap::default();
                if let Some(fl) = u.field_list() {
                    for f in fl.fields() {
                        let fname: Text = text(f.name());
                        let ty_ast = f.ty();
                        let ty = match &ty_ast {
                            Some(t) => lower_type_ref(t, &mut hir.diagnostics),
                            None => TypeRef::Error,
                        };
                        let ty = reject_array_field(
                            ty,
                            ty_ast.as_ref().map(|t| t.syntax()),
                            f.syntax(),
                            &mut hir.diagnostics,
                        );
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
                {
                    hir.diagnostics.emit(
                        name_span(u.name(), u.syntax()),
                        HirError::Resolve(ResolveError::DuplicateItem { name: name.clone() }),
                    );
                }
                hir.items.unions.insert(name, union_id);
            }
            // Each signature in an `extern` block becomes a bodyless
            // [`Function`] flagged `is_extern`, registered in the fn namespace
            // so calls resolve. No AST body, so nothing is pushed to `fn_asts`.
            ast::Item::ExternBlock(eb) => {
                for ef in eb.fns() {
                    let name: Text = text(ef.name());
                    let mut params = SmallVec::new();
                    if let Some(pl) = ef.param_list() {
                        for param_ast in pl.params() {
                            let pname = text(param_ast.name());
                            let pty = match param_ast.ty() {
                                Some(t) => lower_type_ref(&t, &mut hir.diagnostics),
                                None => TypeRef::Error,
                            };
                            params.push(Param {
                                name: pname,
                                ty: pty,
                            });
                        }
                    }
                    let ret = ef
                        .ret_type()
                        .map(|t| lower_type_ref(&t, &mut hir.diagnostics));
                    let fn_id = hir.functions.alloc(Function {
                        name: name.clone(),
                        params,
                        ret,
                        body: None,
                        is_extern: true,
                    });
                    if hir.items.functions.contains_key(&name)
                        || hir.items.structs.contains_key(&name)
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
                let name: Text = text(e.name());
                let mut variants = SmallVec::new();
                let mut variant_index = FxHashMap::default();
                for v in e.variants() {
                    let vname = text(v.name());
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
                if hir.items.structs.contains_key(&name) || hir.items.functions.contains_key(&name)
                {
                    hir.diagnostics.emit(
                        SyntaxNodePtr::new(e.syntax()),
                        HirError::Resolve(ResolveError::DuplicateItem { name: name.clone() }),
                    );
                }
                // Register each variant in the flat index. A second enum
                // claiming the same variant name conflicts with the first
                // and is a hard error (the lookup would otherwise be
                // ambiguous, and the C backend would emit two enum
                // constants with the same name).
                // Parallel to the lowered variants (built in source order above),
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
