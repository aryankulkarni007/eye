--> for fun: testing the language

extern {
    malloc(usize size) -> ptr;
    free(ptr p);
}

structure Vec3 { int32 x, int32 y, int32 z, };

add(Vec3 x, Vec3 y) -> Vec3 {
    Vec3
    {
        x: x.x + y.x,
        y: x.y + y.y,
        z: x.z + y.z,
    }
}

-- loop for the sake of testing +=
sum([int32; 3] data) -> int32 {
    mut usize i = 0;
    mut int32 acc = 0;
    loop {
        if i >= 3 { break; }
        acc += data[i];
        i += 1;
    }
    acc
}

print_vec(Vec3 v) { println("({}, {}, {})", v.x, v.y, v.z); }

main() {
    let int32 x = 1; let int32 y = 1; let int32 z = 1;

    -- testing shorthand and full structure initialization
    let Vec3 a = Vec3 {x, y, z};
    let Vec3 b = Vec3 {x: 1, y: 1, z: 1};

    let Vec3 c = add(a, b);
    mut Vec3* d = malloc(12) as Vec3*;
    *d = add(a, b);

    print_vec(c);
    print_vec(*d);

    -- auto-deref handling test
    -- needs to be mut for c codegen btw
    mut [int32; 3] data_c = [c.x, c.y, c.z];
    mut [int32; 3] data_d = [d.x, d.y, d.z];

    mut int32 i = 0;
    println("({}, {})", sum(data_c), sum(data_d));

    free(d);
}
