//! match-arm pattern lowering.

use ast::AstNode;
use syntax::SyntaxNodePtr;

use super::LoweringCtx;
use crate::core::{Pat, PatId, ResolveError, Text};

impl<'a> LoweringCtx<'a> {
    /// lower a match arm pattern - structurally, with no scrutinee type. a bare
    /// ident is classified by NAME: it is a variant iff it resolves to a known
    /// variant in the flat item-scope index, otherwise it introduces a binding
    /// (an irrefutable named wildcard). this is the rustc/rust-analyzer rule -
    /// a constructor name is always a constructor, never context-dependent - and
    /// it removes the type dependency that blocked the cutover. the judgments
    /// that DO need the scrutinee type (a variant of the wrong enum, a variant
    /// over a primitive, coverage, exhaustiveness, duplicates, unreachable arms)
    /// run in the typeck match pass. a failed name resolution produces
    /// `Pat::Missing`, which the typeck coverage check treats as "uncovered" so
    /// a typo cannot accidentally satisfy exhaustiveness.
    pub(super) fn lower_match_pat(&mut self, pat: &ast::Pat) -> PatId {
        let ptr = SyntaxNodePtr::new(pat.syntax());
        match pat {
            ast::Pat::WildcardPat(_) => self.alloc_pat(Pat::Wildcard, ptr),
            ast::Pat::LiteralPat(lp) => match lp.literal() {
                Some(lit) => {
                    let lit = super::types::lower_literal(&lit);
                    self.check_char_literal(&lit, ptr);
                    self.alloc_pat(Pat::Literal(lit), ptr)
                }
                None => self.alloc_pat(Pat::Missing, ptr),
            },
            // a qualified `Enum.Variant` pattern resolves purely by name (no
            // scrutinee type): the qualifier must be a declared enum and the
            // name one of its variants. whether that enum matches the scrutinee
            // is the typeck pass's job (`PatternEnumMismatch`).
            ast::Pat::PathPat(pp) => {
                let qual: Text = self.text(pp.qualifier().and_then(|n| n.name()));
                let vname: Text = self.text(pp.name().and_then(|n| n.name()));
                let Some(&qual_enum) = self.hir.items.enums.get(&qual) else {
                    self.emit(
                        ptr,
                        ResolveError::UnknownEnumInPattern {
                            enum_name: qual.clone(),
                        },
                    );
                    return self.alloc_pat(Pat::Missing, ptr);
                };
                let enum_def = &self.hir.enums[qual_enum];
                match enum_def.variant_index.get(&vname).copied() {
                    Some(idx) => self.alloc_pat(
                        Pat::Variant {
                            enum_id: qual_enum,
                            idx,
                        },
                        ptr,
                    ),
                    None => {
                        self.emit(
                            ptr,
                            ResolveError::NoSuchVariant {
                                enum_name: qual.clone(),
                                variant: vname.clone(),
                            },
                        );
                        self.alloc_pat(Pat::Missing, ptr)
                    }
                }
            }
            // a bare ident: a variant if the name is one (flat index), else a
            // binding. the binding's type is the scrutinee's, which lowering no
            // longer knows - the typeck pass records it (a `None` local type
            // here, filled by `TypeckResults::local_types`).
            ast::Pat::BareIdentPat(bp) => {
                let name: Text = self.text(bp.name().and_then(|n| n.name()));
                if let Some(&(enum_id, idx)) = self.hir.items.variants.get(&name) {
                    return self.alloc_pat(Pat::Variant { enum_id, idx }, ptr);
                }
                let (pat_id, local_id) = self.alloc_bind_pat(name.clone(), None, false, ptr);
                self.scopes.define(name, local_id);
                pat_id
            }
        }
    }
}
