# cast lattice

the rule for `as`. built 2026-06-16 as part of the S3 judgment pass
(TYPECK.md); before it, `as` was any-to-any. follows the ruling ratified
2026-06-12 (TYPECK.md deferred list). the judgment lives in the typeck walker's
`Cast` arm (`crates/typeck/src/infer/ty.rs`, `cast_allowed` / `cast_class`); the
rejection is `TypeError::CastNotAllowed` (T043).

## classes

every type sorts into one cast class:

= `Int` - a named integer type (`int8`..`uint64`, `usize`, `isize`)
= `Float` - `float32` / `float64`
= `Bool` - `bool`
= `Char` - `char`
= `Enum` - a declared enum (opaque; T035)
= `Pointer` - any reference/pointer (`&T`, `T*`, `ptr`)
= `Aggregate` - an array, struct, or union
= `Fn` - a function type/value
= `Unknown` - an `Error` type or an unresolved type name (a type parameter
the floor cannot resolve); kept lenient so a cast never
cascades a prior error

## lattice

the allowed directed pairs (`from -> to`); everything else rejects:

- `Int -> Int` any width / signedness
- `Int <-> Float` both directions (float->int truncates, CONST.md U4)
- `Float -> Float` width change (`float32` <-> `float64`)
- `Pointer <-> Pointer`
- `Int <-> Pointer` the `0 as ptr` null idiom and address arithmetic
- `Char -> Int` widen a char out to its code point
- `Bool -> Int` widen a bool out to 0/1
- `Enum -> Int` the enum's explicit numeric escape (T035)

the deliberate rejections (each relaxable later, none used by the corpus):

x `_ -> Bool` no `x as bool`; write `x != 0` (the intent is explicit)
x `_ -> Char` no fabricating a char from an arbitrary value
x `Int -> Enum` no validity check exists, so an arbitrary int is not a variant
x `Float <-> Pointer`, `Char/Bool/Enum -> Float`, etc - not numeric-to-numeric
x `Aggregate` either side - an array/struct/union has no value-level conversion
x `Fn` either side - a function is not a value cast target

- `Unknown` either side - allowed (poison absorber, no cascade)

## why this shape

! the no-footgun ruling (Rust model): `as` is the explicit lossy/representation
cast, not a free reinterpret. numeric types (int/float) convert among
themselves; `char`/`bool`/`enum` are tagged - you may widen them OUT to an
integer but not fabricate them IN (`-> char`/`-> bool`/`-> enum` reject),
because there is no validity check to make the result meaningful. a struct is
not an int and a float bit pattern is not an address, so those reject.

-> the lattice is directional: `Char -> Int` is allowed but `Int -> Char` is
not. the asymmetry is the point (you can leave the tagged domains, not enter
them blind), so the rule is written over ordered `(from, to)` pairs.

## interaction with const folding

const-eval reproduces the C cast a value cast would emit (CONST.md, U4): an
integer _target_ truncates to that type's width (`200 as int8` folds to `-56`,
two's-complement). this composes with the U2 range check - an explicit `as`
to a type is the blessed truncation, a bare out-of-range value is rejected.

## not covered

+ `&[uint8; N] -> char*` (a string literal into a `char*` slot) is the
array-reference decay (STRING.md), not a cast. RESOLVED 2026-06-16: the decay
helper (`array_ref_decays_to`) accepts `&[T; N] -> &T`/`T*` on an exact element
type, the `string` (uint8) view, and the `char`<->`uint8` byte pun (`byte_pun`),
so a string literal decays into a `char*` slot - scalar, array element, or FFI
arg. The decay emits an explicit `(char*)` cast (well-defined; silences
`-Wpointer-sign`).
