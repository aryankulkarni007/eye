extern {
    putchar(int32 c) -> int32;
    sqrt(float64 n) -> float64;
}

structure Vec3 {
    float64 x,
    float64 y,
    float64 z,
};

dot(Vec3 a, Vec3 b) -> float64 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

main() {
    let Vec3 light = Vec3 { x: 0.577, y: 0.577, z: -0.577 };

    mut int32 y = 0;
    loop {
        if y >= 30 { break; }

        mut int32 x = 0;
        loop {
            if x >= 60 { break; }

            let float64 fx = x as float64 / 30.0 - 1.0;
            let float64 fy = y as float64 / 15.0 - 1.0;
            let float64 d = fx * fx + fy * fy;

            if d <= 1.0 {
                let float64 fz = sqrt(1.0 - d);
                let Vec3 n = Vec3 { x: fx, y: fy, z: fz };
                let float64 b = dot(n, light);

                -- value-position if expressions
                let int32 ch = if b > 0.5 {
                    '#' as int32
                } else if b > 0.2 {
                    '*' as int32
                } else if b > -0.2 {
                    '.' as int32
                } else {
                    '-' as int32
                };

                putchar(ch);
            } else {
                putchar(' ' as int32);
            }

            x = x + 1;
        }

        println("");
        y = y + 1;
    }
}
