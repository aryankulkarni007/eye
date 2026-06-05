# Build tool (DESIGN, NOT BUILT)

> **DESIGN** - this document is aspirational. None of it is implemented. The
> current CLI is a single-file-input dumper (see `src/cli.rs`). Once the kernel
> freezes and the query architecture lands, this is the target shape.

## Manifest (`eye.toml`)

```toml
[project]
name = "my_project"
version = "0.1.0"

[[bin]]
name = "my_project"
path = "src/main.eye"

[build]
backend = "clang"
backend_args = []
```

Only `[[bin]]` sections. No `[lib]`, no dependencies, no workspace yet.

## CLI

```
eye build              # requires eye.toml in cwd (or parent walk)
eye build --file <f>   # ad-hoc single file, no manifest needed
eye run                # build + execute
eye check              # lex+parse+HIR only, no codegen
eye clean              # rm -rf target/
```

## Output layout

```
target/
  <bin_name>           # compiled binary
  eye-cache/
    state.json         # digests + per-item cache metadata
    <fn_name>.c        # cached C per function
```

## Incremental cache

Cache key: `blake3(source) + blake3(eye.toml) + compiler_version`.

Each top-level function's HIR body is hashed. On rebuild, re-parse (trivial), diff hashes,
skip MIR+codegen for unchanged functions. Concatenate cached `.c` fragments, run clang
only if any item changed or binary is missing.

## Library API

```rust
// eye/src/lib.rs - the compiler as a library
pub fn check_source(source: &str) -> CheckResult;
// shared with LSP: returns structured diagnostics, no subprocess

pub struct Project { .. }
impl Project {
    pub fn find_and_load() -> Result<Self>;     // walk up from cwd
    pub fn load(path: &Path) -> Result<Self>;   // explicit eye.toml
    pub fn build(&self, opts: &BuildOpts) -> Result<()>;
    pub fn run(&self) -> Result<()>;
    pub fn check(&self) -> Result<()>;
    pub fn clean(&self) -> Result<()>;
}
```

The binary becomes a thin CLI that calls into the library. The LSP calls `check_source`
directly with in-memory buffer contents - no subprocess, no JSON serialization.
