-- mutability: bindings are immutable by default. `let` is read-only; `mut` is
-- writable. Immutability is deep - it covers field and index projections rooted
-- in the binding - and the one escape is a write through a raw pointer.

structure Counter {
    int32 hits,
};

main() {
    -- `let` is immutable. You can read it freely.
    let int32 limit = 5;
    println("limit   {}", limit);

    -- `mut` is writable: direct assignment and compound assignment both work.
    mut int32 total = 0;
    total = total + limit;
    total += 1;
    println("total   {}", total);

    -- mutation reaches through projections of a `mut` binding.
    mut Counter c = Counter { hits: 0 };
    c.hits = c.hits + 1;        -- field projection
    println("hits    {}", c.hits);

    mut [int32; 3] xs = [1, 2, 3];
    xs[0] = 99;                 -- index projection
    println("xs0     {}", xs[0]);

    -- The raw-pointer escape: a write through `*p` is NOT mutation-tracked, so
    -- it can reach storage behind an otherwise-immutable view.
    mut int32 n = 7;
    let int32* p = &n;
    *p = 8;
    println("escape  {}", n);

    -- The following are rejected by the compiler (immutable-by-default):
    --   let int32 k = 0;  k = 1;        -- TypeError: AssignToImmutable
    --   let Counter d = Counter { hits: 0 };  d.hits = 1;  -- same, via field
}
