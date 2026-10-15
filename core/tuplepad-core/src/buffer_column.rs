//! The column editor: inserting the same text, or a run of numbers, down a column.

use super::*;
use super::tools::CharEdit;
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberBase {
    Decimal,
    Hex,
    HexUppercase,
    Octal,
    Binary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberPadding {
    None,
    /// Leading zeros, to the width of the longest number.
    Zeros,
    /// Right-aligned with spaces.
    Spaces,
}

/// What the column editor inserts on each line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColumnInsert {
    Text { text: String },
    /// `start`, `start + step`, … with each number repeated `repeat` times.
    Numbers { start: i64, step: i64, repeat: u32, base: NumberBase, padding: NumberPadding },
}

/// The first `count` values the column editor would insert, for a preview.
pub fn column_values(insert: ColumnInsert, count: u32) -> Vec<String> {
    values(&insert, count as usize)
}

fn values(insert: &ColumnInsert, count: usize) -> Vec<String> {
    match insert {
        ColumnInsert::Text { text } => vec![text.clone(); count],
        &ColumnInsert::Numbers { start, step, repeat, base, padding } => {
            let repeat = repeat.max(1) as i64;
            let numbers: Vec<i64> = (0..count as i64)
                .map(|i| start.saturating_add((i / repeat).saturating_mul(step)))
                .collect();
            let digits: Vec<(bool, String)> = numbers.iter().map(|&n| (n < 0, format_digits(n.unsigned_abs(), base))).collect();
            let widest_digits = digits.iter().map(|(_, d)| d.len()).max().unwrap_or(0);
            let widest = digits.iter().map(|(negative, d)| d.len() + usize::from(*negative)).max().unwrap_or(0);
            digits
                .into_iter()
                .map(|(negative, digits)| {
                    let sign = if negative { "-" } else { "" };
                    match padding {
                        NumberPadding::None => format!("{sign}{digits}"),
                        NumberPadding::Zeros => format!("{sign}{digits:0>widest_digits$}"),
                        NumberPadding::Spaces => format!("{:>widest$}", format!("{sign}{digits}")),
                    }
                })
                .collect()
        }
    }
}

fn format_digits(value: u64, base: NumberBase) -> String {
    match base {
        NumberBase::Decimal => value.to_string(),
        NumberBase::Hex => format!("{value:x}"),
        NumberBase::HexUppercase => format!("{value:X}"),
        NumberBase::Octal => format!("{value:o}"),
        NumberBase::Binary => format!("{value:b}"),
    }
}

impl TextBuffer {
    /// Inserts text or numbers down a column, as one undo step. With several carets, at each
    /// one (replacing any selection). With one selection over several lines, at its start column
    /// on each of those lines; with a single caret, from its line to the end of the document.
    /// Short lines are padded with spaces so everything lines up. Returns a caret after each insertion.
    pub fn insert_column(&self, insert: ColumnInsert, cursors: Vec<TextSelection>, tab_width: u32) -> Vec<TextSelection> {
        let mut st = self.write();
        let tab_width = tab_width.max(1) as usize;
        let mut targets: Vec<(usize, usize, String)> = Vec::new(); // (start char, end char, padding)
        if cursors.len() > 1 {
            let mut sorted = cursors.clone();
            sorted.sort_by_key(|c| c.start());
            for cursor in sorted {
                targets.push((st.pos_to_char(cursor.start()), st.pos_to_char(cursor.end()), String::new()));
            }
        } else if let Some(cursor) = cursors.first() {
            let (start, end) = (cursor.start(), cursor.end());
            let first = st.clamp_line(start.line);
            let last = if start.line == end.line && start.column == end.column {
                // To the end of the document, leaving out the empty line after a final line break.
                let last = st.rope.len_lines() - 1;
                if last > first && st.line_content(last).len_chars() == 0 { last - 1 } else { last }
            } else {
                let last = st.clamp_line(end.line);
                if end.column == 0 && last > first { last - 1 } else { last }
            };
            let column = st.display_column(first, start.column as usize, tab_width);
            for line in first..=last {
                let (at, padding) = st.char_at_display_column(line, column, tab_width);
                targets.push((at, at, " ".repeat(padding)));
            }
        }
        if targets.is_empty() {
            return cursors;
        }
        let texts = values(&insert, targets.len());
        let edits: Vec<CharEdit> = targets
            .iter()
            .zip(&texts)
            .map(|((start, end, padding), text)| CharEdit { start: *start, end: *end, text: format!("{padding}{text}") })
            .collect();
        let ends: Vec<(usize, usize)> = targets.iter().map(|&(start, end, _)| (start, end)).collect();
        st.apply_edits(edits, cursors, EditKind::Other, |st, map| {
            // map() puts an offset at the end of text inserted there.
            ends.iter().map(|&(_, end)| TextSelection::caret(st.char_to_pos(map(end)))).collect()
        })
    }
}

impl State {
    /// The on-screen column of a UTF-16 column, with tabs expanded and wide characters counted as two.
    fn display_column(&self, line: usize, utf16_column: usize, tab_width: usize) -> usize {
        let mut width = 0;
        let mut units = 0;
        for c in self.line_content(line).chars() {
            if units >= utf16_column {
                break;
            }
            width += if c == '\t' { tab_width - width % tab_width } else { c.width().unwrap_or(0) };
            units += c.len_utf16();
        }
        width
    }

    /// The char offset at an on-screen column in a line, and how many spaces are needed to reach it
    /// when the line is shorter.
    fn char_at_display_column(&self, line: usize, column: usize, tab_width: usize) -> (usize, usize) {
        let start = self.rope.line_to_char(line);
        let mut width = 0;
        for (index, c) in self.line_content(line).chars().enumerate() {
            if width >= column {
                return (start + index, 0);
            }
            width += if c == '\t' { tab_width - width % tab_width } else { c.width().unwrap_or(0) };
        }
        (start + self.line_content(line).len_chars(), column.saturating_sub(width))
    }
}
