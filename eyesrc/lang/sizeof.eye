--- `sizeof(T)` kernel intrinsic (Horizon 0, Component 2).
---
--- `sizeof(T)` is a compile-time `usize` equal to the target layout size of a
--- type. Eye does not model layout - it leans on the C backend, emitting
--- `sizeof(ctype)` - so the value is the platform's, exactly like C. The floor
--- accepts a bare named type (builtin, struct, union, or enum); compound types
--- (`sizeof(&T)`, `sizeof([T; N])`) are deferred. `sizeof` is recognized by
--- callee name like `print`/`len`, so a user-defined `sizeof` shadows it.

structure Point {
    int32 x,
    int32 y,
};

main() {
    println("int8   {}", sizeof(int8));
    println("int32  {}", sizeof(int32));
    println("int64  {}", sizeof(int64));
    println("char   {}", sizeof(char));
    println("Point  {}", sizeof(Point));

    -- the container-math motivation: `count * sizeof(T)`, the argument to
    -- `malloc`. `sizeof` folds into an ordinary const-typed expression.
    let usize count = 4;
    println("bytes  {}", count * sizeof(Point));
}
