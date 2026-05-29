//! Fixed-array value semantics: the struct-wrap representation.
//!
//! C cannot pass, return, or assign a bare array by value - an array always
//! decays to a pointer at those boundaries. To give Eye arrays real value
//! semantics (copy on assign, pass/return by value, length carried in the
//! type), every `[T; N]` is lowered to a wrapper `struct { T data[N]; }`.
//! Copy, by-value passing, and multi-dimensional nesting then fall out of C
//! struct semantics for free.
//!
//! This is a C-backend representation detail, not an Eye language concept. A
//! future Cranelift backend would emit a stack slot and a memcpy instead, with
//! no wrapper type. The language never mentions `.data`; indexing and `&a[0]`
//! are rewritten onto it here.
//!
//! Wrapper names are derived purely from the type, so [`super::types::CType`]
//! can render an array as its wrapper name with no shared state. The collection
//! pass below only decides which typedefs to emit, and in what order (an
//! element type must be complete before the array that contains it, so nested
//! arrays are emitted innermost-first).

use super::CGen;
use super::types::CType;
use hir::core::TypeRef;
use rustc_hash::FxHashSet;

/// Mangle a type into a fragment of a C identifier. Injective: two distinct
/// Eye types never produce the same fragment, so they never collide on one
/// wrapper name (a collision would dedup two different element types to a
/// single typedef and miscompile one of them).
///
/// Injectivity rests on two rules:
/// - A `Path` name is length-prefixed (`ref_int` -> `7ref_int`). The map
///   `s -> len(s) ++ s` is injective because `n -> digits(n) + n` is strictly
///   increasing, so the prefix pins the name's extent unambiguously.
/// - The `ref_`/`ptr_`/`arr_`/`err` constructors all start with a letter, while
///   a length-prefixed `Path` always starts with a digit. So a user type named
///   `ref_int` (`7ref_int`) can never be confused with `&int` (`ref_3int`).
///   `arr_` puts the length first (`arr_3_5int32`) so the fragment parses
///   front-to-back: after `arr_`, the digit run is the length, then `_`, then
///   the element mangle.
fn array_mangle(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Path(name) => {
            let n = name.to_string();
            format!("{}{}", n.len(), n)
        }
        TypeRef::Ref(inner) => format!("ref_{}", array_mangle(inner)),
        TypeRef::Ptr(inner) => format!("ptr_{}", array_mangle(inner)),
        TypeRef::Array { elem, len } => format!("arr_{}_{}", len, array_mangle(elem)),
        TypeRef::Error => "err".to_string(),
    }
}

/// The C typedef name for the wrapper of `[elem; len]`. Equal to
/// `__eye_` ++ `array_mangle([elem; len])`, spelled inline to avoid a clone.
pub(super) fn array_wrapper_name(elem: &TypeRef, len: u64) -> String {
    format!("__eye_arr_{}_{}", len, array_mangle(elem))
}

/// Walk a type, registering every array node it contains. Innermost-first
/// (post-order) so a nested array's wrapper is emitted before the wrapper that
/// embeds it. Pushes `(elem, len)` for each distinct wrapper, deduped by name.
fn collect(ty: &TypeRef, seen: &mut FxHashSet<String>, ordered: &mut Vec<(TypeRef, u64)>) {
    match ty {
        TypeRef::Array { elem, len } => {
            collect(elem, seen, ordered);
            let name = array_wrapper_name(elem, *len);
            if seen.insert(name) {
                ordered.push((elem.as_ref().clone(), *len));
            }
        }
        TypeRef::Ref(inner) | TypeRef::Ptr(inner) => collect(inner, seen, ordered),
        TypeRef::Path(_) | TypeRef::Error => {}
    }
}

impl<'a> CGen<'a> {
    /// Emit a `typedef struct { T data[N]; }` for every distinct array type
    /// used anywhere in the program. Runs after structs/unions/enums (so an
    /// array of a struct sees the struct complete) and before functions.
    pub(super) fn gen_array_typedefs(&mut self) {
        let mut seen = FxHashSet::default();
        let mut ordered: Vec<(TypeRef, u64)> = Vec::new();

        // Struct and union field types (array fields are rejected in lowering,
        // but walking them is harmless and keeps this exhaustive).
        for (_id, field) in self.hir.fields.iter() {
            collect(&field.ty, &mut seen, &mut ordered);
        }
        // Function signatures.
        for (_id, r#fn) in self.hir.functions.iter() {
            for param in &r#fn.params {
                collect(&param.ty, &mut seen, &mut ordered);
            }
            if let Some(ret) = &r#fn.ret {
                collect(ret, &mut seen, &mut ordered);
            }
        }
        // Body-local types: declared local types, cast/struct-literal target
        // types, and every recovered expression type (covers array literals).
        for (_id, body) in self.hir.bodies.iter() {
            for (_lid, local) in body.locals.iter() {
                if let Some(ty) = &local.ty {
                    collect(ty, &mut seen, &mut ordered);
                }
            }
            for (_eid, expr) in body.exprs.iter() {
                match expr {
                    hir::core::Expr::Cast { ty, .. } | hir::core::Expr::StructLit { ty, .. } => {
                        collect(ty, &mut seen, &mut ordered);
                    }
                    _ => {}
                }
            }
            for (_sid, stmt) in body.stmts.iter() {
                if let hir::core::Stmt::Let { ty: Some(ty), .. } = stmt {
                    collect(ty, &mut seen, &mut ordered);
                }
            }
            for (_eid, ty) in body.expr_types.iter() {
                collect(ty, &mut seen, &mut ordered);
            }
        }

        if ordered.is_empty() {
            return;
        }
        self.output
            .push_str("// fixed-array value wrappers (struct-wrap representation)\n");
        for (elem, len) in ordered {
            emit!(
                self,
                "typedef struct {{ {} data[{}]; }} {};\n",
                CType::new(&elem),
                len,
                array_wrapper_name(&elem, len)
            );
        }
        self.output.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hir::core::Text;

    fn path(name: &str) -> TypeRef {
        TypeRef::Path(Text::from(name))
    }

    /// The mangle must be injective: a reference/pointer/array construction must
    /// never produce the same fragment as a user type literally named after the
    /// constructor prefix. Before length-prefixing, `&int` and `ref_int`, `*int`
    /// and `ptr_int`, `[int; 2]` and `arr_2_3int` all collided.
    #[test]
    fn mangle_is_injective_against_named_types() {
        let cases = [
            (
                TypeRef::Ref(Box::new(path("int"))), // &int
                path("ref_int"),                     // user type `ref_int`
            ),
            (
                TypeRef::Ptr(Box::new(path("int"))), // *int
                path("ptr_int"),                     // user type `ptr_int`
            ),
            (
                TypeRef::Array {
                    elem: Box::new(path("int")),
                    len: 2,
                }, // [int; 2]
                path("arr_2_3int"), // a name that mimics the old mangle
            ),
        ];
        for (constructed, named) in cases {
            assert_ne!(
                array_mangle(&constructed),
                array_mangle(&named),
                "mangle collision between {constructed:?} and {named:?}"
            );
        }
    }

    /// Distinct types across the whole grid produce distinct wrapper names.
    #[test]
    fn wrapper_names_are_all_distinct() {
        let elems = [
            path("int32"),
            path("usize"),
            TypeRef::Ref(Box::new(path("int32"))),
            TypeRef::Ptr(Box::new(path("int32"))),
            TypeRef::Array {
                elem: Box::new(path("int32")),
                len: 2,
            },
        ];
        let mut names = Vec::new();
        for elem in &elems {
            for len in [2u64, 3, 23] {
                names.push(array_wrapper_name(elem, len));
            }
        }
        let unique: FxHashSet<&String> = names.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "duplicate wrapper name: {names:?}"
        );
    }
}
