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
//!   Typed diagnostics live out-of-band in a sibling [`Sink<ParseError>`];
//!   events carry only a [`DiagnosticIdx`]. [`Marker`] open, complete and
//!   abandon all mutate the buffer in place.

use std::cell::Cell;
use std::num::NonZeroU32;

use drop_bomb::DropBomb;
use rowan::{GreenNodeBuilder, Language};
use smallvec::SmallVec;

use text_size::TextRange;

use diagnostics::Sink;
use lexer::SourceText;
use syntax::{EyeLang, SyntaxKind, SyntaxNode};
use token::Token;

mod errors;
mod grammar;

pub use errors::{GrammarError, ParseError, SyntaxError};

/// Index into the sibling [`Sink<ParseError>`]. Keeps [`Event`] POD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticIdx(u32);

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
    Diagnostic(DiagnosticIdx),
    /// An abandoned `Open`, or an `Open` already consumed as a forward
    /// parent. Skipped by [`build_tree`].
    Tombstone,
}

/// The finished parse: a lossless CST plus every diagnostic.
pub struct Parse {
    pub green: SyntaxNode,
    pub diagnostics: Sink<ParseError>,
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
    diagnostics: Sink<ParseError>,
    /// Reset on every [`advance`](Parser::advance); decremented on every
    /// lookahead. Hitting zero means the grammar is spinning.
    fuel: Cell<u32>,
    /// When true, the postfix `{ ... }` form does not start a struct literal.
    /// Set inside `if`/`loop` conditions to disambiguate `if x { ... }` from
    /// `if x_struct_lit_then_block`. Restored to its prior value on entry to
    /// any inner parenthesised context so `if foo(Bar { x }) { ... }` still
    /// parses the inner literal.
    no_struct_lit: Cell<bool>,
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
            diagnostics: Sink::new(),
            fuel: Cell::new(FUEL),
            no_struct_lit: Cell::new(false),
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
    pub fn expect(&mut self, kind: SyntaxKind, err: impl Into<ParseError>) {
        if !self.eat(kind) {
            self.error(err);
        }
    }

    /// Record a diagnostic anchored at the cursor. Emits an [`Event::Diagnostic`]
    /// but does not move the cursor.
    pub fn error(&mut self, err: impl Into<ParseError>) {
        self.error_at(self.cursor_range(), err);
    }

    /// Record a diagnostic anchored at a specific range - used when the relevant
    /// span is a node already consumed (e.g. an assignment in an `if`
    /// condition), not the current cursor.
    pub fn error_at(&mut self, range: TextRange, err: impl Into<ParseError>) {
        let idx = DiagnosticIdx(self.diagnostics.len() as u32);
        self.diagnostics.emit(range, err.into());
        self.events.push(Event::Diagnostic(idx));
    }

    /// Recovery: wrap the unexpected current token in an `ErrorNode` and
    /// record `err`. Always makes progress.
    pub fn error_and_advance(&mut self, err: impl Into<ParseError>) {
        let m = self.open();
        self.error(err);
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

    fn finish(self) -> (Vec<Event>, Sink<ParseError>) {
        (self.events, self.diagnostics)
    }

    /// Suppress (or re-enable) struct-literal recognition by the postfix loop.
    /// Returns the previous value so the caller can restore it - use the RAII
    /// pattern `let prev = p.set_no_struct_lit(true); ...; p.set_no_struct_lit(prev);`.
    pub(crate) fn set_no_struct_lit(&self, v: bool) -> bool {
        self.no_struct_lit.replace(v)
    }

    /// True if struct-literal postfix is currently suppressed.
    pub(crate) fn no_struct_lit(&self) -> bool {
        self.no_struct_lit.get()
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
        CompletedMarker { idx, kind }
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
    kind: SyntaxKind,
}

impl CompletedMarker {
    /// The [`SyntaxKind`] this node was completed as.
    pub(crate) fn kind(self) -> SyntaxKind {
        self.kind
    }

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
    let (events, diagnostics) = p.finish();
    let green = build_tree(tokens, events, source);
    // the CST is lossless: it must reproduce the source byte-for-byte
    debug_assert_eq!(
        green.to_string(),
        source.as_str(),
        "CST round-trip mismatch - build_tree dropped or duplicated text"
    );
    Parse { green, diagnostics }
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
            Event::Diagnostic(_) | Event::Tombstone => {}
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
    let x = 0;
    mut Point p = Point { x, y };
    print(\"{}\", p.x);
}
";

    #[test]
    fn well_formed_input_has_no_diagnostics() {
        let parse = parse_src(SAMPLE);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
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
        assert!(!parse.diagnostics.is_empty(), "expected a diagnostic");
        // the tree still round-trips even through error recovery
        assert_eq!(parse.green.to_string(), "structure { };");
    }

    #[test]
    fn operator_expr_parses_clean_and_round_trips() {
        // a mix of arithmetic, comparison, logical and prefix operators
        let src = "main() {\n    let x = -1 + 2 * 3 == 7 && a || b;\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn struct_lit_shorthand_form_parses_clean() {
        let src = "main() {\n    mut Point p = Point { x, y };\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn struct_lit_explicit_form_parses_clean() {
        let src = "main() {\n    mut Point p = Point { x: x, y: y };\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn struct_lit_mixed_forms_parses_clean() {
        let src = "main() {\n    mut Point p = Point { x, y: 0 };\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn v06_operators_parse_clean_and_round_trip() {
        // modulo, bitwise binary, prefix complement/not, compound assignment
        let src = "main() {\n    mut int32 c = 1 % 2 & 3 | 4 ^ 5 << 6 >> 7;\n    \
                   c += ~c;\n    c -= !c;\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn paren_group_parses_clean_and_round_trips() {
        let src = "main() {\n    let int32 r = a * (b + c);\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// F1: comparison operators are non-associative. `a < b < c` is rejected
    /// (it would silently be `(a < b) < c` = `bool < c` in C). The tree still
    /// round-trips - the error is a diagnostic, not a parse bail.
    #[test]
    fn comparison_chaining_is_rejected() {
        let src = "main() {\n    let bool r = a < b < c;\n}\n";
        let parse = parse_src(src);
        assert!(
            parse
                .diagnostics
                .entries()
                .iter()
                .any(|(_, d)| matches!(d, ParseError::Grammar(GrammarError::ComparisonChain))),
            "expected a non-associativity diagnostic, got: {:?}",
            parse.diagnostics
        );
        assert_eq!(parse.green.to_string(), src);
    }

    /// A single comparison, and comparisons joined by `&&`, are fine.
    #[test]
    fn single_and_logically_joined_comparisons_are_clean() {
        let src = "main() {\n    let bool r = a < b && c < d;\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
    }

    /// `if x = y { }` is the `=`/`==` footgun: an assignment in a condition is
    /// rejected as a grammar error. Recovery still round-trips the source.
    #[test]
    fn assignment_in_if_condition_is_rejected() {
        let src = "main() {\n    if x = y {\n    };\n}\n";
        let parse = parse_src(src);
        assert!(
            parse
                .diagnostics
                .entries()
                .iter()
                .any(|(_, d)| matches!(d, ParseError::Grammar(GrammarError::AssignInIfCondition))),
            "expected an assign-in-if-condition diagnostic, got: {:?}",
            parse.diagnostics
        );
        assert_eq!(parse.green.to_string(), src);
    }

    /// `==` in a condition is the intended compare and must stay clean.
    #[test]
    fn equality_in_if_condition_is_clean() {
        let src = "main() {\n    if x == y {\n    };\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
    }

    #[test]
    fn field_access_expression_parses_clean_and_round_trips() {
        let src = "main() {\n    print(\"{}\", p.x);\n    print(\"{}\", p.y);\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn nested_field_access_parses_clean_and_round_trips() {
        // Chained `a.b.c` exercises the postfix loop in `lhs`, producing
        // `FieldExpr(FieldExpr(a, b), c)` rather than two siblings.
        let src = "main() {\n    print(\"{}\", a.b.c);\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn cst_snapshot() {
        let parse = parse_src(SAMPLE);
        insta::assert_snapshot!(format!("{:#?}", parse.green));
    }

    // ---------- v0.2 grammar coverage ----------

    /// `add(int32 a, int32 b) -> int32 { a + b }` - comma-separated params,
    /// `->` return type, and a block whose body is a single tail expression.
    #[test]
    fn fn_def_with_return_type_and_tail_expr() {
        let src = "add(int32 a, int32 b) -> int32 { a + b }\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    #[test]
    fn empty_param_list_still_parses() {
        let src = "main() {\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
    }

    /// Waterfall enum body: `enum Shape = | A | B | C ;`.
    #[test]
    fn enum_def_waterfall_form_parses_clean() {
        let src = "enum Shape =\n| Square\n| Circle\n| Triangle\n;\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// `mut &Point pt_ref = &pt;` - reference type annotation plus address-of
    /// prefix expression on the right-hand side.
    #[test]
    fn ref_type_and_ref_expr_parse_clean() {
        let src = "main() {\n    mut pt = Point { 10, 20 };\n    mut &Point pt_ref = &pt;\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// `*p` as a prefix expression - the deref form mirrors `&p`.
    #[test]
    fn deref_expr_parses_clean() {
        let src = "main() {\n    mut x = *p;\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// Positional struct literal: `Point { 10, 20 }` has no field names.
    #[test]
    fn positional_struct_lit_parses_clean() {
        let src = "main() {\n    mut pt = Point { 10, 20 };\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// Assignment to a name and to a field of a struct. Confirms the
    /// AssignExpr kind dispatch in `expr_bp` triggers only on `=`.
    #[test]
    fn assign_expr_to_name_and_field_parse_clean() {
        let src = "main() {\n    counter = counter + 1;\n    pt.x = 15;\n    pt_ref.y = 30;\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// `if cond { ... } else { ... }` as the right-hand side of a `let`
    /// binding - exercises if-as-expression and the no-struct-lit gate inside
    /// the condition.
    #[test]
    fn if_expr_as_value_parses_clean() {
        let src = "main() {\n    let max = if x > counter { x } else { counter };\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// Fixed-size array: typed `let` with an `[T; N]` annotation, an `[...]`
    /// literal initializer, and a postfix index. Must round-trip byte-for-byte.
    #[test]
    fn array_decl_literal_and_index_parse_clean() {
        let src = "main() {\n    let [int32; 3] xs = [1, 2, 3];\n    xs[0] = xs[1];\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
        let s = format!("{:#?}", parse.green);
        assert!(s.contains("ArrayType"), "expected ArrayType in:\n{s}");
        assert!(s.contains("ArrayLit"), "expected ArrayLit in:\n{s}");
        assert!(s.contains("IndexExpr"), "expected IndexExpr in:\n{s}");
    }

    /// `else if` chaining. The chained `if` is wrapped in a synthetic Block
    /// (the `else { if ... }` desugar), so the else-branch stays a Block; the
    /// CST must still reproduce the source byte-for-byte and carry no diagnostics.
    #[test]
    fn else_if_chain_parses_clean() {
        let src = "main() {\n    if a { x } else if b { y } else { z }\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// `if` used as a statement-position expression without a trailing `;`,
    /// followed by another statement - the block-like rule in `block`.
    #[test]
    fn if_as_stmt_without_semicolon_parses_clean() {
        let src = "main() {\n    if counter > 10 { break; }\n    counter = counter + 1;\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// `loop { ... }` with `break;` and `continue;` inside.
    #[test]
    fn loop_with_break_and_continue_parses_clean() {
        let src = "main() {\n    loop {\n        if done { break; }\n        if skip { continue; }\n        counter = counter + 1;\n    }\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// `break expr` carries a value; the parser must accept it without
    /// requiring a separator before the expression.
    #[test]
    fn break_with_value_parses_clean() {
        let src = "main() {\n    loop { break 42; }\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// Assignment is right-associative and lowest precedence. `a = b + c`
    /// must group as `a = (b + c)`, not `(a = b) + c`. Walks the CST so the
    /// check catches a swapped grouping rather than just a co-occurrence.
    #[test]
    fn assign_is_right_assoc_and_below_addition() {
        let src = "main() {\n    a = b + c;\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);

        // SourceFile > FnDef > Block > ExprStmt > AssignExpr > {NameRef, BinExpr}
        fn find_kind(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
            if node.kind() == kind {
                return Some(node.clone());
            }
            node.children().find_map(|c| find_kind(&c, kind))
        }

        let assign = find_kind(&parse.green, SyntaxKind::AssignExpr).expect("AssignExpr in tree");
        let kids: Vec<SyntaxKind> = assign.children().map(|c| c.kind()).collect();
        assert_eq!(
            kids,
            vec![SyntaxKind::NameRef, SyntaxKind::BinExpr],
            "AssignExpr children must be (NameRef, BinExpr), got {:?}",
            kids
        );
    }

    /// Struct literal inside a call argument inside an if-condition - the
    /// no_struct_lit gate must be cleared on entry to `arg_list`.
    #[test]
    fn struct_lit_inside_call_inside_if_condition() {
        let src = "main() {\n    if foo(Bar { x: 0 }) { ok }\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// `if x { ... }` - the bare name `x` must not gobble the following block
    /// as a struct literal body.
    #[test]
    fn if_with_bare_name_condition_does_not_eat_block_as_struct_lit() {
        let src = "main() {\n    if cond { ok }\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        // the if's body block must be an IfExpr > Block, not a StructLit
        let s = format!("{:#?}", parse.green);
        assert!(s.contains("IfExpr"));
        assert!(!s.contains("StructLit"), "got StructLit in:\n{s}");
    }

    /// A ref-to-ref type is spelled with a space: `& &Point`. The let-binding
    /// type heuristic sees the leading `&`, commits to a type, and `type_ref`
    /// recurses to produce nested `RefType`s. Round-trips byte-for-byte.
    #[test]
    fn nested_ref_type_in_let_binding_parses_clean() {
        let src = "main() {\n    mut & &Point p = & &q;\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
        // the type annotation is a RefType wrapping another RefType
        let s = format!("{:#?}", parse.green);
        assert_eq!(
            s.matches("RefType").count(),
            2,
            "expected two nested RefType nodes in:\n{s}"
        );
    }

    /// `&&` lexes as a single logical-and token, so `&&Point` cannot denote a
    /// ref-to-ref type - the type-form heuristic sees `&&` (not `&`), reads no
    /// type, and the binding fails. This pins the lexing boundary: ref-to-ref
    /// must be written `& &Point` (see `nested_ref_type_in_let_binding_parses_clean`).
    #[test]
    fn double_amp_is_logical_and_not_a_ref_to_ref_type() {
        let parse = parse_src("main() {\n    mut &&Point p = &q;\n}\n");
        assert!(
            !parse.diagnostics.is_empty(),
            "`&&Point` is logical-and, not a type; expected a diagnostic"
        );
    }

    // ---------- v0.3 match-expression coverage (M3) ----------

    /// `match` over an enum scrutinee with bare-ident, qualified, and
    /// wildcard arms. Exercises every Pat variant the grammar ships in M3.
    #[test]
    fn match_expr_with_every_pattern_form_parses_clean() {
        let src = "\
enum Shape =\n| Circle\n| Rectangle\n| Triangle\n;\n\nmain() {\n    let int32 r = match sh {\n        Circle -> 1,\n        Shape.Rectangle -> 2,\n        _ -> 0,\n    };\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);

        // verify Pat variants are all present in the CST
        let dump = format!("{:#?}", parse.green);
        assert!(dump.contains("MatchExpr"));
        assert!(dump.contains("MatchArmList"));
        assert!(dump.contains("MatchArm"));
        assert!(dump.contains("BareIdentPat"));
        assert!(dump.contains("PathPat"));
        assert!(dump.contains("WildcardPat"));
    }

    /// The scrutinee of `match scrut { ... }` must not be parsed as a struct
    /// literal, mirroring the `if cond { ... }` rule. Otherwise the arm block
    /// gets absorbed as a struct-lit body.
    #[test]
    fn match_scrutinee_does_not_eat_arm_list_as_struct_lit() {
        let src = "main() {\n    match sh {\n        _ -> 0,\n    };\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        let dump = format!("{:#?}", parse.green);
        assert!(dump.contains("MatchExpr"));
        // the only StructLit-shaped node in the tree must be absent: the
        // arm list body lives in MatchArmList, not in a StructLit.
        assert!(
            !dump.contains("StructLit"),
            "scrutinee parsed as a struct literal:\n{dump}"
        );
    }

    /// `match` as the right-hand side of a `let` binding - exercises
    /// match-as-expression in a typed let.
    #[test]
    fn match_expr_as_let_value_parses_clean() {
        let src =
            "main() {\n    let int32 r = match x {\n        A -> 1,\n        _ -> 0,\n    };\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// `expr as Type` parses as a postfix cast and round-trips byte-for-byte.
    /// `a + b as uint8` must nest as `a + (b as uint8)` - the cast binds
    /// tighter than the binary `+`.
    #[test]
    fn cast_expr_parses_clean_and_binds_tight() {
        let src = "main() {\n    let uint8 b = x as uint8;\n    let int32 c = a + b as int32;\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// A union reuses the struct field-list grammar; only the keyword
    /// differs. Parses clean and round-trips.
    #[test]
    fn union_def_parses_clean() {
        let src = "union Bits {\n    int64 i,\n    float64 f,\n};\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// An `extern` block of bodyless C signatures, with a `ptr` return and a
    /// `ptr` parameter. Parses clean and round-trips.
    #[test]
    fn extern_block_parses_clean() {
        let src = "extern {\n    malloc(uint64 size) -> ptr;\n    free(ptr p);\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// A postfix-pointer type in a `let` binding (`Point* p`) - the binding
    /// heuristic must read `Ident *` as a type, not a bare name.
    #[test]
    fn let_binding_with_postfix_pointer_type_parses() {
        let src = "main() {\n    mut Point* p = q as Point*;\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// In statement position `match` is block-like - no trailing `;` is
    /// required between it and the next statement.
    #[test]
    fn match_in_statement_position_needs_no_semicolon() {
        let src =
            "main() {\n    match x {\n        _ -> 0,\n    }\n    counter = counter + 1;\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// The trailing comma on the final arm is optional - both shapes parse.
    #[test]
    fn match_trailing_comma_on_last_arm_is_optional() {
        let with_comma = "main() {\n    match x {\n        A -> 1,\n        B -> 2,\n    };\n}\n";
        let without_comma = "main() {\n    match x {\n        A -> 1,\n        B -> 2\n    };\n}\n";
        for src in [with_comma, without_comma] {
            let parse = parse_src(src);
            assert!(
                parse.diagnostics.is_empty(),
                "{src:?} -> {:?}",
                parse.diagnostics
            );
            assert_eq!(parse.green.to_string(), src);
        }
    }

    /// An empty arm list is structurally valid: the parser emits a
    /// MatchExpr with an empty MatchArmList and no diagnostics. Exhaustiveness
    /// (rejecting the empty form when the scrutinee has variants) is an HIR
    /// concern (M4), not a parse concern.
    #[test]
    fn match_with_empty_arm_list_parses_without_diagnostic() {
        let src = "main() {\n    match x { };\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
        let dump = format!("{:#?}", parse.green);
        assert!(dump.contains("MatchArmList"));
        assert!(
            !dump.contains("MatchArm@"),
            "empty arm list should not produce a MatchArm node:\n{dump}"
        );
    }

    /// Arm body expressions can be any expression, including struct literals -
    /// the suppression gate is cleared on entry to the arm list.
    #[test]
    fn match_arm_body_can_be_a_struct_literal() {
        let src = "main() {\n    let Point r = match x {\n        _ -> Point { x: 0, y: 0 },\n    };\n}\n";
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }

    /// `,` is mandatory between match arms; omitting it produces a
    /// diagnostic. Recovery still parses subsequent arms.
    #[test]
    fn match_missing_comma_between_arms_is_diagnosed() {
        let src = "main() {\n    match x {\n        A -> 1\n        B -> 2,\n    };\n}\n";
        let parse = parse_src(src);
        assert!(
            parse.diagnostics.entries().iter().any(|(_, e)| matches!(
                e,
                ParseError::Syntax(SyntaxError::ExpectedCommaBetweenMatchArms)
            )),
            "expected a missing-comma diagnostic; got {:?}",
            parse.diagnostics
        );
        // both arms still recover into the tree
        let dump = format!("{:#?}", parse.green);
        let arm_count = dump.matches("MatchArm@").count();
        assert_eq!(
            arm_count, 2,
            "both arms must recover into the tree:\n{dump}"
        );
        // tree still round-trips to the source bytes
        assert_eq!(parse.green.to_string(), src);
    }

    /// Missing `->` after a pattern is recovered: a diagnostic is recorded
    /// and the parser still produces a tree that round-trips to the source.
    #[test]
    fn match_arm_missing_arrow_is_recovered() {
        let src = "main() {\n    match x {\n        A 1,\n        _ -> 0,\n    };\n}\n";
        let parse = parse_src(src);
        assert!(
            !parse.diagnostics.is_empty(),
            "expected a diagnostic for the missing '->'"
        );
        // recovery still preserves the input text byte-for-byte
        assert_eq!(parse.green.to_string(), src);
    }

    /// Full `eyesrc/design.eye` parses with zero diagnostics and round-trips
    /// byte-for-byte. This is the integration check the unit tests can miss.
    #[test]
    fn design_eye_parses_clean_and_round_trips() {
        let src = include_str!("../../../eyesrc/design.eye");
        let parse = parse_src(src);
        assert!(parse.diagnostics.is_empty(), "{:?}", parse.diagnostics);
        assert_eq!(parse.green.to_string(), src);
    }
}
