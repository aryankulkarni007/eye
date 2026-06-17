//! a `MirStmt::Switch` rendered as an `if`/`else-if` chain (guard-free) or a
//! flag-gated chain (when any arm carries a guard), never a c `switch` - so a
//! `break`/`continue` in an arm body binds to the enclosing loop, not the
//! switch. plus the per-arm scrutinee test (`gen_arm_test`).

use mir::core::{ArmTest, MirBlock, MirBody, Operand, SwitchArm};

use super::MirGen;

impl<'a> MirGen<'a> {
    /// render a [`MirStmt::Switch`] as an `if`/`else if` chain comparing the
    /// scrutinee tag against each variant, not as a c `switch`. a c `switch`
    /// would capture a `break` that a match arm intends for an enclosing loop;
    /// an `if` chain leaves `break`/`continue` bound to the loop. the scrutinee
    /// is a trivial operand, so re-evaluating it per arm has no side effect.
    pub(crate) fn gen_switch(
        &mut self,
        mir: &MirBody,
        scrut: &Operand,
        arms: &[SwitchArm],
        default: &Option<MirBlock>,
    ) {
        // a guard can fail, so its arm must fall through to the next. with guard
        // temp statements that an `&&` cannot hold, an `if`/`else-if` chain
        // cannot express that, so a guarded switch uses a flag-gated chain. the
        // guard-free common case keeps the clean `if`/`else-if`.
        if arms.iter().any(|a| a.guard.is_some()) {
            self.gen_guarded_switch(mir, scrut, arms, default);
            return;
        }
        // a switch with no `default` is a match HIR proved exhaustive (a
        // non-exhaustive match is diagnosed and never reaches codegen), so the
        // last arm's test is tautological. emit it as the chain's `else`: c
        // cannot see the exhaustiveness, so a tested last arm draws
        // `-Wsometimes-uninitialized` on a value-match's temp and leaves a
        // genuinely uninitialized read if the scrutinee holds a rogue value
        // (e.g. an enum from a bad FFI cast).
        let (chain, last_as_else) = match (default, arms) {
            (None, [chain @ .., last]) => (chain, Some(last)),
            _ => (arms, None),
        };
        let mut first = true;
        for arm in chain {
            self.push_indent();
            self.output
                .push_str(if first { "if (" } else { "else if (" });
            first = false;
            self.gen_arm_test(mir, scrut, &arm.test);
            self.output.push_str(") {\n");
            self.block("}\n", |s| s.gen_block(mir, &arm.body));
        }
        let tail = last_as_else.map(|a| &a.body).or(default.as_ref());
        if let Some(body) = tail {
            self.push_indent();
            self.output.push_str(if first { "{\n" } else { "else {\n" });
            self.block("}\n", |s| s.gen_block(mir, body));
        }
    }

    /// a guarded switch as a flag-gated chain. each arm fires only while no
    /// earlier arm has both matched and passed its guard (`!flag`); a matched arm
    /// whose guard is false leaves `flag` unset, so the next arm's test is
    /// re-evaluated - the fall-through a plain `if`/`else-if` cannot give once a
    /// guard needs temp statements. no c `switch`/`break`, so a `break` /
    /// `continue` in an arm body still binds to the enclosing loop.
    ///
    /// ```c
    /// bool _gn = false;
    /// if (!_gn && <test>) { <guard.stmts> if (<guard.cond>) { <body> _gn = true; } }
    /// ...
    /// if (!_gn) { <default> }
    /// ```
    fn gen_guarded_switch(
        &mut self,
        mir: &MirBody,
        scrut: &Operand,
        arms: &[SwitchArm],
        default: &Option<MirBlock>,
    ) {
        let flag = format!("_g{}", self.guard_flag);
        self.guard_flag += 1;
        self.push_indent();
        self.w(format_args!("bool {flag} = false;\n"));
        // a switch with no `default` is a match HIR proved exhaustive via its
        // UNGUARDED arms (guards do not discharge coverage). so if control
        // reaches the last unguarded arm with the flag still unset, every
        // earlier unguarded arm's test failed and that arm's test is
        // tautological. emit it gated on the flag alone - the guarded chain's
        // analogue of the unguarded chain's `else` (M3): c cannot see the
        // exhaustiveness, and a tested last arm leaves a value-match's hoist
        // temp uninitialized when the scrutinee holds a rogue value (e.g. an
        // enum from a bad FFI cast). arms after it are guarded and dead (the
        // tautological arm fires first); they are emitted unchanged.
        let last_unguarded = match default {
            None => arms.iter().rposition(|a| a.guard.is_none()),
            Some(_) => None,
        };
        for (i, arm) in arms.iter().enumerate() {
            self.push_indent();
            // an `Always` arm (guarded catch-all) has no scrutinee test - it
            // matches anything, gated only by the flag and its own guard.
            match &arm.test {
                _ if last_unguarded == Some(i) => self.w(format_args!("if (!{flag}) {{\n")),
                ArmTest::Always => self.w(format_args!("if (!{flag}) {{\n")),
                _ => {
                    self.w(format_args!("if (!{flag} && "));
                    self.gen_arm_test(mir, scrut, &arm.test);
                    self.output.push_str(") {\n");
                }
            }
            self.block("}\n", |s| match &arm.guard {
                Some(guard) => {
                    for stmt in &guard.stmts {
                        s.gen_stmt(mir, stmt);
                    }
                    s.push_indent();
                    s.output.push_str("if (");
                    s.gen_operand(mir, &guard.cond);
                    s.output.push_str(") {\n");
                    s.block("}\n", |s| {
                        s.gen_block(mir, &arm.body);
                        s.push_indent();
                        s.w(format_args!("{flag} = true;\n"));
                    });
                }
                None => {
                    s.gen_block(mir, &arm.body);
                    s.push_indent();
                    s.w(format_args!("{flag} = true;\n"));
                }
            });
        }
        if let Some(default) = default {
            self.push_indent();
            self.w(format_args!("if (!{flag}) {{\n"));
            self.block("}\n", |s| s.gen_block(mir, default));
        }
    }

    /// render an [`ArmTest`] as a c boolean expression over `scrut` (no
    /// surrounding parens; the caller wraps the chain's `if (...)`). one arm per
    /// `ArmTest` kind: S1 adds `Const`, S4 adds `Range`/`Or`.
    fn gen_arm_test(&mut self, mir: &MirBody, scrut: &Operand, test: &ArmTest) {
        match test {
            ArmTest::Variant(v) => {
                self.gen_operand(mir, scrut);
                let label = &self.hir.enums[v.enum_id].variants[v.idx as usize].name;
                self.w(format_args!(" == {}", label));
            }
            ArmTest::Const(lit) => {
                self.gen_operand(mir, scrut);
                self.output.push_str(" == ");
                self.gen_literal(lit);
            }
            // `Always` carries no scrutinee test and only appears guarded, so
            // `gen_guarded_switch` renders it without a test condition; this branch
            // is unreachable. emit `true` rather than panic.
            ArmTest::Always => self.output.push_str("true"),
        }
    }
}
