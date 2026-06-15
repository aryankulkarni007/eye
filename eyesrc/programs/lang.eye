-- eye port of language simulator in C
-- not that nothing is ever initialised to NULL
-- and we can't have self-referential structs yet
-- so we can't make the linked list but we can port the arena

-- FIXME: we should make SSA-like MIR gen locked behind an optimisation flag
-- the subexpresion unfolding is undebuggable. but we do this so that debugging
-- is more ergonomic


-- WARN: at codegen

-- struct __eye_arr_0_5uint8 { uint8_t data[0]; };

-- That suggests the compiler is representing "" as a zero-length array wrapper
-- and then storing a pointer to the static NUL byte:
-- static const uint8_t __eye_str0[1] = {0};
-- which works, but must inspect carefully because empty strings
-- are notorious edge cases

-- NOTE: to reproduce the C code that we have for this langauge
-- sim we seem to making quite unsafe decisions. compiler safety
-- is not absolute yet

-- NOTE: syntax highlighting is also cooked. We need to fix the treesitter
-- parser at some point

extern {
    memcpy(ptr dst, ptr src, usize n) -> ptr;
    rand() -> int32;
    printf(string fmt, ...) -> int32;
    malloc(usize size) -> ptr;
    free(ptr p);
    strlen(string s) -> usize;
    exit(int32 code);
}

-- in the generated C code this is expanded to 0
-- probably because it is not a macro and what is happening
-- under the hood is that the eye compiler evaluates the cast
-- and substitutes the 0 in, stripping the cast because
-- it has no effect
-- const ptr NULL = 0 as ptr;

-- same as above (evaluated away by the compiler)
const ptr NULL = (0 as ptr);

-- FIXME: we need type checking
-- (`off off` - an undeclared field type - is now caught, R012; field/arg
-- VALUE types are still unchecked, see ledger.md typeck scope)
structure Arena {
    uint8* buffer,
    usize cap,
    usize off,
    -- off off,
};

-- NOTE: we can't express a variable sized array like this in eye
-- is that a logic/valid limitation? only const exprs are allowed in
-- the length place in the array in a struct
structure Language {
    usize len,
    -- [char*; 10] words, -- arbitrarily make it 10 length
    [ptr; 10] words,
    -- ^ hack
};

-- ironic that the eye flips the structure struct thing
-- (a field cannot be named `struct`: C-keyword names are rejected, R010 -
-- reject was chosen over mangling so the emitted C keeps the source name)
structure Syllable {
    string str,
};

init(Arena* arena, usize size) -> ptr {
    -- NOTE: this language is not supposed to have
    -- NUL types but casting to 0 is (void *)0 in C
    -- which is the definition of NULL
    if size == 0 { return NULL; };

    arena.cap = size;
    arena.buffer = malloc(arena.cap);
    if arena.buffer == NULL { return NULL; };

    arena.off = 0;
    -- WARN: should casts be protected
    -- should we be able to arbitrarily cast any type
    -- this needs to be consider at type checking time
    arena.off as ptr
}

alloc(Arena* arena, usize size) -> ptr {
    let usize current_addr = (arena.buffer + arena.off) as usize;
    if arena.off + size > arena.cap { return NULL; };

    arena.off += size;
    current_addr as ptr
}

align_alloc(Arena *arena, usize size) -> ptr {
    let usize addr = (arena.buffer + arena.off) as usize;
    let usize alignment = 8;
    let usize mask = alignment - 1;
    let usize padding = (-addr) & mask;

    let usize total_size = size + padding;
    if arena.off + total_size > arena.cap { return NULL; };

    arena.off += total_size;
    addr as ptr
}

reset(Arena* arena) {
    arena.cap = 0;
    arena.off = 0;

    -- WARN: this is not caught by the compiler
    -- because free has no return value, it is allowed to have
    -- a missing semi because it doesn't violate the
    -- mismatched function return type decl to actual logic

    -- free(arena.buffer)

    free(arena.buffer);
}

generate_lang(Language* lang, Arena* arena, Syllable syl, usize wc) {
    -- NOTE: that these should be able to be const globals or locals
    -- but current code doesn't allow for aggregates in const-expr
    let [char*; 24] consonants = [
                                    "p", "b", "t",  "d",  "k",  "g", "f",  "v",
                                    "s", "z", "sh", "zh", "ch", "j", "th", "dh",
                                    "m", "n", "ng", "l",  "r",  "w", "y",  "h"
                                 ];
    let [char*; 10] vowels = [
        "a", -- cat
        "A", -- father
        "e", -- bed
        "i", -- machine
        "I", -- bit
        "o", -- boat
        "O", -- thought
        "u", -- goose
        "U", -- book
        "@", -- schwa
    ];

    mut usize onset_count = 0;
    mut usize coda_count = 0;

    mut bool seen_nucleus = false;
    mut usize i = 0;
    loop {
        if i >= strlen(syl.str) { break; }
        seen_nucleus = if syl.str == 'v' { true; };
        if syl.str[i] == 'c' && !seen_nucleus { onset_count += 1; }
        if syl.str[i] == 'c' && seen_nucleus  { coda_count += 1;  }
        i += 1;
    };

    if !seen_nucleus {
        println("{} missing mandatory nucleus", syl.str);
        exit(1);
    }

    init(arena, 1024); -- arbitrary arena size
    let usize cons_len = len(consonants);
    let usize vowel_len = len(vowels);

    -- NOTE: had to hardcode here
    mut [char*; 10] word_arr = [""; 10];
    mut usize idx = 0;
    loop {
        if idx >= wc { break; }
        let int32* onset_head = if onset_count > 0 {
            -- FIXME: shouldn't be allowed to omit semi even if the fn return
            -- doesn't match the required type
            malloc(sizeof(int32) * onset_count)
        } else { NULL };

        let int32* coda_head = if coda_count > 0 {
            malloc(sizeof(int32) * coda_count)
        } else { NULL };

        const float64 RAND_MAX = 0x7fffffff;
        mut int32* tmp = onset_head;
        mut usize ii = 0;
        loop {
            if ii >= onset_count { break; }
            -- NOTE: the compiler needs to check that Rvalue matches the declared type
            -- typechecker responsibility
            let int32 onset_idx = (rand() as float64 / (RAND_MAX + 1) * cons_len) as int32;
            memcpy(tmp, &onset_idx, sizeof(int32));
            tmp += 1;
            ii += 1; -- onset_head fill loop
        }

        -- same-scope redeclaration is an error (R015, ruled 2026-06-12); `tmp`
        -- is mut, so re-point the existing binding instead
        tmp = coda_head;
        mut usize ij = 0;
        loop {
            if ij >= coda_count { break; }
            -- NOTE: the compiler needs to check that Rvalue matches the declared type
            -- typechecker responsibility - i guess this is coerced to the right type auto
            -- but i suspect not
            let int32 coda_idx = (rand() as float64 / (RAND_MAX + 1) * cons_len) as int32;
            memcpy(tmp, &coda_idx, sizeof(int32));
            tmp += 1;
            ij += 1; -- coda_head fill loop
        }

        if onset_head != NULL {
            let char* onset = consonants[onset_head[0]];
            let usize onset_bytes = strlen(onset);
            let char* onset_a = align_alloc(arena, onset_bytes) as char*;
            if onset_a == NULL { exit(1); }

            word_arr[idx] = onset_a;
            memcpy(onset_a, onset, onset_bytes);

            mut usize loc_i = 1;
            loop {
                if loc_i >= onset_count { break; }
                let char* loc_onset = consonants[onset_head[loc_i]];
                let usize loc_onset_bytes = strlen(loc_onset);
                let char* loc_onset_a = alloc(arena, loc_onset_bytes) as ptr;
                if loc_onset_a == NULL { exit(1); }
                memcpy(loc_onset_a, loc_onset, loc_onset_bytes);
                loc_i += 1;
            }

        }

        let int32 nucleus_idx = (rand() as float64 / (RAND_MAX + 1) * vowel_len) as int32;
        let char* nucleus = vowels[nucleus_idx];
        let usize nucleus_bytes = strlen(nucleus);
        let char* nucleus_a = alloc(arena, nucleus_bytes) as ptr;

        if onset_head == NULL { word_arr[idx] = nucleus_a; }
        if nucleus_a == NULL  { exit(1);                 }

        memcpy(nucleus_a, nucleus, nucleus_bytes);

        if coda_head != NULL {
            mut usize loc_i = 0;
            loop {
                if loc_i >= coda_count - 1 { break; }

                let char* loc_coda = consonants[coda_head[loc_i]];
                let usize loc_coda_bytes = strlen(loc_coda);
                let char* loc_coda_a = alloc(arena, loc_coda_bytes) as ptr;

                if loc_coda_a == NULL { exit(1); }
                memcpy(loc_coda_a, loc_coda, loc_coda_bytes);
                loc_i += 1;
            }

            let char* coda = consonants[coda_head[loc_i]];
            let usize coda_bytes = strlen(coda) + 1;
            let char* coda_a = alloc(arena, coda_bytes) as ptr;
            if coda_a == NULL { exit(1); }
            memcpy(coda_a, coda, coda_bytes);
        }

        if coda_head == NULL {
            let char* null = alloc(arena, 1) as ptr;
            if null == NULL { exit(1); }
            *null = '\0';
        }

        free(onset_head);
        free(coda_head);
        idx += 1; -- main loop
    }

    lang.words = word_arr;
    return;
}

print_lang(Language lang) {
    mut usize i = 0;
    loop {
        if i >= 10 { break; }
        -- nice error that println cannot format array
        -- casting ptr to a string makes it work tho
        println("{}", lang.words[i] as string);
        if i < 9 { println(", "); }
    }
    println("");
}


main() {
    -- no cli args in eye support yet (hardcode it)
    -- we also must declare and initialise variables

    -- hmm I don't think we have the facilities to write the language
    -- sim in EYE yet. we will need to reason about it differently for sure
    -- under the hood string is not a char* anymore
    -- so we can't use string because it will codegen as a static uint8 array
    -- wait and there is a massive issue with the codegen for an array of
    -- char* here
    -- what if we make an array of void pointers and cast as required?
    -- NOTE: UPDATE this works but it will probably lead to bad things
    -- note that
    -- [char*; 10] as a type seems to be broken. probably because the compiler can't
    -- handle char* yet or some type of C codegen issue. is really a backend
    -- issue, but we need a working transpiler so we have to work around this
    let usize len = 0;
    let [ptr; 10] words = [""; 10];

    -- WARN: compiler doesn't error on unintialised struct! (wait maybe it does)
    -- WARN: compiler doesn't error when we pass in a field with the wrong type
    -- also a type checker responsibility
    mut Language lang = Language { len, words };
    mut Arena arena = Arena { buffer: 0 as uint8*, cap: 0, off: 0 };
    -- FIX: massive issue
    -- string as a struct field codegens as const char* str
    -- but a string is supposed to be a uint8 array under the hood
    let Syllable syllable = Syllable { str: "cvc" };

    -- FIXME: compiler doesn't error on incorrect args and ordering
    -- and all possible issues with that
    generate_lang(&lang, &arena, syllable, 10);
    print_lang(lang);
}
