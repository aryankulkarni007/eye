-- implementation of Rc
-- NOTE: we literally can't make this until we implement some sort
-- of macro or comptime system in the language lmao
-- TODO: one day this should be able to be implemented
-- also how do we store the data if we don't have variable sized arrays
-- lowkey I need to take time to learn my own language

structure Rc {
    RcBox* ctrl_blk,
};

structure RcBox {
    usize s_count,
    usize w_count,
    ptr data,
};

main() {
    println("Hello Rc");
}
