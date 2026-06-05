-- structs: declaration, literals (full + shorthand), nesting, field access,
-- one level of auto-deref through a reference, and struct-by-value return.

structure Color {
    uint8 r,
    uint8 g,
    uint8 b,
};

structure Pixel {
    int32 x,
    int32 y,
    Color color,   -- nested struct, stored by value
};

-- struct-returning function: the value is copied out, not aliased. The channel
-- average widens to int32 first so the sum cannot wrap in uint8, then narrows
-- back on store.
mix(Color a, Color b) -> Color {
    Color {
        r: ((a.r as int32 + b.r as int32) / 2) as uint8,
        g: ((a.g as int32 + b.g as int32) / 2) as uint8,
        b: ((a.b as int32 + b.b as int32) / 2) as uint8,
    }
}

main() {
    -- shorthand init: `r`/`g`/`b` are pulled from same-named bindings.
    let uint8 r = 200;
    let uint8 g = 100;
    let uint8 b = 50;
    let Color sand = Color { r, g, b };

    -- full init with a nested struct literal.
    mut Pixel px = Pixel {
        x: 4,
        y: 9,
        color: Color { r: 0, g: 0, b: 0 },
    };

    -- field access, including a nested field.
    println("pixel   ({}, {})", px.x, px.y);
    println("sand    {} {} {}", sand.r, sand.g, sand.b);

    -- write a whole nested struct by value.
    px.color = mix(sand, Color { r: 0, g: 200, b: 255 });
    println("mixed   {} {} {}", px.color.r, px.color.g, px.color.b);

    -- auto-deref: `.` reaches through one reference with no explicit `*`.
    mut &Pixel ref = &px;
    ref.x = ref.x + 1;
    println("viaref  {}", ref.x);
}
