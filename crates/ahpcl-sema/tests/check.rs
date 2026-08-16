//! Type-checker tests. Each pins a decision from docs/types.md.

use ahpcl_diagnostics::Informer;
use ahpcl_sema::check;
use ahpcl_syntax::parse_source;

/// Type-check a program, returning the error codes it produced.
fn codes(src: &str) -> Vec<String> {
    let (program, parse_errors) = parse_source(src);
    assert!(
        parse_errors.is_empty(),
        "the test source should parse: {:#?}",
        parse_errors.iter().map(|e| (e.code.render(), e.what_went_wrong.clone())).collect::<Vec<_>>()
    );
    let mut informer = Informer::new();
    check(&program, &mut informer)
        .errors
        .into_iter()
        .map(|e| e.code.render())
        .collect()
}

fn clean(src: &str) {
    let got = codes(src);
    assert!(got.is_empty(), "expected no errors, got {got:?}");
}

fn rejects(src: &str, code: &str) {
    let got = codes(src);
    assert!(got.iter().any(|c| c == code), "expected {code}, got {got:?}");
}

/// Informer notes produced while checking.
fn notes(src: &str) -> String {
    let (program, _) = parse_source(src);
    let mut informer = Informer::new();
    check(&program, &mut informer);
    informer.render(&ahpcl_diagnostics::SourceFile::new("t.ahpcl", src))
}

// ── literals are pinned by context ──────────────────────────────────────────

#[test]
fn a_literal_takes_its_type_from_the_declaration() {
    clean("var:deci 'a' = '0.1'.");
    clean("var:int 'n' = '1000'.");
}

#[test]
fn a_fraction_cannot_land_in_an_int() {
    rejects("var:int 'n' = '0.5'.", "AHPCL-TYPE-0002");
}

#[test]
fn division_needs_a_concrete_type() {
    // num spans rat and deci, so it does not pin the result.
    rejects(
        "var:num 'x' = math { 1 / 3 }.",
        "AHPCL-TYPE-0001",
    );
    clean("var:deci 'x' = math { 1 / 3 }.");
    clean("var:rat 'x' = math { 1 / 3 }.");
}

// ── the hierarchy ───────────────────────────────────────────────────────────

#[test]
fn narrower_types_pass_into_wider_ones() {
    clean(
        "func:num 'f' [var:num 'v'] { handback ('v'). }.\n\
         var:int 'w' = '3'.\n\
         var:num 'a' = 'f'[('w')].",
    );
}

#[test]
fn a_plain_num_cannot_satisfy_a_positive_one() {
    // A num might be negative, so it cannot keep a +num promise.
    rejects(
        "func:num 'f' [var:+num 'v'] { handback ('v'). }.\n\
         var:num 'w' = '3'.\n\
         var:num 'a' = 'f'[('w')].",
        "AHPCL-TYPE-0002",
    );
}

#[test]
fn every_widening_is_reported() {
    let out = notes(
        "func:num 'f' [var:num 'v'] { handback ('v'). }.\n\
         var:int 'w' = '3'.\n\
         var:num 'a' = 'f'[('w')].",
    );
    assert!(out.contains("widened"), "{out}");
}

#[test]
fn text_and_numbers_do_not_mix() {
    rejects("var:num 'x' = \"hello\".", "AHPCL-TYPE-0002");
}

// ── shapes ──────────────────────────────────────────────────────────────────

#[test]
fn a_rank_name_must_agree_with_its_shape() {
    rejects("var:matrix:num 'm' [3] = {'1', '2', '3'}.", "AHPCL-SHAPE-0002");
    clean("var:matrix:num 'm' [2, 2] = {{'1', '2'}, {'3', '4'}}.");
}

#[test]
fn tensor_is_for_three_dimensions_or_more() {
    rejects("var:tensor:num 't' [3, 4] = {{'1'}}.", "AHPCL-SHAPE-0002");
}

#[test]
fn a_literal_must_match_the_declared_shape() {
    rejects(
        "var:matrix:num 'm' [3, 2] = {{'1', '2'}, {'3', '4'}}.",
        "AHPCL-SHAPE-0003",
    );
}

#[test]
fn inner_dimensions_must_agree_for_matrix_multiplication() {
    rejects(
        "var:matrix:num 'a' [3, 4] = {{'1'}}.\n\
         var:matrix:num 'b' [5, 2] = {{'1'}}.\n\
         var:matrix:num 'c' = math { ('a') · ('b') }.",
        "AHPCL-SHAPE-0001",
    );
}

#[test]
fn a_partial_shape_still_catches_a_known_mismatch() {
    // [?, 3] · [4, 2] — nobody knows the row count, but 3 and 4 contradict.
    rejects(
        "func:none 'f' [var:matrix:num 'data' [?, 3]] {\n\
             var:matrix:num 'w' [4, 2] = {{'1'}}.\n\
             var:matrix:num 'r' = math { ('data') · ('w') }.\n\
         }.",
        "AHPCL-SHAPE-0001",
    );
}

#[test]
fn cross_product_needs_two_three_element_vectors() {
    rejects(
        "var:vector:num 'a' [4] = {'1','2','3','4'}.\n\
         var:vector:num 'b' [4] = {'1','2','3','4'}.\n\
         var:vector:num 'c' = math { ('a') × ('b') }.",
        "AHPCL-SHAPE-0001",
    );
}

#[test]
fn elementwise_needs_matching_shapes() {
    rejects(
        "var:vector:num 'a' [3] = {'1','2','3'}.\n\
         var:vector:num 'b' [4] = {'1','2','3','4'}.\n\
         var:vector:num 'c' = math { ('a') ⊙ ('b') }.",
        "AHPCL-SHAPE-0001",
    );
}

// ── names and scope ─────────────────────────────────────────────────────────

#[test]
fn an_unknown_name_is_reported_with_a_suggestion() {
    let (program, _) = parse_source("var:num 'widths' = '1'.\nvar:num 'y' = ('width').");
    let mut informer = Informer::new();
    let errors = check(&program, &mut informer).errors;
    assert_eq!(errors[0].code.render(), "AHPCL-NAME-0001");
    assert!(errors[0].suggest_fix.contains("widths"), "{}", errors[0].suggest_fix);
}

#[test]
fn a_variable_does_not_outlive_its_block() {
    rejects(
        "var:bool 'c' = 'true'.\n\
         if ('c') {\n\
             var:num 'y' = '5'.\n\
         }.\n\
         var:num 'z' = ('y').",
        "AHPCL-NAME-0001",
    );
}

#[test]
fn shadowing_is_legal_and_reported() {
    let out = notes(
        "var:num 'y' = '1'.\n\
         var:bool 'c' = 'true'.\n\
         if ('c') {\n\
             var:num 'y' = '5'.\n\
         }.",
    );
    assert!(out.contains("shadows"), "{out}");
}

// ── functions ───────────────────────────────────────────────────────────────

#[test]
fn a_call_checks_its_arguments() {
    rejects(
        "func:num 'f' [var:num 'a', 'b'] { handback ('a'). }.\n\
         var:num 'r' = 'f'['1'].",
        "AHPCL-NAME-0003",
    );
}

#[test]
fn an_unknown_function_is_reported_with_a_suggestion() {
    let (program, _) = parse_source(
        "func:num 'area' [var:num 'a'] { handback ('a'). }.\n\
         var:num 'r' = 'aria'['1'].",
    );
    let mut informer = Informer::new();
    let errors = check(&program, &mut informer).errors;
    assert!(errors.iter().any(|e| e.suggest_fix.contains("area")), "{errors:#?}");
}

#[test]
fn a_function_promising_a_value_must_hand_one_back() {
    rejects(
        "func:num 'f' [var:num 'a'] { print[\"nothing\"]. }.",
        "AHPCL-TYPE-0002",
    );
}

#[test]
fn a_none_function_need_not_hand_anything_back() {
    clean("func:none 'log' [var:str 'm'] { print[('m')]. }.");
}

#[test]
fn functions_are_visible_before_their_declaration() {
    clean(
        "var:num 'r' = 'f'['1'].\n\
         func:num 'f' [var:num 'a'] { handback ('a'). }.",
    );
}

// ── change ──────────────────────────────────────────────────────────────────

#[test]
fn a_restated_type_must_match_the_declaration() {
    // The restated type is documentation, so it is verified — documentation that can
    // drift out of sync is worse than none.
    rejects(
        "var:num 'x' = '1000'.\nchange:var:int 'x' = '2000'.",
        "AHPCL-TYPE-0004",
    );
    clean("var:num 'x' = '1000'.\nchange:var:num 'x' = '2000'.");
}

#[test]
fn changing_an_undeclared_variable_is_reported() {
    rejects("change:var:num 'x' = '1'.", "AHPCL-NAME-0001");
}

#[test]
fn a_loop_counter_is_read_only() {
    rejects(
        "loop:var:int 'i' = math { 1 to 10 } {\n\
             change:var:int 'i' = '99'.\n\
         }.",
        "AHPCL-SIGN-0002",
    );
}

// ── conditions ──────────────────────────────────────────────────────────────

#[test]
fn a_condition_must_be_a_bool() {
    rejects(
        "var:num 'x' = '1'.\nif ('x') { print[\"hi\"]. }.",
        "AHPCL-TYPE-0002",
    );
    clean("var:num 'x' = '1'.\nif math { ('x') > 0 } { print[\"hi\"]. }.");
}

// ── precision ───────────────────────────────────────────────────────────────

#[test]
fn infnum_takes_digits_not_bits() {
    rejects("var:infnum 'x' [64 bit] = '1'.", "AHPCL-PREC-0002");
    // Within what AHPCL knows: asking for more places than that is its own error,
    // checked below.
    clean("var:infnum 'x' [30 digits] = math { pi }.");
}

#[test]
fn deci_widths_follow_ieee() {
    rejects("var:deci 'x' [8 bit] = '0.1'.", "AHPCL-PREC-0003");
    clean("var:deci 'x' [128 bit] = '0.1'.");
}

// ── irrational results ──────────────────────────────────────────────────────

#[test]
fn an_irrational_result_cannot_land_in_an_int() {
    rejects("var:int 'r' = math { sqrt 2 }.", "AHPCL-TYPE-0002");
}

#[test]
fn a_deci_may_hold_a_rounded_irrational_and_says_so() {
    let out = notes("var:deci 'r' = math { sqrt 2 }.");
    assert!(out.contains("irrational"), "{out}");
    assert!(out.contains("rounded"), "{out}");
}

// ── whole programs ──────────────────────────────────────────────────────────

#[test]
fn the_showcase_example_checks_clean() {
    let src = include_str!("../../../examples/showcase.ahpcl");
    let got = codes(src);
    assert!(got.is_empty(), "showcase should check clean, got {got:?}");
}

#[test]
fn the_stats_example_checks_clean() {
    let src = include_str!("../../../examples/stats.ahpcl");
    let got = codes(src);
    assert!(got.is_empty(), "stats should check clean, got {got:?}");
}

// ── regressions found by the stress-testing pass (2026-08-12) ───────────────

#[test]
fn a_literal_must_satisfy_its_sign_refinement() {
    // A refinement is a promise; a literal that breaks it should never reach runtime.
    rejects("var:+int 'n' = '-5'.", "AHPCL-SIGN-0001");
    // Zero lives only in the unprefixed types.
    rejects("var:+int 'n' = '0'.", "AHPCL-SIGN-0001");
    rejects("var:-int 'n' = '5'.", "AHPCL-SIGN-0001");
    clean("var:+int 'n' = '5'.");
}

#[test]
fn a_literal_must_fit_the_stated_width() {
    rejects("var:int 'x' [8 bit] = '1000'.", "AHPCL-PREC-0004");
    clean("var:int 'x' [8 bit] = '100'.");
    // A +int cannot be negative, so the sign bit is free: 1 … 255.
    clean("var:+int 'x' [8 bit] = '200'.");
}

#[test]
fn only_the_offered_widths_are_accepted() {
    rejects("var:int 'x' [7 bit] = '5'.", "AHPCL-PREC-0005");
    clean("var:int 'x' [16 bit] = '5'.");
}

#[test]
fn division_cannot_land_in_an_int() {
    // syntax.md: `//` is how truncation is requested.
    rejects("var:int 'q' = math { 10 / 4 }.", "AHPCL-TYPE-0002");
    clean("var:int 'q' = math { 10 // 4 }.");
}

#[test]
fn a_conditional_used_for_its_value_needs_an_else() {
    rejects(
        "var:int 'x' = '0'.\n\
         var:int 'v' = if math { ('x') > 5 } { handback '5'. }.",
        "AHPCL-TYPE-0002",
    );
    clean(
        "var:int 'x' = '0'.\n\
         var:int 'v' = if math { ('x') > 5 } { handback '5'. }, else { handback '0'. }.",
    );
}

#[test]
fn elementwise_operations_keep_their_shape() {
    clean(
        "var:vector:int 'a' [3] = {'1','2','3'}.\n\
         var:vector:int 'u' [3] = math { ('a'):all; + 1 }.",
    );
    // Mismatched shapes are still caught.
    rejects(
        "var:vector:int 'a' [3] = {'1','2','3'}.\n\
         var:vector:int 'b' [4] = {'1','2','3','4'}.\n\
         var:vector:int 'c' [3] = math { ('a'):all; + ('b'):all; }.",
        "AHPCL-SHAPE-0001",
    );
}

#[test]
fn a_range_selector_reports_the_length_it_selects() {
    clean(
        "var:vector:int 'a' [5] = {'10','20','30','40','50'}.\n\
         var:vector:int 'o' [3] = math { ('a'):1 to 5 by 2; }.",
    );
}

#[test]
fn too_many_selectors_for_the_rank_is_an_error() {
    rejects(
        "var:vector:int 'a' [3] = {'1','2','3'}.\n\
         var:int 'v' = math { ('a'):1;:1; }.",
        "AHPCL-SHAPE-0001",
    );
}

#[test]
fn asking_for_more_places_than_are_known_is_rejected() {
    // types.md: an irrational computed past what AHPCL knows is an error, not a silent
    // approximation. It used to be caught at run time for constants and not at all for
    // square roots, which quietly returned 18 places under a request for 30.
    rejects("var:infnum 'x' [40 digits] = math { pi }.", "AHPCL-PREC-0004");
    rejects("var:infnum 'x' [30 digits] = math { sqrt 2 }.", "AHPCL-PREC-0004");
    clean("var:infnum 'x' [18 digits] = math { sqrt 2 }.");
}

#[test]
fn naming_one_array_as_another_is_refused() {
    // Two readings, no syntax to tell them apart: `'b'` could be an independent copy or
    // a second name for the same array. The interpreter copied, the backend aliased, and
    // neither was wrong because nothing had decided. Refused rather than guessed.
    rejects(
        "var:vector:int 'a' [3] = {'1','2','3'}.\nvar:vector:int 'b' = ('a').",
        "AHPCL-TYPE-0006",
    );
    rejects(
        "var:vector:int 'a' [3] = {'1','2','3'}.\n\
         var:vector:int 'b' [3] = {'0','0','0'}.\n\
         change:var:vector:int 'b' = ('a').",
        "AHPCL-TYPE-0006",
    );
    // `:all;` says copy, and is unaffected.
    clean("var:vector:int 'a' [3] = {'1','2','3'}.\nvar:vector:int 'b' [3] = ('a'):all;.");
    // Rule A: summing a bare reference into a scalar is a different thing entirely.
    clean("var:vector:int 'a' [3] = {'1','2','3'}.\nvar:int 's' [64 bit] = math { ('a') }.");
}
