-- Primitive-domain match (Horizon 0, Component 4 / S1): int, char, and bool
-- scrutinees with literal arms. Enum match already worked; this proves the
-- domain generalization (literal `ArmTest::Const` lowering to `scrut == const`).

-- value-position int match with a `_` catch-all
grade(int32 score) -> char {
    match score {
        1 -> 'A',
        2 -> 'B',
        3 -> 'C',
        _ -> 'F',
    }
}

-- value-position bool match, total without `_` (both values covered)
flip(bool b) -> int32 {
    match b {
        true -> 0,
        false -> 1,
    }
}

-- value-position char match with a `_`
vowel(char c) -> int32 {
    match c {
        'a' -> 1,
        'e' -> 1,
        'i' -> 1,
        _ -> 0,
    }
}

main() {
    println("grade1 {}", grade(1));
    println("grade3 {}", grade(3));
    println("gradeX {}", grade(9));
    println("flipT  {}", flip(true));
    println("flipF  {}", flip(false));
    println("vowelA {}", vowel('a'));
    println("vowelZ {}", vowel('z'));

    -- statement-position int match (runs for effect, no result value)
    let int32 n = 2;
    match n {
        1 -> println("one"),
        2 -> println("two"),
        _ -> println("many"),
    };
}
