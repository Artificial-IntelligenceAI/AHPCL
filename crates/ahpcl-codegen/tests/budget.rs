//! Budget tests: a program must finish in reasonable time and memory.
//!
//! The differential oracle cannot catch this class at all. It compares *output*, and a
//! leaky or quadratic implementation still prints the right answer — so reading a
//! million array elements allocated 3.2GB and took 600x longer than it should, through
//! three stress-test passes and a green suite, because every test only ever asked
//! whether the answer was correct.
//!
//! The bounds here are deliberately loose. They are not performance targets; they are
//! tripwires for a change that makes something allocate or loop per element again.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Peak memory of a run, in bytes, via the system's own accounting.
fn peak_bytes(binary: &std::path::Path) -> u64 {
    let out = Command::new("/usr/bin/time")
        .arg("-l")
        .arg(binary)
        .output()
        .expect("/usr/bin/time should run");
    let text = String::from_utf8_lossy(&out.stderr);
    for line in text.lines() {
        if line.contains("maximum resident set size") {
            if let Some(n) = line.split_whitespace().next() {
                return n.parse().unwrap_or(0);
            }
        }
    }
    0
}

use ahpcl_codegen::compile;
use ahpcl_syntax::parse_source;

fn workdir() -> PathBuf {
    let dir = std::env::temp_dir().join("ahpcl-budget-tests");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

fn runtime_library() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the workspace root")
        .to_path_buf();
    let profile = if cfg!(debug_assertions) { "dev" } else { "release" };
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "ahpcl-runtime", "--profile", profile])
        .current_dir(&root)
        .status()
        .expect("cargo should run");
    assert!(status.success(), "the runtime staticlib should build");
    for p in ["debug", "release"] {
        let c = root.join("target").join(p).join("libahpcl_runtime.a");
        if c.exists() {
            return c;
        }
    }
    panic!("no runtime staticlib");
}

/// Compile, run, and report how long the program itself took.
fn time_it(name: &str, src: &str) -> Duration {
    let (program, errors) = parse_source(src);
    assert!(errors.is_empty(), "should parse: {errors:#?}");
    let dir = workdir();
    let object = dir.join(format!("{name}.o"));
    let binary = dir.join(name);
    compile(&program, &object, name).unwrap_or_else(|u| panic!("should compile: {}", u.what));

    let status = Command::new("cc")
        .arg(&object)
        .arg("-o")
        .arg(&binary)
        .arg(runtime_library())
        .status()
        .expect("cc should run");
    assert!(status.success(), "linking should succeed");

    let started = Instant::now();
    let out = Command::new(&binary).output().expect("the binary should run");
    let elapsed = started.elapsed();
    let _ = std::fs::remove_file(&binary);
    assert!(out.status.success(), "the program should exit cleanly");
    elapsed
}

#[test]
fn reading_a_million_elements_does_not_allocate_per_element() {
    // Every element read used to build four selector descriptor arrays and allocate a
    // fresh array object to hold the one value. Correct, and 120ns and one leak per
    // read. A million reads is milliseconds of real work; a second means the
    // per-element allocation is back.
    let took = time_it(
        "budget_elements",
        "var:vector:int 'data' [1000000] = \
             loop:var:int 'i' = math { 1 to 1000000 } { handback ('i'). }.\n\
         var:int 't' [64 bit] = '0'.\n\
         loop:var:int 'j' = math { 1 to 1000000 } {\n\
             change:var:int 't' = math { ('t') + ('data'):('j'); }.\n\
         }.\n\
         print[('t')].",
    );
    assert!(
        took < Duration::from_secs(2),
        "reading a million elements took {took:?}; it should be milliseconds of work, \
         so something is allocating or scanning per element"
    );
}

#[test]
fn a_counting_loop_stays_a_counting_loop() {
    // Ten million iterations of integer arithmetic. Generous, since a debug-profile
    // test binary still links an optimised runtime and an optimised generated program.
    let took = time_it(
        "budget_counting",
        "var:int 't' [64 bit] = '0'.\n\
         loop:var:int 'i' = math { 1 to 10000000 } {\n\
             change:var:int 't' = math { ('t') + ('i') }.\n\
         }.\n\
         print[('t')].",
    );
    assert!(
        took < Duration::from_secs(2),
        "a ten-million-iteration counting loop took {took:?}"
    );
}

/// Compile and link, handing back the binary rather than timing it.
fn build_only(name: &str, src: &str) -> PathBuf {
    let (program, errors) = parse_source(src);
    assert!(errors.is_empty(), "should parse: {errors:#?}");
    let dir = workdir();
    let object = dir.join(format!("{name}.o"));
    let binary = dir.join(name);
    compile(&program, &object, name).unwrap_or_else(|u| panic!("should compile: {}", u.what));
    let status = Command::new("cc")
        .arg(&object)
        .arg("-o")
        .arg(&binary)
        .arg(runtime_library())
        .status()
        .expect("cc should run");
    assert!(status.success(), "linking should succeed");
    binary
}

fn slicing_loop(iterations: u64) -> String {
    format!(
        "var:vector:int 'data' [1000] = \
             loop:var:int 'i' = math {{ 1 to 1000 }} {{ handback ('i'). }}.\n\
         var:int 't' [64 bit] = '0'.\n\
         loop:var:int 'p' = math {{ 1 to {iterations} }} {{\n\
             var:vector:int 'w' [3] = ('data'):1 to 3;.\n\
             change:var:int 't' = math {{ ('t') + ('w') }}.\n\
         }}.\n\
         print[('t')]."
    )
}

#[test]
fn memory_does_not_grow_with_the_number_of_iterations() {
    // The question a single reading cannot answer. Slicing in a loop leaked an array and
    // a boxed `num` per pass — 800k iterations reached 418MB — and every test still
    // passed, because the printed answer was right the whole time. What matters is not
    // the size but the *slope*: memory must not track the iteration count.
    let small = peak_bytes(&build_only("budget_mem_small", &slicing_loop(20_000)));
    let large = peak_bytes(&build_only("budget_mem_large", &slicing_loop(800_000)));
    assert!(small > 0 && large > 0, "could not read peak memory");
    assert!(
        large < small * 4,
        "memory grew with iteration count: {small} bytes at 20k, {large} at 800k \
         (40x the work). Something is allocated per iteration and never released."
    );
}
