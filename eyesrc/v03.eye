--- v0.3 syntax of EYE

-- no generics so it is a glorified boolean enum
enum Option = Some | None;

enum Shape =
| Circle
| Rectangle
| Triangle
;

main() {
    const int32 x = 0;
    const Shape sh = Rectangle;
    print("{}", x);
    print("{}", sh);

    -- statement-position match: lowers straight to a switch, no temp.
    match sh {
        Circle -> print("round"),
        Rectangle -> print("boxy"),
        Triangle -> print("pointy"),
    };

    -- value-position match: hoisted into `int32 _match0;` + switch, then
    -- the let reads the temp. exhaustive, so no wildcard needed.
    const int32 sides = match sh {
        Circle -> 0,
        Rectangle -> 4,
        Triangle -> 3,
    };
    print("{}", sides);

    -- wildcard arm -> `default:` in the switch.
    const int32 is_round = match sh {
        Circle -> 1,
        _ -> 0,
    };
    print("{}", is_round);
}
