//! Pass 1.5: the bounded const-expr evaluator.
//!
//! This is const, **not** comptime execution (see `docs/design/HORIZON0.md` and
//! `docs/features/PRIME.md`). It folds a deliberately narrow expression subset to a
//! scalar [`ConstValue`]:
//!
//! - integer / float / bool / char literals (scalar only - no aggregates);
//! - the operator set (arithmetic, bitwise, comparison, logical) over const
//!   operands of matching kind;
//! - references to other consts (cycle-checked);
//! - a numeric `as` cast between scalar kinds.
//!
//! It explicitly does **not** fold function calls (that is CTFE, far-future),
//! runtime values, or any aggregate / type-as-value. A non-const operand is a
//! [`ConstError`].
//!
//! Two entry points share the operator appliers below:
//! - [`eval_consts`] folds every `const` item with memoized, cycle-checked
//!   recursion (a const may reference a not-yet-folded const), filling
//!   [`Const::value`] and returning the finished name -> value map.
//! - [`fold_const_length`] reuses that finished map to fold a `[T; N]` length
//!   expression to a `u64` (the A6 const-length-array path).

use ast::AstNode;
use diagnostics::Sink;
use la_arena::Arena;
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use smol_str::SmolStr;
use syntax::SyntaxNodePtr;

use super::scopes::{Binding, Scopes};
use super::types::parse_int_literal;
use crate::core::{ConstError, ConstId, ConstValue, GlobalId, HIR, HirError, LocalConst, Text};

/// A lookup of const values by name. The top-level pass uses the finished
/// name -> value map directly; body lowering layers block-scope consts over it
/// ([`ScopedConsts`]). `None` means the name is not a (successfully folded)
/// const in this environment - a poisoned const reads as absent, matching how
/// [`eval_consts`] drops poisoned entries from the finished map.
pub(super) trait ConstEnv {
    fn const_value(&self, name: &Text) -> Option<ConstValue>;
}

impl ConstEnv for FxHashMap<Text, ConstValue> {
    fn const_value(&self, name: &Text) -> Option<ConstValue> {
        self.get(name).cloned()
    }
}

/// The const environment inside a function body: block-scope consts in the
/// lexical scopes first, then the top-level const map. A runtime local
/// shadowing a const name hides it - the name is not a const there.
pub(super) struct ScopedConsts<'a> {
    pub scopes: &'a Scopes,
    pub local_consts: &'a Arena<LocalConst>,
    pub globals: &'a FxHashMap<Text, ConstValue>,
}

impl ConstEnv for ScopedConsts<'_> {
    fn const_value(&self, name: &Text) -> Option<ConstValue> {
        match self.scopes.lookup(name) {
            Some(Binding::Const(id)) => self.local_consts[id].value.clone(),
            Some(Binding::Local(_)) => None,
            None => self.globals.const_value(name),
        }
    }
}

/// Fold every top-level `const` to its [`ConstValue`], filling each
/// [`Const::value`]. Const-to-const references are resolved by memoized,
/// cycle-checked recursion; a cycle, a non-const name, a non-const operation,
/// or division by zero is diagnosed and leaves that const's value `None`
/// (poison - downstream lowering treats it as already-diagnosed). Returns the
/// finished name -> value map for the array-length path.
pub(super) fn eval_consts(
    hir: &mut HIR,
    const_asts: &[(ConstId, ast::ConstDef)],
) -> FxHashMap<Text, ConstValue> {
    // name -> initializer expression. A duplicate const name keeps the first
    // body (the duplicate was already diagnosed in `collect_consts`).
    let mut bodies: FxHashMap<Text, ast::Expr> =
        FxHashMap::with_capacity_and_hasher(const_asts.len(), FxBuildHasher);
    for (id, c) in const_asts {
        let name = hir.consts[*id].name.clone();
        if let Some(body) = c.value() {
            bodies.entry(name).or_insert(body);
        }
    }

    let mut ev = Evaluator {
        bodies: &bodies,
        memo: FxHashMap::with_capacity_and_hasher(const_asts.len(), FxBuildHasher),
        visiting: FxHashSet::with_capacity_and_hasher(8, FxBuildHasher),
        diagnostics: Sink::new(),
    };
    for (id, _) in const_asts {
        let name = hir.consts[*id].name.clone();
        let value = ev.eval_name(&name);
        hir.consts[*id].value = value;
    }

    let Evaluator {
        memo, diagnostics, ..
    } = ev;
    hir.diagnostics.extend(diagnostics);
    // Keep only the successfully folded values; a poisoned const must not pose
    // as a length of 0.
    memo.into_iter()
        .filter_map(|(name, v)| v.map(|v| (name, v)))
        .collect()
}

/// Fold every top-level global's initializer to its [`ConstValue`] against the
/// finished const map, filling each `Global::value`. A global initializer is
/// the same bounded const-expr as a const (it must be known at C static-init
/// time); a non-const initializer is a [`ConstError`] and leaves the value
/// `None` (poison). Aggregate initializers are out of the scalar floor.
pub(super) fn eval_globals(
    hir: &mut HIR,
    global_asts: &[(GlobalId, ast::GlobalDef)],
    const_values: &FxHashMap<Text, ConstValue>,
) {
    let mut diagnostics = Sink::new();
    for (id, g) in global_asts {
        let value = g
            .value()
            .and_then(|expr| fold_with_map(&expr, const_values, &mut diagnostics));
        hir.globals[*id].value = value;
    }
    hir.diagnostics.extend(diagnostics);
}

/// Fold a `[T; N]` length expression against the finished const map. A bare
/// integer literal or a const-expr over already-folded consts yields the count;
/// anything else is rejected by the caller (`array_len`). Returns `None` and
/// emits a [`ConstError`] when the expression is not a non-negative integer
/// const-expr.
pub(super) fn fold_const_length(
    expr: &ast::Expr,
    consts: &dyn ConstEnv,
    diagnostics: &mut Sink<HirError>,
) -> Option<u64> {
    let value = fold_with_map(expr, consts, diagnostics)?;
    match value {
        ConstValue::Int(v) if v >= 0 => u64::try_from(v).ok().or_else(|| {
            diagnostics.emit(
                SyntaxNodePtr::new(expr.syntax()),
                HirError::Const(ConstError::ArrayLenTooLarge),
            );
            None
        }),
        _ => {
            diagnostics.emit(
                SyntaxNodePtr::new(expr.syntax()),
                HirError::Const(ConstError::ArrayLenNotInteger),
            );
            None
        }
    }
}

/// Memoized, cycle-checked folder over the const-name -> body map. Used while
/// the map is still being built, so a reference recurses into [`eval_name`].
struct Evaluator<'a> {
    bodies: &'a FxHashMap<Text, ast::Expr>,
    memo: FxHashMap<Text, Option<ConstValue>>,
    visiting: FxHashSet<Text>,
    diagnostics: Sink<HirError>,
}

impl Evaluator<'_> {
    /// Fold the const named `name`, memoizing the result. A re-entry while the
    /// name is still being folded is a definition cycle.
    fn eval_name(&mut self, name: &Text) -> Option<ConstValue> {
        if let Some(cached) = self.memo.get(name) {
            return cached.clone();
        }
        let Some(body) = self.bodies.get(name).cloned() else {
            // Not a const at all (a function, struct, or undeclared name used in
            // a const-expr). The caller (`eval_expr`) anchors the diagnostic.
            return None;
        };
        if !self.visiting.insert(name.clone()) {
            self.emit(&body, ConstError::ConstCycle { name: name.clone() });
            self.memo.insert(name.clone(), None);
            return None;
        }
        let value = self.eval_expr(&body);
        self.visiting.remove(name);
        self.memo.insert(name.clone(), value.clone());
        value
    }

    /// Fold one expression to a [`ConstValue`], or `None` (diagnosed).
    fn eval_expr(&mut self, expr: &ast::Expr) -> Option<ConstValue> {
        match expr {
            ast::Expr::Literal(lit) => parse_literal(lit),
            ast::Expr::ParenExpr(p) => p.expr().and_then(|e| self.eval_expr(&e)),
            ast::Expr::NameRef(nr) => {
                let name: Text = nr
                    .name()
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                if self.bodies.contains_key(&name) {
                    self.eval_name(&name)
                } else {
                    self.emit(expr, ConstError::ConstUnknownName { name });
                    None
                }
            }
            ast::Expr::PrefixExpr(p) => {
                let op = p.op()?;
                let operand = p.operand().and_then(|e| self.eval_expr(&e))?;
                match apply_unary(op, operand) {
                    Ok(v) => Some(v),
                    Err(err) => {
                        self.emit(expr, err);
                        None
                    }
                }
            }
            ast::Expr::BinExpr(b) => {
                let op = b.op()?;
                // Fold both sides even if one fails, so a non-const operand on
                // either side is reported, then bail.
                let lhs = b.lhs().and_then(|e| self.eval_expr(&e));
                let rhs = b.rhs().and_then(|e| self.eval_expr(&e));
                let (lhs, rhs) = (lhs?, rhs?);
                match apply_binary(op, lhs, rhs) {
                    Ok(v) => Some(v),
                    Err(err) => {
                        self.emit(expr, err);
                        None
                    }
                }
            }
            ast::Expr::CastExpr(c) => {
                let operand = c.operand().and_then(|e| self.eval_expr(&e))?;
                let target = c.ty().and_then(|t| type_name(&t));
                Some(apply_cast(operand, target.as_deref()))
            }
            // Everything else is not a const expression: a call (CTFE,
            // far-future), control flow, an aggregate, or a place.
            _ => {
                self.emit(expr, ConstError::NotAConstExpr);
                None
            }
        }
    }

    fn emit(&mut self, expr: &ast::Expr, err: ConstError) {
        self.diagnostics
            .emit(SyntaxNodePtr::new(expr.syntax()), HirError::Const(err));
    }
}

/// Fold an expression against an already-complete const environment: a const
/// reference is a plain lookup (no cycle is possible once every visible const
/// is folded). Shares the operator appliers with [`Evaluator`].
pub(super) fn fold_with_map(
    expr: &ast::Expr,
    consts: &dyn ConstEnv,
    diagnostics: &mut Sink<HirError>,
) -> Option<ConstValue> {
    let emit = |diagnostics: &mut Sink<HirError>, e: &ast::Expr, err: ConstError| {
        diagnostics.emit(SyntaxNodePtr::new(e.syntax()), HirError::Const(err));
    };
    match expr {
        ast::Expr::Literal(lit) => parse_literal(lit),
        ast::Expr::ParenExpr(p) => p
            .expr()
            .and_then(|e| fold_with_map(&e, consts, diagnostics)),
        ast::Expr::NameRef(nr) => {
            let name: Text = nr
                .name()
                .map(|t| SmolStr::from(t.text()))
                .unwrap_or_default();
            match consts.const_value(&name) {
                Some(v) => Some(v),
                None => {
                    emit(diagnostics, expr, ConstError::ConstUnknownName { name });
                    None
                }
            }
        }
        ast::Expr::PrefixExpr(p) => {
            let op = p.op()?;
            let operand = p
                .operand()
                .and_then(|e| fold_with_map(&e, consts, diagnostics))?;
            match apply_unary(op, operand) {
                Ok(v) => Some(v),
                Err(err) => {
                    emit(diagnostics, expr, err);
                    None
                }
            }
        }
        ast::Expr::BinExpr(b) => {
            let op = b.op()?;
            let lhs = b.lhs().and_then(|e| fold_with_map(&e, consts, diagnostics));
            let rhs = b.rhs().and_then(|e| fold_with_map(&e, consts, diagnostics));
            let (lhs, rhs) = (lhs?, rhs?);
            match apply_binary(op, lhs, rhs) {
                Ok(v) => Some(v),
                Err(err) => {
                    emit(diagnostics, expr, err);
                    None
                }
            }
        }
        ast::Expr::CastExpr(c) => {
            let operand = c
                .operand()
                .and_then(|e| fold_with_map(&e, consts, diagnostics))?;
            let target = c.ty().and_then(|t| type_name(&t));
            Some(apply_cast(operand, target.as_deref()))
        }
        _ => {
            emit(diagnostics, expr, ConstError::NotAConstExpr);
            None
        }
    }
}

/// Parse a scalar literal token into a [`ConstValue`]. A malformed literal the
/// lexer already flagged folds to `None`.
fn parse_literal(lit: &ast::Literal) -> Option<ConstValue> {
    let token = lit.token()?;
    let text = token.text();
    match lit.literal_kind()? {
        ast::LiteralKind::Int => Some(ConstValue::Int(parse_int_literal(text)? as i128)),
        ast::LiteralKind::Float => text.parse::<f64>().ok().filter(|f| f.is_finite()).map(ConstValue::Float),
        // U3: non-finite float literals (inf, nan) from overflow not
        // rejected. Fix independently of type inference.
        ast::LiteralKind::Bool => Some(ConstValue::Bool(text == "true")),
        ast::LiteralKind::Char => {
            let inner = text
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .unwrap_or(text);
            inner.chars().next().map(ConstValue::Char)
        }
        ast::LiteralKind::String => None, // strings are addressable data, not a const scalar
    }
}

/// The base name of a (scalar) cast target type, for [`apply_cast`]. Only a
/// bare `IdentType` carries a numeric-cast meaning here.
fn type_name(ty: &ast::TypeRef) -> Option<SmolStr> {
    match ty {
        ast::TypeRef::IdentType(it) => it.name().map(|t| SmolStr::from(t.text())),
        _ => None,
    }
}

fn is_float_type(name: &str) -> bool {
    matches!(name, "float32" | "float64")
}

fn is_int_type(name: &str) -> bool {
    matches!(
        name,
        "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "usize"
            | "isize"
    )
}

/// Fold a numeric `as` cast between scalar kinds. Only int<->float conversions
/// reshape the value; every other target keeps it (a same-kind or unknown cast
/// is a no-op fold). This mirrors C cast semantics for the scalar floor.
// U4: int->float overflow to inf, float->int truncation, char/bool->int
// signedness not range-checked. Type inference surgery will add checks.
fn apply_cast(value: ConstValue, target: Option<&str>) -> ConstValue {
    match (target, &value) {
        (Some(t), ConstValue::Int(v)) if is_float_type(t) => ConstValue::Float(*v as f64),
        (Some(t), ConstValue::Float(f)) if is_int_type(t) => ConstValue::Int(*f as i128),
        (Some(t), ConstValue::Char(c)) if is_int_type(t) => ConstValue::Int(*c as i128),
        (Some(t), ConstValue::Bool(b)) if is_int_type(t) => ConstValue::Int(*b as i128),
        _ => value,
    }
}

/// Apply a prefix-unary operator to a folded operand.
fn apply_unary(op: ast::UnaryOp, v: ConstValue) -> Result<ConstValue, ConstError> {
    use ast::UnaryOp::*;
    match (op, v) {
        (Neg, ConstValue::Int(v)) => Ok(ConstValue::Int(-v)),
        (Neg, ConstValue::Float(f)) => Ok(ConstValue::Float(-f)),
        (BitNot, ConstValue::Int(v)) => Ok(ConstValue::Int(!v)),
        (Not, ConstValue::Bool(b)) => Ok(ConstValue::Bool(!b)),
        _ => Err(ConstError::NotAConstExpr),
    }
}

/// Apply a binary operator to two folded operands of matching kind. Mixed kinds
/// (e.g. `int + float`) are rejected: the floor has no implicit numeric
/// promotion, matching the language's explicit-cast rule.
fn apply_binary(op: ast::BinOp, l: ConstValue, r: ConstValue) -> Result<ConstValue, ConstError> {
    use ConstValue::*;
    use ast::BinOp;
    match (l, r) {
        (Int(a), Int(b)) => apply_int(op, a, b),
        (Float(a), Float(b)) => apply_float(op, a, b),
        (Bool(a), Bool(b)) => match op {
            BinOp::And => Ok(Bool(a && b)),
            BinOp::Or => Ok(Bool(a || b)),
            BinOp::Eq => Ok(Bool(a == b)),
            BinOp::Neq => Ok(Bool(a != b)),
            _ => Err(ConstError::NotAConstExpr),
        },
        (Char(a), Char(b)) => match op {
            BinOp::Eq => Ok(Bool(a == b)),
            BinOp::Neq => Ok(Bool(a != b)),
            BinOp::Lt => Ok(Bool(a < b)),
            BinOp::Gt => Ok(Bool(a > b)),
            BinOp::Leq => Ok(Bool(a <= b)),
            BinOp::Geq => Ok(Bool(a >= b)),
            _ => Err(ConstError::NotAConstExpr),
        },
        _ => Err(ConstError::NotAConstExpr),
    }
}

// U2: wrapping arithmetic unchecked against declared type range.
// const X = int8(200) evaluates to 200 (not -56). Type inference
// surgery will add value-in-range checks against the declared type.
fn apply_int(op: ast::BinOp, a: i128, b: i128) -> Result<ConstValue, ConstError> {
    use ConstValue::{Bool, Int};
    use ast::BinOp::*;
    Ok(match op {
        Add => Int(a.wrapping_add(b)),
        Sub => Int(a.wrapping_sub(b)),
        Mul => Int(a.wrapping_mul(b)),
        Div => {
            if b == 0 {
                return Err(ConstError::ConstDivByZero);
            }
            Int(a.wrapping_div(b))
        }
        Rem => {
            if b == 0 {
                return Err(ConstError::ConstDivByZero);
            }
            Int(a.wrapping_rem(b))
        }
        BitAnd => Int(a & b),
        BitOr => Int(a | b),
        BitXor => Int(a ^ b),
        Shl => Int(a.wrapping_shl(b as u32)),
        Shr => Int(a.wrapping_shr(b as u32)),
        Eq => Bool(a == b),
        Neq => Bool(a != b),
        Lt => Bool(a < b),
        Gt => Bool(a > b),
        Leq => Bool(a <= b),
        Geq => Bool(a >= b),
        // `&&` / `||` are bool-only.
        And | Or => return Err(ConstError::NotAConstExpr),
    })
}

fn apply_float(op: ast::BinOp, a: f64, b: f64) -> Result<ConstValue, ConstError> {
    use ConstValue::{Bool, Float};
    use ast::BinOp::*;
    Ok(match op {
        Add => Float(a + b),
        Sub => Float(a - b),
        Mul => Float(a * b),
        Div => Float(a / b),
        Eq => Bool(a == b),
        Neq => Bool(a != b),
        Lt => Bool(a < b),
        Gt => Bool(a > b),
        Leq => Bool(a <= b),
        Geq => Bool(a >= b),
        // `%`, bitwise, shifts, and logical are not float operations.
        Rem | BitAnd | BitOr | BitXor | Shl | Shr | And | Or => {
            return Err(ConstError::NotAConstExpr);
        }
    })
}
