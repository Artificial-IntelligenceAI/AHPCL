//! Parser tests. Each pins a decision from docs/syntax.md.

use ahpcl_syntax::ast::*;
use ahpcl_syntax::parse_source;

fn program(src: &str) -> Program {
    let (p, errors) = parse_source(src);
    assert!(
        errors.is_empty(),
        "unexpected errors: {:#?}",
        errors.iter().map(|e| (e.code.render(), e.rule_conditions.clone())).collect::<Vec<_>>()
    );
    p
}

fn errors(src: &str) -> Vec<String> {
    parse_source(src).1.into_iter().map(|e| e.code.render()).collect()
}

/// Parse a `math { … }` expression and hand back the inner tree.
fn math_expr(src: &str) -> Expr {
    let p = program(&format!("var:num 'r' = math {{ {src} }}."));
    let Stmt::Var(v) = &p.statements[0] else { panic!("expected a declaration") };
    let value = v.bindings[0].value.clone().expect("a value");
    match value.kind {
        ExprKind::Math(inner) => *inner,
        other => panic!("expected a math block, got {other:?}"),
    }
}

/// Render an expression as fully-bracketed text, so precedence is visible.
fn shape_of(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Number(n) => n.clone(),
        ExprKind::Literal(v) => format!("'{v}'"),
        ExprKind::Ref { name, .. } => format!("({name})"),
        ExprKind::Binary { op, lhs, rhs } => {
            format!("({} {:?} {})", shape_of(lhs), op, shape_of(rhs))
        }
        ExprKind::Unary { op, operand } => format!("({:?} {})", op, shape_of(operand)),
        ExprKind::Math(inner) => shape_of(inner),
        ExprKind::Range { from, to, by } => match by {
            Some(b) => format!("({} to {} by {})", shape_of(from), shape_of(to), shape_of(b)),
            None => format!("({} to {})", shape_of(from), shape_of(to)),
        },
        other => format!("{other:?}"),
    }
}

// ── declarations ────────────────────────────────────────────────────────────

#[test]
fn a_declaration_parses() {
    let p = program("var:num 'x' = '1000'.");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    assert_eq!(v.ty.base, "num");
    assert_eq!(v.bindings.len(), 1);
    assert_eq!(v.bindings[0].name, "x");
}

#[test]
fn a_comma_extends_a_declaration_into_separate_variables() {
    let p = program("var:num 'x' = '1000', 'y' = '2000'.");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    assert_eq!(v.bindings.len(), 2);
    assert_eq!(v.bindings[0].name, "x");
    assert_eq!(v.bindings[1].name, "y");
    // The type comes from the shared header.
    assert_eq!(v.ty.base, "num");
}

#[test]
fn precision_sits_with_each_name() {
    let p = program("var:int 'x' [32 bit] = '1000', 'y' [8 bit] = '20'.");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    assert_eq!(v.bindings[0].precision, Some(Precision::Bits(32)));
    assert_eq!(v.bindings[1].precision, Some(Precision::Bits(8)));
}

#[test]
fn shape_comes_before_precision() {
    let p = program("var:matrix:num 'm' [3, 4] [32 bit] = {'1'}.");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    assert_eq!(v.bindings[0].shape, Some(vec![Dim::Known(3), Dim::Known(4)]));
    assert_eq!(v.bindings[0].precision, Some(Precision::Bits(32)));
    assert_eq!(v.ty.rank, Some(Rank::Matrix));
}

#[test]
fn unknown_dimensions_parse() {
    let p = program("var:matrix:num 'd' [?, 3] = read[\"m.csv\"].");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    assert_eq!(v.bindings[0].shape, Some(vec![Dim::Unknown, Dim::Known(3)]));
}

#[test]
fn digits_precision_is_for_irrationals() {
    let p = program("var:infnum 'x' [100 digits] = math { pi }.");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    assert_eq!(v.bindings[0].precision, Some(Precision::Digits(100)));
}

#[test]
fn a_sign_refines_the_element_type_of_an_array() {
    let p = program("var:vector:+num 'widths' [3] = {'3', '5', '2'}.");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    assert_eq!(v.ty.rank, Some(Rank::Vector));
    assert_eq!(v.ty.base, "num");
    assert_eq!(v.ty.sign, Some(Sign::Positive), "every element is positive");
}

#[test]
fn sign_refinements_parse() {
    let p = program("var:+int 'n' = '10'.");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    assert_eq!(v.ty.sign, Some(Sign::Positive));
    assert_eq!(v.ty.base, "int");
}

#[test]
fn an_unknown_type_is_reported() {
    assert!(errors("var:wibble 'x' = '1'.").contains(&"AHPCL-TYPE-0010".to_string()));
}

// ── change ──────────────────────────────────────────────────────────────────

#[test]
fn a_change_restates_the_type() {
    let p = program("change:var:num 'x' = '2000'.");
    let Stmt::Change(c) = &p.statements[0] else { panic!() };
    assert_eq!(c.ty.base, "num");
    assert_eq!(c.targets[0].name, "x");
}

#[test]
fn a_change_may_target_one_element() {
    let p = program("change:var:num 'a':3; = '99'.");
    let Stmt::Change(c) = &p.statements[0] else { panic!() };
    assert_eq!(c.targets[0].selectors.len(), 1);
}

#[test]
fn a_comma_extends_a_change_like_every_other_statement() {
    let p = program("change:var:int 'x' = '1', 'y' = '2'.");
    let Stmt::Change(c) = &p.statements[0] else { panic!() };
    assert_eq!(c.targets.len(), 2);
    assert_eq!(c.targets[0].name, "x");
    assert_eq!(c.targets[1].name, "y");
}

#[test]
fn builtin_options_are_words_not_values() {
    let p = program("var:deci 'a' = parse[\" 42 \" trim].");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    let ExprKind::Builtin { args, .. } = &v.bindings[0].value.as_ref().unwrap().kind else {
        panic!()
    };
    assert!(matches!(&args[1].kind, ExprKind::Option { name, value: None } if name == "trim"));
}

#[test]
fn builtin_options_may_carry_a_value() {
    let p = program("var:deci 'a' = parse[\"1,000\" group:\",\"].");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    let ExprKind::Builtin { args, .. } = &v.bindings[0].value.as_ref().unwrap().kind else {
        panic!()
    };
    assert!(matches!(&args[1].kind, ExprKind::Option { name, value: Some(_) } if name == "group"));
}

// ── precedence, the part most likely to be wrong ─────────────────────────────

#[test]
fn multiplication_binds_before_addition() {
    assert_eq!(shape_of(&math_expr("2 + 3 * 4")), "(2 Add (3 Mul 4))");
}

#[test]
fn unary_minus_binds_below_powers() {
    // -x² is -(x²), so -3 xx 2 is -9 rather than 9. This is what mathematics says,
    // and it is the entry that makes the precedence table non-obvious.
    assert_eq!(shape_of(&math_expr("-3 xx 2")), "(Neg (3 Pow 2))");
}

#[test]
fn unary_minus_binds_above_multiplication() {
    assert_eq!(shape_of(&math_expr("-3 * 2")), "((Neg 3) Mul 2)");
}

#[test]
fn powers_group_right_to_left() {
    // 2^3^2 is 2^(3^2) = 512, not (2^3)^2 = 64.
    assert_eq!(shape_of(&math_expr("2 xx 3 xx 2")), "(2 Pow (3 Pow 2))");
}

#[test]
fn subtraction_and_division_group_left_to_right() {
    assert_eq!(shape_of(&math_expr("10 - 3 - 2")), "((10 Sub 3) Sub 2)");
    assert_eq!(shape_of(&math_expr("100 / 5 / 2")), "((100 Div 5) Div 2)");
}

#[test]
fn comparison_binds_looser_than_arithmetic() {
    assert_eq!(shape_of(&math_expr("1 + 2 < 4")), "((1 Add 2) Less 4)");
}

#[test]
fn logic_binds_loosest_of_all() {
    assert_eq!(
        shape_of(&math_expr("1 < 2 and 3 < 4")),
        "((1 Less 2) And (3 Less 4))"
    );
}

#[test]
fn sqrt_takes_only_the_thing_after_it() {
    // √x + 1 is (√x) + 1, matching the radical's bar in typeset maths.
    assert_eq!(shape_of(&math_expr("sqrt 9 + 1")), "((Sqrt 9) Add 1)");
    assert_eq!(shape_of(&math_expr("√ 9 + 1")), "((Sqrt 9) Add 1)");
}

#[test]
fn absolute_value_is_self_delimiting() {
    assert_eq!(shape_of(&math_expr("|0 - 5| + 1")), "((Abs (0 Sub 5)) Add 1)");
}

#[test]
fn x_is_multiplication_only_when_spaced() {
    assert_eq!(shape_of(&math_expr("2 x 4")), "(2 Mul 4)");
}

#[test]
fn ranges_sit_below_arithmetic() {
    assert_eq!(shape_of(&math_expr("1 to 100 by 2")), "(1 to 100 by 2)");
}

// ── references and selectors ────────────────────────────────────────────────

#[test]
fn references_are_always_quoted() {
    let e = math_expr("('x') + 1");
    assert_eq!(shape_of(&e), "((x) Add 1)");
}

#[test]
fn grouping_is_told_from_a_reference_by_the_quotes() {
    assert_eq!(shape_of(&math_expr("(3 + 4) * 2")), "((3 Add 4) Mul 2)");
}

#[test]
fn selectors_chain_one_per_dimension() {
    let e = math_expr("('m'):1, 3;:2, 4;");
    let ExprKind::Ref { selectors, .. } = &e.kind else { panic!("{e:?}") };
    assert_eq!(selectors.len(), 2);
    assert!(matches!(&selectors[0], Selector::Indices(v) if v.len() == 2));
}

#[test]
fn selector_keywords_parse() {
    for (src, want) in [
        ("('a'):all;", Selector::All),
        ("('a'):length;", Selector::Length),
        ("('a'):shape;", Selector::Shape),
    ] {
        let e = math_expr(src);
        let ExprKind::Ref { selectors, .. } = &e.kind else { panic!() };
        assert_eq!(selectors[0], want);
    }
}

#[test]
fn a_selector_range_parses() {
    let e = math_expr("('a'):1 to 100;");
    let ExprKind::Ref { selectors, .. } = &e.kind else { panic!() };
    assert!(matches!(&selectors[0], Selector::Range { .. }));
}

#[test]
fn a_missing_semicolon_is_reported() {
    assert!(!parse_source("var:num 'y' = math { ('a'):1 }.").1.is_empty());
}

// ── control flow ────────────────────────────────────────────────────────────

#[test]
fn an_if_else_chain_is_one_statement_extended_by_commas() {
    let p = program(
        "if math { ('x') > 5 } {\n\
             print[\"big\"].\n\
         }, else if math { ('x') > 3 } {\n\
             print[\"medium\"].\n\
         }, else {\n\
             print[\"small\"].\n\
         }.",
    );
    assert_eq!(p.statements.len(), 1);
    let Stmt::If(chain) = &p.statements[0] else { panic!() };
    assert_eq!(chain.arms.len(), 3);
    assert!(chain.arms[0].condition.is_some());
    assert!(chain.arms[1].condition.is_some());
    assert!(chain.arms[2].condition.is_none(), "the final else has no condition");
}

#[test]
fn a_conditional_can_produce_a_value() {
    let p = program(
        "var:num 'abs' = if math { ('x') < 0 } { handback math { -('x') }. }, else { hb ('x'). }.",
    );
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    assert!(matches!(v.bindings[0].value.as_ref().unwrap().kind, ExprKind::If(_)));
}

#[test]
fn both_loop_kinds_parse() {
    let p = program("loop:var:int 'i' = math { 1 to 10 } { print[\"hi\"]. }.");
    let Stmt::Loop(l) = &p.statements[0] else { panic!() };
    assert!(matches!(l.kind, LoopKind::Counted { .. }));

    let p = program("loop:while math { ('n') > 1 } { print[\"hi\"]. }.");
    let Stmt::Loop(l) = &p.statements[0] else { panic!() };
    assert!(matches!(l.kind, LoopKind::While { .. }));
}

#[test]
fn a_loop_can_produce_an_array() {
    let p = program(
        "var:vector:num 'squares' = loop:var:int 'i' = math { 1 to 10 } { handback math { ('i') xx 2 }. }.",
    );
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    assert!(matches!(v.bindings[0].value.as_ref().unwrap().kind, ExprKind::Loop(_)));
}

// ── functions ───────────────────────────────────────────────────────────────

#[test]
fn a_function_parses_with_parameters_as_declarations() {
    let p = program(
        "func:num 'area' [var:+num 'width', 'height'] {\n\
             handback math { ('width') x ('height') }.\n\
         }.",
    );
    let Stmt::Func(f) = &p.statements[0] else { panic!() };
    assert_eq!(f.name, "area");
    assert_eq!(f.returns.base, "num");
    assert_eq!(f.params.len(), 2, "a comma extends the parameter list");
    assert_eq!(f.params[0].ty.sign, Some(Sign::Positive));
    assert_eq!(f.params[1].name, "height");
}

#[test]
fn a_parameter_may_carry_a_shape_and_a_precision() {
    let p = program("func:num 'total' [var:matrix:num 'data' [?, 3] [32 bit]] { hb '0'. }.");
    let Stmt::Func(f) = &p.statements[0] else { panic!() };
    assert_eq!(f.params[0].shape, Some(vec![Dim::Unknown, Dim::Known(3)]));
    assert_eq!(f.params[0].precision, Some(Precision::Bits(32)));
}

#[test]
fn a_function_producing_nothing_uses_none() {
    let p = program("func:none 'log' [var:str 'message'] { print[('message')]. }.");
    let Stmt::Func(f) = &p.statements[0] else { panic!() };
    assert_eq!(f.returns.base, "none");
}

#[test]
fn a_quoted_name_with_brackets_is_a_call() {
    let p = program("var:num 'a' = 'area'['3' '4'].");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    let ExprKind::Call { name, args } = &v.bindings[0].value.as_ref().unwrap().kind else {
        panic!()
    };
    assert_eq!(name, "area");
    assert_eq!(args.len(), 2, "arguments are space-separated, no commas");
}

#[test]
fn bare_means_builtin_and_quoted_means_user_defined() {
    let p = program("var:str 'raw' = read[\"data.csv\"].");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    assert!(matches!(
        v.bindings[0].value.as_ref().unwrap().kind,
        ExprKind::Builtin { .. }
    ));

    let p = program("var:str 'raw' = 'read'[\"data.csv\"].");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    assert!(matches!(
        v.bindings[0].value.as_ref().unwrap().kind,
        ExprKind::Call { .. }
    ));
}

// ── print ───────────────────────────────────────────────────────────────────

#[test]
fn print_takes_text_and_references() {
    let p = program("print[\"The variable is \" ('x') \".\"].");
    let Stmt::Print { args, .. } = &p.statements[0] else { panic!() };
    assert_eq!(args.len(), 3);
}

#[test]
fn print_refuses_a_math_block() {
    // Compute into a variable first, then print that.
    let errs = parse_source("print[\"Total: \" math { 1 + 2 }].").1;
    assert!(!errs.is_empty(), "a math block is not a print argument");
}

// ── array literals ──────────────────────────────────────────────────────────

#[test]
fn array_literals_nest_to_mirror_the_shape() {
    let p = program("var:matrix:num 'm' [2, 2] = {{'1', '2'}, {'3', '4'}}.");
    let Stmt::Var(v) = &p.statements[0] else { panic!() };
    let ExprKind::ArrayLit(rows) = &v.bindings[0].value.as_ref().unwrap().kind else { panic!() };
    assert_eq!(rows.len(), 2);
    let ExprKind::ArrayLit(first) = &rows[0].kind else { panic!() };
    assert_eq!(first.len(), 2);
}

// ── whole programs ──────────────────────────────────────────────────────────

#[test]
fn the_stats_sample_parses() {
    let p = program(
        "#3 Statistics over a set of measurements.\n\
            Values are literal; file input is not designed yet.\n\
            This is the sample the design was tested against.\n\
         \n\
         func:deci 'mean' [var:vector:num 'values' [?]] {\n\
             handback math { ('values') / ('values'):length; }.\n\
         }.\n\
         \n\
         var:vector:num 'data' [6] = {'2', '4', '4', '4', '5', '9'}.\n\
         var:deci 'avg' = 'mean'[('data')].\n\
         \n\
         print[\"Mean: \" ('avg')].\n",
    );
    assert_eq!(p.statements.len(), 4);
}

#[test]
fn recovery_keeps_going_after_a_bad_statement() {
    // One broken statement should not cascade.
    let (p, errs) = parse_source("var:num 'x' = .\nvar:num 'y' = '2'.\n");
    assert!(!errs.is_empty());
    assert!(
        p.statements.iter().any(|s| matches!(s, Stmt::Var(v) if v.bindings[0].name == "y")),
        "the second declaration still parsed"
    );
}
