//! Editing the way a code editor does it, for every UI: caret motion by characters, words and
//! lines; typing with automatic bracket and quote pairs; Return that keeps the indentation; Tab
//! and Shift-Tab; smart Backspace; copy, cut and paste with several carets; and selecting the next
//! occurrence. Positions are (line, UTF-16 column) as everywhere in the core.

use unicode_segmentation::GraphemeCursor;

use crate::buffer::TextBuffer;
use crate::search::SearchQuery;
use crate::types::{EditKind, TextPos, TextSelection};

/// What one level of indentation is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indentation {
    Tabs,
    Spaces(u32),
}

impl Indentation {
    /// What one press of Tab inserts at column 0.
    pub fn unit(self) -> String {
        match self {
            Indentation::Tabs => "\t".into(),
            Indentation::Spaces(width) => " ".repeat(width as usize),
        }
    }

    /// The indentation most of a document's first thousand lines use, if it's clear.
    pub fn detect(buffer: &TextBuffer) -> Option<Indentation> {
        let mut tab_lines = 0;
        let mut widths: Vec<(usize, usize)> = Vec::new();
        for line in buffer.lines(0, 1_000) {
            if line.starts_with('\t') {
                tab_lines += 1;
            } else if line.starts_with(' ') {
                let width = line.chars().take_while(|c| *c == ' ').count();
                let rest = line[width..].chars().next();
                if width >= 2 && rest.is_some_and(|c| !c.is_whitespace()) {
                    match widths.iter_mut().find(|(w, _)| *w == width) {
                        Some((_, count)) => *count += 1,
                        None => widths.push((width, 1)),
                    }
                }
            }
        }
        let space_lines: usize = widths.iter().map(|(_, n)| n).sum();
        if tab_lines > space_lines {
            return Some(Indentation::Tabs);
        }
        if space_lines == 0 {
            return None;
        }
        // Most indented lines sit at multiples of the indent width.
        [4, 2, 8, 3].into_iter().find_map(|unit| {
            let matching: usize = widths.iter().filter(|(w, _)| w % unit == 0).map(|(_, n)| n).sum();
            (matching as f64 >= space_lines as f64 * 0.8).then_some(Indentation::Spaces(unit as u32))
        })
    }
}

/// Where a caret can move within the text (moving up and down depends on the layout, so the UI
/// does that).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// One character (grapheme cluster) back, crossing line breaks.
    Left,
    Right,
    /// To the start of the previous word, like ⌥← on a Mac.
    WordLeft,
    /// To the end of the next word.
    WordRight,
    LineStart,
    /// To the end of the indentation, then to the start of the line.
    SmartLineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
}

/// Sorts carets and selections into document order and merges any that overlap (or carets that
/// coincide). A merged selection keeps the direction of the first.
pub fn normalize_cursors(mut cursors: Vec<TextSelection>) -> Vec<TextSelection> {
    cursors.sort_by_key(|c| (c.start(), c.end()));
    let mut merged: Vec<TextSelection> = Vec::with_capacity(cursors.len());
    for cursor in cursors {
        if let Some(last) = merged.last_mut() {
            let same_caret = is_caret(&cursor) && is_caret(last) && cursor.head == last.head;
            if cursor.start() < last.end() || same_caret {
                let (start, end) = (last.start(), last.end().max(cursor.end()));
                *last = if last.head >= last.anchor {
                    TextSelection { anchor: start, head: end }
                } else {
                    TextSelection { anchor: end, head: start }
                };
                continue;
            }
        }
        merged.push(cursor);
    }
    merged
}

fn is_caret(selection: &TextSelection) -> bool {
    selection.anchor == selection.head
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharKind {
    Space,
    Word,
    Punctuation,
}

fn kind(c: char) -> CharKind {
    if c.is_whitespace() {
        CharKind::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharKind::Word
    } else {
        CharKind::Punctuation
    }
}

fn is_word_char(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// UTF-16 column → byte offset in a line (the end if past it).
fn byte_of(text: &str, col: u64) -> usize {
    let mut units = 0u64;
    for (byte, c) in text.char_indices() {
        if units >= col {
            return byte;
        }
        units += c.len_utf16() as u64;
    }
    text.len()
}

fn col_of(text: &str, byte: usize) -> u64 {
    text[..byte.min(text.len())].encode_utf16().count() as u64
}

fn closer_for(opener: char) -> Option<char> {
    match opener {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\'' => Some('\''),
        '`' => Some('`'),
        _ => None,
    }
}

fn is_closer(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '"' | '\'' | '`')
}

fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\'' | '`')
}

impl TextBuffer {
    fn line_end(&self, line: u64) -> TextPos {
        TextPos::new(line, self.line_length(line))
    }

    fn last_line(&self) -> u64 {
        self.line_count().saturating_sub(1)
    }

    /// The end of the document.
    pub fn end_position(&self) -> TextPos {
        self.line_end(self.last_line())
    }

    /// The character just after a position on its line.
    pub fn char_after(&self, pos: TextPos) -> Option<char> {
        let text = self.line(pos.line);
        text[byte_of(&text, pos.column)..].chars().next()
    }

    /// The character just before a position on its line.
    pub fn char_before(&self, pos: TextPos) -> Option<char> {
        if pos.column == 0 {
            return None;
        }
        let text = self.line(pos.line);
        text[..byte_of(&text, pos.column)].chars().next_back()
    }

    /// Where a caret lands after a motion.
    pub fn motion_target(&self, pos: TextPos, motion: Motion) -> TextPos {
        let last = self.last_line();
        let pos = self.clamp(pos);
        let len = self.line_length(pos.line);
        match motion {
            Motion::Left | Motion::WordLeft if pos.column == 0 => {
                if pos.line == 0 { pos } else { self.line_end(pos.line - 1) }
            }
            Motion::Right | Motion::WordRight if pos.column >= len => {
                if pos.line >= last { pos } else { TextPos::new(pos.line + 1, 0) }
            }
            Motion::Left => {
                let text = self.line(pos.line);
                let byte = byte_of(&text, pos.column);
                let previous = GraphemeCursor::new(byte, text.len(), true).prev_boundary(&text, 0).ok().flatten().unwrap_or(0);
                TextPos::new(pos.line, col_of(&text, previous))
            }
            Motion::Right => {
                let text = self.line(pos.line);
                let byte = byte_of(&text, pos.column);
                let next = GraphemeCursor::new(byte, text.len(), true)
                    .next_boundary(&text, 0)
                    .ok()
                    .flatten()
                    .unwrap_or(text.len());
                TextPos::new(pos.line, col_of(&text, next))
            }
            Motion::WordLeft => {
                // Back over spaces, then over one run of word characters or punctuation.
                let text = self.line(pos.line);
                let chars: Vec<(usize, char)> = text[..byte_of(&text, pos.column)].char_indices().collect();
                let mut i = chars.len();
                while i > 0 && kind(chars[i - 1].1) == CharKind::Space {
                    i -= 1;
                }
                if i > 0 {
                    let run = kind(chars[i - 1].1);
                    while i > 0 && kind(chars[i - 1].1) == run {
                        i -= 1;
                    }
                }
                let byte = chars.get(i).map_or(byte_of(&text, pos.column), |&(b, _)| b);
                TextPos::new(pos.line, col_of(&text, if i == 0 { 0 } else { byte }))
            }
            Motion::WordRight => {
                let text = self.line(pos.line);
                let start = byte_of(&text, pos.column);
                let mut chars = text[start..].char_indices().peekable();
                while chars.next_if(|&(_, c)| kind(c) == CharKind::Space).is_some() {}
                if let Some(&(_, first)) = chars.peek() {
                    let run = kind(first);
                    while chars.next_if(|&(_, c)| kind(c) == run).is_some() {}
                }
                let end = chars.peek().map_or(text.len(), |&(offset, _)| start + offset);
                TextPos::new(pos.line, col_of(&text, end))
            }
            Motion::LineStart => TextPos::new(pos.line, 0),
            Motion::SmartLineStart => {
                let text = self.line(pos.line);
                let indent = text.chars().take_while(|c| *c == ' ' || *c == '\t').count() as u64;
                TextPos::new(pos.line, if pos.column == indent { 0 } else { indent })
            }
            Motion::LineEnd => TextPos::new(pos.line, len),
            Motion::DocumentStart => TextPos::new(0, 0),
            Motion::DocumentEnd => self.end_position(),
        }
    }

    /// Moves every caret. Without `extend`, a selection collapses first: Left and Right to the
    /// side they point to, other motions from the caret.
    pub fn move_cursors(&self, cursors: &[TextSelection], motion: Motion, extend: bool) -> Vec<TextSelection> {
        let moved = cursors
            .iter()
            .map(|cursor| {
                if !extend && !is_caret(cursor) {
                    match motion {
                        Motion::Left => return TextSelection::caret(cursor.start()),
                        Motion::Right => return TextSelection::caret(cursor.end()),
                        _ => {}
                    }
                }
                let head = self.motion_target(cursor.head, motion);
                if extend { TextSelection { anchor: cursor.anchor, head } } else { TextSelection::caret(head) }
            })
            .collect();
        self.break_undo_coalescing();
        normalize_cursors(moved)
    }

    /// The word, run of spaces or run of punctuation around a position, for double-clicking.
    pub fn word_range_at(&self, pos: TextPos) -> TextSelection {
        let text = self.line(pos.line);
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        if chars.is_empty() {
            return TextSelection::caret(pos);
        }
        let byte = byte_of(&text, pos.column);
        let probe = chars.iter().position(|&(b, _)| b >= byte).unwrap_or(chars.len()).min(chars.len() - 1);
        let run = kind(chars[probe].1);
        let (mut start, mut end) = (probe, probe + 1);
        while start > 0 && kind(chars[start - 1].1) == run {
            start -= 1;
        }
        while end < chars.len() && kind(chars[end].1) == run {
            end += 1;
        }
        let end_byte = chars.get(end).map_or(text.len(), |&(b, _)| b);
        TextSelection {
            anchor: TextPos::new(pos.line, col_of(&text, chars[start].0)),
            head: TextPos::new(pos.line, col_of(&text, end_byte)),
        }
    }

    /// Whole lines from each selection's first line to just past its last.
    pub fn select_lines(&self, cursors: &[TextSelection]) -> Vec<TextSelection> {
        let last = self.last_line();
        normalize_cursors(
            cursors
                .iter()
                .map(|c| {
                    let end = if c.end().line < last { TextPos::new(c.end().line + 1, 0) } else { self.end_position() };
                    TextSelection { anchor: TextPos::new(c.start().line, 0), head: end }
                })
                .collect(),
        )
    }

    /// Every line touched by a caret or selection, in order. A selection ending at column 0
    /// doesn't count its last line.
    pub fn selected_lines(&self, cursors: &[TextSelection]) -> Vec<u64> {
        let mut lines: Vec<u64> = Vec::new();
        for c in cursors {
            let (start, end) = (c.start(), c.end());
            let last = if end.column == 0 && end.line > start.line { end.line - 1 } else { end.line };
            lines.extend(start.line..=last);
        }
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    // MARK: Typing

    /// Replaces each range with text and returns the carets after, in document order.
    fn replace_ranges(&self, ranges: Vec<TextSelection>, texts: Vec<String>, kind: EditKind, cursors: &[TextSelection]) -> Vec<TextSelection> {
        self.edit(ranges, texts, kind, cursors.to_vec())
    }

    /// Text typed at every caret. With `auto_close`, a bracket or quote also types its closer
    /// (unless a word follows), typing a closer that's already next steps over it, and an opener
    /// typed over selections wraps them.
    pub fn type_text(&self, cursors: &[TextSelection], text: &str, kind: EditKind, auto_close: bool) -> Vec<TextSelection> {
        let mut chars = text.chars();
        let single = match (chars.next(), chars.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        };
        let Some(typed) = single.filter(|_| auto_close && kind == EditKind::Typing) else {
            return self.replace_ranges(cursors.to_vec(), vec![text.to_string()], kind, cursors);
        };
        // Typing a closer that's already next steps over it.
        if is_closer(typed) && cursors.iter().all(|c| is_caret(c) && self.char_after(c.head) == Some(typed)) {
            let width = typed.len_utf16() as u64;
            self.break_undo_coalescing();
            return cursors.iter().map(|c| TextSelection::caret(TextPos::new(c.head.line, c.head.column + width))).collect();
        }
        let Some(closer) = closer_for(typed) else {
            return self.replace_ranges(cursors.to_vec(), vec![text.to_string()], kind, cursors);
        };
        if cursors.iter().any(|c| !is_caret(c)) {
            return self.wrap_selections(cursors, typed, closer);
        }
        if cursors.iter().all(|c| self.should_auto_close(typed, c.head)) {
            let after = self.replace_ranges(cursors.to_vec(), vec![format!("{typed}{closer}")], kind, cursors);
            let back = closer.len_utf16() as u64;
            return after.iter().map(|c| TextSelection::caret(TextPos::new(c.head.line, c.head.column - back))).collect();
        }
        self.replace_ranges(cursors.to_vec(), vec![text.to_string()], kind, cursors)
    }

    /// Whether typing an opener here should add its closer: not right before a word, and for
    /// quotes not right after one (so "don't" stays as typed).
    fn should_auto_close(&self, opener: char, at: TextPos) -> bool {
        if let Some(next) = self.char_after(at)
            && !next.is_whitespace()
            && !is_closer(next)
            && !matches!(next, ',' | ';' | ':')
        {
            return false;
        }
        if is_quote(opener) {
            let previous = self.char_before(at);
            if is_word_char(previous) || previous == Some(opener) {
                return false;
            }
        }
        true
    }

    /// Surrounds each selection with a bracket or quote pair, keeping the text selected.
    fn wrap_selections(&self, cursors: &[TextSelection], open: char, close: char) -> Vec<TextSelection> {
        let ranges: Vec<TextSelection> = cursors.iter().flat_map(|c| [TextSelection::caret(c.start()), TextSelection::caret(c.end())]).collect();
        let texts: Vec<String> = cursors.iter().flat_map(|_| [open.to_string(), close.to_string()]).collect();
        let count = ranges.len();
        let after = self.replace_ranges(ranges, texts, EditKind::Other, cursors);
        if after.len() != count {
            return after;
        }
        let back = close.len_utf16() as u64;
        after
            .chunks(2)
            .map(|pair| TextSelection { anchor: pair[0].head, head: TextPos::new(pair[1].head.line, pair[1].head.column - back) })
            .collect()
    }

    /// Return: carries the line's indentation onto the new line, one level deeper after an
    /// opening bracket (or a colon in Python). Between a pair like {}, the closer moves down to a
    /// line of its own and the caret sits on the indented line between.
    pub fn insert_newline(&self, cursors: &[TextSelection], indentation: Indentation) -> Vec<TextSelection> {
        let eol = self.file_info().line_ending.as_str();
        let python = self.language().as_deref() == Some("python");
        let mut texts = Vec::with_capacity(cursors.len());
        let mut expanded = Vec::with_capacity(cursors.len());
        for cursor in cursors {
            let start = cursor.start();
            let text = self.line(start.line);
            let indent_len = text.chars().take_while(|c| *c == ' ' || *c == '\t').count().min(start.column as usize);
            let indent = &text[..indent_len];
            let before = self.char_before(start);
            let opens = matches!(before, Some('(' | '[' | '{')) || (python && before == Some(':'));
            if !opens {
                texts.push(format!("{eol}{indent}"));
                expanded.push(false);
                continue;
            }
            let inner = format!("{indent}{}", indentation.unit());
            if before.and_then(closer_for).is_some_and(|closer| self.char_after(cursor.end()) == Some(closer)) {
                texts.push(format!("{eol}{inner}{eol}{indent}"));
                expanded.push(true);
            } else {
                texts.push(format!("{eol}{inner}"));
                expanded.push(false);
            }
        }
        let after = self.replace_ranges(cursors.to_vec(), texts, EditKind::Typing, cursors);
        // Carets come back in document order, after the inserted text; for an expanded pair, put
        // them on the middle line.
        let flags = sorted_with_flags(cursors, &expanded);
        after
            .iter()
            .enumerate()
            .map(|(i, caret)| match flags.get(i) {
                Some((_, true)) if caret.head.line > 0 => TextSelection::caret(self.line_end(caret.head.line - 1)),
                _ => *caret,
            })
            .collect()
    }

    /// Tab: indents the selected lines when a selection spans lines, otherwise inserts a tab or
    /// spaces up to the next tab stop at each caret.
    pub fn insert_tab(&self, cursors: &[TextSelection], indentation: Indentation) -> Vec<TextSelection> {
        if cursors.iter().any(|c| c.start().line != c.end().line) {
            return self.indent_lines(cursors, indentation);
        }
        let texts = cursors
            .iter()
            .map(|c| match indentation {
                Indentation::Tabs => "\t".to_string(),
                Indentation::Spaces(width) => " ".repeat((width as u64 - c.start().column % width as u64) as usize),
            })
            .collect();
        self.replace_ranges(cursors.to_vec(), texts, EditKind::Typing, cursors)
    }

    /// Adds one level of indentation to every selected line, keeping the selections on the same text.
    pub fn indent_lines(&self, cursors: &[TextSelection], indentation: Indentation) -> Vec<TextSelection> {
        let lines = self.selected_lines(cursors);
        let unit = indentation.unit();
        let width = unit.encode_utf16().count() as u64;
        let shift = |p: TextPos| if lines.binary_search(&p.line).is_ok() && p.column > 0 { TextPos::new(p.line, p.column + width) } else { p };
        let shifted = cursors.iter().map(|c| TextSelection { anchor: shift(c.anchor), head: shift(c.head) }).collect();
        let ranges = lines.iter().map(|&line| TextSelection::caret(TextPos::new(line, 0))).collect();
        self.replace_ranges(ranges, vec![unit], EditKind::Other, cursors);
        shifted
    }

    /// Shift-Tab: removes one level of indentation from every selected line.
    pub fn outdent_lines(&self, cursors: &[TextSelection], indentation: Indentation) -> Vec<TextSelection> {
        let width = match indentation {
            Indentation::Spaces(width) => width as usize,
            Indentation::Tabs => 4,
        };
        let mut removals: Vec<(u64, u64)> = Vec::new();
        for line in self.selected_lines(cursors) {
            let text = self.line(line);
            let count = if text.starts_with('\t') { 1 } else { text.chars().take(width).take_while(|c| *c == ' ').count() };
            if count > 0 {
                removals.push((line, count as u64));
            }
        }
        if removals.is_empty() {
            return cursors.to_vec();
        }
        let removed = |line: u64| removals.iter().find(|(l, _)| *l == line).map_or(0, |(_, n)| *n);
        let shift = |p: TextPos| TextPos::new(p.line, p.column.saturating_sub(removed(p.line)));
        let shifted = cursors.iter().map(|c| TextSelection { anchor: shift(c.anchor), head: shift(c.head) }).collect();
        let ranges = removals.iter().map(|&(line, n)| TextSelection { anchor: TextPos::new(line, 0), head: TextPos::new(line, n) }).collect();
        self.replace_ranges(ranges, vec![String::new()], EditKind::Other, cursors);
        shifted
    }

    // MARK: Deleting

    fn delete_ranges(&self, cursors: &[TextSelection], ranges: Vec<TextSelection>) -> Vec<TextSelection> {
        if ranges.iter().all(is_caret) {
            return cursors.to_vec();
        }
        self.replace_ranges(ranges, vec![String::new()], EditKind::Deletion, cursors)
    }

    /// Backspace: deletes selections, or the character before each caret. Inside an empty pair
    /// like () it removes both halves, and in leading spaces it goes back to the previous indent stop.
    pub fn delete_backward(&self, cursors: &[TextSelection], indentation: Indentation, auto_close: bool) -> Vec<TextSelection> {
        if auto_close && cursors.iter().all(is_caret) {
            let pairs: Option<Vec<TextSelection>> = cursors
                .iter()
                .map(|c| {
                    let before = self.char_before(c.head)?;
                    let closer = closer_for(before)?;
                    (self.char_after(c.head) == Some(closer)).then(|| TextSelection {
                        anchor: TextPos::new(c.head.line, c.head.column - before.len_utf16() as u64),
                        head: TextPos::new(c.head.line, c.head.column + closer.len_utf16() as u64),
                    })
                })
                .collect();
            if let Some(pairs) = pairs {
                return self.delete_ranges(cursors, pairs);
            }
        }
        let ranges = cursors
            .iter()
            .map(|c| {
                if !is_caret(c) {
                    return *c;
                }
                let head = c.head;
                if let Indentation::Spaces(width) = indentation
                    && head.column > 0
                {
                    let text = self.line(head.line);
                    let leading = text.chars().take_while(|ch| *ch == ' ').count() as u64;
                    if leading >= head.column {
                        let count = (head.column - 1) % width as u64 + 1;
                        return TextSelection { anchor: TextPos::new(head.line, head.column - count), head };
                    }
                }
                TextSelection { anchor: self.motion_target(head, Motion::Left), head }
            })
            .collect();
        self.delete_ranges(cursors, ranges)
    }

    /// Deletes selections, or from each caret to where a motion takes it (⌥⌫, ⌦, ⌘⌫ and so on).
    /// Deleting to a line's start or end at the start or end joins it with its neighbor.
    pub fn delete_to(&self, cursors: &[TextSelection], motion: Motion) -> Vec<TextSelection> {
        let ranges = cursors
            .iter()
            .map(|c| {
                if !is_caret(c) {
                    return *c;
                }
                let mut to = self.motion_target(c.head, motion);
                if to == c.head {
                    to = match motion {
                        Motion::LineStart | Motion::SmartLineStart => self.motion_target(c.head, Motion::Left),
                        Motion::LineEnd => self.motion_target(c.head, Motion::Right),
                        _ => to,
                    };
                }
                TextSelection { anchor: c.head, head: to }
            })
            .collect();
        self.delete_ranges(cursors, ranges)
    }

    // MARK: Clipboard

    /// What Copy puts on the clipboard: the selections joined by line breaks, or with nothing
    /// selected the whole lines the carets are on (as most code editors do).
    pub fn copy_text(&self, cursors: &[TextSelection]) -> String {
        let eol = self.file_info().line_ending.as_str();
        if cursors.iter().all(is_caret) {
            return self.selected_lines(cursors).iter().map(|&line| format!("{}{eol}", self.line(line))).collect();
        }
        cursors.iter().filter(|c| !is_caret(c)).map(|c| self.text_in(c.start(), c.end())).collect::<Vec<_>>().join(eol)
    }

    /// Cut: returns what goes on the clipboard and the carets after. With nothing selected it
    /// removes the carets' whole lines.
    pub fn cut(&self, cursors: &[TextSelection]) -> (String, Vec<TextSelection>) {
        let text = self.copy_text(cursors);
        let ranges = if cursors.iter().all(is_caret) {
            let last = self.last_line();
            self.selected_lines(cursors)
                .into_iter()
                .map(|line| {
                    if line < last {
                        TextSelection { anchor: TextPos::new(line, 0), head: TextPos::new(line + 1, 0) }
                    } else {
                        let start = if line > 0 { self.line_end(line - 1) } else { TextPos::new(line, 0) };
                        TextSelection { anchor: start, head: self.line_end(line) }
                    }
                })
                .collect()
        } else {
            cursors.iter().filter(|c| !is_caret(c)).copied().collect()
        };
        let after = self.replace_ranges(ranges, vec![String::new()], EditKind::Other, cursors);
        (text, after)
    }

    /// Paste, with line breaks converted to the document's. With several carets and as many
    /// pasted lines, each caret gets one line.
    pub fn paste(&self, cursors: &[TextSelection], text: &str) -> Vec<TextSelection> {
        let eol = self.file_info().line_ending.as_str();
        let unix = text.replace("\r\n", "\n").replace('\r', "\n");
        if cursors.len() > 1 {
            let parts: Vec<&str> = unix.strip_suffix('\n').unwrap_or(&unix).split('\n').collect();
            if parts.len() == cursors.len() {
                let texts = parts.iter().map(|s| s.to_string()).collect();
                return self.replace_ranges(cursors.to_vec(), texts, EditKind::Other, cursors);
            }
        }
        let text = if eol == "\n" { unix } else { unix.replace('\n', eol) };
        self.replace_ranges(cursors.to_vec(), vec![text], EditKind::Other, cursors)
    }

    /// Moves (or with `copy`, copies) a selection's text to another place, as one undo step.
    /// Returns the text at its new place, selected; `None` when dropped onto itself.
    pub fn move_text(&self, from: TextSelection, to: TextPos, copy: bool) -> Option<TextSelection> {
        let (start, end) = (from.start(), from.end());
        let to = self.clamp(to);
        if start == end || (to >= start && to <= end) {
            return None;
        }
        let text = self.text_in(start, end);
        let (ranges, texts) = if copy {
            (vec![TextSelection::caret(to)], vec![text.clone()])
        } else {
            (vec![TextSelection { anchor: start, head: end }, TextSelection::caret(to)], vec![String::new(), text.clone()])
        };
        let after = self.edit(ranges, texts, EditKind::Other, vec![from]);
        // Carets come back in document order; the inserted text ends at the one for `to`.
        let inserted = if copy || to < start { *after.first()? } else { *after.last()? };
        let length = text.encode_utf16().count() as u64;
        let end_offset = self.utf16_offset(inserted.head);
        Some(TextSelection { anchor: self.position_at_utf16_offset(end_offset - length), head: inserted.head })
    }

    // MARK: Selecting

    /// ⌘D: selects the word at the last caret, then each next occurrence of the last selection
    /// (wrapping around), adding it to the selections.
    pub fn select_next_occurrence(&self, cursors: &[TextSelection]) -> Vec<TextSelection> {
        let Some(&last) = cursors.last() else { return cursors.to_vec() };
        if is_caret(&last) {
            let Some(word) = self.word_at(last.head) else { return cursors.to_vec() };
            let mut out = cursors[..cursors.len() - 1].to_vec();
            out.push(word);
            return keep_last(normalize_cursors(out), word);
        }
        let query = SearchQuery { pattern: self.text_in(last.start(), last.end()), case_sensitive: true, whole_word: false, regex: false };
        let after = last.end();
        let lines = self.line_count();
        let taken = |m: &TextSelection| cursors.iter().any(|c| c.start() == m.start());
        let found = self
            .find_in_lines(query.clone(), after.line, lines - after.line, 1_000)
            .unwrap_or_default()
            .into_iter()
            .find(|m| m.start() >= after && !taken(m))
            .or_else(|| self.find_in_lines(query, 0, after.line + 1, 1_000).unwrap_or_default().into_iter().find(|m| !taken(m)));
        match found {
            Some(found) => {
                let mut out = cursors.to_vec();
                out.push(found);
                keep_last(normalize_cursors(out), found)
            }
            None => cursors.to_vec(),
        }
    }
}

/// Moves one selection to the end, so it's the one the view follows.
fn keep_last(mut cursors: Vec<TextSelection>, newest: TextSelection) -> Vec<TextSelection> {
    if let Some(index) = cursors.iter().position(|c| *c == newest) {
        let item = cursors.remove(index);
        cursors.push(item);
    }
    cursors
}

/// The cursors in document order, each with its flag.
fn sorted_with_flags(cursors: &[TextSelection], flags: &[bool]) -> Vec<(TextSelection, bool)> {
    let mut pairs: Vec<(TextSelection, bool)> = cursors.iter().copied().zip(flags.iter().copied()).collect();
    pairs.sort_by_key(|(c, _)| (c.start(), c.end()));
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caret(line: u64, column: u64) -> TextSelection {
        TextSelection::caret(TextPos::new(line, column))
    }

    fn sel(a: (u64, u64), h: (u64, u64)) -> TextSelection {
        TextSelection { anchor: TextPos::new(a.0, a.1), head: TextPos::new(h.0, h.1) }
    }

    fn buffer(text: &str) -> std::sync::Arc<TextBuffer> {
        TextBuffer::with_text(text.to_string())
    }

    #[test]
    fn motion_by_characters_words_and_lines() {
        let b = buffer("  let 名前 = \"👍🏽\";\nnext");
        let emoji = TextPos::new(0, 12);
        assert_eq!(b.motion_target(emoji, Motion::Right), TextPos::new(0, 16));
        assert_eq!(b.motion_target(TextPos::new(0, 16), Motion::Left), emoji);
        assert_eq!(b.motion_target(b.line_end(0), Motion::Right), TextPos::new(1, 0));
        assert_eq!(b.motion_target(TextPos::new(1, 0), Motion::Left), b.line_end(0));
        assert_eq!(b.motion_target(TextPos::new(0, 0), Motion::WordRight), TextPos::new(0, 5));
        assert_eq!(b.motion_target(TextPos::new(0, 5), Motion::WordRight), TextPos::new(0, 8));
        assert_eq!(b.motion_target(TextPos::new(0, 8), Motion::WordRight), TextPos::new(0, 10));
        assert_eq!(b.motion_target(TextPos::new(0, 10), Motion::WordLeft), TextPos::new(0, 9));
        assert_eq!(b.motion_target(TextPos::new(0, 8), Motion::WordLeft), TextPos::new(0, 6));
        assert_eq!(b.motion_target(TextPos::new(0, 9), Motion::SmartLineStart), TextPos::new(0, 2));
        assert_eq!(b.motion_target(TextPos::new(0, 2), Motion::SmartLineStart), TextPos::new(0, 0));
        assert_eq!(b.word_range_at(TextPos::new(0, 7)), sel((0, 6), (0, 8)));
        // Collapsing a selection without Shift.
        let moved = b.move_cursors(&[sel((0, 2), (0, 5))], Motion::Left, false);
        assert_eq!(moved, vec![caret(0, 2)]);
        let extended = b.move_cursors(&[caret(0, 2)], Motion::WordRight, true);
        assert_eq!(extended, vec![sel((0, 2), (0, 5))]);
    }

    #[test]
    fn typing_pairs_and_quotes() {
        let b = buffer("x\ndon");
        let after = b.type_text(&[caret(0, 1)], "(", EditKind::Typing, true);
        assert_eq!(b.line(0), "x()");
        assert_eq!(after, vec![caret(0, 2)]);
        let after = b.type_text(&after, ")", EditKind::Typing, true);
        assert_eq!(b.line(0), "x()");
        assert_eq!(after, vec![caret(0, 3)]);
        // No closing quote after a word character.
        b.type_text(&[caret(1, 3)], "'", EditKind::Typing, true);
        assert_eq!(b.line(1), "don'");
        // An opener over a selection wraps it.
        let after = b.type_text(&[sel((1, 0), (1, 3))], "[", EditKind::Typing, true);
        assert_eq!(b.line(1), "[don]'");
        assert_eq!(after, vec![sel((1, 1), (1, 4))]);
        // Backspace in an empty pair removes both halves.
        let b = buffer("f()");
        let after = b.delete_backward(&[caret(0, 2)], Indentation::Spaces(4), true);
        assert_eq!(b.line(0), "f");
        assert_eq!(after, vec![caret(0, 1)]);
    }

    #[test]
    fn newline_tab_and_backspace_follow_indentation() {
        let b = buffer("fn main() {}");
        let after = b.insert_newline(&[caret(0, 11)], Indentation::Spaces(4));
        assert_eq!(b.text(), "fn main() {\n    \n}");
        assert_eq!(after, vec![caret(1, 4)]);
        let after = b.delete_backward(&after, Indentation::Spaces(4), true);
        assert_eq!(b.line(1), "");
        assert_eq!(after, vec![caret(1, 0)]);
        let after = b.insert_tab(&after, Indentation::Spaces(4));
        assert_eq!(b.line(1), "    ");
        let after = b.insert_tab(&[TextSelection { anchor: TextPos::new(0, 3), head: after[0].head }], Indentation::Spaces(4));
        assert_eq!(b.text(), "    fn main() {\n        \n}");
        assert_eq!(after, vec![sel((0, 7), (1, 8))]);
        let after = b.outdent_lines(&after, Indentation::Spaces(4));
        assert_eq!(b.text(), "fn main() {\n    \n}");
        assert_eq!(after, vec![sel((0, 3), (1, 4))]);
        // Python: deeper after a colon.
        let p = buffer("if x:");
        p.set_language(Some("python".into()));
        p.insert_newline(&[caret(0, 5)], Indentation::Spaces(4));
        assert_eq!(p.text(), "if x:\n    ");
        assert_eq!(Indentation::detect(&buffer("a\n  b\n  c\n    d")), Some(Indentation::Spaces(2)));
        assert_eq!(Indentation::detect(&buffer("a\n\tb\n\tc")), Some(Indentation::Tabs));
    }

    #[test]
    fn clipboard_with_several_carets() {
        let b = buffer("one\ntwo\nthree");
        assert_eq!(b.copy_text(&[caret(1, 1)]), "two\n");
        let (text, after) = b.cut(&[caret(1, 1)]);
        assert_eq!(text, "two\n");
        assert_eq!(b.text(), "one\nthree");
        assert_eq!(after, vec![caret(1, 0)]);
        let after = b.paste(&[caret(0, 3), caret(1, 5)], "A\nB\n");
        assert_eq!(b.text(), "oneA\nthreeB");
        assert_eq!(after, vec![caret(0, 4), caret(1, 6)]);
        assert_eq!(b.copy_text(&[sel((0, 0), (0, 3)), sel((1, 0), (1, 5))]), "one\nthree");
    }

    #[test]
    fn dragging_text_moves_or_copies_it() {
        let b = buffer("one two three\nfour");
        let moved = b.move_text(sel((0, 4), (0, 8)), TextPos::new(1, 4), false);
        assert_eq!(b.text(), "one three\nfourtwo ");
        assert_eq!(moved, Some(sel((1, 4), (1, 8))));
        b.undo();
        assert_eq!(b.text(), "one two three\nfour");
        let copied = b.move_text(sel((0, 8), (0, 13)), TextPos::new(0, 0), true);
        assert_eq!(b.text(), "threeone two three\nfour");
        assert_eq!(copied, Some(sel((0, 0), (0, 5))));
        assert_eq!(b.move_text(sel((0, 0), (0, 5)), TextPos::new(0, 2), false), None);
    }

    #[test]
    fn next_occurrence_and_normalizing() {
        let b = buffer("query and query\nquery");
        let first = b.select_next_occurrence(&[caret(0, 2)]);
        assert_eq!(first, vec![sel((0, 0), (0, 5))]);
        let second = b.select_next_occurrence(&first);
        assert_eq!(second, vec![sel((0, 0), (0, 5)), sel((0, 10), (0, 15))]);
        let third = b.select_next_occurrence(&second);
        assert_eq!(third.len(), 3);
        assert_eq!(third[2], sel((1, 0), (1, 5)));
        assert_eq!(normalize_cursors(vec![caret(2, 0), caret(1, 0), caret(2, 0)]), vec![caret(1, 0), caret(2, 0)]);
    }
}
