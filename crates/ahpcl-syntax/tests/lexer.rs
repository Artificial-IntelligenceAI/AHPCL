//! Lexer tests. Each one pins a decision from docs/syntax.md.

use ahpcl_syntax::{lex, TokenKind};

fn kinds(src: &str) -> Vec<TokenKind> {
    let out = lex(src);
    assert!(
        out.errors.is_empty(),
        "unexpected errors: {:?}",
        out.errors.iter().map(|e| e.rule_conditions.clone()).collect::<Vec<_>>()
    );
    out.tokens.into_iter().map(|t| t.kind).filter(|k| *k != TokenKind::Eof).collect()
}

fn errors(src: &str) -> Vec<String> {
    lex(src).errors.into_iter().map(|e| e.code.render()).collect()
}

// ── names and values ────────────────────────────────────────────────────────

#[test]
fn names_may_contain_anything_but_the_delimiters() {
    assert_eq!(kinds("'x'"), vec![TokenKind::Quoted("x".into())]);
    assert_eq!(kinds("'my variable'"), vec![TokenKind::Quoted("my variable".into())]);
    assert_eq!(kinds("'😂'"), vec![TokenKind::Quoted("😂".into())]);
    assert_eq!(kinds("'ความเร็ว'"), vec![TokenKind::Quoted("ความเร็ว".into())]);
    assert_eq!(kinds("'2x'"), vec![TokenKind::Quoted("2x".into())]);
    // A variable named '.' — the case that forced always-quoted references.
    assert_eq!(kinds("'.'"), vec![TokenKind::Quoted(".".into())]);
}

#[test]
fn only_the_delimiters_need_escaping() {
    assert_eq!(kinds(r"'it\'s'"), vec![TokenKind::Quoted("it's".into())]);
    assert_eq!(kinds(r"'back\\slash'"), vec![TokenKind::Quoted(r"back\slash".into())]);
    assert_eq!(kinds(r#""she said \"hi\"""#), vec![TokenKind::Str(r#"she said "hi""#.into())]);
}

#[test]
fn an_unknown_escape_is_an_error() {
    assert_eq!(errors(r"'a\nb'"), vec!["AHPCL-LEX-0003"]);
}

#[test]
fn an_unclosed_quote_is_an_error() {
    assert_eq!(errors("'x"), vec!["AHPCL-LEX-0002"]);
    assert_eq!(errors("\"x"), vec!["AHPCL-LEX-0002"]);
}

// ── the two lexer modes ─────────────────────────────────────────────────────

#[test]
fn dot_terminates_outside_math_and_is_a_decimal_point_inside() {
    // Outside: a statement terminator.
    assert_eq!(
        kinds("'x'."),
        vec![TokenKind::Quoted("x".into()), TokenKind::Dot]
    );

    // Inside: part of the number, and the trailing dot is still a terminator
    // because it sits outside the braces.
    assert_eq!(
        kinds("math { 1.5 }."),
        vec![
            TokenKind::Word("math".into()),
            TokenKind::MathOpen,
            TokenKind::Number("1.5".into()),
            TokenKind::RBrace,
            TokenKind::Dot,
        ]
    );
}

#[test]
fn leading_dot_decimals_are_legal_inside_math() {
    assert_eq!(
        kinds("math { .5 }"),
        vec![
            TokenKind::Word("math".into()),
            TokenKind::MathOpen,
            TokenKind::Number(".5".into()),
            TokenKind::RBrace,
        ]
    );
}

#[test]
fn a_bare_number_outside_math_is_an_error() {
    // Values are quoted outside math { }.
    assert_eq!(errors("var:num 'x' = 1000."), vec!["AHPCL-LEX-0005"]);
}

#[test]
fn bare_numbers_are_fine_in_shapes_and_precision() {
    let out = lex("var:matrix:num 'm' [3, 4] [32 bit] = {'1'}.");
    assert!(out.errors.is_empty(), "{:?}", out.errors);
}

#[test]
fn array_literal_braces_do_not_open_math_mode() {
    // A bare `{` is a literal or block, not math — so a dot inside it still terminates.
    let out = lex("var:vector:num 'v' [2] = {'1', '2'}.");
    assert!(out.errors.is_empty(), "{:?}", out.errors);
}

// ── the `x` whitespace rule ─────────────────────────────────────────────────

#[test]
fn x_is_multiplication_only_with_a_space_on_each_side() {
    let out = lex("math { 2 x 4 }");
    let x = out
        .tokens
        .iter()
        .find(|t| t.word() == Some("x"))
        .expect("found the x token");
    assert!(x.is_spaced_x(), "2 x 4 is multiplication");

    // `2x4` lexes as a number followed by the word `x4`, which is not an operator.
    let out = lex("math { 2x4 }");
    let words: Vec<_> = out.tokens.iter().filter_map(|t| t.word()).collect();
    assert!(words.contains(&"x4"), "2x4 does not produce a multiplication: {words:?}");
    assert!(!out.tokens.iter().any(|t| t.is_spaced_x()));
}

// ── comments ────────────────────────────────────────────────────────────────

#[test]
fn a_bare_hash_comments_one_line() {
    assert_eq!(
        kinds("# note\n'x'"),
        vec![TokenKind::Quoted("x".into())]
    );
}

#[test]
fn a_number_is_a_total_line_count() {
    // #3 covers this line and the two below.
    assert_eq!(
        kinds("#3 one\ntwo\nthree\n'x'"),
        vec![TokenKind::Quoted("x".into())]
    );
}

#[test]
fn plus_counts_additional_lines() {
    // #+3 covers this line and the three below — four in total.
    assert_eq!(
        kinds("#+3 one\ntwo\nthree\nfour\n'x'"),
        vec![TokenKind::Quoted("x".into())]
    );
}

#[test]
fn digits_after_hash_are_always_a_count() {
    // "#3 bugs remaining" comments three lines, as the rule says. The space is the
    // programmer's responsibility.
    assert_eq!(
        kinds("#3 bugs remaining\n'eaten'\n'also eaten'\n'x'"),
        vec![TokenKind::Quoted("x".into())]
    );
    // With a space it is an ordinary one-line comment.
    assert_eq!(
        kinds("# 3 bugs remaining\n'x'"),
        vec![TokenKind::Quoted("x".into())]
    );
}

#[test]
fn overrunning_the_end_of_the_file_is_an_error() {
    assert_eq!(errors("#10 too many\nsecond\nthird\n"), vec!["AHPCL-LEX-0001"]);
}

// ── operators ───────────────────────────────────────────────────────────────

#[test]
fn ascii_and_unicode_operators_both_lex() {
    assert_eq!(
        kinds("math { 1 / 2 ÷ 3 }"),
        vec![
            TokenKind::Word("math".into()),
            TokenKind::MathOpen,
            TokenKind::Number("1".into()),
            TokenKind::Slash,
            TokenKind::Number("2".into()),
            TokenKind::Slash,
            TokenKind::Number("3".into()),
            TokenKind::RBrace,
        ]
    );
}

#[test]
fn the_reserved_array_operators_are_distinct() {
    let ks = kinds("math { · × ⊙ ⊗ √ }");
    assert!(ks.contains(&TokenKind::DotProduct));
    assert!(ks.contains(&TokenKind::CrossProduct));
    assert!(ks.contains(&TokenKind::Hadamard));
    assert!(ks.contains(&TokenKind::TensorProduct));
    assert!(ks.contains(&TokenKind::Sqrt));
}

#[test]
fn two_character_operators_beat_one_character_ones() {
    let ks = kinds("math { 1 // 2 ** 3 <= 4 >= 5 != 6 }");
    assert!(ks.contains(&TokenKind::SlashSlash));
    assert!(ks.contains(&TokenKind::StarStar));
    assert!(ks.contains(&TokenKind::LessEq));
    assert!(ks.contains(&TokenKind::GreaterEq));
    assert!(ks.contains(&TokenKind::NotEq));
}

#[test]
fn question_marks_mark_unknown_dimensions() {
    // [?, 3] — unknown rows, three columns. The partial-shape case.
    let out = lex("var:matrix:num 'data' [?, 3] = read[\"m.csv\"].");
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert!(out.tokens.iter().any(|t| t.kind == TokenKind::Question));
}

// ── lookalike characters ────────────────────────────────────────────────────

#[test]
fn a_pasted_minus_sign_is_named_precisely() {
    let out = lex("math { 5 \u{2212} 3 }");
    assert_eq!(out.errors.len(), 1);
    let e = &out.errors[0];
    assert_eq!(e.code.render(), "AHPCL-LEX-0004");
    assert!(e.rule_conditions.contains("U+2212 MINUS SIGN"));
    assert!(e.suggest_fix.contains("did you mean '-'"));
    assert!(e.suggest_fix.contains("copying from a web page"));
}

#[test]
fn a_no_break_space_is_caught_rather_than_silently_ignored() {
    let out = lex("math { 5 +\u{00A0}3 }");
    assert_eq!(out.errors.len(), 1);
    assert!(out.errors[0].rule_conditions.contains("NO-BREAK SPACE"));
}
