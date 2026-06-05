-- absurdly nested value-position expressions

enum Shape = Circle | Square | Triangle;

sides(Shape s) -> int32 {
    match s {
        Circle -> 0,
        Square -> 4,
        Triangle -> 3,
    }
}

main() {
    let Shape shape = Square;

    let int32 result =
        if sides(shape) > 3 {
            match shape {
                Circle -> 100,
                Square -> 200,
                Triangle -> 300,
            }
        } else {
            0
        };

    println("{}", result);
}
