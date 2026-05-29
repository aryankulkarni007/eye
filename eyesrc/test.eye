-- enum, union, and else-if smoke test

enum Shapes =
| Circle
| Rectangle
| Triangle
;

main() {
    mut uint32 i = 0;
    loop {
        if i > 4 { break; }

        if i == 0 {
            print("Circle");
        } else if i == 1 {
            print("Rectangle");
        } else if i == 2 {
            print("Triangle");
        }
        i = i + 1;
    }
}
