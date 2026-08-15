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
use inkwell::passes::PassBuilderOptions;
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
        fn_params: HashMap::new(),
        current_ret: Native::None,
        handbacks: Vec::new(),
        owned_from: 0,
        continues: Vec::new(),
        temporaries: Vec::new(),
        digits: DEFAULT_DIGITS,
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

    // Invalid IR is a compiler bug, and an unoptimised build hides it: a struct field
    // built at the wrong width still round-trips, then becomes a crash the moment the
    // optimiser believes the types. Verifying here turns that into a message.
    module
        .verify()
        .map_err(|e| Unsupported::new(format!("generated invalid LLVM IR: {}", e.to_string())))?;

    // The middle-end pipeline. Without it the IR reaches instruction selection almost
    // as written: every variable is a stack slot, loaded and stored on every access,
    // with no constant folding or dead-code removal.
    //
    // Safe for AHPCL specifically because there is no floating point anywhere — the
    // usual worry, that an optimiser reassociates arithmetic and changes the answer,
    // applies to floats. Integer and pointer transforms preserve meaning exactly, and
    // the overflow checks are ordinary branches on an intrinsic's result, so they
    // survive. The differential test is what actually holds this to account.
    module
        .run_passes("default<O2>", &machine, PassBuilderOptions::create())
        .map_err(|e| Unsupported::new(format!("the optimiser failed: {e}")))?;

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
    /// The declared representation of each parameter, so a call passes the right thing.
    /// Reading it back off the LLVM type cannot work: several representations share one
    /// LLVM type, and a pointer where a struct belongs is a crash, not an error.
    fn_params: HashMap<String, Vec<Native>>,
    /// What the function being compiled hands back.
    current_ret: Native,
    /// Where `handback` should put its value. A function returns; a conditional used as
    /// a value stores into a slot; a loop used as a value appends to an array.
    handbacks: Vec<Handback<'ctx>>,
    /// The first scope frame the current function owns. Frames below it hold the
    /// parameters, which are *borrowed* from the caller — releasing those frees the
    /// caller's value out from under it.
    owned_from: usize,
    /// Where `handback` jumps to end an iteration: the innermost loop's advance block.
    continues: Vec<inkwell::basic_block::BasicBlock<'ctx>>,
    /// Heap values the current statement produced — arrays and boxed `num`s. Anything
    /// still here when the statement ends was a temporary, and is released.
    temporaries: Vec<(BasicValueEnum<'ctx>, Native)>,
    /// How many places to compute an irrational to, from the declaration in hand.
    /// `[36 digits]` on a `var:deci` sets it for that declaration's value.
    digits: u32,
    current: Option<FunctionValue<'ctx>>,
    string_count: usize,
}

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// The machine type an AHPCL `int` lives in.
    ///
    /// 128 bits, matching the interpreter. At 64 the two disagreed on any value past
    /// about 9.2×10¹⁸: the interpreter kept computing and native overflowed.
    fn int_type(&self) -> inkwell::types::IntType<'ctx> {
        self.context.i128_type()
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

    /// The exact-rational layout: numerator, denominator, failed.
    fn rat_type(&self) -> inkwell::types::StructType<'ctx> {
        self.context.struct_type(
            &[
                self.context.i128_type().into(),
                self.context.i128_type().into(),
                // `failed` is a flag, not a number the language can see.
                self.context.i64_type().into(),
            ],
            false,
        )
    }

    /// The LLVM type behind a native representation.
    fn repr_type(&self, n: Native) -> inkwell::types::BasicTypeEnum<'ctx> {
        match n {
            Native::Bool => self.bool_type().into(),
            Native::Deci => self.deci_type().into(),
            Native::Rat => self.rat_type().into(),
            Native::Str => self.str_type().into(),
            Native::Array(..) | Native::Num => self.ptr().into(),
            _ => self.int_type().into(),
        }
    }

    /// A stack buffer of `count` slots, for passing small lists to the runtime.
    fn alloca_array(
        &self,
        name: &str,
        ty: inkwell::types::BasicTypeEnum<'ctx>,
        count: usize,
    ) -> inkwell::values::PointerValue<'ctx> {
        let entry = self
            .builder
            .get_insert_block()
            .and_then(|b| b.get_parent())
            .and_then(|f| f.get_first_basic_block())
            .expect("a function to allocate in");
        let scratch = self.context.create_builder();
        match entry.get_first_instruction() {
            Some(i) => scratch.position_before(&i),
            None => scratch.position_at_end(entry),
        }
        let n = self.context.i64_type().const_int(count.max(1) as u64, false);
        let slot = scratch.build_array_alloca(ty, n, name).unwrap();
        if let Some(instruction) = slot.as_instruction() {
            // i128 needs 16-byte alignment; LLVM would otherwise pick 8 and the
            // program would work only by luck.
            let _ = instruction.set_alignment(16);
        }
        slot
    }

    /// Integer arithmetic that stops rather than wrapping.
    ///
    /// LLVM's overflow intrinsics hand back `{ result, overflowed }`; the flag is
    /// branched on so an overflow reaches the Error Handler, matching the interpreter
    /// and the documented rule that overflow is an error.
    fn checked_int(
        &mut self,
        a: inkwell::values::IntValue<'ctx>,
        b: inkwell::values::IntValue<'ctx>,
        intrinsic: &str,
        what: &str,
    ) -> Result<inkwell::values::IntValue<'ctx>, Unsupported> {
        let name = format!("llvm.{intrinsic}.with.overflow.i128");
        let function = match self.module.get_function(&name) {
            Some(f) => f,
            None => {
                let ty = self
                    .context
                    .struct_type(&[self.int_type().into(), self.bool_type().into()], false)
                    .fn_type(&[self.int_type().into(), self.int_type().into()], false);
                self.module.add_function(&name, ty, None)
            }
        };
        let call = self
            .builder
            .build_call(function, &[a.into(), b.into()], "chk")
            .unwrap();
        let pair = match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => v.into_struct_value(),
            _ => return Err(Unsupported::new("a checked integer operation")),
        };
        let value = self
            .builder
            .build_extract_value(pair, 0, "chk.v")
            .unwrap()
            .into_int_value();
        let overflowed = self
            .builder
            .build_extract_value(pair, 1, "chk.o")
            .unwrap()
            .into_int_value();

        let function = self.current.expect("inside a function");
        let bad = self.context.append_basic_block(function, "ovf");
        let ok = self.context.append_basic_block(function, "ovf.ok");
        self.builder.build_conditional_branch(overflowed, bad, ok).unwrap();
        self.builder.position_at_end(bad);
        self.fail("AHPCL-PREC-0004", &format!("this integer {what} overflowed"));
        self.builder.position_at_end(ok);
        Ok(value)
    }

    /// Stop the program through the Error Handler, and mark the block unreachable so
    /// LLVM knows nothing follows.
    fn fail(&mut self, code: &str, message: &str) {
        let code = self.global_string(code);
        let message = self.global_string(message);
        self.call_runtime("ahpcl_fail", &[code.into(), message.into()]);
        self.builder.build_unreachable().unwrap();
    }

    fn ptr(&self) -> inkwell::types::PointerType<'ctx> {
        self.context.ptr_type(inkwell::AddressSpace::default())
    }

    /// The text layout: a pointer into constant data or the heap, plus a byte length.
    fn str_type(&self) -> inkwell::types::StructType<'ctx> {
        // `len` is a byte count in the runtime's `AhpclStr`, so it stays 64-bit however
        // wide an AHPCL `int` is. `owner` is null for a literal and points at the
        // reference count for text built while the program runs.
        let ptr = self.context.ptr_type(inkwell::AddressSpace::default());
        self.context
            .struct_type(&[ptr.into(), self.context.i64_type().into(), ptr.into()], false)
    }

    /// A string literal as a `{ptr, len}` value pointing at constant data.
    fn str_value(&mut self, text: &str) -> BasicValueEnum<'ctx> {
        let global = self.global_string(text);
        // A byte length, so it matches `str_type`'s second field and the runtime's
        // `AhpclStr` — not the `int` width.
        let len = self.context.i64_type().const_int(text.len() as u64, false);
        // A literal lives in the binary's constant data, so it owns nothing.
        let owner = self.ptr().const_null();
        self.build_struct(self.str_type(), &[global.into(), len.into(), owner.into()])
    }

    /// Build a struct value, checking each field's type against the struct's.
    ///
    /// `build_insert_value` silently does nothing when the types disagree, leaving the
    /// field `undef`. That is *valid* IR, so `Module::verify` does not catch it and an
    /// unoptimised build often still works — until the optimiser takes `undef` at its
    /// word and the program reads a garbage length. A wrong width here is a bug in this
    /// compiler, not in anyone's program, so it fails loudly and immediately.
    fn build_struct(
        &self,
        ty: inkwell::types::StructType<'ctx>,
        fields: &[BasicValueEnum<'ctx>],
    ) -> BasicValueEnum<'ctx> {
        let mut v = ty.get_undef();
        for (i, field) in fields.iter().enumerate() {
            let want = ty
                .get_field_type_at_index(i as u32)
                .expect("a field at this index");
            assert_eq!(
                field.get_type(),
                want,
                "field {i} of {ty:?} was built as {:?}; the runtime reads it as {want:?}",
                field.get_type()
            );
            v = self
                .builder
                .build_insert_value(v, *field, i as u32, "f")
                .unwrap()
                .into_struct_value();
        }
        v.into()
    }

    /// A 128-bit constant, built from its two halves since `const_int` takes a `u64`.
    fn int_const(&self, v: i128) -> BasicValueEnum<'ctx> {
        let lo = (v as u128 & u64::MAX as u128) as u64;
        let hi = ((v as u128) >> 64) as u64;
        self.int_type()
            .const_int_arbitrary_precision(&[lo, hi])
            .into()
    }

    fn rat_const(&self, num: i128, den: i128) -> BasicValueEnum<'ctx> {
        let word = |v: i128| {
            let lo = (v as u128 & u64::MAX as u128) as u64;
            let hi = ((v as u128) >> 64) as u64;
            self.context.i128_type().const_int_arbitrary_precision(&[lo, hi])
        };
        self.rat_type()
            .const_named_struct(&[word(num).into(), word(den).into(), self.int_type().const_zero().into()])
            .into()
    }

    /// Call a runtime function that writes a rational through an out-pointer.
    fn call_rat(&self, name: &str, args: &[BasicMetadataValueEnum<'ctx>]) -> Option<BasicValueEnum<'ctx>> {
        let out = self.alloca("rt.rout", self.rat_type().into());
        let mut all: Vec<BasicMetadataValueEnum<'ctx>> = vec![out.into()];
        all.extend_from_slice(args);
        let f = self.module.get_function(name)?;
        self.builder.build_call(f, &all, "rt").unwrap();
        Some(self.builder.build_load(self.rat_type(), out, "rt.rval").unwrap())
    }

    /// Declare the runtime.
    ///
    /// Decimals cross the boundary **by pointer**. LLVM IR performs no platform ABI
    /// lowering, so a by-value `{ i128, i32, i32 }` in a signature means register
    /// passing, while the AArch64 C ABI passes a 24-byte struct indirectly. The two
    /// disagree silently and the call does nothing. Pointers avoid the question.
    fn declare_runtime(&mut self) {
        let i32t = self.context.i32_type();
        let i64t = self.int_type();
        let void = self.context.void_type();
        let p = self.context.ptr_type(AddressSpace::default());
        let i8ptr = p;

        for name in ["ahpcl_deci_add", "ahpcl_deci_sub", "ahpcl_deci_mul"] {
            let ty = void.fn_type(&[p.into(), p.into(), p.into()], false);
            self.module.add_function(name, ty, None);
        }
        self.module.add_function(
            "ahpcl_deci_binary",
            void.fn_type(&[p.into(), i32t.into(), p.into(), p.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_rat_binary",
            void.fn_type(&[p.into(), i32t.into(), p.into(), p.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_deci_int_div",
            i64t.fn_type(&[p.into(), p.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_constant",
            void.fn_type(&[p.into(), i32t.into(), i32t.into()], false),
            None,
        );
        for name in ["ahpcl_rat_add", "ahpcl_rat_sub", "ahpcl_rat_mul", "ahpcl_rat_div"] {
            let ty = void.fn_type(&[p.into(), p.into(), p.into()], false);
            self.module.add_function(name, ty, None);
        }
        self.module
            .add_function("ahpcl_rat_from_int", void.fn_type(&[p.into(), i64t.into()], false), None);
        self.module
            .add_function("ahpcl_rat_cmp", i32t.fn_type(&[p.into(), p.into()], false), None);
        self.module
            .add_function("ahpcl_print_rat", void.fn_type(&[p.into()], false), None);
        self.module
            .add_function("ahpcl_print_text", void.fn_type(&[p.into()], false), None);
        self.module
            .add_function("ahpcl_clock", void.fn_type(&[p.into()], false), None);
        self.module
            .add_function("ahpcl_str_cmp", i32t.fn_type(&[p.into(), p.into()], false), None);
        self.module
            .add_function("ahpcl_read_file", void.fn_type(&[p.into(), p.into()], false), None);
        self.module.add_function(
            "ahpcl_parse_int",
            i64t.fn_type(&[p.into(), self.context.i64_type().into(), p.into(), p.into()], false),
            None,
        );
        let ptr = self.context.ptr_type(inkwell::AddressSpace::default());
        let u32t = self.context.i32_type();
        let i8t_early = self.context.i8_type();
        self.module
            .add_function("ahpcl_num_from_int", ptr.fn_type(&[i64t.into()], false), None);
        self.module
            .add_function("ahpcl_num_from_bool", ptr.fn_type(&[i8t_early.into()], false), None);
        for name in ["ahpcl_num_from_deci", "ahpcl_num_from_rat"] {
            self.module
                .add_function(name, ptr.fn_type(&[p.into()], false), None);
        }
        self.module.add_function(
            "ahpcl_num_binary",
            ptr.fn_type(&[u32t.into(), p.into(), p.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_array_offset",
            i64t.fn_type(&[p.into(), p.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_array_compare",
            ptr.fn_type(&[u32t.into(), p.into(), p.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_array_unary",
            ptr.fn_type(&[u32t.into(), p.into(), u32t.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_num_unary",
            ptr.fn_type(&[u32t.into(), p.into(), u32t.into()], false),
            None,
        );
        self.module
            .add_function("ahpcl_num_cmp", i32t.fn_type(&[p.into(), p.into()], false), None);
        self.module
            .add_function("ahpcl_print_num", void.fn_type(&[p.into()], false), None);
        self.module
            .add_function("ahpcl_num_to_int", i64t.fn_type(&[p.into()], false), None);
        for name in ["ahpcl_num_to_deci", "ahpcl_num_to_rat"] {
            self.module
                .add_function(name, void.fn_type(&[p.into(), p.into()], false), None);
        }
        self.module.add_function(
            "ahpcl_deci_unary",
            void.fn_type(&[p.into(), u32t.into(), p.into(), u32t.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_rat_unary",
            void.fn_type(&[p.into(), u32t.into(), p.into(), u32t.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_array_new",
            ptr.fn_type(&[u32t.into(), u32t.into(), p.into()], false),
            None,
        );
        self.module
            .add_function("ahpcl_array_len", i64t.fn_type(&[p.into()], false), None);
        self.module
            .add_function("ahpcl_array_kind", u32t.fn_type(&[p.into()], false), None);
        self.module
            .add_function("ahpcl_array_shape", ptr.fn_type(&[p.into()], false), None);
        self.module
            .add_function("ahpcl_array_sum", ptr.fn_type(&[p.into()], false), None);
        self.module
            .add_function("ahpcl_array_empty", ptr.fn_type(&[u32t.into()], false), None);
        self.module
            .add_function("ahpcl_array_push_int", void.fn_type(&[p.into(), i64t.into()], false), None);
        self.module
            .add_function("ahpcl_array_push_array", void.fn_type(&[p.into(), p.into()], false), None);
        for name in [
            "ahpcl_array_retain",
            "ahpcl_array_release",
            "ahpcl_num_retain",
            "ahpcl_num_release",
            "ahpcl_str_retain",
            "ahpcl_str_release",
        ] {
            self.module.add_function(name, void.fn_type(&[p.into()], false), None);
        }
        self.module.add_function(
            "ahpcl_array_push_bool",
            void.fn_type(&[p.into(), i8t_early.into()], false),
            None,
        );
        for name in [
            "ahpcl_array_push_deci",
            "ahpcl_array_push_rat",
            "ahpcl_array_push_str",
            "ahpcl_array_push_num",
        ] {
            self.module
                .add_function(name, void.fn_type(&[p.into(), p.into()], false), None);
        }
        self.module
            .add_function("ahpcl_print_array", void.fn_type(&[p.into()], false), None);
        self.module.add_function(
            "ahpcl_array_select_run",
            ptr.fn_type(
                &[p.into(), p.into(), p.into(), p.into(), p.into(), i64t.into()],
                false,
            ),
            None,
        );
        self.module
            .add_function("ahpcl_array_is_scalar", i32t.fn_type(&[p.into()], false), None);
        self.module.add_function(
            "ahpcl_array_range",
            ptr.fn_type(&[p.into(), i64t.into(), i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_array_elementwise",
            ptr.fn_type(&[u32t.into(), p.into(), p.into()], false),
            None,
        );
        for name in [
            "ahpcl_array_hadamard",
            "ahpcl_array_dot",
            "ahpcl_array_cross",
            "ahpcl_array_tensor",
        ] {
            self.module
                .add_function(name, ptr.fn_type(&[p.into(), p.into()], false), None);
        }
        self.module.add_function(
            "ahpcl_array_set_int",
            void.fn_type(&[p.into(), i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_array_get_int",
            i64t.fn_type(&[p.into(), i64t.into()], false),
            None,
        );
        let i8t = self.context.i8_type();
        self.module.add_function(
            "ahpcl_array_set_bool",
            void.fn_type(&[p.into(), i64t.into(), i8t.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_array_get_bool",
            i8t.fn_type(&[p.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_array_set_num",
            void.fn_type(&[p.into(), i64t.into(), p.into()], false),
            None,
        );
        self.module.add_function(
            "ahpcl_array_get_num",
            ptr.fn_type(&[p.into(), i64t.into()], false),
            None,
        );
        for name in ["ahpcl_array_set_deci", "ahpcl_array_set_rat", "ahpcl_array_set_str"] {
            let ty = void.fn_type(&[p.into(), i64t.into(), p.into()], false);
            self.module.add_function(name, ty, None);
        }
        for name in ["ahpcl_array_get_deci", "ahpcl_array_get_rat", "ahpcl_array_get_str"] {
            let ty = void.fn_type(&[p.into(), p.into(), i64t.into()], false);
            self.module.add_function(name, ty, None);
        }
        let i64t2 = self.context.i64_type();
        for name in ["ahpcl_parse_deci", "ahpcl_parse_rat"] {
            let ty =
                void.fn_type(&[p.into(), p.into(), i64t2.into(), p.into(), p.into()], false);
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
            "ahpcl_print_bool",
            self.context.void_type().fn_type(&[self.context.i8_type().into()], false),
            None,
        );
        // Integer division goes through the runtime so it is Euclidean, matching the
        // interpreter, and so division by zero fails instead of being undefined.
        for name in ["ahpcl_int_div", "ahpcl_int_mod"] {
            self.module
                .add_function(name, i64t.fn_type(&[i64t.into(), i64t.into()], false), None);
        }
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

    fn call_runtime(&mut self, name: &str, args: &[BasicMetadataValueEnum<'ctx>]) -> Option<BasicValueEnum<'ctx>> {
        let f = self.module.get_function(name)?;
        let call = self.builder.build_call(f, args, "rt").unwrap();
        let out = match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => Some(v),
            _ => None,
        };
        // Anything the runtime allocates belongs to the statement that asked for it.
        // Recording here rather than where an expression finishes is what makes this
        // reliable: intermediates built inside helpers never pass through `expr`, so
        // recording there missed them and they leaked.
        if let Some(v) = out {
            if let Some(kind) = allocates(name) {
                self.temporaries.push((v, kind));
            }
        }
        out
    }

    fn declare_functions(&mut self, program: &Program) -> Result<(), Unsupported> {
        for stmt in &program.statements {
            let Stmt::Func(f) = stmt else { continue };
            let ret_native = native_base(&f.returns)?;
            let mut param_reprs = Vec::new();
            let mut params = Vec::new();
            for p in &f.params {
                param_reprs.push(native_base_shaped(&p.ty, p.shape.as_ref())?);
                params.push(match native_base_shaped(&p.ty, p.shape.as_ref())? {
                    Native::Int => self.int_type().into(),
                    Native::Bool => self.bool_type().into(),
                    Native::Deci => self.deci_type().into(),
                    Native::Rat => self.rat_type().into(),
                    Native::Str => self.str_type().into(),
                    Native::Array(..) | Native::Num => self.ptr().into(),
                    Native::None => return Err(Unsupported::new("a parameter of type none")),
                });
            }
            let fn_type = match ret_native {
                Native::Int => self.int_type().fn_type(&params, false),
                Native::Bool => self.bool_type().fn_type(&params, false),
                Native::Deci => self.deci_type().fn_type(&params, false),
                Native::Rat => self.rat_type().fn_type(&params, false),
                Native::Str => self.str_type().fn_type(&params, false),
                Native::Array(..) | Native::Num => self.ptr().fn_type(&params, false),
                Native::None => self.context.void_type().fn_type(&params, false),
            };
            let value = self.module.add_function(&mangle(&f.name), fn_type, None);
            self.functions.insert(f.name.clone(), value);
            self.fn_repr.insert(f.name.clone(), ret_native);
            self.fn_params.insert(f.name.clone(), param_reprs);
        }
        Ok(())
    }

    fn function_body(&mut self, f: &FuncDecl) -> Result<(), Unsupported> {
        let function = *self.functions.get(&f.name).expect("declared above");
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);
        self.current = Some(function);
        self.current_ret = native_base(&f.returns)?;
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
                .insert(
                    p.name.clone(),
                    native_base_shaped(&p.ty, p.shape.as_ref()).unwrap_or(Native::Int),
                );
        }

        // Parameters live in the frame just pushed; the body gets its own. Only the
        // body's frame is this function's to release.
        self.vars.push(HashMap::new());
        self.var_types.push(HashMap::new());
        let outer_owned = self.owned_from;
        self.owned_from = self.vars.len() - 1;

        let terminated = self.block(&f.body)?;
        if !terminated {
            // Before the implicit return, not after: once the return is emitted the
            // block is terminated and there is nowhere left to release. Doing it
            // afterwards silently leaked every counted local of every call.
            self.release_scope();
            match native_base(&f.returns)? {
                Native::None => {
                    self.builder.build_return(None).unwrap();
                }
                Native::Int => {
                    let zero = self.int_type().const_zero();
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
                Native::Rat => {
                    let zero = self.rat_const(0, 1);
                    self.builder.build_return(Some(&zero)).unwrap();
                }
                Native::Str => {
                    let empty = self.str_value("");
                    self.builder.build_return(Some(&empty)).unwrap();
                }
                Native::Array(..) | Native::Num => {
                    let null = self.ptr().const_null();
                    self.builder.build_return(Some(&null)).unwrap();
                }
            }
        }

        self.owned_from = outer_owned;
        self.vars.pop();
        self.var_types.pop();
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
            // Top-level variables, released before main returns.
            self.release_scope();
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
            // Each statement owns whatever arrays it makes. An array kept by a variable
            // is retained when it is stored, so releasing here brings it back to the
            // one reference the variable holds; a slice nobody kept drops to zero and
            // is freed. Without this, a loop that slices leaks once per iteration.
            let mark = self.temporaries.len();
            let terminated = self.statement(stmt)?;
            if terminated {
                // The statement branched away — `handback`, or a return. There is no
                // longer a block to put a release in, and emitting one would land after
                // a terminator. Forget them instead: leaking on the way out of a block
                // is worse than invalid IR, but only just, and it is bounded.
                self.temporaries.truncate(mark);
                return Ok(true);
            }
            self.release_temporaries(mark);
        }
        Ok(false)
    }

    /// Release every array recorded since `mark`.
    fn release_temporaries(&mut self, mark: usize) {
        if self.at_terminated_block() {
            self.temporaries.truncate(mark);
            return;
        }
        while self.temporaries.len() > mark {
            let (v, repr) = self.temporaries.pop().expect("a recorded temporary");
            let handle = self.counted_handle(v, repr);
            self.call_runtime(repr.release_fn(), &[handle.into()]);
        }
    }



    fn scoped(&mut self, stmts: &[Stmt]) -> Result<bool, Unsupported> {
        self.vars.push(HashMap::new());
        self.var_types.push(HashMap::new());
        let out = self.block(stmts);
        self.release_scope();
        self.vars.pop();
        self.var_types.pop();
        out
    }

    /// The pointer retain and release actually take.
    ///
    /// An array or a `num` *is* the heap pointer. Text is a `{ptr, len, owner}` value
    /// passed around whole, so the count lives behind its third field — null for a
    /// literal, which makes retain and release harmless no-ops on one.
    fn counted_handle(&self, v: BasicValueEnum<'ctx>, repr: Native) -> BasicValueEnum<'ctx> {
        if repr != Native::Str {
            return v;
        }
        self.builder
            .build_extract_value(v.into_struct_value(), 2, "str.owner")
            .unwrap()
    }

    /// Whether the builder sits in a block that has already branched away.
    fn at_terminated_block(&self) -> bool {
        self.builder
            .get_insert_block()
            .map(|b| b.get_terminator().is_some())
            .unwrap_or(true)
    }

    /// Release every live scope, innermost first — for a `handback` that leaves the
    /// whole call rather than one block.
    fn release_all_scopes(&mut self) {
        for depth in (self.owned_from..self.vars.len()).rev() {
            self.release_scope_at(depth);
        }
    }

    /// Release the arrays held by the innermost scope's variables, as it ends.
    fn release_scope(&mut self) {
        if self.vars.is_empty() {
            return;
        }
        self.release_scope_at(self.vars.len() - 1);
    }

    fn release_scope_at(&mut self, depth: usize) {
        if self.at_terminated_block() {
            return;
        }
        let held: Vec<(PointerValue<'ctx>, Native)> = self
            .vars
            .get(depth)
            .map(|frame| {
                frame
                    .iter()
                    .filter_map(|(name, slot)| {
                        let repr = self.var_types.get(depth)?.get(name).copied()?;
                        repr.is_counted().then_some((*slot, repr))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (slot, repr) in held {
            let ty = self.repr_type(repr);
            let v = self.builder.build_load(ty, slot, "scope.held").unwrap();
            let handle = self.counted_handle(v, repr);
            self.call_runtime(repr.release_fn(), &[handle.into()]);
        }
    }

    fn statement(&mut self, stmt: &Stmt) -> Result<bool, Unsupported> {
        match stmt {
            Stmt::Func(_) => Ok(false),
            Stmt::Var(v) => {
                for b in &v.bindings {
                    // Per binding: `,` extends, and each binding carries its own shape.
                    let native = native_base_shaped(&v.ty, b.shape.as_ref())?;
                    // `[n digits]` says how much of an irrational this declaration
                    // wants; it applies to the value being computed, then resets.
                    self.digits = match b.precision.as_ref().or(v.ty.precision.as_ref()) {
                        Some(Precision::Digits(n)) => *n,
                        _ => DEFAULT_DIGITS,
                    };
                    // A shape on the binding is the declared size; the literal supplies
                    // the elements, and the checker has already cross-checked the two.
                    let Some(value) = &b.value else {
                        // Unreachable for a checked program: `var:int 'x'.` is
                        // AHPCL-TYPE-0005. Kept because this backend can be handed an
                        // unchecked tree.
                        return Err(Unsupported::new("a declaration with no value"));
                    };
                    let val = self.expr(value, native)?;
                    // The slot takes the *declared* representation. Taking the value's
                    // type instead meant a mismatch stored a pointer into an i128 slot
                    // — valid IR under opaque pointers, so the verifier passed and the
                    // later load read the pointer plus eight bytes of neighbouring
                    // stack. A mismatch here is a bug in this compiler.
                    let want = self.repr_type(native);
                    assert_eq!(
                        val.get_type(),
                        want,
                        "'{}' is declared as {native:?}, so its slot is {want:?}, but the \
                         value built for it is {:?}",
                        b.name,
                        val.get_type()
                    );
                    let slot = self.alloca(&b.name, want);
                    if native.is_counted() {
                        let handle = self.counted_handle(val, native);
                        self.call_runtime(native.retain_fn(), &[handle.into()]);
                    }
                    self.builder.build_store(slot, val).unwrap();
                    self.vars.last_mut().unwrap().insert(b.name.clone(), slot);
                    self.var_types.last_mut().unwrap().insert(b.name.clone(), native);
                    self.digits = DEFAULT_DIGITS;
                }
                Ok(false)
            }
            Stmt::Change(c) => {
                let native = native_base(&c.ty)?;
                for target in &c.targets {
                    let Some(slot) = self.lookup(&target.name) else {
                        return Err(Unsupported::new(format!("changing unknown '{}'", target.name)));
                    };
                    let held = self
                        .var_types
                        .iter()
                        .rev()
                        .find_map(|f| f.get(&target.name).copied())
                        .unwrap_or(native);

                    // A selector on the left writes into the array rather than
                    // replacing it.
                    if !target.selectors.is_empty() {
                        if !held.is_array() {
                            return Err(Unsupported::new("a selector on a value that is not an array"));
                        }
                        // One single index per dimension, in order. Several indices in
                        // one selector would write to several cells at once, which is a
                        // different statement.
                        let mut chain = Vec::new();
                        for sel in &target.selectors {
                            let Selector::Indices(ix) = sel else {
                                return Err(Unsupported::new("this form of element assignment"));
                            };
                            let [index] = &ix[..] else {
                                return Err(Unsupported::new("writing to several elements at once"));
                            };
                            chain.push(index);
                        }
                        let elem = held.element();
                        let array = self.builder.build_load(self.ptr(), slot, "arr").unwrap();

                        let i = if chain.len() == 1 {
                            self.expr(chain[0], Native::Int)?
                        } else {
                            // Row-major arithmetic belongs with the shape, which only
                            // the runtime knows.
                            let ty = self.int_type();
                            let buffer = self.alloca_array("wpicks", ty.into(), chain.len());
                            for (k, e) in chain.iter().enumerate() {
                                let v = self.expr(e, Native::Int)?;
                                let at = unsafe {
                                    self.builder
                                        .build_gep(ty, buffer, &[ty.const_int(k as u64, false)], "w")
                                        .unwrap()
                                };
                                self.builder.build_store(at, v).unwrap();
                            }
                            let n = ty.const_int(chain.len() as u64, false);
                            self.call_runtime(
                                "ahpcl_array_offset",
                                &[array.into(), buffer.into(), n.into()],
                            )
                            .ok_or_else(|| Unsupported::new("addressing an element"))?
                        };
                        let val = self.expr(&target.value, elem)?;
                        self.array_store_at(array, i, val, elem);
                        continue;
                    }

                    let val = self.expr(&target.value, held)?;
                    if held.is_counted() {
                        // Retain the new value *before* releasing the old, so that
                        // `change:var:vector:int 'a' = ('a'):all;` — or any assignment
                        // where the two are the same value — does not free what it is
                        // about to store.
                        let ty = self.repr_type(held);
                        let old = self.builder.build_load(ty, slot, "old").unwrap();
                        let new_handle = self.counted_handle(val, held);
                        let old_handle = self.counted_handle(old, held);
                        self.call_runtime(held.retain_fn(), &[new_handle.into()]);
                        self.builder.build_store(slot, val).unwrap();
                        self.call_runtime(held.release_fn(), &[old_handle.into()]);
                    } else {
                        self.builder.build_store(slot, val).unwrap();
                    }
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
                // Everything this statement builds has to be released *here*, before the
                // branch: once it has jumped there is no block left to put a release in,
                // and the previous approach — forgetting them — leaked once per
                // iteration of any loop containing a `handback`.
                let mark = self.temporaries.len();
                // Inside a conditional or loop used as a value, `handback` contributes
                // to that value rather than leaving the function.
                match self.handbacks.last().copied() {
                    Some(Handback::Store(slot, repr, merge)) => {
                        let v = self.expr(value, repr)?;
                        // The slot outlives this statement, so the value escapes.
                        if repr.is_counted() {
                            let handle = self.counted_handle(v, repr);
                            self.call_runtime(repr.retain_fn(), &[handle.into()]);
                        }
                        self.builder.build_store(slot, v).unwrap();
                        self.release_temporaries(mark);
                        self.release_scope();
                        self.builder.build_unconditional_branch(merge).unwrap();
                        return Ok(true);
                    }
                    Some(Handback::Push(array, elem)) => {
                        let v = self.expr(value, elem)?;
                        // A row rather than a single value: append its elements and let
                        // the parent's shape grow a dimension.
                        if elem.is_array() {
                            self.call_runtime("ahpcl_array_push_array", &[array.into(), v.into()]);
                            // The push copies the elements, so nothing escapes.
                            self.release_temporaries(mark);
                            self.release_scope();
                            if let Some(step) = self.continues.last().copied() {
                                self.builder.build_unconditional_branch(step).unwrap();
                                return Ok(true);
                            }
                            return Ok(false);
                        }
                        let name = match elem {
                            Native::Bool => "ahpcl_array_push_bool",
                            Native::Deci => "ahpcl_array_push_deci",
                            Native::Rat => "ahpcl_array_push_rat",
                            Native::Str => "ahpcl_array_push_str",
                            Native::Num => "ahpcl_array_push_num",
                            _ => "ahpcl_array_push_int",
                        };
                        match elem {
                            Native::Deci | Native::Rat | Native::Str => {
                                let p = self.spill(v, "push");
                                self.call_runtime(name, &[array.into(), p.into()]);
                            }
                            Native::Bool => {
                                let byte = self
                                    .builder
                                    .build_int_z_extend(
                                        v.into_int_value(),
                                        self.context.i8_type(),
                                        "push.b",
                                    )
                                    .unwrap();
                                self.call_runtime(name, &[array.into(), byte.into()]);
                            }
                            _ => {
                                self.call_runtime(name, &[array.into(), v.into()]);
                            }
                        }
                        // Every `push` copies the value into the collecting array, so
                        // the temporary is this statement's to release as usual.
                        self.release_temporaries(mark);
                        self.release_scope();
                        // `handback` hands the value to whatever collects it and ends
                        // the unit that produced it — one iteration here, the whole
                        // call in a function. Falling through instead would run the
                        // rest of the body, which is not what the word means.
                        if let Some(step) = self.continues.last().copied() {
                            self.builder.build_unconditional_branch(step).unwrap();
                            return Ok(true);
                        }
                        return Ok(false);
                    }
                    None => {}
                }
                let function = self.current.expect("inside a function");
                let ret = function.get_type().get_return_type();
                match ret {
                    None => {
                        self.release_temporaries(mark);
                        self.release_all_scopes();
                        self.builder.build_return(None).unwrap();
                    }
                    Some(_) => {
                        let native = self.current_ret;
                        let v = self.expr(value, native)?;
                        // Returned, so the caller owns it and it must outlive the frames
                        // released below.
                        if native.is_counted() {
                            let handle = self.counted_handle(v, native);
                            self.call_runtime(native.retain_fn(), &[handle.into()]);
                        }
                        self.release_temporaries(mark);
                        self.release_all_scopes();
                        self.builder.build_return(Some(&v)).unwrap();
                    }
                }
                Ok(true)
            }
            Stmt::Expr(e) => {
                // A call performed for its effect hands nothing back, so it must not go
                // through the value path.
                if let ExprKind::Call { name, .. } = &e.kind {
                    if self.fn_repr.get(name).copied() == Some(Native::None) {
                        self.void_call(e)?;
                        return Ok(false);
                    }
                }
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
                _ if self.value_repr(arg) == Native::Num => {
                    let v = self.expr(arg, Native::Num)?;
                    self.call_runtime("ahpcl_print_num", &[v.into()]);
                }
                _ if self.value_repr(arg).is_array() => {
                    let v = self.expr(arg, self.value_repr(arg))?;
                    self.call_runtime("ahpcl_print_array", &[v.into()]);
                }
                _ if self.value_repr(arg) == Native::Str => {
                    let v = self.expr(arg, Native::Str)?;
                    let p = self.spill(v, "print.text");
                    self.call_runtime("ahpcl_print_text", &[p.into()]);
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
                        Native::Rat => {
                            let p = self.spill(v, "print.rat");
                            self.call_runtime("ahpcl_print_rat", &[p.into()]);
                        }
                        Native::Bool => {
                            let widened = self
                                .builder
                                .build_int_z_extend(
                                    v.into_int_value(),
                                    self.context.i8_type(),
                                    "boolbyte",
                                )
                                .unwrap();
                            self.call_runtime("ahpcl_print_bool", &[widened.into()]);
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
            ExprKind::Ref { name, selectors } => {
                let held = self
                    .var_types
                    .iter()
                    .rev()
                    .find_map(|f| f.get(name).copied())
                    .unwrap_or(Native::Int);
                selector_result(held, selectors)
            }
            ExprKind::Math(inner) => self.value_repr(inner),
            ExprKind::Number(t) | ExprKind::Literal(t) if t.contains('.') => Native::Deci,
            ExprKind::Constant(_) => Native::Deci,
            ExprKind::Unary { op, operand } if self.value_repr(operand).is_array()
                && !is_bare_ref(operand)
                && *op != UnOp::Not =>
            {
                self.value_repr(operand)
            }
            ExprKind::Unary { op, operand } => match op {
                // A square root is a decimal even from a whole number; rounding gives
                // a whole number whatever went in; `not` is a bool.
                UnOp::Sqrt | UnOp::Sin | UnOp::Cos | UnOp::Tan | UnOp::Log | UnOp::Ln => {
                    Native::Deci
                }
                UnOp::Floor | UnOp::Ceil => Native::Int,
                UnOp::Not => Native::Bool,
                _ => self.value_repr(operand),
            },
            ExprKind::Str(_) => Native::Str,
            ExprKind::Builtin { name, .. } if name == "read" => Native::Str,
            ExprKind::ArrayLit(items) => Native::Array(0, literal_shape(items).len() as u32),
            ExprKind::Loop(_) => Native::Array(5, 1),
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.value_repr(lhs), self.value_repr(rhs));
                // A dot product of two vectors collapses to a single value.
                if *op == BinOp::Dot && l.is_array() && r.is_array() {
                    return l.element();
                }
                // Rule A: a bare array reference sums, so the result is a num.
                let implies_all = matches!(
                    op,
                    BinOp::Dot | BinOp::Cross | BinOp::Hadamard | BinOp::Tensor
                );
                if !implies_all
                    && ((is_bare_ref(lhs) && l.is_array()) || (is_bare_ref(rhs) && r.is_array()))
                {
                    return Native::Num;
                }
                if l.is_array() || r.is_array() {
                    return if l.is_array() { l } else { r };
                }
                if l == Native::Num || r == Native::Num {
                    Native::Num
                } else if l == Native::Rat || r == Native::Rat {
                    Native::Rat
                } else if l == Native::Deci || r == Native::Deci {
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

                // A bound must be a whole number. Anything else — an array, say — is
                // for the interpreter to diagnose; calling `into_int_value` on it panics
                // the compiler instead.
                let whole = |v: BasicValueEnum<'ctx>| {
                    v.is_int_value()
                        .then(|| v.into_int_value())
                        .ok_or_else(|| Unsupported::new("a loop bound that is not a whole number"))
                };
                let start = whole(self.expr(from, Native::Int)?)?;
                let end = whole(self.expr(to, Native::Int)?)?;
                let step = match by {
                    Some(b) => whole(self.expr(b, Native::Int)?)?,
                    None => self.int_type().const_int(1, true),
                };

                // A step of 0 never advances, so the loop would never finish. The
                // interpreter refuses it; silently running zero times would be worse
                // than either.
                let zero_step = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, step, self.int_type().const_zero(), "step0")
                    .unwrap();
                let bad_bb = self.context.append_basic_block(function, "loop.badstep");
                let ok_bb = self.context.append_basic_block(function, "loop.step.ok");
                self.builder
                    .build_conditional_branch(zero_step, bad_bb, ok_bb)
                    .unwrap();
                self.builder.position_at_end(bad_bb);
                self.fail("AHPCL-RUN-0001", "a loop step of 0 would never finish");
                self.builder.position_at_end(ok_bb);

                let slot = self.alloca(var, self.int_type().into());
                self.builder.build_store(slot, start).unwrap();

                let cond_bb = self.context.append_basic_block(function, "loop.cond");
                let body_bb = self.context.append_basic_block(function, "loop.body");
                let done_bb = self.context.append_basic_block(function, "loop.done");

                self.builder.build_unconditional_branch(cond_bb).unwrap();
                self.builder.position_at_end(cond_bb);
                let i = self
                    .builder
                    .build_load(self.int_type(), slot, "i")
                    .unwrap()
                    .into_int_value();
                // Counting up while i <= end. A negative step counts down instead.
                let up = self
                    .builder
                    .build_int_compare(IntPredicate::SGT, step, self.int_type().const_zero(), "up")
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

                // `handback` ends the iteration, so it needs somewhere to jump that
                // still advances the counter. Branching straight to the condition would
                // skip the increment and loop forever.
                let step_bb = self.context.append_basic_block(function, "loop.step");

                self.builder.position_at_end(body_bb);
                // Both frames, always together. Pushing `vars` alone left the counter
                // with no recorded representation — so it was read as whatever the
                // context wanted — and let declarations inside the body leak into the
                // enclosing scope.
                self.vars.push(HashMap::new());
                self.var_types.push(HashMap::new());
                self.vars.last_mut().unwrap().insert(var.clone(), slot);
                self.var_types.last_mut().unwrap().insert(var.clone(), Native::Int);
                self.continues.push(step_bb);
                let terminated = self.block(&l.body);
                self.continues.pop();
                // The body is a scope, and it runs again and again: an array declared
                // inside it must be released at the end of *each* iteration, or the
                // loop grows without bound. The counted loop manages its frames by hand
                // rather than through `scoped`, so this is easy to forget — and was.
                self.release_scope();
                self.vars.pop();
                self.var_types.pop();
                if !terminated? {
                    self.builder.build_unconditional_branch(step_bb).unwrap();
                }

                self.builder.position_at_end(step_bb);
                let cur = self
                    .builder
                    .build_load(self.int_type(), slot, "i")
                    .unwrap()
                    .into_int_value();
                let next = self.builder.build_int_add(cur, step, "next").unwrap();
                self.builder.build_store(slot, next).unwrap();
                self.builder.build_unconditional_branch(cond_bb).unwrap();

                self.builder.position_at_end(done_bb);
            }
            LoopKind::While { condition } => {
                let cond_bb = self.context.append_basic_block(function, "while.cond");
                let body_bb = self.context.append_basic_block(function, "while.body");
                let done_bb = self.context.append_basic_block(function, "while.done");

                self.builder.build_unconditional_branch(cond_bb).unwrap();
                self.builder.position_at_end(cond_bb);
                // The condition is re-evaluated every iteration, so whatever it builds
                // must be released every iteration too. Left to the enclosing statement,
                // the release landed once in `done_bb` after the loop had finished.
                let mark = self.temporaries.len();
                let c = self.expr(condition, Native::Bool)?;
                self.release_temporaries(mark);
                self.builder
                    .build_conditional_branch(c.into_int_value(), body_bb, done_bb)
                    .unwrap();

                self.builder.position_at_end(body_bb);
                self.continues.push(cond_bb);
                let terminated = self.scoped(&l.body);
                self.continues.pop();
                if !terminated? {
                    self.builder.build_unconditional_branch(cond_bb).unwrap();
                }
                self.builder.position_at_end(done_bb);
            }
        }
        Ok(())
    }

    fn expr(&mut self, e: &Expr, want: Native) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        self.expr_inner(e, want)
    }

    fn expr_inner(&mut self, e: &Expr, want: Native) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        match &e.kind {
            ExprKind::Math(inner) => self.expr(inner, want),
            ExprKind::Number(text) | ExprKind::Literal(text) => {
                if text == "true" {
                    return Ok(self.bool_type().const_int(1, false).into());
                }
                if text == "false" {
                    return Ok(self.bool_type().const_zero().into());
                }
                // A `num` holds a tagged value, so a literal has to be boxed as
                // whichever exact kind it is written as.
                if want == Native::Num {
                    let held = if text.contains('.') { Native::Deci } else { Native::Int };
                    let v = self.expr(e, held)?;
                    return self.convert(v, held, Native::Num);
                }
                if want == Native::Rat {
                    let (mantissa, scale) = decimal_parts(text)
                        .ok_or_else(|| Unsupported::new(format!("the literal '{text}'")))?;
                    let den = 10i128.checked_pow(scale)
                        .ok_or_else(|| Unsupported::new("a rational literal that large"))?;
                    return Ok(self.rat_const(mantissa, den));
                }
                if text.contains('.') || want == Native::Deci {
                    let (mantissa, scale) = decimal_parts(text)
                        .ok_or_else(|| Unsupported::new(format!("the literal '{text}'")))?;
                    return Ok(self.deci_const(mantissa, scale));
                }
                // The whole `i128` range, not just what fits in a machine word: an
                // `int` is 128 bits, so a literal that fits the type must compile.
                let n: i128 = text
                    .parse()
                    .map_err(|_| Unsupported::new(format!("the literal '{text}'")))?;
                Ok(self.int_const(n))
            }
            ExprKind::Ref { name, selectors } => {
                let Some(slot) = self.lookup(name) else {
                    return Err(Unsupported::new(format!("the variable '{name}'")));
                };
                let held = self
                    .var_types
                    .iter()
                    .rev()
                    .find_map(|f| f.get(name).copied())
                    .unwrap_or(want);
                let loaded = self
                    .builder
                    .build_load(self.repr_type(held), slot, name)
                    .unwrap();
                if held.is_array() {
                    let out =
                        self.array_selectors(loaded, selectors, held.element(), want, held.rank())?;
                    // Rule A again: a *bare* reference reduces to the sum of its
                    // elements wherever a single value is wanted, not only inside a
                    // binary operator. `:all;` keeps it an array, so it is left alone.
                    if selectors.is_empty() && !want.is_array() && want != Native::None {
                        let total = self
                            .call_runtime("ahpcl_array_sum", &[out.into()])
                            .ok_or_else(|| Unsupported::new("summing an array"))?;
                        return self.convert(total, Native::Num, want);
                    }
                    return Ok(out);
                }
                if !selectors.is_empty() {
                    return Err(Unsupported::new("selectors on a value that is not an array"));
                }
                // An int flowing into an exact context is widened by the runtime.
                if want == Native::Deci && held == Native::Int {
                    return self
                        .call_deci("ahpcl_deci_from_int", &[loaded.into()])
                        .ok_or_else(|| Unsupported::new("widening an int to a decimal"));
                }
                if want == Native::Rat && held == Native::Int {
                    return self
                        .call_rat("ahpcl_rat_from_int", &[loaded.into()])
                        .ok_or_else(|| Unsupported::new("widening an int to a rational"));
                }
                if want != held {
                    return self.convert(loaded, held, want);
                }
                Ok(loaded)
            }
            ExprKind::Unary { op, operand } => match op {
                // Exact values have no machine negate or square root, so these go
                // through the runtime — the same code the interpreter's results come
                // from, so the digits agree rather than merely being close.
                UnOp::Not => {
                    let v = self.expr(operand, Native::Bool)?.into_int_value();
                    Ok(self.builder.build_not(v, "not").unwrap().into())
                }
                _ => self.unary_exact(*op, operand, want),
            },
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, want),
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
                    let _ = p;
                    let want = self
                        .fn_params
                        .get(name)
                        .and_then(|ps| ps.get(i).copied())
                        .unwrap_or(Native::Int);
                    values.push(self.expr(a, want)?.into());
                }
                let call = self.builder.build_call(function, &values, "call").unwrap();
                match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => {
                        let held = self.fn_repr.get(name).copied().unwrap_or(Native::Int);
                        if held == want {
                            Ok(v)
                        } else {
                            self.convert(v, held, want)
                        }
                    }
                    _ => Err(Unsupported::new("using a none-producing call as a value")),
                }
            }
            ExprKind::Builtin { name, args } => self.builtin(name, args, want),
            ExprKind::ArrayLit(items) => self.array_literal(items, want),
            ExprKind::If(chain) => self.if_value(chain, want),
            ExprKind::Loop(l) => self.loop_value(l, want),
            ExprKind::Constant(c) => {
                let which = match c {
                    Constant::Pi => 0u64,
                    Constant::E => 1,
                    Constant::Tau => 2,
                };
                // Both the constant and the digit count are known now, so asking for
                // more places than AHPCL knows is caught here rather than at run time.
                const CONSTANT_DIGITS: u32 = 36;
                if self.digits > CONSTANT_DIGITS {
                    return Err(Unsupported::new(format!(
                        "a constant to {} places, which is more than the {CONSTANT_DIGITS} AHPCL knows",
                        self.digits
                    )));
                }
                let which = self.context.i32_type().const_int(which, false);
                // The declared precision decides how much of an irrational to compute;
                // without one, the same 15 places everything else defaults to.
                let digits = self.context.i32_type().const_int(self.digits as u64, false);
                let v = self
                    .call_deci("ahpcl_constant", &[which.into(), digits.into()])
                    .ok_or_else(|| Unsupported::new("a constant"))?;
                self.convert(v, Native::Deci, want)
            }
            ExprKind::Str(text) => Ok(self.str_value(text)),
            ExprKind::Range { .. } => Err(Unsupported::new("a range outside a loop")),
            ExprKind::Option { .. } => Err(Unsupported::new("builtin options")),
        }
    }

    /// A conditional used for its value: each arm hands back into one slot, and every
    /// path meets at a single merge point.
    fn if_value(
        &mut self,
        chain: &IfChain,
        want: Native,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        let function = self.current.expect("inside a function");
        let ty = self.repr_type(want);
        let slot = self.alloca("ifval", ty);
        let merge = self.context.append_basic_block(function, "ifval.merge");

        for arm in &chain.arms {
            let body = self.context.append_basic_block(function, "ifval.arm");
            let next = self.context.append_basic_block(function, "ifval.next");
            match &arm.condition {
                Some(c) => {
                    let cond = self.expr(c, Native::Bool)?.into_int_value();
                    self.builder.build_conditional_branch(cond, body, next).unwrap();
                }
                None => {
                    self.builder.build_unconditional_branch(body).unwrap();
                }
            }
            self.builder.position_at_end(body);
            self.handbacks.push(Handback::Store(slot, want, merge));
            let terminated = self.scoped(&arm.body);
            self.handbacks.pop();
            if !terminated? {
                self.builder.build_unconditional_branch(merge).unwrap();
            }
            self.builder.position_at_end(next);
        }
        // An `if` with no matching arm still has to reach the merge point.
        self.builder.build_unconditional_branch(merge).unwrap();
        self.builder.position_at_end(merge);
        Ok(self.builder.build_load(ty, slot, "ifval.out").unwrap())
    }

    /// A loop used for its value: each `handback` contributes one element.
    fn loop_value(
        &mut self,
        l: &LoopStmt,
        want: Native,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        // One dimension per loop: a nested comprehension's outer loop hands back rows.
        let elem = if want.is_array() { want.peel() } else { Native::Num };
        let kind = self.context.i32_type().const_int(elem.kind_tag() as u64, false);
        let array = self
            .call_runtime("ahpcl_array_empty", &[kind.into()])
            .ok_or_else(|| Unsupported::new("collecting a loop's handbacks"))?;
        self.handbacks.push(Handback::Push(array, elem));
        let out = self.loop_stmt(l);
        self.handbacks.pop();
        out?;
        Ok(array)
    }

    /// `-x`, `|x|`, `sqrt x`, `floor` and `ceil`, on whichever representation the
    /// operand actually has.
    fn unary_exact(
        &mut self,
        op: UnOp,
        operand: &Expr,
        want: Native,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        let tag = match op {
            UnOp::Neg => 0u64,
            UnOp::Abs => 1,
            UnOp::Sqrt => {
                // Matches the interpreter: more places than AHPCL computes is an error.
                const SQRT_MAX_DIGITS: u32 = 18;
                if self.digits > SQRT_MAX_DIGITS {
                    return Err(Unsupported::new(format!(
                        "a square root to {} places, which is more than the {SQRT_MAX_DIGITS} \
                         AHPCL computes",
                        self.digits
                    )));
                }
                2
            }
            UnOp::Floor => 3,
            UnOp::Ceil => 4,
            UnOp::Sin => 5,
            UnOp::Cos => 6,
            UnOp::Tan => 7,
            UnOp::Log => 8,
            UnOp::Ln => 9,
            other => return Err(Unsupported::new(format!("the operator {other:?}"))),
        };
        // A square root is a decimal even when its operand is a whole number, and
        // floor and ceil hand back a whole number whatever went in.
        let held = self.value_repr(operand);
        let natural = match op {
            UnOp::Sqrt | UnOp::Sin | UnOp::Cos | UnOp::Tan | UnOp::Log | UnOp::Ln => Native::Deci,
            UnOp::Floor | UnOp::Ceil => Native::Int,
            _ => held,
        };

        // An array operand with a selector stays an array, so the operator applies to
        // each element — the unary half of Rule A.
        if held.is_array() && !is_bare_ref(operand) {
            let array = self.expr(operand, held)?;
            let tag = self.context.i32_type().const_int(tag, false);
            let digits = self.context.i32_type().const_int(self.digits as u64, false);
            return self
                .call_runtime("ahpcl_array_unary", &[tag.into(), array.into(), digits.into()])
                .ok_or_else(|| Unsupported::new("an elementwise unary operator"));
        }

        // Integers keep their machine instructions where the result is still an integer.
        if natural == Native::Int && held == Native::Int && matches!(op, UnOp::Neg | UnOp::Abs) {
            let v = self.expr(operand, Native::Int)?.into_int_value();
            let neg = self.builder.build_int_neg(v, "neg").unwrap();
            if op == UnOp::Neg {
                return self.convert(neg.into(), Native::Int, want);
            }
            let is_neg = self
                .builder
                .build_int_compare(IntPredicate::SLT, v, self.int_type().const_zero(), "isneg")
                .unwrap();
            let abs = self.builder.build_select(is_neg, neg, v, "abs").unwrap();
            return self.convert(abs, Native::Int, want);
        }

        let tag = self.context.i32_type().const_int(tag, false);
        let digits = self.context.i32_type().const_int(self.digits as u64, false);

        // A rational stays rational under negation and absolute value; everything else
        // goes through the tagged `num` path, which handles any kind.
        if held == Native::Rat && matches!(op, UnOp::Neg | UnOp::Abs) {
            let v = self.expr(operand, Native::Rat)?;
            let p = self.spill(v, "un.rat");
            let out = self
                .call_rat("ahpcl_rat_unary", &[tag.into(), p.into(), digits.into()])
                .ok_or_else(|| Unsupported::new("this operator on a rational"))?;
            return self.convert(out, Native::Rat, want);
        }
        if held == Native::Deci && matches!(op, UnOp::Neg | UnOp::Abs | UnOp::Sqrt) {
            let v = self.expr(operand, Native::Deci)?;
            let p = self.spill(v, "un.deci");
            let out = self
                .call_deci("ahpcl_deci_unary", &[tag.into(), p.into(), digits.into()])
                .ok_or_else(|| Unsupported::new("this operator on a decimal"))?;
            return self.convert(out, Native::Deci, want);
        }

        let v = self.expr(operand, Native::Num)?;
        let out = self
            .call_runtime("ahpcl_num_unary", &[tag.into(), v.into(), digits.into()])
            .ok_or_else(|| Unsupported::new("this operator"))?;
        self.convert(out, Native::Num, want)
    }

    /// A call to a function that hands nothing back, performed for its effect.
    fn void_call(&mut self, e: &Expr) -> Result<(), Unsupported> {
        let ExprKind::Call { name, args } = &e.kind else {
            return Err(Unsupported::new("a call"));
        };
        let Some(function) = self.functions.get(name).copied() else {
            return Err(Unsupported::new(format!("the function '{name}'")));
        };
        let mut values: Vec<BasicMetadataValueEnum> = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let want = self
                .fn_params
                .get(name)
                .and_then(|ps| ps.get(i).copied())
                .unwrap_or(Native::Int);
            values.push(self.expr(a, want)?.into());
        }
        self.builder.build_call(function, &values, "").unwrap();
        Ok(())
    }

    /// Move a value between native representations, through the runtime so the result
    /// is exactly what the interpreter would have produced.
    fn convert(
        &mut self,
        v: BasicValueEnum<'ctx>,
        from: Native,
        to: Native,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        if from == to {
            return Ok(v);
        }
        match (from, to) {
            // Anything exact can be boxed into a `num`.
            (Native::Int, Native::Num) => self
                .call_runtime("ahpcl_num_from_int", &[v.into()])
                .ok_or_else(|| Unsupported::new("boxing an int as a num")),
            (Native::Bool, Native::Num) => {
                let byte = self
                    .builder
                    .build_int_z_extend(v.into_int_value(), self.context.i8_type(), "numbool")
                    .unwrap();
                self.call_runtime("ahpcl_num_from_bool", &[byte.into()])
                    .ok_or_else(|| Unsupported::new("boxing a bool as a num"))
            }
            (Native::Deci, Native::Num) => {
                let p = self.spill(v, "numdeci");
                self.call_runtime("ahpcl_num_from_deci", &[p.into()])
                    .ok_or_else(|| Unsupported::new("boxing a decimal as a num"))
            }
            (Native::Rat, Native::Num) => {
                let p = self.spill(v, "numrat");
                self.call_runtime("ahpcl_num_from_rat", &[p.into()])
                    .ok_or_else(|| Unsupported::new("boxing a rational as a num"))
            }
            // …and unboxed again where a narrower type is pinned by context.
            (Native::Num, Native::Deci) => self
                .call_deci("ahpcl_num_to_deci", &[v.into()])
                .ok_or_else(|| Unsupported::new("reading a num as a decimal")),
            (Native::Num, Native::Rat) => self
                .call_rat("ahpcl_num_to_rat", &[v.into()])
                .ok_or_else(|| Unsupported::new("reading a num as a rational")),
            (Native::Num, Native::Int) => self
                .call_runtime("ahpcl_num_to_int", &[v.into()])
                .ok_or_else(|| Unsupported::new("reading a num as an int")),
            (Native::Int, Native::Deci) => self
                .call_deci("ahpcl_deci_from_int", &[v.into()])
                .ok_or_else(|| Unsupported::new("widening an int to a decimal")),
            (Native::Int, Native::Rat) => self
                .call_rat("ahpcl_rat_from_int", &[v.into()])
                .ok_or_else(|| Unsupported::new("widening an int to a rational")),
            // A bool is already a machine word where an int is wanted.
            (Native::Bool, Native::Int) => Ok(self
                .builder
                .build_int_z_extend(v.into_int_value(), self.int_type(), "boolint")
                .unwrap()
                .into()),
            _ => Err(Unsupported::new(format!("converting {from:?} to {to:?}"))),
        }
    }

    /// Arithmetic and comparison on `num`, dispatched by the runtime on the tag each
    /// side actually carries.
    fn num_binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        let a = self.expr(lhs, Native::Num)?;
        let b = self.expr(rhs, Native::Num)?;
        self.num_binary_values(op, a, b)
    }

    fn num_binary_values(
        &mut self,
        op: BinOp,
        a: BasicValueEnum<'ctx>,
        b: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        use BinOp::*;
        if let Eq | NotEq | Less | Greater | LessEq | GreaterEq = op {
            let cmp = self
                .call_runtime("ahpcl_num_cmp", &[a.into(), b.into()])
                .ok_or_else(|| Unsupported::new("num comparison"))?
                .into_int_value();
            let predicate = match op {
                Eq => IntPredicate::EQ,
                NotEq => IntPredicate::NE,
                Less => IntPredicate::SLT,
                Greater => IntPredicate::SGT,
                LessEq => IntPredicate::SLE,
                _ => IntPredicate::SGE,
            };
            let zero = self.context.i32_type().const_zero();
            return Ok(self
                .builder
                .build_int_compare(predicate, cmp, zero, "numcmp")
                .unwrap()
                .into());
        }

        let tag = match op {
            Add => 0,
            Sub => 1,
            Mul => 2,
            Div => 3,
            Pow => 4,
            IntDiv => 5,
            Mod => 6,
            other => return Err(Unsupported::new(format!("{other:?} on num"))),
        };
        let tag = self.context.i32_type().const_int(tag, false);
        self.call_runtime("ahpcl_num_binary", &[tag.into(), a.into(), b.into()])
            .ok_or_else(|| Unsupported::new("num arithmetic"))
    }

    // ── arrays ──────────────────────────────────────────────────────────────

    /// Allocate a runtime array of `len` elements holding `elem`.
    fn array_new(&mut self, elem: Native, dims: &[u64]) -> BasicValueEnum<'ctx> {
        let i64t = self.context.i64_type();
        let shape = self.alloca_array("shape", i64t.into(), dims.len().max(1));
        for (i, d) in dims.iter().enumerate() {
            let slot = unsafe {
                self.builder
                    .build_gep(i64t, shape, &[i64t.const_int(i as u64, false)], "dim")
                    .unwrap()
            };
            self.builder
                .build_store(slot, i64t.const_int(*d, false))
                .unwrap();
        }
        let kind = self.context.i32_type().const_int(elem.kind_tag() as u64, false);
        let rank = self.context.i32_type().const_int(dims.len() as u64, false);
        self.call_runtime("ahpcl_array_new", &[kind.into(), rank.into(), shape.into()])
            .expect("ahpcl_array_new hands back a pointer")
    }

    /// Write one element, choosing the setter that matches its representation.
    fn array_store(
        &mut self,
        array: BasicValueEnum<'ctx>,
        index: u64,
        value: BasicValueEnum<'ctx>,
        elem: Native,
    ) {
        let i = self.int_type().const_int(index, false);
        self.array_store_at(array, i.into(), value, elem);
    }

    /// The same, with an index only known at runtime.
    fn array_store_at(
        &mut self,
        array: BasicValueEnum<'ctx>,
        i: BasicValueEnum<'ctx>,
        value: BasicValueEnum<'ctx>,
        elem: Native,
    ) {
        match elem {
            Native::Deci | Native::Rat | Native::Str => {
                let name = match elem {
                    Native::Deci => "ahpcl_array_set_deci",
                    Native::Rat => "ahpcl_array_set_rat",
                    _ => "ahpcl_array_set_str",
                };
                let p = self.spill(value, "elem");
                self.call_runtime(name, &[array.into(), i.into(), p.into()]);
            }
            Native::Num => {
                self.call_runtime("ahpcl_array_set_num", &[array.into(), i.into(), value.into()]);
            }
            Native::Bool => {
                let byte = self
                    .builder
                    .build_int_z_extend(value.into_int_value(), self.context.i8_type(), "elem.b")
                    .unwrap();
                self.call_runtime("ahpcl_array_set_bool", &[array.into(), i.into(), byte.into()]);
            }
            _ => {
                self.call_runtime("ahpcl_array_set_int", &[array.into(), i.into(), value.into()]);
            }
        }
    }

    /// Read one element back out.
    fn array_load(
        &mut self,
        array: BasicValueEnum<'ctx>,
        index: BasicValueEnum<'ctx>,
        elem: Native,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        match elem {
            Native::Deci => self
                .call_deci("ahpcl_array_get_deci", &[array.into(), index.into()])
                .ok_or_else(|| Unsupported::new("reading a decimal element")),
            Native::Rat => self
                .call_rat("ahpcl_array_get_rat", &[array.into(), index.into()])
                .ok_or_else(|| Unsupported::new("reading a rational element")),
            Native::Str => {
                let out = self.alloca("elem.str", self.str_type().into());
                self.call_runtime("ahpcl_array_get_str", &[out.into(), array.into(), index.into()]);
                let v = self.builder.build_load(self.str_type(), out, "elem.sval").unwrap();
                // Handed back through an out-pointer rather than returned, so the
                // ownership hook on `call_runtime` cannot see it. Record it here.
                self.temporaries.push((v, Native::Str));
                Ok(v)
            }
            Native::Num => self
                .call_runtime("ahpcl_array_get_num", &[array.into(), index.into()])
                .ok_or_else(|| Unsupported::new("reading a num element")),
            Native::Bool => {
                let byte = self
                    .call_runtime("ahpcl_array_get_bool", &[array.into(), index.into()])
                    .ok_or_else(|| Unsupported::new("reading a bool element"))?
                    .into_int_value();
                let zero = self.context.i8_type().const_zero();
                Ok(self
                    .builder
                    .build_int_compare(IntPredicate::NE, byte, zero, "elem.bval")
                    .unwrap()
                    .into())
            }
            _ => self
                .call_runtime("ahpcl_array_get_int", &[array.into(), index.into()])
                .ok_or_else(|| Unsupported::new("reading an int element")),
        }
    }

    /// `{'1','2','3'}`, including the nested form that mirrors a shape.
    fn array_literal(
        &mut self,
        items: &[Expr],
        want: Native,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        let elem = if want.is_array() { want.element() } else { want };
        let flat = flatten_literal(items);
        let dims = literal_shape(items);
        let array = self.array_new(elem, &dims);
        for (i, item) in flat.iter().enumerate() {
            let v = self.expr(item, elem)?;
            // AHPCL indexes from 1, and so does the runtime.
            self.array_store(array, i as u64 + 1, v, elem);
        }
        Ok(array)
    }

    /// A selector chain applied to an array.
    ///
    /// Selectors group into *runs*: `:length;` and `:shape;` answer a question about the
    /// whole array and end the run, while everything else addresses one dimension of it,
    /// in order. Applying them to the flat element buffer instead is right for vectors
    /// and silently wrong for matrices and tensors.
    fn array_selectors(
        &mut self,
        mut current: BasicValueEnum<'ctx>,
        selectors: &[Selector],
        elem: Native,
        want: Native,
        rank: u32,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        let mut run: Vec<&Selector> = Vec::new();
        let mut rank = rank;

        for sel in selectors {
            match sel {
                Selector::Length => {
                    current = self.apply_run(current, &run)?;
                    run.clear();
                    let n = self
                        .call_runtime("ahpcl_array_len", &[current.into()])
                        .ok_or_else(|| Unsupported::new("':length;'"))?;
                    return self.convert(n, Native::Int, want);
                }
                Selector::Shape => {
                    current = self.apply_run(current, &run)?;
                    run.clear();
                    current = self
                        .call_runtime("ahpcl_array_shape", &[current.into()])
                        .ok_or_else(|| Unsupported::new("':shape;'"))?;
                    rank = 1;
                }
                other => run.push(other),
            }
        }

        // Every dimension addressed by a single index collapses; when that accounts for
        // the whole rank, what is left is one value rather than an array.
        let singles = run
            .iter()
            .filter(|s| matches!(s, Selector::Indices(ix) if ix.len() == 1))
            .count();
        let collapses = !run.is_empty() && singles == run.len() && singles as u32 == rank;

        // Reading one element does not need the run machinery at all. Going through it
        // built four descriptor arrays and allocated a whole new array — a `Vec` of
        // cells, a shape, and a box — to hold a single value, then read it back out.
        // That is ~120ns and one leaked object *per element read*: summing a million
        // elements took 3.2GB. Address the element directly instead.
        if collapses && !want.is_array() {
            let index = if run.len() == 1 {
                let Selector::Indices(ix) = run[0] else {
                    unreachable!("a collapsing run holds single indices")
                };
                self.expr(&ix[0], Native::Int)?
            } else {
                // Several dimensions: the runtime knows the shape, so it does the
                // row-major arithmetic — the same call element assignment uses.
                let ty = self.int_type();
                let buffer = self.alloca_array("read.ix", ty.into(), run.len());
                for (k, sel) in run.iter().enumerate() {
                    let Selector::Indices(ix) = sel else {
                        unreachable!("a collapsing run holds single indices")
                    };
                    let v = self.expr(&ix[0], Native::Int)?;
                    let at = unsafe {
                        self.builder
                            .build_gep(ty, buffer, &[ty.const_int(k as u64, false)], "at")
                            .unwrap()
                    };
                    self.builder.build_store(at, v).unwrap();
                }
                let n = ty.const_int(run.len() as u64, false);
                self.call_runtime(
                    "ahpcl_array_offset",
                    &[current.into(), buffer.into(), n.into()],
                )
                .ok_or_else(|| Unsupported::new("addressing an element"))?
            };
            let v = self.array_load(current, index, elem)?;
            return self.convert(v, elem, want);
        }

        let out = self.apply_run(current, &run)?;
        Ok(out)
    }

    /// One run of dimension selectors, described to the runtime as parallel arrays.
    ///
    /// Deliberately not one array of a shared struct: LLVM and Rust disagree about where
    /// an `i128` sits inside a struct, so the fields landed at different offsets on each
    /// side and every selector but `:all;` read garbage. Flat arrays of one primitive
    /// each have no layout to agree on.
    fn apply_run(
        &mut self,
        current: BasicValueEnum<'ctx>,
        run: &[&Selector],
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        if run.is_empty() {
            return Ok(current);
        }
        let n = run.len();
        let value = self.int_type();
        let u32t = self.context.i32_type();
        let u64t = self.context.i64_type();

        let kinds = self.alloca_array("sel.kinds", u32t.into(), n);
        let bounds = self.alloca_array("sel.bounds", value.into(), n * 3);
        let buffers = self.alloca_array("sel.bufs", self.ptr().into(), n);
        let counts = self.alloca_array("sel.counts", u64t.into(), n);

        let at = |cg: &Self, base, ty: inkwell::types::BasicTypeEnum<'ctx>, k: usize| unsafe {
            cg.builder
                .build_gep(ty, base, &[cg.context.i64_type().const_int(k as u64, false)], "at")
                .unwrap()
        };

        for (k, sel) in run.iter().enumerate() {
            let (kind, from, to, by, buffer, count) = match sel {
                Selector::All => (0u64, None, None, None, None, 0u64),
                Selector::Indices(ix) => {
                    let list = self.alloca_array("sel.ix", value.into(), ix.len());
                    for (j, e) in ix.iter().enumerate() {
                        let v = self.expr(e, Native::Int)?;
                        let slot = at(self, list, value.into(), j);
                        self.builder.build_store(slot, v).unwrap();
                    }
                    (1, None, None, None, Some(list), ix.len() as u64)
                }
                Selector::Range { from, to, by } => {
                    let f = self.expr(from, Native::Int)?;
                    let t = self.expr(to, Native::Int)?;
                    let b = match by {
                        Some(e) => self.expr(e, Native::Int)?,
                        None => value.const_int(1, true).into(),
                    };
                    (2, Some(f), Some(t), Some(b), None, 0)
                }
                // Flushed by the caller, so these never reach a run.
                Selector::Length | Selector::Shape => (0, None, None, None, None, 0),
            };

            let slot = at(self, kinds, u32t.into(), k);
            self.builder.build_store(slot, u32t.const_int(kind, false)).unwrap();

            let zero = value.const_zero().into();
            for (offset, v) in [from, to, by].into_iter().enumerate() {
                let slot = at(self, bounds, value.into(), k * 3 + offset);
                self.builder.build_store(slot, v.unwrap_or(zero)).unwrap();
            }

            let slot = at(self, buffers, self.ptr().into(), k);
            let p = buffer.unwrap_or_else(|| self.ptr().const_null());
            self.builder.build_store(slot, p).unwrap();

            let slot = at(self, counts, u64t.into(), k);
            self.builder.build_store(slot, u64t.const_int(count, false)).unwrap();
        }

        let n = value.const_int(n as u64, false);
        self.call_runtime(
            "ahpcl_array_select_run",
            &[
                current.into(),
                kinds.into(),
                bounds.into(),
                buffers.into(),
                counts.into(),
                n.into(),
            ],
        )
        .ok_or_else(|| Unsupported::new("a selector"))
    }

    /// Arithmetic where at least one side is a bare array reference, which Rule A
    /// reduces to the sum of its elements. The sum is a `num`, since the elements may
    /// be any exact kind, so the whole operation runs on the `num` path.
    fn reduced_binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        reduce_l: bool,
        reduce_r: bool,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        let a = self.side_as_num(lhs, reduce_l)?;
        let b = self.side_as_num(rhs, reduce_r)?;
        self.num_binary_values(op, a, b)
    }

    fn side_as_num(
        &mut self,
        e: &Expr,
        reduce: bool,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        if reduce {
            // Ask for the array itself, so the bare-reference reduction in `expr` does
            // not also fire and sum it twice.
            let array = self.expr(e, self.value_repr(e))?;
            return self
                .call_runtime("ahpcl_array_sum", &[array.into()])
                .ok_or_else(|| Unsupported::new("summing an array"));
        }
        self.expr(e, Native::Num)
    }

    /// The four array operators, plus elementwise arithmetic between arrays.
    fn array_binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        want: Native,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        use BinOp::*;
        let lr = self.value_repr(lhs);
        let rr = self.value_repr(rhs);
        // A comparison's result element is `bool`, but its *operands* are not: coercing
        // them to the wanted element turned `('a'):all; > 2` into a comparison against
        // `true`, read back as 1. Each side keeps its own kind, and the runtime promotes
        // across kinds when it compares.
        let operand_want = match op {
            Eq | NotEq | Less | Greater | LessEq | GreaterEq => Native::None,
            _ => want,
        };
        // A scalar beside an array is broadcast, so it becomes a one-element array.
        let a = self.as_array(lhs, lr, operand_want)?;
        let b = self.as_array(rhs, rr, operand_want)?;

        let name = match op {
            Dot => "ahpcl_array_dot",
            Cross => "ahpcl_array_cross",
            Hadamard => "ahpcl_array_hadamard",
            Tensor => "ahpcl_array_tensor",
            Add | Sub | Mul | Div => {
                let tag = match op {
                    Add => 0,
                    Sub => 1,
                    Mul => 2,
                    _ => 3,
                };
                let tag = self.context.i32_type().const_int(tag, false);
                return self
                    .call_runtime("ahpcl_array_elementwise", &[tag.into(), a.into(), b.into()])
                    .ok_or_else(|| Unsupported::new("elementwise arithmetic"));
            }
            // Comparison is elementwise too, handing back an array of bools.
            Eq | NotEq | Less | Greater | LessEq | GreaterEq => {
                let tag = match op {
                    Eq => 10u64,
                    NotEq => 11,
                    Less => 12,
                    Greater => 13,
                    LessEq => 14,
                    _ => 15,
                };
                let tag = self.context.i32_type().const_int(tag, false);
                return self
                    .call_runtime("ahpcl_array_compare", &[tag.into(), a.into(), b.into()])
                    .ok_or_else(|| Unsupported::new("elementwise comparison"));
            }
            other => return Err(Unsupported::new(format!("{other:?} on arrays"))),
        };
        let out = self
            .call_runtime(name, &[a.into(), b.into()])
            .ok_or_else(|| Unsupported::new("an array operator"))?;
        // A dot product of two vectors is a single value, handed back as a
        // one-element array; read it out when the context wants a scalar.
        if op == Dot && !want.is_array() {
            let one = self.int_type().const_int(1, false);
            return self.array_load(out, one.into(), want);
        }
        Ok(out)
    }

    /// Evaluate an operand as an array, wrapping a scalar in a one-element array so the
    /// runtime can broadcast it.
    fn as_array(
        &mut self,
        e: &Expr,
        repr: Native,
        want: Native,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        if repr.is_array() {
            return self.expr(e, repr);
        }
        let elem = if repr == Native::Int && want.is_array() { want.element() } else { repr };
        let v = self.expr(e, elem)?;
        let array = self.array_new(elem, &[1]);
        self.array_store(array, 1, v, elem);
        Ok(array)
    }

    /// `read` and `parse`, the two builtins that produce a value.
    fn builtin(
        &mut self,
        name: &str,
        args: &[Expr],
        want: Native,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        match name {
            "read" => {
                // `read["path"]` reads that whole file as text.
                let path = match args.first() {
                    Some(a) => self.expr(a, Native::Str)?,
                    None => self.str_value(""),
                };
                let path = self.spill(path, "read.path");
                let out = self.alloca("read.out", self.str_type().into());
                self.call_runtime("ahpcl_read_file", &[out.into(), path.into()]);
                let v = self.builder.build_load(self.str_type(), out, "read.val").unwrap();
                self.temporaries.push((v, Native::Str));
                Ok(v)
            }
            "parse" => {
                let first = args
                    .first()
                    .ok_or_else(|| Unsupported::new("parse without a value"))?;
                let text = self.expr(first, Native::Str)?;
                let text = self.spill(text, "parse.text");

                let mut flags = 0u64;
                let mut group = None;
                let mut decimal = None;
                for opt in &args[1..] {
                    let ExprKind::Option { name, value } = &opt.kind else {
                        return Err(Unsupported::new("a parse argument that is not an option"));
                    };
                    match name.as_str() {
                        "trim" => flags |= 1,
                        "scientific" => flags |= 2,
                        "hex" => flags |= 4,
                        "unicode-digits" => flags |= 8,
                        "fraction" => flags |= 16,
                        "group" | "decimal" => {
                            let Some(v) = value else {
                                return Err(Unsupported::new(format!("'{name}' without a value")));
                            };
                            let (ExprKind::Str(t) | ExprKind::Literal(t)) = &v.kind else {
                                return Err(Unsupported::new("a non-literal separator"));
                            };
                            let held = self.str_value(t);
                            let slot = self.spill(held, "parse.sep");
                            if name == "group" { group = Some(slot) } else { decimal = Some(slot) }
                        }
                        other => return Err(Unsupported::new(format!("the parse option '{other}'"))),
                    }
                }

                let null = self.context.ptr_type(inkwell::AddressSpace::default()).const_null();
                let g = group.map(|p| p.into()).unwrap_or(null);
                let d = decimal.map(|p| p.into()).unwrap_or(null);
                let flags = self.context.i64_type().const_int(flags, false);

                match want {
                    Native::Rat => self
                        .call_rat("ahpcl_parse_rat", &[text.into(), flags.into(), g.into(), d.into()])
                        .ok_or_else(|| Unsupported::new("parsing a rational")),
                    Native::Deci => self
                        .call_deci("ahpcl_parse_deci", &[text.into(), flags.into(), g.into(), d.into()])
                        .ok_or_else(|| Unsupported::new("parsing a decimal")),
                    _ => self
                        .call_runtime(
                            "ahpcl_parse_int",
                            &[text.into(), flags.into(), g.into(), d.into()],
                        )
                        .ok_or_else(|| Unsupported::new("parsing an int")),
                }
            }
            "clock" => self
                .call_deci("ahpcl_clock", &[])
                .ok_or_else(|| Unsupported::new("reading the clock")),
            other => Err(Unsupported::new(format!("the builtin '{other}'"))),
        }
    }

    fn binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        want: Native,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
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

        // Exact values have no machine instruction, so their arithmetic becomes a
        // runtime call.
        // The wanted type counts too: `1 / 3` is integer-looking but exact when the
        // context asks for a rational.
        let arithmetic = !matches!(
            op,
            Eq | NotEq | Less | Greater | LessEq | GreaterEq | And | Or
        );
        let reprs = [self.value_repr(lhs), self.value_repr(rhs)];

        // Rule A: a *bare* array reference sums its elements, while one carrying a
        // selector — `('a'):all;` — stays an array and the operation is elementwise.
        // The array operators are the exception: `· × ⊙ ⊗` imply `:all;`, having no
        // scalar meaning at all.
        let implies_all = matches!(op, Dot | Cross | Hadamard | Tensor);
        if !implies_all && (is_bare_ref(lhs) || is_bare_ref(rhs)) {
            let reduce_l = is_bare_ref(lhs) && reprs[0].is_array();
            let reduce_r = is_bare_ref(rhs) && reprs[1].is_array();
            if reduce_l || reduce_r {
                let out = self.reduced_binary(op, lhs, rhs, reduce_l, reduce_r)?;
                if arithmetic && want != Native::Num {
                    return self.convert(out, Native::Num, want);
                }
                return Ok(out);
            }
        }

        if implies_all || reprs.iter().any(|r| r.is_array()) {
            return self.array_binary(op, lhs, rhs, want);
        }
        if reprs.contains(&Native::Str) {
            return self.text_binary(op, lhs, rhs);
        }
        if reprs.contains(&Native::Num) || (arithmetic && want == Native::Num) {
            let out = self.num_binary(op, lhs, rhs)?;
            // A comparison is already a machine bool; arithmetic hands back a boxed
            // `num`, which the surrounding context may have pinned to a narrower type.
            if arithmetic && want != Native::Num {
                return self.convert(out, Native::Num, want);
            }
            return Ok(out);
        }
        if reprs.contains(&Native::Rat) || (arithmetic && want == Native::Rat) {
            return self.rational_binary(op, lhs, rhs);
        }
        if reprs.contains(&Native::Deci) || (arithmetic && want == Native::Deci) {
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
            // Overflow is an error in AHPCL, so these are checked. Plain `add`/`sub`/
            // `mul` wrap silently, which turns an overflow into a wrong answer.
            Add => self.checked_int(a, b, "sadd", "addition")?,
            Sub => self.checked_int(a, b, "ssub", "subtraction")?,
            Mul => self.checked_int(a, b, "smul", "multiplication")?,
            IntDiv | Mod => {
                let name = if op == IntDiv { "ahpcl_int_div" } else { "ahpcl_int_mod" };
                return self
                    .call_runtime(name, &[a.into(), b.into()])
                    .ok_or_else(|| Unsupported::new("integer division"));
            }
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
                let digits = self.context.i32_type().const_int(self.digits as u64, false);
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
            IntDiv => {
                return self
                    .call_runtime("ahpcl_deci_int_div", &[pa.into(), pb.into()])
                    .ok_or_else(|| Unsupported::new("integer division of decimals"));
            }
            Pow | Mod => {
                let tag = self.context.i32_type().const_int(if op == Pow { 4 } else { 6 }, false);
                return self
                    .call_deci("ahpcl_deci_binary", &[tag.into(), pa.into(), pb.into()])
                    .ok_or_else(|| Unsupported::new("this operator on decimals"));
            }
            other => return Err(Unsupported::new(format!("{other:?} on decimals"))),
        };
        self.call_deci(name, &[pa.into(), pb.into()])
            .ok_or_else(|| Unsupported::new("decimal arithmetic"))
    }

    /// Text supports ordering only; the runtime does the byte comparison.
    fn text_binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        use BinOp::*;
        let predicate = match op {
            Eq => IntPredicate::EQ,
            NotEq => IntPredicate::NE,
            Less => IntPredicate::SLT,
            Greater => IntPredicate::SGT,
            LessEq => IntPredicate::SLE,
            GreaterEq => IntPredicate::SGE,
            other => return Err(Unsupported::new(format!("{other:?} on text"))),
        };
        let a = self.expr(lhs, Native::Str)?;
        let b = self.expr(rhs, Native::Str)?;
        let pa = self.spill(a, "slhs");
        let pb = self.spill(b, "srhs");
        let cmp = self
            .call_runtime("ahpcl_str_cmp", &[pa.into(), pb.into()])
            .ok_or_else(|| Unsupported::new("text comparison"))?
            .into_int_value();
        let zero = self.context.i32_type().const_zero();
        Ok(self
            .builder
            .build_int_compare(predicate, cmp, zero, "strcmp")
            .unwrap()
            .into())
    }

    fn rational_binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, Unsupported> {
        use BinOp::*;
        let a = self.expr(lhs, Native::Rat)?;
        let b = self.expr(rhs, Native::Rat)?;
        let pa = self.spill(a, "rlhs");
        let pb = self.spill(b, "rrhs");

        let name = match op {
            Add => "ahpcl_rat_add",
            Sub => "ahpcl_rat_sub",
            Mul => "ahpcl_rat_mul",
            Div => "ahpcl_rat_div",
            Eq | NotEq | Less | Greater | LessEq | GreaterEq => {
                let cmp = self
                    .call_runtime("ahpcl_rat_cmp", &[pa.into(), pb.into()])
                    .ok_or_else(|| Unsupported::new("rational comparison"))?
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
                    .build_int_compare(predicate, cmp, zero, "ratcmp")
                    .unwrap()
                    .into());
            }
            Pow | IntDiv | Mod => {
                let tag = self.context.i32_type().const_int(
                    match op {
                        Pow => 4,
                        IntDiv => 5,
                        _ => 6,
                    },
                    false,
                );
                return self
                    .call_rat("ahpcl_rat_binary", &[tag.into(), pa.into(), pb.into()])
                    .ok_or_else(|| Unsupported::new("this operator on rationals"));
            }
            other => return Err(Unsupported::new(format!("{other:?} on rationals"))),
        };
        self.call_rat(name, &[pa.into(), pb.into()])
            .ok_or_else(|| Unsupported::new("rational arithmetic"))
    }

    /// Integer exponentiation by a loop, since LLVM has no integer power instruction.
    fn int_pow(&mut self, base: inkwell::values::IntValue<'ctx>, exp: inkwell::values::IntValue<'ctx>)
        -> Result<BasicValueEnum<'ctx>, Unsupported>
    {
        let function = self.current.expect("inside a function");
        let acc = self.alloca("pow.acc", self.int_type().into());
        let counter = self.alloca("pow.i", self.int_type().into());
        self.builder.build_store(acc, self.int_type().const_int(1, false)).unwrap();
        self.builder.build_store(counter, self.int_type().const_zero()).unwrap();

        let cond = self.context.append_basic_block(function, "pow.cond");
        let body = self.context.append_basic_block(function, "pow.body");
        let done = self.context.append_basic_block(function, "pow.done");

        self.builder.build_unconditional_branch(cond).unwrap();
        self.builder.position_at_end(cond);
        let i = self.builder.build_load(self.int_type(), counter, "i").unwrap().into_int_value();
        let more = self.builder.build_int_compare(IntPredicate::SLT, i, exp, "more").unwrap();
        self.builder.build_conditional_branch(more, body, done).unwrap();

        self.builder.position_at_end(body);
        let cur = self.builder.build_load(self.int_type(), acc, "acc").unwrap().into_int_value();
        // Checked, like `+`, `-` and `x` above: raising to a power is the one place
        // integer arithmetic used a raw multiply, so `10 xx 39` wrapped silently
        // instead of reporting that it overflowed.
        let next = self.checked_int(cur, base, "smul", "multiplication")?;
        self.builder.build_store(acc, next).unwrap();
        let i2 = self.builder.build_load(self.int_type(), counter, "i").unwrap().into_int_value();
        let i3 = self.builder.build_int_add(i2, self.int_type().const_int(1, false), "i.next").unwrap();
        self.builder.build_store(counter, i3).unwrap();
        self.builder.build_unconditional_branch(cond).unwrap();

        self.builder.position_at_end(done);
        Ok(self.builder.build_load(self.int_type(), acc, "pow").unwrap())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Native {
    Int,
    Bool,
    /// An exact decimal, held as the runtime's struct and operated on by calls.
    Deci,
    /// An exact rational: numerator and denominator, likewise by-pointer.
    Rat,
    /// Text: a pointer and a byte length.
    Str,
    /// `num`, the top of the numeric hierarchy: a tagged value holding whichever
    /// exact kind flowed into it, as an opaque pointer to a runtime object.
    Num,
    /// An array, as an opaque pointer to a runtime-managed object. The first tag says
    /// which element type it holds, matching the `KIND_*` constants in the runtime; the
    /// second is its rank, which decides whether a run of selectors collapses to a
    /// single value.
    Array(u32, u32),
    None,
}

impl Native {
    fn is_array(self) -> bool {
        matches!(self, Native::Array(..))
    }

    /// Whether the runtime owns this value on the heap, so it is counted.
    ///
    /// A `num` is boxed exactly like an array, and forgetting it left a loop growing by
    /// ~190 bytes an iteration even after arrays were counted.
    fn is_counted(self) -> bool {
        self.is_array() || matches!(self, Native::Num | Native::Str)
    }

    /// The runtime call that releases one reference to this kind of value.
    fn release_fn(self) -> &'static str {
        match self {
            Native::Str => "ahpcl_str_release",
            _ if self.is_array() => "ahpcl_array_release",
            _ => "ahpcl_num_release",
        }
    }

    fn retain_fn(self) -> &'static str {
        match self {
            Native::Str => "ahpcl_str_retain",
            _ if self.is_array() => "ahpcl_array_retain",
            _ => "ahpcl_num_retain",
        }
    }

    /// One step into an array: a rank above one yields an array of one less rank, and
    /// a vector yields its element. Mirrors `Type::peel` in the checker.
    fn peel(self) -> Native {
        match self {
            Native::Array(kind, r) if r > 1 => Native::Array(kind, r - 1),
            other => other.element(),
        }
    }

    fn rank(self) -> u32 {
        match self {
            Native::Array(_, r) => r,
            _ => 0,
        }
    }

    /// The runtime's element-kind tag for a scalar representation.
    fn kind_tag(self) -> u32 {
        match self {
            Native::Bool => 1,
            Native::Deci => 2,
            Native::Rat => 3,
            Native::Str => 4,
            Native::Num => 5,
            _ => 0,
        }
    }

    /// The scalar representation an array's elements are read as.
    fn element(self) -> Native {
        match self {
            Native::Array(1, _) => Native::Bool,
            Native::Array(2, _) => Native::Deci,
            Native::Array(3, _) => Native::Rat,
            Native::Array(4, _) => Native::Str,
            Native::Array(5, _) => Native::Num,
            _ => Native::Int,
        }
    }
}

/// How many decimal places a division or an irrational keeps when nothing says
/// otherwise. The interpreter uses the same number, so the two agree digit for digit.
const DEFAULT_DIGITS: u32 = 15;

/// Where a `handback` inside the current block should send its value.
#[derive(Clone, Copy)]
enum Handback<'ctx> {
    /// Into a slot, then on to the merge point: a conditional used as a value.
    Store(inkwell::values::PointerValue<'ctx>, Native, inkwell::basic_block::BasicBlock<'ctx>),
    /// Appended to a collected array: a loop used as a value.
    Push(BasicValueEnum<'ctx>, Native),
}

/// What a chain of selectors produces, given what the variable holds.
///
/// This has to agree exactly with `array_selectors`, which does the work: a dimension
/// addressed by a *single* index collapses, and only when every dimension collapses is
/// the result a single value rather than an array. Deciding from the last selector alone
/// — the old rule — called `('m'):1;` on a matrix a scalar, so a row came back where an
/// element was expected and the generated call did not match its own declaration.
fn selector_result(held: Native, selectors: &[Selector]) -> Native {
    let mut current = held;
    let mut run: Vec<&Selector> = Vec::new();

    for sel in selectors {
        match sel {
            // These end a run and answer a question about the whole array.
            Selector::Length => {
                current = Native::Int;
                run.clear();
            }
            Selector::Shape => {
                current = Native::Array(Native::Int.kind_tag(), 1);
                run.clear();
            }
            other => run.push(other),
        }
    }
    if run.is_empty() || !current.is_array() {
        return current;
    }

    let singles = run
        .iter()
        .filter(|s| matches!(s, Selector::Indices(ix) if ix.len() == 1))
        .count();
    let rank = current.rank() as usize;
    if singles == run.len() && singles == rank {
        // Every dimension pinned: one element.
        return current.element();
    }
    // The dimensions addressed by a single index disappear; the rest survive.
    let left = rank.saturating_sub(singles).max(1);
    Native::Array(current.element().kind_tag(), left as u32)
}

/// Whether a runtime function hands back a freshly allocated value, and of which kind.
///
/// Every function here returns memory the caller now owns. Listing them in one place is
/// the point: ownership is decided by the callee's contract, not by where in the
/// expression tree the call happened to be made.
fn allocates(name: &str) -> Option<Native> {
    let array = matches!(
        name,
        "ahpcl_array_new"
            | "ahpcl_array_empty"
            | "ahpcl_array_shape"
            | "ahpcl_array_select_run"
            | "ahpcl_array_elementwise"
            | "ahpcl_array_hadamard"
            | "ahpcl_array_dot"
            | "ahpcl_array_cross"
            | "ahpcl_array_tensor"
            | "ahpcl_array_compare"
            | "ahpcl_array_unary"
    );
    if array {
        // The rank is not known here and is not needed: releasing only reads the count.
        return Some(Native::Array(0, 1));
    }
    matches!(
        name,
        "ahpcl_num_from_int"
            | "ahpcl_num_from_deci"
            | "ahpcl_num_from_rat"
            | "ahpcl_num_from_bool"
            | "ahpcl_num_binary"
            | "ahpcl_num_unary"
            | "ahpcl_array_sum"
            | "ahpcl_array_get_num"
    )
    .then_some(Native::Num)
}

/// Which native representation a declared type maps onto, if any.
fn native_base(ty: &TypeRef) -> Result<Native, Unsupported> {
    native_base_shaped(ty, None)
}

/// The same, given the shape written on the *binding*.
///
/// `TypeRef.shape` is always `None` for a declaration — the parser puts `[2,2,2]` on
/// `Binding.shape`, not on the type. Reading rank from the type alone therefore fell
/// through to `Rank::dimensions()`, which answers `None` for `tensor` because a tensor
/// may have any rank of three or more. Every tensor then came out as rank 1, and the
/// backend and the runtime disagreed about the shape of every tensor in the program.
fn native_base_shaped(ty: &TypeRef, shape: Option<&Vec<Dim>>) -> Result<Native, Unsupported> {
    if ty.rank.is_some() {
        let scalar = TypeRef { rank: None, ..ty.clone() };
        // The written shape is the most reliable rank; the rank name backs it up. A
        // `tensor` with neither is a rank we do not know, and guessing is what caused
        // the bug above.
        let rank = shape
            .map(|s| s.len())
            .or_else(|| ty.shape.as_ref().map(|s| s.len()))
            .or_else(|| ty.rank.and_then(|r| r.dimensions()));
        let Some(rank) = rank else {
            return Err(Unsupported::new(
                "a tensor whose rank is not written — give it a shape, as in [2,2,2]",
            ));
        };
        return Ok(Native::Array(native_base(&scalar)?.kind_tag(), rank as u32));
    }
    match ty.base.as_str() {
        "int" => Ok(Native::Int),
        "bool" => Ok(Native::Bool),
        "deci" => Ok(Native::Deci),
        "rat" => Ok(Native::Rat),
        "str" => Ok(Native::Str),
        // `nna` is an array of text by definition — it has no rank name because it is
        // always one-dimensional.
        "nna" => Ok(Native::Array(Native::Str.kind_tag(), 1)),
        "num" => Ok(Native::Num),
        // `infnum` is exact and `i128`-backed, so it shares the decimal
        // representation — see the v1 bounds in docs/types.md.
        "infnum" | "∞num" => Ok(Native::Deci),
        "none" => Ok(Native::None),
        other => Err(Unsupported::new(format!("the type '{other}'"))),
    }
}

/// Whether an expression is a plain `('name')` with no selector, which is what Rule A
/// keys on.
fn is_bare_ref(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Ref { selectors, .. } => selectors.is_empty(),
        ExprKind::Math(inner) => is_bare_ref(inner),
        _ => false,
    }
}

/// An array literal may be nested to mirror its shape; the elements are stored in
/// row-major order, so read the leaves left to right.
fn flatten_literal(items: &[Expr]) -> Vec<&Expr> {
    let mut out = Vec::new();
    for item in items {
        match &item.kind {
            ExprKind::ArrayLit(inner) => out.extend(flatten_literal(inner)),
            _ => out.push(item),
        }
    }
    out
}

/// The shape a nested literal describes: `{{1,2},{3,4}}` is `[2, 2]`.
fn literal_shape(items: &[Expr]) -> Vec<u64> {
    let mut dims = vec![items.len() as u64];
    if let Some(Expr { kind: ExprKind::ArrayLit(inner), .. }) = items.first() {
        dims.extend(literal_shape(inner));
    }
    dims
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
