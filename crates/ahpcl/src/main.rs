//! The `ahpcl` command. A thin shell over `ahpcl-driver`.

use std::process::ExitCode;
use std::time::Instant;

use ahpcl_driver::{
    budget_from_flags, build_program, build_temporary, check_with, cli, format_duration,
    run_binary, Built,
};

fn main() -> ExitCode {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    let cmd = match cli::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("AHPCL Error Handler:");
            eprintln!("Hello, I think that there's something wrong.");
            eprintln!();
            eprintln!("rule conditions: {}", e.message);
            eprintln!("suggested fix: {}", e.suggestion);
            eprintln!();
            eprintln!("1 error found.");
            return ExitCode::FAILURE;
        }
    };

    let task = cmd.task.as_deref().unwrap_or("check");
    match task {
        "check" | "build" | "run" => {}
        other => {
            eprintln!("AHPCL Error Handler:");
            eprintln!("Hello, I think that there's something wrong.");
            eprintln!();
            eprintln!("rule conditions: '{other}' is not a task AHPCL knows.");
            eprintln!("suggested fix: try task:check., task:run. or task:build.");
            eprintln!();
            eprintln!("1 error found.");
            return ExitCode::FAILURE;
        }
    }

    if cmd.buildfiles.is_empty() {
        eprintln!("AHPCL Error Handler:");
        eprintln!("Hello, I think that there's something wrong.");
        eprintln!();
        eprintln!("rule conditions: a task needs at least one source file.");
        eprintln!("suggested fix: add buildfile:main.ahpcl.");
        eprintln!();
        eprintln!("1 error found.");
        return ExitCode::FAILURE;
    }

    let mut failed = false;
    for path in &cmd.buildfiles {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("AHPCL Error Handler:");
                eprintln!("Hello, I think that there's something wrong.");
                eprintln!();
                eprintln!("rule conditions: {path} could not be read — {e}.");
                eprintln!("suggested fix: check the path, and that the file exists.");
                eprintln!();
                eprintln!("1 error found.");
                failed = true;
                continue;
            }
        };

        let report = check_with(path.clone(), text, budget_from_flags(&cmd.flags));

        // The Informer and the Error Handler both go to stderr, so anything piped
        // from stdout stays clean.
        let informer = report.informer_text();
        if !informer.is_empty() {
            eprint!("{informer}");
        }
        if !report.ok() {
            eprint!("{}", report.errors_text());
            failed = true;
            continue;
        }

        if task == "build" {
            let name = cmd.resultname.clone().unwrap_or_else(|| {
                std::path::Path::new(path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "a.out".to_string())
            });
            let dir = cmd.to.clone().unwrap_or_else(|| ".".to_string());
            let out = std::path::Path::new(&dir).join(&name);

            match build_program(&report, &out) {
                Ok(Built::Native { path, ir_lines, elapsed }) => {
                    eprintln!(
                        "informer: generated {ir_lines} lines of LLVM IR and linked in {}",
                        format_duration(elapsed)
                    );
                    eprintln!("wrote {}", path.display());
                }
                Ok(Built::NotYetNative { what }) => {
                    not_yet_native(&what);
                    failed = true;
                }
                Err(message) => {
                    linker_trouble(&message);
                    failed = true;
                }
            }
        }

        if task == "run" {
            // Compiled, not interpreted: one execution path means what runs here is
            // exactly what a built binary does.
            let name = cmd
                .buildfiles
                .first()
                .and_then(|p| std::path::Path::new(p).file_stem())
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "ahpcl".to_string());

            match build_temporary(&report, &name) {
                Ok(Built::Native { path, ir_lines, elapsed }) => {
                    // Reported before the program runs, so the informer does not land
                    // in the middle of its output.
                    eprintln!(
                        "informer: generated {ir_lines} lines of LLVM IR and linked in {}",
                        format_duration(elapsed)
                    );
                    eprintln!("─────");
                    match run_binary(&path) {
                        Ok((ok, ran_in)) => {
                            eprintln!("─────");
                            eprintln!("finished in {}", format_duration(ran_in));
                            if !ok {
                                failed = true;
                            }
                        }
                        Err(message) => {
                            linker_trouble(&message);
                            failed = true;
                        }
                    }
                }
                Ok(Built::NotYetNative { what }) => {
                    not_yet_native(&what);
                    failed = true;
                }
                Err(message) => {
                    linker_trouble(&message);
                    failed = true;
                }
            }
        }
    }



    eprintln!();
    eprintln!("finished in {}", format_duration(started.elapsed()));

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

const USAGE: &str = "\
AHPCL — Advanced High-Performance Calculations Language

The command line speaks the same syntax as the language.

  ahpcl task:check. buildfile:main.ahpcl.
  ahpcl task:run.   buildfile:main.ahpcl.
  ahpcl task:build. buildfile:main.ahpcl, lib.ahpcl. resultname:myprogram.

Directives:
  task:         check | run | build
  buildfile:    source file(s); ',' extends, '.' ends
  resultname:   name of the output
  to:           where to write it
  flag:         compiler flags, e.g. flag:loop-evaluation=limit
";

/// The backend cannot compile this program yet. There is no second way to run it — the
/// interpreter is a test oracle, not an execution mode — so this stops.
fn not_yet_native(what: &str) {
    eprintln!("AHPCL Error Handler:");
    eprintln!("Hello, I think that there's something wrong.");
    eprintln!();
    eprintln!("rule conditions: {what} is not in the compiler yet, so this program cannot be built.");
    eprintln!("suggested fix: rewrite that part, or open an issue — this is a gap in AHPCL, not in your program.");
    eprintln!();
    eprintln!("1 error found.");
}

fn linker_trouble(message: &str) {
    eprintln!("AHPCL Error Handler:");
    eprintln!("Hello, I think that there's something wrong.");
    eprintln!();
    eprintln!("rule conditions: {message}");
    eprintln!("suggested fix: check that a C compiler is installed and on PATH.");
    eprintln!();
    eprintln!("1 error found.");
}
