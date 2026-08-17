//! LLVM code generation.
//!
//! Emits real LLVM IR, which is assembled to an object file and linked into a native
//! executable. See docs/toolchain.md.
//!
//! **What compiles natively today:** `int` and `bool` values, all arithmetic and
//! comparison on them, `if`/`else` chains, both loop kinds, functions, `change:`, and
//! `print` of text and integers. Programs using `deci`, `rat` or arrays are *not*
//! rejected — the driver runs them on the interpreter instead and the Informer says so.
//! Extending the backend to those needs a runtime library for exact decimal and
//! rational arithmetic, which is the next stage.
//!
//! The split is honest rather than arbitrary: integers map onto machine words, while
//! AHPCL's exact decimals and rationals have no native LLVM representation at all.

mod native;

pub use native::{compile, compile_with_widths, Compiled, Unsupported};
