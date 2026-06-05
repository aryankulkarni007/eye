--- Compile-time constants (Horizon 0, Component 1).
---
--- A `const` is a compile-time VALUE, not storage: it has no guaranteed
--- address (`&MAX` is illegal) and a reference to it is inlined. The
--- initializer is a bounded const-expr folded at compile time - literals, the
--- operator set, and references to other consts. It does NOT run code at
--- compile time (no function calls); that is the far-future prime layer.

const int32   MAX  = 100;          -- a scalar value
const int32   DBL  = MAX * 2;      -- a const-expr may reference other consts
const int32   NEG  = 0 - 5;        -- negative folds correctly
const int32   BITS = 0xF0 | 0x0F;  -- the full operator set folds
const bool    BIG  = MAX > 50;     -- a comparison folds to bool
const float64 PI   = 3.0;
const float64 TAU  = PI * 2.0;     -- float arithmetic folds
const char    MARK = 'A';
const int32   ITAU = TAU as int32; -- a const-expr may cast: float 6.0 -> int 6
const usize   SIZE = 4;            -- usable as an array length (A6)

main() {
    println("max        {}", MAX);
    println("dbl        {}", DBL);
    println("neg        {}", NEG);
    println("bits       {}", BITS);
    println("big        {}", BIG);
    println("tau        {}", TAU);
    println("mark       {}", MARK);
    println("itau       {}", ITAU);

    -- A const drives a fixed-array length (A6, docs/planning/DEFER.md): `SIZE` and the
    -- const-expr `SIZE * 2` both fold to a count.
    mut [int32; SIZE] xs = [10, 20, 30, 40];
    xs[0] = MAX;
    println("len        {}", len(xs));
    println("xs0        {}", xs[0]);

    let [int32; SIZE * 2] ys = [1, 2, 3, 4, 5, 6, 7, 8];
    println("ys-len     {}", len(ys));

    -- Block-scope const: the same value semantics (inlined, no address, not
    -- assignable), scoped to the declaring block. The initializer may
    -- reference top-level consts and earlier local consts.
    const int32 LOC = MAX + 1;
    println("loc        {}", LOC);
    const int32 LOC2 = LOC * 2;
    println("loc2       {}", LOC2);

    -- An inner block shadows; the outer const is unaffected after it.
    if true {
        const int32 LOC = 7;
        println("inner      {}", LOC);
    }
    println("outer      {}", LOC);

    -- A local const drives a fixed-array length, like a top-level const.
    let [int32; LOC - 99] pair = [1, 2];
    println("pair-len   {}", len(pair));

    -- A negative local const folds and inlines through the spill path.
    const int32 NEGL = 0 - 3;
    println("negl       {}", NEGL);
}
