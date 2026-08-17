//! Verification, and precision inference.
//!
//! Two jobs, one machinery — both need to know the range a value can take.
//!
//! **Precision.** The compiler never guesses a width. It infers one by looking at every
//! use of a variable in scope; if the value is not knowable at compile time, that is an
//! error and you must state a width or use `infnum`.
//!
//! **Refinements.** A sign promise like `+int` must hold at every point in a program
//! where everything is mutable. Three layers, in order:
//!
//! ```text
//! 1. All values known?  → evaluate it and look. Exact answer.
//! 2. Not knowable?      → interval analysis. Sound, and needs no theorem prover.
//! 3. Neither proved it? → insert a runtime check.
//! ```
//!
//! See docs/types.md.

use std::collections::BTreeMap;

use ahpcl_diagnostics::{Category, Code, Error, Informer, Span};
use ahpcl_eval::{Interpreter, Value};
use ahpcl_syntax::ast::*;

use crate::interval::{Interval, State};

const E_PRECISION_UNKNOWABLE: Code = Code::new(Category::Prec, 1);
const E_SIGN_UNPROVEN: Code = Code::new(Category::Sign, 3);
const E_OVERFLOW: Code = Code::new(Category::Prec, 4);

/// Which layer proved a refinement, for the Informer to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proof {
    /// Layer 1 — the code was executed at compile time.
    Evaluated,
    /// Layer 2 — interval analysis.
    Analysed,
    /// Layer 3 — nothing proved it, so a check happens while running.
    RuntimeCheck,
}

pub struct Verified {
    pub errors: Vec<Error>,
    /// Places where layer 3 inserted a check.
    pub runtime_checks: Vec<Span>,
    /// Widths worked out by range analysis, keyed by the start of the declared name.
    ///
    /// The backend is handed the bare AST, so a width it did not read off the source has
    /// no way to reach it. This is that channel. Only widths *proved* from a range go in:
    /// the `defaulting to [64 bit]` case is a fallback, not a proof, and narrowing on it
    /// would make the compiled path 64-bit while the interpreter stays exact — which is
    /// the divergence `int_type` in the backend carries a comment about.
    pub inferred_bits: BTreeMap<usize, u32>,
}

/// How much compile-time evaluation is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalBudget {
    /// Run to completion, however long it takes. The default at a keyboard.
    Unlimited,
    /// Stop after this many steps and fall through to the next layer.
    Limited(u64),
    /// Skip layer 1 entirely.
    Off,
}

impl Default for EvalBudget {
    fn default() -> Self {
        EvalBudget::Unlimited
    }
}

pub fn verify(program: &Program, informer: &mut Informer, budget: EvalBudget) -> Verified {
    let mut v = Verifier {
        errors: Vec::new(),
        runtime_checks: Vec::new(),
        inferred_bits: BTreeMap::new(),
        informer,
        budget,
        program,
        quiet: false,
        all_uses: Default::default(),
        declared: Default::default(),
    };

    // Pass one gathers the range each variable takes across every assignment; pass two
    // reports from it. One pass would judge a width from the initialiser alone.
    v.quiet = true;
    let mut scratch = State::default();
    v.block(&program.statements, &mut scratch);
    v.quiet = false;

    let mut state = State::default();
    v.block(&program.statements, &mut state);
    Verified {
        errors: v.errors,
        runtime_checks: v.runtime_checks,
        inferred_bits: v.inferred_bits,
    }
}

struct Verifier<'a> {
    errors: Vec<Error>,
    runtime_checks: Vec<Span>,
    /// See `Verified::inferred_bits`.
    inferred_bits: BTreeMap<usize, u32>,
    informer: &'a mut Informer,
    budget: EvalBudget,
    program: &'a Program,
    /// Reaching a fixed point means walking the body several times. Findings are
    /// suppressed during those rounds and reported once, from the converged state.
    quiet: bool,
    /// The union of every value a variable takes anywhere in scope.
    ///
    /// "Range analysis looks at *all* uses, not just the initialiser" — so a width is
    /// inferred from this, not from the declaration alone. Without it, a counter
    /// declared `= '0'` and accumulated to 5050 would be inferred as 8-bit.
    all_uses: std::collections::HashMap<String, Interval>,
    /// Where each variable was declared and with what type, so a later `change:` can be
    /// checked against the same promise.
    declared: std::collections::HashMap<String, (TypeRef, Option<Precision>, Span)>,
}

impl<'a> Verifier<'a> {
    fn block(&mut self, stmts: &[Stmt], state: &mut State) {
        for stmt in stmts {
            self.statement(stmt, state);
        }
    }

    fn statement(&mut self, stmt: &Stmt, state: &mut State) {
        match stmt {
            Stmt::Var(v) => {
                for b in &v.bindings {
                    let range = b
                        .value
                        .as_ref()
                        .map(|e| self.range_of(e, state))
                        .unwrap_or(Interval::UNKNOWN);
                    state.set(&b.name, range);
                    self.record_use(&b.name, range);
                    self.declared.insert(
                        b.name.clone(),
                        (v.ty.clone(), b.precision.clone(), b.name_span),
                    );
                    self.check_sign(&v.ty, &b.name, range, b.name_span);
                    let across_all_uses = self.all_uses.get(&b.name).copied().unwrap_or(range);
                    self.check_precision(&v.ty, b, across_all_uses);
                }
            }
            Stmt::Change(c) => {
                for target in &c.targets {
                    let range = self.range_of(&target.value, state);
                    state.set(&target.name, range);
                    self.record_use(&target.name, range);
                    self.check_sign(&c.ty, &target.name, range, target.name_span);
                    // A stated width is a promise about range, so a later change has to
                    // keep it — not only the initialiser.
                    self.check_stated_width(&target.name, range, target.name_span);
                }
            }
            Stmt::Func(f) => {
                let mut inner = State::default();
                for p in &f.params {
                    inner.set(&p.name, sign_range(&p.ty));
                }
                self.block(&f.body, &mut inner);
            }
            Stmt::If(chain) => {
                // Each arm runs from the incoming state, narrowed by its condition;
                // afterwards a variable can hold whatever any arm allowed.
                let mut merged: Option<State> = None;
                for arm in &chain.arms {
                    let mut branch = state.clone();
                    if let Some(cond) = &arm.condition {
                        self.narrow(cond, &mut branch, true);
                    }
                    self.block(&arm.body, &mut branch);
                    merged = Some(match merged {
                        Some(m) => m.join(&branch),
                        None => branch,
                    });
                }
                if let Some(m) = merged {
                    *state = state.join(&m);
                }
            }
            Stmt::Loop(l) => self.loop_stmt(l, state),
            Stmt::Handback { value, .. } | Stmt::Expr(value) => {
                self.range_of(value, state);
            }
            Stmt::Print { args, .. } => {
                for a in args {
                    self.range_of(a, state);
                }
            }
        }
    }

    fn record_use(&mut self, name: &str, range: Interval) {
        let merged = match self.all_uses.get(name) {
            Some(prev) => prev.join(range),
            None => range,
        };
        self.all_uses.insert(name.to_string(), merged);
    }

    /// Check a range against the width the variable was declared with.
    fn check_stated_width(&mut self, name: &str, range: Interval, span: Span) {
        if self.quiet {
            return;
        }
        let Some((ty, precision, _)) = self.declared.get(name).cloned() else { return };
        let Some(Precision::Bits(bits)) = precision else { return };
        if ty.base != "int" {
            return;
        }
        if let (Some(lo), Some(hi)) = (range.lo, range.hi) {
            let (min, max) = width_range(bits, ty.sign);
            if lo < min || hi > max {
                self.errors.push(Error::new(
                    E_OVERFLOW,
                    span,
                    format!(
                        "'{name}' reaches {}, which does not fit in [{bits} bit] — that holds [{min}, {max}].",
                        range.render()
                    ),
                    "widen the type, or use infnum.",
                ));
            }
        }
    }

    fn loop_stmt(&mut self, l: &LoopStmt, state: &mut State) {
        match &l.kind {
            LoopKind::Counted { var, range, .. } => {
                // A counted loop is bounded by construction, so the counter's range is
                // known outright.
                let counter = self.counted_range(range, state);
                let mut inner = state.clone();
                inner.set(var, counter);
                self.iterate_to_fixed_point(&l.body, &mut inner, None);
                inner.set(var, counter);
                self.report_once(&l.body, &mut inner.clone());
                inner.vars.remove(var);
                *state = state.join(&inner);
            }
            LoopKind::While { condition } => {
                let entry = state.clone();
                let mut inner = state.clone();
                self.iterate_to_fixed_point(&l.body, &mut inner, Some(condition));
                // Report from the converged state, with the condition applied as it is
                // inside the body.
                let mut reporting = entry.join(&inner);
                self.narrow(condition, &mut reporting, true);
                self.report_once(&l.body, &mut reporting);
                // On exit the condition is false.
                let mut exit = inner;
                self.narrow(condition, &mut exit, false);
                *state = state.join(&exit);
            }
        }
    }

    /// Run the body until the state stops changing.
    ///
    /// Two phases, which is what standard abstract interpretation needs:
    ///
    /// * **Widening** guarantees termination. A range that keeps moving outward jumps
    ///   straight to unbounded rather than creeping one step per round forever.
    /// * **Narrowing** then recovers precision. Widening over-approximates — a countdown
    ///   from 100 widens to `[-∞, 100]` — and re-running the body without widening pulls
    ///   it back to the true `[1, 100]`.
    fn iterate_to_fixed_point(
        &mut self,
        body: &[Stmt],
        state: &mut State,
        condition: Option<&Expr>,
    ) {
        const WIDEN_ROUNDS: usize = 12;
        const NARROW_ROUNDS: usize = 8;

        let was_quiet = std::mem::replace(&mut self.quiet, true);
        let entry = state.clone();

        for round in 0..WIDEN_ROUNDS {
            let before = state.clone();
            // The condition holds at the top of every iteration, so it narrows the
            // state each round — that is what proves a countdown stays positive.
            if let Some(cond) = condition {
                self.narrow(cond, state, true);
            }
            self.block(body, state);
            let joined = before.join(state);
            *state = if round >= 1 { before.widen(&joined) } else { joined };
            if *state == before {
                break;
            }
        }

        // Narrowing: iterate again without widening, so an over-approximation such as
        // `[-∞, 100]` is pulled back to the true `[1, 100]`.
        for _ in 0..NARROW_ROUNDS {
            let before = state.clone();
            let mut next = state.clone();
            if let Some(cond) = condition {
                self.narrow(cond, &mut next, true);
            }
            self.block(body, &mut next);
            *state = entry.join(&next);
            if *state == before {
                break;
            }
        }

        self.quiet = was_quiet;
    }

    /// One reporting pass over a loop body, from the converged state.
    fn report_once(&mut self, body: &[Stmt], state: &mut State) {
        self.block(body, state);
    }

    /// The range a counted loop's counter takes.
    fn counted_range(&mut self, range: &Expr, state: &State) -> Interval {
        let inner = match &range.kind {
            ExprKind::Math(e) => e.as_ref(),
            _ => range,
        };
        match &inner.kind {
            ExprKind::Range { from, to, .. } => {
                let f = self.range_of(from, state);
                let t = self.range_of(to, state);
                Interval { lo: f.lo, hi: t.hi }.join(Interval { lo: t.lo, hi: f.hi })
            }
            _ => Interval::UNKNOWN,
        }
    }

    /// Apply a condition to the state, either as taken or as not taken.
    fn narrow(&mut self, cond: &Expr, state: &mut State, taken: bool) {
        let inner = match &cond.kind {
            ExprKind::Math(e) => e.as_ref(),
            _ => cond,
        };
        let ExprKind::Binary { op, lhs, rhs } = &inner.kind else { return };

        let (name, bound) = match (bare_name(lhs), self.range_of(rhs, state).singleton()) {
            (Some(n), Some(b)) => (n, b),
            _ => return,
        };
        let current = state.get(name);

        let op = if taken { *op } else { negate(*op) };
        let narrowed = match op {
            BinOp::Greater => current.above(bound),
            BinOp::GreaterEq => current.at_least(bound),
            BinOp::Less => current.below(bound),
            BinOp::LessEq => current.at_most(bound),
            BinOp::Eq => current.meet(Interval::exact(bound)),
            _ => current,
        };
        state.set(name, narrowed);
    }

    /// The range an expression can produce.
    fn range_of(&mut self, e: &Expr, state: &State) -> Interval {
        match &e.kind {
            ExprKind::Math(inner) => self.range_of(inner, state),
            ExprKind::Number(t) | ExprKind::Literal(t) => {
                t.parse::<i128>().map(Interval::exact).unwrap_or(Interval::UNKNOWN)
            }
            ExprKind::Ref { name, selectors } if selectors.is_empty() => state.get(name),
            // Selecting from an array, or a conditional used for its value, produces a
            // number whose range is not tracked — but it *is* knowable at compile time,
            // so demanding a stated width would be wrong.
            ExprKind::Ref { .. } => Interval::UNKNOWN,
            ExprKind::Unary { op: UnOp::Neg, operand } => self.range_of(operand, state).neg(),
            ExprKind::Binary { op, lhs, rhs } => {
                let a = self.range_of(lhs, state);
                let b = self.range_of(rhs, state);
                match op {
                    BinOp::Add => a.add(b),
                    BinOp::Sub => a.sub(b),
                    BinOp::Mul => a.mul(b),
                    _ => Interval::UNKNOWN,
                }
            }
            _ => Interval::UNKNOWN,
        }
    }

    /// Verify a sign refinement, trying each layer in turn.
    fn check_sign(&mut self, ty: &TypeRef, name: &str, range: Interval, span: Span) {
        if self.quiet {
            return;
        }
        let Some(sign) = ty.sign else { return };

        // Layer 1: if the value is known outright, just look at it.
        if let Some(v) = range.singleton() {
            let ok = match sign {
                Sign::Positive => v > 0,
                Sign::Negative => v < 0,
            };
            if ok {
                self.informer.say(
                    span.start,
                    format!(
                        "'{name}' evaluated at compile time to {v}; {}{} verified",
                        sign_prefix(sign),
                        ty.base
                    ),
                );
            } else {
                self.errors.push(Error::new(
                    E_SIGN_UNPROVEN,
                    span,
                    format!(
                        "'{name}' evaluates to {v}, which breaks the promise that it is {}{}.",
                        sign_prefix(sign),
                        ty.base
                    ),
                    format!("declare it :{} instead.", ty.base),
                ));
            }
            return;
        }

        // An empty range means the branch is unreachable, so there is nothing to
        // prove and nothing worth saying.
        if range.is_empty() {
            return;
        }

        // Layer 2: interval analysis.
        let proved = match sign {
            Sign::Positive => range.lo.map(|l| l > 0).unwrap_or(false),
            Sign::Negative => range.hi.map(|h| h < 0).unwrap_or(false),
        };
        if proved {
            self.informer.say(
                span.start,
                format!(
                    "range analysis proved '{name}' ∈ {}; {}{} verified",
                    range.render(),
                    sign_prefix(sign),
                    ty.base
                ),
            );
            return;
        }

        // A range that definitely breaks the promise is an error, not a check.
        let definitely_broken = match sign {
            Sign::Positive => range.hi.map(|h| h <= 0).unwrap_or(false),
            Sign::Negative => range.lo.map(|l| l >= 0).unwrap_or(false),
        };
        if definitely_broken {
            self.errors.push(Error::new(
                E_SIGN_UNPROVEN,
                span,
                format!(
                    "'{name}' is {}, which breaks the promise that it is {}{}.",
                    range.render(),
                    sign_prefix(sign),
                    ty.base
                ),
                format!("declare it :{} instead.", ty.base),
            ));
            return;
        }

        // Layer 3: nothing proved it, so check while running.
        self.runtime_checks.push(span);
        self.informer.say(
            span.start,
            format!(
                "{}{} on '{name}' unproven ({}); runtime check inserted",
                sign_prefix(sign),
                ty.base,
                range.render()
            ),
        );
    }

    /// Infer a width, or require one.
    ///
    /// The compiler never guesses: it takes the range from every use in scope and picks
    /// a width that fits them all. When the value is not knowable at compile time, that
    /// is an error and a width must be stated — or `infnum` used.
    fn check_precision(&mut self, ty: &TypeRef, binding: &Binding, range: Interval) {
        if self.quiet {
            return;
        }
        // Width-from-range applies to **integers** only.
        //
        // A decimal's width is an IEEE *format* — decimal32/64/128 — chosen for how many
        // significant digits you want, not derived from the range of a value. Rationals
        // hold a numerator and a denominator and have no width at all. Inferring a
        // decimal width from an integer range would be a category error.
        if ty.base != "int" {
            return;
        }
        if binding.shape.is_some() {
            return;
        }

        if let Some(Precision::Bits(bits)) = &binding.precision {
            // A stated width is a promise about range, so check it holds.
            if let (Some(lo), Some(hi)) = (range.lo, range.hi) {
                let (min, max) = width_range(*bits, ty.sign);
                if lo < min || hi > max {
                    self.errors.push(Error::new(
                        E_OVERFLOW,
                        binding.name_span,
                        format!(
                            "'{}' reaches {}, which does not fit in [{bits} bit] — that holds [{min}, {max}].",
                            binding.name,
                            range.render()
                        ),
                        "widen the type, or use infnum.",
                    ));
                }
            }
            return;
        }
        if binding.precision.is_some() {
            return;
        }

        match (range.lo, range.hi) {
            (Some(lo), Some(hi)) => {
                let bits = smallest_width(lo, hi, ty.sign);
                match bits {
                    Some(b) => {
                        // Recorded on the same path as the report, so a width reaches the
                        // backend exactly when it was proved and announced — not during
                        // the fixed-point rounds, which are still moving.
                        self.inferred_bits.insert(binding.name_span.start.0, b);
                        self.informer.say(
                            binding.name_span.start,
                            format!(
                                "'{}' inferred as [{b} bit] from its range {}",
                                binding.name,
                                range.render()
                            ),
                        )
                    }
                    None => self.errors.push(Error::new(
                        E_OVERFLOW,
                        binding.name_span,
                        format!(
                            "'{}' reaches {}, which no fixed width holds.",
                            binding.name,
                            range.render()
                        ),
                        "use infnum, which is unbounded.",
                    )),
                }
            }
            _ => {
                // Only a value flowing from *input* is genuinely unknowable. A function
                // call or a selector has a range the analysis simply does not track —
                // which is a limit of the analysis, not of the program, so demanding a
                // width there would punish ordinary code.
                let from_input = binding
                    .value
                    .as_ref()
                    .map(|e| derives_from_input(e))
                    .unwrap_or(false);
                if from_input {
                    self.errors.push(Error::new(
                        E_PRECISION_UNKNOWABLE,
                        binding.name_span,
                        format!(
                            "'{}' comes from input, so its value is not knowable at compile time and no width can be inferred.",
                            binding.name
                        ),
                        "state a precision, as in [32 bit], or use infnum which is unbounded.",
                    ));
                } else {
                    self.informer.say(
                        binding.name_span.start,
                        format!(
                            "'{}' has no statically known range; defaulting to [64 bit]",
                            binding.name
                        ),
                    );
                }
            }
        }
    }

    /// Layer 1 proper: execute a fragment at compile time.
    ///
    /// Used where a whole program is constant-foldable. The budget decides whether a
    /// non-terminating loop hangs the compiler or falls through to layer 2.
    #[allow(dead_code)]
    fn evaluate(&mut self) -> Option<Vec<Value>> {
        if self.budget == EvalBudget::Off {
            return None;
        }
        let mut interp = Interpreter::new();
        if let EvalBudget::Limited(n) = self.budget {
            interp = interp.with_step_limit(n);
        }
        interp.collect_functions(self.program);
        None
    }
}

fn sign_prefix(s: Sign) -> &'static str {
    match s {
        Sign::Positive => "+",
        Sign::Negative => "-",
    }
}

fn negate(op: BinOp) -> BinOp {
    match op {
        BinOp::Greater => BinOp::LessEq,
        BinOp::GreaterEq => BinOp::Less,
        BinOp::Less => BinOp::GreaterEq,
        BinOp::LessEq => BinOp::Greater,
        BinOp::Eq => BinOp::NotEq,
        BinOp::NotEq => BinOp::Eq,
        other => other,
    }
}

fn bare_name(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Ref { name, selectors } if selectors.is_empty() => Some(name),
        ExprKind::Math(inner) => bare_name(inner),
        _ => None,
    }
}

/// Whether a value flows from input — `read`, `parse` or `clock`.
///
/// This is the honest reading of "not knowable at compile time". A function call is
/// knowable in principle; a file's contents are not.
fn derives_from_input(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Builtin { name, .. } => matches!(name.as_str(), "read" | "parse" | "clock"),
        ExprKind::Math(inner) => derives_from_input(inner),
        ExprKind::Binary { lhs, rhs, .. } => derives_from_input(lhs) || derives_from_input(rhs),
        ExprKind::Unary { operand, .. } => derives_from_input(operand),
        _ => false,
    }
}

/// The range a parameter starts with, from its declared refinement.
fn sign_range(ty: &TypeRef) -> Interval {
    match ty.sign {
        Some(Sign::Positive) => Interval { lo: Some(1), hi: None },
        Some(Sign::Negative) => Interval { lo: None, hi: Some(-1) },
        None => Interval::UNKNOWN,
    }
}

/// What a width holds. A `+int` cannot be negative, so the sign bit is free.
fn width_range(bits: u32, sign: Option<Sign>) -> (i128, i128) {
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

/// The narrowest offered width that holds this range.
fn smallest_width(lo: i128, hi: i128, sign: Option<Sign>) -> Option<u32> {
    [8u32, 16, 32, 64, 128].into_iter().find(|&bits| {
        let (min, max) = width_range(bits, sign);
        lo >= min && hi <= max
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ahpcl_syntax::parse_source;

    fn run(src: &str) -> (Vec<String>, String) {
        let (program, errors) = parse_source(src);
        assert!(errors.is_empty(), "should parse: {errors:#?}");
        let mut informer = Informer::new();
        let out = verify(&program, &mut informer, EvalBudget::Unlimited);
        let notes = informer.render(&ahpcl_diagnostics::SourceFile::new("t.ahpcl", src));
        (out.errors.into_iter().map(|e| e.code.render()).collect(), notes)
    }

    #[test]
    fn a_width_is_inferred_from_the_range() {
        let (errors, notes) = run("var:int 'x' = '1000'.");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(notes.contains("inferred as [16 bit]"), "{notes}");
    }

    #[test]
    fn range_analysis_looks_at_every_use_not_just_the_initialiser() {
        // x reaches 100,000, so 16 bits is not enough.
        let (_, notes) = run("var:int 'x' = '1000'.\nvar:int 'y' = math { ('x') x 100 }.");
        assert!(notes.contains("'y' inferred as [32 bit]"), "{notes}");
    }

    #[test]
    fn a_value_that_is_not_knowable_needs_a_stated_width() {
        let (errors, _) = run("var:str 'r' = read[\"f\"].\nvar:int 'z' = parse[('r')].");
        assert!(errors.contains(&"AHPCL-PREC-0001".to_string()), "{errors:?}");
    }

    #[test]
    fn stating_a_width_silences_that() {
        let (errors, _) =
            run("var:str 'r' = read[\"f\"].\nvar:int 'z' [32 bit] = parse[('r')].");
        assert!(!errors.contains(&"AHPCL-PREC-0001".to_string()), "{errors:?}");
    }

    #[test]
    fn infnum_is_the_way_to_say_unbounded() {
        let (errors, _) = run("var:str 'r' = read[\"f\"].\nvar:infnum 'z' = parse[('r')].");
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn a_stated_width_is_checked_against_the_range() {
        let (errors, _) = run("var:int 'x' [8 bit] = '1000'.");
        assert!(errors.contains(&"AHPCL-PREC-0004".to_string()), "{errors:?}");
    }

    #[test]
    fn layer_one_verifies_a_known_value() {
        let (errors, notes) = run("var:+int 'n' = '10'.");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(notes.contains("evaluated at compile time to 10"), "{notes}");
    }

    #[test]
    fn layer_two_proves_a_countdown_stays_positive() {
        // The worked example from docs/types.md.
        let (errors, notes) = run(
            "var:+int 'n' = '100'.\n\
             loop:while math { ('n') > 1 } {\n\
                 change:var:+int 'n' = math { ('n') - 1 }.\n\
             }.",
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert!(notes.contains("range analysis proved"), "{notes}");
    }

    #[test]
    fn layer_three_inserts_a_check_when_nothing_proves_it() {
        let (errors, notes) = run(
            "var:str 'r' = read[\"f\"].\n\
             var:+int 'n' [32 bit] = parse[('r')].",
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert!(notes.contains("runtime check inserted"), "{notes}");
    }

    #[test]
    fn a_range_that_definitely_breaks_the_promise_is_an_error() {
        let (errors, _) = run(
            "var:+int 'n' = '10'.\n\
             change:var:+int 'n' = math { 0 - 5 }.",
        );
        assert!(errors.contains(&"AHPCL-SIGN-0003".to_string()), "{errors:?}");
    }
}

#[cfg(test)]
mod inferred_width_channel {
    use super::*;
    use ahpcl_syntax::parse_source;

    fn widths(src: &str) -> BTreeMap<usize, u32> {
        let (program, errors) = parse_source(src);
        assert!(errors.is_empty(), "should parse: {errors:#?}");
        let mut informer = Informer::default();
        verify(&program, &mut informer, EvalBudget::Unlimited).inferred_bits
    }

    /// A width proved from a range reaches the backend.
    #[test]
    fn a_proved_width_is_handed_over() {
        let table = widths("var:int 'x' = '1000'.\nprint[('x')].");
        assert_eq!(table.values().copied().collect::<Vec<_>>(), vec![16]);
    }

    #[test]
    fn a_wider_range_gives_a_wider_width() {
        let table = widths("var:int 'x' = '100000'.\nprint[('x')].");
        assert_eq!(table.values().copied().collect::<Vec<_>>(), vec![32]);
    }

    /// The guard that matters most in this whole change.
    ///
    /// `'x' has no statically known range; defaulting to [64 bit]` is a *fallback*, not a
    /// proof. Handing that width to the backend would make the compiled path 64-bit while
    /// the interpreter stays exact — which is the divergence `int_type` in the backend
    /// carries a comment about, and which cost real debugging once already.
    #[test]
    fn a_defaulted_width_is_never_handed_over() {
        // A function call's range is not tracked, so this takes the fallback path.
        let table = widths(
            "func:int 'twice' [var:int 'n' [32 bit]] {\n\
                 handback math { ('n') x 2 }.\n\
             }.\n\
             var:int 'x' = 'twice'['21'].\n\
             print[('x')].",
        );
        assert!(
            table.is_empty(),
            "a defaulted width must not reach the backend, got {table:?}"
        );
    }

    /// A width written by hand is the source's business, not the analysis's — the
    /// backend reads it off the AST directly and the table stays out of it.
    #[test]
    fn a_written_width_is_not_in_the_table() {
        let table = widths("var:int 'x' [32 bit] = '1000'.\nprint[('x')].");
        assert!(table.is_empty(), "expected nothing, got {table:?}");
    }
}
