-- A tiny accumulator calculator. A program is a fixed list of (operator,
-- operand) steps applied left to right to a running total. Composes enums,
-- a value-position match for dispatch, structs, a struct array, and a loop.

enum Op = Add | Sub | Mul | Div;

structure Step {
    Op op,
    int32 operand,
};

-- dispatch on the operator. match is exhaustive over Op, so no wildcard.
apply(Op op, int32 acc, int32 x) -> int32 {
    match op {
        Add -> acc + x,
        Sub -> acc - x,
        Mul -> acc * x,
        Div -> acc / x,
    }
}

-- the operator's symbol, for the trace line.
symbol(Op op) -> char {
    match op {
        Add -> '+',
        Sub -> '-',
        Mul -> '*',
        Div -> '/',
    }
}

main() {
    -- evaluates ((((0 + 7) * 6) - 5) / 2).
    let [Step; 4] program = [
        Step { op: Add, operand: 7 },
        Step { op: Mul, operand: 6 },
        Step { op: Sub, operand: 5 },
        Step { op: Div, operand: 2 },
    ];

    mut int32 acc = 0;
    mut usize i = 0;
    loop {
        if i >= len(program) { break; }
        let Step s = program[i];
        acc = apply(s.op, acc, s.operand);
        println("{} {}  ->  {}", symbol(s.op), s.operand, acc);
        i += 1;
    }

    println("result   {}", acc);
}
