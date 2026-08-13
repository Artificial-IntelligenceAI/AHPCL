//! The driver: a library first, a CLI second.
//!
//! The compiler is a library because the JIT decision requires it — something has to
//! embed this. `crates/ahpcl` is a thin shell over it.

pub mod cli;

use std::time::Instant;

use ahpcl_diagnostics::{error, Error, Informer, SourceFile};
use ahpcl_eval::run as evaluate;
use ahpcl_sema::{check as typecheck, verify, EvalBudget};
use ahpcl_syntax::{lex, parse, Program};

pub struct Report {
    pub source: SourceFile,
    pub errors: Vec<Error>,
    pub informer: Informer,
    pub program: Program,
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

/// `task:check` — lex, parse, and report what is wrong.
///
/// The type checker and code generation follow; this is a v1 iteration.
pub fn check(name: impl Into<String>, text: impl Into<String>) -> Report {
    check_with(name, text, EvalBudget::default())
}

/// As `check`, with control over how much compile-time evaluation is allowed.
pub fn check_with(
    name: impl Into<String>,
    text: impl Into<String>,
    budget: EvalBudget,
) -> Report {
    let source = SourceFile::new(name, text);
    let mut informer = Informer::new();
    let mut errors = Vec::new();

    let started = Instant::now();
    let lexed = lex(&source.text);
    let lex_time = started.elapsed();
    let token_count = lexed.tokens.len();
    errors.extend(lexed.errors);

    informer.say_global(format!(
        "lexed {} line{} into {token_count} tokens in {}",
        source.line_count(),
        if source.line_count() == 1 { "" } else { "s" },
        format_duration(lex_time)
    ));

    let started = Instant::now();
    let parsed = parse(lexed.tokens);
    let parse_time = started.elapsed();
    errors.extend(parsed.errors);

    informer.say_global(format!(
        "parsed {} statement{} in {}",
        parsed.program.statements.len(),
        if parsed.program.statements.len() == 1 { "" } else { "s" },
        format_duration(parse_time)
    ));

    // Type checking only runs on a program the parser could make sense of; otherwise
    // it would report a cascade of consequences rather than causes.
    if errors.is_empty() {
        let started = Instant::now();
        let checked = typecheck(&parsed.program, &mut informer);
        let check_time = started.elapsed();
        errors.extend(checked.errors);
        informer.say_global(format!("type-checked in {}", format_duration(check_time)));

        // Verification only runs on a program that type-checks; otherwise it would
        // reason about types that are already known to be wrong.
        if errors.is_empty() {
            let started = Instant::now();
            let verified = verify(&parsed.program, &mut informer, budget);
            let verify_time = started.elapsed();
            let checks = verified.runtime_checks.len();
            errors.extend(verified.errors);
            informer.say_global(format!(
                "verified in {} ({checks} runtime check{} inserted)",
                format_duration(verify_time),
                if checks == 1 { "" } else { "s" }
            ));
        }
    }

    Report { source, errors, informer, program: parsed.program }
}

/// The result of running a program.
pub struct RunOutcome {
    /// What the program printed. Goes to stdout, so pipes stay clean.
    pub lines: Vec<String>,
    /// A runtime failure, which stops the program.
    pub error: Option<Error>,
    pub elapsed: std::time::Duration,
}

/// `task:run` — check, then execute.
///
/// Runs on the interpreter. LLVM code generation is the next stage; the interpreter
/// exists regardless, because verification needs it.
pub fn run_program(report: &Report) -> RunOutcome {
    let started = Instant::now();
    let out = evaluate(&report.program);
    RunOutcome { lines: out.lines, error: out.error, elapsed: started.elapsed() }
}

/// Read the `loop-evaluation` flag.
///
/// Deliberately a *caller* decision rather than something sniffed from the
/// environment: the same source must mean the same thing however it was compiled.
pub fn budget_from_flags(flags: &std::collections::BTreeMap<String, String>) -> EvalBudget {
    match flags.get("loop-evaluation").map(String::as_str) {
        Some("limit") | Some("limited") => EvalBudget::Limited(1_000_000),
        Some("off") | Some("none") => EvalBudget::Off,
        _ => EvalBudget::Unlimited,
    }
}

/// What `task:build` produced.
pub enum Built {
    /// A native executable at this path.
    Native { path: std::path::PathBuf, ir_lines: usize, elapsed: std::time::Duration },
    /// The program uses a feature the backend does not cover yet. Not an error — the
    /// interpreter runs it instead.
    NotYetNative { what: String },
}

/// `task:build` — compile to a native executable via LLVM.
pub fn build_program(
    report: &Report,
    output: &std::path::Path,
) -> Result<Built, String> {
    let started = Instant::now();
    let object = output.with_extension("o");
    let name = output
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "ahpcl".to_string());

    let compiled = match ahpcl_codegen::compile(&report.program, &object, &name) {
        Ok(c) => c,
        Err(u) => return Ok(Built::NotYetNative { what: u.what }),
    };
    let ir_lines = compiled.ir.lines().count();

    // Link with the system compiler. The object only needs libc, since print becomes
    // a printf call.
    let status = std::process::Command::new("cc")
        .arg(&object)
        .arg("-o")
        .arg(output)
        .status()
        .map_err(|e| format!("could not run the system linker: {e}"))?;
    let _ = std::fs::remove_file(&object);

    if !status.success() {
        return Err("the system linker failed".to_string());
    }
    Ok(Built::Native {
        path: output.to_path_buf(),
        ir_lines,
        elapsed: started.elapsed(),
    })
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
        assert!(r.ok(), "{}", r.errors_text());
        assert_eq!(r.errors_text(), "");
        assert_eq!(r.program.statements.len(), 1);
    }

    #[test]
    fn the_stats_example_checks_clean() {
        let src = include_str!("../../../examples/stats.ahpcl");
        let r = check("stats.ahpcl", src);
        assert!(r.ok(), "{}", r.errors_text());
        assert_eq!(r.program.statements.len(), 4);
    }

    #[test]
    fn a_broken_file_reports_through_the_error_handler() {
        // A pasted minus sign. The lexer names it; the parser then also complains,
        // since what is left is not a well-formed statement.
        let r = check("t.ahpcl", "var:num 'x' = math { 5 \u{2212} 3 }.\n");
        assert!(!r.ok());
        let text = r.errors_text();
        assert!(text.starts_with("AHPCL Error Handler:"));
        assert!(text.contains("U+2212 MINUS SIGN"));
        assert!(text.contains("did you mean '-'"));
        assert!(text.contains("error"));
    }

    #[test]
    fn one_error_reads_singular_and_is_not_numbered() {
        let r = check("t.ahpcl", "var:num 'x' = 1000.\n");
        let text = r.errors_text();
        assert!(text.contains("there's something wrong"));
        assert!(text.contains("1 error found."));
        assert!(!text.contains("Error 1 of"));
    }

    #[test]
    fn durations_read_sensibly() {
        use std::time::Duration;
        assert_eq!(format_duration(Duration::from_nanos(500)), "500ns");
        assert_eq!(format_duration(Duration::from_micros(1500)), "1.5ms");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.50s");
    }
}
