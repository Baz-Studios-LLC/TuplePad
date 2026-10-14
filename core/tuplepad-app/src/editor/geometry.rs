//! Where a line's text sits on screen: where it wraps, and the x of every column. Lines up to
//! `LONG_LINE` columns are laid out exactly (parley, over Slint's fonts). Longer ones, like
//! minified files and giant log lines, sit on a monospace grid, and only the part on screen is
//! read and drawn.

use std::rc::Rc;

use slint::SharedString;

use super::display::{self, DisplayLine, Fragment, INVISIBLE_STYLE};
use super::layout::LineLayout;
use crate::{Band, Span};

/// Lines longer than this (in UTF-16 columns) use the grid.
pub const LONG_LINE: usize = 4096;

enum Kind {
    /// The whole line, laid out exactly.
    Laid { display: DisplayLine, layout: Rc<LineLayout> },
    /// A very long line on a grid of `cell`-wide columns; `text` is the part from column `start`.
    Grid { cell: f32, start: usize, text: String },
}

pub struct LineGeometry {
    kind: Kind,
    /// Source columns where continuation rows start.
    breaks: Vec<usize>,
    /// How far continuation rows are indented, in pixels.
    indent_x: f32,
    /// The line's length in source columns.
    len: usize,
    /// Where composing text was inserted, in source columns.
    preedit_at: Option<usize>,
    pub blank: bool,
    pub indent_cells: usize,
}

impl LineGeometry {
    pub fn laid(display: DisplayLine, layout: Rc<LineLayout>, breaks: Vec<usize>, indent_x: f32, len: usize, preedit_at: Option<usize>) -> Self {
        let (blank, indent_cells) = (display.blank, display.indent_cells);
        LineGeometry { kind: Kind::Laid { display, layout }, breaks, indent_x, len, preedit_at, blank, indent_cells }
    }

    pub fn grid(cell: f32, start: usize, text: String, breaks: Vec<usize>, indent_x: f32, len: usize) -> Self {
        let indent = if start == 0 { text.chars().take_while(|c| *c == ' ' || *c == '\t').count() } else { 0 };
        LineGeometry { kind: Kind::Grid { cell, start, text }, breaks, indent_x, len, preedit_at: None, blank: false, indent_cells: indent }
    }

    /// Identifies the text drawn, so unchanged rows aren't rebuilt.
    pub fn content_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        match &self.kind {
            Kind::Laid { display, .. } => (&display.text, display.preedit).hash(&mut hasher),
            Kind::Grid { start, text, .. } => (start, text, self.len).hash(&mut hasher),
        }
        hasher.finish()
    }

    pub fn rows(&self) -> usize {
        self.breaks.len() + 1
    }

    /// The source columns a row covers, and whether it ends the line.
    pub fn row_range(&self, sub: usize) -> (usize, usize, bool) {
        let sub = sub.min(self.breaks.len());
        let start = if sub == 0 { 0 } else { self.breaks[sub - 1] };
        let last = sub == self.breaks.len();
        (start, if last { self.len } else { self.breaks[sub] }, last)
    }

    /// The x of a source column on the unwrapped line.
    fn line_x(&self, col: usize) -> f32 {
        match &self.kind {
            Kind::Laid { display, layout } => layout.x(display.col(col)),
            Kind::Grid { cell, .. } => col as f32 * cell,
        }
    }

    /// How far a row's text moves from its unwrapped place.
    fn shift(&self, sub: usize) -> f32 {
        if sub == 0 { 0.0 } else { self.indent_x - self.line_x(self.row_range(sub).0) }
    }

    /// The row a source column is drawn on: a continuation row starts at its break.
    pub fn sub_of(&self, col: usize) -> usize {
        self.breaks.iter().take_while(|&&b| b <= col).count()
    }

    /// The row and x of a source column.
    pub fn locate(&self, col: usize) -> (usize, f32) {
        let sub = self.sub_of(col);
        (sub, self.line_x(col) + self.shift(sub))
    }

    /// Where the main caret goes while composing: after the composing text.
    pub fn locate_after_preedit(&self) -> Option<(usize, f32)> {
        let (Kind::Laid { display, layout }, Some(at)) = (&self.kind, self.preedit_at) else { return None };
        let (_, end) = display.preedit?;
        let sub = self.sub_of(at);
        Some((sub, layout.x(end) + self.shift(sub)))
    }

    /// The underline under composing text: its row and x range.
    pub fn preedit_band(&self, first_row: u64) -> Option<Band> {
        let (Kind::Laid { display, layout }, Some(at)) = (&self.kind, self.preedit_at) else { return None };
        let (start, end) = display.preedit?;
        let sub = self.sub_of(at);
        let shift = self.shift(sub);
        let (x0, x1) = (layout.x(start) + shift, layout.x(end) + shift);
        Some(Band { row: (first_row + sub as u64) as i32, x: x0, width: x1 - x0 })
    }

    /// The source column nearest to `x` on one of the line's rows.
    pub fn column_at(&self, sub: usize, x: f32) -> usize {
        let (start, end, last) = self.row_range(sub);
        let x = x - self.shift(sub);
        let col = match &self.kind {
            Kind::Laid { display, layout } => display.source_col(layout.column_at(x)),
            Kind::Grid { cell, .. } => (x / cell).round().max(0.0) as usize,
        };
        let mut col = col.clamp(start, end);
        if !last && col >= end && end > start {
            // The end of a wrapped row is the start of the next one; stay on this row.
            col = end - 1;
        }
        col
    }

    /// Bands covering source columns `from..to`, one per row they cross. `through_eol` adds
    /// `eol` of width at the end of the line, for a selected line break.
    pub fn bands(&self, first_row: u64, from: usize, to: usize, through_eol: bool, eol: f32) -> Vec<Band> {
        let mut bands = Vec::new();
        for sub in 0..self.rows() {
            let (start, end, last) = self.row_range(sub);
            let (s, e) = (from.max(start), to.min(end));
            if e < s || (e == s && !(last && through_eol)) {
                continue;
            }
            let shift = self.shift(sub);
            let x0 = self.line_x(s) + shift;
            let mut x1 = self.line_x(e) + shift;
            if last && through_eol {
                x1 += eol;
            }
            if x1 > x0 {
                bands.push(Band { row: (first_row + sub as u64) as i32, x: x0, width: x1 - x0 });
            }
        }
        bands
    }

    /// The colored runs of one row. `colors` are (start, end, style) in source columns.
    pub fn row_spans(&self, sub: usize, colors: &[(usize, usize, i32)], invisibles: bool, line_break: bool) -> Vec<Span> {
        let (start, end, last) = self.row_range(sub);
        let shift = self.shift(sub);
        match &self.kind {
            Kind::Laid { display, layout } => {
                let fragment = Fragment {
                    // The first row starts at the start, composing text included.
                    start: if sub == 0 { 0 } else { display.col(start) },
                    end: if last { layout.col_x.len() - 1 } else { display.col(end) },
                    x_shift: shift,
                    last,
                };
                display::row_spans(display, layout, colors, &fragment, invisibles, line_break)
            }
            Kind::Grid { cell, start: text_start, text } => {
                grid_spans(text, *text_start, start, end, *cell, shift, colors, invisibles, line_break && last, self.len)
            }
        }
    }
}

/// Runs of a grid line: each run is placed at its column, so wide or proportional characters
/// can only drift within their own run.
#[allow(clippy::too_many_arguments)]
fn grid_spans(
    text: &str,
    text_start: usize,
    from: usize,
    to: usize,
    cell: f32,
    shift: f32,
    colors: &[(usize, usize, i32)],
    invisibles: bool,
    line_break: bool,
    len: usize,
) -> Vec<Span> {
    let style_at = |col: usize| colors.iter().rev().find(|(s, e, _)| *s <= col && col < *e).map_or(0, |c| c.2);
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_start = 0;
    let mut run_style = 0;
    let mut run_space = false;
    let mut col = text_start;
    let mut flush = |run: &mut String, start: usize, style: i32, space: bool| {
        if run.is_empty() {
            return;
        }
        if !space || invisibles {
            let x = start as f32 * cell + shift;
            let (text, style) = if space { (run.chars().map(|c| if c == '\t' { '→' } else { '·' }).collect(), INVISIBLE_STYLE) } else { (run.clone(), style) };
            spans.push(Span { x, text: SharedString::from(text), style, bold: matches!(style, 17 | 19), italic: style == 18 });
        }
        run.clear();
    };
    for ch in text.chars() {
        let units = ch.len_utf16();
        if col + units > to {
            break;
        }
        if col >= from {
            let space = ch == ' ' || ch == '\t';
            let style = if space { 0 } else { style_at(col) };
            if run.is_empty() || space != run_space || style != run_style || run.len() > 256 {
                flush(&mut run, run_start, run_style, run_space);
                run_start = col;
                run_style = style;
                run_space = space;
            }
            run.push(if ch == '\t' { ' ' } else { ch });
        }
        col += units;
    }
    flush(&mut run, run_start, run_style, run_space);
    if invisibles && line_break && to >= len {
        spans.push(Span { x: len as f32 * cell + shift + 1.0, text: "¬".into(), style: INVISIBLE_STYLE, ..Default::default() });
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_lines_place_runs_by_column() {
        let geometry = LineGeometry::grid(10.0, 100, "ab  cd".into(), vec![], 0.0, 10_000);
        assert_eq!(geometry.locate(103), (0, 1030.0));
        assert_eq!(geometry.column_at(0, 1047.0), 105);
        let spans = geometry.row_spans(0, &[(104, 106, 1)], false, false);
        let placed: Vec<(f32, String, i32)> = spans.iter().map(|s| (s.x, s.text.to_string(), s.style)).collect();
        assert_eq!(placed, vec![(1000.0, "ab".into(), 0), (1040.0, "cd".into(), 1)]);
        let visible = geometry.row_spans(0, &[], true, false);
        assert_eq!(visible[1].text.as_str(), "··");
    }

    #[test]
    fn wrapped_rows_and_bands() {
        let geometry = LineGeometry::grid(10.0, 0, "x".repeat(30), vec![10, 20], 40.0, 30);
        assert_eq!(geometry.rows(), 3);
        assert_eq!(geometry.row_range(1), (10, 20, false));
        assert_eq!(geometry.locate(10), (1, 40.0));
        assert_eq!(geometry.column_at(1, 1000.0), 19);
        assert_eq!(geometry.column_at(2, 1000.0), 30);
        let bands = geometry.bands(5, 5, 25, false, 0.0);
        let rows: Vec<(i32, f32, f32)> = bands.iter().map(|b| (b.row, b.x, b.width)).collect();
        assert_eq!(rows, vec![(5, 50.0, 50.0), (6, 40.0, 100.0), (7, 40.0, 50.0)]);
    }
}
