//! The parser.
//!
//! Recursive descent for statements, precedence climbing for expressions. Hand-written
//! rather than generated, because error messages and recovery are the point.
//!
//! See docs/syntax.md for every rule this encodes.

use ahpcl_diagnostics::{Category, Code, Error, Span};

use crate::ast::*;
use crate::token::{Token, TokenKind};

const E_EXPECTED: Code = Code::new(Category::Syn, 1);
const E_BAD_TYPE: Code = Code::new(Category::Type, 10);
const E_BAD_SHAPE: Code = Code::new(Category::Shape, 10);
const E_BAD_PRECISION: Code = Code::new(Category::Prec, 10);
const E_RANK_MISMATCH: Code = Code::new(Category::Shape, 2);

/// Base type names. `∞num` is the Unicode spelling of `infnum`.
const BASE_TYPES: &[&str] = &[
    "num", "rat", "deci", "int", "infnum", "∞num", "str", "nna", "bool", "none",
];

const WORD_OPERATORS: &[(&str, BinOp)] = &[
    ("x", BinOp::Mul),
    ("xx", BinOp::Pow),
    ("mod", BinOp::Mod),
    ("and", BinOp::And),
    ("or", BinOp::Or),
];

const WORD_UNARY: &[(&str, UnOp)] = &[
    ("not", UnOp::Not),
    ("sqrt", UnOp::Sqrt),
    ("abs", UnOp::Abs),
    ("floor", UnOp::Floor),
    ("ceil", UnOp::Ceil),
    ("sin", UnOp::Sin),
    ("cos", UnOp::Cos),
    ("tan", UnOp::Tan),
    ("log", UnOp::Log),
    ("ln", UnOp::Ln),
];

const BUILTINS: &[&str] = &["print", "read", "parse", "clock"];

pub struct Parsed {
    pub program: Program,
    pub errors: Vec<Error>,
}

pub fn parse(tokens: Vec<Token>) -> Parsed {
    Parser::new(tokens).run()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    errors: Vec<Error>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0, errors: Vec::new() }
    }

    // ── token helpers ───────────────────────────────────────────────────────

    /// The span of the token most recently consumed.
    ///
    /// Closing a construct's span with `peek()` instead reached into the token *after*
    /// it, so every caret covered one token too many: the marker under `('nope')` also
    /// underlined the string that followed it. The reported column was right; the
    /// picture was not.
    fn last_span(&self) -> Span {
        let i = self.pos.saturating_sub(1).min(self.tokens.len() - 1);
        self.tokens[i].span
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn peek_at(&self, n: usize) -> &Token {
        &self.tokens[(self.pos + n).min(self.tokens.len() - 1)]
    }

    fn at_end(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }

    fn advance(&mut self) -> Token {
        let t = self.peek().clone();
        if !self.at_end() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.peek_kind() == kind {
            self.advance();
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, word: &str) -> bool {
        if self.peek().word() == Some(word) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn at_word(&self, word: &str) -> bool {
        self.peek().word() == Some(word)
    }

    fn expect(&mut self, kind: TokenKind, what: &str, fix: &str) -> bool {
        if self.eat(&kind) {
            return true;
        }
        let span = self.peek().span;
        self.errors.push(Error::new(
            E_EXPECTED,
            span,
            format!("expected {what} here."),
            fix.to_string(),
        ));
        false
    }

    fn error(&mut self, span: Span, rule: impl Into<String>, fix: impl Into<String>) {
        self.errors.push(Error::new(E_EXPECTED, span, rule, fix));
    }

    /// Skip to just past the next statement terminator, so one bad statement does not
    /// cascade into a page of nonsense.
    fn recover(&mut self) {
        while !self.at_end() {
            if self.eat(&TokenKind::Dot) {
                return;
            }
            // Do not swallow a closing brace: the block above needs it.
            if matches!(self.peek_kind(), TokenKind::RBrace) {
                return;
            }
            self.advance();
        }
    }

    // ── entry point ─────────────────────────────────────────────────────────

    fn run(mut self) -> Parsed {
        let mut program = Program::default();
        while !self.at_end() {
            let before = self.pos;
            match self.statement() {
                Some(s) => program.statements.push(s),
                None => self.recover(),
            }
            if self.pos == before {
                self.advance();
            }
        }
        Parsed { program, errors: self.errors }
    }

    // ── statements ──────────────────────────────────────────────────────────

    fn statement(&mut self) -> Option<Stmt> {
        let start = self.peek().span;

        if self.at_word("var") {
            return self.var_decl().map(Stmt::Var);
        }
        if self.at_word("change") {
            return self.change_stmt().map(Stmt::Change);
        }
        if self.at_word("func") {
            return self.func_decl().map(Stmt::Func);
        }
        if self.at_word("if") {
            let chain = self.if_chain()?;
            self.expect(TokenKind::Dot, "'.' to end the if", "add a '.' after the last '}'.");
            return Some(Stmt::If(chain));
        }
        if self.at_word("loop") {
            let l = self.loop_stmt()?;
            self.expect(TokenKind::Dot, "'.' to end the loop", "add a '.' after the '}'.");
            return Some(Stmt::Loop(l));
        }
        if self.at_word("handback") || self.at_word("hb") {
            self.advance();
            let value = self.expression()?;
            let span = start.to(self.last_span());
            self.expect(TokenKind::Dot, "'.' to end the statement", "add a '.'.");
            return Some(Stmt::Handback { value, span });
        }
        if self.at_word("print") {
            self.advance();
            let args = self.print_args()?;
            let span = start.to(self.last_span());
            self.expect(TokenKind::Dot, "'.' to end the statement", "add a '.'.");
            return Some(Stmt::Print { args, span });
        }

        // Anything else is an expression statement, such as a call for its effect.
        let expr = self.expression()?;
        self.expect(TokenKind::Dot, "'.' to end the statement", "add a '.'.");
        Some(Stmt::Expr(expr))
    }

    /// `var:TYPE 'name' [shape] [prec] = value, 'other' = value.`
    fn var_decl(&mut self) -> Option<VarDecl> {
        let start = self.peek().span;
        self.advance(); // var
        self.expect(TokenKind::Colon, "':' after 'var'", "write var:num 'x' = '1'.")
            .then_some(())?;
        let ty = self.type_ref()?;

        let mut bindings = Vec::new();
        loop {
            let binding = self.binding()?;
            bindings.push(binding);
            if self.eat(&TokenKind::Comma) {
                continue;
            }
            break;
        }

        let span = start.to(self.last_span());
        self.expect(TokenKind::Dot, "'.' to end the declaration", "add a '.'.");
        Some(VarDecl { ty, bindings, span })
    }

    fn binding(&mut self) -> Option<Binding> {
        let (name, name_span) = self.quoted_name("a variable name")?;
        let shape = self.optional_shape();
        let precision = self.optional_precision();

        let value = if self.eat(&TokenKind::Equals) {
            Some(self.expression()?)
        } else {
            None
        };

        Some(Binding { name, name_span, shape, precision, value })
    }

    /// `change:var:TYPE 'name'[:sel;] = value.`
    fn change_stmt(&mut self) -> Option<ChangeStmt> {
        let start = self.peek().span;
        self.advance(); // change
        self.expect(TokenKind::Colon, "':' after 'change'", "write change:var:num 'x' = '2'.")
            .then_some(())?;
        if !self.eat_word("var") {
            let span = self.peek().span;
            self.error(span, "a change restates the declaration.", "write change:var:num 'x' = '2'.");
            return None;
        }
        self.expect(TokenKind::Colon, "':' after 'var'", "write change:var:num 'x' = '2'.")
            .then_some(())?;
        let ty = self.type_ref()?;

        let mut targets = Vec::new();
        loop {
            let (name, name_span) = self.quoted_name("the variable being changed")?;
            let selectors = self.selectors();
            self.expect(TokenKind::Equals, "'=' before the new value", "write change:var:num 'x' = '2'.")
                .then_some(())?;
            let value = self.expression()?;
            targets.push(ChangeTarget { name, name_span, selectors, value });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        let span = start.to(self.last_span());
        self.expect(TokenKind::Dot, "'.' to end the statement", "add a '.'.");
        Some(ChangeStmt { ty, targets, span })
    }

    /// `func:TYPE 'name' [params] { body }.`
    fn func_decl(&mut self) -> Option<FuncDecl> {
        let start = self.peek().span;
        self.advance(); // func
        self.expect(TokenKind::Colon, "':' after 'func'", "write func:num 'name' [] { }.")
            .then_some(())?;
        let returns = self.type_ref()?;
        let (name, name_span) = self.quoted_name("a function name")?;

        let mut params = Vec::new();
        if self.expect(TokenKind::LBracket, "'[' to open the parameter list", "write [] for no parameters.") {
            while !matches!(self.peek_kind(), TokenKind::RBracket) && !self.at_end() {
                // Parameters are ordinary declarations, so the whole grammar applies.
                if !self.eat_word("var") {
                    let span = self.peek().span;
                    self.error(
                        span,
                        "parameters are ordinary declarations.",
                        "write [var:num 'n'].",
                    );
                    break;
                }
                self.expect(TokenKind::Colon, "':' after 'var'", "write [var:num 'n'].");
                let ty = self.type_ref()?;
                loop {
                    let (pname, pspan) = self.quoted_name("a parameter name")?;
                    let shape = self.optional_shape();
                    let precision = self.optional_precision();
                    params.push(Param {
                        ty: ty.clone(),
                        name: pname,
                        name_span: pspan,
                        shape,
                        precision,
                    });
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                    // A comma may introduce another name sharing the type, or a whole
                    // new parameter with its own `var:`.
                    if self.at_word("var") {
                        break;
                    }
                }
            }
            self.expect(TokenKind::RBracket, "']' to close the parameter list", "add a ']'.");
        }

        let body = self.block()?;
        let span = start.to(self.last_span());
        self.expect(TokenKind::Dot, "'.' to end the function", "add a '.' after the '}'.");
        Some(FuncDecl { returns, name, name_span, params, body, span })
    }

    /// `if cond { } , else if cond { } , else { }` — `,` extends the chain.
    fn if_chain(&mut self) -> Option<IfChain> {
        let start = self.peek().span;
        self.advance(); // if
        let condition = self.expression()?;
        let body = self.block()?;
        let mut arms = vec![IfArm { condition: Some(condition), body }];

        loop {
            // The comma is what extends the chain; `else` is what distinguishes it
            // from a second variable in a declaration.
            let save = self.pos;
            let extended = self.eat(&TokenKind::Comma);
            if !self.at_word("else") {
                if extended {
                    self.pos = save;
                }
                break;
            }
            self.advance(); // else

            if self.at_word("if") {
                self.advance();
                let condition = self.expression()?;
                let body = self.block()?;
                arms.push(IfArm { condition: Some(condition), body });
            } else {
                let body = self.block()?;
                arms.push(IfArm { condition: None, body });
                break;
            }
        }

        Some(IfChain { arms, span: start.to(self.last_span()) })
    }

    /// `loop:var:int 'i' = range { }` or `loop:while cond { }`.
    fn loop_stmt(&mut self) -> Option<LoopStmt> {
        let start = self.peek().span;
        self.advance(); // loop
        self.expect(TokenKind::Colon, "':' after 'loop'", "write loop:while … or loop:var:int 'i' = ….")
            .then_some(())?;

        let kind = if self.eat_word("while") {
            let condition = self.expression()?;
            LoopKind::While { condition }
        } else if self.eat_word("var") {
            self.expect(TokenKind::Colon, "':' after 'var'", "write loop:var:int 'i' = math { 1 to 10 }.")
                .then_some(())?;
            let ty = self.type_ref()?;
            let (var, var_span) = self.quoted_name("the loop counter")?;
            // A counter may carry a precision, exactly as any other declaration does.
            let _precision = self.optional_precision();
            self.expect(TokenKind::Equals, "'=' before the range", "write = math { 1 to 10 }.")
                .then_some(())?;
            let range = self.expression()?;
            LoopKind::Counted { ty, var, var_span, range }
        } else {
            let span = self.peek().span;
            self.error(
                span,
                "a loop is either counted or conditional.",
                "write loop:var:int 'i' = math { 1 to 10 } { … } or loop:while math { … } { … }.",
            );
            return None;
        };

        let body = self.block()?;
        Some(LoopStmt { kind, body, span: start.to(self.last_span()) })
    }

    fn block(&mut self) -> Option<Block> {
        if !self.expect(TokenKind::LBrace, "'{' to open a block", "add a '{'.") {
            return None;
        }
        let mut stmts = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace) && !self.at_end() {
            let before = self.pos;
            match self.statement() {
                Some(s) => stmts.push(s),
                None => self.recover(),
            }
            if self.pos == before {
                self.advance();
            }
        }
        self.expect(TokenKind::RBrace, "'}' to close the block", "add a '}'.");
        Some(stmts)
    }

    /// `print[ items ]` — string literals and references only. Deliberately not
    /// expressions: compute into a variable first.
    fn print_args(&mut self) -> Option<Vec<Expr>> {
        if !self.expect(TokenKind::LBracket, "'[' after print", "write print[\"hello\"].") {
            return None;
        }
        let mut args = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBracket) && !self.at_end() {
            let span = self.peek().span;
            match self.peek_kind().clone() {
                TokenKind::Str(s) => {
                    self.advance();
                    args.push(Expr { kind: ExprKind::Str(s), span });
                }
                TokenKind::LParen => {
                    let e = self.reference()?;
                    args.push(e);
                }
                _ => {
                    self.error(
                        span,
                        "print takes text and references only.",
                        "compute the value into a variable first, then print that.",
                    );
                    self.advance();
                }
            }
        }
        self.expect(TokenKind::RBracket, "']' to close print", "add a ']'.");
        Some(args)
    }

    // ── types ───────────────────────────────────────────────────────────────

    /// `vector:+num`, `+int`, `deci`, `∞num`, `none`, and so on.
    ///
    /// For an array the sign refines the **element** type, so it is written after the
    /// rank: `vector:+num` is a vector whose every element is positive.
    fn type_ref(&mut self) -> Option<TypeRef> {
        let start = self.peek().span;

        let leading_sign = self.optional_sign();

        let first = match self.peek().word() {
            Some(w) => w.to_string(),
            None => {
                let span = self.peek().span;
                self.error(span, "expected a type name here.", "for example num, int, deci or matrix:num.");
                return None;
            }
        };
        self.advance();

        let (rank, base, sign) = if let Some(rank) = Rank::from_word(&first) {
            if !self.expect(TokenKind::Colon, "':' and an element type", "write matrix:num.") {
                return None;
            }
            let element_sign = self.optional_sign();
            let base = match self.peek().word() {
                Some(w) => w.to_string(),
                None => {
                    let span = self.peek().span;
                    self.error(span, "expected an element type here.", "write matrix:num.");
                    return None;
                }
            };
            self.advance();
            (Some(rank), base, element_sign.or(leading_sign))
        } else {
            (None, first, leading_sign)
        };

        if !BASE_TYPES.contains(&base.as_str()) {
            let span = start.to(self.last_span());
            self.errors.push(Error::new(
                E_BAD_TYPE,
                span,
                format!("'{base}' is not a type AHPCL knows."),
                "the numeric types are num, rat, deci, int and infnum; there are also str, nna, bool and none."
                    .to_string(),
            ));
        }

        Some(TypeRef {
            sign,
            rank,
            base,
            shape: None,
            precision: None,
            span: start.to(self.last_span()),
        })
    }

    fn optional_sign(&mut self) -> Option<Sign> {
        match self.peek_kind() {
            TokenKind::Plus => {
                self.advance();
                Some(Sign::Positive)
            }
            TokenKind::Minus => {
                self.advance();
                Some(Sign::Negative)
            }
            _ => None,
        }
    }

    /// `[3, 4]` or `[?, 3]`, distinguished from precision by not saying "bit"/"digits".
    fn optional_shape(&mut self) -> Option<Vec<Dim>> {
        if !matches!(self.peek_kind(), TokenKind::LBracket) {
            return None;
        }
        // Precision is `[32 bit]`; a shape is anything else in brackets.
        if self.bracket_is_precision() {
            return None;
        }
        self.advance(); // [

        let mut dims = Vec::new();
        loop {
            match self.peek_kind().clone() {
                TokenKind::Question => {
                    self.advance();
                    dims.push(Dim::Unknown);
                }
                TokenKind::Number(n) => {
                    self.advance();
                    match n.parse::<u64>() {
                        Ok(v) => dims.push(Dim::Known(v)),
                        Err(_) => {
                            let span = self.peek().span;
                            self.errors.push(Error::new(
                                E_BAD_SHAPE,
                                span,
                                format!("'{n}' is not a whole number of elements."),
                                "a dimension is a whole number, or ? when it is not knowable.".to_string(),
                            ));
                        }
                    }
                }
                _ => {
                    let span = self.peek().span;
                    self.errors.push(Error::new(
                        E_BAD_SHAPE,
                        span,
                        "a shape holds whole numbers, or ? for an unknown dimension.",
                        "write [3, 4] or [?, 3].",
                    ));
                    break;
                }
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RBracket, "']' to close the shape", "add a ']'.");
        Some(dims)
    }

    /// `[32 bit]` or `[100 digits]`.
    fn optional_precision(&mut self) -> Option<Precision> {
        if !matches!(self.peek_kind(), TokenKind::LBracket) || !self.bracket_is_precision() {
            return None;
        }
        self.advance(); // [
        let n = match self.peek_kind().clone() {
            TokenKind::Number(n) => {
                self.advance();
                n
            }
            _ => {
                let span = self.peek().span;
                self.errors.push(Error::new(
                    E_BAD_PRECISION,
                    span,
                    "precision is a number followed by 'bit' or 'digits'.",
                    "write [32 bit] or [100 digits].",
                ));
                return None;
            }
        };
        let unit = self.peek().word().unwrap_or("").to_string();
        self.advance();
        self.expect(TokenKind::RBracket, "']' to close the precision", "add a ']'.");

        let value: u32 = n.parse().unwrap_or(0);
        match unit.as_str() {
            "bit" | "bits" => Some(Precision::Bits(value)),
            "digit" | "digits" => Some(Precision::Digits(value)),
            _ => None,
        }
    }

    /// Look ahead for the `bit`/`digits` that marks a bracket as precision.
    fn bracket_is_precision(&self) -> bool {
        let mut i = 1;
        loop {
            match &self.peek_at(i).kind {
                TokenKind::RBracket | TokenKind::Eof => return false,
                TokenKind::Word(w) if w == "bit" || w == "bits" || w == "digit" || w == "digits" => {
                    return true
                }
                _ => i += 1,
            }
            if i > 8 {
                return false;
            }
        }
    }

    fn quoted_name(&mut self, what: &str) -> Option<(String, Span)> {
        let span = self.peek().span;
        match self.peek_kind().clone() {
            TokenKind::Quoted(name) => {
                self.advance();
                Some((name, span))
            }
            _ => {
                self.error(
                    span,
                    format!("expected {what} here, in quotes."),
                    "names are always quoted, so 'x' rather than x.",
                );
                None
            }
        }
    }

    // ── expressions ─────────────────────────────────────────────────────────

    fn expression(&mut self) -> Option<Expr> {
        self.binary_expr(0)
    }

    /// Precedence climbing. Powers group right to left; everything else left to right.
    fn binary_expr(&mut self, min_prec: u8) -> Option<Expr> {
        let mut lhs = self.unary_expr()?;

        loop {
            let Some(op) = self.peek_binop() else { break };
            let prec = op.precedence();
            if prec < min_prec {
                break;
            }
            self.advance_binop(op);

            let next_min = if op.right_associative() { prec } else { prec + 1 };
            let rhs = self.binary_expr(next_min)?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) },
                span,
            };
        }

        // `a to b by c` sits below every arithmetic operator.
        if self.at_word("to") && min_prec == 0 {
            self.advance();
            let to = self.binary_expr(0)?;
            let by = if self.eat_word("by") {
                Some(Box::new(self.binary_expr(0)?))
            } else {
                None
            };
            let span = lhs.span.to(to.span);
            lhs = Expr {
                kind: ExprKind::Range { from: Box::new(lhs), to: Box::new(to), by },
                span,
            };
        }

        Some(lhs)
    }

    fn peek_binop(&self) -> Option<BinOp> {
        let tok = self.peek();
        let op = match &tok.kind {
            TokenKind::Plus => BinOp::Add,
            TokenKind::Minus => BinOp::Sub,
            TokenKind::Star => BinOp::Mul,
            TokenKind::Slash => BinOp::Div,
            TokenKind::SlashSlash => BinOp::IntDiv,
            TokenKind::Caret | TokenKind::StarStar => BinOp::Pow,
            TokenKind::Equals => BinOp::Eq,
            TokenKind::NotEq => BinOp::NotEq,
            TokenKind::Less => BinOp::Less,
            TokenKind::Greater => BinOp::Greater,
            TokenKind::LessEq => BinOp::LessEq,
            TokenKind::GreaterEq => BinOp::GreaterEq,
            TokenKind::AndSym => BinOp::And,
            TokenKind::OrSym => BinOp::Or,
            TokenKind::DotProduct => BinOp::Dot,
            TokenKind::CrossProduct => BinOp::Cross,
            TokenKind::Hadamard => BinOp::Hadamard,
            TokenKind::TensorProduct => BinOp::Tensor,
            TokenKind::Word(w) => {
                // `x` is multiplication only with a space on each side.
                if w == "x" && !tok.is_spaced_x() {
                    return None;
                }
                *WORD_OPERATORS.iter().find(|(n, _)| n == w).map(|(_, o)| o)?
            }
            _ => return None,
        };
        Some(op)
    }

    fn advance_binop(&mut self, _op: BinOp) {
        self.advance();
    }

    fn unary_expr(&mut self) -> Option<Expr> {
        let start = self.peek().span;

        // Unary minus binds below powers and above multiplication, so `-x xx 2`
        // is `-(x²)`.
        if matches!(self.peek_kind(), TokenKind::Minus) {
            self.advance();
            let operand = self.binary_expr(UNARY_MINUS_PRECEDENCE)?;
            let span = start.to(operand.span);
            return Some(Expr {
                kind: ExprKind::Unary { op: UnOp::Neg, operand: Box::new(operand) },
                span,
            });
        }

        if matches!(self.peek_kind(), TokenKind::NotSym) {
            self.advance();
            let operand = self.binary_expr(NOT_PRECEDENCE)?;
            let span = start.to(operand.span);
            return Some(Expr {
                kind: ExprKind::Unary { op: UnOp::Not, operand: Box::new(operand) },
                span,
            });
        }

        if matches!(self.peek_kind(), TokenKind::Sqrt) {
            self.advance();
            let operand = self.unary_expr()?;
            let span = start.to(operand.span);
            return Some(Expr {
                kind: ExprKind::Unary { op: UnOp::Sqrt, operand: Box::new(operand) },
                span,
            });
        }

        // `|x|`, `⌊x⌋`, `⌈x⌉` — self-delimiting notation, so they bind tightest.
        for (open, close, op) in [
            (TokenKind::Bar, TokenKind::Bar, UnOp::Abs),
            (TokenKind::FloorOpen, TokenKind::FloorClose, UnOp::Floor),
            (TokenKind::CeilOpen, TokenKind::CeilClose, UnOp::Ceil),
        ] {
            if self.peek_kind() == &open {
                self.advance();
                let operand = self.expression()?;
                self.expect(close, "the closing bracket of the notation", "close it.");
                let span = start.to(self.last_span());
                return Some(Expr {
                    kind: ExprKind::Unary { op, operand: Box::new(operand) },
                    span,
                });
            }
        }

        if let Some(w) = self.peek().word() {
            if let Some((_, op)) = WORD_UNARY.iter().find(|(n, _)| *n == w) {
                let op = *op;
                self.advance();
                let operand = if op == UnOp::Not {
                    self.binary_expr(NOT_PRECEDENCE)?
                } else {
                    self.unary_expr()?
                };
                let span = start.to(operand.span);
                return Some(Expr {
                    kind: ExprKind::Unary { op, operand: Box::new(operand) },
                    span,
                });
            }
        }

        self.primary_expr()
    }

    fn primary_expr(&mut self) -> Option<Expr> {
        let span = self.peek().span;

        match self.peek_kind().clone() {
            TokenKind::Number(n) => {
                self.advance();
                Some(Expr { kind: ExprKind::Number(n), span })
            }
            TokenKind::Str(s) => {
                self.advance();
                Some(Expr { kind: ExprKind::Str(s), span })
            }
            TokenKind::Quoted(name) => {
                // A quoted name followed by `[` is a call; otherwise a literal value.
                if matches!(self.peek_at(1).kind, TokenKind::LBracket) {
                    self.advance();
                    let args = self.call_args()?;
                    let full = span.to(self.peek().span);
                    Some(Expr { kind: ExprKind::Call { name, args }, span: full })
                } else {
                    self.advance();
                    Some(Expr { kind: ExprKind::Literal(name), span })
                }
            }
            TokenKind::LParen => self.reference(),
            TokenKind::LBrace => self.array_literal(),
            TokenKind::MathOpen => {
                // `math` was already consumed as a word by the caller path below.
                self.advance();
                let inner = self.expression()?;
                self.expect(TokenKind::RBrace, "'}' to close the math block", "add a '}'.");
                let full = span.to(self.peek().span);
                Some(Expr { kind: ExprKind::Math(Box::new(inner)), span: full })
            }
            TokenKind::Word(w) => {
                if w == "math" {
                    self.advance();
                    if !self.expect(TokenKind::MathOpen, "'{' after math", "write math { … }.") {
                        return None;
                    }
                    let inner = self.expression()?;
                    self.expect(TokenKind::RBrace, "'}' to close the math block", "add a '}'.");
                    let full = span.to(self.peek().span);
                    return Some(Expr { kind: ExprKind::Math(Box::new(inner)), span: full });
                }
                if w == "if" {
                    let chain = self.if_chain()?;
                    let full = span.to(self.peek().span);
                    return Some(Expr { kind: ExprKind::If(Box::new(chain)), span: full });
                }
                if w == "loop" {
                    let l = self.loop_stmt()?;
                    let full = span.to(self.peek().span);
                    return Some(Expr { kind: ExprKind::Loop(Box::new(l)), span: full });
                }
                if let Some(c) = constant_from_word(&w) {
                    self.advance();
                    return Some(Expr { kind: ExprKind::Constant(c), span });
                }
                if BUILTINS.contains(&w.as_str()) {
                    self.advance();
                    let args = self.builtin_args()?;
                    let full = span.to(self.peek().span);
                    return Some(Expr { kind: ExprKind::Builtin { name: w, args }, span: full });
                }
                self.error(
                    span,
                    format!("'{w}' is not something AHPCL can evaluate here."),
                    "names are quoted, so write ('x') for a variable and 'f'[…] for a call.",
                );
                None
            }
            _ => {
                self.error(
                    span,
                    "expected a value here.",
                    "a value is a quoted literal, a ('reference'), a number inside math { }, or a call.",
                );
                None
            }
        }
    }

    /// `('name')` with any chained selectors, or `(expr)` as grouping.
    fn reference(&mut self) -> Option<Expr> {
        let start = self.peek().span;
        self.advance(); // (

        // A lone quoted name is a reference; anything else is a grouped expression.
        if let TokenKind::Quoted(name) = self.peek_kind().clone() {
            if matches!(self.peek_at(1).kind, TokenKind::RParen) {
                self.advance(); // name
                self.advance(); // )
                let selectors = self.selectors();
                let span = start.to(self.last_span());
                return Some(Expr { kind: ExprKind::Ref { name, selectors }, span });
            }
        }

        let inner = self.expression()?;
        self.expect(TokenKind::RParen, "')' to close the group", "add a ')'.");
        let selectors = self.selectors();
        let span = start.to(self.last_span());
        if selectors.is_empty() {
            Some(inner)
        } else {
            // A selector on a grouped expression: keep the group, attach the selectors
            // by wrapping it in a reference-like node is wrong, so treat it as a
            // parse error the type checker can explain better later.
            Some(Expr { kind: inner.kind, span })
        }
    }

    /// Zero or more chained `:…;` selectors.
    fn selectors(&mut self) -> Vec<Selector> {
        let mut out = Vec::new();
        while matches!(self.peek_kind(), TokenKind::Colon) {
            self.advance(); // :
            let Some(sel) = self.one_selector() else { break };
            out.push(sel);
            if !self.eat(&TokenKind::Semicolon) {
                let span = self.peek().span;
                self.error(
                    span,
                    "a selector is closed with ';'.",
                    "write ('a'):1, 3; — the semicolon says where the selection stops.",
                );
                break;
            }
        }
        out
    }

    fn one_selector(&mut self) -> Option<Selector> {
        if self.eat_word("all") {
            return Some(Selector::All);
        }
        if self.eat_word("length") {
            return Some(Selector::Length);
        }
        if self.eat_word("shape") {
            return Some(Selector::Shape);
        }

        // Parse above the range level, so `to` is left for us rather than being
        // folded into the expression.
        let first = self.binary_expr(1)?;

        if self.at_word("to") {
            self.advance();
            let to = self.binary_expr(1)?;
            let by = if self.eat_word("by") {
                Some(Box::new(self.binary_expr(1)?))
            } else {
                None
            };
            return Some(Selector::Range { from: Box::new(first), to: Box::new(to), by });
        }

        let mut indices = vec![first];
        while self.eat(&TokenKind::Comma) {
            indices.push(self.binary_expr(1)?);
        }
        Some(Selector::Indices(indices))
    }

    /// Builtin arguments, which may include bare option words: `parse[('t') trim]`
    /// and `parse[('t') group:"," decimal:"."]`.
    fn builtin_args(&mut self) -> Option<Vec<Expr>> {
        if !self.expect(TokenKind::LBracket, "'[' to open the arguments", "add a '['.") {
            return None;
        }
        let mut args = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBracket) && !self.at_end() {
            let before = self.pos;
            let span = self.peek().span;

            // A bare word here is an option, not a value — values are quoted.
            if let Some(word) = self.peek().word().map(str::to_string) {
                if constant_from_word(&word).is_none() {
                    self.advance();
                    let value = if self.eat(&TokenKind::Colon) {
                        Some(Box::new(self.primary_expr()?))
                    } else {
                        None
                    };
                    let full = span.to(self.peek().span);
                    args.push(Expr { kind: ExprKind::Option { name: word, value }, span: full });
                    continue;
                }
            }

            match self.expression() {
                Some(e) => args.push(e),
                None => break,
            }
            if self.pos == before {
                self.advance();
            }
        }
        self.expect(TokenKind::RBracket, "']' to close the arguments", "add a ']'.");
        Some(args)
    }

    /// `[a b c]` — space-separated, no commas.
    fn call_args(&mut self) -> Option<Vec<Expr>> {
        if !self.expect(TokenKind::LBracket, "'[' to open the arguments", "add a '['.") {
            return None;
        }
        let mut args = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBracket) && !self.at_end() {
            let before = self.pos;
            match self.expression() {
                Some(e) => args.push(e),
                None => break,
            }
            if self.pos == before {
                self.advance();
            }
        }
        self.expect(TokenKind::RBracket, "']' to close the arguments", "add a ']'.");
        Some(args)
    }

    /// `{ 'a', 'b' }` or `{ {…}, {…} }` — nested to mirror the shape.
    fn array_literal(&mut self) -> Option<Expr> {
        let start = self.peek().span;
        self.advance(); // {
        let mut items = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace) && !self.at_end() {
            let before = self.pos;
            match self.expression() {
                Some(e) => items.push(e),
                None => break,
            }
            if self.pos == before {
                self.advance();
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RBrace, "'}' to close the array", "add a '}'.");
        let span = start.to(self.last_span());
        Some(Expr { kind: ExprKind::ArrayLit(items), span })
    }
}

fn constant_from_word(w: &str) -> Option<Constant> {
    match w {
        "pi" | "π" => Some(Constant::Pi),
        "e" => Some(Constant::E),
        "tau" | "τ" => Some(Constant::Tau),
        _ => None,
    }
}

/// Check a rank name against its shape. `matrix` with `[3]` is an error.
pub fn check_rank(ty: &TypeRef, shape: &[Dim], span: Span) -> Option<Error> {
    let rank = ty.rank?;
    let expected = rank.dimensions();
    match expected {
        Some(n) if shape.len() != n => Some(Error::new(
            E_RANK_MISMATCH,
            span,
            format!(
                "'{}' is {}-dimensional, but shape [{}] is {}-dimensional.",
                rank.name(),
                n,
                shape.len(),
                shape.len()
            ),
            format!("use {} dimensions, or a different rank name.", n),
        )),
        None if shape.len() < 3 => Some(Error::new(
            E_RANK_MISMATCH,
            span,
            format!(
                "'tensor' is for three dimensions or more, but this shape has {}.",
                shape.len()
            ),
            "use vector for one dimension or matrix for two.".to_string(),
        )),
        _ => None,
    }
}
