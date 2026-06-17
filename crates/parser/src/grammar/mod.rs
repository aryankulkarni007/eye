//! the eye grammar - the full v0.7 surface: items (struct, union, enum,
//! `extern` FFI, fn), references / pointers / fixed arrays in the type system,
//! the operator set (arithmetic, bitwise, comparison, logical, compound
//! assignment), `match`, `as` casts, and array literals / indexing. exercised
//! end to end by `eyesrc/*.eye` (see `eyesrc/operators.eye` for the operator surface
//! and `eyesrc/arrays.eye` for arrays).
//!
//! ```text
//! source_file := item*
//! item := const_def | struct_def | union_def | extern_block | enum_def | fn_def
//! const_def := 'const' type_ref ident '=' expr ';' // compile-time value
//! // also valid as a stmt
//! struct_def := 'structure' ident field_list ';'
//! union_def := 'union' ident field_list ';'
//! extern_block := 'extern' '{' (extern_fn | extern_type)* '}'
//! extern_fn := ident param_list ('->' type_ref)? ';'
//! extern_type := 'type' ident ';' // opaque FFI type
//! enum_def := 'enum' ident '=' '|'? variant ('|' variant)* ';'
//! variant := ident // leading '|' before the first is optional
//! fn_def := ident param_list ('->' type_ref)? block
//! field_list := '{' (field ',')* '}' // the ',' terminates every field
//! field := type_ref ident
//! param_list := '(' (param (',' param)* ','?)? '...'? ')' // '...' extern-only, last
//! param := type_ref ident
//! type_ref := array_type | ('&' type_ref) | (ident postfix_ptr*)
//! array_type := '[' type_ref ';' expr ']' // fixed-size array
//! postfix_ptr := '*' // wraps the base in a ptrtype
//!
//! block := '{' (stmt)* expr? '}'
//! stmt := let_stmt | const_def | expr_stmt
//! let_stmt := ('let' | 'mut') ((type_ref? ident) | struct_pat) '=' expr ';'
//! expr_stmt := expr ';' // or block-like expr w/o ';'
//! expr := pratt
//! pratt := prefix (infix prefix)*
//! prefix := '-' prefix | '~' prefix | '!' prefix // prefixexpr
//! | '&' prefix | '*' prefix | postfix // ref/deref expr
//! postfix := base (call | index | struct_body | '.' ident | 'as' type_ref)*
//! call := '(' (expr (',' expr)* ','?)? ')'
//! index := '[' expr ']'
//! base := '(' expr ')' | atom // parenthesized group or atom
//! atom := int | float | string | true | false | char | nameref
//! | if_expr | loop_expr | break_expr | continue_expr
//! | return_expr | match_expr | array_lit
//! array_lit := '[' (expr ((';' expr) | (',' expr)* ','?))? ']' // list or `[v; N]` repeat
//! if_expr := 'if' expr_no_struct block ('else' (if_expr | block))?
//! loop_expr := 'loop' block
//! break_expr := 'break' expr?
//! continue_expr:= 'continue'
//! return_expr := 'return' expr?
//! match_expr := 'match' expr_no_struct '{' match_arm* '}'
//! match_arm := pat ('if' expr)? '->' expr ','? // ',' optional on last arm
//! pat := '_' | (nameref '.' nameref) | literal | nameref
//! // precedence is rust-style (no-footgun): every bitwise op binds tighter
//! // than comparison, and comparison is non-associative (no chaining). '=' and
//! // the compound forms are right-associative and lowest; 'as' / call / index /
//! // field bind tightest, above every prefix unary.
//! infix := '=' | '+=' | '-=' | '||' | '&&' | comparison
//! | '|' | '^' | '&' | '<<' | '>>' | '+' | '-' | '*' | '/' | '%'
//! comparison := '==' | '!=' | '<' | '>' | '<=' | '>='
//! struct_body := '{' (struct_lit_field (',' struct_lit_field)* ','?)? '}'
//! struct_lit_field := ident (':' expr)? | expr // last is positional
//! ```
//!
//! every function opens a [`Marker`], parses, and completes it with a node
//! kind. parsing is resilient: an unexpected token is wrapped in an
//! `ErrorNode` and skipped - the parser never bails, so a tree always comes
//! out.
//!
//! [`Marker`]: crate::marker

//!
//! split by concern (same layout as the other oversized modules): [`items`]
//! (the `source_file` driver + item forms), [`types`] (`type_ref`), [`stmt`]
//! (block + statement forms), [`expr`] (the pratt loop + expression forms), and
//! [`pat`] (pattern forms). every function stays `pub(crate)` within this
//! private module; siblings reach each other through the re-globs below.

mod expr;
mod items;
mod pat;
mod stmt;
mod types;

pub(crate) use items::source_file;

use expr::*;
use items::*;
use pat::*;
use stmt::*;
use types::*;
