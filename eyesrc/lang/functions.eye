-- functions: call-form declaration, void functions (no `->`), the block's last
-- expression as the implicit return value, explicit `return expr;` / `return;`
-- early return, and recursion.

-- value-returning: the last expression is the result, no `return` needed.
square(int32 n) -> int32 {
    n * n
}

-- explicit early return: bail out before the tail.
clamp_low(int32 n, int32 floor) -> int32 {
    if n < floor {
        return floor;
    }
    n
}

-- void function: omits the arrow entirely. `return;` exits early with no value.
report(int32 n) {
    if n == 0 {
        println("report  zero");
        return;
    }
    println("report  {}", n);
}

-- recursion: factorial via self-call.
factorial(int32 n) -> int32 {
    if n <= 1 {
        return 1;
    }
    n * factorial(n - 1)
}

main() {
    println("square  {}", square(6));
    println("clamp   {}", clamp_low(2, 10));
    println("clamp   {}", clamp_low(20, 10));
    report(0);
    report(42);
    println("fact    {}", factorial(5));
}
