//! The `print` intrinsic: lowers `print("...{}...", args)` to `printf` with a
//! per-argument format specifier substituted for each `{}` placeholder.

use super::CGen;
use hir::core::{Body, Expr, ExprId, Literal};

impl<'a> CGen<'a> {
    pub(super) fn gen_print(&mut self, args: &[ExprId], body: &Body) {
        self.output.push_str("printf(");

        let Some(format_str_idx) = args.first() else {
            self.output.push(')');
            return;
        };

        // Non-literal format string: emit as-is, no `{}` substitution.
        let Expr::Literal(Literal::String(s)) = &body.exprs[*format_str_idx] else {
            self.gen_expr(*format_str_idx, body);
            for arg in args.iter().skip(1) {
                self.output.push_str(", ");
                self.gen_expr(*arg, body);
            }
            self.output.push(')');
            return;
        };

        // Substitute each `{}` with a per-arg format specifier driven by the
        // value's HIR type. Args without a matching placeholder are still
        // forwarded so libc surfaces an arity warning at C-compile time.
        let value_args = &args[1..];
        let mut value_iter = value_args.iter();
        let mut rendered = String::with_capacity(s.len() + value_args.len() * 2);
        // Iterate by `char`, not by byte: `{`, `}`, and `%` are all ASCII, so
        // placeholder and escape detection still work, while any multibyte
        // UTF-8 character (e.g. `é`, `→`) is preserved intact. Byte-wise
        // copying via `byte as char` would re-encode each byte as its own
        // Latin-1 codepoint and corrupt the string.
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' && chars.peek() == Some(&'}') {
                chars.next(); // consume the matching `}`
                let spec = value_iter
                    .next()
                    .map(|&a| self.format_spec_for(a, body))
                    .unwrap_or("%d");
                rendered.push_str(spec);
            } else if c == '%' {
                // Escape a literal `%` so printf does not read it as the start
                // of a conversion spec. The `{}` branch above emits its own
                // single-`%` specs and is intentionally not routed here.
                rendered.push_str("%%");
            } else {
                rendered.push(c);
            }
        }
        emit!(self, "\"{}\\n\"", rendered);

        for arg in value_args {
            self.output.push_str(", ");
            self.gen_expr(*arg, body);
        }
        self.output.push(')');
    }

    /// Pick a printf specifier for `arg` based on its HIR type. Falls back to
    /// literal-kind inspection when no `expr_types` entry exists (literals
    /// don't get one today).
    fn format_spec_for(&self, arg: ExprId, body: &Body) -> &'static str {
        // An explicitly recorded type wins, even for a literal: `arr.len`
        // lowers to a `usize` integer constant and must print `%zu`, not the
        // `%d` its literal kind would suggest.
        if let Some(ty) = body.expr_types.get(arg) {
            return Self::spec_for_type(ty);
        }
        if let Expr::Literal(lit) = &body.exprs[arg] {
            return match lit {
                Literal::Int(_) => "%d",
                Literal::Float(_) => "%f",
                Literal::String(_) => "%s",
                Literal::Bool(_) => "%d",
                Literal::Char(_) => "%c",
            };
        }
        match self.get_expr_type(arg, body) {
            Some(ty) => Self::spec_for_type(&ty),
            None => "%d",
        }
    }
}
