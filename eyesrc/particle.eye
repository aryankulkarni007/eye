structure Particle {
    int32 x,
    int32 y,
};

main() {
    -- explicit typing and struct construction
    var Particle p = Particle { x: 0, y: 0 };

    -- pointer usage (v0.2 pointer support)
    var &Particle p_ref = &p;

    -- Control flow and field access
    var int32 i = 0;
    loop {
        if i > 10 {
            break;
        }

        -- updating fields via pointer
        p_ref.x = p_ref.x + 1;
        p_ref.y = p_ref.y + 2;
        print("{}", p_ref.x);
        print("{}", p_ref.y);

        i = i + 1;
    }
}
