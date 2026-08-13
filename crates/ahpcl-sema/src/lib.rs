//! Semantic analysis: name resolution and type checking.

pub mod check;
pub mod interval;
pub mod scope;
pub mod types;
pub mod verify;

pub use check::{check, Checked};
pub use types::{Base, Shape, Type};
pub use verify::{verify, EvalBudget, Verified};
