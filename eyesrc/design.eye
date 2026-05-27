--- BASIC V0.2 DESIGN.EYE ---

--- STRUCTURE ---
structure Point {
    int32 x,
    int32 y,
};

--- ENUM ---
--*
  * with waterfall syntax
--*
enum Shape =
| Square
| Circle
| Triangle
;

add(int32 a, int32 b) -> int32 {
    a + b
}

main() {
    -- primitive bindings
    const int32 x = 10;
    var int32 counter = 0;

    -- struct instantiation
    var Point pt = Point { x: 10, y: 20 };

    -- member access & modification
    pt.x = 15;

    -- pointer & reference usage
    var &Point pt_ref = &pt;
    pt_ref.y = 30;

    -- expression-based assignment
    const int32 max = if x > counter { x } else { counter };

    loop {
        if counter > 10 { break; }
        print("{}", counter);
        counter = counter + 1;
    }
}
