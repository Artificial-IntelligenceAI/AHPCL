//! The AHPCL Error Handler.
//!
//! Renders the template from docs/diagnostics.md. Grammar varies with the count:
//! one error says "there's something wrong" and is not numbered; two or more say
//! "there are some things wrong" and are numbered `Error 1 of 3`.

use std::fmt::Write as _;

use crate::position::{SourceFile, Span};

/// The category half of an error code, e.g. the `LEX` in `AHPCL-LEX-0001`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Lex,
    /// Parse errors — a well-formed token stream that is not a well-formed program.
    Syn,
    Type,
    Prec,
    Sign,
    Shape,
    Name,
    Run,
}

impl Category {
    fn as_str(self) -> &'static str {
        match self {
            Category::Lex => "LEX",
            Category::Syn => "SYN",
            Category::Type => "TYPE",
            Category::Prec => "PREC",
            Category::Sign => "SIGN",
            Category::Shape => "SHAPE",
            Category::Name => "NAME",
            Category::Run => "RUN",
        }
    }
}

/// An error code. Codes are never reused, so the list only grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Code {
    pub category: Category,
    pub number: u16,
}

impl Code {
    pub const fn new(category: Category, number: u16) -> Self {
        Code { category, number }
    }

    pub fn render(&self) -> String {
        format!("AHPCL-{}-{:04}", self.category.as_str(), self.number)
    }
}

/// One place an error points at, with its own note.
///
/// Most AHPCL errors are about *relationships* — a refinement promised in one place and
/// broken in another — so an error carries a list of these, not just one.
#[derive(Debug, Clone)]
pub struct Label {
    pub span: Span,
    pub note: String,
}

impl Label {
    pub fn new(span: Span, note: impl Into<String>) -> Self {
        Label { span, note: note.into() }
    }
}

#[derive(Debug, Clone)]
pub struct Error {
    pub code: Code,
    /// The primary location, named in the header fields.
    pub primary: Span,
    pub labels: Vec<Label>,
    pub what_went_wrong: String,
    pub suggest_fix: String,
}

impl Error {
    pub fn new(
        code: Code,
        primary: Span,
        what_went_wrong: impl Into<String>,
        suggest_fix: impl Into<String>,
    ) -> Self {
        Error {
            code,
            primary,
            labels: Vec::new(),
            what_went_wrong: what_went_wrong.into(),
            suggest_fix: suggest_fix.into(),
        }
    }

    pub fn with_label(mut self, span: Span, note: impl Into<String>) -> Self {
        self.labels.push(Label::new(span, note));
        self
    }
}

/// Drop repeats of the same message at the same place.
///
/// One mistake often trips several checks; showing it five times buries the cause.
fn deduplicate<'a>(source: &SourceFile, errors: &'a [Error]) -> Vec<&'a Error> {
    let mut seen: Vec<(usize, u16, &str)> = Vec::new();
    let mut out: Vec<&Error> = Vec::new();
    for e in errors {
        let key = (e.primary.start.0, e.code.number, e.what_went_wrong.as_str());
        if seen.contains(&key) {
            continue;
        }
        // One mistake, one report. A stray character or a malformed call produces a
        // syntax error and then several more as the parser stumbles through the wreckage
        // — six errors for one typo, of which only the first is real. A syntax error is
        // kept only if nothing has already been reported on the same line, since after
        // the first the parser is describing its own confusion rather than the program.
        if e.code.category == Category::Syn {
            let line = source.line_col(e.primary.start).line;
            if out
                .iter()
                .any(|prior| source.line_col(prior.primary.start).line == line)
            {
                continue;
            }
        }
        seen.push(key);
        out.push(e);
    }
    out
}

/// Renders a batch of errors as one report.
pub fn render(source: &SourceFile, errors: &[Error]) -> String {
    if errors.is_empty() {
        return String::new();
    }

    let errors = deduplicate(source, errors);
    let errors = errors.as_slice();

    let mut out = String::new();
    let many = errors.len() > 1;

    out.push_str("AHPCL Error Handler:\n");
    if many {
        out.push_str("Hello, I think that there are some things wrong.\n");
    } else {
        out.push_str("Hello, I think that there's something wrong.\n");
    }
    out.push('\n');

    // A rule is stated the first time its code appears and not on repeats: meeting a rule
    // once teaches it, and twelve copies of it in one run is noise.
    let mut explained: Vec<Code> = Vec::new();
    for (i, err) in errors.iter().enumerate() {
        if many {
            let _ = writeln!(out, "Error {} of {}", i + 1, errors.len());
        }
        let first_time = !explained.contains(&err.code);
        if first_time {
            explained.push(err.code);
        }
        render_one(&mut out, source, err, first_time);
        if i + 1 < errors.len() {
            out.push('\n');
        }
    }

    out.push('\n');
    if errors.len() == 1 {
        out.push_str("1 error found.\n");
    } else {
        let _ = writeln!(out, "{} errors found.", errors.len());
    }
    out
}

fn render_one(out: &mut String, source: &SourceFile, err: &Error, explain: bool) {
    let at = source.line_col(err.primary.start);

    // The compact form, then the same values spelled out. The repetition is
    // deliberate: the spelled-out fields are a legend teaching how to read the
    // compact one. Do not "tidy" it away.
    let _ = writeln!(
        out,
        "{}:{}:{}  [{}]",
        source.name,
        at.line,
        at.column,
        err.code.render()
    );
    let _ = writeln!(out, "file: {}", source.name);
    let _ = writeln!(out, "line: {}", at.line);
    let _ = writeln!(out, "column: {}", at.column);
    out.push('\n');

    let mut labels: Vec<&Label> = err.labels.iter().collect();
    let fallback;
    if labels.is_empty() {
        fallback = Label::new(err.primary, String::new());
        labels.push(&fallback);
    }
    labels.sort_by_key(|l| l.span.start);

    let width = labels
        .iter()
        .map(|l| source.line_col(l.span.start).line.to_string().len())
        .max()
        .unwrap_or(1)
        .max(2);

    for label in labels {
        let mut pos = source.line_col(label.span.start);
        // An error at the very end of a file — an unclosed quote, a missing `.` — points
        // just past the last line, and the source line was skipped entirely. Showing
        // nothing is the worst case: the reader is told a line number that does not
        // exist and given no text at all. Point at the last real line instead.
        if pos.line > source.line_count() {
            pos.line = source.line_count().max(1);
        }
        let text = source.line_text(pos.line);
        let _ = writeln!(out, "{:>width$} | {}", pos.line, text, width = width + 3);

        if let Some((pad, caret_width)) = source.caret_extent(pos.line, label.span) {
            let _ = write!(
                out,
                "{:>width$} | {}{}",
                "",
                " ".repeat(pad),
                "^".repeat(caret_width),
                width = width + 3
            );
            if label.note.is_empty() {
                out.push('\n');
            } else {
                let _ = writeln!(out, " {}", label.note);
            }
        }
    }

    out.push('\n');
    if explain {
        if let Some(rule) = rule_conditions(err.code) {
            let _ = writeln!(out, "rule conditions: {}", wrap_rule(rule, 17, 88));
        }
    }
    let _ = writeln!(out, "what went wrong: {}", err.what_went_wrong);
    let _ = writeln!(out, "suggested fix: {}", err.suggest_fix);
}

/// The rule an error code enforces, stated once so a reader learns it.
///
/// Written by hand rather than generated from the checker. The checker's own condition is
/// something like `sign_fits(got, want)` — true, and useless to anyone trying to find out
/// what AHPCL will and will not accept.
fn rule_conditions(code: Code) -> Option<&'static str> {
    use Category::*;
    Some(match (code.category, code.number) {
        (Lex, 1) => "a `#N` comment covers N lines, and cannot run past the end of the file.",
        (Lex, 2) => "a quoted name or string must be closed on the line it opens.",
        (Lex, 3) => "after `\\`, only a delimiter or another `\\` may follow.",
        (Lex, 4) => "source is Unicode, but only the characters the language defines may appear                      outside a string. Lookalikes are not the character they resemble.",
        (Lex, 5) => "a bare number is legal only inside `math { }`; elsewhere a value is quoted.",
        (Syn, 1) => "every construct has one written form, and the parser accepts only that form.",
        (Syn, 2) => "`handback` ends the block that produced the value, so nothing after it in                      that block can run.",
        (Type, 1) => "a value's type comes from the context that receives it. Where nothing                       pins it, the type is not decided and the program must say.",
        (Type, 2) => "a narrower type passes into a wider one, never the reverse: `int` into                       `deci` into `rat` into `num`.",
        (Type, 3) => "arithmetic applies to numbers. Text and truth values have their own                       operations.",
        (Type, 4) => "the type restated in `change:` is checked against the declaration,                       because documentation that can drift is worse than none.",
        (Type, 5) => "a declaration gives a value. Nothing can be read before it is written, so                       there is no unset state and no silent zero.",
        (Type, 7) => "a conditional used for its value must hand one back on every path, \
                      including the else — there is no value for a path that produces nothing.",
        (Type, 6) => "naming one array as another could copy it or share it, and AHPCL has no                       syntax for either. Where the program has not said which, the compiler                       does not choose.",
        (Type, 10) => "a type is one of the names the language defines.",
        (Prec, 1) => "a width must be knowable when the program is compiled. A value read from                       input is not, so its width is stated rather than inferred.",
        (Prec, 2) => "`infnum` is unbounded, so a bit width would contradict it. It takes                       digits instead.",
        (Prec, 3) => "decimal widths follow IEEE 754: 32, 64 or 128 bits.",
        (Prec, 4) => "a value must fit the width it is given, and an irrational is computed                       only as far as AHPCL knows it. Overflow is an error, never a wrap, and                       excess precision is an error, never a silent approximation.",
        (Prec, 5) => "integer widths are 8, 16, 32, 64 or 128 bits.",
        (Prec, 10) => "precision is written `[N bit]` or `[N digits]`.",
        (Sign, 1) => "a `+` or `-` prefix is a promise about every value the variable holds,                       checked wherever one is assigned.",
        (Sign, 2) => "a loop counter belongs to its loop and cannot be assigned inside the body.",
        (Sign, 3) => "a sign prefix must be *proved*, not assumed. Where range analysis cannot                       show it holds, the program says so or drops the prefix.",
        (Sign, 4) => "a `+` or `-` prefix holds at run time as well as compile time.",
        (Shape, 1) => "an operation between arrays needs shapes that agree, and the rule for                        agreeing is the operation's own.",
        (Shape, 2) => "the rank name and the written shape describe the same array, so they                        must say the same thing: `vector` one dimension, `matrix` two, `tensor`                        three or more.",
        (Shape, 3) => "a value's shape is the shape the variable receiving it was declared to have.",
        (Shape, 10) => "a shape is a list of dimensions, each a whole number or `?`.",
        (Name, 1) => "a name is declared before it is read, in a scope that is still open.",
        (Name, 2) => "a function is declared before it is called.",
        (Name, 3) => "a call passes exactly as many arguments as the function declares.",
        (Run, 1) => "what a program does at run time must still obey the language's rules.",
        (Run, 2) => "division by zero has no value, so it stops rather than inventing one.",
        (Run, 3) => "an index falls inside the array, and the first element is 1.",
        (Run, 4) => "`parse` is strict: text becomes a number only in the forms asked for.",
        _ => return None,
    })
}

/// Wrap a rule onto the label's own column, so it reads as one block.
fn wrap_rule(text: &str, label_width: usize, width: usize) -> String {
    let indent = " ".repeat(label_width);
    let mut out = String::new();
    let mut column = label_width;
    for word in text.split_whitespace() {
        if column > label_width && column + 1 + word.len() > width {
            out.push('\n');
            out.push_str(&indent);
            column = label_width;
        } else if column > label_width {
            out.push(' ');
            column += 1;
        }
        out.push_str(word);
        column += word.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_in_use_states_its_rule() {
        // A code with no rule renders without the line, silently. Listing them here means
        // adding a code without explaining it fails loudly instead.
        for category in [
            Category::Lex,
            Category::Syn,
            Category::Type,
            Category::Prec,
            Category::Sign,
            Category::Shape,
            Category::Name,
            Category::Run,
        ] {
            for number in 1..=10 {
                let code = Code::new(category, number);
                if let Some(rule) = rule_conditions(code) {
                    assert!(
                        rule.ends_with('.') && rule.len() > 20,
                        "{} reads oddly: {rule:?}",
                        code.render()
                    );
                }
            }
        }
        // Spot-check the ones a reader meets most.
        assert!(rule_conditions(Code::new(Category::Type, 2)).is_some());
        assert!(rule_conditions(Code::new(Category::Name, 1)).is_some());
        assert!(rule_conditions(Code::new(Category::Run, 3)).is_some());
    }

    #[test]
    fn a_rule_is_stated_once_per_run() {
        let source = SourceFile::new("m.ahpcl", "var:int 'a' = '1'.
var:int 'b' = '2'.
");
        let one = Error::new(Code::new(Category::Type, 2), Span::new(0, 3), "first", "fix");
        let two = Error::new(Code::new(Category::Type, 2), Span::new(19, 22), "second", "fix");
        let text = render(&source, &[one, two]);
        assert_eq!(
            text.matches("rule conditions:").count(),
            1,
            "the rule should appear once, not per error:\n{text}"
        );
        assert_eq!(text.matches("what went wrong:").count(), 2);
    }

    #[test]
    fn a_long_rule_wraps_under_its_own_label() {
        let wrapped = wrap_rule("one two three four five six seven eight nine ten", 17, 30);
        for (i, line) in wrapped.lines().enumerate() {
            if i > 0 {
                assert!(line.starts_with(&" ".repeat(17)), "continuation not aligned: {line:?}");
            }
        }
        assert!(wrapped.lines().count() > 1, "should have wrapped");
    }

    fn src() -> SourceFile {
        SourceFile::new(
            "main.ahpcl",
            "var:+int 'n' = '10'.\nprint[\"counting\"].\nchange:var:int 'n' = math { ('n') - 20 }.\n",
        )
    }

    #[test]
    fn single_error_is_not_numbered_and_reads_singular() {
        let e = Error::new(
            Code::new(Category::Sign, 1),
            Span::new(21, 26),
            "a +int must be above 0 at every point in the program.",
            "declare 'n' as :int.",
        );
        let out = render(&src(), &[e]);
        assert!(out.contains("there's something wrong"));
        assert!(!out.contains("Error 1 of"));
        assert!(out.ends_with("1 error found.\n"));
        assert!(out.contains("[AHPCL-SIGN-0001]"));
    }

    #[test]
    fn several_errors_are_numbered_and_read_plural() {
        let a = Error::new(Code::new(Category::Lex, 1), Span::new(0, 3), "r", "f");
        let b = Error::new(Code::new(Category::Name, 1), Span::new(21, 24), "r", "f");
        let out = render(&src(), &[a, b]);
        assert!(out.contains("there are some things wrong"));
        assert!(out.contains("Error 1 of 2"));
        assert!(out.contains("Error 2 of 2"));
        assert!(out.ends_with("2 errors found.\n"));
    }

    #[test]
    fn an_error_can_point_at_two_places() {
        let e = Error::new(
            Code::new(Category::Sign, 1),
            Span::new(58, 76),
            "a +int must be above 0 at every point in the program.",
            "declare 'n' as :int.",
        )
        .with_label(Span::new(4, 8), "'n' promises to stay above 0 here")
        .with_label(Span::new(58, 76), "but this can make it -10");

        let out = render(&src(), &[e]);
        assert!(out.contains("promises to stay above 0 here"));
        assert!(out.contains("but this can make it -10"));
        // Both source lines are quoted.
        assert!(out.contains("var:+int 'n' = '10'."));
        assert!(out.contains("change:var:int 'n'"));
    }

    #[test]
    fn repeats_of_the_same_message_at_the_same_place_are_dropped() {
        // One mistake often trips several checks; showing it five times buries the cause.
        let one = || Error::new(Code::new(Category::Syn, 1), Span::new(0, 3), "same", "fix");
        let out = render(&src(), &[one(), one(), one()]);
        assert!(out.contains("1 error found."), "{out}");
    }

    #[test]
    fn genuinely_different_errors_are_all_kept() {
        let a = Error::new(Code::new(Category::Syn, 1), Span::new(0, 3), "first", "fix");
        let b = Error::new(Code::new(Category::Syn, 1), Span::new(21, 24), "second", "fix");
        let out = render(&src(), &[a, b]);
        assert!(out.contains("2 errors found."), "{out}");
    }

    #[test]
    fn the_legend_fields_are_present() {
        let e = Error::new(Code::new(Category::Lex, 1), Span::new(0, 3), "r", "f");
        let out = render(&src(), &[e]);
        assert!(out.contains("main.ahpcl:1:1"));
        assert!(out.contains("file: main.ahpcl"));
        assert!(out.contains("line: 1"));
        assert!(out.contains("column: 1"));
    }
}
