--- DESIGN.EYE ---

--- STRUCTURES ---
structure Point {
    int32 x,
    int32 y,
};

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
    const x = 10;
    var counter = 0;

    -- struct instantiation
    var pt = Point { 10, 20 };

    -- Member Access & Modification
    pt.x = 15;

    -- pointer & reference usage
    var &Point pt_ref = &pt;
    pt_ref.y = 30;

    -- expression-based assignment
    let max = if x > counter { x } else { counter };

    loop {
        if counter > 10 { break; }
        counter = counter + 1;
    }
}
