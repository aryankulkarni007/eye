-- operators: modulo, bitwise binary (& | ^ << >>), prefix unary (~ !),
-- compound assignment (+= -=), and parenthesised grouping as the precedence
-- escape hatch.

main() {
    -- modulo (O1): native C `%`, int only
    let int32 m = 17 % 5;
    println("mod        {}", m);

    -- bitwise binary (O3): & | ^ << >>. infix `&`/`|` are disambiguated from
    -- prefix-ref / enum-separator by parser position.
    let int32 a = 12;
    let int32 b = 10;
    println("bitand     {}", a & b);
    println("bitor      {}", a | b);
    println("bitxor     {}", a ^ b);
    println("shl        {}", a << 2);
    println("shr        {}", a >> 1);

    -- prefix unary (O2): `~` bitwise-not (preserves type), `!` logical-not
    -- (types bool).
    println("bitnot     {}", ~a);
    let bool flag = false;
    println("lognot     {}", !flag);

    -- compound assignment (O4): `+=` / `-=` only, straight to native C.
    mut int32 c = 100;
    c += 5;
    c -= 20;
    println("compound   {}", c);

    -- grouping (G1): `( expr )` overrides precedence. Operators now have
    -- fixed, no-footgun binding power, so parens are the one escape hatch.
    println("grouped    {}", 2 * (3 + 4));
}
