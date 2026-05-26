--- v0.4 syntax of EYE: sized/unsigned integer primitives + `as` casts

main() {
    -- new sized signed integers (lower to int8_t .. int64_t)
    let int8 i8 = 1;
    let int16 i16 = 2;
    let int32 i32 = 3;
    let int64 i64 = 4;
    print("signed     {} {} {} {}", i8, i16, i32, i64);

    -- new unsigned integers (lower to uint8_t .. uint64_t)
    let uint8 u8 = 5;
    let uint16 u16 = 6;
    let uint32 u32 = 7;
    let uint64 u64 = 8;
    print("unsigned   {} {} {} {}", u8, u16, u32, u64);

    -- `as` cast: C cast semantics. 300 wraps mod 256 -> 44.
    let int32 big = 300;
    let uint8 wrapped = big as uint8;
    print("truncate   {}", wrapped);

    -- cast drives the value type: int promoted to float64 -> float division.
    let int32 n = 7;
    let float64 half = n as float64 / 2.0;
    print("promote    {}", half);

    -- cast binds tighter than binary ops: `a + b as int64` == `a + (b as int64)`.
    let int32 a = 10;
    let int16 b = 20;
    let int64 sum = a as int64 + b as int64;
    print("widen-add  {}", sum);

    -- narrowing chain: widen then narrow back.
    let uint64 wide = u8 as uint64;
    let uint8 narrow = wide as uint8;
    print("roundtrip  {}", narrow);
}
