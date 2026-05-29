--- v0.4: FFI extern block + unions

-- libc signatures. names enter the global namespace, resolve at link.
extern {
    malloc(uint64 size) -> ptr;
    free(ptr p);
}

structure Point {
    int32 x,
    int32 y,
};

-- overlapping storage: an int64 and a float64 share one slot.
union Bits {
    int64 i,
    float64 f,
};

main() {
    -- heap-allocate a Point via libc malloc, cast the opaque ptr to Point*.
    mut Point* p = malloc(8) as Point*;

    -- union: write one member, read it back.
    mut Bits b = Bits { i: 42 };
    print("union i = {}", b.i);

    mut Bits g = Bits { f: 3.5 };
    print("union f = {}", g.f);

    -- hand the heap block back to libc.
    free(p as ptr);
    print("freed");
}
