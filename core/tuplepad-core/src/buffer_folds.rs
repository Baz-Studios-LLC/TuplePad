//! Code folding: which lines can fold (from the syntax tree, or indentation), and which are folded.

use super::*;
use super::marks::LineEdit;

/// A foldable (or folded) region: `start` stays visible, `start + 1 ..= end` hide.
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRange {
    pub start: u64,
    pub end: u64,
}

/// Folding is off for documents longer than this, to keep them fast.
const MAX_FOLDING_LINES: usize = 400_000;
/// Indentation folds look this far ahead for the end of a block.
const MAX_INDENT_SCAN: usize = 20_000;

#[uniffi::export]
impl TextBuffer {
    /// Regions that start on lines `start_line ..< start_line + count` and can fold.
    pub fn fold_ranges(&self, start_line: u64, count: u64, tab_width: u32) -> Vec<FoldRange> {
        let st = self.read();
        st.foldable(start_line as usize, count as usize, tab_width as usize)
            .into_iter()
            .map(|(start, end)| FoldRange { start: start as u64, end: end as u64 })
            .collect()
    }

    /// Folded regions, in order.
    pub fn folds(&self) -> Vec<FoldRange> {
        let st = self.read();
        let lines = st.rope.len_lines();
        st.folds
            .iter()
            .filter(|&(_, &end)| end < lines)
            .map(|(&start, &end)| FoldRange { start: start as u64, end: end as u64 })
            .collect()
    }

    /// Restores folds (from a saved session).
    pub fn set_folds(&self, folds: Vec<FoldRange>) {
        let mut st = self.write();
        st.folds = folds.into_iter().filter(|f| f.end > f.start).map(|f| (f.start as usize, f.end as usize)).collect();
    }

    /// Folds the innermost foldable region around `line` that isn't folded yet. Returns it.
    pub fn fold(&self, line: u64, tab_width: u32) -> Option<FoldRange> {
        let mut st = self.write();
        let line = line as usize;
        // Regions starting on this line or up to a few thousand lines above that contain it.
        let first = line.saturating_sub(5_000);
        let mut candidates = st.foldable(first, line - first + 1, tab_width as usize);
        candidates.retain(|&(start, end)| start <= line && line <= end && !st.folds.contains_key(&start));
        let (start, end) = candidates.into_iter().max_by_key(|&(start, _)| start)?;
        st.folds.insert(start, end);
        Some(FoldRange { start: start as u64, end: end as u64 })
    }

    /// Folds the region starting on `line`, or unfolds it if it's folded. Returns whether it's folded now.
    pub fn toggle_fold(&self, line: u64, tab_width: u32) -> bool {
        let mut st = self.write();
        let line = line as usize;
        if st.folds.remove(&line).is_some() {
            return false;
        }
        let Some(&(start, end)) = st.foldable(line, 1, tab_width as usize).first() else { return false };
        st.folds.insert(start, end);
        true
    }

    /// Unfolds the folds that hide `line` or start on it. Returns whether anything changed.
    pub fn unfold(&self, line: u64) -> bool {
        let mut st = self.write();
        let line = line as usize;
        let before = st.folds.len();
        st.folds.retain(|&start, &mut end| !(start <= line && line <= end));
        st.folds.len() != before
    }

    /// Unfolds whatever hides any of these lines (so carets are never inside a fold).
    pub fn reveal_lines(&self, lines: Vec<u64>) -> bool {
        let mut st = self.write();
        let before = st.folds.len();
        st.folds.retain(|&start, &mut end| !lines.iter().any(|&line| start < line as usize && line as usize <= end));
        st.folds.len() != before
    }

    /// Folds every region (the outermost ones end up visible).
    pub fn fold_all(&self, tab_width: u32) {
        let mut st = self.write();
        let lines = st.rope.len_lines();
        let all = st.foldable(0, lines, tab_width as usize);
        st.folds.extend(all);
    }

    pub fn unfold_all(&self) {
        self.write().folds.clear();
    }
}

impl State {
    /// Foldable regions starting in a range of lines, from the syntax tree or by indentation.
    fn foldable(&self, start_line: usize, count: usize, tab_width: usize) -> Vec<(usize, usize)> {
        let lines = self.rope.len_lines();
        if lines > MAX_FOLDING_LINES || start_line >= lines || count == 0 {
            return Vec::new();
        }
        let last = (start_line + count - 1).min(lines - 1);
        if let Some(ranges) = self.syntax.as_ref().and_then(|syntax| syntax.fold_ranges(&self.rope, start_line, last)) {
            return ranges;
        }
        (start_line..=last).filter_map(|line| self.indent_fold(line, tab_width.max(1)).map(|end| (line, end))).collect()
    }

    /// A line's indentation in cells, or nil for a blank line.
    fn indent_width(&self, line: usize, tab_width: usize) -> Option<usize> {
        let mut width = 0;
        for c in self.line_content(line).chars() {
            match c {
                ' ' => width += 1,
                '\t' => width += tab_width - width % tab_width,
                c if c.is_whitespace() => {}
                _ => return Some(width),
            }
        }
        None
    }

    /// The block under a line that's indented more deeply than it, ending at its last non-blank line.
    fn indent_fold(&self, line: usize, tab_width: usize) -> Option<usize> {
        let base = self.indent_width(line, tab_width)?;
        let lines = self.rope.len_lines();
        let limit = (line + MAX_INDENT_SCAN).min(lines);
        let mut end = None;
        for next in line + 1..limit {
            match self.indent_width(next, tab_width) {
                None => continue,
                Some(width) if width > base => end = Some(next),
                Some(_) => break,
            }
        }
        end
    }

    /// Keeps folds on their text after an edit. A fold whose lines were edited opens; typing on
    /// a fold's first line keeps it.
    pub(super) fn move_folds(&mut self, edit: LineEdit, inserted_breaks: usize) {
        let edit_first = edit.line;
        let edit_last = edit.line + edit.removed_breaks;
        let delta = inserted_breaks as isize - edit.removed_breaks as isize;
        let first_moving = if edit.before_line_text { edit_last } else { edit_last + 1 };
        let within_line = edit.removed_breaks == 0 && inserted_breaks == 0;
        self.folds = std::mem::take(&mut self.folds)
            .into_iter()
            .filter_map(|(start, end)| {
                if end < edit_first {
                    Some((start, end))
                } else if start >= first_moving {
                    Some(((start as isize + delta) as usize, (end as isize + delta) as usize))
                } else if start == edit_first && within_line {
                    Some((start, end))
                } else {
                    None
                }
            })
            .collect();
    }
}
