-- Array repeat literal `[value; N]`: the array-fill primitive, peer of the list
-- form `[a, b, c]`. `value` is evaluated once and copied `N` times (value
-- semantics); `N` is a const length (literal or `const`). Result type `[T; N]`.

const usize SIZE = 4;

structure Point { int32 x, int32 y, };

mut int32 calls = 0;
next() -> int32 { calls = calls + 1; calls }

main() {
    -- scalar fill, literal length
    let [int32; 3] a = [7; 3];
    println("a {} {} {} len {}", a[0], a[1], a[2], len(a));

    -- length from a const
    let [bool; SIZE] flags = [true; SIZE];
    println("flags {} {} len {}", flags[0], flags[3], len(flags));

    -- element coerces to the declared type (the int literal 0 -> int64)
    let [int64; 2] big = [0; 2];
    println("big {} {}", big[0], big[1]);

    -- struct value fill: each element is an independent copy
    let [Point; 2] ps = [Point { x: 1, y: 2 }; 2];
    println("ps {} {} {} {}", ps[0].x, ps[0].y, ps[1].x, ps[1].y);

    -- nested fill -> multi-dimensional
    let [[int32; 2]; 3] grid = [[9; 2]; 3];
    println("grid {} {} {}", grid[0][0], grid[1][1], grid[2][0]);

    -- evaluate-once: next() runs a single time, not N times
    let [int32; 4] same = [next(); 4];
    println("same {} {} {} {} calls {}", same[0], same[1], same[2], same[3], calls);
}
