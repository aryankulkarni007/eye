//! source-text management: [`SourceText`] owns the input (a heap [`String`] or
//! an `mmap`, via [`SourceHolder`]) and answers byte-offset -> [`LineCol`] /
//! slice queries; [`SourceFile`] bundles a source with its [`Interner`] (the
//! single-file precursor to a multi-file source cache).

use memchr::memchr_iter;
use memmap2::Mmap;
use smol_str::SmolStr;
use text_size::{TextRange, TextSize};

use syntax::StringTable;

use crate::Interner;

/// a one-based source position: `line` and `col` (a byte offset from the line
/// start, not a character count). both are `u32` rather than `usize` to halve
/// the struct's size; this caps a source file at ~4 billion lines and ~4 billion
/// bytes per line, far beyond any real input.
#[derive(Debug)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

#[derive(Debug)]
pub enum SourceHolder {
    Owned(String),
    Mmap(Mmap),
}

impl SourceHolder {
    #[inline(always)]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            SourceHolder::Owned(s) => s.as_bytes(),
            SourceHolder::Mmap(m) => m,
        }
    }
}

impl std::ops::Deref for SourceHolder {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        match self {
            SourceHolder::Owned(s) => s.as_bytes(),
            SourceHolder::Mmap(m) => m.as_ref(),
        }
    }
}

#[derive(Debug)]
pub struct SourceText {
    pub source: SourceHolder,
    pub lstart: Vec<usize>,
}

/// calculates lstarts using memchr_iter (SIMD)
fn lstarts(bytes: &[u8]) -> Vec<usize> {
    let mut lstart = Vec::with_capacity(bytes.len() / 40);
    lstart.push(0);

    for n_pos in memchr_iter(b'\n', bytes) {
        lstart.push(n_pos + 1);
    }
    lstart
}

impl SourceText {
    #[inline(always)]
    pub fn as_bytes(&self) -> &[u8] {
        self.source.as_bytes()
    }

    /// create from an mmap. returns an error when the file is not valid
    /// UTF-8 - the user sees a graceful diagnostic rather than a panic.
    /// both constructors validate UTF-8 so that [`SourceText::as_str`] is
    /// safe (the `unsafe` call is justified by construction).
    pub fn from_mmap(mmap: Mmap) -> Result<Self, std::str::Utf8Error> {
        std::str::from_utf8(&mmap)?;
        let lstart = lstarts(&mmap);
        Ok(SourceText {
            source: SourceHolder::Mmap(mmap),
            lstart,
        })
    }

    /// create from a string (tests/internal)
    pub fn new(content: String) -> Self {
        // we scan the string bytes directly
        let lstart = lstarts(content.as_bytes());
        SourceText {
            source: SourceHolder::Owned(content),
            lstart,
        }
    }

    #[inline(always)]
    pub fn as_str(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(&self.source) }
    }

    pub fn len(&self) -> usize {
        self.source.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.source.len() == 0
    }

    /// converts a byte offset to one-based line and col
    pub fn line_col(&self, offset: TextSize) -> LineCol {
        let offset = usize::from(offset);
        assert!(offset <= self.source.len(), "offset out of bounds");
        let line = self.lstart.partition_point(|&start| start <= offset) - 1;
        let col = offset - self.lstart[line];
        LineCol {
            line: (line + 1) as u32,
            col: (col + 1) as u32,
        }
    }

    /// converts a byte offset to a one-based line and a one-based column
    /// counted in UTF-16 code units - the LSP default position encoding.
    /// [`Self::line_col`] reports byte columns; an LSP payload built from
    /// those mis-places every position after a multibyte character on the
    /// same line. `offset` must lie on a `char` boundary (token and node
    /// ranges always do).
    pub fn line_col_utf16(&self, offset: TextSize) -> LineCol {
        let offset = usize::from(offset);
        assert!(offset <= self.source.len(), "offset out of bounds");
        let line = self.lstart.partition_point(|&start| start <= offset) - 1;
        let prefix = &self.as_str()[self.lstart[line]..offset];
        let col: usize = prefix.chars().map(char::len_utf16).sum();
        LineCol {
            line: (line + 1) as u32,
            col: (col + 1) as u32,
        }
    }

    /// converts a zero-based line + zero-based UTF-16 column (the LSP position
    /// encoding) to a byte offset - the inverse of [`Self::line_col_utf16`]. a
    /// column past the line's end clamps to the line end; an out-of-range line
    /// clamps to end-of-source. used by the hover handler to resolve a cursor
    /// position to an expression.
    pub fn offset_utf16(&self, line: u32, col_utf16: u32) -> TextSize {
        let len = self.source.len();
        let Some(&line_start) = self.lstart.get(line as usize) else {
            return TextSize::new(len as u32);
        };
        let line_end = self.lstart.get(line as usize + 1).copied().unwrap_or(len);
        let target = col_utf16 as usize;
        let mut u16_seen = 0usize;
        let mut byte = line_start;
        for ch in self.as_str()[line_start..line_end].chars() {
            if u16_seen >= target {
                break;
            }
            u16_seen += ch.len_utf16();
            byte += ch.len_utf8();
        }
        TextSize::new(byte as u32)
    }

    /// the source text a [`TextRange`] covers, or `None` if it is out of
    /// bounds or not on `char` boundaries.
    pub fn slice(&self, range: TextRange) -> Option<&str> {
        self.as_str()
            .get(usize::from(range.start())..usize::from(range.end()))
    }
}

/// per-source-file context bundling the source text and the
/// lexer's string table. this is the single-file precursor to a multi-file
/// `SourceCache` (QUERY.md) that would map `FileId → SourceFile`.
///
/// the `StringTable` impl delegates to the inner [`Interner`], so HIR lowering
/// can request canonical strings without knowing which concrete type owns the
/// table -- crucial when the source comes from a project database rather than a
/// single invocation.
#[derive(Debug)]
pub struct SourceFile {
    pub text: SourceText,
    pub interner: Interner,
}

impl SourceFile {
    pub fn new(text: SourceText, interner: Interner) -> Self {
        Self { text, interner }
    }
}

impl StringTable for SourceFile {
    fn get(&self, s: &str) -> Option<SmolStr> {
        self.interner.get(s)
    }
}
