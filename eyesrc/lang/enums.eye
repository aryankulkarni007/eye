-- enums and match: enum declarations (inline and waterfall), statement-position
-- match (a bare switch, run for effect) and value-position match (hoisted into a
-- temp), exhaustive arms, and the wildcard `_`.

-- no generics so it is a glorified boolean enum
enum Option = Some | None;

enum Shape =
| Circle
| Rectangle
| Triangle
;

main() {
    let int32 x = 0;
    let Shape sh = Rectangle;
    println("{}", x);
    println("{}", sh);

    -- statement-position match: lowers straight to a switch, no temp.
    match sh {
        Circle -> println("round"),
        Rectangle -> println("boxy"),
        Triangle -> println("pointy"),
    };

    -- value-position match: hoisted into `int32 _match0;` + switch, then
    -- the let reads the temp. exhaustive, so no wildcard needed.
    let int32 sides = match sh {
        Circle -> 0,
        Rectangle -> 4,
        Triangle -> 3,
    };
    println("{}", sides);

    -- wildcard arm -> `default:` in the switch.
    let int32 is_round = match sh {
        Circle -> 1,
        _ -> 0,
    };
    println("{}", is_round);
}
