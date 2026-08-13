//! End-to-end tests: source in, output out.

use ahpcl_eval::run;
use ahpcl_syntax::parse_source;

/// Run a program and hand back what it printed.
fn output(src: &str) -> Vec<String> {
    let (program, errors) = parse_source(src);
    assert!(errors.is_empty(), "should parse: {errors:#?}");
    let out = run(&program);
    assert!(
        out.error.is_none(),
        "should run cleanly: {:?}",
        out.error.map(|e| e.rule_conditions)
    );
    out.lines
}

fn one(src: &str) -> String {
    output(src).join("\n")
}

/// Run a program expected to fail, returning the error code.
fn fails(src: &str) -> String {
    let (program, errors) = parse_source(src);
    assert!(errors.is_empty(), "should parse: {errors:#?}");
    let out = run(&program);
    out.error.expect("expected a runtime failure").code.render()
}

// ── the exactness guarantees ────────────────────────────────────────────────

#[test]
fn point_one_plus_point_two_is_exactly_point_three() {
    // The headline case. In binary floating point this prints 0.30000000000000004.
    assert_eq!(
        one("var:deci 'a' = '0.1', 'b' = '0.2'.\n\
             var:deci 's' = math { ('a') + ('b') }.\n\
             print[('s')]."),
        "0.3"
    );
}

#[test]
fn the_comparison_agrees_too() {
    assert_eq!(
        one("var:deci 'a' = '0.1', 'b' = '0.2'.\n\
             var:deci 's' = math { ('a') + ('b') }.\n\
             var:bool 'q' = math { ('s') = '0.3' }.\n\
             print[('q')]."),
        "true"
    );
}

#[test]
fn a_third_is_exact_and_three_of_them_make_one() {
    assert_eq!(
        one("var:rat 't' = math { 1 / 3 }.\nprint[('t')]."),
        "1/3"
    );
    assert_eq!(
        one("var:rat 't' = math { 1 / 3 }.\n\
             var:rat 'w' = math { ('t') + ('t') + ('t') }.\n\
             print[('w')]."),
        "1"
    );
}

#[test]
fn an_exact_root_stays_exact() {
    assert_eq!(one("var:deci 'r' = math { sqrt 9 }.\nprint[('r')]."), "3");
}

// ── arithmetic and precedence ───────────────────────────────────────────────

#[test]
fn precedence_matches_mathematics_at_runtime() {
    // -3² is -9, not 9: the power binds tighter than the minus.
    assert_eq!(one("var:int 'r' = math { -3 xx 2 }.\nprint[('r')]."), "-9");
    // 2^3^2 is 2^(3^2) = 512, because powers group right to left.
    assert_eq!(one("var:int 'r' = math { 2 xx 3 xx 2 }.\nprint[('r')]."), "512");
    // Multiplication before addition.
    assert_eq!(one("var:int 'r' = math { 2 + 3 * 4 }.\nprint[('r')]."), "14");
}

#[test]
fn integer_division_and_remainder() {
    assert_eq!(one("var:int 'r' = math { 10 // 4 }.\nprint[('r')]."), "2");
    assert_eq!(one("var:int 'r' = math { 10 mod 4 }.\nprint[('r')]."), "2");
}

#[test]
fn absolute_value_uses_its_real_notation() {
    assert_eq!(one("var:int 'r' = math { |0 - 5| }.\nprint[('r')]."), "5");
}

#[test]
fn division_by_zero_fails_rather_than_producing_infinity() {
    assert_eq!(
        fails("var:deci 'r' = math { 1 / 0 }.\nprint[('r')]."),
        "AHPCL-RUN-0002"
    );
}

#[test]
fn overflow_is_reported_rather_than_wrapping() {
    assert_eq!(
        fails(
            "var:int 'big' = '170141183460469231731687303715884105727'.\n\
             var:int 'r' = math { ('big') + 1 }.\n\
             print[('r')]."
        ),
        "AHPCL-PREC-0004"
    );
}

// ── arrays ──────────────────────────────────────────────────────────────────

#[test]
fn a_bare_array_reference_sums_its_elements() {
    assert_eq!(
        one("var:vector:int 'a' [3] = {'1', '2', '3'}.\n\
             var:int 's' = math { ('a') }.\n\
             print[('s')]."),
        "6"
    );
}

#[test]
fn one_index_gives_a_plain_value() {
    assert_eq!(
        one("var:vector:int 'a' [3] = {'10', '20', '30'}.\n\
             var:int 'v' = math { ('a'):2; }.\n\
             print[('v')]."),
        "20"
    );
}

#[test]
fn indices_are_one_based() {
    assert_eq!(
        one("var:vector:int 'a' [3] = {'10', '20', '30'}.\n\
             var:int 'v' = math { ('a'):1; }.\n\
             print[('v')]."),
        "10"
    );
}

#[test]
fn length_and_shape_answer_different_questions() {
    assert_eq!(
        one("var:matrix:int 'm' [2, 3] = {{'1','2','3'}, {'4','5','6'}}.\n\
             var:int 'n' = math { ('m'):length; }.\n\
             print[('n')]."),
        "6"
    );
    assert_eq!(
        one("var:matrix:int 'm' [2, 3] = {{'1','2','3'}, {'4','5','6'}}.\n\
             var:vector:int 's' = math { ('m'):shape; }.\n\
             print[('s')]."),
        "{2, 3}"
    );
}

#[test]
fn the_dot_product_collapses_to_one_number() {
    assert_eq!(
        one("var:vector:int 'a' [3] = {'1','2','3'}.\n\
             var:vector:int 'b' [3] = {'4','5','6'}.\n\
             var:int 'd' = math { ('a') · ('b') }.\n\
             print[('d')]."),
        "32"
    );
}

#[test]
fn the_cross_product_gives_a_perpendicular_vector() {
    assert_eq!(
        one("var:vector:int 'a' [3] = {'1','2','3'}.\n\
             var:vector:int 'b' [3] = {'4','5','6'}.\n\
             var:vector:int 'c' = math { ('a') × ('b') }.\n\
             print[('c')]."),
        "{-3, 6, -3}"
    );
}

#[test]
fn elementwise_keeps_the_elements_separate() {
    assert_eq!(
        one("var:vector:int 'a' [3] = {'1','2','3'}.\n\
             var:vector:int 'b' [3] = {'4','5','6'}.\n\
             var:vector:int 'c' = math { ('a') ⊙ ('b') }.\n\
             print[('c')]."),
        "{4, 10, 18}"
    );
}

#[test]
fn matrix_multiplication_is_the_same_operator_as_the_dot_product() {
    assert_eq!(
        one("var:matrix:int 'a' [2, 2] = {{'1','2'}, {'3','4'}}.\n\
             var:matrix:int 'b' [2, 2] = {{'5','6'}, {'7','8'}}.\n\
             var:matrix:int 'c' = math { ('a') · ('b') }.\n\
             print[('c')]."),
        "{19, 22, 43, 50}"
    );
}

#[test]
fn an_out_of_range_index_is_caught() {
    assert_eq!(
        fails(
            "var:vector:int 'a' [3] = {'1','2','3'}.\n\
             var:int 'v' = math { ('a'):9; }.\n\
             print[('v')]."
        ),
        "AHPCL-RUN-0003"
    );
}

// ── control flow ────────────────────────────────────────────────────────────

#[test]
fn a_conditional_produces_a_value() {
    assert_eq!(
        one("var:int 'x' = math { 0 - 5 }.\n\
             var:int 'a' = if math { ('x') < 0 } { handback math { -('x') }. }, else { hb ('x'). }.\n\
             print[('a')]."),
        "5"
    );
}

#[test]
fn an_else_if_chain_picks_the_right_arm() {
    let src = "var:int 'x' = '4'.\n\
               var:str 'r' = if math { ('x') > 5 } { hb \"big\". }, \
                             else if math { ('x') > 3 } { hb \"medium\". }, \
                             else { hb \"small\". }.\n\
               print[('r')].";
    assert_eq!(one(src), "medium");
}

#[test]
fn a_loop_builds_an_array_one_handback_at_a_time() {
    assert_eq!(
        one("var:vector:int 'sq' = loop:var:int 'i' = math { 1 to 5 } {\n\
                 handback math { ('i') xx 2 }.\n\
             }.\n\
             print[('sq')]."),
        "{1, 4, 9, 16, 25}"
    );
}

#[test]
fn a_loop_step_is_honoured() {
    assert_eq!(
        one("var:vector:int 'o' = loop:var:int 'i' = math { 1 to 9 by 2 } { hb ('i'). }.\n\
             print[('o')]."),
        "{1, 3, 5, 7, 9}"
    );
}

#[test]
fn nesting_loops_builds_a_matrix() {
    assert_eq!(
        one("var:matrix:int 't' = loop:var:int 'i' = math { 1 to 2 } {\n\
                 handback loop:var:int 'j' = math { 1 to 3 } {\n\
                     handback math { ('i') x ('j') }.\n\
                 }.\n\
             }.\n\
             print[('t')]."),
        "{1, 2, 3, 2, 4, 6}"
    );
}

#[test]
fn a_condition_loop_runs_until_it_is_false() {
    assert_eq!(
        one("var:int 'n' = '5'.\n\
             loop:while math { ('n') > 0 } {\n\
                 change:var:int 'n' = math { ('n') - 1 }.\n\
             }.\n\
             print[('n')]."),
        "0"
    );
}

// ── functions ───────────────────────────────────────────────────────────────

#[test]
fn a_function_is_called_with_different_values() {
    assert_eq!(
        one("func:int 'area' [var:+int 'w', 'h'] { handback math { ('w') x ('h') }. }.\n\
             var:int 'k' = 'area'['3' '4'].\n\
             var:int 'b' = 'area'['5' '6'].\n\
             print[('k')].\n\
             print[('b')]."),
        "12\n30"
    );
}

#[test]
fn changing_a_variable_works() {
    assert_eq!(
        one("var:int 'x' = '1000'.\n\
             change:var:int 'x' = '2000'.\n\
             print[('x')]."),
        "2000"
    );
}

#[test]
fn writing_to_one_element_leaves_the_rest_alone() {
    assert_eq!(
        one("var:vector:int 'a' [3] = {'10', '20', '30'}.\n\
             change:var:int 'a':2; = '99'.\n\
             print[('a')]."),
        "{10, 99, 30}"
    );
}

// ── scope ───────────────────────────────────────────────────────────────────

#[test]
fn a_block_variable_does_not_escape() {
    assert_eq!(
        fails(
            "var:bool 'c' = 'true'.\n\
             if ('c') { var:int 'y' = '5'. }.\n\
             print[('y')]."
        ),
        "AHPCL-RUN-0001"
    );
}

// ── printing ────────────────────────────────────────────────────────────────

#[test]
fn print_juxtaposes_its_arguments() {
    assert_eq!(
        one("var:int 'x' = '42'.\nprint[\"x is \" ('x') \".\"]."),
        "x is 42."
    );
}

#[test]
fn unicode_names_work_end_to_end() {
    assert_eq!(
        one("var:str '😂' = \"laughing\".\n\
             var:str '.' = \"!\".\n\
             print[\"Bro, I'm \" ('😂') ('.')]."),
        "Bro, I'm laughing!"
    );
}

// ── regressions found by the stress-testing pass (2026-08-12) ───────────────

#[test]
fn all_makes_an_operation_elementwise_rather_than_summing() {
    // The rule was only half implemented: `:all;` was ignored and every array
    // operation silently reduced to a sum.
    assert_eq!(
        one("var:vector:int 'a' [3] = {'1','2','3'}.\n\
             var:vector:int 'u' [3] = math { ('a'):all; + 1 }.\n\
             print[('u')]."),
        "{2, 3, 4}"
    );
    assert_eq!(
        one("var:vector:int 'a' [3] = {'1','2','3'}.\n\
             var:vector:int 'b' [3] = {'4','5','6'}.\n\
             var:vector:int 'v' [3] = math { ('a'):all; + ('b'):all; }.\n\
             print[('v')]."),
        "{5, 7, 9}"
    );
}

#[test]
fn a_bare_reference_still_sums() {
    assert_eq!(
        one("var:vector:int 'a' [3] = {'1','2','3'}.\n\
             var:int 's' = math { ('a') + 1 }.\n\
             print[('s')]."),
        "7"
    );
}

#[test]
fn all_and_the_hadamard_operator_agree() {
    // types.md: `('a'):all; x ('b'):all;` and `('a') ⊙ ('b')` are the same operation.
    let spelled_out = one("var:vector:int 'a' [3] = {'1','2','3'}.\n\
                           var:vector:int 'b' [3] = {'4','5','6'}.\n\
                           var:vector:int 'c' [3] = math { ('a'):all; x ('b'):all; }.\n\
                           print[('c')].");
    let symbol = one("var:vector:int 'a' [3] = {'1','2','3'}.\n\
                      var:vector:int 'b' [3] = {'4','5','6'}.\n\
                      var:vector:int 'c' [3] = math { ('a') ⊙ ('b') }.\n\
                      print[('c')].");
    assert_eq!(spelled_out, symbol);
    assert_eq!(spelled_out, "{4, 10, 18}");
}

#[test]
fn chained_selectors_address_successive_dimensions() {
    // syntax.md: `('m'):all;:2;` is "every row, column 2" — not row 2 again.
    assert_eq!(
        one("var:matrix:int 'm' [2, 2] = {{'1','2'},{'3','4'}}.\n\
             var:vector:int 'c' [2] = math { ('m'):all;:2; }.\n\
             print[('c')]."),
        "{2, 4}"
    );
    assert_eq!(
        one("var:matrix:int 'm' [2, 2] = {{'1','2'},{'3','4'}}.\n\
             var:int 'v' = math { ('m'):2;:1; }.\n\
             print[('v')]."),
        "3"
    );
}

#[test]
fn a_range_selector_has_the_length_it_selects() {
    assert_eq!(
        one("var:vector:int 'a' [5] = {'10','20','30','40','50'}.\n\
             var:vector:int 'o' [3] = math { ('a'):1 to 5 by 2; }.\n\
             print[('o')]."),
        "{10, 30, 50}"
    );
}

#[test]
fn truncating_division_truncates_and_mod_gives_the_remainder() {
    assert_eq!(
        one("var:deci 'a' = '7.5'.\nvar:int 'q' = math { ('a') // 2 }.\nprint[('q')]."),
        "3"
    );
    assert_eq!(
        one("var:deci 'a' = '7.5'.\nvar:deci 'r' = math { ('a') mod 2 }.\nprint[('r')]."),
        "1.5"
    );
}

#[test]
fn integer_powers_of_rationals_stay_exact() {
    // Was 111111/1000000, from a round trip through f64.
    assert_eq!(
        one("var:rat 't' = math { 1 / 3 }.\n\
             var:rat 's' = math { ('t') xx 2 }.\n\
             print[('s')]."),
        "1/9"
    );
}

#[test]
fn decimal_division_and_powers_use_the_true_digits() {
    // Was 19.333333333333332 — the f64 bit pattern rather than the real value.
    assert_eq!(
        one("var:deci 'a' = math { 58 / 3 }.\nprint[('a')]."),
        "19.333333333333333"
    );
    // 1.1^20 is exact; f64 was wrong from the 13th decimal place.
    assert!(
        one("var:deci 'p' = math { 1.1 xx 20 }.\nprint[('p')].")
            .starts_with("6.7274999493256000"),
    );
}

#[test]
fn summing_an_array_does_not_wrap_on_overflow() {
    assert_eq!(
        fails(
            "var:vector:int 'v' [2] = {'170141183460469231731687303715884105727','1'}.\n\
             var:int 's' = math { ('v') + 0 }.\n\
             print[('s')]."
        ),
        "AHPCL-PREC-0004"
    );
}

#[test]
fn decimal_division_and_remainder_by_zero_are_errors() {
    assert_eq!(
        fails("var:deci 'a' = '7.5'.\nvar:int 'r' = math { ('a') // 0 }.\nprint[('r')]."),
        "AHPCL-RUN-0002"
    );
    assert_eq!(
        fails("var:deci 'a' = '7.5'.\nvar:deci 'r' = math { ('a') mod 0 }.\nprint[('r')]."),
        "AHPCL-RUN-0002"
    );
}

#[test]
fn a_loop_whose_handbacks_disagree_on_shape_is_an_error_not_a_panic() {
    let src = "var:matrix:int 'm' [3, 3] = loop:var:int 'i' = math { 1 to 3 } {\n\
                   handback loop:var:int 'j' = math { 1 to math { 4 - ('i') } } {\n\
                       handback math { ('j') }.\n\
                   }.\n\
               }.\n\
               print[('m')].";
    assert_eq!(fails(src), "AHPCL-RUN-0001");
}

#[test]
fn parse_options_are_honoured() {
    assert_eq!(one("var:deci 'a' = parse[\" 42 \" trim].\nprint[('a')]."), "42");
    assert_eq!(one("var:deci 'a' = parse[\"1e5\" scientific].\nprint[('a')]."), "100000");
    assert_eq!(one("var:int 'a' = parse[\"0x2A\" hex].\nprint[('a')]."), "42");
    assert_eq!(one("var:int 'a' = parse[\"๔๒\" unicode-digits].\nprint[('a')]."), "42");
}

#[test]
fn stating_the_convention_removes_the_thousands_ambiguity() {
    // "1,000" means one thousand in Britain and one in Germany. Saying which is which
    // is how the ambiguity is removed rather than guessed at.
    assert_eq!(
        one("var:deci 'a' = parse[\"1,000.5\" group:\",\" decimal:\".\"].\nprint[('a')]."),
        "1000.5"
    );
    assert_eq!(
        one("var:deci 'a' = parse[\"1.000,5\" group:\".\" decimal:\",\"].\nprint[('a')]."),
        "1000.5"
    );
}

#[test]
fn nna_holds_text_without_being_summed() {
    assert_eq!(
        one("var:nna 'names' = {\"hello\", \"John Doe\"}.\nprint[('names')]."),
        "{hello, John Doe}"
    );
}

#[test]
fn a_comma_extends_a_change() {
    assert_eq!(
        one("var:int 'x' = '1', 'y' = '2'.\n\
             change:var:int 'x' = '10', 'y' = '20'.\n\
             print[('x')].\nprint[('y')]."),
        "10\n20"
    );
}

#[test]
fn a_matrix_times_a_vector_gives_a_vector() {
    assert_eq!(
        one("var:matrix:int 'm' [2, 3] = {{'1','2','3'},{'4','5','6'}}.\n\
             var:vector:int 'v' [3] = {'1','1','1'}.\n\
             var:vector:int 'r' [2] = math { ('m') · ('v') }.\n\
             print[('r')]."),
        "{6, 15}"
    );
}

#[test]
fn constants_are_exact_rather_than_computed_from_a_float() {
    // An f64 carries about 16 significant digits; beyond that it was noise.
    assert_eq!(
        one("var:infnum 'a' [30 digits] = math { pi }.\nprint[('a')]."),
        "3.14159265358979323846264338328"
    );
}

#[test]
fn square_roots_are_exact_to_the_digits_asked_for() {
    // √2 = 1.41421356237309504880168872420969807…
    assert_eq!(
        one("var:deci 'a' [128 bit] = math { sqrt 2 }.\nprint[('a')]."),
        "1.414213562373095048"
    );
}

#[test]
fn a_declared_decimal_width_limits_the_digits() {
    // decimal32 holds 7 significant digits, so that is what it shows.
    assert_eq!(
        one("var:deci 'a' [32 bit] = math { sqrt 2 }.\nprint[('a')]."),
        "1.4142135"
    );
}

// ── regressions from the second stress-testing pass (2026-08-12) ────────────

#[test]
fn a_broken_refinement_actually_stops_the_program() {
    // Layer 3 previously announced a runtime check and inserted nothing, so a +int
    // could hold a negative number and the program ran to completion.
    std::fs::create_dir_all("/tmp/ahpcl-regress").ok();
    std::fs::write("/tmp/ahpcl-regress/neg.txt", "-7").unwrap();
    assert_eq!(
        fails(
            "var:str 'raw' = read[\"/tmp/ahpcl-regress/neg.txt\"].\n\
             var:+int 'n' [32 bit] = parse[('raw')].\n\
             print[('n')]."
        ),
        "AHPCL-SIGN-0004"
    );
}

#[test]
fn a_stated_width_is_enforced_while_running() {
    assert_eq!(
        fails("var:int 'x' [8 bit] = '100'.\nprint[('x')].".replace("100", "1000").as_str()),
        "AHPCL-PREC-0004"
    );
}

#[test]
fn a_refined_value_can_be_compared_against_zero() {
    // `math { ('n') > 0 }` on a +int was rejected, which made the documented idiom
    // for keeping a +int positive unwritable.
    assert_eq!(
        one("var:+int 'n' [32 bit] = '5'.\n\
             var:bool 'b' = math { ('n') > 0 }.\n\
             print[('b')]."),
        "true"
    );
}

#[test]
fn a_lone_bare_array_reference_sums() {
    assert_eq!(
        one("var:vector:deci 'a' [3] = {'1.5','2.5','3.0'}.\n\
             var:deci 't' = math { ('a') }.\n\
             print[('t')]."),
        "7"
    );
}

#[test]
fn floor_and_ceil_pin_a_bare_literal() {
    assert_eq!(one("var:int 'h' [32 bit] = math { floor 5 }.\nprint[('h')]."), "5");
    assert_eq!(one("var:int 'h' [32 bit] = math { ceil 5 }.\nprint[('h')]."), "5");
}

#[test]
fn mod_on_a_decimal_keeps_the_fraction() {
    assert_eq!(
        one("var:deci 'a' [64 bit] = '2.5'.\n\
             var:deci 'm' [64 bit] = math { ('a') mod 2 }.\n\
             print[('m')]."),
        "0.5"
    );
}

#[test]
fn a_loop_counter_may_carry_a_precision() {
    assert_eq!(
        one("loop:var:int 'i' [32 bit] = math { 1 to 3 } { print[('i')]. }."),
        "1\n2\n3"
    );
}

#[test]
fn a_functions_return_type_is_the_context_its_handback_divides_in() {
    // Division follows the context, and a function's declared return type *is* the
    // context. Without that, this handed back 0.333333333333333 from a `rat`.
    assert_eq!(
        one(
            "func:rat 'third' [var:rat 'x'] { hb math { ('x') / '3' }. }.\n\
             var:rat 'r' = 'third'['1'].\nprint[('r')]."
        ),
        "1/3"
    );
}

#[test]
fn assigning_a_bare_array_reference_to_a_scalar_sums_it() {
    // `change:` skipped the reduction a declaration performs, so a whole array went
    // into an `int` variable.
    assert_eq!(
        one(
            "var:vector:int 'a' [3] = {'1','2','3'}.\n\
             var:int 's' [64 bit] = '0'.\n\
             change:var:int 's' = math { ('a') }.\nprint[('s')]."
        ),
        "6"
    );
}

#[test]
fn a_unary_operator_on_an_all_selector_stays_elementwise() {
    // Reducing unconditionally let a `vector` variable hold a single value, which the
    // checker had already typed as an array.
    assert_eq!(
        one(
            "var:vector:int 'a' [3] = {'1','2','3'}.\n\
             var:vector:int 'b' [3] = math { -('a'):all; }.\nprint[('b')]."
        ),
        "{-1, -2, -3}"
    );
}
