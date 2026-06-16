//! fixed-array value semantics: the struct-wrap representation.
//!
//! c cannot pass, return, or assign a bare array by value - an array always
//! decays to a pointer at those boundaries. to give eye arrays real value
//! semantics (copy on assign, pass/return by value, length carried in the
//! type), every `[T; N]` is lowered to a wrapper `struct { T data[N]; }`.
//! copy, by-value passing, and multi-dimensional nesting then fall out of c
//! struct semantics for free.
//!
//! this is a c-backend representation detail, not an eye language concept. a
//! future cranelift backend would emit a stack slot and a memcpy instead, with
//! no wrapper type. the language never mentions `.data`; indexing and `&a[0]`
//! are rewritten onto it here.
//!
//! wrapper names are derived purely from the type via interned handles, so
//! [`super::types::CType`] can render an array as its wrapper name with no
//! shared state. this module is now just the wrapper naming (the injective
//! mangle); deciding which wrappers to emit and in what order is the shared
//! type-declaration topology ([`hir::core::topo_order`]), driven from the MIR
//! emitter.

use hir::core::{TypeInterner, TypeKind, TypeRef};

/// mangle a type into a fragment of a c identifier. injective: two distinct
/// eye types never produce the same fragment, so they never collide on one
/// wrapper name (a collision would dedup two different element types to a
/// single typedef and miscompile one of them).
///
/// injectivity rests on two rules:
/// - a `Path` name is length-prefixed (`ref_int` -> `7ref_int`). the map `s -> len(s) ++ s` is injective because `n -> digits(n) + n` is strictly
/// increasing, so the prefix pins the name's extent unambiguously.
/// - the `ref_`/`ptr_`/`arr_`/`err` constructors all start with a letter, while
/// a length-prefixed `Path` always starts with a digit. so a user type named
/// `ref_int` (`7ref_int`) can never be confused with `&int` (`ref_3int`).
/// `arr_` puts the length first (`arr_3_5int32`) so the fragment parses
/// front-to-back: after `arr_`, the digit run is the length, then `_`, then
/// the element mangle.
fn array_mangle(ty: TypeRef, types: &TypeInterner) -> String {
    match types.lookup(ty) {
        TypeKind::Path(name) => {
            let n = name.to_string();
            format!("{}{}", n.len(), n)
        }
        TypeKind::Ref(inner) => format!("ref_{}", array_mangle(*inner, types)),
        TypeKind::Ptr(inner) => format!("ptr_{}", array_mangle(*inner, types)),
        // injective against a user type literally named `rawptr`: a `Path`
        // mangles length-prefixed (`6rawptr`), so the bare fragment is free.
        TypeKind::RawPtr => "rawptr".to_string(),
        TypeKind::Array { elem, len } => {
            format!("arr_{}_{}", len, array_mangle(*elem, types))
        }
        TypeKind::Fn {
            params,
            ret,
            variadic,
        } => array_mangle_fn(params, *ret, *variadic, types),
        TypeKind::Error => "err".to_string(),
    }
}

/// the c typedef name for the wrapper of `[elem; len]`. equal to
/// `__eye_` ++ `array_mangle([elem; len])`, spelled inline to avoid a clone.
pub(super) fn array_wrapper_name(elem: TypeRef, len: u64, types: &TypeInterner) -> String {
    format!("__eye_arr_{}_{}", len, array_mangle(elem, types))
}

/// the c typedef name for a function-pointer type `(params) -> ret`.
pub(super) fn fn_typedef_name(
    params: &[TypeRef],
    ret: Option<TypeRef>,
    variadic: bool,
    types: &TypeInterner,
) -> String {
    format!("__eye_{}", array_mangle_fn(params, ret, variadic, types))
}

fn array_mangle_fn(
    params: &[TypeRef],
    ret: Option<TypeRef>,
    variadic: bool,
    types: &TypeInterner,
) -> String {
    // `fn{n}v_` for variadic vs `fn{n}_`: the digit run is the fixed param
    // count, the `v` marks the trailing `...`, so the two can never collide.
    let mut s = format!("fn{}{}", params.len(), if variadic { "v" } else { "" });
    for &p in params {
        s.push('_');
        s.push_str(&array_mangle(p, types));
    }
    s.push_str("_to_");
    match ret {
        Some(r) => s.push_str(&array_mangle(r, types)),
        None => s.push_str("void"),
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use hir::core::Text;
    use rustc_hash::FxHashSet;

    fn new_types() -> TypeInterner {
        TypeInterner::new()
    }

    fn path(types: &mut TypeInterner, name: &str) -> TypeRef {
        types.intern(TypeKind::Path(Text::from(name)))
    }

    /// the mangle must be injective: a reference/pointer/array construction must
    /// never produce the same fragment as a user type literally named after the
    /// constructor prefix. before length-prefixing, `&int` and `ref_int`, `*int`
    /// and `ptr_int`, `[int; 2]` and `arr_2_3int` all collided.
    #[test]
    fn mangle_is_injective_against_named_types() {
        let mut t = new_types();
        let int = path(&mut t, "int");
        let ref_int = path(&mut t, "ref_int");
        let ptr_int = path(&mut t, "ptr_int");
        let arr_2_3int = path(&mut t, "arr_2_3int");
        let fn0_to_void = path(&mut t, "fn0_to_void");

        let cases = [
            (t.intern(TypeKind::Ref(int)), ref_int),
            (t.intern(TypeKind::Ptr(int)), ptr_int),
            (t.intern(TypeKind::Array { elem: int, len: 2 }), arr_2_3int),
            (
                t.intern(TypeKind::Fn {
                    params: vec![],
                    ret: None,
                    variadic: false,
                }),
                fn0_to_void,
            ),
        ];
        let mut set = FxHashSet::default();
        for (a, b) in &cases {
            let ma = array_mangle(*a, &t);
            let mb = array_mangle(*b, &t);
            assert_ne!(ma, mb, "mangle collision: {ma}");
            assert!(set.insert(ma), "duplicate mangle across cases");
        }
    }
}
