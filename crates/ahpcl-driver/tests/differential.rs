//! The oracle test: every program must produce identical output compiled and interpreted.
//!
//! This is the interpreter's whole job. AHPCL runs compiled code and nothing else, so the
//! interpreter is not a second way to run a program — it is a second *opinion* about what
//! a program means, written independently of the backend.
//!
//! That independence is the point. A hand-written test encodes the assumptions of whoever
//! wrote it, so it cannot catch a bug that came from those same assumptions; two
//! implementations disagreeing can. Nearly every codegen bug found so far surfaced here
//! first — loop counters read as the wrong type, selectors indexing flat storage instead
//! of by dimension, a bare array reference handing back a pointer.
//!
//! When this test fails, one of the two is wrong, and which one is a question to answer
//! rather than assume: some failures have been the interpreter's fault, not the backend's.

use std::path::{Path, PathBuf};
use std::process::Command;

use ahpcl_driver::{build_program, check_with, run_program, Built};
use ahpcl_sema::EvalBudget;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the workspace root")
        .to_path_buf()
}

/// What the interpreter says the program prints.
fn interpreted(source: &str, name: &str) -> String {
    let report = check_with(name, source, EvalBudget::Unlimited);
    assert!(
        report.errors.is_empty(),
        "{name} should check cleanly: {:#?}",
        report.errors
    );
    let outcome = run_program(&report);
    assert!(
        outcome.error.is_none(),
        "{name} should run cleanly on the interpreter: {:#?}",
        outcome.error
    );
    outcome.lines.join("\n").trim_end().to_string()
}

/// Build the runtime staticlib before anything links against it.
///
/// Declaring `ahpcl-runtime` as a dev-dependency is *not* enough: that makes cargo build
/// the rlib, while linking needs the `staticlib` artifact, which is only produced when
/// the crate is built as a target in its own right. Without this the tests link whatever
/// `libahpcl_runtime.a` was last left in `target/`, so a broken runtime can pass — which
/// it did, silently, until an injected bug failed to show up.
fn ensure_runtime_is_current() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let profile = if cfg!(debug_assertions) { "dev" } else { "release" };
        let status = Command::new(env!("CARGO"))
            .args(["build", "-p", "ahpcl-runtime", "--profile", profile])
            .current_dir(workspace_root())
            .status()
            .expect("cargo should run");
        assert!(status.success(), "the runtime staticlib should build");
    });
}

/// What the compiled program actually prints.
fn compiled(source: &str, name: &str) -> String {
    ensure_runtime_is_current();
    let report = check_with(name, source, EvalBudget::Unlimited);
    let dir = std::env::temp_dir().join("ahpcl-differential");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let binary = dir.join(name);

    match build_program(&report, &binary).expect("the linker should run") {
        Built::Native { .. } => {}
        Built::NotYetNative { what } => {
            panic!("{name} did not compile natively: {what} is not in the backend yet")
        }
    }

    let out = Command::new(&binary).output().expect("the binary should run");
    let _ = std::fs::remove_file(&binary);
    assert!(
        out.status.success(),
        "{name} should exit cleanly, got {:?}:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

fn agree(name: &str, source: &str) {
    let a = interpreted(source, name);
    let b = compiled(source, name);
    assert_eq!(
        a, b,
        "\n{name}: the two implementations disagree.\n\
         interpreted:\n{a}\n\ncompiled:\n{b}\n"
    );
}

#[test]
fn every_example_agrees_on_both_paths() {
    let examples = workspace_root().join("examples");
    let mut checked = 0;
    for entry in std::fs::read_dir(&examples).expect("the examples directory") {
        let path = entry.expect("a directory entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ahpcl") {
            continue;
        }
        let name = path
            .file_stem()
            .expect("a file stem")
            .to_string_lossy()
            .to_string();
        let source = std::fs::read_to_string(&path).expect("a readable example");
        agree(&name, &source);
        checked += 1;
    }
    assert!(checked > 0, "no examples were found to compare");
}

/// Programs chosen to cross the seams where the two implementations could drift apart:
/// each exact type, arrays and their operators, selectors on rank above one, the
/// value-forms, and the arithmetic whose rounding rules are easy to get subtly wrong.
const CASES: &[(&str, &str)] = &[
    (
        "exact_decimals",
        "var:deci 'a' = '0.1', 'b' = '0.2'.\n\
         var:deci 's' = math { ('a') + ('b') }.\n\
         var:bool 'exact' = math { ('s') = '0.3' }.\n\
         print[('s')].\nprint[('exact')].",
    ),
    (
        "exact_rationals",
        "var:rat 'a' = math { 1 / 3 }.\n\
         var:rat 't' = math { ('a') + ('a') + ('a') }.\n\
         var:rat 'sq' = math { ('a') x ('a') }.\n\
         print[('a')].\nprint[('t')].\nprint[('sq')].",
    ),
    (
        "euclidean_division",
        "var:int 'a' [32 bit] = math { 0 - 17 }.\n\
         var:int 'b' [32 bit] = '5'.\n\
         var:int 'q' [32 bit] = math { ('a') // ('b') }.\n\
         var:int 'r' [32 bit] = math { ('a') mod ('b') }.\n\
         print[('q')].\nprint[('r')].",
    ),
    (
        "text_and_comparison",
        "var:str 'n' = \"Alice\".\n\
         var:bool 'same' = math { ('n') = \"Alice\" }.\n\
         var:bool 'before' = math { ('n') < \"Bob\" }.\n\
         print[('n')].\nprint[('same')].\nprint[('before')].",
    ),
    (
        "array_operators",
        "var:vector:int 'u' [3] = {'1','2','3'}.\n\
         var:vector:int 'v' [3] = {'4','5','6'}.\n\
         var:int 'dot' [64 bit] = math { ('u') · ('v') }.\n\
         var:vector:int 'had' [3] = math { ('u') ⊙ ('v') }.\n\
         var:vector:int 'crs' [3] = math { ('u') × ('v') }.\n\
         print[('dot')].\nprint[('had')].\nprint[('crs')].",
    ),
    (
        "matrix_selectors",
        "var:matrix:int 'm' [3,4] = {{'1','2','3','4'},{'5','6','7','8'},{'9','10','11','12'}}.\n\
         var:vector:int 'row' [4] = ('m'):2;.\n\
         var:int 'cell' [64 bit] = ('m'):2;:3;.\n\
         var:vector:int 'shp' = ('m'):shape;.\n\
         var:int 'len' [64 bit] = ('m'):length;.\n\
         print[('row')].\nprint[('cell')].\nprint[('shp')].\nprint[('len')].",
    ),
    (
        "rule_a_reduction",
        "var:vector:int 'a' [4] = {'1','2','3','4'}.\n\
         var:int 's' [64 bit] = math { ('a') }.\n\
         var:vector:int 'd' [4] = math { -('a'):all; }.\n\
         print[('s')].\nprint[('d')].",
    ),
    (
        "loop_counter_in_exact_context",
        "loop:var:int 'i' = math { 1 to 4 } {\n\
             var:deci 'x' = math { ('i') / 8 }.\n\
             print[('x')].\n\
         }.",
    ),
    (
        "loop_as_a_value",
        "var:vector:int 'squares' = loop:var:int 'i' = math { 1 to 5 } {\n\
             handback math { ('i') x ('i') }.\n\
         }.\n\
         print[('squares')].",
    ),
    (
        "functions_and_recursion",
        "func:int 'fact' [var:int 'n' [32 bit]] {\n\
             if math { ('n') <= 1 } { handback '1'. }, else {\n\
                 handback math { ('n') x 'fact'[math { ('n') - 1 }] }.\n\
             }.\n\
         }.\n\
         var:int 'f' [64 bit] = 'fact'['10'].\n\
         print[('f')].",
    ),
    (
        // `handback` ends the iteration, so the tail runs only when it did not fire.
        // This is the case where the two implementations disagreed: native used to run
        // the rest of the body after handing a value back.
        "handback_ends_the_iteration",
        "var:vector:int 'v' = loop:var:int 'i' = math { 1 to 4 } {\n\
             if math { ('i') > 2 } {\n\
                 handback ('i').\n\
             }.\n\
             print[\"tail\"].\n\
         }.\n\
         print[('v')].",
    ),
    (
        "handback_from_a_while_loop",
        "var:int 'n' [32 bit] = '3'.\n\
         var:vector:int 'v' = loop:while math { ('n') > 0 } {\n\
             change:var:int 'n' = math { ('n') - 1 }.\n\
             handback ('n').\n\
         }.\n\
         print[('v')].",
    ),
    (
        // Native integers were 64-bit while the interpreter was 128-bit, so anything
        // past ~9.2×10¹⁸ diverged: the interpreter kept going, native wrapped silently
        // and later errored. These values are checked against Python.
        "large_integers",
        "var:int 'a' [128 bit] = '9223372036854775807'.\n\
         var:int 'b' [128 bit] = math { ('a') + ('a') }.\n\
         var:int 'c' [128 bit] = '99999999999'.\n\
         var:int 'd' [128 bit] = math { ('c') x ('c') }.\n\
         var:int 'e' [128 bit] = math { 10 xx 30 }.\n\
         var:int 'm' [128 bit] = '170141183460469231731687303715884105727'.\n\
         print[('b')].\nprint[('d')].\nprint[('e')].\nprint[('m')].",
    ),
    (
        // The last three gaps the backend used to decline outright.
        "nna_text_arrays",
        // Not indexed: what an `nna` element narrows to is still open (types.md), and
        // the checker currently calls it `nna` rather than `str`.
        "var:nna 'names' = {\"hello\", \"John Doe\", \"Lol😂\"}.\n\
         print[('names')].",
    ),
    (
        // A comparison's result is a bool array, but its operands keep their own kinds:
        // coercing them to the result element compared against `true`, so every operator
        // silently used 1 as the right-hand side.
        "elementwise_comparison",
        "var:vector:int 'a' [3] = {'1','2','3'}.\n\
         var:vector:bool 'big' [3] = math { ('a'):all; > 2 }.\n\
         var:vector:bool 'small' [3] = math { ('a'):all; <= 2 }.\n\
         var:vector:int 'b' [3] = {'1','9','3'}.\n\
         var:vector:bool 'same' [3] = math { ('a'):all; = ('b'):all; }.\n\
         print[('big')].\nprint[('small')].\nprint[('same')].",
    ),
    (
        "parse_fractions",
        "var:rat 'a' [64 bit] = parse[\"2/6\" fraction].\n\
         var:rat 'b' [64 bit] = parse[\"7/2\" fraction].\n\
         var:rat 'c' [64 bit] = parse[\"0.5\"].\n\
         print[('a')].\nprint[('b')].\nprint[('c')].",
    ),
    (
        "chained_element_assignment",
        "var:matrix:int 'm' [2,3] = {{'1','2','3'},{'4','5','6'}}.\n\
         change:var:int 'm':2;:1; = '99'.\n\
         change:var:int 'm':1;:3; = '7'.\n\
         print[('m')].",
    ),
    (
        "powers_stay_exact",
        "var:deci 'a' = '1.1'.\n\
         var:deci 'p' = math { ('a') xx 20 }.\n\
         print[('p')].",
    ),
    (
        "parsing_options",
        "var:int 'a' [32 bit] = parse[\"1,234\" group:\",\"].\n\
         var:deci 'b' [64 bit] = parse[\"  3.25  \" trim].\n\
         var:int 'c' [32 bit] = parse[\"0xff\" hex].\n\
         print[('a')].\nprint[('b')].\nprint[('c')].",
    ),
];

#[test]
fn the_two_implementations_agree_across_the_language() {
    for (name, source) in CASES {
        agree(name, source);
    }
}
