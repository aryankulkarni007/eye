--- Fixed-size arrays as a first-class value type (v0.7).
--- `[T; N]` is a value: it copies on assignment, passes and returns by value,
--- and carries its length in its type. `&[T; N]` is a reference (no copy) that
--- still knows its length. A slice (a length-erased view) is NOT in the kernel.

--- Reference parameter: no copy. `len(xs)` reads the static length; `xs[i]`
--- reads through the reference.
sum(&[int32; 3] xs) -> int32 {
    mut int32 acc = 0;
    mut usize i = 0;
    loop {
        if i >= len(xs) { break; }
        acc += xs[i];
        i += 1;
    }
    acc
}

--- Return by value: the array is copied out, not a dangling stack pointer.
make() -> [int32; 3] { [10, 20, 30] }

main() {
    --- literal init, index as rvalue and lvalue
    mut [int32; 4] xs = [1, 2, 3, 4];
    xs[1] = 99;
    println("idx        {}", xs[1]);

    --- `len(xs)` is a compile-time usize constant from the type
    println("len        {}", len(xs));

    --- value copy: `b` is independent of `a`
    mut [int32; 3] a = [1, 2, 3];
    mut [int32; 3] b = a;
    b[0] = 77;
    println("copy       {} {}", a[0], b[0]);

    --- return by value
    let [int32; 3] r = make();
    println("return     {}", r[2]);

    --- reference (no copy): `sum` reads through `&[int32; 3]`
    println("sumref     {}", sum(&a));

    --- multi-dimensional arrays compose
    let [[int32; 2]; 2] g = [[1, 2], [3, 4]];
    println("grid       {}", g[1][0]);

    --- pointer-width element type composes with the array machinery
    let [usize; 2] sizes = [100, 200];
    println("usize      {}", sizes[1]);
}
