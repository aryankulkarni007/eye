--- Top-level globals: addressable static storage (Horizon 0, Component 3).
---
--- A top-level `let`/`mut` is a GLOBAL - static storage with an address,
--- distinct from a `const` (a value with no address). `let` is read-only,
--- `mut` is mutable. The initializer must be const-evaluable (C requires a
--- constant static initializer); it may reference a `const`. Unlike a const,
--- `&G` is legal and a `mut` global may be written.

const int32 BASE = 10;

let int32  ORIGIN  = 0;        -- read-only static
mut int32  counter = BASE;     -- mutable static, initialized from a const
let bool   ENABLED = true;

bump() {
    counter = counter + 1;     -- a `mut` global is writable
}

main() {
    println("origin   {}", ORIGIN);
    println("counter  {}", counter);
    bump();
    bump();
    println("counter  {}", counter);
    println("enabled  {}", ENABLED);

    -- a global has an address (unlike a const): `&counter` is legal.
    let int32* p = &counter;
    println("via-ptr  {}", *p);
}
