-- testing 2D array deref


main() {
    -- FIXME: it seems there is an issue which non-int32 typed
    -- 2D arrays in eye
    let [[usize; 2]; 2] arr = [[1, 0], [0, 1]];

    -- WORKS:
    -- let [[int32; 2]; 2] arr = [[1, 0], [0, 1]];
    -- let [usize; 2] arr2 = [1, 0];

    print("{}", arr[0][0]);
    print("{}", arr[0][1]);
    print("{}", arr[1][0]);
    print("{}", arr[1][1]);
}
