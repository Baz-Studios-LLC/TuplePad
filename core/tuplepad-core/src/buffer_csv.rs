//! CSV and TSV: colored columns, aligning columns, and sorting by a column.

use super::*;
use super::tools::CharEdit;
use crate::syntax::HighlightStyle;
use unicode_width::UnicodeWidthStr;

/// The colors columns cycle through (theme colors, so they suit every theme).
const COLUMN_STYLES: [HighlightStyle; 8] = [
    HighlightStyle::Keyword,
    HighlightStyle::String,
    HighlightStyle::Function,
    HighlightStyle::Number,
    HighlightStyle::Type,
    HighlightStyle::Tag,
    HighlightStyle::Property,
    HighlightStyle::Attribute,
];

/// The fields of one line as char ranges (quotes respected; a quote can't span lines here).
pub(crate) fn fields(line: &str, delimiter: char) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let chars: Vec<char> = line.chars().collect();
    for (index, &c) in chars.iter().enumerate() {
        if c == '"' {
            in_quotes = !in_quotes;
        } else if c == delimiter && !in_quotes {
            ranges.push((start, index));
            start = index + 1;
        }
    }
    ranges.push((start, chars.len()));
    ranges
}

/// Guesses the delimiter from the first lines: the one that splits them most consistently.
pub(crate) fn detect_delimiter(rope: &Rope) -> char {
    let candidates = [',', '\t', ';', '|'];
    let lines: Vec<String> = rope.lines().take(30).map(|l| l.to_string()).filter(|l| !l.trim().is_empty()).collect();
    let mut best = (',', 0usize);
    for delimiter in candidates {
        let counts: Vec<usize> = lines.iter().map(|line| fields(line.trim_end(), delimiter).len() - 1).collect();
        let Some(&first) = counts.first() else { continue };
        if first == 0 {
            continue;
        }
        let consistent = counts.iter().filter(|&&n| n == first).count();
        let score = consistent * 100 + first;
        if score > best.1 {
            best = (delimiter, score);
        }
    }
    best.0
}

impl State {
    pub(super) fn csv_highlights(&self, delimiter: char, start_line: usize, count: usize) -> Vec<HighlightSpan> {
        let mut spans = Vec::new();
        let end = (start_line + count).min(self.rope.len_lines());
        for line in start_line..end {
            let text = self.line_content(line).to_string();
            if text.len() > 64 * 1024 {
                continue;
            }
            let chars: Vec<char> = text.chars().collect();
            let utf16 = |index: usize| chars[..index].iter().map(|c| c.len_utf16()).sum::<usize>() as u64;
            for (column, (start, end)) in fields(&text, delimiter).into_iter().enumerate() {
                if start < end {
                    spans.push(HighlightSpan {
                        line: line as u64,
                        start: utf16(start),
                        end: utf16(end),
                        style: COLUMN_STYLES[column % COLUMN_STYLES.len()],
                    });
                }
            }
        }
        spans
    }

    /// The last line with content (the empty line after a final line break doesn't count).
    fn last_content_line(&self) -> usize {
        let last = self.rope.len_lines() - 1;
        if last > 0 && self.line_content(last).len_chars() == 0 { last - 1 } else { last }
    }

    /// Every line split into trimmed fields.
    fn csv_rows(&self, delimiter: char) -> Vec<Vec<String>> {
        (0..=self.last_content_line())
            .map(|line| {
                let text = self.line_content(line).to_string();
                let chars: Vec<char> = text.chars().collect();
                fields(&text, delimiter).into_iter().map(|(s, e)| chars[s..e].iter().collect::<String>().trim().to_string()).collect()
            })
            .collect()
    }

    /// Replaces all the lines (except a final empty one) as one undo step.
    fn replace_lines(&mut self, lines: Vec<String>, cursors: Vec<TextSelection>) -> Vec<TextSelection> {
        let last = self.last_content_line();
        let edit = CharEdit { start: 0, end: self.rope.line_to_char(last) + self.line_content(last).len_chars(), text: lines.join(self.line_ending.as_str()) };
        let old = self.cursor_chars(&cursors);
        self.apply_edits(vec![edit], cursors, EditKind::Other, |st, map| st.mapped(&old, map))
    }
}

#[uniffi::export]
impl TextBuffer {
    /// The CSV delimiter in use, or nil if the document isn't CSV.
    pub fn csv_delimiter(&self) -> Option<String> {
        self.read().csv.map(String::from)
    }

    /// Uses a different delimiter (and treats the document as CSV).
    pub fn set_csv_delimiter(&self, delimiter: String) {
        let Some(c) = delimiter.chars().next() else { return };
        let mut st = self.write();
        st.csv = Some(c);
        st.syntax = None;
        st.version += 1;
    }

    /// The first row's fields, for choosing a column.
    pub fn csv_header(&self) -> Vec<String> {
        let st = self.read();
        let delimiter = st.csv.unwrap_or_else(|| detect_delimiter(&st.rope));
        st.csv_rows(delimiter).into_iter().next().unwrap_or_default()
    }

    /// Pads every field so the columns line up, as one undo step.
    pub fn csv_align(&self, cursors: Vec<TextSelection>) -> Vec<TextSelection> {
        let mut st = self.write();
        let delimiter = st.csv.unwrap_or_else(|| detect_delimiter(&st.rope));
        let rows = st.csv_rows(delimiter);
        let mut widths: Vec<usize> = Vec::new();
        for row in &rows {
            for (column, field) in row.iter().enumerate() {
                if widths.len() <= column {
                    widths.push(0);
                }
                widths[column] = widths[column].max(field.width());
            }
        }
        let lines = rows
            .iter()
            .map(|row| {
                if row.len() == 1 && row[0].is_empty() {
                    return String::new();
                }
                let last = row.len() - 1;
                row.iter()
                    .enumerate()
                    .map(|(column, field)| {
                        let padding = if column < last { widths[column] - field.width() } else { 0 };
                        format!("{field}{}", " ".repeat(padding))
                    })
                    .collect::<Vec<_>>()
                    .join(&delimiter.to_string())
            })
            .collect();
        st.replace_lines(lines, cursors)
    }

    /// Removes the padding `csv_align` added, as one undo step.
    pub fn csv_shrink(&self, cursors: Vec<TextSelection>) -> Vec<TextSelection> {
        let mut st = self.write();
        let delimiter = st.csv.unwrap_or_else(|| detect_delimiter(&st.rope));
        let lines = st.csv_rows(delimiter).into_iter().map(|row| row.join(&delimiter.to_string())).collect();
        st.replace_lines(lines, cursors)
    }

    /// Sorts the rows by a column (0-based), keeping a header row on top, as one undo step.
    pub fn csv_sort(&self, column: u32, descending: bool, numeric: bool, has_header: bool, cursors: Vec<TextSelection>) -> Vec<TextSelection> {
        let mut st = self.write();
        let delimiter = st.csv.unwrap_or_else(|| detect_delimiter(&st.rope));
        let last = st.last_content_line();
        let rows = st.csv_rows(delimiter);
        let mut lines: Vec<(String, String)> = (0..=last)
            .map(|line| (st.line_content(line).to_string(), rows[line].get(column as usize).cloned().unwrap_or_default()))
            .collect();
        let header = if has_header && !lines.is_empty() { Some(lines.remove(0)) } else { None };
        let unquote = |s: &str| s.trim_matches('"').to_string();
        lines.sort_by(|a, b| {
            let (a, b) = (unquote(&a.1), unquote(&b.1));
            let order = if numeric {
                let (x, y) = (a.parse::<f64>().ok(), b.parse::<f64>().ok());
                x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                a.to_lowercase().cmp(&b.to_lowercase())
            };
            if descending { order.reverse() } else { order }
        });
        let result: Vec<String> = header.into_iter().chain(lines).map(|(line, _)| line).collect();
        st.replace_lines(result, cursors)
    }
}
