--- Fixed-size arrays: `[T; N]` type, `[...]` literal, indexing.
--- Current supported path: one-dimensional local arrays with integer-literal
--- lengths. Function-boundary decay and pointer arithmetic are not specified.

main() {
    --- read-only array, literal init, index rvalue
    let [int32; 4] xs = [10, 20, 30, 40];
    print("{}", xs[0]);
    print("{}", xs[3]);

    --- mutable array, lvalue index assignment
    mut [int32; 3] ys = [1, 2, 3];
    ys[1] = 99;
    print("{}", ys[0]);
    print("{}", ys[1]);
    print("{}", ys[2]);

    --- index by a variable
    let int32 i = 2;
    print("{}", xs[i]);

    --- pointer-width element type composes with the array machinery
    let [usize; 2] sizes = [100, 200];
    print("{}", sizes[1]);
}
