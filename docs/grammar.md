# Eye Language Grammar (Formal)

This document defines the concrete syntax of the Eye programming language in EBNF notation.

## Notation

| Symbol     | Meaning                          |
|------------|----------------------------------|
| `A = B`    | Production: A expands to B       |
| `A \| B`   | Choice: A or B                   |
| `A*`       | Repetition: zero or more         |
| `A+`       | Repetition: one or more          |
| `A?`       | Option: zero or one              |
| `{ A }`    | Grouping                         |
| `( A )`    | Same as `{ A }`                  |
| `"token"`  | Literal token                    |
| `@A`       | Named production reference       |

## Lexical Grammar

### Tokens

```
token     = whitespace | comment | keyword | literal | identifier | punct
whitespace = { " " | "\t" | "\r" | "\n" }+
comment   = line_comment | block_comment
line_comment = "--" { not_newline }*
block_comment = "--*" { any_char }* "--*"
```

### Identifiers

```
identifier = XID_Start { XID_Continue }*     (* Unicode XID *)
```

The lone underscore `_` is the **wildcard** keyword, not an identifier.

### Keywords

All keywords are reserved and cannot be used as identifiers:

```
keyword = "let" | "mut" | "const" | "structure" | "enum" | "union"
        | "extern" | "if" | "else" | "loop" | "break" | "continue"
        | "return" | "match" | "as" | "_" | "true" | "false"
```

### Literals

```
literal  = int_literal | float_literal | string_literal | char_literal | bool_literal

int_literal    = decimal_literal | hex_literal | binary_literal | octal_literal
decimal_literal = digit+
hex_literal     = "0x" hex_digit+ | "0X" hex_digit+
binary_literal  = "0b" bin_digit+  | "0B" bin_digit+
octal_literal   = "0o" oct_digit+  | "0O" oct_digit+

float_literal   = digit+ "." digit+

string_literal  = '"' { string_char } '"'
char_literal    = "'" char_char "'"       (* exactly one char, or escape *)

bool_literal    = "true" | "false"

string_char     = printable_ascii - '"' | escape_sequence
char_char       = printable_ascii - "'" | escape_sequence
escape_sequence = "\" ("n" | "t" | "r" | "0" | "\" | '"' | "'")
```

### Operators and Delimiters

```
operator = arithmetic_op | bitwise_op | comparison_op | logical_op | assignment_op | prefix_op

arithmetic_op = "+" | "-" | "*" | "/" | "%"
bitwise_op    = "&" | "|" | "^" | "~" | "<<" | ">>"
comparison_op = "==" | "!=" | "<" | ">" | "<=" | ">="
logical_op    = "&&" | "||"
assignment_op = "=" | "+=" | "-=" | "*=" | "/=" | "%="
              | "&=" | "|=" | "^=" | "<<=" | ">>="
prefix_op     = "-" | "~" | "!" | "&" | "*"

delimiter     = "(" | ")" | "{" | "}" | "[" | "]" | "," | ";" | ":"
              | "." | "->" | "=>"
```

The ampersand `&` and pipe `|` are disambiguated by position:
- Prefix `&` is a reference expression (`RefExpr`)
- Infix `&` is bitwise AND (`BinExpr`)
- Infix `|` after `=` in `enum` decl is a variant separator
- Infix `|` is bitwise OR (`BinExpr`)

## Syntax Grammar

The syntax is defined as a concrete syntax tree (CST) via a recursive-descent / Pratt parser. Non-terminals in **bold** are CST node kinds.

### 1. Source Unit

```
source_file = item*
```

### 2. Items

```
item = const_def
     | global_def
     | struct_def
     | union_def
     | enum_def
     | extern_block
     | fn_def
```

#### 2.1 Const Definition

Declares a compile-time constant value (no address, inlined at use sites).

```
const_def = "const" type_ref identifier "=" expression ";"
```

Example: `const int32 MAX = 100;`

#### 2.2 Global Definition

Declares an addressable static-storage binding.

```
global_def = ("let" | "mut") type_ref identifier "=" expression ";"
```

Example: `let int32 SIZE = 4;` or `mut int32 counter = 0;`

#### 2.3 Struct Definition

```
struct_def = "structure" identifier field_list ";"
```

#### 2.4 Union Definition

```
union_def = "union" identifier field_list ";"
```

#### 2.5 Field List

Shared by struct and union.

```
field_list = "{" { field "," } "}"
field      = type_ref identifier
```

Trailing comma required after each field.

Example:
```
structure Point {
    int32 x,
    int32 y,
};
```

#### 2.6 Enum Definition

Style: waterfall C-style enum with pipe separators.

```
enum_def = "enum" identifier "=" "|"? variant { "|" variant } ";"
variant  = identifier
```

Example:
```
enum Color = Red | Green | Blue;
enum Shape =
| Circle
| Square
;
```

#### 2.7 Extern Block

FFI declarations for functions linked at the C level.

```
extern_block  = "extern" "{" { extern_fn } "}"
extern_fn     = identifier param_list ( "->" type_ref )? ";"
```

External functions have no body -- they are resolved by the C linker.

Example:
```
extern {
    malloc(usize size) -> ptr;
    free(ptr p);
}
```

#### 2.8 Function Definition

```
fn_def = identifier param_list ( "->" type_ref )? block

param_list = "(" [ param { "," param } [","] ] ")"
param      = type_ref identifier
```

If `-> type_ref` is omitted, the function returns `void`.

Examples:
```
add(int32 a, int32 b) -> int32 { a + b }
main() { ... }
report(int32 n) { ... }              -- void return (no arrow)
```

### 3. Types

```
type_ref = array_type
         | ref_type
         | ptr_type
         | fn_type
         | ident_type

ident_type = identifier                      (* e.g. int32, Point, Color *)
ref_type   = "&" type_ref                    (* e.g. &Point, &[int32; 3] *)
ptr_type   = type_ref "*"                    (* e.g. Point*, int32* -- postfix *)
array_type = "[" type_ref ";" expression "]" (* e.g. [int32; 3] *)
fn_type    = "(" [ fn_type_param { "," fn_type_param } ] ")" ( "->" type_ref )?
fn_type_param = type_ref [","]
```

Notes:
- Reference-to-reference must be written `& &T` (space required to avoid the `&&` token).
- Function pointer type syntax: `(int32, bool) -> float64`

### 4. Blocks

```
block = "{" { statement } [ tail_expression ] "}"
```

The final expression (not terminated by `;`) is the block's value.

### 5. Statements

```
statement = let_statement
          | expression_statement
```

#### 5.1 Let Statement

```
let_statement = ("let" | "mut") [ type_ref identifier | struct_pattern ]
                "=" expression ";"
```

If `type_ref identifier` is omitted, the type is inferred from the initializer.
If the token after `let`/`mut` is an identifier followed by `{`, it is parsed as a struct destructure pattern.

Examples:
```
let x = 42;                                          (* inferred *)
let int32 x = expr;                                  (* explicit type *)
mut Point p = Point { x: 1, y: 2 };                  (* mutable *)
let &Point r = &p;                                   (* reference *)
let [int32; 3] xs = [10, 20, 30];                    (* array *)
let Point { x, y } = p;                              (* destructure *)
```

#### 5.2 Expression Statement

```
expression_statement = expression ";"
```

Block-like expressions (`if`, `loop`, `match`) do NOT require a semicolon in statement position (the parser treats them as statements automatically).

### 6. Expressions

Expressions are parsed via a Pratt parser with the precedence levels below.

```
expression = assignment_expression

assignment_expression = logical_or_expression { assignment_op assignment_expression }
                       (* right-associative *)

logical_or_expression  = logical_and_expression { "||" logical_and_expression }
logical_and_expression = bitwise_or_expression { "&&" bitwise_or_expression }
bitwise_or_expression  = xor_expression { "|" xor_expression }
xor_expression         = bitwise_and_expression { "^" bitwise_and_expression }
bitwise_and_expression = shift_expression { "&" shift_expression }
shift_expression       = additive_expression { ("<<" | ">>") additive_expression }
additive_expression    = multiplicative_expression { ("+" | "-") multiplicative_expression }
multiplicative_expression = unary_expression { ("*" | "/" | "%") unary_expression }
```

#### 6.1 Unary / Prefix Expressions

```
unary_expression = prefix_expression
                 | postfix_expression

prefix_expression = ("-" | "~" | "!" | "&" | "*") unary_expression
```

| Prefix | Kind            |
|--------|-----------------|
| `-`    | Neg (`PrefixExpr`) |
| `~`    | BitNot (`PrefixExpr`) |
| `!`    | Not (`PrefixExpr`) |
| `&`    | Ref (`RefExpr`)  |
| `*`    | Deref (`DerefExpr`) |

#### 6.2 Postfix Expressions

```
postfix_expression = primary_expression { postfix_operator }

postfix_operator = call
                 | index
                 | field_access
                 | cast
                 | struct_body

call        = "(" [ expression { "," expression } [","] ] ")"
index       = "[" expression "]"
field_access = "." identifier
cast        = "as" type_ref
struct_body = "{" { struct_lit_field "," } "}"
```

#### 6.3 Struct Literal Fields

```
struct_lit_field = identifier ( ":" expression )?    (* named: x: val *)
                 | expression                        (* positional: val *)
```

Three forms:
- Shorthand: `Point { x, y }` -- field name becomes the binding.
- Explicit: `Point { x: 0, y: 1 }`
- Positional: `Point { 10, 20 }`

#### 6.4 Primary Expressions

```
primary_expression = literal
                   | identifier
                   | parenthesized_expression
                   | if_expression
                   | loop_expression
                   | break_expression
                   | continue_expression
                   | return_expression
                   | match_expression
                   | array_literal

parenthesized_expression = "(" expression ")"
(* a `;` after the first element selects the repeat form `[value; count]` *)
array_literal = "[" [ expression ( ";" expression | { "," expression } [","] ) ] "]"
```

### 6.5 If Expression

```
if_expression = "if" expression block ( "else" ( if_expression | block ) )?
```

Value-position: `let x = if cond { a } else { b };`

Assignment in the condition (`if x = 5`) is a deliberate compile error.

### 6.6 Loop Expression

```
loop_expression = "loop" block
```

The only loop primitive. `while` and `for` are not part of the kernel.

### 6.7 Break / Continue

```
break_expression    = "break" expression?
continue_expression = "continue"
```

`break` may carry a value: `break 42`.

### 6.8 Return Expression

```
return_expression = "return" expression?
```

### 6.9 Match Expression

```
match_expression = "match" expression "{" { match_arm } "}"
match_arm       = pattern ( "if" expression )? "->" expression ","?
```

Pattern forms:

```
pattern = wildcard_pattern
        | literal_pattern
        | path_pattern
        | bare_ident_pattern

wildcard_pattern  = "_"
literal_pattern   = int_literal | char_literal | bool_literal
path_pattern      = identifier "." identifier    (* qualified: Shape.Circle *)
bare_ident_pattern = identifier                  (* bare: Circle -- resolves to enum variant *)
```

The trailing comma on the last arm is optional.
Guard expressions are experimental: `A if flag -> body`.

### 7. Struct Destructure Pattern

```
struct_pattern           = identifier struct_pattern_field_list
struct_pattern_field_list = "{" { struct_pattern_field } "}"
struct_pattern_field     = identifier ( ":" identifier )?
```

Shorthand: `let Point { x, y } = p;` -- binds each field to a local of the same name.
Rename: `let Point { x: px, y: py } = p;` -- binds under a different name.

Every field must be bound (no `..`/rest pattern yet).

## Operator Precedence

From lowest to highest binding power:

| Level | Operators | Assoc | Node |
|-------|-----------|-------|------|
| 1     | `=` `+=` `-=` `*=` `/=` `%=` `&=` `|=` `^=` `<<=` `>>=` | Right | `AssignExpr` |
| 2     | `\|\|` | Left | `BinExpr` (LogicalOr) |
| 3     | `&&` | Left | `BinExpr` (LogicalAnd) |
| 4     | `==` `!=` `<` `>` `<=` `>=` | -- | `BinExpr` (comparison) |
| 5     | `\|` | Left | `BinExpr` (BitOr) |
| 6     | `^` | Left | `BinExpr` (BitXor) |
| 7     | `&` | Left | `BinExpr` (BitAnd) |
| 8     | `<<` `>>` | Left | `BinExpr` (Shl, Shr) |
| 9     | `+` `-` | Left | `BinExpr` (Add, Sub) |
| 10    | `*` `/` `%` | Left | `BinExpr` (Mul, Div, Rem) |
| Prefix | `-` `~` `!` `&` `*` | -- | `PrefixExpr` / `RefExpr` / `DerefExpr` |
| Postfix | `()` `[]` `.` `as` `{}` | -- | call, index, field, cast, struct-body |

Note: Comparison operators are non-associative -- `a < b < c` is a deliberate compile error (`GrammarError::ComparisonChain`).

## Diagnostics

The parser reports two classes of diagnostics:

- **Syntax errors** (`S` class, ~50 variants): missing tokens, unexpected tokens, malformed constructs.
- **Grammar errors** (`G` class): intentional footgun rejections:
  - `ComparisonChain`: chained comparisons (`a < b < c`)
  - `AssignInIfCondition`: assignment in `if` condition (`if x = 5`)
