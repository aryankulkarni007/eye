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
}
