//! A line as drawn: tabs expanded to spaces, composing text inserted at the caret, and very long
//! lines cut short; then split into colored runs for Slint.

use slint::SharedString;
use unicode_width::UnicodeWidthChar;

use super::layout::{LineLayout, MAX_LAYOUT_COLUMNS};
use crate::Span;

/// `Theme.syntax` index for invisible characters.
pub const INVISIBLE_STYLE: i32 = 24;

pub struct DisplayLine {
    pub text: String,
    /// Display column of each source column (one more entry than the source has columns);
    /// `None` when they're the same.
    map: Option<Vec<usize>>,
    /// Where composing text sits, in display columns.
    pub preedit: Option<(usize, usize)>,
    /// Display columns where an expanded tab starts.
    tab_starts: Vec<usize>,
    /// Display columns of control characters, drawn as faint symbols (␀ for NUL and so on).
    controls: Vec<usize>,
    /// Width of the leading whitespace, in cells.
    pub indent_cells: usize,
    /// Whether the line is only whitespace.
    pub blank: bool,
}

impl DisplayLine {
    pub fn new(source: &str, preedit: Option<(usize, &str)>, tab_width: usize) -> Self {
        let tab_width = tab_width.max(1);
        let plain = preedit.is_none() && !source.contains('\t');
        let mut text = String::with_capacity(source.len());
        let mut map = if plain { None } else { Some(Vec::with_capacity(source.len() + 1)) };
        let mut tab_starts = Vec::new();
        let mut controls = Vec::new();
        let mut shown = None;
        let (mut src, mut col, mut cells) = (0, 0, 0);
        let mut indent_cells = None;
        let insert_preedit = |text: &mut String, col: &mut usize, cells: &mut usize, shown: &mut Option<(usize, usize)>| {
            if let Some((_, composing)) = preedit {
                text.push_str(composing);
                let len: usize = composing.chars().map(char::len_utf16).sum();
                *shown = Some((*col, *col + len));
                *col += len;
                *cells += composing.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>();
            }
        };
        for ch in source.chars() {
            if preedit.is_some_and(|(at, _)| at == src) && shown.is_none() {
                insert_preedit(&mut text, &mut col, &mut cells, &mut shown);
            }
            if col >= MAX_LAYOUT_COLUMNS {
                break;
            }
            if indent_cells.is_none() && ch != ' ' && ch != '\t' {
                indent_cells = Some(cells);
            }
            let units = ch.len_utf16();
            if let Some(map) = map.as_mut() {
                map.extend((0..units).map(|i| col + i.min(1)));
            }
            if ch == '\t' {
                let spaces = tab_width - cells % tab_width;
                tab_starts.push(col);
                text.extend(std::iter::repeat_n(' ', spaces));
                col += spaces;
                cells += spaces;
            } else if is_control(ch) {
                // Drawn as a chip in its cell (fonts rarely have the Control Pictures), rather
                // than nothing: binary files and stray NULs would mislead.
                controls.push(col);
                text.push(' ');
                col += units;
                cells += 1;
            } else {
                text.push(ch);
                col += units;
                cells += ch.width().unwrap_or(0);
            }
            src += units;
        }
        if preedit.is_some() && shown.is_none() {
            insert_preedit(&mut text, &mut col, &mut cells, &mut shown);
        }
        if let Some(map) = map.as_mut() {
            map.push(col);
        }
        let blank = indent_cells.is_none();
        DisplayLine { text, map, preedit: shown, tab_starts, controls, indent_cells: indent_cells.unwrap_or(cells), blank }
    }

    pub fn col(&self, source_col: usize) -> usize {
        match &self.map {
            Some(map) => map.get(source_col).copied().unwrap_or(*map.last().unwrap_or(&0)),
            None => source_col,
        }
    }

    /// Source columns of the control characters.
    pub fn control_columns(&self) -> Vec<usize> {
        self.controls.iter().map(|&c| self.source_col(c)).collect()
    }

    pub fn source_col(&self, display_col: usize) -> usize {
        match &self.map {
            // The last source column drawn at or before this display column.
            Some(map) => map.partition_point(|&c| c <= display_col).saturating_sub(1),
            None => display_col,
        }
    }
}

/// What a row of a line shows.
pub struct Fragment {
    /// Display columns of the row.
    pub start: usize,
    pub end: usize,
    /// Added to layout x positions to place the row's text (continuation rows are indented).
    pub x_shift: f32,
    /// The row ends the line (and so draws the line-break marker).
    pub last: bool,
}

/// Colored runs of one row of a line. `colors` are (start, end, style) in source columns.
/// Whitespace is left out, or drawn as faint dots and arrows when `invisibles` is on.
pub fn row_spans(
    display: &DisplayLine,
    layout: &LineLayout,
    colors: &[(usize, usize, i32)],
    fragment: &Fragment,
    invisibles: bool,
    line_break: bool,
) -> Vec<Span> {
    let text = &display.text;
    let columns = layout.col_x.len() - 1;
    let (from, to) = (fragment.start.min(columns), fragment.end.min(columns));
    let mut styles = vec![0i32; to - from];
    for &(start, end, style) in colors {
        let (start, end) = (display.col(start).clamp(from, to), display.col(end).clamp(from, to));
        styles[start - from..end - from].fill(style);
    }
    if let Some((start, end)) = display.preedit {
        let (start, end) = (start.clamp(from, to), end.clamp(from, to));
        styles[start - from..end - from].fill(0);
    }
    // Byte offset of every display column.
    let mut col_byte = Vec::with_capacity(columns + 1);
    for (byte, ch) in text.char_indices() {
        col_byte.push(byte);
        if ch.len_utf16() == 2 {
            col_byte.push(byte);
        }
    }
    col_byte.push(text.len());

    let mut spans = Vec::new();
    let mut start = from;
    while start < to {
        let style = styles[start - from];
        let mut end = start + 1;
        while end < to && styles[end - from] == style {
            end += 1;
        }
        let piece = &text[col_byte[start]..col_byte[end]];
        let x = layout.x(start) + fragment.x_shift;
        if !piece.trim().is_empty() {
            // Split off leading and trailing whitespace so it can show as invisibles.
            push_text(&mut spans, piece, start, x, style, layout, fragment, display, invisibles);
        } else if invisibles {
            spans.push(invisible_span(piece, start, x, display));
        }
        start = end;
    }
    if invisibles && line_break && fragment.last {
        spans.push(Span { x: layout.width + fragment.x_shift + 1.0, text: "¬".into(), style: INVISIBLE_STYLE, ..Default::default() });
    }
    spans
}

#[allow(clippy::too_many_arguments)]
fn push_text(
    spans: &mut Vec<Span>,
    piece: &str,
    start: usize,
    x: f32,
    style: i32,
    layout: &LineLayout,
    fragment: &Fragment,
    display: &DisplayLine,
    invisibles: bool,
) {
    if !invisibles {
        spans.push(styled(piece, x, style));
        return;
    }
    // Whitespace runs inside the piece become invisible spans.
    let mut chunk_start = 0;
    let mut chunk_is_space = None;
    let mut col = start;
    let mut chunk_col = start;
    for (byte, ch) in piece.char_indices() {
        let is_space = ch == ' ';
        if chunk_is_space != Some(is_space) {
            if let Some(space) = chunk_is_space {
                let chunk = &piece[chunk_start..byte];
                let cx = layout.x(chunk_col) + fragment.x_shift;
                spans.push(if space { invisible_span(chunk, chunk_col, cx, display) } else { styled(chunk, cx, style) });
            }
            chunk_start = byte;
            chunk_col = col;
            chunk_is_space = Some(is_space);
        }
        col += ch.len_utf16();
    }
    if let Some(space) = chunk_is_space {
        let chunk = &piece[chunk_start..];
        let cx = if chunk_start == 0 { x } else { layout.x(chunk_col) + fragment.x_shift };
        spans.push(if space { invisible_span(chunk, chunk_col, cx, display) } else { styled(chunk, cx, style) });
    }
}

fn styled(text: &str, x: f32, style: i32) -> Span {
    Span { x, text: SharedString::from(text), style, bold: matches!(style, 17 | 19), italic: style == 18 }
}

/// Control characters other than tab (line breaks never reach a line's text).
fn is_control(ch: char) -> bool {
    ch != '\t' && (ch < ' ' || ch == '\u{7f}')
}

/// Spaces as middle dots, and the start of each tab as an arrow.
fn invisible_span(piece: &str, start: usize, x: f32, display: &DisplayLine) -> Span {
    let text: String = piece
        .chars()
        .enumerate()
        .map(|(i, _)| if display.tab_starts.binary_search(&(start + i)).is_ok() { '→' } else { '·' })
        .collect();
    Span { x, text: text.into(), style: INVISIBLE_STYLE, ..Default::default() }
}

/// Where indent guides go on a row: at the start of each indentation level inside the line's
/// indentation.
pub fn guides(indent_cells: usize, unit_cells: usize, cell: f32) -> Vec<f32> {
    let unit = unit_cells.max(1);
    (0..)
        .map(|level| level * unit)
        .take_while(|&cells| cells < indent_cells)
        .map(|cells| (cells as f32 * cell).round() + 0.5)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_lines_expand_tabs_and_show_composing_text() {
        let line = DisplayLine::new("\tab\tc", None, 4);
        assert_eq!(line.text, "    ab  c");
        assert_eq!(line.col(1), 4);
        assert_eq!(line.col(3), 6);
        assert_eq!(line.col(4), 8);
        assert_eq!(line.source_col(5), 2);
        assert_eq!(line.source_col(7), 3);
        assert_eq!(line.indent_cells, 4);
        assert_eq!(line.tab_starts, vec![0, 6]);

        let composing = DisplayLine::new("ab", Some((1, "にほ")), 4);
        assert_eq!(composing.text, "aにほb");
        assert_eq!(composing.preedit, Some((1, 3)));
        assert_eq!(composing.col(2), 4);
        let at_end = DisplayLine::new("ab", Some((2, "x")), 4);
        assert_eq!(at_end.text, "abx");
        assert_eq!(at_end.preedit, Some((2, 3)));
        assert!(DisplayLine::new("   ", None, 4).blank);
        // Tab stops count wide characters as two cells.
        assert_eq!(DisplayLine::new("名\tx", None, 4).text, "名  x");
        // Control characters show as symbols in the same columns.
        let binary = DisplayLine::new("a\u{0}\u{1b}b\u{7f}", None, 4);
        assert_eq!(binary.text, "a  b ");
        assert_eq!(binary.controls, vec![1, 2, 4]);
        assert_eq!(binary.control_columns(), vec![1, 2, 4]);
    }

    #[test]
    fn guides_sit_inside_the_indentation() {
        assert_eq!(guides(8, 4, 10.0), vec![0.5, 40.5]);
        assert_eq!(guides(9, 4, 10.0), vec![0.5, 40.5, 80.5]);
        assert_eq!(guides(4, 4, 10.0), vec![0.5]);
        assert!(guides(0, 4, 10.0).is_empty());
    }
}
