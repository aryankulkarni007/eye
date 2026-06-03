--*  Lite‑statistics library  --*
--*  Works on any contiguous array of float64 values via a pointer + length. --*

extern {
    sqrt(float64 x) -> float64;   -- libc square root
}

-- ── basic aggregates ────────────────────────────────────────────────

-- FIXME: this line doesn't highlight properly
sum(float64* data, usize len) -> float64 {
    mut float64 acc = 0.0;
    mut usize i = 0;
    loop {
        if i >= len { break; }
        acc = acc + data[i];       -- pointer indexing is zero‑cost
        i = i + 1;
    }
    acc
}

-- TESTING proves that the float is not the issue
test1(int32 a) {}
test2(float64* b) {}
test3(&float64 c) {}

mean(float64* data, usize len) -> float64 {
    sum(data, len) / len as float64
}

-- ── min / max ───────────────────────────────────────────────────────

min_value(float64* data, usize len) -> float64 {
    mut float64 best = data[0];
    mut usize i = 1;
    loop {
        if i >= len { break; }
        if data[i] < best { best = data[i]; }
        i = i + 1;
    }
    best
}

max_value(float64* data, usize len) -> float64 {
    mut float64 best = data[0];
    mut usize i = 1;
    loop {
        if i >= len { break; }
        if data[i] > best { best = data[i]; }
        i = i + 1;
    }
    best
}

-- ── variance / stddev (population) ──────────────────────────────────

variance(float64* data, usize len) -> float64 {
    let float64 avg = mean(data, len);
    mut float64 sum_sq = 0.0;
    mut usize i = 0;
    loop {
        if i >= len { break; }
        let float64 diff = data[i] - avg;
        sum_sq = sum_sq + diff * diff;
        i = i + 1;
    }
    sum_sq / len as float64
}

stddev(float64* data, usize len) -> float64 {
    sqrt(variance(data, len))
}

-- ── quick demo ──────────────────────────────────────────────────────

main() {
    -- our sample data
    let [float64; 6] xs = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0];

    -- obtain a pointer to the first element + length
    mut float64* ptr = &xs[0];
    let usize n = len(xs);

    print("data:     {} {} {} {} {} {}", xs[0], xs[1], xs[2], xs[3], xs[4], xs[5]);
    print("count:    {}", n);
    print("sum:      {}", sum(ptr, n));
    print("mean:     {}", mean(ptr, n));
    print("min:      {}", min_value(ptr, n));
    print("max:      {}", max_value(ptr, n));
    print("variance: {}", variance(ptr, n));
    print("stddev:   {}", stddev(ptr, n));
}
