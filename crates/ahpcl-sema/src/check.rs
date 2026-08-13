//! The type checker.
//!
//! Bidirectional: an expression is checked *against* an expected type where one is
//! known, and inferred otherwise. That is what makes "polymorphic until pinned by
//! context" work for numeric literals and for division — and what makes the absence of
//! a pin an error rather than a silent default.

use ahpcl_diagnostics::{Category, Code, Error, Informer, Span};
use ahpcl_syntax::ast::*;

use crate::scope::{closest, Declared, Function, Scopes, Variable};
use crate::types::*;

const E_AMBIGUOUS: Code = Code::new(Category::Type, 1);
const E_MISMATCH: Code = Code::new(Category::Type, 2);
const E_NOT_NUMERIC: Code = Code::new(Category::Type, 3);
const E_NO_SUCH_NAME: Code = Code::new(Category::Name, 1);
const E_NO_SUCH_FUNC: Code = Code::new(Category::Name, 2);
const E_ARG_COUNT: Code = Code::new(Category::Name, 3);
const E_SHAPE_MISMATCH: Code = Code::new(Category::Shape, 1);
const E_RANK_MISMATCH: Code = Code::new(Category::Shape, 2);
const E_LITERAL_SHAPE: Code = Code::new(Category::Shape, 3);
const E_READ_ONLY: Code = Code::new(Category::Sign, 2);
const E_TYPE_RESTATED: Code = Code::new(Category::Type, 4);
const E_PREC_ON_INFNUM: Code = Code::new(Category::Prec, 2);
const E_DECI_WIDTH: Code = Code::new(Category::Prec, 3);

pub struct Checked {
    pub errors: Vec<Error>,
}

pub fn check(program: &Program, informer: &mut Informer) -> Checked {
    let mut c = Checker { scopes: Scopes::new(), errors: Vec::new(), informer };
    c.collect_functions(program);
    for stmt in &program.statements {
        c.statement(stmt, None);
    }
    Checked { errors: c.errors }
}

struct Checker<'a> {
    scopes: Scopes,
    errors: Vec<Error>,
    informer: &'a mut Informer,
}

impl<'a> Checker<'a> {
    fn err(&mut self, code: Code, span: Span, rule: impl Into<String>, fix: impl Into<String>) {
        self.errors.push(Error::new(code, span, rule, fix));
    }

    /// Functions are visible before their declaration, so order does not matter.
    fn collect_functions(&mut self, program: &Program) {
        for stmt in &program.statements {
            if let Stmt::Func(f) = stmt {
                let params: Vec<Type> = f
                    .params
                    .iter()
                    .filter_map(|p| {
                        from_type_ref(&p.ty, p.shape.as_ref(), p.precision.as_ref())
                    })
                    .collect();
                let Some(returns) = from_type_ref(&f.returns, None, None) else { continue };
                self.scopes.functions.insert(
                    f.name.clone(),
                    Function {
                        params,
                        param_names: f.params.iter().map(|p| p.name.clone()).collect(),
                        returns,
                        declared_at: f.name_span,
                    },
                );
            }
        }
    }

    // ── statements ──────────────────────────────────────────────────────────

    /// `expected_handback` is the type a `handback` must produce, when inside a
    /// function body.
    fn statement(&mut self, stmt: &Stmt, expected_handback: Option<&Type>) {
        match stmt {
            Stmt::Var(v) => self.var_decl(v),
            Stmt::Change(c) => self.change(c),
            Stmt::Func(f) => self.func(f),
            Stmt::If(chain) => {
                self.if_chain(chain, None, expected_handback);
            }
            Stmt::Loop(l) => {
                self.loop_stmt(l, expected_handback);
            }
            Stmt::Print { args, .. } => {
                for a in args {
                    self.expr(a, None);
                }
            }
            Stmt::Handback { value, span } => {
                let got = self.expr(value, expected_handback);
                if let (Some(want), Some(got)) = (expected_handback, got) {
                    self.require_fits(&got, want, *span, "handed back");
                }
            }
            Stmt::Expr(e) => {
                self.expr(e, None);
            }
        }
    }

    fn var_decl(&mut self, v: &VarDecl) {
        for binding in &v.bindings {
            let Some(mut ty) = from_type_ref(&v.ty, binding.shape.as_ref(), binding.precision.as_ref())
            else {
                continue;
            };

            self.check_precision(&ty, v.ty.span);
            self.check_rank(&v.ty, &ty, binding.name_span);

            if let Some(value) = &binding.value {
                if let Some(got) = self.expr(value, Some(&ty)) {
                    self.require_fits(&got, &ty, value.span, "assigned");
                    // A literal may determine the shape when none was written.
                    if ty.shape.is_none() {
                        if let Some(shape) = got.shape.clone() {
                            ty.shape = Some(shape);
                        }
                    } else if let (Some(want), Some(have)) = (&ty.shape, &got.shape) {
                        if !want.agrees_with(have) {
                            self.err(
                                E_LITERAL_SHAPE,
                                value.span,
                                format!(
                                    "the value is {} but '{}' is declared {}.",
                                    have.render(),
                                    binding.name,
                                    want.render()
                                ),
                                "make the literal match the declared shape, or drop the shape and let the literal decide.",
                            );
                        }
                    }
                }
            }

            self.declare(&binding.name, ty, binding.name_span, false);
        }
    }

    fn declare(&mut self, name: &str, ty: Type, span: Span, read_only: bool) {
        let outcome = self
            .scopes
            .declare(name, Variable { ty, declared_at: span, read_only });
        if let Declared::Shadows { previous } = outcome {
            let msg = format!("'{name}' here shadows '{name}' declared earlier");
            let _ = previous;
            self.informer.say(span.start, msg);
        }
    }

    fn change(&mut self, c: &ChangeStmt) {
        let Some(stated) = from_type_ref(&c.ty, None, None) else { return };

        let Some(existing) = self.scopes.lookup(&c.name).cloned() else {
            self.unknown_name(&c.name, c.name_span);
            return;
        };

        if existing.read_only {
            self.err(
                E_READ_ONLY,
                c.name_span,
                format!("'{}' is a loop counter, which is read-only inside the body.", c.name),
                "a counted loop runs a fixed number of times; use a separate variable.",
            );
            return;
        }

        // The restated type is documentation for the reader, so it is verified.
        // Documentation that can drift out of sync is worse than none.
        let target = if c.selectors.is_empty() {
            existing.ty.clone()
        } else {
            existing.ty.element()
        };

        if stated.base != target.base || stated.sign != target.sign {
            self.errors.push(
                Error::new(
                    E_TYPE_RESTATED,
                    c.ty.span,
                    format!(
                        "'{}' is {}, but this says {}.",
                        c.name,
                        target.render(),
                        stated.render()
                    ),
                    format!("write change:var:{} '{}' = … .", target.render(), c.name),
                )
                .with_label(existing.declared_at, "declared here")
                .with_label(c.ty.span, "restated differently here"),
            );
        }

        if let Some(got) = self.expr(&c.value, Some(&target)) {
            self.require_fits(&got, &target, c.value.span, "assigned");
        }
    }

    fn func(&mut self, f: &FuncDecl) {
        let Some(returns) = from_type_ref(&f.returns, None, None) else { return };
        self.scopes.push();
        for p in &f.params {
            if let Some(ty) = from_type_ref(&p.ty, p.shape.as_ref(), p.precision.as_ref()) {
                self.check_rank(&p.ty, &ty, p.name_span);
                self.declare(&p.name, ty, p.name_span, false);
            }
        }
        let want = if returns.base == Base::None { None } else { Some(&returns) };
        for stmt in &f.body {
            self.statement(stmt, want);
        }
        self.scopes.pop();

        if returns.base != Base::None && !block_hands_back(&f.body) {
            self.err(
                E_MISMATCH,
                f.name_span,
                format!("'{}' produces {}, but no path hands a value back.", f.name, returns.render()),
                "add a handback, or declare the function as func:none.",
            );
        }
    }

    /// Returns the type the chain produces, when every arm hands one back.
    fn if_chain(
        &mut self,
        chain: &IfChain,
        expected: Option<&Type>,
        outer_handback: Option<&Type>,
    ) -> Option<Type> {
        let mut produced: Option<Type> = None;
        let mut all_produce = !chain.arms.is_empty();
        let has_else = chain.arms.iter().any(|a| a.condition.is_none());

        for arm in &chain.arms {
            if let Some(cond) = &arm.condition {
                if let Some(ty) = self.expr(cond, Some(&Type::scalar(Base::Bool))) {
                    if ty.base != Base::Bool {
                        self.err(
                            E_MISMATCH,
                            cond.span,
                            format!("a condition must be a bool, but this is {}.", ty.render()),
                            "compare something, as in math { ('x') > 5 }.",
                        );
                    }
                }
            }

            self.scopes.push();
            // Inside an arm, a handback belongs to the value the if produces when the
            // if is being used for its value; otherwise to the enclosing function.
            let inner_want = expected.or(outer_handback);
            for stmt in &arm.body {
                self.statement(stmt, inner_want);
            }
            self.scopes.pop();

            match handback_type(&arm.body) {
                Some(_) => {
                    if produced.is_none() {
                        produced = expected.cloned();
                    }
                }
                None => all_produce = false,
            }
        }

        if expected.is_some() && (!all_produce || !has_else) {
            return None;
        }
        produced.or_else(|| expected.cloned())
    }

    fn loop_stmt(&mut self, l: &LoopStmt, outer_handback: Option<&Type>) -> Option<Type> {
        self.scopes.push();

        match &l.kind {
            LoopKind::Counted { ty, var, var_span, range } => {
                let counter = from_type_ref(ty, None, None).unwrap_or(Type::scalar(Base::Int));
                self.expr(range, Some(&counter));
                // The counter is read-only, which is what makes a counted loop
                // provably terminating.
                self.declare(var, counter, *var_span, true);
            }
            LoopKind::While { condition } => {
                if let Some(ty) = self.expr(condition, Some(&Type::scalar(Base::Bool))) {
                    if ty.base != Base::Bool {
                        self.err(
                            E_MISMATCH,
                            condition.span,
                            format!("a loop condition must be a bool, but this is {}.", ty.render()),
                            "compare something, as in math { ('n') > 1 }.",
                        );
                    }
                }
            }
        }

        for stmt in &l.body {
            self.statement(stmt, outer_handback);
        }
        self.scopes.pop();
        None
    }

    // ── expressions ─────────────────────────────────────────────────────────

    fn expr(&mut self, e: &Expr, expected: Option<&Type>) -> Option<Type> {
        match &e.kind {
            ExprKind::Literal(text) => self.literal(text, e.span, expected),
            ExprKind::Str(_) => Some(Type::scalar(Base::Str)),
            ExprKind::Number(text) => self.literal(text, e.span, expected),
            ExprKind::Constant(_) => {
                // π, e and τ are irrational, so they are polymorphic until pinned.
                match expected {
                    Some(t) if t.base.is_numeric() => Some(t.clone()),
                    _ => Some(Type::scalar(Base::Deci)),
                }
            }
            ExprKind::Ref { name, selectors } => self.reference(name, selectors, e.span),
            ExprKind::Math(inner) => self.expr(inner, expected),
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e.span, expected),
            ExprKind::Unary { op, operand } => self.unary(*op, operand, e.span, expected),
            ExprKind::Call { name, args } => self.call(name, args, e.span),
            ExprKind::Builtin { name, args } => self.builtin(name, args, e.span, expected),
            ExprKind::ArrayLit(items) => self.array_literal(items, e.span, expected),
            ExprKind::If(chain) => self.if_chain(chain, expected, None),
            ExprKind::Loop(l) => {
                self.loop_stmt(l, expected.map(|t| t.element()).as_ref());
                expected.cloned()
            }
            ExprKind::Range { from, to, by } => {
                let want = Type::scalar(Base::Int);
                self.expr(from, Some(&want));
                self.expr(to, Some(&want));
                if let Some(b) = by {
                    self.expr(b, Some(&want));
                }
                Some(want)
            }
        }
    }

    /// A numeric literal is polymorphic until pinned by context. With nothing to pin
    /// it, that is an error rather than a silent default.
    fn literal(&mut self, text: &str, span: Span, expected: Option<&Type>) -> Option<Type> {
        let looks_numeric = text
            .strip_prefix(['-', '+'])
            .unwrap_or(text)
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.')
            && text.chars().any(|c| c.is_ascii_digit());

        if !looks_numeric {
            // 'true' / 'false' are the bool literals.
            if text == "true" || text == "false" {
                return Some(Type::scalar(Base::Bool));
            }
            return Some(Type::scalar(Base::Str));
        }

        let has_point = text.contains('.');
        match expected {
            Some(t) if t.base.is_numeric() => {
                let elem = t.element();
                if has_point && elem.base == Base::Int {
                    self.err(
                        E_MISMATCH,
                        span,
                        format!("'{text}' has a fractional part, but an int was expected."),
                        "use deci, rat or num, or write a whole number.",
                    );
                }
                Some(elem)
            }
            Some(t) => {
                self.err(
                    E_MISMATCH,
                    span,
                    format!("'{text}' is a number, but {} was expected.", t.render()),
                    "check the declared type.",
                );
                None
            }
            None => {
                self.err(
                    E_AMBIGUOUS,
                    span,
                    format!("nothing here says what kind of number '{text}' is."),
                    "give the surrounding declaration a type, such as :deci or :int.",
                );
                None
            }
        }
    }

    fn reference(&mut self, name: &str, selectors: &[Selector], span: Span) -> Option<Type> {
        let Some(var) = self.scopes.lookup(name).cloned() else {
            self.unknown_name(name, span);
            return None;
        };

        let mut ty = var.ty.clone();
        for sel in selectors {
            ty = self.apply_selector(ty, sel, span)?;
        }
        Some(ty)
    }

    /// `:length;` is a count, `:shape;` is a vector of dimensions, and indices reduce
    /// the rank — one index gives a plain value, several give an array.
    fn apply_selector(&mut self, ty: Type, sel: &Selector, span: Span) -> Option<Type> {
        match sel {
            Selector::Length => Some(Type::scalar(Base::Int)),
            Selector::Shape => Some(Type {
                base: Base::Int,
                sign: None,
                shape: Some(Shape(vec![Dim::Known(
                    ty.shape.as_ref().map(|s| s.rank() as u64).unwrap_or(0),
                )])),
                precision: None,
            }),
            Selector::All => Some(ty),
            Selector::Indices(indices) => {
                for i in indices {
                    self.expr(i, Some(&Type::scalar(Base::Int)));
                }
                let Some(shape) = ty.shape.clone() else {
                    self.err(
                        E_SHAPE_MISMATCH,
                        span,
                        format!("{} is not an array, so it cannot be selected from.", ty.render()),
                        "selectors apply to vectors, matrices and tensors.",
                    );
                    return None;
                };
                if indices.len() == 1 {
                    // One index gives a plain value; the remaining dimensions stay.
                    let rest: Vec<Dim> = shape.0[1..].to_vec();
                    Some(Type {
                        base: ty.base,
                        sign: ty.sign,
                        shape: if rest.is_empty() { None } else { Some(Shape(rest)) },
                        precision: ty.precision.clone(),
                    })
                } else {
                    let mut dims = shape.0.clone();
                    dims[0] = Dim::Known(indices.len() as u64);
                    Some(Type { shape: Some(Shape(dims)), ..ty })
                }
            }
            Selector::Range { from, to, by } => {
                let want = Type::scalar(Base::Int);
                self.expr(from, Some(&want));
                self.expr(to, Some(&want));
                if let Some(b) = by {
                    self.expr(b, Some(&want));
                }
                Some(ty)
            }
        }
    }

    fn binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
        expected: Option<&Type>,
    ) -> Option<Type> {
        use BinOp::*;

        // Comparisons and logic produce a bool, so the operands are not pinned by the
        // surrounding expectation.
        let comparison = matches!(op, Eq | NotEq | Less | Greater | LessEq | GreaterEq);
        let logical = matches!(op, And | Or);

        if logical {
            let want = Type::scalar(Base::Bool);
            self.expr(lhs, Some(&want));
            self.expr(rhs, Some(&want));
            return Some(want);
        }

        let operand_expectation = if comparison { None } else { expected };
        let a = self.expr(lhs, operand_expectation);
        let b = self.expr(rhs, operand_expectation.or(a.as_ref()));

        if comparison {
            return Some(Type::scalar(Base::Bool));
        }

        let (a, b) = (a?, b?);

        // The array operators imply `:all;` and demand arrays.
        if matches!(op, Dot | Cross | Hadamard | Tensor) {
            return self.array_operator(op, &a, &b, span);
        }

        if !a.base.is_numeric() || !b.base.is_numeric() {
            let offender = if a.base.is_numeric() { &b } else { &a };
            self.err(
                E_NOT_NUMERIC,
                span,
                format!("arithmetic needs numbers, but this is {}.", offender.render()),
                "only num, rat, deci, int and infnum take arithmetic.",
            );
            return None;
        }

        // A bare array reference sums its elements, so arithmetic on it is scalar.
        let a_elem = a.element();
        let b_elem = b.element();

        let base = match op {
            Div => {
                // Division's result is pinned by context; `num` spans rat and deci, so
                // it does not pin, and that is an error rather than a guess.
                match expected.map(|t| t.element()) {
                    Some(t) if matches!(t.base, Base::Rat | Base::Deci | Base::InfNum) => t.base,
                    Some(t) if t.base == Base::Int => Base::Int,
                    _ => {
                        self.err(
                            E_AMBIGUOUS,
                            span,
                            "division produces either an exact rational or a decimal, and nothing here says which.",
                            "give the surrounding declaration a concrete type, such as :deci or :rat.",
                        );
                        return None;
                    }
                }
            }
            IntDiv | Mod => Base::Int,
            _ => a_elem.base.join(b_elem.base)?,
        };

        let sign = match op {
            Add => sign_add(a_elem.sign, b_elem.sign),
            Sub => sign_sub(a_elem.sign, b_elem.sign),
            Mul | Div | IntDiv => sign_mul(a_elem.sign, b_elem.sign),
            Pow => sign_pow(a_elem.sign),
            _ => None,
        };

        Some(Type { base, sign, shape: None, precision: None })
    }

    fn array_operator(&mut self, op: BinOp, a: &Type, b: &Type, span: Span) -> Option<Type> {
        let (Some(sa), Some(sb)) = (&a.shape, &b.shape) else {
            self.err(
                E_SHAPE_MISMATCH,
                span,
                "this operator works on arrays, but one side is a single value.",
                "use ordinary arithmetic for single numbers.",
            );
            return None;
        };

        match op {
            BinOp::Dot => {
                // The dot product *is* matrix multiplication: the inner dimensions
                // must agree.
                let inner_a = sa.0.last().cloned();
                let inner_b = sb.0.first().cloned();
                if let (Some(Dim::Known(x)), Some(Dim::Known(y))) = (&inner_a, &inner_b) {
                    if x != y {
                        self.err(
                            E_SHAPE_MISMATCH,
                            span,
                            format!(
                                "matrix multiplication requires inner dimensions to agree. \
                                 {} · {} — {x} ≠ {y}.",
                                sa.render(),
                                sb.render()
                            ),
                            "transpose one side, or declare a matching shape.",
                        );
                        return None;
                    }
                }
                if sa.rank() == 1 && sb.rank() == 1 {
                    Some(Type::scalar(a.base.join(b.base)?))
                } else {
                    let dims = vec![
                        sa.0.first().cloned().unwrap_or(Dim::Unknown),
                        sb.0.last().cloned().unwrap_or(Dim::Unknown),
                    ];
                    Some(Type {
                        base: a.base.join(b.base)?,
                        sign: sign_mul(a.sign, b.sign),
                        shape: Some(Shape(dims)),
                        precision: None,
                    })
                }
            }
            BinOp::Cross => {
                let three = |s: &Shape| s.rank() == 1 && matches!(s.0[0], Dim::Known(3));
                if !three(sa) || !three(sb) {
                    self.err(
                        E_SHAPE_MISMATCH,
                        span,
                        format!(
                            "cross product is defined for two 3-element vectors, but these are {} and {}.",
                            sa.render(),
                            sb.render()
                        ),
                        "use · for the dot product, or ⊙ for elementwise.",
                    );
                    return None;
                }
                Some(a.clone())
            }
            BinOp::Hadamard => {
                if !sa.agrees_with(sb) {
                    self.err(
                        E_SHAPE_MISMATCH,
                        span,
                        format!(
                            "elementwise operations need matching shapes, but these are {} and {}.",
                            sa.render(),
                            sb.render()
                        ),
                        "make the shapes agree.",
                    );
                    return None;
                }
                Some(Type {
                    base: a.base.join(b.base)?,
                    sign: sign_mul(a.sign, b.sign),
                    shape: Some(sa.clone()),
                    precision: None,
                })
            }
            BinOp::Tensor => {
                let dims: Vec<Dim> = sa.0.iter().chain(&sb.0).cloned().collect();
                Some(Type {
                    base: a.base.join(b.base)?,
                    sign: sign_mul(a.sign, b.sign),
                    shape: Some(Shape(dims)),
                    precision: None,
                })
            }
            _ => None,
        }
    }

    fn unary(&mut self, op: UnOp, operand: &Expr, span: Span, expected: Option<&Type>) -> Option<Type> {
        match op {
            UnOp::Not => {
                let want = Type::scalar(Base::Bool);
                self.expr(operand, Some(&want));
                Some(want)
            }
            UnOp::Neg => {
                let t = self.expr(operand, expected)?;
                Some(Type { sign: sign_neg(t.sign), ..t })
            }
            UnOp::Floor | UnOp::Ceil => {
                let t = self.expr(operand, None)?;
                Some(Type::scalar(if t.base.is_numeric() { Base::Int } else { t.base }))
            }
            UnOp::Abs => {
                let t = self.expr(operand, expected)?;
                Some(Type { sign: None, ..t })
            }
            // sqrt, sin, cos, tan, log, ln usually produce an irrational. A deci may
            // hold a rounded one, and the Informer reports the rounding.
            _ => {
                // The operand still needs pinning, or a bare number inside it would be
                // reported as ambiguous.
                let operand_want = match expected.map(|t| t.element()) {
                    Some(t) if t.base.is_numeric() => t,
                    _ => Type::scalar(Base::Deci),
                };
                let t = self.expr(operand, Some(&operand_want));
                if let Some(t) = &t {
                    if !t.base.is_numeric() {
                        self.err(
                            E_NOT_NUMERIC,
                            span,
                            format!("this operation needs a number, but got {}.", t.render()),
                            "check the operand's type.",
                        );
                        return None;
                    }
                }
                let result = match expected.map(|t| t.element()) {
                    Some(t) if t.base.is_numeric() => t,
                    _ => Type::scalar(Base::Deci),
                };
                if result.base == Base::Int {
                    self.err(
                        E_MISMATCH,
                        span,
                        "this operation usually produces an irrational, which an int cannot hold.                          √9 is exactly 3, but √2 is not, and which one you have is not always                          knowable before the program runs.",
                        "use deci for a rounded result, or infnum with a digit count.",
                    );
                    return None;
                }
                if result.base == Base::Deci {
                    self.informer.say(
                        span.start,
                        "result may be irrational; rounded to the declared decimal precision",
                    );
                }
                Some(result)
            }
        }
    }

    fn call(&mut self, name: &str, args: &[Expr], span: Span) -> Option<Type> {
        let Some(func) = self.scopes.functions.get(name).cloned() else {
            let names: Vec<String> = self.scopes.functions.keys().cloned().collect();
            let suggestion = closest(name, names.iter().map(String::as_str));
            self.err(
                E_NO_SUCH_FUNC,
                span,
                format!("there is no function called '{name}'."),
                match suggestion {
                    Some(s) => format!("did you mean '{s}'?"),
                    None => "check the name, and that the function is declared.".to_string(),
                },
            );
            return None;
        };

        if args.len() != func.params.len() {
            self.err(
                E_ARG_COUNT,
                span,
                format!(
                    "'{name}' takes {} argument{}, but {} {} given.",
                    func.params.len(),
                    if func.params.len() == 1 { "" } else { "s" },
                    args.len(),
                    if args.len() == 1 { "was" } else { "were" }
                ),
                "arguments are space-separated, in the order the parameters were declared.",
            );
        }

        for (i, arg) in args.iter().enumerate() {
            let want = func.params.get(i);
            if let Some(got) = self.expr(arg, want) {
                if let Some(want) = want {
                    self.require_fits_named(&got, want, arg.span, &func.param_names[i]);
                }
            }
        }

        Some(func.returns)
    }

    fn builtin(&mut self, name: &str, args: &[Expr], span: Span, expected: Option<&Type>) -> Option<Type> {
        match name {
            "print" => {
                for a in args {
                    self.expr(a, None);
                }
                Some(Type::scalar(Base::None))
            }
            "read" => {
                for a in args {
                    self.expr(a, Some(&Type::scalar(Base::Str)));
                }
                Some(Type::scalar(Base::Str))
            }
            "parse" => {
                if let Some(first) = args.first() {
                    self.expr(first, Some(&Type::scalar(Base::Str)));
                }
                // Options after the first argument are words, not values.
                match expected.map(|t| t.element()) {
                    Some(t) if t.base.is_numeric() => Some(t),
                    _ => {
                        self.err(
                            E_AMBIGUOUS,
                            span,
                            "parse produces a number, and nothing here says which kind.",
                            "give the surrounding declaration a type, such as :int or :deci.",
                        );
                        None
                    }
                }
            }
            "clock" => Some(Type::scalar(Base::Deci)),
            _ => {
                self.err(
                    E_NO_SUCH_FUNC,
                    span,
                    format!("'{name}' is not a builtin AHPCL knows."),
                    "the builtins are print, read, parse and clock. A user function is quoted: 'name'[…].",
                );
                None
            }
        }
    }

    fn array_literal(&mut self, items: &[Expr], span: Span, expected: Option<&Type>) -> Option<Type> {
        let elem_expect = expected.map(|t| t.element());

        // A nested literal gains a dimension.
        let mut inner_shape: Option<Shape> = None;
        for item in items {
            if let ExprKind::ArrayLit(_) = item.kind {
                let nested_expect = expected.and_then(|t| {
                    t.shape.as_ref().map(|s| Type {
                        shape: Some(Shape(s.0[1..].to_vec())),
                        ..t.clone()
                    })
                });
                if let Some(t) = self.expr(item, nested_expect.as_ref().or(elem_expect.as_ref())) {
                    if let Some(s) = t.shape {
                        match &inner_shape {
                            Some(first) if !first.agrees_with(&s) => {
                                self.err(
                                    E_LITERAL_SHAPE,
                                    item.span,
                                    "every row of an array must be the same length.",
                                    "arrays are rectangular; ragged rows are not allowed.",
                                );
                            }
                            None => inner_shape = Some(s),
                            _ => {}
                        }
                    }
                }
            } else {
                self.expr(item, elem_expect.as_ref());
            }
        }

        let mut dims = vec![Dim::Known(items.len() as u64)];
        if let Some(inner) = inner_shape {
            dims.extend(inner.0);
        }

        let base = elem_expect.as_ref().map(|t| t.base).unwrap_or(Base::Num);
        let sign = elem_expect.as_ref().and_then(|t| t.sign);
        let _ = span;
        Some(Type { base, sign, shape: Some(Shape(dims)), precision: None })
    }

    // ── checks ──────────────────────────────────────────────────────────────

    fn require_fits(&mut self, got: &Type, want: &Type, span: Span, what: &str) {
        if got.fits_in(want) {
            if got.widens_to(want) {
                self.informer.say(
                    span.start,
                    format!("{} as {} where {} expected; widened", what, got.render(), want.render()),
                );
            }
            return;
        }
        self.err(
            E_MISMATCH,
            span,
            format!("this is {}, but {} was expected.", got.render(), want.render()),
            widening_hint(got, want),
        );
    }

    fn require_fits_named(&mut self, got: &Type, want: &Type, span: Span, param: &str) {
        if got.fits_in(want) {
            if got.widens_to(want) {
                self.informer.say(
                    span.start,
                    format!(
                        "'{param}' passed as {} where {} expected; widened",
                        got.render(),
                        want.render()
                    ),
                );
            }
            return;
        }
        self.err(
            E_MISMATCH,
            span,
            format!(
                "'{param}' expects {}, but this is {}.",
                want.render(),
                got.render()
            ),
            widening_hint(got, want),
        );
    }

    fn check_rank(&mut self, ty_ref: &TypeRef, ty: &Type, span: Span) {
        let (Some(rank), Some(shape)) = (ty_ref.rank, &ty.shape) else { return };
        if !rank_matches(rank, shape) {
            let expected = match rank.dimensions() {
                Some(n) => format!("{n}-dimensional"),
                None => "three or more dimensions".to_string(),
            };
            self.err(
                E_RANK_MISMATCH,
                span,
                format!(
                    "'{}' is {expected}, but shape {} has {}.",
                    rank.name(),
                    shape.render(),
                    shape.rank()
                ),
                "use vector for one dimension, matrix for two, tensor for three or more.",
            );
        }
    }

    fn check_precision(&mut self, ty: &Type, span: Span) {
        let Some(p) = &ty.precision else { return };
        match (ty.base, p) {
            (Base::InfNum, Precision::Bits(_)) => self.err(
                E_PREC_ON_INFNUM,
                span,
                "infnum is unbounded, so a bit width means nothing for it.",
                "drop the width, or use [n digits] to say how much of an irrational to compute.",
            ),
            (Base::Deci, Precision::Bits(b)) if !matches!(b, 32 | 64 | 128) => self.err(
                E_DECI_WIDTH,
                span,
                format!("{b} is not an IEEE decimal format."),
                "decimals are 32, 64 or 128 bit. decimal128 gives 34 digits, which is what financial systems use.",
            ),
            _ => {}
        }
    }

    fn unknown_name(&mut self, name: &str, span: Span) {
        let visible: Vec<String> = self.scopes.visible_names().iter().map(|s| s.to_string()).collect();
        let suggestion = closest(name, visible.iter().map(String::as_str));
        self.err(
            E_NO_SUCH_NAME,
            span,
            format!("there is no variable called '{name}' in scope."),
            match suggestion {
                Some(s) => format!("did you mean '{s}'?"),
                None => "check the spelling, and that it is declared before this point.".to_string(),
            },
        );
    }
}

fn widening_hint(got: &Type, want: &Type) -> String {
    if want.sign.is_some() && got.sign.is_none() {
        format!(
            "a {} might be negative, so it cannot satisfy {}. Declare it {} instead.",
            got.base.name(),
            want.render(),
            want.render()
        )
    } else if got.base.is_numeric() && want.base.is_numeric() {
        format!("a {} is wider than {}. Narrower types pass into wider ones, never the reverse.", got.base.name(), want.render())
    } else {
        "check the declared type.".to_string()
    }
}

/// Whether every path through a block hands a value back.
fn block_hands_back(body: &Block) -> bool {
    body.iter().any(|s| match s {
        Stmt::Handback { .. } => true,
        Stmt::If(chain) => {
            chain.arms.iter().any(|a| a.condition.is_none())
                && chain.arms.iter().all(|a| block_hands_back(&a.body))
        }
        _ => false,
    })
}

fn handback_type(body: &Block) -> Option<()> {
    block_hands_back(body).then_some(())
}
