//! The lexer.
//!
//! Two modes, tracked with a brace stack. `math {` enters math mode; the matching `}`
//! leaves it. Every other `{` — an array literal, a block body — stays in normal mode.
//!
//! What differs between them:
//!   normal: `.` terminates a statement; digits cannot start a bare token
//!   math:   `.` is a decimal point; bare numbers are values; `x` is multiplication
//!
//! See docs/syntax.md.

use ahpcl_diagnostics::{Category, Code, Error, Span};
use unicode_segmentation::UnicodeSegmentation;

use crate::token::{Token, TokenKind};

const E_UNTERMINATED_QUOTE: Code = Code::new(Category::Lex, 2);
const E_BAD_ESCAPE: Code = Code::new(Category::Lex, 3);
const E_UNEXPECTED_CHAR: Code = Code::new(Category::Lex, 4);
const E_COMMENT_OVERRUN: Code = Code::new(Category::Lex, 1);
const E_BARE_NUMBER: Code = Code::new(Category::Lex, 5);

/// Characters that look like operators but are not the ones AHPCL uses. Copying a
/// formula from a web page or PDF is how these arrive.
fn lookalike(c: char) -> Option<(&'static str, &'static str, char)> {
    match c {
        '\u{2212}' => Some(("U+2212 MINUS SIGN", "this often happens when copying from a web page", '-')),
        '\u{2010}' | '\u{2011}' | '\u{2013}' | '\u{2014}' => {
            Some(("a dash", "this often happens when copying from a word processor", '-'))
        }
        '\u{2018}' | '\u{2019}' => Some(("a curly quote", "this often happens when copying from a word processor", '\'')),
        '\u{201C}' | '\u{201D}' => Some(("a curly double quote", "this often happens when copying from a word processor", '"')),
        '\u{00A0}' => Some(("U+00A0 NO-BREAK SPACE", "it looks like a space but is not one", ' ')),
        '\u{FEFF}' => Some(("U+FEFF ZERO WIDTH NO-BREAK SPACE", "it is invisible, and often survives a copy-paste", ' ')),
        '\u{200B}' => Some(("U+200B ZERO WIDTH SPACE", "it is invisible", ' ')),
        c if c.is_whitespace() => Some(("an unusual space character", "it looks like an ordinary space but is not one", ' ')),
        _ => None,
    }
}

/// Only ASCII whitespace separates tokens.
///
/// Rust's `char::is_whitespace` also accepts U+00A0 and friends, which would let a
/// pasted no-break space vanish silently. Those are exactly the lookalikes the lexer
/// exists to catch, so they fall through to the error path instead.
fn is_ahpcl_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

pub struct Lexed {
    pub tokens: Vec<Token>,
    pub errors: Vec<Error>,
}

pub fn lex(text: &str) -> Lexed {
    Lexer::new(text).run()
}

/// What a `{` opened, so the matching `}` knows whether to leave math mode.
#[derive(Clone, Copy, PartialEq)]
enum Brace {
    Math,
    Plain,
}

struct Lexer<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    tokens: Vec<Token>,
    errors: Vec<Error>,
    braces: Vec<Brace>,
    /// Byte offset at which each line starts, for `#N` comment counting.
    line_starts: Vec<usize>,
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Lexer {
            text,
            bytes: text.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
            errors: Vec::new(),
            braces: Vec::new(),
            line_starts,
        }
    }

    fn in_math(&self) -> bool {
        self.braces.last() == Some(&Brace::Math)
    }

    fn peek_char(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn peek_char_at(&self, offset: usize) -> Option<char> {
        self.text[self.pos..].chars().nth(offset)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek_char()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn starts_with(&self, s: &str) -> bool {
        self.text[self.pos..].starts_with(s)
    }

    fn at_whitespace_before(&self, start: usize) -> bool {
        if start == 0 {
            return true;
        }
        let prev = self.text[..start].chars().next_back();
        matches!(prev, Some(c) if is_ahpcl_whitespace(c))
    }

    fn at_whitespace_after(&self) -> bool {
        match self.peek_char() {
            None => true,
            Some(c) => is_ahpcl_whitespace(c),
        }
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        let space_before = self.at_whitespace_before(start);
        let space_after = self.at_whitespace_after();
        self.tokens.push(Token {
            kind,
            span: Span::new(start, self.pos),
            space_before,
            space_after,
        });
    }

    /// 1-based line containing `pos`.
    fn line_of(&self, pos: usize) -> usize {
        match self.line_starts.binary_search(&pos) {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    }

    /// Number of lines with content. A trailing newline does not start a new line.
    fn line_count(&self) -> usize {
        if self.text.ends_with('\n') {
            self.line_starts.len().saturating_sub(1).max(1)
        } else {
            self.line_starts.len()
        }
    }

    fn run(mut self) -> Lexed {
        loop {
            self.skip_whitespace();
            if self.pos >= self.bytes.len() {
                break;
            }
            let start = self.pos;
            let c = match self.peek_char() {
                Some(c) => c,
                None => break,
            };

            if c == '#' {
                self.lex_comment();
                continue;
            }

            if c == '\'' {
                self.lex_quoted(start);
                continue;
            }

            if c == '"' {
                self.lex_string(start);
                continue;
            }

            if c.is_ascii_digit() || (c == '.' && self.in_math() && matches!(self.peek_char_at(1), Some(d) if d.is_ascii_digit())) {
                self.lex_number(start);
                continue;
            }

            if is_word_start(c) {
                self.lex_word(start);
                continue;
            }

            if let Some(kind) = self.lex_operator() {
                self.push(kind, start);
                continue;
            }

            // Nothing matched.
            self.bump();
            let span = Span::new(start, self.pos);
            if let Some((name, hint, suggestion)) = lookalike(c) {
                self.errors.push(Error::new(
                    E_UNEXPECTED_CHAR,
                    span,
                    format!("'{c}' is {name}, which AHPCL does not use."),
                    format!("did you mean '{suggestion}'? {hint}."),
                ));
            } else {
                self.errors.push(Error::new(
                    E_UNEXPECTED_CHAR,
                    span,
                    format!("'{c}' (U+{:04X}) is not part of AHPCL's syntax.", c as u32),
                    "remove it, or put it inside a string.".to_string(),
                ));
            }
        }

        let end = self.pos;
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::at(end),
            space_before: true,
            space_after: true,
        });
        Lexed { tokens: self.tokens, errors: self.errors }
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek_char() {
            if is_ahpcl_whitespace(c) {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
    }

    /// `#` this line, `#3` this line and the two below, `#+3` this line and the three below.
    /// A bare number is a *total*; `+N` counts additional lines.
    fn lex_comment(&mut self) {
        let start = self.pos;
        self.bump(); // '#'

        let mut additional = false;
        if self.peek_char() == Some('+') {
            additional = true;
            self.bump();
        }

        let digits_start = self.pos;
        while matches!(self.peek_char(), Some(c) if c.is_ascii_digit()) {
            self.bump();
        }
        let digits = &self.text[digits_start..self.pos];

        // `#` immediately followed by digits is always a count. `# 3 bugs` — with a
        // space — is a plain one-line comment. The space is the programmer's
        // responsibility, by decision.
        let lines = if digits.is_empty() {
            1
        } else {
            let n: usize = digits.parse().unwrap_or(1);
            if additional {
                n + 1
            } else {
                n.max(1)
            }
        };

        let first_line = self.line_of(start);
        let last_line = first_line + lines - 1;

        if last_line > self.line_count() {
            let available = self.line_count() - first_line + 1;
            self.errors.push(Error::new(
                E_COMMENT_OVERRUN,
                Span::new(start, self.pos),
                format!(
                    "a comment line-count may not run past the end of the file. \
                     This asks for {lines} lines, but only {available} remain."
                ),
                "What the heck am I supposed to do?".to_string(),
            ));
        }

        // Consume to the end of the last covered line.
        let stop = if last_line >= self.line_count() {
            self.text.len()
        } else {
            self.line_starts[last_line]
        };
        self.pos = stop.max(self.pos);
    }

    /// `'…'` — a name or a literal value. Only `\'` and `\\` need escaping; emoji,
    /// spaces, dots and everything else are literal.
    fn lex_quoted(&mut self, start: usize) {
        self.bump(); // opening '
        let mut value = String::new();
        loop {
            match self.bump() {
                None => {
                    self.errors.push(Error::new(
                        E_UNTERMINATED_QUOTE,
                        Span::new(start, self.pos),
                        "a quoted name or value must be closed with a matching '.",
                        "add the closing quote.",
                    ));
                    break;
                }
                Some('\'') => break,
                Some('\\') => match self.bump() {
                    Some('\'') => value.push('\''),
                    Some('\\') => value.push('\\'),
                    Some(other) => {
                        let esc_start = self.pos - other.len_utf8() - 1;
                        self.errors.push(Error::new(
                            E_BAD_ESCAPE,
                            Span::new(esc_start, self.pos),
                            format!("\\{other} is not an escape AHPCL knows."),
                            "inside '…' only \\' and \\\\ need escaping.",
                        ));
                        value.push(other);
                    }
                    None => break,
                },
                Some(c) => value.push(c),
            }
        }
        self.push(TokenKind::Quoted(value), start);
    }

    fn lex_string(&mut self, start: usize) {
        self.bump(); // opening "
        let mut value = String::new();
        loop {
            match self.bump() {
                None => {
                    self.errors.push(Error::new(
                        E_UNTERMINATED_QUOTE,
                        Span::new(start, self.pos),
                        "a text string must be closed with a matching \".",
                        "add the closing quote.",
                    ));
                    break;
                }
                Some('"') => break,
                Some('\\') => match self.bump() {
                    Some('"') => value.push('"'),
                    Some('\\') => value.push('\\'),
                    Some(other) => {
                        let esc_start = self.pos - other.len_utf8() - 1;
                        self.errors.push(Error::new(
                            E_BAD_ESCAPE,
                            Span::new(esc_start, self.pos),
                            format!("\\{other} is not an escape AHPCL knows."),
                            "inside \"…\" only \\\" and \\\\ need escaping.",
                        ));
                        value.push(other);
                    }
                    None => break,
                },
                Some(c) => value.push(c),
            }
        }
        self.push(TokenKind::Str(value), start);
    }

    /// Bare numbers. Legal inside `math { }`, and inside `[…]` for shapes and precision,
    /// where they are always whole. Elsewhere a bare digit is an error, because values
    /// must be quoted.
    fn lex_number(&mut self, start: usize) {
        let allow_decimal = self.in_math();
        while let Some(c) = self.peek_char() {
            if c.is_ascii_digit() {
                self.bump();
            } else if c == '.' && allow_decimal && matches!(self.peek_char_at(1), Some(d) if d.is_ascii_digit()) {
                self.bump();
            } else {
                break;
            }
        }
        let text = self.text[start..self.pos].to_string();

        if !self.in_math() && !self.inside_brackets() && !self.inside_selector() {
            self.errors.push(Error::new(
                E_BARE_NUMBER,
                Span::new(start, self.pos),
                "values are quoted outside math { }.",
                format!("write '{text}', or put the arithmetic in a math {{ }} block."),
            ));
        }
        self.push(TokenKind::Number(text), start);
    }

    /// Selector indices are bare whole numbers: `('a'):1, 3, 9;`.
    ///
    /// A selector opens with a `:` that follows `)` (the end of a reference), `;` (the
    /// close of the previous selector in a chain), or a quoted name (the target of a
    /// `change:`). It closes at `;`. Every other `:` follows a bare word — `var:num`,
    /// `matrix:num`, `task:build` — so there is no ambiguity.
    fn inside_selector(&self) -> bool {
        for (i, tok) in self.tokens.iter().enumerate().rev() {
            match tok.kind {
                TokenKind::Semicolon => return false,
                TokenKind::Colon => {
                    let prev = i.checked_sub(1).map(|j| &self.tokens[j].kind);
                    return matches!(
                        prev,
                        Some(TokenKind::RParen)
                            | Some(TokenKind::Semicolon)
                            | Some(TokenKind::Quoted(_))
                    );
                }
                TokenKind::Dot | TokenKind::LBrace | TokenKind::MathOpen | TokenKind::RBrace => {
                    return false
                }
                _ => {}
            }
        }
        false
    }

    /// Shapes (`[3, 4]`) and precision (`[32 bit]`) hold bare whole numbers.
    fn inside_brackets(&self) -> bool {
        let mut depth = 0i32;
        for tok in self.tokens.iter().rev() {
            match tok.kind {
                TokenKind::RBracket => depth += 1,
                TokenKind::LBracket => {
                    if depth == 0 {
                        return true;
                    }
                    depth -= 1;
                }
                TokenKind::Dot | TokenKind::LBrace | TokenKind::MathOpen | TokenKind::RBrace => {
                    return false
                }
                _ => {}
            }
        }
        false
    }

    fn lex_word(&mut self, start: usize) {
        while let Some(c) = self.peek_char() {
            if is_word_continue(c) {
                self.bump();
            } else {
                break;
            }
        }
        let word = self.text[start..self.pos].to_string();

        // `math` immediately followed by `{` opens math mode. The `{` is consumed here
        // so the brace stack stays honest.
        if word == "math" {
            let save = self.pos;
            self.skip_whitespace();
            if self.peek_char() == Some('{') {
                let brace_start = self.pos;
                self.bump();
                self.braces.push(Brace::Math);
                self.push(TokenKind::Word(word), start);
                self.push(TokenKind::MathOpen, brace_start);
                return;
            }
            self.pos = save;
        }

        self.push(TokenKind::Word(word), start);
    }

    fn lex_operator(&mut self) -> Option<TokenKind> {
        // Two-character forms first.
        for (text, kind) in [
            ("//", TokenKind::SlashSlash),
            ("**", TokenKind::StarStar),
            ("<=", TokenKind::LessEq),
            (">=", TokenKind::GreaterEq),
            ("!=", TokenKind::NotEq),
        ] {
            if self.starts_with(text) {
                self.pos += text.len();
                return Some(kind);
            }
        }

        let c = self.peek_char()?;
        let kind = match c {
            '.' if !self.in_math() => TokenKind::Dot,
            ',' => TokenKind::Comma,
            ':' => TokenKind::Colon,
            ';' => TokenKind::Semicolon,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            '{' => {
                self.braces.push(Brace::Plain);
                TokenKind::LBrace
            }
            '}' => {
                self.braces.pop();
                TokenKind::RBrace
            }
            '+' => TokenKind::Plus,
            '-' => TokenKind::Minus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '^' => TokenKind::Caret,
            '=' => TokenKind::Equals,
            '<' => TokenKind::Less,
            '>' => TokenKind::Greater,
            '|' => TokenKind::Bar,
            '?' => TokenKind::Question,
            '÷' => TokenKind::Slash,
            '≠' => TokenKind::NotEq,
            '≤' => TokenKind::LessEq,
            '≥' => TokenKind::GreaterEq,
            '·' => TokenKind::DotProduct,
            '×' => TokenKind::CrossProduct,
            '⊙' => TokenKind::Hadamard,
            '⊗' => TokenKind::TensorProduct,
            '√' => TokenKind::Sqrt,
            '∧' => TokenKind::AndSym,
            '∨' => TokenKind::OrSym,
            '¬' => TokenKind::NotSym,
            '⌊' => TokenKind::FloorOpen,
            '⌋' => TokenKind::FloorClose,
            '⌈' => TokenKind::CeilOpen,
            '⌉' => TokenKind::CeilClose,
            _ => return None,
        };
        self.pos += c.len_utf8();
        Some(kind)
    }
}

/// Names are quoted, so a bare word is always a keyword, type name, word-operator or
/// constant. Unicode letters are allowed so `π` and `τ` work.
fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Count graphemes, for tests and diagnostics that need a character count.
pub fn grapheme_count(s: &str) -> usize {
    s.graphemes(true).count()
}
