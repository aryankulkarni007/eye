-- example eye program

structure Point {
    int32 x,
    int32 y,
};

main() {
    const int32 x = 0;
    const int32 y = 0;
    var Point p = Point { x, y };

    print("{}", p);
}
