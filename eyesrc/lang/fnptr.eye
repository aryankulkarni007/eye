add(int32 a, int32 b) -> int32 {
    return a + b;
}

sub(int32 a, int32 b) -> int32 {
    return a - b;
}

-- A void-returning function (omits the '->' arrow entirely per specification)
log_event(int32 status) {
    println("Callback triggered! Status value: {}\n", status);
}

-- A high-order function accepting both a value-returning function pointer
-- and a void-returning side-effect callback
execute_and_report(
    int32 x,
    int32 y,
    (int32, int32) -> int32 operation,
    (int32) callback
) {
    let int32 result = operation(x, y); -- Indirect call
    callback(result);                   -- Indirect void call
}

main() {
    -- immutable binding via bare-name decay (No '&' allowed or required)
    let (int32, int32) -> int32 basic_add = add;
    println("Fixed pointer execution: {}\n", basic_add(10, 5));

    -- mutable binding (The pointer variable can be reassigned)
    mut (int32, int32) -> int32 dynamic_op = add;
    println("Dynamic op (initial add): {}\n", dynamic_op(20, 10));

    dynamic_op = sub; -- Repointing the mutable binding
    println("Dynamic op (after reassignment to sub): {}\n", dynamic_op(20, 10));

    -- array of function pointers
    -- uses the specified array type declaration grammar
    let [(int32, int32) -> int32; 2] op_table = [add, sub];

    let int32 array_res_0 = op_table[0](40, 2);
    let int32 array_res_1 = op_table[1](42, 2);
    println("Array dispatch index 0: {}\n", array_res_0);
    println("Array dispatch index 1: {}\n", array_res_1);

    -- full high-order pipeline composition
    -- passing both a mathematical operation and a void logger down the stack
    execute_and_report(100, 34, sub, log_event);
}
