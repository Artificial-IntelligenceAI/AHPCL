//! Diagnostics for AHPCL: the Error Handler and the Informer.
//!
//! See docs/diagnostics.md. The design decisions encoded here:
//!   * columns count grapheme clusters; carets are drawn in display width
//!   * one error may point at several places, each with its own note
//!   * grammar varies with the error count
//!   * the Informer is one line per note, never the error template

pub mod error;
pub mod informer;
pub mod position;

pub use error::{Category, Code, Error, Label};
pub use informer::Informer;
pub use position::{BytePos, LineCol, SourceFile, Span};
