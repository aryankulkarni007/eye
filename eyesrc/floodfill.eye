-- recursive flood fill using value arrays and references

structure Point {
    int32 x,
    int32 y,
};

fill(
    &[[int32; 8]; 8] grid,
    Point p,
    int32 value,
    int32 replacement
) {
    if value == replacement { return; }

    if p.x < 0 || p.x >= 8 { return; }
    if p.y < 0 || p.y >= 8 { return; }

    if grid[p.y][p.x] != value {
        return;
    }

    grid[p.y][p.x] = replacement;

    fill(grid, Point { x: p.x + 1, y: p.y }, value, replacement);
    fill(grid, Point { x: p.x - 1, y: p.y }, value, replacement);
    fill(grid, Point { x: p.x, y: p.y + 1 }, value, replacement);
    fill(grid, Point { x: p.x, y: p.y - 1 }, value, replacement);
}

main() {
    mut [[int32; 8]; 8] grid = [
        [1,1,1,0,0,0,0,0],
        [1,0,1,0,1,1,1,0],
        [1,0,1,0,1,0,1,0],
        [1,1,1,0,1,0,1,0],
        [0,0,0,0,1,0,1,0],
        [1,1,1,1,1,0,1,0],
        [1,0,0,0,0,0,1,0],
        [1,1,1,1,1,1,1,0],
    ];

    fill(&grid, Point { x: 0, y: 0 }, 1, 9);

    print("{}", grid[0][0]);
    print("{}", grid[3][2]);
    print("{}", grid[7][6]);
}
