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
        var_types: Vec::new(),
        fn_repr: HashMap::new(),
        current: None,
        string_count: 0,
    };

    cg.declare_printf();
    cg.declare_runtime();
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
    /// What representation each variable holds, so `print` and arithmetic pick the
    /// right path.
    var_types: Vec<HashMap<String, Native>>,
    fn_repr: HashMap<String, Native>,
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

    /// The exact-decimal layout shared with the runtime: mantissa, scale, failed.
    ///
    /// An exact decimal has no native LLVM representation, so arithmetic on it becomes
    /// a call into `ahpcl-runtime` rather than a machine instruction.
    fn deci_type(&self) -> inkwell::types::StructType<'ctx> {
        self.context.struct_type(
            &[
                self.context.i128_type().into(),
                self.context.i32_type().into(),
                self.context.i32_type().into(),
            ],
            false,
        )
    }

    /// Declare the runtime.
    ///
    /// Decimals cross the boundary **by pointer**. LLVM IR performs no platform ABI
    /// lowering, so a by-value `{ i128, i32, i32 }` in a signature means register
    /// passing, while the AArch64 C ABI passes a 24-byte struct indirectly. The two
    /// disagree silently and the call does nothing. Pointers avoid the question.
    fn declare_runtime(&mut self) {
        let i32t = self.context.i32_type();
        let i64t = self.i64();
        let void = self.context.void_type();
        let p = self.context.ptr_type(AddressSpace::default());
        let i8ptr = p;

        for name in ["ahpcl_deci_add", "ahpcl_deci_sub", "ahpcl_deci_mul"] {
            let ty = void.fn_type(&[p.into(), p.into(), p.into()], false);
            self.module.add_function(name, ty, None);
        }
        self.module.add_function(
            "ahpcl_deci_div",
            void.fn_type(&[p.into(), p.into(), p.into(), i32t.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_deci_cmp",
            i32t.fn_type(&[p.into(), p.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_deci_from_int",
            void.fn_type(&[p.into(), i64t.into()], false),
            None,
        );
        self.module
            .add_function("ahpcl_print_deci", void.fn_type(&[p.into()], false), None);
        self.module.add_function(
            "ahpcl_print_int",
            self.context.void_type().fn_type(&[i64t.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_print_str",
            self.context.void_type().fn_type(&[i8ptr.into()], false),
            None,
        );
        self.module
            .add_function("ahpcl_print_newline", self.context.void_type().fn_type(&[], false), None);
        self.module.add_function(
            "ahpcl_fail",
            self.context.void_type().fn_type(&[i8ptr.into(), i8ptr.into()], false),
            None,
        );
    }

    /// A decimal constant, as an LLVM struct value.
    fn deci_const(&self, mantissa: i128, scale: u32) -> BasicValueEnum<'ctx> {
        let lo = (mantissa as u128 & u64::MAX as u128) as u64;
        let hi = ((mantissa as u128) >> 64) as u64;
        let m = self.context.i128_type().const_int_arbitrary_precision(&[lo, hi]);
        self.deci_type()
            .const_named_struct(&[
                m.into(),
                self.context.i32_type().const_int(scale as u64, false).into(),
                self.context.i32_type().const_zero().into(),
            ])
            .into()
    }

    /// Put a decimal value into a stack slot and hand back its address.
    fn spill(&self, v: BasicValueEnum<'ctx>, name: &str) -> PointerValue<'ctx> {
        let slot = self.alloca(name, v.get_type());
        self.builder.build_store(slot, v).unwrap();
        slot
    }

    /// Call a runtime function that writes its decimal result through an out-pointer.
    fn call_deci(&self, name: &str, args: &[BasicMetadataValueEnum<'ctx>]) -> Option<BasicValueEnum<'ctx>> {
        let out = self.alloca("rt.out", self.deci_type().into());
        let mut all: Vec<BasicMetadataValueEnum<'ctx>> = vec![out.into()];
        all.extend_from_slice(args);
        let f = self.module.get_function(name)?;
        self.builder.build_call(f, &all, "rt").unwrap();
        Some(self.builder.build_load(self.deci_type(), out, "rt.val").unwrap())
    }

    fn call_runtime(&self, name: &str, args: &[BasicMetadataValueEnum<'ctx>]) -> Option<BasicValueEnum<'ctx>> {
        let f = self.module.get_function(name)?;
        let call = self.builder.build_call(f, args, "rt").unwrap();
        match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => Some(v),
            _ => None,
        }
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
                    Native::Deci => self.deci_type().into(),
                    Native::None => return Err(Unsupported::new("a parameter of type none")),
                });
            }
            let fn_type = match ret_native {
                Native::Int => self.i64().fn_type(&params, false),
                Native::Bool => self.bool_type().fn_type(&params, false),
                Native::Deci => self.deci_type().fn_type(&params, false),
                Native::None => self.context.void_type().fn_type(&params, false),
            };
            let value = self.module.add_function(&mangle(&f.name), fn_type, None);
            self.functions.insert(f.name.clone(), value);
            self.fn_repr.insert(f.name.clone(), ret_native);
        }
        Ok(())
    }

    fn function_body(&mut self, f: &FuncDecl) -> Result<(), Unsupported> {
        let function = *self.functions.get(&f.name).expect("declared above");
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);
        self.current = Some(function);
        self.vars.push(HashMap::new());
        self.var_types.push(HashMap::new());

        for (i, p) in f.params.iter().enumerate() {
            let arg = function.get_nth_param(i as u32).expect("a parameter");
            let slot = self.alloca(&p.name, arg.get_type());
            self.builder.build_store(slot, arg).unwrap();
            self.vars.last_mut().unwrap().insert(p.name.clone(), slot);
            self.var_types
                .last_mut()
                .unwrap()
                .insert(p.name.clone(), native_base(&p.ty).unwrap_or(Native::Int));
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
                Native::Deci => {
                    let zero = self.deci_const(0, 0);
                    self.builder.build_return(Some(&zero)).unwrap();
                }
            }
        }

        self.vars.pop();
        self.var_types.pop();
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
        self.var_types.push(HashMap::new());

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
        self.var_types.pop();
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
        let slot = tmp.build_alloca(ty, name).unwrap();

        // A decimal contains an `i128`, which the platform requires to be 16-byte
        // aligned. LLVM defaults a struct alloca to 8, and Rust's `#[repr(C)]` layout
        // assumes 16 — reading through the mismatch is undefined behaviour that
        // happens to work in release and aborts under debug assertions.
        if ty.is_struct_type() {
            if let Some(inst) = slot.as_instruction() {
                let _ = inst.set_alignment(16);
            }
        }
        slot
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
        self.var_types.push(HashMap::new());
        let out = self.block(stmts);
        self.vars.pop();
        self.var_types.pop();
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
                    self.var_types.last_mut().unwrap().insert(b.name.clone(), native);
                }
                Ok(false)
            }
            Stmt::Change(c) => {
                let native = native_base(&c.ty)?;
                for target in &c.targets {
                    if !target.selectors.is_empty() {
                        return Err(Unsupported::new("writing to an array element"));
                    }
                    let val = self.expr(&target.value, native)?;
                    let Some(slot) = self.lookup(&target.name) else {
                        return Err(Unsupported::new(format!("changing unknown '{}'", target.name)));
                    };
                    self.builder.build_store(slot, val).unwrap();
                }
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
        for arg in args {
            match &arg.kind {
                ExprKind::Str(s) => {
                    let text = self.global_string(s);
                    self.call_runtime("ahpcl_print_str", &[text.into()]);
                }
                _ => {
                    // Print through the runtime so a decimal keeps its exact digits.
                    let want = self.value_repr(arg);
                    let v = self.expr(arg, want)?;
                    match want {
                        Native::Deci => {
                            let p = self.spill(v, "print.deci");
                            self.call_runtime("ahpcl_print_deci", &[p.into()]);
                        }
                        _ => {
                            self.call_runtime("ahpcl_print_int", &[v.into()]);
                        }
                    }
                }
            }
        }
        self.call_runtime("ahpcl_print_newline", &[]);
        Ok(())
    }

    /// Which representation an expression naturally produces.
    fn value_repr(&self, e: &Expr) -> Native {
        match &e.kind {
            ExprKind::Ref { name, .. } => self
                .var_types
                .iter()
                .rev()
                .find_map(|f| f.get(name).copied())
                .unwrap_or(Native::Int),
            ExprKind::Math(inner) => self.value_repr(inner),
            ExprKind::Number(t) | ExprKind::Literal(t) if t.contains('.') => Native::Deci,
            ExprKind::Binary { lhs, rhs, .. } => {
                if self.value_repr(lhs) == Native::Deci || self.value_repr(rhs) == Native::Deci {
                    Native::Deci
                } else {
                    Native::Int
                }
            }
            ExprKind::Call { name, .. } => self
                .fn_repr
                .get(name)
                .copied()
                .unwrap_or(Native::Int),
            _ => Native::Int,
        }
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
                if text.contains('.') || want == Native::Deci {
                    let (mantissa, scale) = decimal_parts(text)
                        .ok_or_else(|| Unsupported::new(format!("the literal '{text}'")))?;
                    return Ok(self.deci_const(mantissa, scale));
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
                let held = self
                    .var_types
                    .iter()
                    .rev()
                    .find_map(|f| f.get(name).copied())
                    .unwrap_or(want);
                let loaded = match held {
                    Native::Bool => self.builder.build_load(self.bool_type(), slot, name).unwrap(),
                    Native::Deci => self.builder.build_load(self.deci_type(), slot, name).unwrap(),
                    _ => self.builder.build_load(self.i64(), slot, name).unwrap(),
                };
                // An int flowing into a decimal context is widened by the runtime.
                if want == Native::Deci && held != Native::Deci {
                    return self
                        .call_deci("ahpcl_deci_from_int", &[loaded.into()])
                        .ok_or_else(|| Unsupported::new("widening an int to a decimal"));
                }
                Ok(loaded)
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
            ExprKind::Option { .. } => Err(Unsupported::new("builtin options")),
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

        // Exact decimals have no machine instruction, so their arithmetic becomes a
        // runtime call.
        if self.value_repr(lhs) == Native::Deci || self.value_repr(rhs) == Native::Deci {
            return self.decimal_binary(op, lhs, rhs);
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
            Div => {
                return Err(Unsupported::new(
                    "division between integers, whose result is a decimal or rational",
                ))
            }
            other => return Err(Unsupported::new(format!("the operator {other:?}"))),
        };
        Ok(v.into())
    }

    fn decimal_binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        use BinOp::*;
        let a = self.expr(lhs, Native::Deci)?;
        let b = self.expr(rhs, Native::Deci)?;

        let pa = self.spill(a, "lhs");
        let pb = self.spill(b, "rhs");

        let name = match op {
            Add => "ahpcl_deci_add",
            Sub => "ahpcl_deci_sub",
            Mul => "ahpcl_deci_mul",
            Div => {
                let digits = self.context.i32_type().const_int(15, false);
                return self
                    .call_deci("ahpcl_deci_div", &[pa.into(), pb.into(), digits.into()])
                    .ok_or_else(|| Unsupported::new("decimal division"));
            }
            Eq | NotEq | Less | Greater | LessEq | GreaterEq => {
                let cmp = self
                    .call_runtime("ahpcl_deci_cmp", &[pa.into(), pb.into()])
                    .ok_or_else(|| Unsupported::new("decimal comparison"))?
                    .into_int_value();
                let zero = self.context.i32_type().const_zero();
                let predicate = match op {
                    Eq => IntPredicate::EQ,
                    NotEq => IntPredicate::NE,
                    Less => IntPredicate::SLT,
                    Greater => IntPredicate::SGT,
                    LessEq => IntPredicate::SLE,
                    _ => IntPredicate::SGE,
                };
                return Ok(self
                    .builder
                    .build_int_compare(predicate, cmp, zero, "deccmp")
                    .unwrap()
                    .into());
            }
            other => return Err(Unsupported::new(format!("{other:?} on decimals"))),
        };
        self.call_deci(name, &[pa.into(), pb.into()])
            .ok_or_else(|| Unsupported::new("decimal arithmetic"))
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
    /// An exact decimal, held as the runtime's struct and operated on by calls.
    Deci,
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
        "deci" => Ok(Native::Deci),
        "none" => Ok(Native::None),
        other => Err(Unsupported::new(format!("the type '{other}'"))),
    }
}

/// Split `"0.1"` into mantissa and scale: `(1, 1)`, meaning 1/10¹.
fn decimal_parts(text: &str) -> Option<(i128, u32)> {
    let t = text.trim();
    let (negative, body) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (whole, frac) = body.split_once('.').unwrap_or((body, ""));
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let joined = format!("{whole}{frac}");
    let m: i128 = joined.parse().ok()?;
    Some((if negative { -m } else { m }, frac.len() as u32))
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
