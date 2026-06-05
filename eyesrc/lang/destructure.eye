-- Struct destructuring in `let` (Horizon 0, Component 4 / S2): sugar for binding
-- each field over several lines. Exhaustive - every field must be bound.

structure Point {
    int32 x,
    int32 y,
};

structure Pair {
    Point a,
    Point b,
};

make() -> Point {
    Point { x: 3, y: 4 }
}

main() {
    let Point p = Point { x: 10, y: 20 };

    -- shorthand: binds x and y
    let Point { x, y } = p;
    println("x={} y={}", x, y);

    -- rename: binds px / py
    let Point { x: px, y: py } = p;
    println("px={} py={}", px, py);

    -- init is a call result (spilled to a temp, then projected)
    let Point { x: cx, y: cy } = make();
    println("cx={} cy={}", cx, cy);

    -- nested struct value: destructure the outer, fields are Point values
    let Pair pr = Pair { a: Point { x: 1, y: 2 }, b: Point { x: 5, y: 6 } };
    let Pair { a, b } = pr;
    println("a.x={} b.y={}", a.x, b.y);
}
