//! Match-arm pattern lowering.

use ast::AstNode;
use syntax::SyntaxNodePtr;

use super::LoweringCtx;
use crate::core::{EnumId, Pat, PatId, ResolveError, Text};

impl<'a> LoweringCtx<'a> {
    /// Lower a match arm pattern. Bare-ident and qualified-path patterns are
    /// resolved against the scrutinee enum directly (spec says no bindings),
    /// so a name that doesn't match a variant of `scrut_enum` is an error
    /// rather than silently introducing a binding. Failure produces
    /// `Pat::Missing`; the caller's coverage check treats Missing as
    /// "uncovered" so a typo can't accidentally satisfy exhaustiveness.
    pub(super) fn lower_match_pat(&mut self, pat: &ast::Pat, scrut_enum: Option<EnumId>) -> PatId {
        let ptr = SyntaxNodePtr::new(pat.syntax());
        match pat {
            ast::Pat::WildcardPat(_) => self.alloc_pat(Pat::Wildcard, ptr),
            ast::Pat::PathPat(pp) => {
                let qual: Text = Self::text(pp.qualifier().and_then(|n| n.name()));
                let vname: Text = Self::text(pp.name().and_then(|n| n.name()));
                let Some(&qual_enum) = self.hir.items.enums.get(&qual) else {
                    self.emit(
                        ptr,
                        ResolveError::UnknownEnumInPattern {
                            enum_name: qual.clone(),
                        },
                    );
                    return self.alloc_pat(Pat::Missing, ptr);
                };
                if let Some(scrut_eid) = scrut_enum
                    && scrut_eid != qual_enum
                {
                    let scrut_name = self.hir.enums[scrut_eid].name.clone();
                    self.emit(
                        ptr,
                        ResolveError::PatternEnumMismatch {
                            pattern_enum: qual.clone(),
                            scrutinee_enum: scrut_name,
                        },
                    );
                    return self.alloc_pat(Pat::Missing, ptr);
                }
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
            ast::Pat::BareIdentPat(bp) => {
                let name: Text = Self::text(bp.name().and_then(|n| n.name()));
                // Scrutinee enum known: resolve strictly against its variants
                // so cross-enum bare patterns become a clean diagnostic.
                if let Some(eid) = scrut_enum {
                    let enum_def = &self.hir.enums[eid];
                    if let Some(&idx) = enum_def.variant_index.get(&name) {
                        return self.alloc_pat(Pat::Variant { enum_id: eid, idx }, ptr);
                    }
                    let enum_name = enum_def.name.clone();
                    self.emit(
                        ptr,
                        ResolveError::NoSuchVariant {
                            enum_name,
                            variant: name.clone(),
                        },
                    );
                    return self.alloc_pat(Pat::Missing, ptr);
                }
                // Scrutinee type unknown: fall back to the global variant
                // index. Still no bindings - an unresolved name is an error,
                // not a fresh local.
                if let Some(&(enum_id, idx)) = self.hir.items.variants.get(&name) {
                    return self.alloc_pat(Pat::Variant { enum_id, idx }, ptr);
                }
                self.emit(
                    ptr,
                    ResolveError::UnknownVariantInPattern {
                        variant: name.clone(),
                    },
                );
                self.alloc_pat(Pat::Missing, ptr)
            }
        }
    }
}
