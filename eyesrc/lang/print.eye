-- exercises println() format specifiers across every primitive type
-- plus reference-to-struct (lowers to %p)

structure Box {
    int32 n,
};

main() {
    -- integer
    let int32 i = 42;
    println("int32      i = {}", i);

    -- float32 (printf promotes to double, %f works)
    let float32 f32 = 1.5;
    println("float32    f32 = {}", f32);

    -- float64
    let float64 f64 = 3.14159;
    println("float64    f64 = {}", f64);

    -- bool (prints as 1 / 0 via %d)
    let bool t = true;
    let bool f = false;
    println("bool       t = {}  f = {}", t, f);

    -- char
    let char c = 'A';
    println("char       c = {}", c);

    -- string literal
    println("string     s = {}", "hello");

    -- reference to struct -> %p
    mut Box b = Box { n: 7 };
    mut &Box r = &b;
    println("&Box       r = {}", r);

    -- mixed multi-arg in one call
    println("mixed      i={} f64={} c={} s={} bool={}", i, f64, c, "world", t);

    -- literals straight through (literal-kind fallback path)
    println("literals   {} {} {} {} {}", 100, 2.71, 'Z', "lit", false);
}
