--- String literals as `&[uint8; N]` (Horizon 0, Component 3, Part B).

--- A string literal is a reference to a fixed byte array: `"hello"` has type
--- `&[uint8; 5]`, where N is the visible byte count (the NUL is storage-only).
--- This reuses the array machine - `len`, indexing, and OOB checks all work -
--- and gives a real string type (closing the old `print`-renders-`%d` bug).
--- `char` = `uint8` at the floor, so indexing yields a byte value.

--- Length is part of the type, so two different-length strings are different
--- types. A length-generic function would need either monomorphization (prime,
--- far-future) or a slice (stdlib); the kernel answer is to decay to `&uint8`
--- (deferred). So `&[uint8; N]` is for the literal site, where N is static.

-- a function over `string` (the length-erased byte-pointer view). A
-- `&[uint8; N]` argument decays to it - the kernel answer to length
-- polymorphism (no monomorphization, no slice).

extern {
    strlen(string s) -> usize;
}

first(string s) -> char {
    s[0]
}

last(string s) -> char {
    s[strlen(s) - 1]
}

main() {
    -- a literal printed directly renders as text (`%s`), not an address
    println("greet  {}", "world");

    -- stored in a binding of its exact type
    let &[uint8; 5] s = "hello";
    println("stored {}", s);
    println("len    {}", len(s));

    -- indexing yields the byte (uint8); 'h' = 104, 'o' = 111
    println("s[0]   {}", s[0]);
    println("s[4]   {}", s[4]);

    -- the stored string decays to `string` when passed to `first`
    println("first  {}", first(s));
    println("last   {}", last(s));

    -- escapes decode to real bytes, so `len` is the decoded count
    let &[uint8; 3] esc = "a\nb";
    println("esclen {}", len(esc));

    -- a `string`-typed (decayed) binding prints its text directly
    let string m = "hi";
    println("strvar {}", m);
}
