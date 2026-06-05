extern {
    rand() -> int32;    -- POSIX rand(3)
}

main() {
    let int32 SAMPLES = 500000;
    mut int32 inside = 0;
    mut int32 i = 0;

    loop {
        if i >= SAMPLES { break; }

        -- Scale to [0, 1). RAND_MAX is platform-defined but always ≥ 32767
        let float64 x = rand() as float64 / 2147483647.0;
        let float64 y = rand() as float64 / 2147483647.0;

        if x * x + y * y <= 1.0 {
            inside = inside + 1;
        }

        i = i + 1;
    }

    let float64 pi = 4.0 * inside as float64 / SAMPLES as float64;
    println("After {} samples, π ≈ {}", SAMPLES, pi);
}
