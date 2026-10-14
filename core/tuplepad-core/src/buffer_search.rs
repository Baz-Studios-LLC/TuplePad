//! Searching a document for result lists (Find in Open Documents), counting matches, previewing
//! replacements, and bookmarked lines. Bookmarks aren't text, so changing them doesn't change
//! the buffer's version.

use super::*;
use crate::search::{LineMatches, ReplacementPreview};

/// Search results grouped by line, for a results list.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct LineResults {
    pub lines: Vec<LineMatches>,
    /// Every match, including any on lines past the limit.
    pub match_count: u64,
    /// There were more matching lines than the limit.
    pub truncated: bool,
}

/// How an edit touched lines, measured before it was applied.
#[derive(Clone, Copy)]
pub(super) struct LineEdit {
    line: usize,
    removed_breaks: usize,
    at: usize,
    /// The edit starts at the beginning of a line and removes only whole lines, so that line's
    /// own text (and its bookmark) moves along with the lines after it.
    before_line_text: bool,
}

#[uniffi::export]
impl TextBuffer {
    /// How many matches there are in the whole document.
    pub fn count_matches(&self, query: SearchQuery) -> CoreResult<u64> {
        let Some(regex) = search::compile(&query)? else { return Ok(0) };
        let st = self.read();
        Ok(search::count(&st.rope, &regex, &query))
    }

    /// Matches grouped by line with previews, for search results. At most `max_lines` lines.
    pub fn find_lines(&self, query: SearchQuery, max_lines: u64) -> CoreResult<LineResults> {
        let Some(regex) = search::compile(&query)? else {
            return Ok(LineResults { lines: Vec::new(), match_count: 0, truncated: false });
        };
        let st = self.read();
        let (found, _) = search::find(&st.rope, &regex, &query, None, usize::MAX);
        let lines = search::group_by_line(&st.rope, &found, max_lines as usize);
        let shown_lines = lines.len();
        let truncated = found.last().is_some_and(|f| {
            lines.last().is_some_and(|last| st.rope.char_to_line(f.start) as u64 > last.line)
        }) && shown_lines as u64 == max_lines;
        Ok(LineResults { lines, match_count: found.len() as u64, truncated })
    }

    /// What Replace All would change, as before/after stretches of lines (at most `max_groups`).
    pub fn replacement_preview(&self, query: SearchQuery, replacement: String, max_groups: u64) -> CoreResult<Vec<ReplacementPreview>> {
        let Some(regex) = search::compile(&query)? else { return Ok(Vec::new()) };
        let st = self.read();
        let (found, _) = search::find(&st.rope, &regex, &query, Some(&replacement), usize::MAX);
        Ok(search::preview_replacements(&st.rope, &found, max_groups as usize))
    }

    // MARK: Bookmarks

    /// Bookmarked lines, in order.
    pub fn bookmarks(&self) -> Vec<u64> {
        let st = self.read();
        let lines = st.rope.len_lines();
        st.bookmarks.iter().filter(|&&line| line < lines).map(|&line| line as u64).collect()
    }

    /// Replaces all bookmarks (for restoring a session).
    pub fn set_bookmarks(&self, lines: Vec<u64>) {
        let mut st = self.write();
        st.bookmarks = lines.into_iter().map(|line| line as usize).collect();
    }

    /// Bookmarks a line, or removes its bookmark. Returns whether it's bookmarked now.
    pub fn toggle_bookmark(&self, line: u64) -> bool {
        let mut st = self.write();
        let line = st.clamp_line(line);
        st.bookmarks.insert(line) || !st.bookmarks.remove(&line)
    }

    pub fn clear_bookmarks(&self) {
        let mut st = self.write();
        st.bookmarks.clear();
    }

    /// The next bookmark after `line` (or before it, going backward), wrapping around.
    pub fn next_bookmark(&self, line: u64, forward: bool) -> Option<u64> {
        let marks = self.bookmarks();
        let next = if forward {
            marks.iter().find(|&&mark| mark > line).or(marks.first())
        } else {
            marks.iter().rev().find(|&&mark| mark < line).or(marks.last())
        };
        next.copied()
    }

    /// Bookmarks every line with a match. Returns how many lines were newly bookmarked.
    pub fn bookmark_matching_lines(&self, query: SearchQuery) -> CoreResult<u64> {
        let Some(regex) = search::compile(&query)? else { return Ok(0) };
        let mut st = self.write();
        let (found, _) = search::find(&st.rope, &regex, &query, None, usize::MAX);
        let lines: Vec<usize> = found.iter().map(|f| st.rope.char_to_line(f.start)).collect();
        let added = lines.into_iter().filter(|&line| st.bookmarks.insert(line)).count();
        Ok(added as u64)
    }

    /// The text of every bookmarked line, one per line, for copying.
    pub fn bookmarked_text(&self) -> String {
        let marks = self.bookmarks();
        let st = self.read();
        marks.iter().map(|&line| st.line_content(line as usize).to_string()).collect::<Vec<_>>().join("\n")
    }

    /// Deletes every bookmarked line as one undo step. Returns where the carets end up.
    pub fn delete_bookmarked_lines(&self, cursors: Vec<TextSelection>) -> Vec<TextSelection> {
        let marks = self.bookmarks();
        let mut st = self.write();
        if marks.is_empty() {
            return cursors;
        }
        // Runs of consecutive lines become one edit each.
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for line in marks.into_iter().map(|line| line as usize) {
            match runs.last_mut() {
                Some(run) if run.1 + 1 == line => run.1 = line,
                _ => runs.push((line, line)),
            }
        }
        let edits = runs.iter().map(|&(first, last)| st.block_edit(first, last, &[])).collect();
        let old = st.cursor_chars(&cursors);
        st.apply_edits(edits, cursors, EditKind::Other, |st, map| st.mapped(&old, map))
    }
}

impl State {
    /// Notes where an edit of the chars `at..end` starts, before it's applied.
    pub(super) fn line_edit_start(&self, at: usize, end: usize) -> LineEdit {
        let line = self.rope.char_to_line(at);
        let end_line = self.rope.char_to_line(end);
        let before_line_text = at == self.rope.line_to_char(line) && end == self.rope.line_to_char(end_line);
        LineEdit { line, removed_breaks: end_line - line, at, before_line_text }
    }

    /// Keeps bookmarks on the same text after an edit: lines below shift, and lines whose text
    /// was deleted lose their bookmark.
    pub(super) fn move_bookmarks(&mut self, edit: LineEdit, inserted_end: usize) {
        let inserted_breaks = self.rope.char_to_line(inserted_end) - self.rope.char_to_line(edit.at);
        if edit.removed_breaks == 0 && inserted_breaks == 0 {
            return;
        }
        let delta = inserted_breaks as isize - edit.removed_breaks as isize;
        // Lines whose text was removed, and the first line that moves.
        let (gone, first_moving) = if edit.before_line_text {
            (edit.line..edit.line + edit.removed_breaks, edit.line + edit.removed_breaks)
        } else {
            (edit.line + 1..edit.line + 1 + edit.removed_breaks, edit.line + 1 + edit.removed_breaks)
        };
        self.bookmarks = std::mem::take(&mut self.bookmarks)
            .into_iter()
            .filter(|line| !gone.contains(line))
            .map(|line| if line >= first_moving { (line as isize + delta) as usize } else { line })
            .collect();
    }
}
