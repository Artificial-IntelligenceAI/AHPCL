//! The syntax tree.
//!
//! Arena-free and owned: the tree is small relative to compile time, and index-based
//! IR belongs further down the pipeline where it earns its keep.

use ahpcl_diagnostics::Span;

// ── types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    Positive,
    Negative,
}

/// `vector` / `matrix` / `tensor`. The name is required and cross-checks the shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Vector,
    Matrix,
    Tensor,
}

impl Rank {
    pub fn name(self) -> &'static str {
        match self {
            Rank::Vector => "vector",
            Rank::Matrix => "matrix",
            Rank::Tensor => "tensor",
        }
    }

    /// How many dimensions the name claims. `tensor` accepts three or more.
    pub fn dimensions(self) -> Option<usize> {
        match self {
            Rank::Vector => Some(1),
            Rank::Matrix => Some(2),
            Rank::Tensor => None,
        }
    }

    pub fn from_word(w: &str) -> Option<Rank> {
        match w {
            "vector" => Some(Rank::Vector),
            "matrix" => Some(Rank::Matrix),
            "tensor" => Some(Rank::Tensor),
            _ => None,
        }
    }
}

/// One dimension of a shape. `?` means "not knowable at compile time".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dim {
    Known(u64),
    Unknown,
}

/// `[32 bit]` or `[100 digits]`. Bits are storage size; digits are how much of an
/// irrational to compute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precision {
    Bits(u32),
    Digits(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    pub sign: Option<Sign>,
    pub rank: Option<Rank>,
    /// `num`, `int`, `deci`, `rat`, `infnum`, `str`, `nna`, `bool`, `none`.
    pub base: String,
    pub shape: Option<Vec<Dim>>,
    pub precision: Option<Precision>,
    pub span: Span,
}

// ── expressions ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    IntDiv,
    Mod,
    Pow,
    Eq,
    NotEq,
    Less,
    Greater,
    LessEq,
    GreaterEq,
    And,
    Or,
    /// `·` — dot product, and matrix multiplication, which are the same operation.
    Dot,
    /// `×` — cross product.
    Cross,
    /// `⊙` — elementwise.
    Hadamard,
    /// `⊗` — tensor product.
    Tensor,
}

impl BinOp {
    /// Standard mathematical precedence. Higher binds first.
    pub fn precedence(self) -> u8 {
        match self {
            BinOp::Pow => 7,
            BinOp::Mul
            | BinOp::Div
            | BinOp::IntDiv
            | BinOp::Mod
            | BinOp::Dot
            | BinOp::Cross
            | BinOp::Hadamard
            | BinOp::Tensor => 5,
            BinOp::Add | BinOp::Sub => 4,
            BinOp::Eq
            | BinOp::NotEq
            | BinOp::Less
            | BinOp::Greater
            | BinOp::LessEq
            | BinOp::GreaterEq => 3,
            BinOp::And => 1,
            BinOp::Or => 0,
        }
    }

    /// Powers group right to left; everything else groups left to right.
    pub fn right_associative(self) -> bool {
        matches!(self, BinOp::Pow)
    }
}

/// Unary minus sits at precedence 6 — below powers, above multiplication. That is
/// what makes `-x xx 2` mean `-(x²)`.
pub const UNARY_MINUS_PRECEDENCE: u8 = 6;
/// `not` binds looser than comparison, so `not a = b` is `not (a = b)`.
pub const NOT_PRECEDENCE: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Sqrt,
    Abs,
    Floor,
    Ceil,
    Sin,
    Cos,
    Tan,
    Log,
    Ln,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Constant {
    Pi,
    E,
    Tau,
}

/// A selector: `:all;`, `:3;`, `:1, 3, 9;`, `:1 to 100 by 2;`, `:length;`, `:shape;`.
#[derive(Debug, Clone, PartialEq)]
pub enum Selector {
    All,
    Length,
    Shape,
    Indices(Vec<Expr>),
    Range {
        from: Box<Expr>,
        to: Box<Expr>,
        by: Option<Box<Expr>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// `'1000'` — a quoted literal value.
    Literal(String),
    /// `"text"`.
    Str(String),
    /// A bare number inside `math { }`.
    Number(String),
    /// `('name')` with any chained selectors.
    Ref { name: String, selectors: Vec<Selector> },
    /// `math { … }`.
    Math(Box<Expr>),
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Unary { op: UnOp, operand: Box<Expr> },
    /// `'name'[args]` — a user function.
    Call { name: String, args: Vec<Expr> },
    /// `name[args]` — a builtin. Bare means builtin.
    Builtin { name: String, args: Vec<Expr> },
    /// `{ … }` — an array literal, nested to mirror the shape.
    ArrayLit(Vec<Expr>),
    Constant(Constant),
    /// A conditional used for its value.
    If(Box<IfChain>),
    /// A loop used for its value: each `handback` contributes one element.
    Loop(Box<LoopStmt>),
    /// `a to b by c`, outside a selector.
    Range { from: Box<Expr>, to: Box<Expr>, by: Option<Box<Expr>> },
}

// ── statements ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub name: String,
    pub name_span: Span,
    pub shape: Option<Vec<Dim>>,
    pub precision: Option<Precision>,
    pub value: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub ty: TypeRef,
    /// `,` extends: several separate variables sharing one type header.
    pub bindings: Vec<Binding>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeStmt {
    pub ty: TypeRef,
    pub name: String,
    pub name_span: Span,
    /// A selector on the left writes to one element.
    pub selectors: Vec<Selector>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub ty: TypeRef,
    pub name: String,
    pub name_span: Span,
    pub shape: Option<Vec<Dim>>,
    pub precision: Option<Precision>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuncDecl {
    pub returns: TypeRef,
    pub name: String,
    pub name_span: Span,
    pub params: Vec<Param>,
    pub body: Block,
    pub span: Span,
}

pub type Block = Vec<Stmt>;

#[derive(Debug, Clone, PartialEq)]
pub struct IfArm {
    pub condition: Option<Expr>,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfChain {
    pub arms: Vec<IfArm>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoopKind {
    /// `loop:var:int 'i' = math { 1 to 10 }` — always terminates.
    Counted {
        ty: TypeRef,
        var: String,
        var_span: Span,
        range: Expr,
    },
    /// `loop:while math { … }` — may not terminate.
    While { condition: Expr },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoopStmt {
    pub kind: LoopKind,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Var(VarDecl),
    Change(ChangeStmt),
    Func(FuncDecl),
    If(IfChain),
    Loop(LoopStmt),
    Print { args: Vec<Expr>, span: Span },
    /// `handback` / `hb` — hands a value out of a block.
    Handback { value: Expr, span: Span },
    /// A bare expression, such as a call performed for its effect.
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub statements: Vec<Stmt>,
}
