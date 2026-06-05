-- unions: overlapping storage. Every field shares one slot, so a union holds
-- exactly one member at a time. A union literal sets one member; reading a
-- different member reinterprets the same bytes.

union Bits {
    int64 i,
    float64 f,
};

-- the kernel union is untagged. The common safe pattern is to pair it with an
-- ordinary tag field that records which member is currently live.
structure Value {
    bool is_float,
    Bits payload,
};

main() {
    -- write the integer member, read it back.
    let Bits a = Bits { i: 42 };
    println("int     {}", a.i);

    -- write the float member of a separate union.
    let Bits b = Bits { f: 3.5 };
    println("float   {}", b.f);

    -- type punning: write as float64, read the raw bits back as int64. Both
    -- members alias the same 8 bytes, so this exposes the IEEE-754 encoding of
    -- 1.0 (0x3FF0000000000000 = 4607182418800017408).
    let Bits punned = Bits { f: 1.0 };
    println("rawbits {}", punned.i);

    -- the tagged pattern: the struct says how to read its union.
    let Value v = Value { is_float: true, payload: Bits { f: 2.5 } };
    if v.is_float {
        println("tagged  {}", v.payload.f);
    } else {
        println("tagged  {}", v.payload.i);
    }
}
