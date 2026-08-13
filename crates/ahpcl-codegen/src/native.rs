//! The native backend.
//!
//! One pass over the tree, emitting LLVM IR. Integers are `i64`, booleans `i1`, and
//! `print` becomes a `printf` call — so the produced object links against libc with no
//! AHPCL runtime library needed yet.

use std::collections::HashMap;
use std::path::Path;

use ahpcl_syntax::ast::*;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FunctionValue, PointerValue};
use inkwell::{AddressSpace, IntPredicate, OptimizationLevel};

/// Why a program could not be compiled natively yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub what: String,
}

impl Unsupported {
    fn new(what: impl Into<String>) -> Unsupported {
        Unsupported { what: what.into() }
    }
}

pub struct Compiled {
    pub ir: String,
}

/// Compile a program to an object file at `object_path`, returning the IR.
pub fn compile(program: &Program, object_path: &Path, module_name: &str) -> Result<Compiled, Unsupported> {
    let context = Context::create();
    let module = context.create_module(module_name);
    let builder = context.create_builder();

    let mut cg = Codegen {
        context: &context,
        module: &module,
        builder: &builder,
        vars: Vec::new(),
        functions: HashMap::new(),
        current: None,
        string_count: 0,
    };

    cg.declare_printf();
    cg.declare_functions(program)?;
    for stmt in &program.statements {
        if let Stmt::Func(f) = stmt {
            cg.function_body(f)?;
        }
    }
    cg.main(program)?;

    let ir = module.print_to_string().to_string();
    emit_object(&module, object_path)?;
    Ok(Compiled { ir })
}

fn emit_object(module: &Module, path: &Path) -> Result<(), Unsupported> {
    Target::initialize_native(&InitializationConfig::default())
        .map_err(|e| Unsupported::new(format!("LLVM native target unavailable: {e}")))?;

    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple)
        .map_err(|e| Unsupported::new(format!("no LLVM target for {triple:?}: {e}")))?;
    let machine = target
        .create_target_machine(
            &triple,
            &TargetMachine::get_host_cpu_name().to_string(),
            &TargetMachine::get_host_cpu_features().to_string(),
            OptimizationLevel::Default,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| Unsupported::new("could not create an LLVM target machine"))?;

    machine
        .write_to_file(module, FileType::Object, path)
        .map_err(|e| Unsupported::new(format!("could not write the object file: {e}")))
}

struct Codegen<'ctx, 'a> {
    context: &'ctx Context,
    module: &'a Module<'ctx>,
    builder: &'a Builder<'ctx>,
    /// One frame per scope, because blocks scope.
    vars: Vec<HashMap<String, PointerValue<'ctx>>>,
    functions: HashMap<String, FunctionValue<'ctx>>,
    current: Option<FunctionValue<'ctx>>,
    string_count: usize,
}

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    fn i64(&self) -> inkwell::types::IntType<'ctx> {
        self.context.i64_type()
    }

    fn bool_type(&self) -> inkwell::types::IntType<'ctx> {
        self.context.bool_type()
    }

    fn declare_printf(&mut self) {
        let i8ptr = self.context.ptr_type(AddressSpace::default());
        let ty = self.context.i32_type().fn_type(&[i8ptr.into()], true);
        self.module.add_function("printf", ty, None);
    }

    fn declare_functions(&mut self, program: &Program) -> Result<(), Unsupported> {
        for stmt in &program.statements {
            let Stmt::Func(f) = stmt else { continue };
            let ret_native = native_base(&f.returns)?;
            let mut params = Vec::new();
            for p in &f.params {
                if p.shape.is_some() {
                    return Err(Unsupported::new("array parameters"));
                }
                params.push(match native_base(&p.ty)? {
                    Native::Int => self.i64().into(),
                    Native::Bool => self.bool_type().into(),
                    Native::None => return Err(Unsupported::new("a parameter of type none")),
                });
            }
            let fn_type = match ret_native {
                Native::Int => self.i64().fn_type(&params, false),
                Native::Bool => self.bool_type().fn_type(&params, false),
                Native::None => self.context.void_type().fn_type(&params, false),
            };
            let value = self.module.add_function(&mangle(&f.name), fn_type, None);
            self.functions.insert(f.name.clone(), value);
        }
        Ok(())
    }

    fn function_body(&mut self, f: &FuncDecl) -> Result<(), Unsupported> {
        let function = *self.functions.get(&f.name).expect("declared above");
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);
        self.current = Some(function);
        self.vars.push(HashMap::new());

        for (i, p) in f.params.iter().enumerate() {
            let arg = function.get_nth_param(i as u32).expect("a parameter");
            let slot = self.alloca(&p.name, arg.get_type());
            self.builder.build_store(slot, arg).unwrap();
            self.vars.last_mut().unwrap().insert(p.name.clone(), slot);
        }

        let terminated = self.block(&f.body)?;
        if !terminated {
            match native_base(&f.returns)? {
                Native::None => {
                    self.builder.build_return(None).unwrap();
                }
                Native::Int => {
                    let zero = self.i64().const_zero();
                    self.builder.build_return(Some(&zero)).unwrap();
                }
                Native::Bool => {
                    let zero = self.bool_type().const_zero();
                    self.builder.build_return(Some(&zero)).unwrap();
                }
            }
        }

        self.vars.pop();
        self.current = None;
        Ok(())
    }

    fn main(&mut self, program: &Program) -> Result<(), Unsupported> {
        let fn_type = self.context.i32_type().fn_type(&[], false);
        let function = self.module.add_function("main", fn_type, None);
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);
        self.current = Some(function);
        self.vars.push(HashMap::new());

        let top: Vec<Stmt> = program
            .statements
            .iter()
            .filter(|s| !matches!(s, Stmt::Func(_)))
            .cloned()
            .collect();
        let terminated = self.block(&top)?;
        if !terminated {
            let zero = self.context.i32_type().const_zero();
            self.builder.build_return(Some(&zero)).unwrap();
        }

        self.vars.pop();
        self.current = None;
        Ok(())
    }

    fn alloca(&self, name: &str, ty: inkwell::types::BasicTypeEnum<'ctx>) -> PointerValue<'ctx> {
        // Allocate in the entry block so the stack slot is hoisted, which is what LLVM
        // expects for mem2reg to promote it to a register.
        let function = self.current.expect("inside a function");
        let entry = function.get_first_basic_block().expect("an entry block");
        let tmp = self.context.create_builder();
        match entry.get_first_instruction() {
            Some(first) => tmp.position_before(&first),
            None => tmp.position_at_end(entry),
        }
        tmp.build_alloca(ty, name).unwrap()
    }

    fn lookup(&self, name: &str) -> Option<PointerValue<'ctx>> {
        self.vars.iter().rev().find_map(|f| f.get(name).copied())
    }

    /// Returns true when the block ended with a terminator, so the caller must not
    /// append another.
    fn block(&mut self, stmts: &[Stmt]) -> Result<bool, Unsupported> {
        for stmt in stmts {
            if self.statement(stmt)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn scoped(&mut self, stmts: &[Stmt]) -> Result<bool, Unsupported> {
        self.vars.push(HashMap::new());
        let out = self.block(stmts);
        self.vars.pop();
        out
    }

    fn statement(&mut self, stmt: &Stmt) -> Result<bool, Unsupported> {
        match stmt {
            Stmt::Func(_) => Ok(false),
            Stmt::Var(v) => {
                let native = native_base(&v.ty)?;
                for b in &v.bindings {
                    if b.shape.is_some() {
                        return Err(Unsupported::new("arrays"));
                    }
                    let Some(value) = &b.value else {
                        return Err(Unsupported::new("a declaration with no value"));
                    };
                    let val = self.expr(value, native)?;
                    let slot = self.alloca(&b.name, val.get_type());
                    self.builder.build_store(slot, val).unwrap();
                    self.vars.last_mut().unwrap().insert(b.name.clone(), slot);
                }
                Ok(false)
            }
            Stmt::Change(c) => {
                if !c.selectors.is_empty() {
                    return Err(Unsupported::new("writing to an array element"));
                }
                let native = native_base(&c.ty)?;
                let val = self.expr(&c.value, native)?;
                let Some(slot) = self.lookup(&c.name) else {
                    return Err(Unsupported::new(format!("changing unknown '{}'", c.name)));
                };
                self.builder.build_store(slot, val).unwrap();
                Ok(false)
            }
            Stmt::Print { args, .. } => {
                self.print(args)?;
                Ok(false)
            }
            Stmt::If(chain) => self.if_chain(chain),
            Stmt::Loop(l) => {
                self.loop_stmt(l)?;
                Ok(false)
            }
            Stmt::Handback { value, .. } => {
                let function = self.current.expect("inside a function");
                let ret = function.get_type().get_return_type();
                match ret {
                    None => {
                        self.builder.build_return(None).unwrap();
                    }
                    Some(t) => {
                        let native = if t.is_int_type() && t.into_int_type().get_bit_width() == 1 {
                            Native::Bool
                        } else {
                            Native::Int
                        };
                        let v = self.expr(value, native)?;
                        self.builder.build_return(Some(&v)).unwrap();
                    }
                }
                Ok(true)
            }
            Stmt::Expr(e) => {
                self.expr(e, Native::Int)?;
                Ok(false)
            }
        }
    }

    fn print(&mut self, args: &[Expr]) -> Result<(), Unsupported> {
        let printf = self.module.get_function("printf").expect("declared");
        for arg in args {
            match &arg.kind {
                ExprKind::Str(s) => {
                    let fmt = self.global_string(s);
                    self.builder
                        .build_call(printf, &[fmt.into()], "print")
                        .unwrap();
                }
                _ => {
                    let v = self.expr(arg, Native::Int)?;
                    let fmt = self.global_string("%lld");
                    let args: Vec<BasicMetadataValueEnum> = vec![fmt.into(), v.into()];
                    self.builder.build_call(printf, &args, "print").unwrap();
                }
            }
        }
        let newline = self.global_string("\n");
        self.builder
            .build_call(printf, &[newline.into()], "nl")
            .unwrap();
        Ok(())
    }

    fn global_string(&mut self, text: &str) -> PointerValue<'ctx> {
        self.string_count += 1;
        let name = format!("str.{}", self.string_count);
        self.builder
            .build_global_string_ptr(text, &name)
            .unwrap()
            .as_pointer_value()
    }

    fn if_chain(&mut self, chain: &IfChain) -> Result<bool, Unsupported> {
        let function = self.current.expect("inside a function");
        let merge = self.context.append_basic_block(function, "endif");
        let mut all_terminated = true;

        for arm in &chain.arms {
            match &arm.condition {
                Some(cond) => {
                    let c = self.expr(cond, Native::Bool)?;
                    let then_bb = self.context.append_basic_block(function, "then");
                    let else_bb = self.context.append_basic_block(function, "else");
                    self.builder
                        .build_conditional_branch(c.into_int_value(), then_bb, else_bb)
                        .unwrap();

                    self.builder.position_at_end(then_bb);
                    let terminated = self.scoped(&arm.body)?;
                    if !terminated {
                        all_terminated = false;
                        self.builder.build_unconditional_branch(merge).unwrap();
                    }
                    self.builder.position_at_end(else_bb);
                }
                None => {
                    let terminated = self.scoped(&arm.body)?;
                    if !terminated {
                        all_terminated = false;
                        self.builder.build_unconditional_branch(merge).unwrap();
                    }
                }
            }
        }

        // Whatever block we are left positioned in falls through to the merge.
        if self
            .builder
            .get_insert_block()
            .map(|b| b.get_terminator().is_none())
            .unwrap_or(false)
        {
            all_terminated = false;
            self.builder.build_unconditional_branch(merge).unwrap();
        }

        self.builder.position_at_end(merge);
        if all_terminated {
            // Nothing reaches the merge; give it a terminator so the IR verifies.
            self.builder.build_unreachable().unwrap();
            return Ok(true);
        }
        Ok(false)
    }

    fn loop_stmt(&mut self, l: &LoopStmt) -> Result<(), Unsupported> {
        let function = self.current.expect("inside a function");

        match &l.kind {
            LoopKind::Counted { var, range, .. } => {
                let inner = match &range.kind {
                    ExprKind::Math(e) => e.as_ref(),
                    _ => range,
                };
                let ExprKind::Range { from, to, by } = &inner.kind else {
                    return Err(Unsupported::new("a counted loop without a range"));
                };

                let start = self.expr(from, Native::Int)?.into_int_value();
                let end = self.expr(to, Native::Int)?.into_int_value();
                let step = match by {
                    Some(b) => self.expr(b, Native::Int)?.into_int_value(),
                    None => self.i64().const_int(1, true),
                };

                let slot = self.alloca(var, self.i64().into());
                self.builder.build_store(slot, start).unwrap();

                let cond_bb = self.context.append_basic_block(function, "loop.cond");
                let body_bb = self.context.append_basic_block(function, "loop.body");
                let done_bb = self.context.append_basic_block(function, "loop.done");

                self.builder.build_unconditional_branch(cond_bb).unwrap();
                self.builder.position_at_end(cond_bb);
                let i = self
                    .builder
                    .build_load(self.i64(), slot, "i")
                    .unwrap()
                    .into_int_value();
                // Counting up while i <= end. A negative step counts down instead.
                let up = self
                    .builder
                    .build_int_compare(IntPredicate::SGT, step, self.i64().const_zero(), "up")
                    .unwrap();
                let le = self
                    .builder
                    .build_int_compare(IntPredicate::SLE, i, end, "le")
                    .unwrap();
                let ge = self
                    .builder
                    .build_int_compare(IntPredicate::SGE, i, end, "ge")
                    .unwrap();
                let cont = self.builder.build_select(up, le, ge, "cont").unwrap();
                self.builder
                    .build_conditional_branch(cont.into_int_value(), body_bb, done_bb)
                    .unwrap();

                self.builder.position_at_end(body_bb);
                self.vars.push(HashMap::new());
                self.vars.last_mut().unwrap().insert(var.clone(), slot);
                let terminated = self.block(&l.body)?;
                self.vars.pop();
                if !terminated {
                    let cur = self
                        .builder
                        .build_load(self.i64(), slot, "i")
                        .unwrap()
                        .into_int_value();
                    let next = self.builder.build_int_add(cur, step, "next").unwrap();
                    self.builder.build_store(slot, next).unwrap();
                    self.builder.build_unconditional_branch(cond_bb).unwrap();
                }

                self.builder.position_at_end(done_bb);
            }
            LoopKind::While { condition } => {
                let cond_bb = self.context.append_basic_block(function, "while.cond");
                let body_bb = self.context.append_basic_block(function, "while.body");
                let done_bb = self.context.append_basic_block(function, "while.done");

                self.builder.build_unconditional_branch(cond_bb).unwrap();
                self.builder.position_at_end(cond_bb);
                let c = self.expr(condition, Native::Bool)?;
                self.builder
                    .build_conditional_branch(c.into_int_value(), body_bb, done_bb)
                    .unwrap();

                self.builder.position_at_end(body_bb);
                let terminated = self.scoped(&l.body)?;
                if !terminated {
                    self.builder.build_unconditional_branch(cond_bb).unwrap();
                }
                self.builder.position_at_end(done_bb);
            }
        }
        Ok(())
    }

    fn expr(&mut self, e: &Expr, want: Native) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        match &e.kind {
            ExprKind::Math(inner) => self.expr(inner, want),
            ExprKind::Number(text) | ExprKind::Literal(text) => {
                if text == "true" {
                    return Ok(self.bool_type().const_int(1, false).into());
                }
                if text == "false" {
                    return Ok(self.bool_type().const_zero().into());
                }
                if text.contains('.') {
                    return Err(Unsupported::new("decimal values"));
                }
                let n: i64 = text
                    .parse()
                    .map_err(|_| Unsupported::new(format!("the literal '{text}'")))?;
                Ok(self.i64().const_int(n as u64, true).into())
            }
            ExprKind::Ref { name, selectors } => {
                if !selectors.is_empty() {
                    return Err(Unsupported::new("selectors"));
                }
                let Some(slot) = self.lookup(name) else {
                    return Err(Unsupported::new(format!("the variable '{name}'")));
                };
                let ty = match want {
                    Native::Bool => self.bool_type(),
                    _ => self.i64(),
                };
                Ok(self.builder.build_load(ty, slot, name).unwrap())
            }
            ExprKind::Unary { op, operand } => match op {
                UnOp::Neg => {
                    let v = self.expr(operand, Native::Int)?.into_int_value();
                    Ok(self.builder.build_int_neg(v, "neg").unwrap().into())
                }
                UnOp::Not => {
                    let v = self.expr(operand, Native::Bool)?.into_int_value();
                    Ok(self.builder.build_not(v, "not").unwrap().into())
                }
                UnOp::Abs => {
                    let v = self.expr(operand, Native::Int)?.into_int_value();
                    let neg = self.builder.build_int_neg(v, "neg").unwrap();
                    let is_neg = self
                        .builder
                        .build_int_compare(IntPredicate::SLT, v, self.i64().const_zero(), "isneg")
                        .unwrap();
                    Ok(self.builder.build_select(is_neg, neg, v, "abs").unwrap())
                }
                other => Err(Unsupported::new(format!("the operator {other:?}"))),
            },
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),
            ExprKind::Call { name, args } => {
                let Some(function) = self.functions.get(name).copied() else {
                    return Err(Unsupported::new(format!("the function '{name}'")));
                };
                let mut values: Vec<BasicMetadataValueEnum> = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let p = function
                        .get_type()
                        .get_param_types()
                        .get(i)
                        .copied()
                        .ok_or_else(|| Unsupported::new("too many arguments"))?;
                    let want = if p.is_int_type() && p.into_int_type().get_bit_width() == 1 {
                        Native::Bool
                    } else {
                        Native::Int
                    };
                    values.push(self.expr(a, want)?.into());
                }
                let call = self.builder.build_call(function, &values, "call").unwrap();
                match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Ok(v),
                    _ => Err(Unsupported::new("using a none-producing call as a value")),
                }
            }
            ExprKind::Builtin { name, .. } => {
                Err(Unsupported::new(format!("the builtin '{name}'")))
            }
            ExprKind::ArrayLit(_) => Err(Unsupported::new("array literals")),
            ExprKind::If(_) => Err(Unsupported::new("a conditional used as a value")),
            ExprKind::Loop(_) => Err(Unsupported::new("a loop used as a value")),
            ExprKind::Constant(_) => Err(Unsupported::new("π, e and τ")),
            ExprKind::Str(_) => Err(Unsupported::new("text values")),
            ExprKind::Range { .. } => Err(Unsupported::new("a range outside a loop")),
        }
    }

    fn binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        use BinOp::*;

        if matches!(op, And | Or) {
            let a = self.expr(lhs, Native::Bool)?.into_int_value();
            let b = self.expr(rhs, Native::Bool)?.into_int_value();
            let v = if op == And {
                self.builder.build_and(a, b, "and").unwrap()
            } else {
                self.builder.build_or(a, b, "or").unwrap()
            };
            return Ok(v.into());
        }

        let a = self.expr(lhs, Native::Int)?.into_int_value();
        let b = self.expr(rhs, Native::Int)?.into_int_value();

        let predicate = match op {
            Eq => Some(IntPredicate::EQ),
            NotEq => Some(IntPredicate::NE),
            Less => Some(IntPredicate::SLT),
            Greater => Some(IntPredicate::SGT),
            LessEq => Some(IntPredicate::SLE),
            GreaterEq => Some(IntPredicate::SGE),
            _ => None,
        };
        if let Some(p) = predicate {
            return Ok(self.builder.build_int_compare(p, a, b, "cmp").unwrap().into());
        }

        let v = match op {
            Add => self.builder.build_int_add(a, b, "add").unwrap(),
            Sub => self.builder.build_int_sub(a, b, "sub").unwrap(),
            Mul => self.builder.build_int_mul(a, b, "mul").unwrap(),
            IntDiv => self.builder.build_int_signed_div(a, b, "idiv").unwrap(),
            Mod => self.builder.build_int_signed_rem(a, b, "mod").unwrap(),
            Pow => return self.int_pow(a, b),
            Div => return Err(Unsupported::new("division, which produces a decimal or rational")),
            other => return Err(Unsupported::new(format!("the operator {other:?}"))),
        };
        Ok(v.into())
    }

    /// Integer exponentiation by a loop, since LLVM has no integer power instruction.
    fn int_pow(&mut self, base: inkwell::values::IntValue<'ctx>, exp: inkwell::values::IntValue<'ctx>)
        -> Result<BasicValueEnum<'ctx>, Unsupported>
    {
        let function = self.current.expect("inside a function");
        let acc = self.alloca("pow.acc", self.i64().into());
        let counter = self.alloca("pow.i", self.i64().into());
        self.builder.build_store(acc, self.i64().const_int(1, false)).unwrap();
        self.builder.build_store(counter, self.i64().const_zero()).unwrap();

        let cond = self.context.append_basic_block(function, "pow.cond");
        let body = self.context.append_basic_block(function, "pow.body");
        let done = self.context.append_basic_block(function, "pow.done");

        self.builder.build_unconditional_branch(cond).unwrap();
        self.builder.position_at_end(cond);
        let i = self.builder.build_load(self.i64(), counter, "i").unwrap().into_int_value();
        let more = self.builder.build_int_compare(IntPredicate::SLT, i, exp, "more").unwrap();
        self.builder.build_conditional_branch(more, body, done).unwrap();

        self.builder.position_at_end(body);
        let cur = self.builder.build_load(self.i64(), acc, "acc").unwrap().into_int_value();
        let next = self.builder.build_int_mul(cur, base, "acc.next").unwrap();
        self.builder.build_store(acc, next).unwrap();
        let i2 = self.builder.build_load(self.i64(), counter, "i").unwrap().into_int_value();
        let i3 = self.builder.build_int_add(i2, self.i64().const_int(1, false), "i.next").unwrap();
        self.builder.build_store(counter, i3).unwrap();
        self.builder.build_unconditional_branch(cond).unwrap();

        self.builder.position_at_end(done);
        Ok(self.builder.build_load(self.i64(), acc, "pow").unwrap())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Native {
    Int,
    Bool,
    None,
}

/// Which native representation a declared type maps onto, if any.
fn native_base(ty: &TypeRef) -> Result<Native, Unsupported> {
    if ty.rank.is_some() {
        return Err(Unsupported::new("arrays"));
    }
    match ty.base.as_str() {
        "int" => Ok(Native::Int),
        "bool" => Ok(Native::Bool),
        "none" => Ok(Native::None),
        other => Err(Unsupported::new(format!("the type '{other}'"))),
    }
}

/// AHPCL names may contain anything, so they need mangling to be legal symbols.
fn mangle(name: &str) -> String {
    let mut out = String::from("ahpcl_");
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else {
            out.push_str(&format!("_{:x}", c as u32));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_mangle_into_legal_symbols() {
        assert_eq!(mangle("area"), "ahpcl_area");
        assert!(mangle("my variable").chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        assert!(mangle("😂").chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        assert_ne!(mangle("a b"), mangle("ab"), "mangling stays unambiguous");
    }
}
