-- exercises print() format specifiers across every primitive type
-- plus reference-to-struct (lowers to %p)

structure Box {
    int32 n,
};

main() {
    -- integer
    const int32 i = 42;
    print("int32      i = {}", i);

    -- float32 (printf promotes to double, %f works)
    const float32 f32 = 1.5;
    print("float32    f32 = {}", f32);

    -- float64
    const float64 f64 = 3.14159;
    print("float64    f64 = {}", f64);

    -- bool (prints as 1 / 0 via %d)
    const bool t = true;
    const bool f = false;
    print("bool       t = {}  f = {}", t, f);

    -- char
    const char c = 'A';
    print("char       c = {}", c);

    -- string literal
    print("string     s = {}", "hello");

    -- reference to struct -> %p
    var Box b = Box { n: 7 };
    var &Box r = &b;
    print("&Box       r = {}", r);

    -- mixed multi-arg in one call
    print("mixed      i={} f64={} c={} s={} bool={}", i, f64, c, "world", t);

    -- literals straight through (literal-kind fallback path)
    print("literals   {} {} {} {} {}", 100, 2.71, 'Z', "lit", false);
}
