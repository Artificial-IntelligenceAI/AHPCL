//! The evaluator.
//!
//! Does three jobs, which is why it was worth building before code generation:
//!   * runs programs, so AHPCL works today
//!   * layer 1 of verification — executing a loop at compile time to check a refinement
//!   * constant folding
//!
//! Runtime failures stop the program, reported in the Error Handler's style. That was
//! chosen as a deliberate starting point; failures-as-values remain addable later.

use std::collections::HashMap;

use ahpcl_diagnostics::{Category, Code, Error, Span};
use ahpcl_syntax::ast::*;

use crate::value::{Array, Decimal, Rational, Value};

const E_RUNTIME: Code = Code::new(Category::Run, 1);
const E_OVERFLOW: Code = Code::new(Category::Prec, 4);
const E_DIV_ZERO: Code = Code::new(Category::Run, 2);
const E_BOUNDS: Code = Code::new(Category::Run, 3);
const E_PARSE: Code = Code::new(Category::Run, 4);

/// How a block finished.
enum Flow {
    Normal,
    /// A `handback` produced a value.
    Handback(Value),
}

pub struct Output {
    pub lines: Vec<String>,
    pub error: Option<Error>,
}

pub struct Interpreter<'a> {
    scopes: Vec<HashMap<String, Value>>,
    functions: HashMap<String, &'a FuncDecl>,
    pub lines: Vec<String>,
    /// A guard against a runaway loop taking the whole machine with it. Only used
    /// when evaluating at compile time.
    steps: u64,
    step_limit: Option<u64>,
}

/// Run a whole program.
pub fn run(program: &Program) -> Output {
    let mut interp = Interpreter::new();
    interp.collect_functions(program);
    let result = interp.block(&program.statements);
    let error = match result {
        Ok(_) => None,
        Err(e) => Some(e),
    };
    Output { lines: interp.lines, error }
}

impl<'a> Interpreter<'a> {
    pub fn new() -> Self {
        Interpreter {
            scopes: vec![HashMap::new()],
            functions: HashMap::new(),
            lines: Vec::new(),
            steps: 0,
            step_limit: None,
        }
    }

    /// Cap the work done, for compile-time evaluation where a non-terminating loop
    /// would otherwise hang the compiler.
    pub fn with_step_limit(mut self, limit: u64) -> Self {
        self.step_limit = Some(limit);
        self
    }

    pub fn collect_functions(&mut self, program: &'a Program) {
        for stmt in &program.statements {
            if let Stmt::Func(f) = stmt {
                self.functions.insert(f.name.clone(), f);
            }
        }
    }

    fn err(&self, code: Code, span: Span, rule: impl Into<String>, fix: impl Into<String>) -> Error {
        Error::new(code, span, rule, fix)
    }

    fn sum_overflowed(&self, span: Span) -> Error {
        self.err(
            E_OVERFLOW,
            span,
            "summing this array overflowed the value's precision.",
            "widen the element type, or use infnum.",
        )
    }

    fn not_a_number(&self, v: &Value, span: Span) -> Error {
        self.err(
            E_RUNTIME,
            span,
            format!("arithmetic needs a number, but this is {}.", v.type_name()),
            "check the operand's type.",
        )
    }

    fn tick(&mut self, span: Span) -> Result<(), Error> {
        self.steps += 1;
        if let Some(limit) = self.step_limit {
            if self.steps > limit {
                return Err(self.err(
                    E_RUNTIME,
                    span,
                    "compile-time evaluation exceeded its step limit.",
                    "this loop may not terminate; run with flag:loop-evaluation = limit.",
                ));
            }
        }
        Ok(())
    }

    fn lookup(&self, name: &str) -> Option<&Value> {
        self.scopes.iter().rev().find_map(|f| f.get(name))
    }

    fn define(&mut self, name: &str, value: Value) {
        self.scopes.last_mut().expect("a scope").insert(name.to_string(), value);
    }

    fn assign(&mut self, name: &str, value: Value) {
        for frame in self.scopes.iter_mut().rev() {
            if frame.contains_key(name) {
                frame.insert(name.to_string(), value);
                return;
            }
        }
        self.define(name, value);
    }

    // ── statements ──────────────────────────────────────────────────────────

    fn block(&mut self, stmts: &'a [Stmt]) -> Result<Flow, Error> {
        for stmt in stmts {
            match self.statement(stmt)? {
                Flow::Normal => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    fn scoped_block(&mut self, stmts: &'a [Stmt]) -> Result<Flow, Error> {
        self.scopes.push(HashMap::new());
        let out = self.block(stmts);
        self.scopes.pop();
        out
    }

    fn statement(&mut self, stmt: &'a Stmt) -> Result<Flow, Error> {
        match stmt {
            Stmt::Var(v) => {
                for b in &v.bindings {
                    if let Some(value) = &b.value {
                        let hint = numeric_hint(&v.ty);
                        let val = self.expr(value, hint)?;
                        // A bare array reference sums its elements — that holds when
                        // assigning to a scalar, not only inside arithmetic.
                        let val = if v.ty.rank.is_none() {
                            let span = value.span;
                            try_reduce_array(val).ok_or_else(|| self.sum_overflowed(span))?
                        } else {
                            val
                        };
                        self.define(&b.name, val);
                    } else {
                        self.define(&b.name, Value::Nothing);
                    }
                }
                Ok(Flow::Normal)
            }
            Stmt::Change(c) => {
                let hint = numeric_hint(&c.ty);
                let val = self.expr(&c.value, hint)?;
                if c.selectors.is_empty() {
                    self.assign(&c.name, val);
                } else {
                    self.assign_element(c, val)?;
                }
                Ok(Flow::Normal)
            }
            Stmt::Func(_) => Ok(Flow::Normal),
            Stmt::If(chain) => self.if_chain(chain),
            Stmt::Loop(l) => {
                self.loop_stmt(l)?;
                Ok(Flow::Normal)
            }
            Stmt::Print { args, .. } => {
                let mut line = String::new();
                for a in args {
                    let v = self.expr(a, None)?;
                    line.push_str(&v.to_string());
                }
                self.lines.push(line);
                Ok(Flow::Normal)
            }
            Stmt::Handback { value, .. } => {
                let v = self.expr(value, None)?;
                Ok(Flow::Handback(v))
            }
            Stmt::Expr(e) => {
                self.expr(e, None)?;
                Ok(Flow::Normal)
            }
        }
    }

    fn assign_element(&mut self, c: &'a ChangeStmt, val: Value) -> Result<(), Error> {
        let mut indices = Vec::new();
        for sel in &c.selectors {
            if let Selector::Indices(items) = sel {
                for item in items {
                    let v = self.expr(item, Some(Numeric::Int))?;
                    indices.push(as_index(&v, item.span)?);
                }
            }
        }
        let Some(Value::Array(arr)) = self.lookup(&c.name).cloned() else {
            return Err(self.err(
                E_RUNTIME,
                c.name_span,
                format!("'{}' is not an array, so it has no elements to change.", c.name),
                "selectors apply to vectors, matrices and tensors.",
            ));
        };

        let flat = flat_index(&arr.shape, &indices).ok_or_else(|| {
            self.err(
                E_BOUNDS,
                c.name_span,
                format!(
                    "index out of range for '{}', which is {:?}.",
                    c.name, arr.shape
                ),
                "indices start at 1.",
            )
        })?;

        let mut arr = arr;
        arr.items[flat] = val;
        self.assign(&c.name, Value::Array(arr));
        Ok(())
    }

    fn if_chain(&mut self, chain: &'a IfChain) -> Result<Flow, Error> {
        for arm in &chain.arms {
            let take = match &arm.condition {
                None => true,
                Some(cond) => match self.expr(cond, None)? {
                    Value::Bool(b) => b,
                    other => {
                        return Err(self.err(
                            E_RUNTIME,
                            cond.span,
                            format!("a condition must be a bool, but this is {}.", other.type_name()),
                            "compare something, as in math { ('x') > 5 }.",
                        ))
                    }
                },
            };
            if take {
                return self.scoped_block(&arm.body);
            }
        }
        Ok(Flow::Normal)
    }

    /// Runs a loop, collecting each `handback` as one element — which is what makes a
    /// loop an array comprehension.
    fn loop_stmt(&mut self, l: &'a LoopStmt) -> Result<Vec<Value>, Error> {
        let mut collected = Vec::new();

        match &l.kind {
            LoopKind::Counted { var, range, .. } => {
                let (from, to, by) = self.range_bounds(range)?;
                let mut i = from;
                while (by > 0 && i <= to) || (by < 0 && i >= to) {
                    self.tick(l.span)?;
                    self.scopes.push(HashMap::new());
                    self.define(var, Value::Int(i));
                    let flow = self.block(&l.body);
                    self.scopes.pop();
                    if let Flow::Handback(v) = flow? {
                        collected.push(v);
                    }
                    i += by;
                }
            }
            LoopKind::While { condition } => loop {
                self.tick(l.span)?;
                match self.expr(condition, None)? {
                    Value::Bool(true) => {}
                    Value::Bool(false) => break,
                    other => {
                        return Err(self.err(
                            E_RUNTIME,
                            condition.span,
                            format!("a loop condition must be a bool, but this is {}.", other.type_name()),
                            "compare something, as in math { ('n') > 1 }.",
                        ))
                    }
                }
                self.scopes.push(HashMap::new());
                let flow = self.block(&l.body);
                self.scopes.pop();
                if let Flow::Handback(v) = flow? {
                    collected.push(v);
                }
            },
        }

        Ok(collected)
    }

    fn range_bounds(&mut self, range: &'a Expr) -> Result<(i128, i128, i128), Error> {
        let inner = match &range.kind {
            ExprKind::Math(e) => e,
            _ => range,
        };
        match &inner.kind {
            ExprKind::Range { from, to, by } => {
                let f = self.expr(from, Some(Numeric::Int))?;
                let t = self.expr(to, Some(Numeric::Int))?;
                let step = match by {
                    Some(b) => as_int(&self.expr(b, Some(Numeric::Int))?, b.span)?,
                    None => 1,
                };
                if step == 0 {
                    return Err(self.err(
                        E_RUNTIME,
                        range.span,
                        "a loop step of 0 would never finish.",
                        "use a non-zero step.",
                    ));
                }
                Ok((as_int(&f, from.span)?, as_int(&t, to.span)?, step))
            }
            _ => Err(self.err(
                E_RUNTIME,
                range.span,
                "a counted loop needs a range.",
                "write loop:var:int 'i' = math { 1 to 10 } { … }.",
            )),
        }
    }

    // ── expressions ─────────────────────────────────────────────────────────

    fn expr(&mut self, e: &'a Expr, hint: Option<Numeric>) -> Result<Value, Error> {
        match &e.kind {
            ExprKind::Literal(text) => Ok(literal_value(text, hint)),
            ExprKind::Number(text) => Ok(literal_value(text, hint)),
            ExprKind::Str(s) => Ok(Value::Str(s.clone())),
            ExprKind::Constant(c) => {
                let v = match c {
                    Constant::Pi => std::f64::consts::PI,
                    Constant::E => std::f64::consts::E,
                    Constant::Tau => std::f64::consts::TAU,
                };
                Ok(Value::Deci(Decimal::from_f64(v, 15)))
            }
            ExprKind::Math(inner) => self.expr(inner, hint),
            ExprKind::Ref { name, selectors } => {
                let Some(base) = self.lookup(name).cloned() else {
                    return Err(self.err(
                        E_RUNTIME,
                        e.span,
                        format!("there is no variable called '{name}' here."),
                        "check the spelling and that it is declared before this point.",
                    ));
                };
                self.apply_selectors(base, selectors, e.span)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let mut a = self.expr(lhs, hint)?;
                let mut b = self.expr(rhs, hint)?;
                // Rule A: a *bare* array reference sums its elements. One carrying a
                // selector — `('a'):all;` — stays an array, so the operation is
                // elementwise.
                //
                // The array operators are the exception: `· × ⊙ ⊗` imply `:all;`,
                // because they have no scalar meaning at all.
                let implies_all = matches!(
                    op,
                    BinOp::Dot | BinOp::Cross | BinOp::Hadamard | BinOp::Tensor
                );
                if !implies_all {
                    if is_bare_ref(lhs) {
                        a = try_reduce_array(a).ok_or_else(|| self.sum_overflowed(lhs.span))?;
                    }
                    if is_bare_ref(rhs) {
                        b = try_reduce_array(b).ok_or_else(|| self.sum_overflowed(rhs.span))?;
                    }
                }
                self.binary(*op, a, b, e.span, hint)
            }
            ExprKind::Unary { op, operand } => {
                let v = self.expr(operand, hint)?;
                self.unary(*op, v, e.span, hint)
            }
            ExprKind::ArrayLit(items) => {
                let mut values = Vec::new();
                let mut inner_shape: Option<Vec<usize>> = None;
                for item in items {
                    let v = self.expr(item, hint)?;
                    if let Value::Array(a) = &v {
                        inner_shape.get_or_insert_with(|| a.shape.clone());
                        values.extend(a.items.clone());
                    } else {
                        values.push(v);
                    }
                }
                let mut shape = vec![items.len()];
                if let Some(inner) = inner_shape {
                    shape.extend(inner);
                }
                Ok(Value::Array(Array { items: values, shape }))
            }
            ExprKind::If(chain) => match self.if_chain(chain)? {
                Flow::Handback(v) => Ok(v),
                Flow::Normal => Ok(Value::Nothing),
            },
            ExprKind::Loop(l) => {
                let items = self.loop_stmt(l)?;
                // Nesting builds higher rank: an inner loop handing back a vector
                // makes the outer one a matrix.
                let outer = items.len();
                let mut inner_shape: Option<Vec<usize>> = None;
                let mut flat = Vec::new();
                for item in items {
                    match item {
                        Value::Array(a) => {
                            match &inner_shape {
                                Some(first) if *first != a.shape => {
                                    // Every handback must produce the same shape, or
                                    // the array would be ragged.
                                    return Err(self.err(
                                        E_RUNTIME,
                                        l.span,
                                        format!(
                                            "every iteration must hand back the same shape, but one gave {:?} and another {:?}.",
                                            first, a.shape
                                        ),
                                        "arrays are rectangular; make each iteration produce the same shape.",
                                    ));
                                }
                                None => inner_shape = Some(a.shape.clone()),
                                _ => {}
                            }
                            flat.extend(a.items);
                        }
                        other => flat.push(other),
                    }
                }
                let mut shape = vec![outer];
                if let Some(inner) = inner_shape {
                    shape.extend(inner);
                }
                Ok(Value::Array(Array { items: flat, shape }))
            }
            ExprKind::Range { .. } => Err(self.err(
                E_RUNTIME,
                e.span,
                "a range is only meaningful in a loop or a selector.",
                "write it as loop:var:int 'i' = math { 1 to 10 } { … }.",
            )),
            ExprKind::Call { name, args } => self.call(name, args, e.span),
            ExprKind::Builtin { name, args } => self.builtin(name, args, e.span, hint),
        }
    }

    fn call(&mut self, name: &str, args: &'a [Expr], span: Span) -> Result<Value, Error> {
        let Some(func) = self.functions.get(name).copied() else {
            return Err(self.err(
                E_RUNTIME,
                span,
                format!("there is no function called '{name}'."),
                "check the name, and that the function is declared.",
            ));
        };

        let mut values = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            let hint = func.params.get(i).and_then(|p| numeric_hint(&p.ty));
            values.push(self.expr(arg, hint)?);
        }

        self.scopes.push(HashMap::new());
        for (param, value) in func.params.iter().zip(values) {
            self.define(&param.name, value);
        }
        let flow = self.block(&func.body);
        self.scopes.pop();

        match flow? {
            Flow::Handback(v) => Ok(v),
            Flow::Normal => Ok(Value::Nothing),
        }
    }

    fn builtin(
        &mut self,
        name: &str,
        args: &'a [Expr],
        span: Span,
        hint: Option<Numeric>,
    ) -> Result<Value, Error> {
        match name {
            "print" => {
                let mut line = String::new();
                for a in args {
                    line.push_str(&self.expr(a, None)?.to_string());
                }
                self.lines.push(line);
                Ok(Value::Nothing)
            }
            "read" => {
                let path = match args.first() {
                    Some(a) => self.expr(a, None)?.to_string(),
                    None => String::new(),
                };
                match std::fs::read_to_string(&path) {
                    Ok(text) => Ok(Value::Str(text)),
                    Err(e) => Err(self.err(
                        E_RUNTIME,
                        span,
                        format!("{path} could not be read — {e}."),
                        "check the path, and that the file exists.",
                    )),
                }
            }
            "parse" => {
                let text = match args.first() {
                    Some(a) => self.expr(a, None)?.to_string(),
                    None => String::new(),
                };
                let options: Vec<String> = args[1..]
                    .iter()
                    .filter_map(|a| match &a.kind {
                        ExprKind::Ref { name, .. } => Some(name.clone()),
                        ExprKind::Literal(v) => Some(v.clone()),
                        _ => None,
                    })
                    .collect();
                let candidate = if options.iter().any(|o| o == "trim") {
                    text.trim()
                } else {
                    text.as_str()
                };
                match Decimal::parse(candidate) {
                    Some(d) => Ok(coerce(Value::Deci(d), hint)),
                    None => Err(self.err(
                        E_PARSE,
                        span,
                        format!("{candidate:?} is not a number AHPCL can read."),
                        "parse is strict by default; add trim for surrounding spaces.",
                    )),
                }
            }
            "clock" => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                Ok(Value::Deci(Decimal::from_f64(now, 6)))
            }
            _ => Err(self.err(
                E_RUNTIME,
                span,
                format!("'{name}' is not a builtin AHPCL knows."),
                "the builtins are print, read, parse and clock.",
            )),
        }
    }

    /// Apply a whole selector chain.
    ///
    /// Consecutive index selectors address **successive dimensions** — the first picks
    /// rows, the second columns, and so on. `:length;` and `:shape;` are questions
    /// about the array rather than dimension selectors, so they consume whatever has
    /// been narrowed so far and the chain continues on their result.
    fn apply_selectors(
        &mut self,
        base: Value,
        selectors: &'a [Selector],
        span: Span,
    ) -> Result<Value, Error> {
        let mut value = base;
        let mut run: Vec<&Selector> = Vec::new();

        for sel in selectors {
            match sel {
                Selector::Length | Selector::Shape => {
                    value = self.apply_dimension_run(value, &run, span)?;
                    run.clear();
                    value = self.query(value, sel, span)?;
                }
                _ => run.push(sel),
            }
        }
        self.apply_dimension_run(value, &run, span)
    }

    /// `:length;` and `:shape;`.
    fn query(&mut self, value: Value, sel: &Selector, span: Span) -> Result<Value, Error> {
        let Value::Array(arr) = &value else {
            return match sel {
                Selector::Length => Ok(Value::Int(1)),
                _ => Err(self.err(
                    E_RUNTIME,
                    span,
                    format!("{} is not an array, so it has no shape.", value.type_name()),
                    "selectors apply to vectors, matrices and tensors.",
                )),
            };
        };
        Ok(match sel {
            Selector::Length => Value::Int(arr.items.len() as i128),
            _ => Value::Array(Array::vector(
                arr.shape.iter().map(|d| Value::Int(*d as i128)).collect(),
            )),
        })
    }

    /// Apply one run of dimension selectors, one per dimension in order.
    fn apply_dimension_run(
        &mut self,
        value: Value,
        run: &[&'a Selector],
        span: Span,
    ) -> Result<Value, Error> {
        if run.is_empty() {
            return Ok(value);
        }
        let Value::Array(arr) = value else {
            return Err(self.err(
                E_RUNTIME,
                span,
                "this is not an array, so it cannot be selected from.",
                "selectors apply to vectors, matrices and tensors.",
            ));
        };
        if run.len() > arr.shape.len() {
            return Err(self.err(
                E_RUNTIME,
                span,
                format!(
                    "{} selectors were given, but this array has {} dimension{}.",
                    run.len(),
                    arr.shape.len(),
                    if arr.shape.len() == 1 { "" } else { "s" }
                ),
                "one selector addresses one dimension.",
            ));
        }

        // Which positions to keep along each dimension, and whether that dimension
        // collapses (a single index gives a plain value).
        let mut picks: Vec<Vec<usize>> = Vec::new();
        let mut collapse: Vec<bool> = Vec::new();
        for (dim, sel) in run.iter().enumerate() {
            let extent = arr.shape[dim];
            let (chosen, single) = self.positions(sel, extent, span)?;
            for &p in &chosen {
                if p >= extent {
                    return Err(self.err(
                        E_BOUNDS,
                        span,
                        format!(
                            "index {} is out of range for dimension {} of length {extent}.",
                            p + 1,
                            dim + 1
                        ),
                        "indices start at 1.",
                    ));
                }
            }
            picks.push(chosen);
            collapse.push(single);
        }
        // Dimensions with no selector are kept whole.
        for dim in run.len()..arr.shape.len() {
            picks.push((0..arr.shape[dim]).collect());
            collapse.push(false);
        }

        let strides: Vec<usize> = (0..arr.shape.len())
            .map(|d| arr.shape[d + 1..].iter().product::<usize>().max(1))
            .collect();

        let mut items = Vec::new();
        let mut counter = vec![0usize; picks.len()];
        loop {
            let offset: usize = counter
                .iter()
                .enumerate()
                .map(|(d, &c)| picks[d][c] * strides[d])
                .sum();
            items.push(arr.items[offset].clone());

            let mut d = picks.len();
            loop {
                if d == 0 {
                    let shape: Vec<usize> = picks
                        .iter()
                        .zip(&collapse)
                        .filter(|(_, c)| !**c)
                        .map(|(p, _)| p.len())
                        .collect();
                    return Ok(if shape.is_empty() {
                        items.into_iter().next().expect("one value")
                    } else {
                        Value::Array(Array { items, shape })
                    });
                }
                d -= 1;
                counter[d] += 1;
                if counter[d] < picks[d].len() {
                    break;
                }
                counter[d] = 0;
            }
        }
    }

    /// Zero-based positions a selector picks along one dimension, and whether it was a
    /// single index (which collapses the dimension).
    fn positions(
        &mut self,
        sel: &'a Selector,
        extent: usize,
        span: Span,
    ) -> Result<(Vec<usize>, bool), Error> {
        match sel {
            Selector::All => Ok(((0..extent).collect(), false)),
            Selector::Indices(items) => {
                let mut out = Vec::new();
                for item in items {
                    let v = self.expr(item, Some(Numeric::Int))?;
                    out.push(as_index(&v, item.span)? - 1);
                }
                let single = out.len() == 1;
                Ok((out, single))
            }
            Selector::Range { from, to, by } => {
                let f = as_int(&self.expr(from, Some(Numeric::Int))?, from.span)?;
                let t = as_int(&self.expr(to, Some(Numeric::Int))?, to.span)?;
                let step = match by {
                    Some(b) => as_int(&self.expr(b, Some(Numeric::Int))?, b.span)?,
                    None => 1,
                };
                if step == 0 {
                    return Err(self.err(
                        E_RUNTIME,
                        span,
                        "a selector step of 0 would never finish.",
                        "use a non-zero step.",
                    ));
                }
                let mut out = Vec::new();
                let mut i = f;
                while (step > 0 && i <= t) || (step < 0 && i >= t) {
                    if i < 1 {
                        return Err(self.err(
                            E_BOUNDS,
                            span,
                            format!("{i} is not a valid index."),
                            "indices start at 1.",
                        ));
                    }
                    out.push(i as usize - 1);
                    i += step;
                }
                Ok((out, false))
            }
            Selector::Length | Selector::Shape => unreachable!("handled by apply_selectors"),
        }
    }

    #[allow(dead_code)]
    fn select(&mut self, value: Value, sel: &'a Selector, span: Span) -> Result<Value, Error> {
        let arr = match &value {
            Value::Array(a) => a.clone(),
            _ => match sel {
                Selector::Length => return Ok(Value::Int(1)),
                _ => {
                    return Err(self.err(
                        E_RUNTIME,
                        span,
                        format!("{} is not an array, so it cannot be selected from.", value.type_name()),
                        "selectors apply to vectors, matrices and tensors.",
                    ))
                }
            },
        };

        match sel {
            Selector::All => Ok(Value::Array(arr)),
            Selector::Length => Ok(Value::Int(arr.items.len() as i128)),
            Selector::Shape => Ok(Value::Array(Array::vector(
                arr.shape.iter().map(|d| Value::Int(*d as i128)).collect(),
            ))),
            Selector::Indices(items) => {
                let mut picked = Vec::new();
                for item in items {
                    let v = self.expr(item, Some(Numeric::Int))?;
                    let idx = as_index(&v, item.span)?;
                    let flat = flat_index(&arr.shape, &[idx]).ok_or_else(|| {
                        self.err(
                            E_BOUNDS,
                            item.span,
                            format!("index {idx} is out of range for an array of {} elements.", arr.items.len()),
                            "indices start at 1.",
                        )
                    })?;
                    // With more than one dimension, one index selects a whole row.
                    let stride: usize = arr.shape[1..].iter().product::<usize>().max(1);
                    if arr.shape.len() > 1 {
                        let start = (idx - 1) * stride;
                        picked.push(Value::Array(Array {
                            items: arr.items[start..start + stride].to_vec(),
                            shape: arr.shape[1..].to_vec(),
                        }));
                    } else {
                        picked.push(arr.items[flat].clone());
                    }
                }
                if picked.len() == 1 {
                    Ok(picked.into_iter().next().expect("one item"))
                } else {
                    Ok(Value::Array(Array::vector(picked)))
                }
            }
            Selector::Range { from, to, by } => {
                let f = as_int(&self.expr(from, Some(Numeric::Int))?, from.span)?;
                let t = as_int(&self.expr(to, Some(Numeric::Int))?, to.span)?;
                let step = match by {
                    Some(b) => as_int(&self.expr(b, Some(Numeric::Int))?, b.span)?,
                    None => 1,
                };
                let mut picked = Vec::new();
                let mut i = f;
                while (step > 0 && i <= t) || (step < 0 && i >= t) {
                    if i >= 1 && (i as usize) <= arr.items.len() {
                        picked.push(arr.items[i as usize - 1].clone());
                    }
                    i += step;
                }
                Ok(Value::Array(Array::vector(picked)))
            }
        }
    }

    fn binary(
        &mut self,
        op: BinOp,
        a: Value,
        b: Value,
        span: Span,
        hint: Option<Numeric>,
    ) -> Result<Value, Error> {
        use BinOp::*;

        if matches!(op, And | Or) {
            let (x, y) = (as_bool(&a, span)?, as_bool(&b, span)?);
            return Ok(Value::Bool(if op == And { x && y } else { x || y }));
        }

        if matches!(op, Hadamard | Cross | Tensor | Dot) {
            return self.array_binary(op, a, b, span);
        }

        // Anything still an array here came through `:all;`, so the operation runs
        // elementwise. A scalar on the other side broadcasts across it.
        if matches!(a, Value::Array(_)) || matches!(b, Value::Array(_)) {
            return self.elementwise(op, a, b, span, hint);
        }

        if matches!(op, Eq | NotEq | Less | Greater | LessEq | GreaterEq) {
            return self.compare(op, a, b, span);
        }

        self.arithmetic(op, a, b, span, hint)
    }

    /// Apply `op` position by position, broadcasting a scalar across the array.
    fn elementwise(
        &mut self,
        op: BinOp,
        a: Value,
        b: Value,
        span: Span,
        hint: Option<Numeric>,
    ) -> Result<Value, Error> {
        let (items, shape) = match (&a, &b) {
            (Value::Array(x), Value::Array(y)) => {
                if x.items.len() != y.items.len() {
                    return Err(self.err(
                        E_RUNTIME,
                        span,
                        format!(
                            "elementwise operations need matching shapes, but these are {:?} and {:?}.",
                            x.shape, y.shape
                        ),
                        "make the shapes agree.",
                    ));
                }
                let pairs: Vec<(Value, Value)> =
                    x.items.iter().cloned().zip(y.items.iter().cloned()).collect();
                (pairs, x.shape.clone())
            }
            (Value::Array(x), scalar) => (
                x.items.iter().cloned().map(|v| (v, scalar.clone())).collect(),
                x.shape.clone(),
            ),
            (scalar, Value::Array(y)) => (
                y.items.iter().cloned().map(|v| (scalar.clone(), v)).collect(),
                y.shape.clone(),
            ),
            _ => unreachable!("at least one side is an array"),
        };

        let mut out = Vec::with_capacity(items.len());
        for (p, q) in items {
            let v = if matches!(op, BinOp::Eq | BinOp::NotEq | BinOp::Less | BinOp::Greater | BinOp::LessEq | BinOp::GreaterEq) {
                self.compare(op, p, q, span)?
            } else {
                self.arithmetic(op, p, q, span, hint)?
            };
            out.push(v);
        }
        Ok(Value::Array(Array { items: out, shape }))
    }

    fn compare(&mut self, op: BinOp, a: Value, b: Value, span: Span) -> Result<Value, Error> {
        use std::cmp::Ordering;
        let ord = match (&a, &b) {
            (Value::Str(x), Value::Str(y)) => x.cmp(y),
            (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
            _ => {
                let (x, y) = (to_rational(&a, span)?, to_rational(&b, span)?);
                x.compare(y).ok_or_else(|| {
                    self.err(E_OVERFLOW, span, "comparison overflowed.", "use smaller values.")
                })?
            }
        };
        Ok(Value::Bool(match op {
            BinOp::Eq => ord == Ordering::Equal,
            BinOp::NotEq => ord != Ordering::Equal,
            BinOp::Less => ord == Ordering::Less,
            BinOp::Greater => ord == Ordering::Greater,
            BinOp::LessEq => ord != Ordering::Greater,
            BinOp::GreaterEq => ord != Ordering::Less,
            _ => unreachable!("not a comparison"),
        }))
    }

    fn arithmetic(
        &mut self,
        op: BinOp,
        a: Value,
        b: Value,
        span: Span,
        hint: Option<Numeric>,
    ) -> Result<Value, Error> {
        use BinOp::*;

        let overflow = |_this: &Self| {
            Error::new(
                E_OVERFLOW,
                span,
                "this calculation overflowed the value's precision.",
                "widen the type, or use infnum.",
            )
        };

        // Integers stay integers where they can, which keeps results exact and small.
        if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
            let (x, y) = (*x, *y);
            let result = match op {
                Add => x.checked_add(y),
                Sub => x.checked_sub(y),
                Mul => x.checked_mul(y),
                IntDiv => {
                    if y == 0 {
                        return Err(self.err(E_DIV_ZERO, span, "division by zero.", "check the divisor."));
                    }
                    Some(x.div_euclid(y))
                }
                Mod => {
                    if y == 0 {
                        return Err(self.err(E_DIV_ZERO, span, "remainder by zero.", "check the divisor."));
                    }
                    Some(x.rem_euclid(y))
                }
                Pow => {
                    if y < 0 || y > u32::MAX as i128 {
                        None
                    } else {
                        x.checked_pow(y as u32)
                    }
                }
                Div => None, // handled below, since the result is usually not an integer
                _ => None,
            };
            if let Some(v) = result {
                return Ok(Value::Int(v));
            }
            if !matches!(op, Div) {
                return Err(overflow(self));
            }
        }

        // Division follows the context: rat where exactness was asked for, decimal
        // otherwise. Both paths are exact — no f64 anywhere.
        if op == Div {
            if let Some(Numeric::Rat) = hint {
                let (x, y) = (to_rational(&a, span)?, to_rational(&b, span)?);
                let r = x.div(y).ok_or_else(|| {
                    self.err(E_DIV_ZERO, span, "division by zero.", "check the divisor.")
                })?;
                return Ok(Value::Rat(r));
            }
            let (x, y) = (
                to_decimal(&a).ok_or_else(|| self.not_a_number(&a, span))?,
                to_decimal(&b).ok_or_else(|| self.not_a_number(&b, span))?,
            );
            let d = x.div_exact(y, 15).ok_or_else(|| {
                if y.is_zero() {
                    self.err(E_DIV_ZERO, span, "division by zero.", "check the divisor.")
                } else {
                    overflow(self)
                }
            })?;
            return Ok(Value::Deci(d.normalised()));
        }

        // Exact decimal arithmetic where either side is a decimal.
        if matches!(a, Value::Deci(_)) || matches!(b, Value::Deci(_)) {
            if let (Some(x), Some(y)) = (to_decimal(&a), to_decimal(&b)) {
                if matches!(op, IntDiv | Mod) && y.is_zero() {
                    return Err(self.err(
                        E_DIV_ZERO,
                        span,
                        if op == IntDiv { "division by zero." } else { "remainder by zero." },
                        "check the divisor.",
                    ));
                }
                // `//` truncates and gives a whole number; `mod` gives the remainder.
                // They are different operations.
                if op == IntDiv {
                    return x.int_div(y).map(Value::Int).ok_or_else(|| overflow(self));
                }
                let out = match op {
                    Add => x.add(y),
                    Sub => x.sub(y),
                    Mul => x.mul(y),
                    // Integer exponents stay exact rather than round-tripping f64.
                    Pow => match y.normalised() {
                        e if e.scale == 0 && e.mantissa >= 0 && e.mantissa <= u32::MAX as i128 => {
                            x.pow_int(e.mantissa as u32)
                        }
                        _ => Decimal::from_f64_checked(x.to_f64().powf(y.to_f64()), 15),
                    },
                    Mod => x.rem(y),
                    _ => None,
                };
                return out.map(|d| Value::Deci(d.normalised())).ok_or_else(|| overflow(self));
            }
        }

        // Otherwise exact rational arithmetic.
        let (x, y) = (to_rational(&a, span)?, to_rational(&b, span)?);
        let out = match op {
            Add => x.add(y),
            Sub => x.sub(y),
            Mul => x.mul(y),
            // An integer exponent keeps a rational exact: (1/3)² is 1/9.
            Pow if y.den == 1 && y.num.abs() <= i32::MAX as i128 => x.pow_int(y.num as i32),
            _ => None,
        };
        out.map(Value::Rat).ok_or_else(|| overflow(self))
    }

    fn array_binary(&mut self, op: BinOp, a: Value, b: Value, span: Span) -> Result<Value, Error> {
        let (Value::Array(x), Value::Array(y)) = (&a, &b) else {
            return Err(self.err(
                E_RUNTIME,
                span,
                "this operator works on arrays, but one side is a single value.",
                "use ordinary arithmetic for single numbers.",
            ));
        };

        match op {
            BinOp::Hadamard => {
                if x.items.len() != y.items.len() {
                    return Err(self.err(
                        E_RUNTIME,
                        span,
                        format!(
                            "elementwise operations need matching shapes, but these are {:?} and {:?}.",
                            x.shape, y.shape
                        ),
                        "make the shapes agree.",
                    ));
                }
                let mut items = Vec::new();
                for (p, q) in x.items.iter().zip(&y.items) {
                    items.push(self.arithmetic(BinOp::Mul, p.clone(), q.clone(), span, None)?);
                }
                Ok(Value::Array(Array { items, shape: x.shape.clone() }))
            }
            BinOp::Dot => {
                // For two vectors this is the dot product; the general case is matrix
                // multiplication, which is the same operation.
                if x.shape.len() == 1 && y.shape.len() == 1 {
                    let mut total = Value::Int(0);
                    for (p, q) in x.items.iter().zip(&y.items) {
                        let prod = self.arithmetic(BinOp::Mul, p.clone(), q.clone(), span, None)?;
                        total = self.arithmetic(BinOp::Add, total, prod, span, None)?;
                    }
                    return Ok(total);
                }
                self.matmul(x, y, span)
            }
            BinOp::Cross => {
                let (u, v) = (&x.items, &y.items);
                if u.len() != 3 || v.len() != 3 {
                    return Err(self.err(
                        E_RUNTIME,
                        span,
                        "cross product is defined for two 3-element vectors.",
                        "use · for the dot product, or ⊙ for elementwise.",
                    ));
                }
                let mut out = Vec::new();
                for (i, j) in [(1, 2), (2, 0), (0, 1)] {
                    let l = self.arithmetic(BinOp::Mul, u[i].clone(), v[j].clone(), span, None)?;
                    let r = self.arithmetic(BinOp::Mul, u[j].clone(), v[i].clone(), span, None)?;
                    out.push(self.arithmetic(BinOp::Sub, l, r, span, None)?);
                }
                Ok(Value::Array(Array::vector(out)))
            }
            BinOp::Tensor => {
                let mut items = Vec::new();
                for p in &x.items {
                    for q in &y.items {
                        items.push(self.arithmetic(BinOp::Mul, p.clone(), q.clone(), span, None)?);
                    }
                }
                let shape: Vec<usize> = x.shape.iter().chain(&y.shape).copied().collect();
                Ok(Value::Array(Array { items, shape }))
            }
            _ => Err(self.err(E_RUNTIME, span, "unsupported array operation.", "")),
        }
    }

    fn matmul(&mut self, x: &Array, y: &Array, span: Span) -> Result<Value, Error> {
        let (m, k) = (x.shape[0], *x.shape.get(1).unwrap_or(&1));
        let (k2, n) = (y.shape[0], *y.shape.get(1).unwrap_or(&1));
        if k != k2 {
            return Err(self.err(
                E_RUNTIME,
                span,
                format!("matrix multiplication requires inner dimensions to agree: {k} ≠ {k2}."),
                "transpose one side, or use a matching shape.",
            ));
        }
        let mut items = Vec::with_capacity(m * n);
        for row in 0..m {
            for col in 0..n {
                let mut total = Value::Int(0);
                for i in 0..k {
                    let a = x.items[row * k + i].clone();
                    let b = y.items[i * n + col].clone();
                    let prod = self.arithmetic(BinOp::Mul, a, b, span, None)?;
                    total = self.arithmetic(BinOp::Add, total, prod, span, None)?;
                }
                items.push(total);
            }
        }
        Ok(Value::Array(Array { items, shape: vec![m, n] }))
    }

    fn unary(&mut self, op: UnOp, v: Value, span: Span, hint: Option<Numeric>) -> Result<Value, Error> {
        match op {
            UnOp::Not => Ok(Value::Bool(!as_bool(&v, span)?)),
            UnOp::Neg => match try_reduce_array(v).ok_or_else(|| self.sum_overflowed(span))? {
                Value::Int(n) => Ok(Value::Int(-n)),
                Value::Deci(d) => Ok(Value::Deci(Decimal::new(-d.mantissa, d.scale))),
                Value::Rat(r) => Ok(Value::Rat(Rational { num: -r.num, den: r.den })),
                other => Err(self.err(
                    E_RUNTIME,
                    span,
                    format!("negation needs a number, but this is {}.", other.type_name()),
                    "check the operand.",
                )),
            },
            UnOp::Abs => match try_reduce_array(v).ok_or_else(|| self.sum_overflowed(span))? {
                Value::Int(n) => Ok(Value::Int(n.abs())),
                Value::Deci(d) => Ok(Value::Deci(Decimal::new(d.mantissa.abs(), d.scale))),
                Value::Rat(r) => Ok(Value::Rat(Rational { num: r.num.abs(), den: r.den })),
                other => Err(self.err(
                    E_RUNTIME,
                    span,
                    format!("absolute value needs a number, but this is {}.", other.type_name()),
                    "check the operand.",
                )),
            },
            UnOp::Floor | UnOp::Ceil => {
                let f = try_reduce_array(v)
                    .and_then(|r| r.to_f64())
                    .ok_or_else(|| {
                        self.err(E_RUNTIME, span, "this needs a number.", "check the operand.")
                    })?;
                Ok(Value::Int(if op == UnOp::Floor {
                    f.floor() as i128
                } else {
                    f.ceil() as i128
                }))
            }
            _ => {
                // sqrt, sin, cos, tan, log, ln — usually irrational, so the result is a
                // rounded decimal. The Informer reports the rounding at check time.
                let f = try_reduce_array(v)
                    .and_then(|r| r.to_f64())
                    .ok_or_else(|| {
                        self.err(E_RUNTIME, span, "this operation needs a number.", "check the operand.")
                    })?;
                if op == UnOp::Sqrt && f < 0.0 {
                    return Err(self.err(
                        E_RUNTIME,
                        span,
                        "square root of a negative number is not a real number.",
                        "check the operand, or take the absolute value first.",
                    ));
                }
                let out = match op {
                    UnOp::Sqrt => f.sqrt(),
                    UnOp::Sin => f.sin(),
                    UnOp::Cos => f.cos(),
                    UnOp::Tan => f.tan(),
                    UnOp::Log => f.log10(),
                    UnOp::Ln => f.ln(),
                    _ => f,
                };
                // An exact root stays exact: √9 is 3, not 3.0000000001.
                if op == UnOp::Sqrt && out.fract() == 0.0 && hint != Some(Numeric::Deci) {
                    return Ok(Value::Int(out as i128));
                }
                Ok(Value::Deci(Decimal::from_f64(out, 15).normalised()))
            }
        }
    }
}

/// Which numeric family the surrounding context asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Numeric {
    Int,
    Deci,
    Rat,
}

fn numeric_hint(ty: &TypeRef) -> Option<Numeric> {
    match ty.base.as_str() {
        "int" => Some(Numeric::Int),
        "deci" => Some(Numeric::Deci),
        "rat" => Some(Numeric::Rat),
        _ => None,
    }
}

fn literal_value(text: &str, hint: Option<Numeric>) -> Value {
    if text == "true" {
        return Value::Bool(true);
    }
    if text == "false" {
        return Value::Bool(false);
    }
    match Decimal::parse(text) {
        Some(d) => coerce(Value::Deci(d), hint),
        None => Value::Str(text.to_string()),
    }
}

fn coerce(v: Value, hint: Option<Numeric>) -> Value {
    match (&v, hint) {
        (Value::Deci(d), Some(Numeric::Int)) if d.scale == 0 => Value::Int(d.mantissa),
        (Value::Deci(d), Some(Numeric::Rat)) => {
            Rational::from_decimal(*d).map(Value::Rat).unwrap_or(v)
        }
        (Value::Deci(d), None) if d.scale == 0 => Value::Int(d.mantissa),
        _ => v,
    }
}

/// A bare array reference sums its elements — rule A from docs/types.md.
///
/// Returns `None` on overflow rather than wrapping, which every other integer path
/// already did.
fn try_reduce_array(v: Value) -> Option<Value> {
    match v {
        Value::Array(a) => {
            let mut total = Value::Int(0);
            for item in a.items {
                total = match (total, item) {
                    (Value::Int(x), Value::Int(y)) => Value::Int(x.checked_add(y)?),
                    (x, y) => {
                        let (p, q) = (to_decimal(&x)?, to_decimal(&y)?);
                        Value::Deci(p.add(q)?.normalised())
                    }
                };
            }
            Some(total)
        }
        other => Some(other),
    }
}

/// Whether an expression is a bare array reference — one with no selector — which is
/// what rule A reduces.
fn is_bare_ref(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Ref { selectors, .. } => selectors.is_empty(),
        ExprKind::Math(inner) => is_bare_ref(inner),
        _ => false,
    }
}

fn to_decimal(v: &Value) -> Option<Decimal> {
    match v {
        Value::Int(n) => Some(Decimal::from_int(*n)),
        Value::Deci(d) => Some(*d),
        Value::Rat(r) => Some(Decimal::from_f64(r.to_f64(), 15)),
        _ => None,
    }
}

fn to_rational(v: &Value, _span: Span) -> Result<Rational, Error> {
    match v {
        Value::Int(n) => Ok(Rational::from_int(*n)),
        Value::Rat(r) => Ok(*r),
        Value::Deci(d) => Ok(Rational::from_decimal(*d).unwrap_or(Rational::from_int(0))),
        _ => Ok(Rational::from_int(0)),
    }
}

fn as_bool(v: &Value, span: Span) -> Result<bool, Error> {
    match v {
        Value::Bool(b) => Ok(*b),
        other => Err(Error::new(
            E_RUNTIME,
            span,
            format!("this needs a bool, but it is {}.", other.type_name()),
            "compare something, as in math { ('x') > 5 }.",
        )),
    }
}

fn as_int(v: &Value, span: Span) -> Result<i128, Error> {
    match v {
        Value::Int(n) => Ok(*n),
        Value::Deci(d) if d.scale == 0 => Ok(d.mantissa),
        other => Err(Error::new(
            E_RUNTIME,
            span,
            format!("this needs a whole number, but it is {}.", other.type_name()),
            "use an int.",
        )),
    }
}

fn as_index(v: &Value, span: Span) -> Result<usize, Error> {
    let n = as_int(v, span)?;
    if n < 1 {
        return Err(Error::new(
            E_BOUNDS,
            span,
            format!("{n} is not a valid index."),
            "indices start at 1.",
        ));
    }
    Ok(n as usize)
}

/// Row-major flat offset. Indices are 1-based.
fn flat_index(shape: &[usize], indices: &[usize]) -> Option<usize> {
    if indices.is_empty() {
        return None;
    }
    let mut offset = 0;
    for (dim, idx) in indices.iter().enumerate() {
        if *idx < 1 || *idx > *shape.get(dim)? {
            return None;
        }
        let stride: usize = shape[dim + 1..].iter().product::<usize>().max(1);
        offset += (idx - 1) * stride;
    }
    Some(offset)
}
