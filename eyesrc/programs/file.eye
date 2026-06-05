--*  Read a file and print its contents using libc I/O.
     Exercises the C seam: an opaque FFI type (`type FILE;`), a variadic
     extern (`printf`), and `FILE*`-typed signatures - the extern block is
     the sole prototype (no auto-included header).  --*

extern {
    type FILE;
    printf(string fmt, ...) -> int32;
    fopen(string path, string mode) -> FILE*;
    fclose(FILE* f) -> int32;
    fgets(ptr buf, int32 n, FILE* f) -> ptr;
    calloc(usize count, usize size) -> ptr;
    free(ptr p);
    exit(int32 status);
}

main() {
    mut FILE* file = fopen("eyesrc/programs/file.eye", "r");
    if file == (0 as FILE*) {
        println("fopen error");
        exit(1);
    }

    mut ptr buf = calloc(256, 1);
    if buf == (0 as ptr) {
        println("calloc error");
        fclose(file);
        exit(1);
    }

    mut ptr line = fgets(buf, 255, file);
    loop {
        if line == (0 as ptr) { break; }
        printf("%s", buf as string);
        line = fgets(buf, 255, file);
    }

    fclose(file);
    free(buf);
}
