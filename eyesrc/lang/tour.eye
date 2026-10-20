-- tour.eye: every Eye construct on one page.
-- comments: `--` to end of line, `--* ... --*` for a block.
-- items (top level): extern, const, global, structure, union, enum, fn.
-- a fn body takes no trailing `;`; structure/union/enum items do.

extern {                                -- C ABI seam, resolved by the linker
    malloc(usize n) -> ptr;             -- `ptr` is the untyped raw pointer
    free(ptr p);                        -- no `->` means returns void
    printf(string fmt, ...) -> int32;   -- `...` last, after one named param
    type FILE;                          -- opaque type, legal behind * or & only
}

const int32 MAX = 100;                  -- compile-time value: inlined, no address
let int32 LIMIT = 4;                    -- global static, read-only
mut int32 counter = 0;                  -- global static, writable

structure Point { int32 x, int32 y, }; -- trailing comma required, `;` ends the item
union Bits { int64 i, float64 f, };     -- overlapping storage
enum Color = Red | Green | Blue;        -- plain tags, no payloads

add(int32 a, int32 b) -> int32 { a + b }       -- last expr without `;` is the value

twice((int32) -> int32 f, int32 v) -> int32 {  -- fn pointer type: (args) -> ret
    f(f(v))
}

bump(Point* p) { p.x += 1; }            -- `.` auto-derefs through pointers
sign(int32 n) -> int32 {
    if n < 0 { return -1; }             -- early return
    if n == 0 { return 0; }
    1
}

rank(Color c) -> int32 {                -- match is exhaustive, arms use `->`
    match c { Red -> 1, Green -> 2, Blue -> 3 }
}

main() {
    -- bindings: `let` immutable, `mut` writable, deep. type comes first.
    let int32 a = 0x1F + 0b101 + 0o17;  -- literals: dec, 0x, 0b, 0o
    mut float64 f = 2.5;                -- floats: digits.digits only
    let bool ok = true;
    let char ch = 'z';                  -- escapes: \n \t \r \0 \\ \" \'
    let string s = "hi\n";              -- really &[uint8; N], NUL-terminated
    -- primitive types: int8..int64 uint8..uint64 isize usize
    --                  float32 float64 bool char string ptr

    -- operators, high to low: unary (- ~ ! & *), * / %, + -, << >>, &, ^, |,
    -- comparisons (non-chaining: a < b < c is an error), &&, ||, assignment.
    mut int32 n = (a << 2 | 1) & 0xFF ^ 3;
    n += 1; n -= 1; n *= 2; n /= 2; n %= 7;      -- compound assign:
    n &= 7; n |= 8; n ^= 1; n <<= 1; n >>= 1;    -- all ten forms
    f = (n as float64) / 2.0;           -- `as` is the only conversion
    if n == 1 { n = 2; }                -- `if x = 5` is a compile error

    -- if is an expression; loop is the only loop (while/for are not kernel)
    let int32 big = if n > 5 { 1 } else if n > 2 { 2 } else { 3 };
    mut int32 i = 0;
    loop {
        i += 1;
        if i % 2 == 0 { continue; }
        if i > 9 { break; }
    }

    -- structs: named or shorthand literals only; destructure in let
    mut Point p = Point { x: 1, y: 2 };
    let int32 x = 5;
    let Point q = Point { x, y: 7 };
    let Point { x: qx, y: qy } = q;     -- every field must be bound
    p.y = qy;
    bump(&p);

    -- references and raw pointers: &T is shared, T* is raw, `*p` derefs
    let &Point rp = &p;
    mut int32 target = 7;
    let int32* ip = &target;
    *ip = 8;                            -- writes through raw pointers escape `let`
    let Point* heap = malloc(sizeof(Point)) as Point*;
    free(heap as ptr);

    -- arrays: by-value, length is a const expr, bounds checked when constant
    mut [int32; 3] xs = [10, 20, 30];
    let [int32; 4] zeros = [0; 4];      -- repeat form [value; count]
    let [[int32; 2]; 2] grid = [[1, 2], [3, 4]];
    xs[0] = grid[1][0] + (len(xs) as int32) + MAX;  -- len(x): usize, cast to mix
    let &[int32; 3] view = &xs;

    -- unions, enums, match (enum, int/char/bool literals, `_` wildcard)
    let Bits bits = Bits { i: 42 };
    let Color c = Green;                -- bare variant, resolved by expected type
    let int32 code = match c { Red -> 1, Green -> 2, _ -> 3 };
    match ch { 'a' -> println("a"), _ -> println("other") };

    -- function values: a fn name is a value; calls through it are ordinary
    let (int32, int32) -> int32 op = add;
    let int32 six = twice(sign, op(1, 2));
    counter += LIMIT;

    -- intrinsics: println("{} ...", args) types the spec; sizeof(T) is a usize
    println("{} {} {} {}", sign(-5), rank(c), sizeof(Point), six);
    println("{} {} {} {} {}", ok, big, qx, rp.x, view[0]);
    println("{} {} {} {}", zeros[3], bits.i, f, code);
    printf("raw %d %s", code, s);               -- variadic FFI call
}
