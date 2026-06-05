-- Caesar cipher. Shift each letter of a string by a fixed amount, wrapping
-- within its case; non-letters pass through unchanged. Composes string
-- indexing, char/int arithmetic, early return, and libc putchar/strlen.

extern {
    putchar(int32 c) -> int32;
    strlen(string s) -> usize;
}

-- shift one code point by `k`, staying inside 'A'..'Z' or 'a'..'z'.
shift(int32 c, int32 k) -> int32 {
    if c >= 65 && c <= 90 {            -- 'A'..'Z'
        return (c - 65 + k) % 26 + 65;
    }
    if c >= 97 && c <= 122 {           -- 'a'..'z'
        return (c - 97 + k) % 26 + 97;
    }
    c                                  -- punctuation, spaces, digits: untouched
}

-- encode `s` to stdout with the given shift.
encode(string s, int32 k) {
    let usize n = strlen(s);
    mut usize i = 0;
    loop {
        if i >= n { break; }
        putchar(shift(s[i] as int32, k));
        i += 1;
    }
    println("");   -- trailing newline
}

main() {
    let string message = "hello caeser";

    println("plain:");
    encode(message, 0);     -- shift 0 echoes the input

    println("cipher (+3):");
    encode(message, 3);     -- classic Caesar shift of three
}
