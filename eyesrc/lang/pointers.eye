-- pointers and references: `T*` (raw pointer) vs `&T` (reference), address-of
-- `&`, dereference `*`, writing through a pointer, and the no-copy `&[T; N]`.

structure Cell {
    int32 value,
};

-- a reference parameter mutates the caller's storage with no copy.
bump(&Cell c) {
    c.value = c.value + 1;   -- auto-deref through the reference
}

-- a raw pointer parameter: `*p` is the explicit deref read/write.
double_through(int32* p) {
    *p = *p * 2;
}

main() {
    mut Cell c = Cell { value: 10 };

    -- reference: address-of, then mutate through it ergonomically.
    bump(&c);
    bump(&c);
    println("cell    {}", c.value);

    -- raw pointer: `*p` reads and writes the pointee directly.
    mut int32 n = 21;
    mut int32* p = &n;
    println("deref   {}", *p);
    double_through(p);
    println("written {}", n);

    -- `*p = v` is the raw-pointer escape hatch (not mutation-tracked).
    *p = 100;
    println("escape  {}", n);
}
