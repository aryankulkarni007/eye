//! the parse-event stream and the tree it builds: the [`Event`] enum (the flat
//! POD buffer the grammar appends to), the [`Marker`] / [`CompletedMarker`]
//! handles that open / complete / wrap nodes, and [`build_tree`], which replays
//! the events plus the raw token stream into a lossless rowan green tree.

use std::num::NonZeroU32;

use drop_bomb::DropBomb;
use rowan::{GreenNodeBuilder, Language};
use smallvec::SmallVec;

use lexer::SourceText;
use syntax::{EyeLang, SyntaxKind, SyntaxNode};
use token::Token;

use crate::{DiagnosticIdx, Parser};

/// a parse event. `Copy` and pointer-free so the whole stream is one flat
/// buffer of POD slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// start an internal node. `fwd_parent`, when set, is the event index of
    /// a later `Open` that should become this node's parent - the retroactive
    /// wrap produced by [`CompletedMarker::precede`].
    Open {
        kind: SyntaxKind,
        fwd_parent: Option<NonZeroU32>,
    },
    /// finish the most recently started node.
    Close,
    /// consume one non-trivia token from the stream.
    Advance,
    /// a diagnostic was recorded; no effect on tree shape.
    Diagnostic(DiagnosticIdx),
    /// an abandoned `Open`, or an `Open` already consumed as a forward
    /// parent. skipped by [`build_tree`].
    Tombstone,
}

/// an open node. the [`DropBomb`] panics if the marker is dropped without
/// being completed or abandoned, catching unbalanced grammar code.
#[must_use]
pub struct Marker {
    pub(crate) idx: u32,
    pub(crate) bomb: DropBomb,
}

impl Marker {
    /// turn the placeholder at `idx` into a real `Open` node and close it.
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

    /// discard the node. any events emitted since `open()` become children of
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

/// a completed node, the handle [`precede`](completedmarker::precede) needs to
/// retroactively wrap it in a parent.
#[derive(Clone, Copy)]
pub struct CompletedMarker {
    idx: u32,
    kind: SyntaxKind,
}

impl CompletedMarker {
    /// the [`SyntaxKind`] this node was completed as.
    pub(crate) fn kind(self) -> SyntaxKind {
        self.kind
    }

    /// open a fresh node *before* this one and make this completed node its
    /// first child. this is how postfix/precedence forms wrap a node already
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

/// walks the event stream and the raw token stream together, driving a
/// `GreenNodeBuilder`. trivia tokens are interleaved back in here so the tree
/// round-trips to the original source.
pub(crate) fn build_tree(
    tokens: &[Token],
    mut events: Vec<Event>,
    source: &SourceText,
) -> SyntaxNode {
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
