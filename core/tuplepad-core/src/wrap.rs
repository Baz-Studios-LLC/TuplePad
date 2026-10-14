//! Soft wrapping. Lines are measured in monospace cells (wide characters take two,
//! tabs run to the next stop), so every line wraps the same way whether or not it has
//! been drawn yet, and scroll heights are exact.

use ropey::RopeSlice;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Where a long line wraps.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct WrappedLine {
    /// UTF-16 columns where each continuation row starts. Empty if the line fits on one row.
    pub breaks: Vec<u32>,
    /// How far continuation rows are indented, in cells (the line's own indentation).
    pub indent: u32,
}

pub(crate) fn cell_width(grapheme: &str) -> usize {
    UnicodeWidthStr::width(grapheme)
}

/// How many cells a tab at `column` takes.
fn tab_cells(column: usize, tab_width: usize) -> usize {
    tab_width - column % tab_width
}

/// Wraps one line (without its line break) to `width` cells. Returns the break columns and
/// the continuation indent. With `hanging_indent` off, continuation rows start at the left edge.
pub(crate) fn wrap_str(line: &str, width: usize, tab_width: usize, hanging_indent: bool) -> (Vec<u32>, u32) {
    let width = width.max(8);
    let tab_width = tab_width.max(1);

    // Continuation rows line up with the line's indentation, unless that leaves too little room.
    let mut indent = 0;
    for c in line.chars() {
        match c {
            ' ' => indent += 1,
            '\t' => indent += tab_cells(indent, tab_width),
            _ => break,
        }
    }
    let indent = if hanging_indent && indent * 2 <= width { indent } else { 0 };

    let mut breaks = Vec::new();
    let mut row_cells = 0; // cells used in the current row, including any continuation indent
    let mut line_cells = 0; // cells from the start of the line, for tab stops
    let mut position = 0u32; // UTF-16 column
    // The most recent place the row could break (just after whitespace), and the row's cells there.
    let mut opportunity: Option<(u32, usize)> = None;
    let mut row_start = 0u32;

    for grapheme in line.graphemes(true) {
        let is_tab = grapheme == "\t";
        let is_space = is_tab || grapheme == " ";
        let cells = if is_tab { tab_cells(line_cells, tab_width) } else { cell_width(grapheme) };
        let row_floor = if breaks.is_empty() { 0 } else { indent };

        // Spaces may hang past the edge; anything else that doesn't fit starts a new row.
        if row_cells + cells > width && row_cells > row_floor && !is_space {
            match opportunity {
                Some((at, cells_there)) if at > row_start => {
                    breaks.push(at);
                    row_start = at;
                    row_cells = indent + (row_cells - cells_there);
                }
                _ => {
                    breaks.push(position);
                    row_start = position;
                    row_cells = indent;
                }
            }
            opportunity = None;
        }

        row_cells += cells;
        line_cells += cells;
        position += grapheme.encode_utf16().count() as u32;
        if is_space {
            opportunity = Some((position, row_cells));
        }
    }
    (breaks, indent as u32)
}

/// Rows a line takes, with a fast path for lines that obviously fit.
pub(crate) fn row_count(line: RopeSlice, width: usize, tab_width: usize) -> u32 {
    if line.len_chars() * tab_width.max(2) <= width {
        return 1;
    }
    let text = line.as_str().map(std::borrow::Cow::Borrowed).unwrap_or_else(|| line.to_string().into());
    wrap_str(&text, width, tab_width, true).0.len() as u32 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(line: &str, width: usize) -> Vec<String> {
        let (breaks, _) = wrap_str(line, width, 4, true);
        let units: Vec<u16> = line.encode_utf16().collect();
        let mut start = 0;
        breaks
            .iter()
            .chain(std::iter::once(&(units.len() as u32)))
            .map(|&end| {
                let row = String::from_utf16_lossy(&units[start..end as usize]);
                start = end as usize;
                row
            })
            .collect()
    }

    #[test]
    fn short_lines_stay_whole() {
        assert_eq!(rows("hello world", 20), ["hello world"]);
        assert!(wrap_str("", 20, 4, true).0.is_empty());
    }

    #[test]
    fn breaks_after_spaces() {
        assert_eq!(rows("the quick brown fox jumps", 10), ["the quick ", "brown fox ", "jumps"]);
    }

    #[test]
    fn long_words_break_anywhere() {
        assert_eq!(rows("abcdefghijklmnop", 8), ["abcdefgh", "ijklmnop"]);
    }

    #[test]
    fn continuation_rows_keep_indentation() {
        let (breaks, indent) = wrap_str("    alpha beta gamma delta", 14, 4, true);
        assert_eq!(indent, 4);
        assert_eq!(breaks, vec![15, 21]);
    }

    #[test]
    fn wide_characters_take_two_cells() {
        assert_eq!(rows("日本語のテキスト", 8), ["日本語の", "テキスト"]);
        assert_eq!(rows("👋👋👋👋👋", 8), ["👋👋👋👋", "👋"]);
    }

    #[test]
    fn counts_rows_quickly() {
        let rope = ropey::Rope::from_str("short\nthe quick brown fox jumps\n");
        assert_eq!(row_count(rope.line(0), 10, 4), 1);
        assert_eq!(row_count(rope.line(1), 10, 4), 3);
    }
}
