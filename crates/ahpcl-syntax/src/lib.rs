//! Lexer, parser and syntax tree for AHPCL.

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod token;

pub use ast::{Program, Stmt};
pub use lexer::{lex, Lexed};
pub use parser::{parse, Parsed};
pub use token::{Token, TokenKind};

/// Lex and parse in one step, collecting every error from both.
pub fn parse_source(text: &str) -> (Program, Vec<ahpcl_diagnostics::Error>) {
    let lexed = lex(text);
    let mut errors = lexed.errors;
    let parsed = parse(lexed.tokens);
    errors.extend(parsed.errors);
    (parsed.program, errors)
}
