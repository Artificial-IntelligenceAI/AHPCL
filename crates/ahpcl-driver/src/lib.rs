//! The driver: a library first, a CLI second.
//!
//! The compiler is a library because the JIT decision requires it — something has to
//! embed this. `crates/ahpcl` is a thin shell over it.

pub mod cli;

use std::time::Instant;

use ahpcl_diagnostics::{error, Error, Informer, SourceFile};
use ahpcl_syntax::lex;

pub struct Report {
    pub source: SourceFile,
    pub errors: Vec<Error>,
    pub informer: Informer,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// The Error Handler's rendering, or empty when nothing is wrong.
    pub fn errors_text(&self) -> String {
        error::render(&self.source, &self.errors)
    }

    pub fn informer_text(&self) -> String {
        self.informer.render(&self.source)
    }
}

/// `task:check` — lex, and report what is wrong.
///
/// Only the lexer exists so far; the parser and the rest follow.
pub fn check(name: impl Into<String>, text: impl Into<String>) -> Report {
    let source = SourceFile::new(name, text);
    let mut informer = Informer::new();

    let started = Instant::now();
    let lexed = lex(&source.text);
    let elapsed = started.elapsed();

    informer.say_global(format!(
        "lexed {} line{} in {}",
        source.line_count(),
        if source.line_count() == 1 { "" } else { "s" },
        format_duration(elapsed)
    ));

    Report { source, errors: lexed.errors, informer }
}

/// Human-readable durations. Sub-millisecond work is common, so µs matter.
pub fn format_duration(d: std::time::Duration) -> String {
    let ns = d.as_nanos();
    if ns < 1_000 {
        format!("{ns}ns")
    } else if ns < 1_000_000 {
        format!("{:.1}µs", ns as f64 / 1_000.0)
    } else if ns < 1_000_000_000 {
        format!("{:.1}ms", ns as f64 / 1_000_000.0)
    } else {
        format!("{:.2}s", ns as f64 / 1_000_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_file_reports_nothing() {
        let r = check("t.ahpcl", "print[\"Hello, World!\"].\n");
        assert!(r.ok());
        assert_eq!(r.errors_text(), "");
    }

    #[test]
    fn a_broken_file_reports_through_the_error_handler() {
        let r = check("t.ahpcl", "math { 5 \u{2212} 3 }\n");
        assert!(!r.ok());
        let text = r.errors_text();
        assert!(text.starts_with("AHPCL Error Handler:"));
        assert!(text.contains("U+2212 MINUS SIGN"));
        assert!(text.contains("1 error found."));
    }

    #[test]
    fn durations_read_sensibly() {
        use std::time::Duration;
        assert_eq!(format_duration(Duration::from_nanos(500)), "500ns");
        assert_eq!(format_duration(Duration::from_micros(1500)), "1.5ms");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.50s");
    }
}
