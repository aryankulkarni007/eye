-- example eye program

structure Point {
    int32 x,
    int32 y,
};

main() {
    const x = 0;
    const y = 0;
    var Point p = Point { x, y };

    print("{}", p);
}
