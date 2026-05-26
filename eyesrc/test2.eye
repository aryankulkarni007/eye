--- testing match as tail expr

enum Shape =
| Circle
| Triangle
| Rectangle
;

-- FIXED:
-- to longer need to manually hoist this
-- when match is the tail expr, it should be hoisted during codegen
-- and it is as it should be
pick(Shape sh) -> int32 {
    match sh {
        Circle      -> 0,
        Triangle    -> 1,
        Rectangle   -> 2,
    }
}

main() {
    let Shape sh = Triangle;
    print("{}", pick(sh));
}
