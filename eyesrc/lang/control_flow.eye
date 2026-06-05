-- control flow: `if` / `else if` / `else`, the single loop primitive `loop`
-- with `break` and `continue`, and `if` used in value position.

main() {
    -- if / else if / else chain.
    let int32 score = 72;
    if score >= 90 {
        println("grade   A");
    } else if score >= 80 {
        println("grade   B");
    } else if score >= 70 {
        println("grade   C");
    } else {
        println("grade   F");
    }

    -- `loop` is the only loop. `break` exits; the guard does the job of a
    -- `while` condition (while/for are deliberately not in the kernel).
    mut int32 sum = 0;
    mut int32 i = 1;
    loop {
        if i > 10 { break; }
        sum = sum + i;
        i = i + 1;
    }
    println("sum     {}", sum);

    -- `continue` skips the rest of the body: sum only the odd numbers below 10.
    mut int32 odd = 0;
    mut int32 j = 0;
    loop {
        j = j + 1;
        if j >= 10 { break; }
        if j % 2 == 0 { continue; }
        odd = odd + j;
    }
    println("odd     {}", odd);

    -- value-position `if`: the whole conditional is the initializer's value.
    -- both branches must agree on one result type.
    let int32 capped = if sum > 50 { 50 } else { sum };
    println("capped  {}", capped);
}
