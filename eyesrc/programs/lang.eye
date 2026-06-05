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
    printf(string fmt, ...) -> int32;
    malloc(usize size) -> ptr;
    free(ptr p);
    exit(int code);
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
-- we can declare off of type off and it is not caught
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
-- FIXME: this produces illegal C we cannot have struct
-- as the name of a field could be fixed to struct_0
structure Syllable {
    string str,
};

init(Arena* arena, usize size) -> ptr {
    -- NOTE: this language is not supposed to have
    -- NUL types but casting to 0 is (void *)0 in C
    -- which is the definition of NULL
    if size == 0 {
        return NULL;
    };

    arena.cap = size;
    arena.buffer = malloc(arena.cap);
    if arena.buffer == NULL {
        return NULL;
    };

    arena.off = 0;
    -- WARN: should casts be protected
    -- should we be able to arbitrarily cast any type
    -- this needs to be consider at type checking time
    arena.off as ptr
}

alloc(Arena* arena, usize size) -> ptr {
    let usize current_addr = (arena.buffer + arena.off) as usize;
    if arena.off + size > arena.cap {
        return NULL;
    };

    arena.off += size;
    current_addr as ptr
}

align_alloc(Arena *arena, usize size) -> ptr {
    let usize current_addr = (arena.buffer + arena.off) as usize;
    let usize mask = 8 - 1;
    let usize padding = (7 - (current_addr & mask)) & mask;

    let usize total_size = size + padding;
    if arena.off + total_size > arena.cap {
        return NULL;
    };

    arena.off += total_size;
    current_addr as ptr
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

}

print_lang(Language lang) {
    mut usize i = 0;
    loop {
        if i >= 10 { break; }
        -- nice error that println cannot format array
        -- does casting ptr to string work
        println("{}", lang.words[i] as string);
        if i >= 10 { println(", "); }
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
}
