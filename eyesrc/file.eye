--*  Read a file and print its contents using libc I/O  --*

extern {
    calloc(usize count, usize size) -> ptr;
    free(ptr p);
    exit(int32 status);
}

main() {
    mut ptr file = fopen("eyesrc/file.eye", "r");
    if file == (0 as ptr) {
        print("fopen error");
        exit(1);
    }

    mut ptr buf = calloc(256, 1);
    if buf == (0 as ptr) {
        print("calloc error");
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
