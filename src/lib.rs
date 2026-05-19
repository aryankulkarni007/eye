//! The eye compiler library — the `lex → parse → CST → AST` pipeline.
//!
//! Each stage is a public module so it can be driven and tested in
//! isolation; [`main`](../main/index.html) is a thin driver over this API.

pub mod ast;
mod grammar;
pub mod lexer;
pub mod parser;
pub mod syntax;
pub mod token;
