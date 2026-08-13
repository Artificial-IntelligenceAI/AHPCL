//! Source positions.
//!
//! Columns count **grapheme clusters** — one user-perceived character. A family emoji
//! is 7 code points and ~25 bytes, and exactly 1 column.
//!
//! Carets are drawn in **display width** instead, because a caret's only job is to line
//! up on screen. The two measures are never compared. See docs/diagnostics.md.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A byte offset into a source file. Cheap to carry; converted to line/column only
/// when something is actually reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BytePos(pub usize);

/// A half-open byte range: `start` up to but not including `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: BytePos,
    pub end: BytePos,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start: BytePos(start), end: BytePos(end) }
    }

    /// A zero-width span, for pointing between characters.
    pub fn at(pos: usize) -> Self {
        Span::new(pos, pos)
    }

    pub fn to(self, other: Span) -> Span {
        Span { start: self.start, end: other.end }
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// A human-facing position: 1-based line, 1-based column in grapheme clusters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: usize,
    pub column: usize,
}

/// One source file, with a precomputed line index.
pub struct SourceFile {
    pub name: String,
    pub text: String,
    /// Byte offset at which each line starts.
    line_starts: Vec<usize>,
}

impl SourceFile {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        SourceFile { name: name.into(), text, line_starts }
    }

    /// Number of lines with content. A trailing newline does not start a new line.
    pub fn line_count(&self) -> usize {
        if self.text.ends_with('\n') {
            self.line_starts.len().saturating_sub(1).max(1)
        } else {
            self.line_starts.len()
        }
    }

    /// 1-based line number containing `pos`.
    fn line_index(&self, pos: usize) -> usize {
        match self.line_starts.binary_search(&pos) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    /// Text of a 1-based line, without its trailing newline.
    pub fn line_text(&self, line: usize) -> &str {
        let idx = line - 1;
        let start = self.line_starts[idx];
        let end = self
            .line_starts
            .get(idx + 1)
            .map(|&e| e)
            .unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\n', '\r'])
    }

    /// Convert a byte offset to line and grapheme column, both 1-based.
    pub fn line_col(&self, pos: BytePos) -> LineCol {
        let pos = pos.0.min(self.text.len());
        let idx = self.line_index(pos);
        let line_start = self.line_starts[idx];
        let prefix = &self.text[line_start..pos];
        LineCol {
            line: idx + 1,
            column: prefix.graphemes(true).count() + 1,
        }
    }

    /// How many terminal cells `text` occupies. Used only to position carets.
    pub fn display_width(text: &str) -> usize {
        UnicodeWidthStr::width(text)
    }

    /// Cells to skip, then cells to underline, for a span on one line.
    /// Returns `None` if the span is not on that line at all.
    pub fn caret_extent(&self, line: usize, span: Span) -> Option<(usize, usize)> {
        let idx = line - 1;
        let line_start = *self.line_starts.get(idx)?;
        let line_end = self
            .line_starts
            .get(idx + 1)
            .map(|&e| e)
            .unwrap_or(self.text.len());

        let s = span.start.0.max(line_start).min(line_end);
        let e = span.end.0.max(line_start).min(line_end);
        if s > line_end || e < line_start {
            return None;
        }

        let before = &self.text[line_start..s];
        let inner = &self.text[s..e];
        let pad = Self::display_width(before.trim_end_matches(['\n', '\r']));
        let width = Self::display_width(inner.trim_end_matches(['\n', '\r'])).max(1);
        Some((pad, width))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_count_graphemes_not_bytes() {
        // The family emoji is 4 people joined by 3 zero-width joiners:
        // 7 code points, 25 bytes, and exactly one column.
        let family = "\u{1F9D1}\u{200D}\u{1F9D1}\u{200D}\u{1F9D2}\u{200D}\u{1F9D2}";
        assert_eq!(family.chars().count(), 7);
        assert_eq!(family.len(), 25);

        let src = SourceFile::new("t.ahpcl", format!("{family}C"));
        // The 'C' sits immediately after one grapheme, so it is column 2.
        let at_c = src.line_col(BytePos(family.len()));
        assert_eq!(at_c.column, 2);
    }

    #[test]
    fn two_family_emoji_are_two_columns() {
        let family = "\u{1F9D1}\u{200D}\u{1F9D1}\u{200D}\u{1F9D2}\u{200D}\u{1F9D2}";
        let src = SourceFile::new("t.ahpcl", format!("{family}{family}X"));
        let at_x = src.line_col(BytePos(family.len() * 2));
        assert_eq!(at_x.column, 3);
    }

    #[test]
    fn thai_tone_mark_does_not_add_a_column() {
        // ก with a tone mark is one grapheme, so one column.
        let src = SourceFile::new("t.ahpcl", "ก\u{0E48}X");
        let at_x = src.line_col(BytePos("ก\u{0E48}".len()));
        assert_eq!(at_x.column, 2);
    }

    #[test]
    fn carets_use_display_width_not_graphemes() {
        // An emoji is 1 column but 2 terminal cells, so the caret pads by 2.
        let src = SourceFile::new("t.ahpcl", "😂ab");
        let span = Span::new("😂".len(), "😂a".len());
        let (pad, width) = src.caret_extent(1, span).unwrap();
        assert_eq!(pad, 2, "emoji occupies two cells");
        assert_eq!(width, 1);
    }

    #[test]
    fn lines_and_columns_are_one_based() {
        let src = SourceFile::new("t.ahpcl", "abc\ndef\n");
        assert_eq!(src.line_col(BytePos(0)), LineCol { line: 1, column: 1 });
        assert_eq!(src.line_col(BytePos(4)), LineCol { line: 2, column: 1 });
        assert_eq!(src.line_col(BytePos(6)), LineCol { line: 2, column: 3 });
        assert_eq!(src.line_text(2), "def");
    }
}
