//! The AHPCL Informer.
//!
//! Reports everything the compiler decided or inferred on your behalf. On by default,
//! full detail. One line per note — deliberately *not* the error template, because the
//! volume is completely different: errors are rare, Informer notes are numerous.

use std::fmt::Write as _;

use crate::position::{BytePos, SourceFile};

#[derive(Debug, Clone)]
pub struct Note {
    pub at: Option<BytePos>,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct Informer {
    notes: Vec<Note>,
    enabled: bool,
}

impl Informer {
    /// On by default, full detail.
    pub fn new() -> Self {
        Informer { notes: Vec::new(), enabled: true }
    }

    pub fn silent() -> Self {
        Informer { notes: Vec::new(), enabled: false }
    }

    pub fn say(&mut self, at: BytePos, message: impl Into<String>) {
        if self.enabled {
            self.notes.push(Note { at: Some(at), message: message.into() });
        }
    }

    /// A note with no source position, such as a phase timing.
    pub fn say_global(&mut self, message: impl Into<String>) {
        if self.enabled {
            self.notes.push(Note { at: None, message: message.into() });
        }
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn render(&self, source: &SourceFile) -> String {
        if self.notes.is_empty() {
            return String::new();
        }
        let mut out = String::from("AHPCL Informer:\n");
        for note in &self.notes {
            match note.at {
                Some(pos) => {
                    let lc = source.line_col(pos);
                    let _ = writeln!(
                        out,
                        "informer: {}:{}:{} — {}",
                        source.name, lc.line, lc.column, note.message
                    );
                }
                None => {
                    let _ = writeln!(out, "informer: {}", note.message);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_carry_positions_and_render_one_per_line() {
        let src = SourceFile::new("main.ahpcl", "var:int 'x' = '1000'.\n");
        let mut inf = Informer::new();
        inf.say(BytePos(8), "'x' widened to 32-bit because of line 12");
        inf.say_global("lexed 1 line in 0.1ms");
        let out = inf.render(&src);
        assert!(out.starts_with("AHPCL Informer:\n"));
        assert!(out.contains("informer: main.ahpcl:1:9 — 'x' widened to 32-bit"));
        assert!(out.contains("informer: lexed 1 line in 0.1ms"));
    }

    #[test]
    fn silent_informer_records_nothing() {
        let mut inf = Informer::silent();
        inf.say_global("ignored");
        assert!(inf.is_empty());
    }
}
