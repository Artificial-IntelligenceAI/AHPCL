//! Semantic analysis: name resolution and type checking.

pub mod check;
pub mod scope;
pub mod types;

pub use check::{check, Checked};
pub use types::{Base, Shape, Type};
