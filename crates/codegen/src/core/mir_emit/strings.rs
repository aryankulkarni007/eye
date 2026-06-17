//! the string-literal pool: `collect_strings` walks every emitted body to find
//! the unique referenced literals (mirroring the emitter exactly so no static is
//! dead or missing), `gen_string_statics` emits one NUL-terminated byte static
//! per literal, and `string_id` maps a literal back to its static's index.

use hir::core::{FnId, HIR, Literal, Text};
use mir::core::{ArmTest, MirBlock, MirBody, MirStmt, Operand, Place, RValue};
use rustc_hash::{FxBuildHasher, FxHashMap};

use super::MirGen;

/// collect the unique string-literal contents the emitted c will actually
/// reference, in deterministic order (function arena order, then discovery
/// order within a body), so each gets one shared file-scope static.
///
/// the walk mirrors the emitter exactly (P2): every operand position emits a
/// wrapper pointer into the static (`gen_literal`), EXCEPT inside a `Println`
/// whose format is a string constant - there the format and every value are
/// inlined as c string literals (`gen_println` / `gen_println_value`), so
/// emitting the static would leave dead bytes in the binary
/// (`-Wunused-const-variable` under the strict gate).
pub(crate) fn collect_strings(
    hir: &HIR,
    mirs: &FxHashMap<FnId, MirBody>,
) -> (Vec<Text>, FxHashMap<Text, usize>) {
    struct Pool {
        out: Vec<Text>,
        index: FxHashMap<Text, usize>,
    }
    impl Pool {
        fn add(&mut self, s: &Text) {
            if !self.index.contains_key(s) {
                self.index.insert(s.clone(), self.out.len());
                self.out.push(s.clone());
            }
        }
        fn operand(&mut self, o: &Operand) {
            match o {
                Operand::Const(Literal::String(s)) => self.add(s),
                Operand::Const(_) => {}
                Operand::Copy(p) => self.place(p),
            }
        }
        fn place(&mut self, p: &Place) {
            match p {
                Place::Local(_) | Place::Global(_) => {}
                Place::Field(base, _) | Place::Deref(base) => self.place(base),
                Place::Index(base, idx) => {
                    self.place(base);
                    self.operand(idx);
                }
            }
        }
        fn rvalue(&mut self, r: &RValue) {
            match r {
                RValue::Use(o) | RValue::Unary(_, o) | RValue::Deref(o) | RValue::Cast(o, _) => {
                    self.operand(o)
                }
                RValue::Binary(_, a, b) => {
                    self.operand(a);
                    self.operand(b);
                }
                RValue::Call { args, .. } => args.iter().for_each(|a| self.operand(a)),
                RValue::CallIndirect { callee, args } => {
                    self.operand(callee);
                    args.iter().for_each(|a| self.operand(a));
                }
                RValue::Println { args } => {
                    // a string-constant format inlines the format and every
                    // value; a non-literal format forwards the operands
                    // unchanged, so a string value argument references its
                    // static.
                    if !matches!(args.first(), Some(Operand::Const(Literal::String(_)))) {
                        args.iter().for_each(|a| self.operand(a));
                    }
                }
                RValue::Func(_) | RValue::Variant(_) | RValue::SizeOf(_) => {}
                RValue::Ref(p) => self.place(p),
                RValue::ArrayLit { elems, .. } => elems.iter().for_each(|e| self.operand(e)),
                RValue::ArrayRepeat { value, .. } => self.operand(value),
                RValue::StructLit { fields, .. } => {
                    fields.iter().for_each(|(_, o)| self.operand(o))
                }
            }
        }
        fn block(&mut self, b: &MirBlock) {
            for s in &b.stmts {
                self.stmt(s);
            }
        }
        fn stmt(&mut self, s: &MirStmt) {
            match s {
                MirStmt::Let { init, .. } => {
                    if let Some(r) = init {
                        self.rvalue(r);
                    }
                }
                MirStmt::Assign { place, value } => {
                    self.place(place);
                    self.rvalue(value);
                }
                MirStmt::Eval(r) => self.rvalue(r),
                MirStmt::If {
                    cond,
                    then_block,
                    else_block,
                } => {
                    self.operand(cond);
                    self.block(then_block);
                    if let Some(e) = else_block {
                        self.block(e);
                    }
                }
                MirStmt::Loop { body } => self.block(body),
                MirStmt::Switch {
                    scrut,
                    arms,
                    default,
                } => {
                    self.operand(scrut);
                    for arm in arms {
                        // string patterns do not exist (S1 domains are
                        // int/char/bool); walked anyway so a future domain
                        // cannot silently miss its static.
                        if let ArmTest::Const(Literal::String(s)) = &arm.test {
                            self.add(s);
                        }
                        if let Some(g) = &arm.guard {
                            for st in &g.stmts {
                                self.stmt(st);
                            }
                            self.operand(&g.cond);
                        }
                        self.block(&arm.body);
                    }
                    if let Some(d) = default {
                        self.block(d);
                    }
                }
                MirStmt::Break | MirStmt::Continue => {}
                MirStmt::Return(o) => {
                    if let Some(o) = o {
                        self.operand(o);
                    }
                }
            }
        }
    }
    let mut pool = Pool {
        out: Vec::new(),
        index: FxHashMap::with_capacity_and_hasher(hir.bodies.len() * 2, FxBuildHasher),
    };
    // function arena order keeps static ids deterministic across runs (the
    // MIR map is a hash map). globals cannot reference a string: their
    // initializers are folded scalars (`ConstValue`).
    for (id, f) in hir.functions.iter() {
        if f.is_extern {
            continue;
        }
        if let Some(mir) = mirs.get(&id) {
            pool.block(&mir.body);
        }
    }
    (pool.out, pool.index)
}

impl<'a> MirGen<'a> {
    /// emit one NUL-terminated `uint8_t[]` static per unique string literal.
    /// the literal's source text is decoded first (`decode_string_literal`:
    /// escapes expanded to their real bytes), so `N` = decoded byte count,
    /// matching the `&[uint8; N]` type the literal carries. the NUL at index
    /// `N` lives in the static but outside the wrapper's `data[N]`, so a byte
    /// pointer (`->data`) read to the NUL is in-bounds (the storage is `N + 1`).
    pub(crate) fn gen_string_statics(&mut self) {
        if self.strings.is_empty() {
            return;
        }
        let string_data: Vec<(usize, Vec<u8>)> = self
            .strings
            .iter()
            .enumerate()
            .map(|(id, s)| (id, hir::core::decode_string_literal(s)))
            .collect();
        for (id, bytes) in &string_data {
            self.w(format_args!(
                "static const uint8_t __eye_str{}[{}] = {{",
                id,
                bytes.len() + 1
            ));
            for b in bytes {
                self.w(format_args!("{},", b));
            }
            self.output.push_str("0};\n");
        }
        self.output.push('\n');
    }

    /// the c id of a string literal's backing static (its index in the pool).
    // A4: unwrap_or(0) silently returned the wrong static if a string was
    // absent from string_index. collect_strings mirrors every emitter path
    // that reaches here, so this never fires on correct data; expect() for
    // defense (a miss now means the collection walk and the emitter drifted).
    pub(crate) fn string_id(&self, s: &Text) -> usize {
        self.string_index
            .get(s)
            .copied()
            .expect("string literal in string_index")
    }
}
