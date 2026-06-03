
swap(&[int32; 8] xs, usize a, usize b) {
    let int32 tmp = xs[a];
    xs[a] = xs[b];
    xs[b] = tmp;
}

sort(&[int32; 8] xs) {
    mut int32 i = 0;

    loop {
        if i >= 8 { break; }

        mut int32 j = 0;

        loop {
            if j >= 7 { break; }

            -- (FIXED) this file used to have a > operation error because of
            -- using it to compare arrays -> fixed by doing this instead
            let int32 left = xs[j as usize];
            let int32 right = xs[(j + 1) as usize];

            if left > right {
                swap(xs, j as usize, (j + 1) as usize);
            }

            j = j + 1;
        }

        i = i + 1;
    }
}

main() {
    mut [int32; 8] data = [9, 2, 7, 1, 8, 3, 5, 4];

    mut int32 i = 0;
    loop {
        if i >= 8 { break; }

        printf("%zu ", data[i] as usize);
        i = i + 1;
    }

    print("");
    sort(&data);

    i = 0;
    loop {
        if i >= 8 { break; }

        printf("%zu ", data[i] as usize);
        i = i + 1;
    }
    print("");
}
