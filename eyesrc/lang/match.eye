--- match - kernel status as of v0.5 work

--- kernel `match` is a small CLOSED discriminant-dispatch primitive: given a
--- scrutinee that reduces to a discrete discriminant, pick one arm and
--- optionally produce one value. enum-only for now. see docs/features/MATCH.md.

enum Shape =
| Circle
| Rectangle
| Triangle
;

enum Color = Red | Green | Blue;

-- enum-returning fn: a match scrutinee can be any expr whose type is a known
-- enum, including a call result.
pick() -> Color { Green }

-- VALUE-POSITION match as a function's implicit return. The declared return
-- type (int32) is the result type: arms are checked against it, and codegen
-- hoists `_matchN` then `return`s it.
rank(Color col) -> int32 {
    match col {
        Red -> 1,
        Green -> 2,
        Blue -> 3,
    }
}

main() {
    let Shape sh = Rectangle;

    -- 1. STATEMENT-POSITION match -> lowers straight to a C `switch`, no temp,
    --    no result value. arms run for effect. no trailing-value requirement.
    match sh {
        Circle -> println("round"),
        Rectangle -> println("boxy"),
        Triangle -> println("pointy"),
    };

    -- 2. VALUE-POSITION match into a typed `let` -> hoisted into `_matchN` temp
    --    + assigning switch, then the let reads it. exhaustive over the enum,
    --    so no wildcard needed. bare variant names resolve against the
    --    scrutinee enum.
    let int32 sides = match sh {
        Circle -> 0,
        Rectangle -> 4,
        Triangle -> 3,
    };
    println("sides = {}", sides);

    -- 3. QUALIFIED `Enum.Variant` patterns mix freely with bare ones.
    let int32 is_tri = match sh {
        Shape.Triangle -> 1,
        _ -> 0,
    };
    println("is_tri = {}", is_tri);

    -- 4. WILDCARD `_` -> `default:`. silences exhaustiveness; arms after `_`
    --    are unreachable (diagnosed).
    let int32 is_round = match sh {
        Circle -> 1,
        _ -> 0,
    };
    println("is_round = {}", is_round);

    -- 5. EXPLICIT BINDING TYPE drives the result type. the `int64` binding is
    --    re-recorded onto the match, so the hoist temp is declared `int64_t`,
    --    not the first arm's `int32`. integer-literal arms widen freely.
    let int64 big = match sh {
        Circle -> 1,
        Rectangle -> 2,
        Triangle -> 3,
    };
    println("big = {}", big);

    -- 6. SCRUTINEE IS A CALL RESULT. `pick()` returns `Color`, a known enum, so
    --    it matches like any enum value.
    let int32 code = match pick() {
        Red -> 10,
        Green -> 20,
        Blue -> 30,
    };
    println("code = {}", code);

    -- 7. VALUE-POSITION match in any consuming context, not only a `let`. As a
    --    function-call argument here; as an implicit return inside `rank`. Both
    --    are arm-checked against their context type and hoisted to a temp.
    println("rank = {}", rank(pick()));
}

--*
  WHAT THE KERNEL ENFORCES (local correctness):
    - non-enum scrutinee            -> diagnostic
    - duplicate discriminant arm    -> diagnostic
    - arm after `_` wildcard        -> unreachable diagnostic
    - missing variants, no `_`      -> non-exhaustive diagnostic
    - value-position arms must all  -> "match arm type mismatch" diagnostic
      produce the result type          (e.g. `Rectangle -> "bad"` in an
      (known types only)                int match is rejected, not silent C)
    - fn tail vs declared return    -> "return type mismatch" diagnostic
      type (any tail expr, not          (a match tail is anchored on the
      just match)                       return type, then arm-checked)

  VALUE-POSITION COVERAGE: a match that produces a value is arm-checked and
  hoisted in every consuming context - `let` init, fn-call argument, operator
  operand, and implicit-return / block tail. A match in statement position (its
  value discarded) keeps the "no result type" rule and lowers to a bare switch.

  OUT OF KERNEL SCOPE (stdlib / supermacro later, or not yet built):
    - payload variants, destructuring, `Some(x)` bindings
    - guards, or-patterns, struct/tuple/array patterns
    - bool / integer / char / range discriminants (enum-only today)
    - block-bodied arms via surface `{ ... }` (not parsed as an arm body)
    - a match nested in a CONDITIONAL sub-expression (e.g. a branch of an
      `if`-expression used as a value): can't be hoisted as an unconditional
      preceding statement, so it stays uncovered (emits a visible marker).
--*
