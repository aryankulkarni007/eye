-- Match arm guards (pat if expr -> body).
--
-- A guard is an extra bool condition evaluated after the pattern matches.
-- If the pattern matches but the guard is false, the arm is skipped and
-- fallthrough continues to the next arm (or the wildcard). A guard is allowed on
-- any arm, including a bare-ident binding (`x if cond`) or a wildcard
-- (`_ if cond`): the guarded arm is lowered to an ordered fall-through, so a
-- false guard moves on to the next arm. Because a guarded arm may fail, it does
-- not count toward exhaustiveness - a match with guards still needs an
-- unconditional catch-all.

enum E = A | B;

main() {
    mut bool flag = true;
    let E e = A;

    -- Guard true: arm matches, prints 1
    let int32 r1 = match e {
        A if flag -> 1,
        B -> 2,
        _ -> 3,
    };
    println("r1 = {}", r1);

    -- Guard false: A is skipped, falls through to B (fails), then wildcard
    flag = false;
    let int32 r2 = match e {
        A if flag -> 4,
        B -> 5,
        _ -> 6,
    };
    println("r2 = {}", r2);

    -- Multiple guards: one arm matches guard, other arm guard fails
    let E e2 = B;
    flag = true;
    let int32 r3 = match e2 {
        A if flag -> 7,
        B if flag -> 8,
        _ -> 9,
    };
    println("r3 = {}", r3);

    -- Bare-ident binding catch-all with a guard: `n` binds the scrutinee and is
    -- in scope for both the guard and the body. Guard false -> falls to `_`.
    let int32 v = 20;
    let int32 r4 = match v {
        0 -> 100,
        n if n > 10 -> n,
        _ -> 0,
    };
    println("r4 = {}", r4);
}
