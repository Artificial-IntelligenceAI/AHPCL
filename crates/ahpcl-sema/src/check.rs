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
const E_OVERFLOW: Code = Code::new(Category::Prec, 4);
const E_BAD_WIDTH: Code = Code::new(Category::Prec, 5);
const E_SIGN_VIOLATION: Code = Code::new(Category::Sign, 1);
const E_NO_VALUE: Code = Code::new(Category::Type, 5);
const E_UNREACHABLE: Code = Code::new(Category::Syn, 2);

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

    /// `handback` hands its value to whatever collects it and ends the unit that
    /// produced it — the whole call in a function, one iteration in a loop. So anything
    /// written after it in the same block can never run.
    ///
    /// Catching that here is what keeps the loop form honest: `loop:while … { handback
    /// n. change:var:int 'n' = … }` would otherwise never advance and never finish.
    fn check_unreachable(&mut self, body: &Block) {
        let Some(at) = body.iter().position(|s| matches!(s, Stmt::Handback { .. })) else {
            return;
        };
        let Some(next) = body.get(at + 1) else { return };
        let span = stmt_span(next);
        self.err(
            E_UNREACHABLE,
            span,
            "this can never run, because the handback above it ends the block.",
            "move it above the handback, or take it out.",
        );
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
                    // A shape disagreement is reported by the shape check below, so
                    // do not also report it as a type mismatch.
                    let shape_only = got.base.fits_in(ty.base)
                        && got.shape.is_some()
                        && ty.shape.is_some()
                        && !got.fits_in(&ty);
                    if !shape_only {
                        self.require_fits_deferring_sign(&got, &ty, value.span, "assigned");
                    }
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
            } else {
                // A declaration must say what the variable holds. Nothing can be read
                // before it is written, and a silent 0 would be exactly the kind of
                // default AHPCL does not do.
                self.err(
                    E_NO_VALUE,
                    binding.name_span,
                    format!("'{}' is declared but never given a value.", binding.name),
                    format!(
                        "give it one, as in {}:{} '{}' = <value>.",
                        "var",
                        v.ty.base,
                        binding.name
                    ),
                );
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

        for target in &c.targets {
            let Some(existing) = self.scopes.lookup(&target.name).cloned() else {
                self.unknown_name(&target.name, target.name_span);
                continue;
            };

            if existing.read_only {
                self.err(
                    E_READ_ONLY,
                    target.name_span,
                    format!("'{}' is a loop counter, which is read-only inside the body.", target.name),
                    "a counted loop runs a fixed number of times; use a separate variable.",
                );
                continue;
            }

            // The restated type is documentation for the reader, so it is verified.
            // Documentation that can drift out of sync is worse than none.
            let want = if target.selectors.is_empty() {
                existing.ty.clone()
            } else {
                existing.ty.element()
            };

            if stated.base != want.base || stated.sign != want.sign {
                self.errors.push(
                    Error::new(
                        E_TYPE_RESTATED,
                        c.ty.span,
                        format!(
                            "'{}' is {}, but this says {}.",
                            target.name,
                            want.render(),
                            stated.render()
                        ),
                        format!("write change:var:{} '{}' = … .", want.render(), target.name),
                    )
                    .with_label(existing.declared_at, "declared here")
                    .with_label(c.ty.span, "restated differently here"),
                );
            }

            if let Some(got) = self.expr(&target.value, Some(&want)) {
                self.require_fits_deferring_sign(&got, &want, target.value.span, "assigned");
            }
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
        self.check_unreachable(&f.body);
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
            self.check_unreachable(&arm.body);
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
            self.err(
                E_MISMATCH,
                chain.span,
                if has_else {
                    "this conditional is used for its value, but not every branch hands one back."
                } else {
                    "this conditional is used for its value, but it has no else, so one path produces nothing."
                },
                "add an else, and a handback in every branch.",
            );
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

        self.check_unreachable(&l.body);
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
            ExprKind::Math(inner) => {
                let t = self.expr(inner, expected)?;
                // A bare array reference sums, including when it is the whole
                // expression — `math { ('areas') }` is the sum, not the array.
                if is_bare_ref(inner) && t.is_array() {
                    let scalar_wanted = expected.map(|e| !e.is_array()).unwrap_or(true);
                    if scalar_wanted {
                        return Some(t.element());
                    }
                }
                Some(t)
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e.span, expected),
            ExprKind::Unary { op, operand } => self.unary(*op, operand, e.span, expected),
            ExprKind::Call { name, args } => self.call(name, args, e.span),
            ExprKind::Builtin { name, args } => self.builtin(name, args, e.span, expected),
            ExprKind::ArrayLit(items) => self.array_literal(items, e.span, expected),
            ExprKind::If(chain) => self.if_chain(chain, expected, None),
            ExprKind::Loop(l) => {
                let elem = expected.map(|t| t.element());
                self.loop_stmt(l, elem.as_ref());
                // Each handback contributes one element, so the length comes from the
                // range — "the shape falls out for free".
                let outer = match &l.kind {
                    LoopKind::Counted { range, .. } => loop_length(range),
                    LoopKind::While { .. } => None,
                };
                let base = elem.as_ref().map(|t| t.base).unwrap_or(Base::Num);
                let sign = elem.as_ref().and_then(|t| t.sign);
                let mut dims = vec![outer.map(Dim::Known).unwrap_or(Dim::Unknown)];
                // A nested loop gains a dimension.
                if let Some(inner) = expected.and_then(|t| t.shape.as_ref()) {
                    if inner.rank() > 1 {
                        dims.extend(inner.0[1..].iter().cloned());
                    }
                }
                Some(Type { base, sign, shape: Some(Shape(dims)), precision: None })
            }
            ExprKind::Option { .. } => None,
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
                self.check_literal_value(text, &elem, span);
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

        self.apply_selectors(var.ty.clone(), selectors, span)
    }

    /// Apply a selector chain. Consecutive index selectors address **successive
    /// dimensions**; `:length;` and `:shape;` are questions about the array and restart
    /// the chain on their result.
    fn apply_selectors(&mut self, ty: Type, selectors: &[Selector], span: Span) -> Option<Type> {
        let mut ty = ty;
        let mut run: Vec<&Selector> = Vec::new();
        for sel in selectors {
            match sel {
                Selector::Length => {
                    let _ = self.apply_dimension_run(ty, &run, span)?;
                    run.clear();
                    ty = Type::scalar(Base::Int);
                }
                Selector::Shape => {
                    ty = self.apply_dimension_run(ty, &run, span)?;
                    run.clear();
                    let rank = ty.shape.as_ref().map(|s| s.rank()).unwrap_or(0);
                    ty = Type {
                        base: Base::Int,
                        sign: None,
                        shape: Some(Shape(vec![Dim::Known(rank as u64)])),
                        precision: None,
                    };
                }
                _ => run.push(sel),
            }
        }
        self.apply_dimension_run(ty, &run, span)
    }

    fn apply_dimension_run(&mut self, ty: Type, run: &[&Selector], span: Span) -> Option<Type> {
        if run.is_empty() {
            return Some(ty);
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
        if run.len() > shape.rank() {
            self.err(
                E_SHAPE_MISMATCH,
                span,
                format!(
                    "{} selectors were given, but {} has {} dimension{}.",
                    run.len(),
                    ty.render(),
                    shape.rank(),
                    if shape.rank() == 1 { "" } else { "s" }
                ),
                "one selector addresses one dimension.",
            );
            return None;
        }

        let mut dims: Vec<Dim> = Vec::new();
        for (i, sel) in run.iter().enumerate() {
            match sel {
                Selector::All => dims.push(shape.0[i].clone()),
                Selector::Indices(items) => {
                    for e in items.iter() {
                        self.expr(e, Some(&Type::scalar(Base::Int)));
                    }
                    // One index collapses the dimension; several keep it, resized.
                    if items.len() > 1 {
                        dims.push(Dim::Known(items.len() as u64));
                    }
                }
                Selector::Range { from, to, by } => {
                    let want = Type::scalar(Base::Int);
                    self.expr(from, Some(&want));
                    self.expr(to, Some(&want));
                    if let Some(b) = by {
                        self.expr(b, Some(&want));
                    }
                    // The length is knowable whenever the bounds are literals.
                    dims.push(match range_length(from, to, by.as_deref()) {
                        Some(n) => Dim::Known(n),
                        None => Dim::Unknown,
                    });
                }
                Selector::Length | Selector::Shape => unreachable!("handled above"),
            }
        }
        // Dimensions with no selector are kept whole.
        for d in shape.0.iter().skip(run.len()) {
            dims.push(d.clone());
        }

        Some(Type {
            base: ty.base,
            sign: ty.sign,
            shape: if dims.is_empty() { None } else { Some(Shape(dims)) },
            precision: ty.precision.clone(),
        })
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

        // A refinement constrains the *result*, not each operand. `math { 0 - 5 }`
        // assigned to a +int is broken by its answer, not by the `0` — and deciding
        // that is verification's job, not the type checker's.
        let unrefined = expected.map(|t| Type { sign: None, ..t.clone() });
        let operand_expectation = if comparison { None } else { unrefined.as_ref() };
        let a = self.expr(lhs, operand_expectation);
        // A comparison bound is an ordinary number, not a member of the refined type.
        // Pinning `0` to `+int` in `math { ('n') > 0 }` would reject the very idiom
        // used to keep a +int positive.
        let b_expectation = if comparison {
            a.as_ref().map(|t| Type { sign: None, precision: None, ..t.clone() })
        } else {
            operand_expectation.cloned().or_else(|| a.clone())
        };
        let b = self.expr(rhs, b_expectation.as_ref());

        // Rule A: a *bare* array reference sums, so it is scalar here. One carrying a
        // selector stays an array, and the operation runs elementwise.
        // The array operators are the exception: `· × ⊙ ⊗` imply `:all;`, because they
        // have no scalar meaning at all.
        let implies_all = matches!(op, Dot | Cross | Hadamard | Tensor);
        let keeps_shape = |bare: bool, t: &Type| {
            (implies_all || !bare) && t.is_array()
        };
        let a_array = a
            .as_ref()
            .and_then(|t| keeps_shape(is_bare_ref(lhs), t).then(|| t.shape.clone().expect("an array")));
        let b_array = b
            .as_ref()
            .and_then(|t| keeps_shape(is_bare_ref(rhs), t).then(|| t.shape.clone().expect("an array")));

        if comparison {
            // An elementwise comparison produces an array of bools.
            let shape = a_array.clone().or(b_array.clone());
            return Some(match shape {
                Some(s) => Type { base: Base::Bool, sign: None, shape: Some(s), precision: None },
                None => Type::scalar(Base::Bool),
            });
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
                    Some(t) if t.base == Base::Int => {
                        self.err(
                            E_MISMATCH,
                            span,
                            "division does not produce a whole number, so it cannot land in an int.",
                            "use // for truncating division, or declare the result :deci or :rat.",
                        );
                        return None;
                    }
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
            // `//` truncates to a whole number; `mod` keeps the operand's kind, so
            // 2.5 mod 2 is 0.5 rather than an int holding a fraction.
            IntDiv => Base::Int,
            Mod => a_elem.base.join(b_elem.base)?,
            _ => a_elem.base.join(b_elem.base)?,
        };

        let sign = match op {
            Add => sign_add(a_elem.sign, b_elem.sign),
            Sub => sign_sub(a_elem.sign, b_elem.sign),
            Mul | Div | IntDiv => sign_mul(a_elem.sign, b_elem.sign),
            Pow => sign_pow(a_elem.sign),
            _ => None,
        };

        // An elementwise operation keeps its shape; a scalar on the other side
        // broadcasts across it.
        if let (Some(x), Some(y)) = (&a_array, &b_array) {
            if !x.agrees_with(y) {
                self.err(
                    E_SHAPE_MISMATCH,
                    span,
                    format!(
                        "elementwise operations need matching shapes, but these are {} and {}.",
                        x.render(),
                        y.render()
                    ),
                    "make the shapes agree.",
                );
                return None;
            }
        }
        let shape = a_array.or(b_array);

        Some(Type { base, sign, shape, precision: None })
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
                // The dot product *is* matrix multiplication. A rank-1 operand acts as
                // a row on the left and a column on the right, so the inner dimensions
                // to compare differ by side.
                let inner_a = sa.0.last().cloned();
                let inner_b = if sb.rank() == 1 { sb.0.first().cloned() } else { sb.0.first().cloned() };
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
                let base = a.base.join(b.base)?;
                let sign = sign_mul(a.sign, b.sign);
                // vec·vec → a single number; mat·vec → vec; vec·mat → vec; mat·mat → mat.
                let dims: Vec<Dim> = match (sa.rank(), sb.rank()) {
                    (1, 1) => vec![],
                    (_, 1) => vec![sa.0[0].clone()],
                    (1, _) => vec![sb.0[sb.rank() - 1].clone()],
                    _ => vec![sa.0[0].clone(), sb.0[sb.rank() - 1].clone()],
                };
                Some(Type {
                    base,
                    sign,
                    shape: if dims.is_empty() { None } else { Some(Shape(dims)) },
                    precision: None,
                })
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
                // The operand still needs pinning, or a bare number in it is reported
                // as ambiguous.
                let want = Type::scalar(Base::Deci);
                let t = self.expr(operand, Some(&want))?;
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

    /// A sign-only mismatch on an assignment is **not** a type error.
    ///
    /// The sign algebra is deliberately conservative — `+int - +int` widens to `int`,
    /// because `7 - 7` is 0. Rejecting here would make the refinement useless with
    /// mutation, and would pre-empt the very thing verification exists to decide:
    /// layer 1 evaluates it, layer 2 analyses the range, layer 3 inserts a check.
    ///
    /// Call arguments and handbacks stay strict, because verification does not reason
    /// across function boundaries.
    fn require_fits_deferring_sign(&mut self, got: &Type, want: &Type, span: Span, what: &str) {
        let sign_only = want.sign.is_some()
            && got.base.fits_in(want.base)
            && shapes_agree(got, want)
            && !got.fits_in(want);
        if sign_only {
            return;
        }
        self.require_fits(got, want, span, what);
    }

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

    /// A refinement is a promise, so a literal that breaks it is caught here rather
    /// than surviving to runtime.
    fn check_literal_value(&mut self, text: &str, ty: &Type, span: Span) {
        let negative = text.starts_with('-');
        let is_zero = text
            .trim_start_matches(['-', '+'])
            .chars()
            .all(|c| c == '0' || c == '.');

        match ty.sign {
            Some(Sign::Positive) if negative || is_zero => self.err(
                E_SIGN_VIOLATION,
                span,
                format!("'{text}' is not strictly positive, but the type says +{}.", ty.base.name()),
                "zero lives only in the unprefixed types; use a plain type, or a positive value.",
            ),
            Some(Sign::Negative) if !negative || is_zero => self.err(
                E_SIGN_VIOLATION,
                span,
                format!("'{text}' is not strictly negative, but the type says -{}.", ty.base.name()),
                "zero lives only in the unprefixed types; use a plain type, or a negative value.",
            ),
            _ => {}
        }

        // A stated width is a promise about range, so check the literal fits.
        if let (Base::Int, Some(Precision::Bits(bits))) = (ty.base, &ty.precision) {
            if let Ok(value) = text.parse::<i128>() {
                let (low, high) = int_range(*bits, ty.sign);
                if value < low || value > high {
                    self.err(
                        E_OVERFLOW,
                        span,
                        format!("{value} does not fit in {} [{bits} bit], which holds {low} to {high}.", ty.render()),
                        "widen the type, or use infnum.",
                    );
                }
            }
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
            (_, Precision::Bits(b)) if !matches!(b, 8 | 16 | 32 | 64 | 128) => self.err(
                E_BAD_WIDTH,
                span,
                format!("{b} is not a width AHPCL offers."),
                "the widths are 8, 16, 32, 64 and 128.",
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

/// The range a width holds. A `+int` cannot be negative, so the sign bit is free —
/// which is where unsigned ranges come from without a separate `uint` family.
fn int_range(bits: u32, sign: Option<Sign>) -> (i128, i128) {
    // Guard the shifts: `1i128 << 127` overflows, so the widest cases are named
    // outright rather than computed.
    let unsigned_max = |w: u32| -> i128 {
        if w >= 127 { i128::MAX } else { (1i128 << w) - 1 }
    };
    let signed_max = |w: u32| -> i128 {
        if w >= 128 { i128::MAX } else { (1i128 << (w - 1)) - 1 }
    };
    let signed_min = |w: u32| -> i128 {
        if w >= 128 { i128::MIN } else { -(1i128 << (w - 1)) }
    };
    match sign {
        Some(Sign::Positive) => (1, unsigned_max(bits)),
        Some(Sign::Negative) => (-unsigned_max(bits), -1),
        None => (signed_min(bits), signed_max(bits)),
    }
}

/// How many iterations a counted loop runs, when its range is literal.
fn loop_length(range: &Expr) -> Option<u64> {
    let inner = match &range.kind {
        ExprKind::Math(e) => e.as_ref(),
        _ => range,
    };
    match &inner.kind {
        ExprKind::Range { from, to, by } => range_length(from, to, by.as_deref()),
        _ => None,
    }
}

/// How many values a literal range covers, when its bounds are known.
fn range_length(from: &Expr, to: &Expr, by: Option<&Expr>) -> Option<u64> {
    let literal = |e: &Expr| -> Option<i64> {
        match &e.kind {
            ExprKind::Number(t) | ExprKind::Literal(t) => t.parse().ok(),
            ExprKind::Math(inner) => match &inner.kind {
                ExprKind::Number(t) | ExprKind::Literal(t) => t.parse().ok(),
                _ => None,
            },
            _ => None,
        }
    };
    let f = literal(from)?;
    let t = literal(to)?;
    let step = match by {
        Some(b) => literal(b)?,
        None => 1,
    };
    if step == 0 {
        return None;
    }
    let span = t - f;
    if (step > 0 && span < 0) || (step < 0 && span > 0) {
        return Some(0);
    }
    Some((span / step + 1) as u64)
}

fn shapes_agree(a: &Type, b: &Type) -> bool {
    match (&a.shape, &b.shape) {
        (None, None) => true,
        (Some(x), Some(y)) => x.agrees_with(y),
        _ => false,
    }
}

/// Whether an expression is a bare array reference — one with no selector.
fn is_bare_ref(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Ref { selectors, .. } => selectors.is_empty(),
        ExprKind::Math(inner) => is_bare_ref(inner),
        _ => false,
    }
}

fn widening_hint(got: &Type, want: &Type) -> String {
    // A shape disagreement is not about width, so say so rather than talking about
    // narrower and wider types.
    match (&got.shape, &want.shape) {
        (Some(a), Some(b)) if !a.agrees_with(b) => {
            return format!("the shapes {} and {} do not agree.", a.render(), b.render());
        }
        (Some(a), None) => {
            return format!(
                "this is an array of {}, but a single value was expected. A bare reference sums; use :all; to keep it an array.",
                a.render()
            );
        }
        (None, Some(b)) => {
            return format!("a single value cannot fill an array of {}.", b.render());
        }
        _ => {}
    }

    if want.sign.is_some() && got.sign.is_none() {
        format!(
            "{} might be negative, so it cannot satisfy {}. Declare it {} instead.",
            article(got.base.name()),
            want.render(),
            want.render()
        )
    } else if got.base.is_numeric() && want.base.is_numeric() {
        format!(
            "{} is wider than {}. Narrower types pass into wider ones, never the reverse.",
            article(got.base.name()),
            want.render()
        )
    } else {
        "check the declared type.".to_string()
    }
}

/// "an int", "a deci".
fn article(word: &str) -> String {
    let vowel = word.starts_with(['a', 'e', 'i', 'o', 'u']);
    format!("{} {word}", if vowel { "an" } else { "a" })
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

/// Where a statement sits in the source, for pointing at it.
fn stmt_span(s: &Stmt) -> Span {
    match s {
        Stmt::Var(v) => v.span,
        Stmt::Change(c) => c.span,
        Stmt::Func(f) => f.span,
        Stmt::If(c) => c.span,
        Stmt::Loop(l) => l.span,
        Stmt::Print { span, .. } => *span,
        Stmt::Handback { span, .. } => *span,
        Stmt::Expr(e) => e.span,
    }
}
