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
    pub rule_conditions: String,
    pub suggest_fix: String,
}

impl Error {
    pub fn new(
        code: Code,
        primary: Span,
        rule_conditions: impl Into<String>,
        suggest_fix: impl Into<String>,
    ) -> Self {
        Error {
            code,
            primary,
            labels: Vec::new(),
            rule_conditions: rule_conditions.into(),
            suggest_fix: suggest_fix.into(),
        }
    }

    pub fn with_label(mut self, span: Span, note: impl Into<String>) -> Self {
        self.labels.push(Label::new(span, note));
        self
    }
}

/// Renders a batch of errors as one report.
pub fn render(source: &SourceFile, errors: &[Error]) -> String {
    if errors.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let many = errors.len() > 1;

    out.push_str("AHPCL Error Handler:\n");
    if many {
        out.push_str("Hello, I think that there are some things wrong.\n");
    } else {
        out.push_str("Hello, I think that there's something wrong.\n");
    }
    out.push('\n');

    for (i, err) in errors.iter().enumerate() {
        if many {
            let _ = writeln!(out, "Error {} of {}", i + 1, errors.len());
        }
        render_one(&mut out, source, err);
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

fn render_one(out: &mut String, source: &SourceFile, err: &Error) {
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
        let pos = source.line_col(label.span.start);
        if pos.line > source.line_count() {
            continue;
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
    let _ = writeln!(out, "rule conditions: {}", err.rule_conditions);
    let _ = writeln!(out, "suggest fix: {}", err.suggest_fix);
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_legend_fields_are_present() {
        let e = Error::new(Code::new(Category::Lex, 1), Span::new(0, 3), "r", "f");
        let out = render(&src(), &[e]);
        assert!(out.contains("main.ahpcl:1:1"));
        assert!(out.contains("file: main.ahpcl"));
        assert!(out.contains("line: 1"));
        assert!(out.contains("column: 1"));
    }
}
