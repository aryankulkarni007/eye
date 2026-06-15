# VFS: the virtual filesystem archive

**status: design. not built.** the archiver is being prototyped in C at
`vlt/` (outside this workspace). it will be rewritten in zig to validate the
wire format and memory model, then ported to rust as a `crates/vfs` crate.
this document designs the rust version's feature set and data model.

for grounding, read alongside:

- [LSP-ROADMAP.md](../planning/LSP-ROADMAP.md) — how the VFS enables LSP features
- [MASTERPLAN.md](../planning/MASTERPLAN.md) — the compiler's strategic horizon map
- [FUTURE.md](../planning/FUTURE.md) — current language and compiler status

---

## the thesis

the build tool will not produce a `target/` directory. it produces a single
compressed, cryptographically-sealed **virtual filesystem** — an `.ivlt` archive
that contains every intermediate artifact, every dependency, and every output.

this is not a tarball. it is a random-access, block-level, content-addressed,
streaming archive designed for a compiler build tool's access patterns: read one
file, read one function's lowered output, check whether anything changed since
last build, verify nothing is corrupt. the `target/` directory is a leaky
abstraction — every build tool that uses it spends engineering effort avoiding
its failure modes (stale artifacts, partial writes, hash mismatches, disk
pressure, network filesystem races). the archive is a single file. none of those
problems apply.

---

## what the C prototype proves

`vlt/` (at `/dev/vlt/`, outside workspace) is a single-file C archiver:

+ pack: reads a file, wraps it in a header with magic + metadata, writes `.ivlt`
+ unpack: validates magic, extracts payload
~ arena allocator: simple bump allocator with realloc growth

the C prototype validates the basic shape. the zig port will add:

- multi-entry archives (not just one file per archive)
- block-level structure (entries split into independently-hashed blocks)
- zstd compression per block
- blake3 hash chain for sealing
- streaming reads (no mmap of the entire archive)

the rust crate is the production version, built when the wire format is stable
after the zig port.

---

## design for the rust crate

### crate shape

```
crates/vfs/
├── Cargo.toml
└── src/
    ├── lib.rs          # public API: Archive, Entry, Reader, Writer
    ├── format.rs       # wire format: header, block index, hash chain
    ├── compress.rs     # compression: zstd codec, optional dict training
    ├── crypto.rs       # sealing: blake3 hash chain, optional ed25519 signing
    ├── stream.rs       # streaming entry reader: pull bytes for one path
    ├── embed.rs        # #[link_section] archive embedding + loader
    ├── repair.rs       # self-healing: entry reconstruction from hash chain
    └── fs.rs           # VfsFilesystem trait: lookup, read_entries, open_entry
```

### dependencies (cargo.toml sketch)

```toml
[dependencies]
blake3 = "1"          # content hashing, hash chain
zstd  = { version = "1", features = ["experimental"] }  # per-block compression
ed25519-dalek = { version = "2", optional = true }       # optional signing
rayon = { version = "1", optional = true }               # parallel verify/decompress
serde = { version = "1", features = ["derive"] }         # index serialization
serde_json = "1"                                          # manifest (human-readable)
text-size = { path = "../text-size" }                    # existing workspace dep
```

### the wire format

```
.ivlt archive
┌─────────────────────────────────┐
│ preamble                        │  ── magic (4 bytes), version (2 bytes),
│                                 │     flags (2 bytes), manifest hash (32 bytes)
├─────────────────────────────────┤
│ manifest                        │  ── serialized index tree (json or bincode).
│                                 │     maps path → [block_hash; N].
│                                 │     each block_hash is blake3 of compressed data.
│                                 │     signed when SEALED flag is set.
├─────────────────────────────────┤
│ block index                     │  ── sorted array of (block_hash, offset, length)
├─────────────────────────────────┤
│ block data (zstd-compressed)    │  ── 64 KiB uncompressed per block (or smaller
│ block data                      │     for the last block of an entry). stored in
│ ...                             │     block-index order.
└─────────────────────────────────┘
```

key design rules:

- **content-addressed blocks** — every block is identified by `blake3(compressed_block)`. duplicate content across entries deduplicates automatically at the block level. changing one byte of one file changes exactly one block's hash.
- **manifest is separate** — the manifest (path → block list) is not content-addressed. it is the root of trust. the preamble stores its hash. the manifest can be signed (ed25519) for distribution.
- **no inline metadata** — file name, permissions, timestamps live in the manifest. the block layer knows only hashes and byte counts. this is what makes the block layer content-addressed: a block does not know which file it belongs to.
- **appends only** — the archive is never modified in place. the build tool reads the old archive, writes a new one with changed entries. unchanged blocks are not recompressed (their hashes prove they are identical). the preamble hash changes only because the manifest changed.

### the hash chain (self-healing)

each block carries its hash in the block index. the block index is hashed into
the manifest. the manifest hash is in the preamble. this forms a hash chain:

```
preamble.hash = blake3(manifest)
  manifest contains block_hashes for every entry
    each block_hash = blake3(compressed_block)
```

on read, every block is verified against its hash before decompression. if a
block fails (bit rot, tamper, incomplete write):

1. the error is specific: "entry `foo.eye`, block 3 of 7 hash mismatch"
2. if the build tool tracks the previous archive version, the block can be
   reconstructed from the prior manifest's block chain (same path, prior hash)
3. if no prior archive exists, the entry is reported as corrupt — the compiler
   never serves garbage

this is what makes the archive **self-healing**: any block can be located in any
prior version by walking the hash chain backward through the archive lineage.

### streaming reads

the reader API is designed for lazy access:

```rust
impl ArchiveReader {
    /// opens an archive. reads the preamble + manifest immediately.
    /// does NOT read any block data.
    pub fn open(path: &Path) -> Result<Self>;

    /// returns metadata about an entry (size, block count, compression ratio)
    /// without reading any block data.
    pub fn stat(&self, path: &str) -> Result<EntryInfo>;

    /// reads the entire entry. decompresses every block.
    pub fn read(&self, path: &str) -> Result<Vec<u8>>;

    /// returns a streaming reader. blocks are decompressed on demand.
    /// the caller can read byte ranges (seek) without decompressing
    /// blocks outside the range.
    pub fn open_entry(&self, path: &str) -> Result<EntryReader>;
}
```

`EntryReader` implements `Read + Seek`:

```rust
impl EntryReader {
    /// which byte offset is this block responsible for?
    /// we know from the block index + uncompressed size per block.
    fn block_for(offset: u64) -> usize;

    /// decompress the block into the internal cache. caller may request
    /// bytes from a block that has not been decompressed yet — we
    /// decompress on first touch.
    fn ensure_block(&mut self, block_idx: usize) -> Result<()>;
}
```

the streaming API is what makes `import "std/io.eye"` possible without
decompressing the entire stdlib archive. the compiler reads the header, resolves
the import, opens the entry, seeks to the function body, and reads only the
blocks that cover it.

### embedded archive loader

the standard library archive can be linked into the `eye` binary:

```rust
// in crates/vfs/src/embed.rs

/// link an archive into the binary via include_bytes!
/// the ARCHIVE static is placed in its own section so the linker can
/// discard it if unreferenced (─ffunction-sections + ─gc-sections).
#[macro_export]
macro_rules! embed_archive {
    ($path:expr) => {
        #[used]
        #[link_section = "__eye_archive"]
        static ARCHIVE: &[u8] = include_bytes!($path);
    };
}

/// wraps an embedded `&[u8]` as an ArchiveReader.
/// zero heap allocation for the preamble + manifest (parsed in place or
/// deserialized, then cached).
pub struct EmbeddedArchive {
    data: &'static [u8],
    // parsed lazily
    manifest: OnceCell<Manifest>,
    block_index: OnceCell<Vec<BlockEntry>>,
}
```

the embedded archive initializes once at first use. after that, `import "std/"`
resolves to a `&[u8]` slice that is already in the process address space — zero
system calls, zero page faults after the first touch.

### caching layer

the VFS crate provides a caching reader wrapper:

```rust
pub struct VfsCache {
    inner: Box<dyn ArchiveReader>,
    // recently-read entries: path → decompressed bytes
    entries: LruCache<String, Arc<[u8]>>,
    // recently-read blocks: block_hash → decompressed bytes
    blocks: LruCache<[u8; 32], Arc<[u8]>>,
}
```

the LSP's hot loop hits this cache for every open file. the LRU size is tuned
to hold every file in the active workspace (typically <100 files, <50 MB).
cache misses cost one block decompression (sub-microsecond for zstd on 64 KiB).

### repair / self-healing

```rust
impl ArchiveReader {
    /// verify every block in the archive.
    /// returns a list of corrupt entries (block hash mismatch).
    pub fn verify(&self) -> Result<Vec<CorruptEntry>>;

    /// attempt to repair a corrupt entry by scanning prior archive versions.
    pub fn repair(
        &mut self,
        entry: &str,
        history: &[ArchiveReader],
    ) -> Result<()>;

    /// rebuild the archive: recompress every entry, verify all hashes.
    /// produces a new archive file. the old one is preserved.
    pub fn rebuild(&self, output: &Path) -> Result<()>;
}
```

the build tool archives every version it produces. when a `verify()` finds a
corrupt entry, it walks the version history backward, finds the last uncorrupt
copy of that entry's block chain, and swaps the corrupt blocks. if no prior
version exists, the entry is marked corrupt and rebuilt from source.

### integration with the compiler

the VFS crate exposes a single trait that the compiler (and LSP) use:

```rust
/// the interface the compiler needs from its filesystem:
/// look up a source file, read its contents.
pub trait VfsFilesystem {
    /// resolve a path (relative or absolute) to an entry in the archive.
    fn resolve(&self, path: &str) -> Result<Entry>;

    /// read the entire contents of an entry.
    fn read_entry(&self, entry: &Entry) -> Result<Vec<u8>>;

    /// list all entries under a prefix (for workspace symbol index).
    fn list_prefix(&self, prefix: &str) -> Result<Vec<String>>;

    /// the underlying archive reader, for advanced use.
    fn archive(&self) -> &dyn ArchiveReader;
}
```

the compiler's source manager wraps this trait. the LSP's `DocumentStore` reads
from it. when the archive is embedded, `resolve("std/print.eye")` reads from a
`&[u8]` in the binary. when the archive is on disk, it decompresses the
relevant blocks. neither path writes anything to a `target/` directory.

---

## features vs non-features

### what the VFS is

- a read-only random-access archive format
- content-addressed at the block level (deduplication, incremental rebuild)
- cryptographically sealed (blake3 hash chain, optional ed25519 signing)
- streaming (read one entry without decompressing the entire archive)
- self-healing (reconstruct corrupt blocks from prior versions)
- embeddable (stdlib archive linked into the compiler binary)
- append-only (never mutates in place; produces new versions)

### what the VFS is not

- not a package manager — packages are fetched and stored separately. the VFS
  is the local cache format, not the distribution format.
- not a database — the manifest is a flat map, not a query engine. the compiler
  does not run SQL against it.
- not a general-purpose filesystem — no directories, no permissions, no symlinks,
  no hard links. entries are flat paths with forward-slash separators.
  `std/io/print.eye` is a path string, not a directory tree.
- not a build graph — the archive stores artifacts. dependency tracking happens
  in the build tool's scheduler, not in the archive format.
- not a network protocol — the VFS is a local file format. fetching remote
  packages is the package manager's job.

---

## migration path (for reference; not building yet)

the rust VFS crate will be built in stages:

| stage | scope | ships with |
|---|---|---|
| M1 — flat file manager | `SourceManager` that reads from disk. no archive format. plain `std::fs`. | phase 0.5 of LSP |
| M2 — archive reader | `ArchiveReader` trait + `SimpleArchive` that wraps gzipped tarball. validates the trait API. | phase 2 of LSP (needs multi-file) |
| M3 — ivlt reader | `IvltArchive` implementing the wire format above. read-only. | phase 5 (needs streaming libs) |
| M4 — ivlt writer | build tool produces `.ivlt` archives. incremental rebuild (reuse unchanged blocks). | build tool initial version |
| M5 — embedding | `EmbeddedArchive` + `embed_archive!` macro. stdlib shipped inside the binary. | stdlib extraction phase |

each stage is independently useful and replaces the prior stage transparently
(the trait bounds are the same).

---

## why this beats a `target/` directory

| concern | `target/` | `.ivlt` archive |
|---|---|---|
| stale artifacts | every build tool has a cache-invalidation bug | append-only; old versions are never mutated |
| partial writes | crash during write → corrupt artifact | crash during write → old archive is intact |
| disk pressure | `target/` grows without bound; user must `cargo clean` periodically | a single file; delete old versions or keep them for self-healing |
| network filesystem races | concurrent reads/writes produce hard-to-debug failures | read-only at rest; one writer produces one file atomically |
| incremental rebuild | hash every file on disk, compare against stored hashes | block hashes are in the manifest; read the block index (44 bytes per block), compare against previous manifest |
| binary distribution | zip/tar the `target/` directory (includes dead artifacts) | ship the archive; it contains exactly what is needed |
| corruption detection | silent: clang may produce wrong output from a corrupted .o | every block verified before use; self-healing available |
| library streaming | must decompress or download the entire library | blocks are independent; fetch/decompress only the ones covering the used function |
