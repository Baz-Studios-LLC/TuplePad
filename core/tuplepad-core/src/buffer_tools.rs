//! The Text menu's tools on `TextBuffer`: line operations, case changes, comments, moving and
//! duplicating lines, bracket matching, word wrap layout, and searching just the visible lines.
//! Each tool is one undo step.

use super::*;
use crate::search::{self, SearchQuery};
use crate::transform::{self, CaseChange, LineOperation};
use crate::wrap::{self, WrappedLine};

/// A bracket at the caret and its partner.
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq)]
pub struct BracketMatch {
    pub open: TextPos,
    pub close: TextPos,
}

/// Replaces the chars `start..end`, measured in the text before the batch is applied.
pub(super) struct CharEdit {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) text: String,
}

/// Where a char offset ends up after a batch of edits, given as sorted (start, end, inserted chars).
/// Text inserted exactly at the offset pushes it along; an offset inside replaced text keeps its
/// distance from the start, clamped to the new text.
fn map_position(edits: &[(usize, usize, usize)], position: usize) -> usize {
    let mut delta: isize = 0;
    for &(start, end, inserted) in edits {
        if position < start || (position == start && start != end) {
            break;
        }
        if position >= end {
            delta += inserted as isize - (end - start) as isize;
            continue;
        }
        return (start as isize + delta) as usize + (position - start).min(inserted);
    }
    (position as isize + delta) as usize
}

impl State {
    fn content_end(&self, line: usize) -> usize {
        self.rope.line_to_char(line) + self.line_content(line).len_chars()
    }

    fn indent_chars(&self, line: usize) -> usize {
        self.line_content(line).chars().take_while(|c| *c == ' ' || *c == '\t').count()
    }

    fn is_blank(&self, line: usize) -> bool {
        self.line_content(line).chars().all(char::is_whitespace)
    }

    /// The lines each cursor touches, merged into blocks (blocks next to each other merge too).
    /// A selection ending at column 0 stops at the line before.
    fn line_blocks(&self, cursors: &[TextSelection]) -> Vec<(usize, usize)> {
        let mut blocks: Vec<(usize, usize)> = cursors
            .iter()
            .map(|c| {
                let (start, end) = (self.clamp_line(c.start().line), self.clamp_line(c.end().line));
                let end = if c.end().column == 0 && end > start { end - 1 } else { end };
                (start, end)
            })
            .collect();
        blocks.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(blocks.len());
        for (start, end) in blocks {
            match merged.last_mut() {
                Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        merged
    }

    /// Cursors as (anchor, head) char offsets.
    pub(super) fn cursor_chars(&self, cursors: &[TextSelection]) -> Vec<(usize, usize)> {
        cursors.iter().map(|c| (self.pos_to_char(c.anchor), self.pos_to_char(c.head))).collect()
    }

    /// Replaces lines `first..=last` with `lines`. With no lines left, the block's line break goes too.
    pub(super) fn block_edit(&self, first: usize, last: usize, lines: &[String]) -> CharEdit {
        if lines.is_empty() {
            let (start, end) = if last + 1 < self.rope.len_lines() {
                (self.rope.line_to_char(first), self.rope.line_to_char(last + 1))
            } else if first > 0 {
                (self.content_end(first - 1), self.content_end(last))
            } else {
                (0, self.content_end(last))
            };
            return CharEdit { start, end, text: String::new() };
        }
        CharEdit {
            start: self.rope.line_to_char(first),
            end: self.content_end(last),
            text: lines.join(self.line_ending.as_str()),
        }
    }

    fn lines_text(&self, first: usize, last: usize) -> Vec<String> {
        (first..=last).map(|line| self.line_content(line).to_string()).collect()
    }

    /// Applies non-overlapping edits as one undo step. `after` gets the updated state and a function
    /// mapping old char offsets to new ones, and returns the cursors to show.
    pub(super) fn apply_edits(
        &mut self,
        mut edits: Vec<CharEdit>,
        before: Vec<TextSelection>,
        kind: EditKind,
        after: impl FnOnce(&State, &dyn Fn(usize) -> usize) -> Vec<TextSelection>,
    ) -> Vec<TextSelection> {
        edits.retain(|e| e.start != e.end || !e.text.is_empty());
        edits.retain(|e| e.start == e.end || self.rope.slice(e.start..e.end) != e.text.as_str());
        if edits.is_empty() {
            return before;
        }
        edits.sort_by_key(|e| e.start);
        let mapping: Vec<(usize, usize, usize)> = edits.iter().map(|e| (e.start, e.end, e.text.chars().count())).collect();
        let mut changes = Vec::with_capacity(edits.len());
        // Back to front, so earlier offsets stay valid.
        for edit in edits.iter().rev() {
            let removed = self.rope.slice(edit.start..edit.end).to_string();
            self.replace_chars(edit.start, edit.end, &edit.text);
            changes.push(Change::Splice { at: edit.start, removed, inserted: edit.text.clone() });
        }
        self.reparse();
        let map = |position: usize| map_position(&mapping, position);
        let cursors = after(self, &map);
        self.history.record(changes, before, cursors.clone(), kind);
        self.version += 1;
        cursors
    }

    pub(super) fn mapped(&self, old: &[(usize, usize)], map: &dyn Fn(usize) -> usize) -> Vec<TextSelection> {
        old.iter()
            .map(|&(anchor, head)| TextSelection { anchor: self.char_to_pos(map(anchor)), head: self.char_to_pos(map(head)) })
            .collect()
    }

    /// The word (letters, digits and underscores) at or just before a char offset.
    fn word_around(&self, at: usize) -> Option<(usize, usize)> {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let len = self.rope.len_chars();
        let probe = if at < len && is_word(self.rope.char(at)) {
            at
        } else if at > 0 && is_word(self.rope.char(at - 1)) {
            at - 1
        } else {
            return None;
        };
        let mut start = probe;
        while start > 0 && is_word(self.rope.char(start - 1)) {
            start -= 1;
        }
        let mut end = probe + 1;
        while end < len && is_word(self.rope.char(end)) {
            end += 1;
        }
        Some((start, end))
    }
}

#[uniffi::export]
impl TextBuffer {
    /// Sorts, removes, joins or cleans up the selected lines, or the whole document if nothing is selected.
    pub fn transform_lines(&self, operation: LineOperation, cursors: Vec<TextSelection>, tab_width: u32) -> Vec<TextSelection> {
        self.edit_lines(cursors, operation.keeps_lines(), |lines| transform::apply_to_lines(operation, lines, tab_width as usize))
    }

    /// Hard-wraps the selected lines (or the whole document) at `width` columns.
    pub fn split_lines(&self, width: u32, tab_width: u32, cursors: Vec<TextSelection>) -> Vec<TextSelection> {
        self.edit_lines(cursors, false, |lines| {
            lines.iter().flat_map(|line| transform::split_line(line, width as usize, tab_width as usize)).collect()
        })
    }

    /// Changes the case of each selection, or of the word at each caret.
    pub fn change_case(&self, change: CaseChange, cursors: Vec<TextSelection>) -> Vec<TextSelection> {
        let mut st = self.write();
        let old = st.cursor_chars(&cursors);
        let mut ranges: Vec<(usize, usize)> = old
            .iter()
            .filter_map(|&(a, h)| if a != h { Some((a.min(h), a.max(h))) } else { st.word_around(h) })
            .collect();
        ranges.sort_unstable();
        ranges.dedup_by(|b, a| b.0 < a.1); // drop ranges overlapping the previous one
        let edits: Vec<CharEdit> = ranges
            .iter()
            .map(|&(start, end)| {
                let text = transform::change_case(&st.rope.slice(start..end).to_string(), change);
                CharEdit { start, end, text }
            })
            .collect();
        let lengths: Vec<(usize, usize, usize)> = edits.iter().map(|e| (e.start, e.end, e.text.chars().count())).collect();
        st.apply_edits(edits, cursors, EditKind::Other, move |st, map| {
            old.iter()
                .map(|&(anchor, head)| {
                    // A selection keeps covering its (possibly longer) new text.
                    let covered = lengths.iter().find(|r| anchor != head && r.0 == anchor.min(head) && r.1 == anchor.max(head));
                    let (a, h) = match covered {
                        Some(&(start, _, length)) => {
                            let new_start = map(start);
                            if anchor < head { (new_start, new_start + length) } else { (new_start + length, new_start) }
                        }
                        None => (map(anchor), map(head)),
                    };
                    TextSelection { anchor: st.char_to_pos(a), head: st.char_to_pos(h) }
                })
                .collect()
        })
    }

    /// Comments or uncomments the lines with cursors, using the language's comment style.
    /// Returns the cursors unchanged for plain text.
    pub fn toggle_comment(&self, cursors: Vec<TextSelection>) -> Vec<TextSelection> {
        let mut st = self.write();
        let Some(tokens) = st.syntax.as_ref().and_then(|s| s.comment_tokens()) else { return cursors };
        let old = st.cursor_chars(&cursors);
        let blocks = st.line_blocks(&cursors);
        let mut edits = Vec::new();

        if let Some(token) = tokens.line {
            let lines: Vec<usize> = blocks.iter().flat_map(|&(a, b)| a..=b).filter(|&l| !st.is_blank(l)).collect();
            let commented = |line: usize| {
                let content = st.line_content(line).to_string();
                content.trim_start_matches([' ', '\t']).starts_with(token)
            };
            if !lines.is_empty() && lines.iter().all(|&l| commented(l)) {
                for &line in &lines {
                    let start = st.rope.line_to_char(line) + st.indent_chars(line);
                    let mut end = start + token.chars().count();
                    if end < st.content_end(line) && st.rope.char(end) == ' ' {
                        end += 1;
                    }
                    edits.push(CharEdit { start, end, text: String::new() });
                }
            } else {
                for &(first, last) in &blocks {
                    let block_lines: Vec<usize> = (first..=last).filter(|&l| !st.is_blank(l)).collect();
                    let indent = block_lines.iter().map(|&l| st.indent_chars(l)).min().unwrap_or(0);
                    for line in block_lines {
                        let at = st.rope.line_to_char(line) + indent;
                        edits.push(CharEdit { start: at, end: at, text: format!("{token} ") });
                    }
                }
            }
        } else if let Some((open, close)) = tokens.block {
            for &(first, last) in &blocks {
                let Some(top) = (first..=last).find(|&l| !st.is_blank(l)) else { continue };
                let bottom = (first..=last).rev().find(|&l| !st.is_blank(l)).unwrap_or(top);
                let start = st.rope.line_to_char(top) + st.indent_chars(top);
                let end = st.content_end(bottom);
                let text = st.rope.slice(start..end).to_string();
                let trimmed = text.trim_end();
                if trimmed.starts_with(open) && trimmed.ends_with(close) && trimmed.len() >= open.len() + close.len() {
                    let open_len = open.chars().count() + usize::from(text[open.len()..].starts_with(' '));
                    let close_start = start + trimmed.chars().count() - close.chars().count();
                    let space_before = close_start > start && st.rope.char(close_start - 1) == ' ';
                    edits.push(CharEdit { start, end: start + open_len, text: String::new() });
                    let from = if space_before && close_start - 1 >= start + open_len { close_start - 1 } else { close_start };
                    edits.push(CharEdit { start: from, end: close_start + close.chars().count(), text: String::new() });
                } else {
                    edits.push(CharEdit { start, end: start, text: format!("{open} ") });
                    edits.push(CharEdit { start: end, end, text: format!(" {close}") });
                }
            }
        }
        st.apply_edits(edits, cursors, EditKind::Other, move |st, map| st.mapped(&old, map))
    }

    /// Moves the lines with cursors up or down one line, cursors and all. Does nothing at the edges.
    pub fn move_lines(&self, up: bool, cursors: Vec<TextSelection>) -> Vec<TextSelection> {
        let mut st = self.write();
        let blocks = st.line_blocks(&cursors);
        let total = st.rope.len_lines();
        if blocks.iter().any(|&(first, last)| if up { first == 0 } else { last + 1 >= total }) {
            return cursors;
        }
        let ending = st.line_ending.as_str();
        let edits: Vec<CharEdit> = blocks
            .iter()
            .map(|&(first, last)| {
                let block = st.lines_text(first, last).join(ending);
                if up {
                    let above = st.line_content(first - 1).to_string();
                    CharEdit { start: st.rope.line_to_char(first - 1), end: st.content_end(last), text: format!("{block}{ending}{above}") }
                } else {
                    let below = st.line_content(last + 1).to_string();
                    CharEdit { start: st.rope.line_to_char(first), end: st.content_end(last + 1), text: format!("{below}{ending}{block}") }
                }
            })
            .collect();
        let moved = cursors.clone();
        st.apply_edits(edits, cursors, EditKind::Other, move |st, _| {
            let shift = |p: TextPos| {
                let line = if up { p.line - 1 } else { p.line + 1 };
                st.clamp_pos(TextPos::new(line, p.column))
            };
            moved.iter().map(|c| TextSelection { anchor: shift(c.anchor), head: shift(c.head) }).collect()
        })
    }

    /// Duplicates the lines with cursors below themselves and moves the cursors onto the copies.
    pub fn duplicate_lines(&self, cursors: Vec<TextSelection>) -> Vec<TextSelection> {
        let mut st = self.write();
        let blocks = st.line_blocks(&cursors);
        let ending = st.line_ending.as_str();
        let edits: Vec<CharEdit> = blocks
            .iter()
            .map(|&(first, last)| {
                let at = st.content_end(last);
                CharEdit { start: at, end: at, text: format!("{ending}{}", st.lines_text(first, last).join(ending)) }
            })
            .collect();
        let moved = cursors.clone();
        st.apply_edits(edits, cursors, EditKind::Other, move |st, _| {
            let shift = |p: TextPos| {
                // Lines added by this block and every block above it.
                let added: usize = blocks
                    .iter()
                    .filter(|&&(first, _)| first as u64 <= p.line)
                    .map(|&(first, last)| last - first + 1)
                    .sum();
                st.clamp_pos(TextPos::new(p.line + added as u64, p.column))
            };
            moved.iter().map(|c| TextSelection { anchor: shift(c.anchor), head: shift(c.head) }).collect()
        })
    }

    /// Deletes the lines with cursors, leaving a caret where each block was.
    pub fn delete_lines(&self, cursors: Vec<TextSelection>) -> Vec<TextSelection> {
        let mut st = self.write();
        let blocks = st.line_blocks(&cursors);
        let edits: Vec<CharEdit> = blocks.iter().map(|&(first, last)| st.block_edit(first, last, &[])).collect();
        let columns: Vec<u64> = blocks
            .iter()
            .map(|&(first, _)| cursors.iter().find(|c| c.head.line as usize >= first).map_or(0, |c| c.head.column))
            .collect();
        st.apply_edits(edits, cursors, EditKind::Other, move |st, _| {
            let mut removed = 0;
            let mut carets: Vec<TextSelection> = Vec::new();
            for (&(first, last), &column) in blocks.iter().zip(&columns) {
                let line = (first - removed) as u64;
                let caret = TextSelection::caret(st.clamp_pos(TextPos::new(line, column)));
                if carets.last() != Some(&caret) {
                    carets.push(caret);
                }
                removed += last - first + 1;
            }
            carets
        })
    }

    /// The word at a position, if there is one.
    pub fn word_at(&self, pos: TextPos) -> Option<TextSelection> {
        let st = self.read();
        let (start, end) = st.word_around(st.pos_to_char(pos))?;
        Some(TextSelection { anchor: st.char_to_pos(start), head: st.char_to_pos(end) })
    }

    /// The bracket just before or after a position and its partner, skipping brackets in strings and comments.
    pub fn matching_bracket(&self, pos: TextPos) -> Option<BracketMatch> {
        const LIMIT: usize = 200_000;
        let st = self.read();
        let at = st.pos_to_char(pos);
        let len = st.rope.len_chars();
        let pair = |c: char| match c {
            '(' => Some(('(', ')', true)),
            '[' => Some(('[', ']', true)),
            '{' => Some(('{', '}', true)),
            ')' => Some(('(', ')', false)),
            ']' => Some(('[', ']', false)),
            '}' => Some(('{', '}', false)),
            _ => None,
        };
        let candidates = [at.checked_sub(1), (at < len).then_some(at)];
        for index in candidates.into_iter().flatten() {
            let Some((open, close, forward)) = pair(st.rope.char(index)) else { continue };
            let skippable = |i: usize| match &st.syntax {
                Some(syntax) => syntax.in_string_or_comment(st.rope.char_to_byte(i)),
                None => false,
            };
            // Inside a string or comment, match plainly; otherwise ignore brackets in them.
            let skip = !skippable(index);
            let mut depth = 0usize;
            let mut found = None;
            if forward {
                for (i, c) in st.rope.chars_at(index + 1).enumerate().take(LIMIT) {
                    let i = index + 1 + i;
                    if c != open && c != close || (skip && skippable(i)) {
                        continue;
                    }
                    if c == open {
                        depth += 1;
                    } else if depth == 0 {
                        found = Some(i);
                        break;
                    } else {
                        depth -= 1;
                    }
                }
            } else {
                let mut chars = st.rope.chars_at(index);
                let mut i = index;
                while let Some(c) = chars.prev() {
                    i -= 1;
                    if index - i > LIMIT {
                        break;
                    }
                    if c != open && c != close || (skip && skippable(i)) {
                        continue;
                    }
                    if c == close {
                        depth += 1;
                    } else if depth == 0 {
                        found = Some(i);
                        break;
                    } else {
                        depth -= 1;
                    }
                }
            }
            if let Some(other) = found {
                let (a, b) = if forward { (index, other) } else { (other, index) };
                return Some(BracketMatch { open: st.char_to_pos(a), close: st.char_to_pos(b) });
            }
        }
        None
    }

    /// Matches of `query` within `count` lines from `start_line` (for highlighting what's on screen).
    pub fn find_in_lines(&self, query: SearchQuery, start_line: u64, count: u64, limit: u64) -> CoreResult<Vec<TextSelection>> {
        let Some(regex) = search::compile(&query)? else { return Ok(Vec::new()) };
        let st = self.read();
        let total = st.rope.len_lines();
        let first = (start_line as usize).min(total - 1);
        let last = (first + count as usize).min(total);
        let from = st.rope.line_to_char(first);
        let to = if last < total { st.rope.line_to_char(last) } else { st.rope.len_chars() };
        let slice = Rope::from(st.rope.slice(from..to));
        let (found, _) = search::find(&slice, &regex, &query, None, limit as usize);
        Ok(found
            .iter()
            .map(|f| TextSelection { anchor: st.char_to_pos(from + f.start), head: st.char_to_pos(from + f.end) })
            .collect())
    }

    /// The line break each line ends with (nil for the last line), for showing invisibles.
    pub fn line_endings(&self, start: u64, count: u64) -> Vec<Option<LineEnding>> {
        let st = self.read();
        let total = st.rope.len_lines();
        (start as usize..(start.saturating_add(count) as usize).min(total))
            .map(|line| {
                let slice = st.rope.line(line);
                let len = slice.len_chars();
                match (len.checked_sub(2).map(|i| slice.char(i)), len.checked_sub(1).map(|i| slice.char(i))) {
                    (Some('\r'), Some('\n')) => Some(LineEnding::CrLf),
                    (_, Some('\n')) => Some(LineEnding::Lf),
                    (_, Some('\r')) => Some(LineEnding::Cr),
                    _ => None,
                }
            })
            .collect()
    }

    /// Rows each line takes when soft-wrapped at `width` cells, for `count` lines from `start`.
    pub fn wrap_rows(&self, width: u32, tab_width: u32, start: u64, count: u64) -> Vec<u32> {
        let st = self.read();
        let total = st.rope.len_lines();
        (start as usize..(start.saturating_add(count) as usize).min(total))
            .map(|line| wrap::row_count(st.line_content(line), width as usize, tab_width as usize))
            .collect()
    }

    /// Where each of `count` lines from `start` wraps at `width` cells.
    pub fn wrap_layout(&self, width: u32, tab_width: u32, start: u64, count: u64) -> Vec<WrappedLine> {
        let st = self.read();
        let total = st.rope.len_lines();
        (start as usize..(start.saturating_add(count) as usize).min(total))
            .map(|line| {
                let text = st.line_content(line).to_string();
                let (breaks, indent) = wrap::wrap_str(&text, width as usize, tab_width as usize, true);
                WrappedLine { breaks, indent }
            })
            .collect()
    }
}

impl TextBuffer {
    /// Shared by the line operations: replaces the selected lines (or all lines) with `transform(lines)`.
    /// In-place operations edit only the lines that change, so cursors elsewhere don't move.
    fn edit_lines(&self, cursors: Vec<TextSelection>, in_place: bool, transform: impl Fn(Vec<String>) -> Vec<String>) -> Vec<TextSelection> {
        let mut st = self.write();
        let selections: Vec<TextSelection> = cursors.iter().copied().filter(|c| c.anchor != c.head).collect();
        let whole_document = selections.is_empty();
        // A document ending with a line break has an empty last line that isn't really a line.
        let last_line = st.rope.len_lines() - 1;
        let last_line = if last_line > 0 && st.line_content(last_line).len_chars() == 0 { last_line - 1 } else { last_line };
        let blocks = if whole_document { vec![(0, last_line)] } else { st.line_blocks(&selections) };
        let old = st.cursor_chars(&cursors);
        let mut edits = Vec::new();
        for &(first, last) in &blocks {
            let lines = st.lines_text(first, last);
            let new_lines = transform(lines.clone());
            if in_place && new_lines.len() == lines.len() {
                for (offset, (before, after)) in lines.iter().zip(&new_lines).enumerate() {
                    if before != after {
                        let line = first + offset;
                        edits.push(CharEdit { start: st.rope.line_to_char(line), end: st.content_end(line), text: after.clone() });
                    }
                }
            } else {
                edits.push(st.block_edit(first, last, &new_lines));
            }
        }
        let blocks_new: Vec<(usize, usize)> = edits.iter().map(|e| (e.start, e.text.chars().count())).collect();
        let original = cursors.clone();
        st.apply_edits(edits, cursors, EditKind::Other, move |st, map| {
            if in_place {
                st.mapped(&old, map)
            } else if whole_document {
                original.iter().map(|c| TextSelection { anchor: st.clamp_pos(c.anchor), head: st.clamp_pos(c.head) }).collect()
            } else {
                // Select each rewritten block.
                blocks_new
                    .iter()
                    .map(|&(start, length)| {
                        let start = map(start);
                        TextSelection { anchor: st.char_to_pos(start), head: st.char_to_pos(start + length) }
                    })
                    .collect()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caret(line: u64, column: u64) -> TextSelection {
        TextSelection::caret(TextPos::new(line, column))
    }

    fn sel(a: (u64, u64), b: (u64, u64)) -> TextSelection {
        TextSelection { anchor: TextPos::new(a.0, a.1), head: TextPos::new(b.0, b.1) }
    }

    #[test]
    fn maps_positions_through_edits() {
        // Insert 3 chars at 2, replace 5..7 with 1 char.
        let edits = [(2, 2, 3), (5, 7, 1)];
        assert_eq!(map_position(&edits, 1), 1);
        assert_eq!(map_position(&edits, 2), 5); // insertion at the caret pushes it along
        assert_eq!(map_position(&edits, 5), 8); // start of replaced text stays put (after earlier shift)
        assert_eq!(map_position(&edits, 6), 9); // inside replaced text, clamped to its new length
        assert_eq!(map_position(&edits, 7), 9);
        assert_eq!(map_position(&edits, 10), 12);
    }

    #[test]
    fn sorts_the_whole_document_or_just_the_selection() {
        let buffer = TextBuffer::with_text("c\nb\na\n".into());
        buffer.transform_lines(LineOperation::SortAscending, vec![caret(0, 0)], 4);
        assert_eq!(buffer.text(), "a\nb\nc\n");
        buffer.undo();
        assert_eq!(buffer.text(), "c\nb\na\n");

        let buffer = TextBuffer::with_text("z\nc\nb\na".into());
        let selected = buffer.transform_lines(LineOperation::SortAscending, vec![sel((1, 0), (3, 1))], 4);
        assert_eq!(buffer.text(), "z\na\nb\nc");
        assert_eq!(selected, vec![sel((1, 0), (3, 1))]);
    }

    #[test]
    fn removing_every_line_in_a_block_removes_its_line_breaks() {
        let buffer = TextBuffer::with_text("a\n\n  \nb".into());
        buffer.transform_lines(LineOperation::RemoveBlank, vec![sel((1, 0), (2, 2))], 4);
        assert_eq!(buffer.text(), "a\nb");
        // The whole document keeps its final line break.
        let buffer = TextBuffer::with_text("a\n\n".into());
        buffer.transform_lines(LineOperation::RemoveBlank, vec![caret(0, 0)], 4);
        assert_eq!(buffer.text(), "a\n");
    }

    #[test]
    fn trimming_keeps_other_cursors_in_place() {
        let buffer = TextBuffer::with_text("a  \nbb\t\nccc".into());
        let after = buffer.transform_lines(LineOperation::TrimTrailing, vec![caret(2, 2)], 4);
        assert_eq!(buffer.text(), "a\nbb\nccc");
        assert_eq!(after, vec![caret(2, 2)]);
    }

    #[test]
    fn changes_case_of_selections_and_words() {
        let buffer = TextBuffer::with_text("hello world foo_bar".into());
        let after = buffer.change_case(CaseChange::Upper, vec![sel((0, 0), (0, 5)), caret(0, 14)]);
        assert_eq!(buffer.text(), "HELLO world FOO_BAR");
        assert_eq!(after, vec![sel((0, 0), (0, 5)), caret(0, 14)]);
        // A selection grows with its text.
        let buffer = TextBuffer::with_text("straße".into());
        let after = buffer.change_case(CaseChange::Upper, vec![sel((0, 0), (0, 6))]);
        assert_eq!(buffer.text(), "STRASSE");
        assert_eq!(after, vec![sel((0, 0), (0, 7))]);
    }

    #[test]
    fn toggles_line_comments() {
        let buffer = TextBuffer::with_text("fn a() {\n    one();\n\n    two();\n}".into());
        buffer.set_language(Some("rust".into()));
        let after = buffer.toggle_comment(vec![sel((1, 4), (3, 4))]);
        assert_eq!(buffer.text(), "fn a() {\n    // one();\n\n    // two();\n}");
        assert_eq!(after, vec![sel((1, 7), (3, 7))]);
        buffer.toggle_comment(after);
        assert_eq!(buffer.text(), "fn a() {\n    one();\n\n    two();\n}");
    }

    #[test]
    fn toggles_block_comments() {
        let buffer = TextBuffer::with_text("<p>hi</p>".into());
        buffer.set_language(Some("html".into()));
        buffer.toggle_comment(vec![caret(0, 0)]);
        assert_eq!(buffer.text(), "<!-- <p>hi</p> -->");
        buffer.toggle_comment(vec![caret(0, 3)]);
        assert_eq!(buffer.text(), "<p>hi</p>");
    }

    #[test]
    fn plain_text_has_no_comments() {
        let buffer = TextBuffer::with_text("hello".into());
        assert_eq!(buffer.toggle_comment(vec![caret(0, 0)]), vec![caret(0, 0)]);
        assert_eq!(buffer.text(), "hello");
    }

    #[test]
    fn moves_lines_with_their_cursors() {
        let buffer = TextBuffer::with_text("1\n2\n3\n4".into());
        let after = buffer.move_lines(true, vec![caret(2, 1)]);
        assert_eq!(buffer.text(), "1\n3\n2\n4");
        assert_eq!(after, vec![caret(1, 1)]);
        let after = buffer.move_lines(false, vec![sel((2, 0), (2, 1))]);
        assert_eq!(buffer.text(), "1\n3\n4\n2");
        assert_eq!(after, vec![sel((3, 0), (3, 1))]);
        // At the edge nothing moves.
        assert_eq!(buffer.move_lines(true, vec![caret(0, 0)]), vec![caret(0, 0)]);
        assert_eq!(buffer.text(), "1\n3\n4\n2");
    }

    #[test]
    fn duplicates_and_deletes_lines() {
        let buffer = TextBuffer::with_text("a\nb\nc".into());
        let after = buffer.duplicate_lines(vec![caret(1, 1)]);
        assert_eq!(buffer.text(), "a\nb\nb\nc");
        assert_eq!(after, vec![caret(2, 1)]);
        let after = buffer.delete_lines(vec![caret(2, 1), caret(3, 0)]);
        assert_eq!(buffer.text(), "a\nb");
        assert_eq!(after, vec![caret(1, 1)]);
        let after = buffer.delete_lines(vec![caret(1, 0)]);
        assert_eq!(buffer.text(), "a");
        assert_eq!(after, vec![caret(0, 0)]);
    }

    #[test]
    fn matches_brackets_skipping_strings() {
        let buffer = TextBuffer::with_text("f(a, \")\", [b]);".into());
        buffer.set_language(Some("javascript".into()));
        let pair = buffer.matching_bracket(TextPos::new(0, 1)).unwrap();
        assert_eq!((pair.open, pair.close), (TextPos::new(0, 1), TextPos::new(0, 13)));
        // From the closing side.
        let pair = buffer.matching_bracket(TextPos::new(0, 14)).unwrap();
        assert_eq!(pair.open, TextPos::new(0, 1));
        assert_eq!(buffer.matching_bracket(TextPos::new(0, 3)), None);
    }

    #[test]
    fn finds_words_and_visible_matches() {
        let buffer = TextBuffer::with_text("let foo = 1;\nfoo += foobar;\nfoo".into());
        assert_eq!(buffer.word_at(TextPos::new(0, 5)), Some(sel((0, 4), (0, 7))));
        assert_eq!(buffer.word_at(TextPos::new(0, 7)), Some(sel((0, 4), (0, 7))));
        assert_eq!(buffer.word_at(TextPos::new(0, 8)), None);
        let query = SearchQuery { pattern: "foo".into(), case_sensitive: true, whole_word: true, regex: false };
        assert_eq!(buffer.find_in_lines(query, 1, 1, 100).unwrap(), vec![sel((1, 0), (1, 3))]);
    }

    #[test]
    fn reports_line_endings_and_wraps() {
        let buffer = TextBuffer::with_text("a\r\nb\nc".into());
        assert_eq!(buffer.line_endings(0, 10), vec![Some(LineEnding::CrLf), Some(LineEnding::Lf), None]);
        let buffer = TextBuffer::with_text("short\nthe quick brown fox jumps".into());
        assert_eq!(buffer.wrap_rows(10, 4, 0, 10), vec![1, 3]);
        assert_eq!(buffer.wrap_layout(10, 4, 1, 1), vec![WrappedLine { breaks: vec![10, 20], indent: 0 }]);
    }
}
