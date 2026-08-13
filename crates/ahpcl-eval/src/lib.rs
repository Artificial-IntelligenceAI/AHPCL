//! Evaluation: exact values, and an interpreter.
//!
//! The interpreter does three jobs — it runs programs, it is layer 1 of verification
//! (executing a loop at compile time to check a refinement), and it folds constants.

pub mod eval;
pub mod value;

pub use eval::{run, Interpreter, Output};
pub use value::{Array, Decimal, Rational, Value};
