-- A frequency histogram of dice rolls, drawn as ASCII bars. Tally a fixed
-- sample into per-face counts, then draw each face as a row of '#'. Composes
-- arrays, nested loops, casts, and libc putchar for newline-free output.

extern {
    putchar(int32 c) -> int32;
    rand() -> int32;
}

main() {
    -- twenty dice rolls (faces 1..6).
    mut [int32; 20] samples = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];

    -- fill the samples array with randomized dice rolls (1..6)
    mut usize fill_i = 0;
    loop {
        if fill_i >= len(samples) { break; }
        samples[fill_i] = (rand() % 6) + 1;
        fill_i += 1;
    }

    -- counts[face] accumulates the tally. index 0 is unused; faces are 1..6.
    mut [int32; 7] counts = [0, 0, 0, 0, 0, 0, 0];

    mut usize i = 0;
    loop {
        if i >= len(samples) { break; }
        let usize face = samples[i] as usize;
        counts[face] = counts[face] + 1;
        i += 1;
    }

    -- draw one row per face: "<digit>: ####".
    mut int32 face = 1;
    loop {
        if face > 6 { break; }

        putchar('0' as int32 + face);   -- the face digit
        putchar(':' as int32);
        putchar(' ' as int32);

        mut int32 bar = 0;
        loop {
            if bar >= counts[face as usize] { break; }
            putchar('#' as int32);
            bar += 1;
        }

        println("");   -- end the row (print always appends a newline)
        face += 1;
    }
}
