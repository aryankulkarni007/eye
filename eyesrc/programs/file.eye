--> my typa self-file read quine

extern {
    type FILE;
    printf(string fmt, ...) -> int32;
    perror(string fmt);
    fopen(string path, string mode) -> FILE*;
    fclose(FILE* f) -> int32;
    calloc(usize count, usize size) -> ptr;
    free(ptr p);

    fseek(FILE* f, int64 offset, int32 whence) -> int32;
    ftell(FILE* f) -> usize;
    rewind(FILE* f);
    fread(ptr buffer, usize size, usize count, FILE* f) -> usize;
}

const ptr NULL = 0 as ptr;
const int32 SEEK_END = 2;

main() {
    let FILE* file = fopen("eyesrc/programs/file.eye", "rb");
    if file == NULL {
        perror("fopen error");
        return;
    }

    fseek(file, 0, SEEK_END);
    let usize buffer_size = ftell(file);
    rewind(file);


    mut char* buf = calloc(buffer_size, 1) as char*;
    if buf == NULL {
        perror("calloc error");
        fclose(file);
        return;
    }

    let usize bytes_read = fread(buf, 1, buffer_size, file);
    if bytes_read != buffer_size {
        println("fread error");
        fclose(file);
        free(buf);
        return;
    }

    buf[buffer_size] = '\0';

    printf("%s", buf);

    fclose(file);
    free(buf);
}
