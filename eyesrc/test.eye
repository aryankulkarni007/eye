-- testing enum + unions
-- FIXME: we can't even chain if statements with else if yet bro

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
        }
        if i == 1 {
            print("Rectangle");
        }
        if i == 2 {
            print("Triangle");
        }
        i = i + 1;
    }
}

