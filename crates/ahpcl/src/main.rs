//! The `ahpcl` command. A thin shell over `ahpcl-driver`.

use std::process::ExitCode;
use std::time::Instant;

use ahpcl_driver::{build_program, check, cli, format_duration, run_program, Built};

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
            eprintln!("suggest fix: {}", e.suggestion);
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
            eprintln!("suggest fix: try task:check., task:run. or task:build.");
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
        eprintln!("suggest fix: add buildfile:main.ahpcl.");
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
                eprintln!("suggest fix: check the path, and that the file exists.");
                eprintln!();
                eprintln!("1 error found.");
                failed = true;
                continue;
            }
        };

        let report = check(path.clone(), text);

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
                    eprintln!("informer: not compiled natively — {what} is not in the backend yet");
                    eprintln!("informer: running on the interpreter instead");
                    let outcome = run_program(&report);
                    for line in &outcome.lines {
                        println!("{line}");
                    }
                    if let Some(err) = outcome.error {
                        eprint!("{}", ahpcl_diagnostics::error::render(&report.source, &[err]));
                        failed = true;
                    }
                }
                Err(message) => {
                    eprintln!("AHPCL Error Handler:");
                    eprintln!("Hello, I think that there's something wrong.");
                    eprintln!();
                    eprintln!("rule conditions: {message}");
                    eprintln!("suggest fix: check that a C compiler is installed and on PATH.");
                    eprintln!();
                    eprintln!("1 error found.");
                    failed = true;
                }
            }
        }

        if task == "run" {
            let outcome = run_program(&report);
            // Program output goes to stdout; everything else to stderr, so anything
            // piped from stdout stays clean.
            for line in &outcome.lines {
                println!("{line}");
            }
            if let Some(err) = outcome.error {
                eprintln!();
                // `render` prints its own header and greeting, so do not add another.
                eprint!("{}", ahpcl_diagnostics::error::render(&report.source, &[err]));
                failed = true;
            } else {
                eprintln!("─────");
                eprintln!("finished in {}", format_duration(outcome.elapsed));
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
