//! The parser event stream.
//!
//! The parser never builds a tree. It emits a flat, append-only buffer of
//! [`Event`]s - the rust-analyzer model. A second pass ([`build_tree`]) walks
//! the events alongside the token stream and drives `rowan::GreenNodeBuilder`
//! to produce a lossless concrete syntax tree.
//!
//! ## Allocation budget
//!
//! The event stream is a single [`Vec<Event>`], preallocated with enough
//! headroom that typical input never reallocates; past that it grows only by
//! amortized doubling. The real guarantee is per-item: [`Event`] is `Copy` POD
//! - no `String`/`Box`/`Vec` inside any variant, so no event ever allocates.
//!   Diagnostic messages live out-of-band in a sibling [`Vec<ParseError>`] and
//!   are `&'static str`; events carry only an [`ErrorIdx`]. [`Marker`] open,
//!   complete and abandon all mutate the buffer in place.

use std::cell::Cell;
use std::num::NonZeroU32;

use drop_bomb::DropBomb;
use rowan::{GreenNodeBuilder, Language};
use smallvec::SmallVec;
use thin_vec::ThinVec;

use text_size::TextRange;

use lexer::SourceText;
use syntax::{EyeLang, SyntaxKind, SyntaxNode};
use token::Token;

mod grammar;

/// Index into the sibling [`Vec<ParseError>`]. Keeps [`Event`] POD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorIdx(u32);

/// A parse event. `Copy` and pointer-free so the whole stream is one flat
/// buffer of POD slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    /// Start an internal node. `fwd_parent`, when set, is the event index of
    /// a later `Open` that should become this node's parent - the retroactive
    /// wrap produced by [`CompletedMarker::precede`].
    Open {
        kind: SyntaxKind,
        fwd_parent: Option<NonZeroU32>,
    },
    /// Finish the most recently started node.
    Close,
    /// Consume one non-trivia token from the stream.
    Advance,
    /// A diagnostic was recorded; no effect on tree shape.
    Error(ErrorIdx),
    /// An abandoned `Open`, or an `Open` already consumed as a forward
    /// parent. Skipped by [`build_tree`].
    Tombstone,
}

/// A parser diagnostic. `msg` is `&'static str` so [`ParseError`] never
/// allocates.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub msg: &'static str,
    pub range: TextRange,
}

/// The finished parse: a lossless CST plus every diagnostic.
pub struct Parse {
    pub green: SyntaxNode,
    pub errors: ThinVec<ParseError>,
}

/// How many lookahead probes the parser may make without consuming a token
/// before [`Parser::nth`] panics - a guard against non-progressing grammar
/// loops.
const FUEL: u32 = 256;

pub struct Parser<'t> {
    /// The full lexer output, trivia included.
    tokens: &'t [Token],
    /// Raw index of the next non-trivia token (the parser's logical cursor).
    pos: usize,
    events: Vec<Event>,
    errors: ThinVec<ParseError>,
    /// Reset on every [`advance`](Parser::advance); decremented on every
    /// lookahead. Hitting zero means the grammar is spinning.
    fuel: Cell<u32>,
}

impl<'t> Parser<'t> {
    fn new(tokens: &'t [Token]) -> Self {
        let mut p = Parser {
            tokens,
            pos: 0,
            // events run a small constant factor over the token count
            // (Advance per token, Open + Close per node); 2x is generous
            // headroom so typical input fills this without reallocating
            events: Vec::with_capacity(tokens.len() * 2),
            errors: ThinVec::new(),
            fuel: Cell::new(FUEL),
        };
        p.pos = p.skip_trivia(0);
        p
    }

    /// First non-trivia raw index at or after `idx`.
    fn skip_trivia(&self, mut idx: usize) -> usize {
        while idx < self.tokens.len() {
            let kind: SyntaxKind = self.tokens[idx].kind.into();
            if !kind.is_trivia() {
                break;
            }
            idx += 1;
        }
        idx
    }

    /// Kind of the token under the cursor.
    pub fn nth0(&self) -> SyntaxKind {
        self.nth(0)
    }

    /// Kind of the `n`-th non-trivia token ahead of the cursor. Past the end
    /// of the stream this is always [`SyntaxKind::Eof`].
    pub fn nth(&self, n: usize) -> SyntaxKind {
        let fuel = self.fuel.get();
        assert!(fuel != 0, "parser ran out of fuel - non-progressing loop");
        self.fuel.set(fuel - 1);

        let mut idx = self.pos;
        let mut remaining = n;
        loop {
            if idx >= self.tokens.len() {
                return SyntaxKind::Eof;
            }
            let kind: SyntaxKind = self.tokens[idx].kind.into();
            if kind.is_trivia() {
                idx += 1;
                continue;
            }
            if remaining == 0 {
                return kind;
            }
            remaining -= 1;
            idx += 1;
        }
    }

    pub fn at(&self, kind: SyntaxKind) -> bool {
        self.nth(0) == kind
    }

    pub fn at_eof(&self) -> bool {
        self.at(SyntaxKind::Eof)
    }

    /// Range of the token under the cursor - the anchor for diagnostics.
    fn cursor_range(&self) -> TextRange {
        self.tokens[self.pos.min(self.tokens.len() - 1)].range
    }

    /// Consume the current non-trivia token.
    pub fn advance(&mut self) {
        debug_assert!(!self.at_eof(), "advance past Eof");
        self.events.push(Event::Advance);
        self.pos = self.skip_trivia(self.pos + 1);
        self.fuel.set(FUEL);
    }

    /// Consume the current token iff it is `kind`.
    pub fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Consume `kind`, or record a diagnostic without consuming anything.
    pub fn expect(&mut self, kind: SyntaxKind, msg: &'static str) {
        if !self.eat(kind) {
            self.error(msg);
        }
    }

    /// Record a diagnostic anchored at the cursor. Emits an [`Event::Error`]
    /// but does not move the cursor.
    pub fn error(&mut self, msg: &'static str) {
        let idx = ErrorIdx(self.errors.len() as u32);
        self.errors.push(ParseError {
            msg,
            range: self.cursor_range(),
        });
        self.events.push(Event::Error(idx));
    }

    /// Recovery: wrap the unexpected current token in an `ErrorNode` and
    /// record `msg`. Always makes progress.
    pub fn error_and_advance(&mut self, msg: &'static str) {
        let m = self.open();
        self.error(msg);
        self.advance();
        m.complete(self, SyntaxKind::ErrorNode);
    }

    /// Start a node. Returns a [`Marker`] that *must* be completed or
    /// abandoned - its [`DropBomb`] enforces this.
    pub fn open(&mut self) -> Marker {
        let idx = self.events.len() as u32;
        self.events.push(Event::Tombstone);
        Marker {
            idx,
            bomb: DropBomb::new("Marker dropped without complete()/abandon()"),
        }
    }

    fn finish(self) -> (Vec<Event>, ThinVec<ParseError>) {
        (self.events, self.errors)
    }
}

/// An open node. The [`DropBomb`] panics if the marker is dropped without
/// being completed or abandoned, catching unbalanced grammar code.
#[must_use]
pub struct Marker {
    idx: u32,
    bomb: DropBomb,
}

impl Marker {
    /// Turn the placeholder at `idx` into a real `Open` node and close it.
    pub fn complete(self, p: &mut Parser, kind: SyntaxKind) -> CompletedMarker {
        let Marker { idx, mut bomb } = self;
        bomb.defuse();
        p.events[idx as usize] = Event::Open {
            kind,
            fwd_parent: None,
        };
        p.events.push(Event::Close);
        CompletedMarker { idx }
    }

    /// Discard the node. Any events emitted since `open()` become children of
    /// the surrounding node instead.
    pub fn abandon(self, p: &mut Parser) {
        let Marker { idx, mut bomb } = self;
        bomb.defuse();
        if idx as usize == p.events.len() - 1 {
            p.events.pop();
        } else {
            p.events[idx as usize] = Event::Tombstone;
        }
    }
}

/// A completed node, the handle [`precede`](CompletedMarker::precede) needs to
/// retroactively wrap it in a parent.
#[derive(Clone, Copy)]
pub struct CompletedMarker {
    idx: u32,
}

impl CompletedMarker {
    /// Open a fresh node *before* this one and make this completed node its
    /// first child. This is how postfix/precedence forms wrap a node already
    /// emitted to the stream - without shifting the buffer.
    pub fn precede(self, p: &mut Parser) -> Marker {
        let m = p.open();
        match &mut p.events[self.idx as usize] {
            Event::Open { fwd_parent, .. } => {
                *fwd_parent = Some(NonZeroU32::new(m.idx).expect("preceded node is the root"));
            }
            _ => unreachable!("precede() on a non-Open event"),
        }
        m
    }
}

/// Parse a token stream into a lossless CST.
pub fn parse(tokens: &[Token], source: &SourceText) -> Parse {
    let mut p = Parser::new(tokens);
    crate::grammar::source_file(&mut p);
    let (events, errors) = p.finish();
    let green = build_tree(tokens, events, source);
    // the CST is lossless: it must reproduce the source byte-for-byte
    debug_assert_eq!(
        green.to_string(),
        source.as_str(),
        "CST round-trip mismatch - build_tree dropped or duplicated text"
    );
    Parse { green, errors }
}

/// Walks the event stream and the raw token stream together, driving a
/// `GreenNodeBuilder`. Trivia tokens are interleaved back in here so the tree
/// round-trips to the original source.
fn build_tree(tokens: &[Token], mut events: Vec<Event>, source: &SourceText) -> SyntaxNode {
    let mut builder = GreenNodeBuilder::new();
    let mut raw = 0usize;
    // scratch reused across every forward-parent chain - inline storage
    // sized for the precede-chain depth typical exprs hit, so a node's
    // forward-parent walk never allocates
    let mut parents: SmallVec<[SyntaxKind; 4]> = SmallVec::new();

    let emit_trivia = |builder: &mut GreenNodeBuilder, raw: &mut usize| {
        while *raw < tokens.len() {
            let tok = tokens[*raw];
            let kind: SyntaxKind = tok.kind.into();
            if !kind.is_trivia() {
                break;
            }
            // a token span is always a valid slice of its own source; a
            // failure here means the token stream and source disagree
            let text = source.slice(tok.range).expect("token range outside source");
            builder.token(EyeLang::kind_to_raw(kind), text);
            *raw += 1;
        }
    };

    for i in 0..events.len() {
        match std::mem::replace(&mut events[i], Event::Tombstone) {
            Event::Open { kind, fwd_parent } => {
                // gather this node and every forward parent, outermost last
                parents.clear();
                parents.push(kind);
                let mut next = fwd_parent;
                while let Some(idx) = next {
                    match std::mem::replace(&mut events[idx.get() as usize], Event::Tombstone) {
                        Event::Open { kind, fwd_parent } => {
                            parents.push(kind);
                            next = fwd_parent;
                        }
                        _ => unreachable!("fwd_parent points at a non-Open event"),
                    }
                }
                // start the outermost parent first
                for &kind in parents.iter().rev() {
                    builder.start_node(EyeLang::kind_to_raw(kind));
                }
            }
            Event::Close => {
                // flush trailing trivia into the closing node before it ends
                emit_trivia(&mut builder, &mut raw);
                builder.finish_node();
            }
            Event::Advance => {
                emit_trivia(&mut builder, &mut raw);
                let tok = tokens[raw];
                let kind: SyntaxKind = tok.kind.into();
                let text = source.slice(tok.range).expect("token range outside source");
                builder.token(EyeLang::kind_to_raw(kind), text);
                raw += 1;
            }
            Event::Error(_) | Event::Tombstone => {}
        }
    }

    SyntaxNode::new_root(builder.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexer::{Lexer, SourceText};

    /// Lex + parse `src` into a [`Parse`].
    fn parse_src(src: &str) -> Parse {
        let source = SourceText::new(src.to_string());
        let tokens = Lexer::new(&source).tokenize().tokens;
        parse(&tokens, &source)
    }

    /// A program exercising every v0.1 node kind.
    const SAMPLE: &str = "\
structure Point {
    int32 x,
    int32 y,
};

main() {
    const x = 0;
    var Point p = Point { x, y };
    print(\"{}\", p.x);
}
";

    #[test]
    fn well_formed_input_has_no_errors() {
        let parse = parse_src(SAMPLE);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    }

    #[test]
    fn cst_round_trips_to_source() {
        // the CST is lossless - it reproduces the input bytes exactly
        let parse = parse_src(SAMPLE);
        assert_eq!(parse.green.to_string(), SAMPLE);
    }

    #[test]
    fn malformed_input_still_produces_a_tree() {
        // a struct missing its name is recovered, not fatal
        let parse = parse_src("structure { };");
        assert!(!parse.errors.is_empty(), "expected a diagnostic");
        // the tree still round-trips even through error recovery
        assert_eq!(parse.green.to_string(), "structure { };");
    }

    #[test]
    fn operator_expr_parses_clean_and_round_trips() {
        // a mix of arithmetic, comparison, logical and prefix operators
        let src = "main() {\n    const x = -1 + 2 * 3 == 7 && a || b;\n}\n";
        let parse = parse_src(src);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn struct_lit_shorthand_form_parses_clean() {
        let src = "main() {\n    var Point p = Point { x, y };\n}\n";
        let parse = parse_src(src);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn struct_lit_explicit_form_parses_clean() {
        let src = "main() {\n    var Point p = Point { x: x, y: y };\n}\n";
        let parse = parse_src(src);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn struct_lit_mixed_forms_parses_clean() {
        let src = "main() {\n    var Point p = Point { x, y: 0 };\n}\n";
        let parse = parse_src(src);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn field_access_expression_parses_clean_and_round_trips() {
        let src = "main() {\n    print(\"{}\", p.x);\n    print(\"{}\", p.y);\n}\n";
        let parse = parse_src(src);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn nested_field_access_parses_clean_and_round_trips() {
        // Chained `a.b.c` exercises the postfix loop in `lhs`, producing
        // `FieldExpr(FieldExpr(a, b), c)` rather than two siblings.
        let src = "main() {\n    print(\"{}\", a.b.c);\n}\n";
        let parse = parse_src(src);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn cst_snapshot() {
        let parse = parse_src(SAMPLE);
        insta::assert_snapshot!(format!("{:#?}", parse.green));
    }
}
