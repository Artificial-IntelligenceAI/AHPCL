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

/// The compiled runtime staticlib. Generated code calls into it for every value that
/// is not a machine word, so a produced binary will not link without it.
///
/// It is built here rather than assumed: a dev-dependency links the *rlib*, so the
/// staticlib can sit stale on disk while the crate itself is up to date. That stale
/// copy fails as undefined symbols at link time, a long way from the cause.
fn runtime_library() -> PathBuf {
    static BUILT: std::sync::Once = std::sync::Once::new();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the workspace root")
        .to_path_buf();

    BUILT.call_once(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "-p", "ahpcl-runtime"])
            .current_dir(&root)
            .status()
            .expect("cargo should run");
        assert!(status.success(), "the runtime staticlib should build");
    });

    let candidate = root.join("target").join("debug").join("libahpcl_runtime.a");
    assert!(candidate.exists(), "expected {}", candidate.display());
    candidate
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
fn a_conditional_used_as_a_value_compiles() {
    assert_eq!(
        compile_and_run(
            "ifval",
            "var:int 'r' = if math { 1 > 0 } { hb '1'. }, else { hb '2'. }.\nprint[('r')]."
        ),
        "1"
    );
}

#[test]
fn a_loop_used_as_a_value_collects_its_handbacks() {
    assert_eq!(
        compile_and_run(
            "loopval",
            "var:vector:int 'squares' = loop:var:int 'i' = math { 1 to 4 } {\n\
                 handback math { ('i') x ('i') }.\n\
             }.\n\
             print[('squares')]."
        ),
        "{1, 4, 9, 16}"
    );
}

// ── text in native code ─────────────────────────────────────────────────────

#[test]
fn text_values_compile() {
    assert_eq!(
        compile_and_run("textval", "var:str 's' = \"hello\".\nprint[('s')]."),
        "hello"
    );
}

#[test]
fn text_comparison_compiles() {
    assert_eq!(
        compile_and_run(
            "textcmp",
            "var:str 'a' = \"Alice\".\n\
             var:bool 'same' = math { ('a') = \"Alice\" }.\n\
             var:bool 'after' = math { ('a') > \"Bob\" }.\n\
             print[('same')].\nprint[('after')]."
        ),
        "true\nfalse"
    );
}

#[test]
fn parsing_compiles_with_its_options() {
    assert_eq!(
        compile_and_run(
            "parseopt",
            "var:int 'n' [32 bit] = parse[\"1,234\" group:\",\"].\n\
             var:deci 'd' [64 bit] = parse[\"  3.25  \" trim].\n\
             var:int 'h' [32 bit] = parse[\"0xff\" hex].\n\
             print[('n')].\nprint[('d')].\nprint[('h')]."
        ),
        "1234\n3.25\n255"
    );
}

// ── exact rationals in native code ──────────────────────────────────────────

#[test]
fn rationals_are_exact_in_native_code() {
    // The guarantee that binary floating point cannot make: three thirds is one.
    assert_eq!(
        compile_and_run(
            "ratexact",
            "var:rat 'a' = math { 1 / 3 }.\n\
             var:rat 'b' = math { ('a') + ('a') + ('a') }.\n\
             print[('a')].\nprint[('b')]."
        ),
        "1/3\n1"
    );
}

#[test]
fn rationals_reduce_natively() {
    // Decimal text, since that is what `parse` accepts — see open question 31 on
    // whether it should also read "2/6".
    assert_eq!(
        compile_and_run(
            "ratreduce",
            "var:rat 'r' [64 bit] = parse[\"0.5\"].\nprint[('r')]."
        ),
        "1/2"
    );
}

#[test]
fn infnum_compiles_and_stays_exact() {
    assert_eq!(
        compile_and_run(
            "infexact",
            "var:infnum 'x' = '1.1'.\n\
             var:infnum 'y' = math { ('x') x ('x') }.\n\
             print[('y')]."
        ),
        "1.21"
    );
}

#[test]
fn division_between_integers_compiles_into_a_decimal() {
    // 1/3 has no integer result, so the decimal path takes it and keeps the digits.
    assert_eq!(
        compile_and_run("intdivdeci", "var:deci 'r' = math { 1 / 3 }.\nprint[('r')]."),
        "0.333333333333333"
    );
}

// ── arrays in native code ───────────────────────────────────────────────────

#[test]
fn arrays_compile_and_print_like_the_interpreter() {
    assert_eq!(
        compile_and_run(
            "arrlit",
            "var:vector:int 'a' [3] = {'1','2','3'}.\nprint[('a')]."
        ),
        "{1, 2, 3}"
    );
}

#[test]
fn the_array_operators_compile() {
    let src = "var:vector:int 'a' [3] = {'1','2','3'}.\n\
               var:vector:int 'b' [3] = {'4','5','6'}.\n\
               var:int 'd' [64 bit] = math { ('a') · ('b') }.\n\
               var:vector:int 'h' [3] = math { ('a') ⊙ ('b') }.\n\
               var:vector:int 'c' [3] = math { ('a') × ('b') }.\n\
               print[('d')].\nprint[('h')].\nprint[('c')].";
    assert_eq!(compile_and_run("arrops", src), "32\n{4, 10, 18}\n{-3, 6, -3}");
}

#[test]
fn array_selectors_compile() {
    let src = "var:vector:int 'a' [5] = {'10','20','30','40','50'}.\n\
               var:int 'n' [64 bit] = ('a'):length;.\n\
               var:int 'third' [64 bit] = ('a'):3;.\n\
               var:vector:int 'odd' [3] = ('a'):1 to 5 by 2;.\n\
               print[('n')].\nprint[('third')].\nprint[('odd')].";
    assert_eq!(compile_and_run("arrsel", src), "5\n30\n{10, 30, 50}");
}

#[test]
fn writing_to_an_array_element_compiles() {
    let src = "var:vector:int 'a' [3] = {'1','2','3'}.\n\
               change:var:int 'a':2; = '99'.\n\
               print[('a')].";
    assert_eq!(compile_and_run("arrset", src), "{1, 99, 3}");
}

#[test]
fn arrays_of_decimals_stay_exact_natively() {
    // `:all;` keeps both sides arrays, so the addition is elementwise. A *bare*
    // reference would sum instead — Rule A — which the next test pins down.
    let src = "var:vector:deci 'a' [2] = {'0.1','0.2'}.\n\
               var:vector:deci 'b' [2] = {'0.2','0.1'}.\n\
               var:vector:deci 's' [2] = math { ('a'):all; + ('b'):all; }.\n\
               print[('s')].";
    assert_eq!(compile_and_run("arrdeci", src), "{0.3, 0.3}");
}

#[test]
fn a_bare_array_reference_sums_natively() {
    // Rule A, and the reason the native backend cannot treat every array operand as
    // elementwise: a bare reference reduces to the total of its elements.
    let src = "var:vector:deci 'a' [2] = {'0.1','0.2'}.\n\
               var:vector:deci 'b' [2] = {'0.2','0.1'}.\n\
               var:deci 's' = math { ('a') + ('b') }.\n\
               print[('s')].";
    assert_eq!(compile_and_run("arrsum", src), "0.6");
}

#[test]
fn matrix_multiplication_compiles() {
    let src = "var:matrix:int 'a' [2, 2] = {{'1','2'},{'3','4'}}.\n\
               var:matrix:int 'b' [2, 2] = {{'5','6'},{'7','8'}}.\n\
               var:matrix:int 'p' [2, 2] = math { ('a') · ('b') }.\n\
               print[('p')].";
    assert_eq!(compile_and_run("matmul", src), "{19, 22, 43, 50}");
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
fn negating_a_decimal_compiles_rather_than_panicking() {
    // This used to panic in the compiler with "expected the IntValue variant", then
    // declined for a while; now it goes through the runtime and gives the answer.
    assert_eq!(
        compile_and_run(
            "decineg",
            "var:deci 'a' [64 bit] = '2.5'.\n\
             var:deci 'n' [64 bit] = math { -('a') }.\n\
             print[('n')]."
        ),
        "-2.5"
    );
}

#[test]
fn square_root_is_exact_natively() {
    assert_eq!(
        compile_and_run(
            "sqrtnat",
            "var:deci 'r' = math { sqrt '2' }.\nprint[('r')]."
        ),
        "1.414213562373095"
    );
}

#[test]
fn the_constants_compile_to_their_declared_precision() {
    assert_eq!(
        compile_and_run(
            "constants",
            "var:deci 'a' [10 digits] = math { pi }.\n\
             var:deci 'b' [5 digits] = math { e }.\n\
             print[('a')].\nprint[('b')]."
        ),
        "3.1415926536\n2.71828"
    );
}

#[test]
fn powers_of_exact_values_compile() {
    // 1.1^20 went wrong from the thirteenth digit through f64; this is the exact answer.
    assert_eq!(
        compile_and_run(
            "exactpow",
            "var:deci 'a' = '1.1'.\n\
             var:deci 'b' = math { ('a') xx 20 }.\n\
             var:rat 'r' = math { 1 / 3 }.\n\
             var:rat 'c' = math { ('r') xx 3 }.\n\
             print[('b')].\nprint[('c')]."
        ),
        "6.72749994932560009201\n1/27"
    );
}

#[test]
fn decimal_division_and_remainder_are_euclidean_natively() {
    // The same rule integers follow, so a negative operand does not split the two
    // implementations apart.
    assert_eq!(
        compile_and_run(
            "decieucl",
            "var:deci 'a' = '-7.5'.\n\
             var:deci 'b' = '2'.\n\
             var:int 'q' [32 bit] = math { ('a') // ('b') }.\n\
             var:deci 'r' = math { ('a') mod ('b') }.\n\
             print[('q')].\nprint[('r')]."
        ),
        "-4\n0.5"
    );
}

#[test]
fn text_and_bool_arrays_compile() {
    assert_eq!(
        compile_and_run(
            "arrmixed",
            "var:vector:str 's' [3] = {\"a\",\"b\",\"c\"}.\n\
             var:vector:bool 'b' [2] = {'true','false'}.\n\
             print[('s')].\nprint[('b')].\nprint[('s'):2;]."
        ),
        "{a, b, c}\n{true, false}\nb"
    );
}

#[test]
fn the_transcendental_operators_compile() {
    // No exact decimal answer exists, so these go through f64 — the same route the
    // interpreter takes, so the two land on the same digits.
    assert_eq!(
        compile_and_run(
            "transcendental",
            "var:deci 'a' [6 digits] = math { cos '0' }.\n\
             var:deci 'b' [6 digits] = math { log '100' }.\n\
             print[('a')].\nprint[('b')]."
        ),
        "1\n2"
    );
}

#[test]
fn reading_a_file_compiles() {
    // `read` takes a path and hands back the whole file, not a line of input.
    let path = workdir().join("read-me.txt");
    std::fs::write(&path, "hello from a file\n").expect("a readable file");
    let src = format!(
        "var:str 'text' = read[\"{}\"].\nprint[('text')].",
        path.display()
    );
    assert_eq!(compile_and_run("readfile", &src), "hello from a file");
}

// ── regressions from the third stress-test pass ─────────────────────────────

#[test]
fn a_loop_counter_reads_correctly_in_an_exact_context() {
    // The counter was registered in `vars` but not `var_types`, so it was read as
    // whatever the surrounding context wanted — an i64 slot loaded as a decimal.
    assert_eq!(
        compile_and_run(
            "loopcounter",
            "loop:var:int 'i' = math { 1 to 3 } {\n\
                 var:deci 'x' = math { ('i') / 4 }.\n\
                 print[('x')].\n\
             }."
        ),
        "0.25\n0.5\n0.75"
    );
}

#[test]
fn a_declaration_inside_a_loop_does_not_leak_out_of_it() {
    // The counted loop pushed a `vars` frame without a matching `var_types` frame, so
    // an inner declaration overwrote the outer variable's recorded type.
    assert_eq!(
        compile_and_run(
            "loopscope",
            "var:int 'x' [32 bit] = '7'.\n\
             loop:var:int 'i' = math { 1 to 1 } {\n\
                 var:deci 'x' = '1.5'.\n\
                 print[('x')].\n\
             }.\n\
             print[('x')]."
        ),
        "1.5\n7"
    );
}

#[test]
fn a_bare_array_reference_sums_even_on_its_own() {
    // Rule A applied only inside binary operators, so a bare reference standing alone
    // handed back the array pointer as an integer.
    assert_eq!(
        compile_and_run(
            "barealone",
            "var:vector:int 'a' [4] = {'1','2','3','4'}.\n\
             var:int 's' [64 bit] = math { ('a') }.\n\
             print[('s')]."
        ),
        "10"
    );
}

#[test]
fn a_selector_addresses_a_dimension_not_the_flat_buffer() {
    // `('m'):2;` is the second row of a matrix, not its second element.
    let src = "var:matrix:int 'm' [3,4] = {{'1','2','3','4'},{'5','6','7','8'},{'9','10','11','12'}}.\n\
               var:vector:int 'row' [4] = ('m'):2;.\n\
               var:int 'cell' [64 bit] = ('m'):2;:3;.\n\
               print[('row')].\nprint[('cell')].";
    assert_eq!(compile_and_run("matsel", src), "{5, 6, 7, 8}\n7");
}

#[test]
fn an_elementwise_unary_keeps_the_array() {
    // `:all;` keeps it an array for unary operators too, not only binary ones.
    assert_eq!(
        compile_and_run(
            "unaryall",
            "var:vector:int 'a' [3] = {'1','2','3'}.\n\
             var:vector:int 'b' [3] = math { -('a'):all; }.\n\
             print[('b')]."
        ),
        "{-1, -2, -3}"
    );
}

#[test]
fn integers_are_128_bit_natively() {
    // The backend emitted i64, so anything past ~9.2×10¹⁸ wrapped silently while the
    // interpreter kept computing. Both values checked against Python.
    assert_eq!(
        compile_and_run(
            "int128",
            "var:int 'a' [128 bit] = '9223372036854775807'.\n\
             var:int 'b' [128 bit] = math { ('a') + ('a') }.\n\
             print[('b')]."
        ),
        "18446744073709551614"
    );
    assert_eq!(
        compile_and_run(
            "int128mul",
            "var:int 'a' [128 bit] = '99999999999'.\n\
             var:int 'b' [128 bit] = math { ('a') x ('a') }.\n\
             print[('b')]."
        ),
        "9999999999800000000001"
    );
}

#[test]
fn a_literal_may_use_the_whole_int_range() {
    // A literal above i64::MAX used to decline native compilation outright.
    assert_eq!(
        compile_and_run(
            "bigliteral",
            "var:int 'm' [128 bit] = '170141183460469231731687303715884105727'.\nprint[('m')]."
        ),
        "170141183460469231731687303715884105727"
    );
}

#[test]
fn elementwise_comparison_compares_the_operands_not_the_result() {
    // The result element is `bool`, but the operands are not: coercing them to the
    // wanted element compared `('a'):all;` against `true`, read back as 1, so every
    // operator silently used the wrong right-hand side.
    assert_eq!(
        compile_and_run(
            "arraycmp",
            "var:vector:int 'a' [3] = {'1','2','3'}.\n\
             var:vector:bool 'b' [3] = math { ('a'):all; > 2 }.\n\
             print[('b')]."
        ),
        "{false, false, true}"
    );
}

#[test]
fn parse_reads_a_fraction_when_asked() {
    assert_eq!(
        compile_and_run(
            "parsefrac",
            "var:rat 'r' [64 bit] = parse[\"2/6\" fraction].\nprint[('r')]."
        ),
        "1/3"
    );
}
