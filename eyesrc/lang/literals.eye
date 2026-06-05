-- literals: integer base prefixes, character literals, and booleans. Base
-- prefixes are parsed at compile time and emitted in decimal, so the same
-- value can be written in whichever base reads clearest.

main() {
    -- the same number, four ways. all equal 255.
    let int32 dec = 255;
    let int32 hex = 0xFF;
    let int32 oct = 0o377;
    let int32 bin = 0b11111111;
    println("bases   {} {} {} {}", dec, hex, oct, bin);

    -- uppercase prefixes work too: 0X / 0O / 0B.
    let int32 mask = 0xFF00;
    println("mask    {}", mask);

    -- hex is the natural notation for bit work: a color channel layout.
    let uint32 rgba = 0x11223344;
    let uint32 red = (rgba >> 24) & 0xFF;
    let uint32 green = (rgba >> 16) & 0xFF;
    println("red     {}", red);
    println("green   {}", green);

    -- character literals are values; cast to an integer for the code point.
    let char a = 'A';
    println("char    {} = {}", a, a as int32);

    -- booleans.
    let bool yes = true;
    let bool no = false;
    println("bool    {} {}", yes, no);
}
