-- println intrinsic: formats values with {} placeholders and auto-appends \n.
--
-- Supported types: int32, int64, float32, float64, bool, char, string
-- (&[uint8; N] byte arrays), and &T references (prints as %p).

main() {
    -- Integers
    let int32 i = 42;
    let int64 big = 1000000;
    println("int32  i   = {}", i);
    println("int64  big = {}", big);

    -- Floats
    let float32 f = 2.5;
    let float64 d = 3.1415926535;
    println("float32 f   = {}", f);
    println("float64 d   = {}", d);

    -- Bool prints as 1/0
    println("true   = {}", true);
    println("false  = {}", false);

    -- Char
    println("char   = {}", 'Z');

    -- String literal
    println("string = {}", "hello world");

    -- Multiple args in one call
    println("mixed  i={} f={} c={}", i, d, 'X');
}
