-- EYE CANNONICAL DESIGN

-- some type of arena based memory system as first class in the lang
-- doesn't have to be in MVP to

-- this is a line comment
--- this is a doc comment

--*
  * this is a block comment
  * yes it is --* --*
  * symmetric
--*

--- STRUCTURES ---
structure Point {
    int x,
    int y,
};

-- trailing comma waterfall style :)
enum Shape =
| Square
| Circle
| Triangle
;

-- function defintion
-- no semi colon is implicit return
add(int32 a, int32 b) -> int32 {
    a + b
}

--- VARIABLE BINDING ---
-- primitive examples
int32, int64, uint8, float32, float64, bool

-- type infered immutable and mutable binding
const x = 10;
var counter = 0;

-- explicit immutable and mutable binding
const int32 y = 20;
var int32 x = 10;

-- array type
[int32; 3] arr = [1, 2, 3];

string name = "Aryan";

-- reference / pointer
&string          -- reference
&var string      -- mutable reference
*char            -- raw pointer

--- CONTROL FLOW ---
-- expression based language
let max = if x > y { x } else { y };

-- i want to be able to omit braces for single liners but it creates too many issues to justify
if x > 0 {
    println("positive");
} else if x == 0 {
    println("zero");
} else {
    println("negative");
}

-- only loop for now
-- loop / break / continue
loop {
    if counter > 10 { break; }
    counter = counter + 1;
}

--- OPERATORS ---

-- arithmetic
a + b - c * d / e % f

-- bitwise / shift
a & b | c ^ d
a << 2; b >> 3;

-- logical
a && b; a || b; !a;

-- comparison
a == b; a != b; a < b; a > b; a <= b; a >= b;

-- assignment & compound
x = 1;
x += 1; x -= 1; x *= 2; x /= 2; x %= 3;

--- UNIT TYPE ---
()

main() {
    print("Hello Eye");
    print("{}", age); -- string subsitution
}
