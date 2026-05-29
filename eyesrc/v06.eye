--- v0.6 syntax of EYE: operator completeness - modulo, bitwise, unary, compound assign

main() {
    -- modulo (O1): native C `%`, int only
    let int32 m = 17 % 5;
    print("mod        {}", m);

    -- bitwise binary (O3): & | ^ << >>. infix `&`/`|` are disambiguated from
    -- prefix-ref / enum-separator by parser position.
    let int32 a = 12;
    let int32 b = 10;
    print("bitand     {}", a & b);
    print("bitor      {}", a | b);
    print("bitxor     {}", a ^ b);
    print("shl        {}", a << 2);
    print("shr        {}", a >> 1);

    -- prefix unary (O2): `~` bitwise-not (preserves type), `!` logical-not
    -- (types bool).
    print("bitnot     {}", ~a);
    let bool flag = false;
    print("lognot     {}", !flag);

    -- compound assignment (O4): `+=` / `-=` only, straight to native C.
    mut int32 c = 100;
    c += 5;
    c -= 20;
    print("compound   {}", c);
}
