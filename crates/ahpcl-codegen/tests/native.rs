//! Codegen tests: compile a program, link it, run it, check what it printed.

use std::path::PathBuf;
use std::process::Command;

use ahpcl_codegen::compile;
use ahpcl_syntax::parse_source;

fn workdir() -> PathBuf {
    let dir = std::env::temp_dir().join("ahpcl-codegen-tests");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// The compiled runtime staticlib. Generated code calls into it for exact decimals,
/// so a produced binary will not link without it.
fn runtime_library() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the workspace root")
        .to_path_buf();
    for profile in ["debug", "release"] {
        let candidate = root.join("target").join(profile).join("libahpcl_runtime.a");
        if candidate.exists() {
            return candidate;
        }
    }
    panic!("build ahpcl-runtime first: cargo build -p ahpcl-runtime");
}

/// Compile, link and run, returning stdout.
fn compile_and_run(name: &str, src: &str) -> String {
    let (program, errors) = parse_source(src);
    assert!(errors.is_empty(), "should parse: {errors:#?}");

    let dir = workdir();
    let object = dir.join(format!("{name}.o"));
    let binary = dir.join(name);

    compile(&program, &object, name).unwrap_or_else(|u| panic!("should compile natively: {}", u.what));

    let status = Command::new("cc")
        .arg(&object)
        .arg("-o")
        .arg(&binary)
        .arg(runtime_library())
        .status()
        .expect("cc should run");
    assert!(status.success(), "linking should succeed");

    let out = Command::new(&binary).output().expect("the binary should run");
    assert!(out.status.success(), "the program should exit cleanly");
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

/// Compile only, expecting the backend to decline.
fn declines(src: &str) -> String {
    let (program, errors) = parse_source(src);
    assert!(errors.is_empty(), "should parse: {errors:#?}");
    let object = workdir().join("declined.o");
    compile(&program, &object, "declined")
        .err()
        .expect("expected the backend to decline")
        .what
}

#[test]
fn arithmetic_compiles_and_gives_the_right_answer() {
    assert_eq!(
        compile_and_run("arith", "var:int 'r' = math { 2 + 3 * 4 }.\nprint[('r')]."),
        "14"
    );
}

#[test]
fn precedence_survives_code_generation() {
    // -3² is -9: the power binds tighter than the minus, natively too.
    assert_eq!(
        compile_and_run("prec", "var:int 'r' = math { -3 xx 2 }.\nprint[('r')]."),
        "-9"
    );
    // Powers group right to left, so this is 2^(3^2) = 512.
    assert_eq!(
        compile_and_run("prec2", "var:int 'r' = math { 2 xx 3 xx 2 }.\nprint[('r')]."),
        "512"
    );
}

#[test]
fn integer_division_and_remainder_compile() {
    assert_eq!(
        compile_and_run("idiv", "var:int 'r' = math { 10 // 4 }.\nprint[('r')]."),
        "2"
    );
    assert_eq!(
        compile_and_run("imod", "var:int 'r' = math { 10 mod 4 }.\nprint[('r')]."),
        "2"
    );
}

#[test]
fn a_counted_loop_compiles() {
    assert_eq!(
        compile_and_run(
            "counted",
            "var:int 't' = '0'.\n\
             loop:var:int 'i' = math { 1 to 100 } {\n\
                 change:var:int 't' = math { ('t') + ('i') }.\n\
             }.\n\
             print[('t')]."
        ),
        "5050"
    );
}

#[test]
fn a_loop_step_compiles() {
    assert_eq!(
        compile_and_run(
            "step",
            "var:int 't' = '0'.\n\
             loop:var:int 'i' = math { 1 to 9 by 2 } {\n\
                 change:var:int 't' = math { ('t') + ('i') }.\n\
             }.\n\
             print[('t')]."
        ),
        "25"
    );
}

#[test]
fn a_condition_loop_compiles() {
    assert_eq!(
        compile_and_run(
            "while",
            "var:int 'n' = '5'.\n\
             loop:while math { ('n') > 0 } {\n\
                 change:var:int 'n' = math { ('n') - 1 }.\n\
             }.\n\
             print[('n')]."
        ),
        "0"
    );
}

#[test]
fn an_if_else_chain_compiles() {
    let src = "var:int 'x' = '4'.\n\
               var:int 'r' = '0'.\n\
               if math { ('x') > 5 } {\n\
                   change:var:int 'r' = '1'.\n\
               }, else if math { ('x') > 3 } {\n\
                   change:var:int 'r' = '2'.\n\
               }, else {\n\
                   change:var:int 'r' = '3'.\n\
               }.\n\
               print[('r')].";
    assert_eq!(compile_and_run("ifchain", src), "2");
}

#[test]
fn functions_compile_and_are_callable() {
    assert_eq!(
        compile_and_run(
            "func",
            "func:int 'square' [var:int 'n'] { handback math { ('n') x ('n') }. }.\n\
             var:int 'a' = 'square'['7'].\n\
             print[('a')]."
        ),
        "49"
    );
}

#[test]
fn a_name_with_spaces_and_emoji_survives_mangling() {
    assert_eq!(
        compile_and_run(
            "mangle",
            "func:int 'my 😂 helper' [var:int 'n'] { handback math { ('n') + 1 }. }.\n\
             var:int 'r' = 'my 😂 helper'['41'].\n\
             print[('r')]."
        ),
        "42"
    );
}

#[test]
fn text_and_values_print_together() {
    assert_eq!(
        compile_and_run(
            "printing",
            "var:int 'x' = '42'.\nprint[\"x is\"].\nprint[('x')]."
        ),
        "x is\n42"
    );
}

// ── what the backend declines, and why ──────────────────────────────────────

#[test]
fn text_values_are_declined() {
    // Strings as values need heap management in the runtime, which is a later stage.
    let what = declines("var:str 's' = \"hello\".\nprint[('s')].");
    assert!(!what.is_empty(), "{what}");
}

#[test]
fn arrays_are_declined() {
    let what = declines("var:vector:int 'a' [3] = {'1','2','3'}.\nprint[('a')].");
    assert!(what.contains("array"), "{what}");
}

#[test]
fn integer_division_producing_a_fraction_is_declined() {
    // 1/3 between two *integers* has no integer result, and the decimal path needs
    // decimal operands.
    let what = declines("var:deci 'r' = math { 1 / 3 }.\nprint[('r')].");
    assert!(!what.is_empty());
}

// ── exact decimals in native code ───────────────────────────────────────────

#[test]
fn decimals_are_exact_in_native_code_too() {
    // The headline guarantee, compiled to machine code rather than interpreted.
    assert_eq!(
        compile_and_run(
            "deciadd",
            "var:deci 'a' = '0.1', 'b' = '0.2'.\n\
             var:deci 's' = math { ('a') + ('b') }.\n\
             print[('s')]."
        ),
        "0.3"
    );
}

#[test]
fn decimal_division_uses_the_true_digits_natively() {
    assert_eq!(
        compile_and_run(
            "decidiv",
            "var:deci 'a' = '58', 'b' = '3'.\n\
             var:deci 'q' = math { ('a') / ('b') }.\n\
             print[('q')]."
        ),
        "19.333333333333333"
    );
}

#[test]
fn decimal_comparison_compiles() {
    assert_eq!(
        compile_and_run(
            "decicmp",
            "var:deci 'a' = '0.1', 'b' = '0.2'.\n\
             var:deci 's' = math { ('a') + ('b') }.\n\
             var:int 'r' = '0'.\n\
             if math { ('s') = '0.3' } { change:var:int 'r' = '1'. }, else { change:var:int 'r' = '2'. }.\n\
             print[('r')]."
        ),
        "1"
    );
}

#[test]
fn an_int_widens_into_a_decimal_context() {
    assert_eq!(
        compile_and_run(
            "deciwiden",
            "var:deci 'a' = '0.5'.\n\
             var:int 'n' = '3'.\n\
             var:deci 's' = math { ('a') + ('n') }.\n\
             print[('s')]."
        ),
        "3.5"
    );
}

// ── native and interpreter must agree ───────────────────────────────────────

#[test]
fn integer_division_is_euclidean_natively() {
    // LLVM's sdiv truncates toward zero; the interpreter is Euclidean. They disagreed
    // on every negative operand until division went through the runtime.
    assert_eq!(
        compile_and_run(
            "eucl",
            "var:int 'a' [32 bit] = math { 0 - 7 }.\n\
             var:int 'b' [32 bit] = '3'.\n\
             var:int 'q' [32 bit] = math { ('a') // ('b') }.\n\
             var:int 'r' [32 bit] = math { ('a') mod ('b') }.\n\
             print[('q')].\nprint[('r')]."
        ),
        "-3\n2"
    );
}

#[test]
fn booleans_print_as_words_natively() {
    assert_eq!(
        compile_and_run("boolp", "var:bool 'a' = 'true'.\nprint[('a')]."),
        "true"
    );
}

#[test]
fn negating_a_decimal_falls_back_rather_than_panicking() {
    // This used to panic in the compiler with "expected the IntValue variant".
    let what = declines("var:deci 'a' [64 bit] = '2.5'.\nvar:deci 'n' [64 bit] = math { -('a') }.");
    assert!(!what.is_empty(), "{what}");
}
