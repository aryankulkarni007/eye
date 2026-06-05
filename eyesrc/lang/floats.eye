-- floats: `float32` (C float) and `float64` (C double), literals, arithmetic,
-- `%f`-style printing, and casts between integer and floating-point types.

extern {
    sqrt(float64 x) -> float64;   -- libc square root
}

-- pythagorean hypotenuse, all in float64.
hypot(float64 a, float64 b) -> float64 {
    sqrt(a * a + b * b)
}

main() {
    -- both float widths.
    let float32 f32 = 1.5;
    let float64 f64 = 3.141592653589793;
    println("f32     {}", f32);
    println("f64     {}", f64);

    -- arithmetic stays in floating point.
    let float64 area = 3.14159 * 2.0 * 2.0;
    println("area    {}", area);

    -- int -> float cast drives the operation: this is float division, not the
    -- truncating integer division `7 / 2` would give.
    let int32 n = 7;
    let float64 half = n as float64 / 2.0;
    println("half    {}", half);

    -- float -> int cast truncates toward zero.
    let float64 pi = 3.99;
    let int32 trunc = pi as int32;
    println("trunc   {}", trunc);

    -- compose with an extern: a 3-4-5 right triangle.
    println("hypot   {}", hypot(3.0, 4.0));
}
