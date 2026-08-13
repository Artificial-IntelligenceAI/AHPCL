//! Tokens.
//!
//! The lexer has two modes. Inside `math { }` numbers are bare, `.` is a decimal point,
//! and `x` is multiplication. Outside, numbers are quoted, `.` terminates a statement,
//! and `x` is an ordinary letter. See docs/syntax.md.

use ahpcl_diagnostics::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    /// `'…'` — a name, or a literal value. Quotes are stripped, escapes resolved.
    Quoted(String),
    /// `"…"` — a text string.
    Str(String),
    /// A bare number. Only ever produced inside `math { }`.
    Number(String),
    /// A bare word: a keyword, type name, or word-operator. The parser decides which.
    Word(String),

    Dot,
    Comma,
    Colon,
    Semicolon,
    LParen,
    RParen,
    LBracket,
    RBracket,
    /// `{` — opens an array literal or a block.
    LBrace,
    /// `{` immediately following the word `math`.
    MathOpen,
    RBrace,

    Plus,
    Minus,
    Star,
    Slash,
    SlashSlash,
    Caret,
    StarStar,
    Equals,
    Less,
    Greater,
    LessEq,
    GreaterEq,
    NotEq,

    /// `·` dot product, which is also matrix multiplication.
    DotProduct,
    /// `×` cross product.
    CrossProduct,
    /// `⊙` elementwise product.
    Hadamard,
    /// `⊗` tensor product.
    TensorProduct,
    /// `√`
    Sqrt,
    /// `|` — absolute value, on both sides of its operand.
    Bar,
    /// `⌊` `⌋`
    FloorOpen,
    FloorClose,
    /// `⌈` `⌉`
    CeilOpen,
    CeilClose,

    /// `∧`
    AndSym,
    /// `∨`
    OrSym,
    /// `¬`
    NotSym,

    /// `?` — an unknown dimension in a shape, as in `[?, 3]`.
    Question,

    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// True when whitespace (or a line start) immediately precedes this token.
    /// `x` is multiplication only with a space on each side.
    pub space_before: bool,
    /// True when whitespace, a newline, or end-of-input immediately follows.
    pub space_after: bool,
}

impl Token {
    /// Whether this token is the word-operator `x`, which needs a space on each side.
    /// `2 x 4` is arithmetic; `2x4` is not.
    pub fn is_spaced_x(&self) -> bool {
        matches!(&self.kind, TokenKind::Word(w) if w == "x") && self.space_before && self.space_after
    }

    pub fn word(&self) -> Option<&str> {
        match &self.kind {
            TokenKind::Word(w) => Some(w.as_str()),
            _ => None,
        }
    }
}
