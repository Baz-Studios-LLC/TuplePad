//! Keys, typing, input methods and the mouse, applied to the current document through the core's
//! editing functions. Commands with menu shortcuts are matched before keys get here (see
//! `commands`); this handles the keys that move carets and edit text.

use std::time::{Duration, Instant};

use tuplepad_core::{EditKind, Motion, TextBuffer, TextPos, TextSelection, normalize_cursors, now_seconds};

use super::{Drag, Editor};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Slint's key codes (private-use characters, like AppKit's).
pub mod keys {
    pub const UP: char = '\u{F700}';
    pub const DOWN: char = '\u{F701}';
    pub const LEFT: char = '\u{F702}';
    pub const RIGHT: char = '\u{F703}';
    pub const HOME: char = '\u{F729}';
    pub const END: char = '\u{F72B}';
    pub const PAGE_UP: char = '\u{F72C}';
    pub const PAGE_DOWN: char = '\u{F72D}';
    pub const BACKSPACE: char = '\u{8}';
    pub const TAB: char = '\t';
    pub const BACKTAB: char = '\u{19}';
    pub const RETURN: char = '\n';
    pub const ENTER: char = '\r';
    pub const ESCAPE: char = '\u{1b}';
    pub const DELETE: char = '\u{7f}';
}

/// The modifiers of a key press, named for what they are on each system.
#[derive(Clone, Copy, Debug, Default)]
pub struct Modifiers {
    /// ⌘ on a Mac, Ctrl elsewhere (Slint's `control`).
    pub command: bool,
    /// The Control key on a Mac (Slint's `meta`); the Windows/Super key elsewhere.
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

const MAC: bool = cfg!(target_os = "macos");

impl Editor {
    /// Runs an edit on the current document: `f` gets the buffer and the cursors and returns the
    /// cursors after.
    pub fn apply(&mut self, f: impl FnOnce(&TextBuffer, &[TextSelection]) -> Vec<TextSelection>) {
        let buffer = self.buffer();
        let before = self.doc().cursors.clone();
        let (old_count, version) = (buffer.line_count(), buffer.version());
        let after = f(&buffer, &before);
        if buffer.version() != version {
            self.note_edit(old_count, &before, &after);
            self.doc_mut().last_edited = now_seconds();
        }
        if !after.is_empty() {
            self.doc_mut().cursors = after;
        }
        self.goals = None;
        self.touch();
        self.reveal();
    }

    pub fn undo(&mut self) {
        self.preedit.clear();
        if let Some(cursors) = self.buffer().undo() {
            self.note_any_edit();
            self.doc_mut().cursors = cursors;
            self.doc_mut().last_edited = now_seconds();
            self.reveal();
        }
    }

    pub fn redo(&mut self) {
        self.preedit.clear();
        if let Some(cursors) = self.buffer().redo() {
            self.note_any_edit();
            self.doc_mut().cursors = cursors;
            self.doc_mut().last_edited = now_seconds();
            self.reveal();
        }
    }

    /// Text from the keyboard or an input method.
    pub fn insert_text(&mut self, text: &str) {
        let kind = if self.composed { EditKind::Composition } else { EditKind::Typing };
        self.preedit.clear();
        self.composed = false;
        let auto_close = self.options.auto_close;
        self.apply(|buffer, cursors| buffer.type_text(cursors, text, kind, auto_close));
    }

    /// Composing text from an input method, shown inline at the main caret until it's committed.
    pub fn set_preedit(&mut self, text: &str) {
        if !text.is_empty() {
            self.composed = true;
        }
        self.preedit = text.to_string();
        self.touch();
        self.reveal();
    }

    /// Handles a key that moves carets or edits text. Returns false for keys that should type
    /// text, so the text input (and with it any input method) inserts them.
    pub fn key(&mut self, text: &str, m: Modifiers) -> bool {
        let mut chars = text.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return m.command;
        };
        self.touch();
        let shift = m.shift;
        // Word-wise moves: ⌥ on a Mac, Ctrl elsewhere.
        let word = if MAC { m.alt && !m.command } else { m.command };
        let indentation = self.indentation();
        let auto_close = self.options.auto_close;
        match ch {
            keys::LEFT if MAC && m.command => self.move_cursors(Motion::SmartLineStart, shift),
            keys::LEFT if word => self.move_cursors(Motion::WordLeft, shift),
            keys::LEFT => self.move_cursors(Motion::Left, shift),
            keys::RIGHT if MAC && m.command => self.move_cursors(Motion::LineEnd, shift),
            keys::RIGHT if word => self.move_cursors(Motion::WordRight, shift),
            keys::RIGHT => self.move_cursors(Motion::Right, shift),
            keys::UP if MAC && m.command => self.move_cursors(Motion::DocumentStart, shift),
            keys::DOWN if MAC && m.command => self.move_cursors(Motion::DocumentEnd, shift),
            keys::UP => self.move_vertically(-1, shift),
            keys::DOWN => self.move_vertically(1, shift),
            // On a Mac, Home, End and the page keys scroll without moving the caret (with ⌥ they move it).
            keys::HOME if MAC && !m.alt => self.scroll_to_end(false),
            keys::END if MAC && !m.alt => self.scroll_to_end(true),
            keys::PAGE_UP if MAC && !m.alt => self.scroll_page(-1),
            keys::PAGE_DOWN if MAC && !m.alt => self.scroll_page(1),
            keys::HOME if m.command => self.move_cursors(Motion::DocumentStart, shift),
            keys::END if m.command => self.move_cursors(Motion::DocumentEnd, shift),
            keys::HOME => self.move_cursors(Motion::SmartLineStart, shift),
            keys::END => self.move_cursors(Motion::LineEnd, shift),
            keys::PAGE_UP => self.move_page(-1, shift),
            keys::PAGE_DOWN => self.move_page(1, shift),
            keys::BACKSPACE if MAC && m.command => self.apply(|b, c| b.delete_to(c, Motion::LineStart)),
            keys::BACKSPACE if word => self.apply(|b, c| b.delete_to(c, Motion::WordLeft)),
            keys::BACKSPACE => self.apply(|b, c| b.delete_backward(c, indentation, auto_close)),
            keys::DELETE if MAC && m.command => self.apply(|b, c| b.delete_to(c, Motion::LineEnd)),
            keys::DELETE if word => self.apply(|b, c| b.delete_to(c, Motion::WordRight)),
            keys::DELETE => self.apply(|b, c| b.delete_to(c, Motion::Right)),
            keys::RETURN | keys::ENTER if !m.command => self.apply(|b, c| b.insert_newline(c, indentation)),
            keys::BACKTAB => self.apply(|b, c| b.outdent_lines(c, indentation)),
            keys::TAB if shift => self.apply(|b, c| b.outdent_lines(c, indentation)),
            keys::TAB if !m.command => self.apply(|b, c| b.insert_tab(c, indentation)),
            keys::ESCAPE => self.single_cursor(),
            // The Control-key bindings every Mac text view has.
            'a' if MAC && m.control => self.move_cursors(Motion::LineStart, shift),
            'e' if MAC && m.control => self.move_cursors(Motion::LineEnd, shift),
            'f' if MAC && m.control => self.move_cursors(Motion::Right, shift),
            'b' if MAC && m.control => self.move_cursors(Motion::Left, shift),
            'n' if MAC && m.control => self.move_vertically(1, shift),
            'p' if MAC && m.control => self.move_vertically(-1, shift),
            'd' if MAC && m.control => self.apply(|b, c| b.delete_to(c, Motion::Right)),
            'h' if MAC && m.control => self.apply(|b, c| b.delete_backward(c, indentation, auto_close)),
            'k' if MAC && m.control => self.apply(|b, c| b.delete_to(c, Motion::LineEnd)),
            // Other shortcuts are the menus' business; never let them type.
            _ if m.command || m.control => return true,
            _ => return false,
        }
        true
    }

    // MARK: Moving carets

    pub fn move_cursors(&mut self, motion: Motion, extend: bool) {
        self.goals = None;
        let cursors = self.doc().cursors.clone();
        self.doc_mut().cursors = self.buffer().move_cursors(&cursors, motion, extend);
        self.reveal();
    }

    /// Moves every caret `delta` visual rows, aiming for the x it started from.
    pub fn move_vertically(&mut self, delta: i64, extend: bool) {
        let cursors = self.doc().cursors.clone();
        if self.goals.as_ref().is_none_or(|g| g.len() != cursors.len()) {
            let goals = cursors.iter().map(|c| self.locate(c.head).1).collect();
            self.goals = Some(goals);
        }
        let goals = self.goals.clone().unwrap_or_default();
        let total = self.rows.total_rows() as i64;
        let end = self.buffer().end_position();
        let mut moved = Vec::with_capacity(cursors.len());
        for (i, cursor) in cursors.iter().enumerate() {
            let collapsed = cursor.anchor == cursor.head;
            let origin = if !extend && !collapsed { if delta < 0 { cursor.start() } else { cursor.end() } } else { cursor.head };
            let (row, _) = self.locate(origin);
            let target = row as i64 + delta;
            let head = if target < 0 {
                TextPos::new(0, 0)
            } else if target >= total {
                end
            } else {
                self.position_on_row(target as u64, goals.get(i).copied().unwrap_or(0.0))
            };
            moved.push(if extend { TextSelection { anchor: cursor.anchor, head } } else { TextSelection::caret(head) });
        }
        let merged = normalize_cursors(moved);
        if merged.len() != cursors.len() {
            self.goals = None;
        }
        self.buffer().break_undo_coalescing();
        self.doc_mut().cursors = merged;
        self.reveal();
    }

    fn page_rows(&self) -> i64 {
        self.visible_rows().saturating_sub(1).max(1) as i64
    }

    fn move_page(&mut self, direction: i64, extend: bool) {
        let rows = self.page_rows() * direction;
        self.doc_mut().scroll_y += rows as f64 * self.line_height as f64;
        self.move_vertically(rows, extend);
    }

    fn scroll_page(&mut self, direction: i64) {
        let rows = self.page_rows() * direction;
        self.scroll_by(0.0, -(rows as f32 * self.line_height));
    }

    fn scroll_to_end(&mut self, end: bool) {
        let distance = if end { -1e12 } else { 1e12 };
        self.scroll_by(0.0, distance);
    }

    /// Adds a caret on the row above the first caret or below the last one.
    pub fn add_caret_vertically(&mut self, below: bool) {
        let cursors = self.doc().cursors.clone();
        let Some(edge) = (if below { cursors.last() } else { cursors.first() }).map(|c| c.head) else { return };
        let (row, x) = self.locate(edge);
        let total = self.rows.total_rows();
        if (below && row + 1 >= total) || (!below && row == 0) {
            return;
        }
        let target = if below { row + 1 } else { row - 1 };
        let caret = TextSelection::caret(self.position_on_row(target, x));
        let mut all = cursors;
        all.push(caret);
        // Keep the newest caret last, so it's the one the view follows.
        let mut merged = normalize_cursors(all);
        if let Some(index) = merged.iter().position(|c| *c == caret) {
            let newest = merged.remove(index);
            merged.push(newest);
        }
        self.doc_mut().cursors = merged;
        self.goals = None;
        self.reveal();
    }

    /// Escape: back to a single caret.
    pub fn single_cursor(&mut self) {
        let doc = self.doc_mut();
        if doc.cursors.len() > 1
            && let Some(&last) = doc.cursors.last()
        {
            doc.cursors = vec![last];
        }
    }

    pub fn select_all(&mut self) {
        let end = self.buffer().end_position();
        self.doc_mut().cursors = vec![TextSelection { anchor: TextPos::new(0, 0), head: end }];
        self.buffer().break_undo_coalescing();
    }

    pub fn select_lines(&mut self) {
        let cursors = self.doc().cursors.clone();
        self.doc_mut().cursors = self.buffer().select_lines(&cursors);
        self.reveal();
    }

    pub fn select_next_occurrence(&mut self) {
        let cursors = self.doc().cursors.clone();
        self.doc_mut().cursors = self.buffer().select_next_occurrence(&cursors);
        self.reveal();
    }

    // MARK: Clipboard

    pub fn copy(&mut self) {
        let text = self.buffer().copy_text(&self.doc().cursors);
        self.write_clipboard(text);
    }

    pub fn cut(&mut self) {
        let mut cut = String::new();
        self.apply(|buffer, cursors| {
            let (text, after) = buffer.cut(cursors);
            cut = text;
            after
        });
        self.write_clipboard(cut);
    }

    pub fn paste(&mut self) {
        let Some(text) = self.read_clipboard() else { return };
        self.apply(|buffer, cursors| buffer.paste(cursors, &text));
    }

    fn write_clipboard(&mut self, text: String) {
        if self.private_clipboard.is_some() {
            self.private_clipboard = Some(text);
        } else if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(text);
        }
    }

    fn read_clipboard(&mut self) -> Option<String> {
        match &self.private_clipboard {
            Some(text) => Some(text.clone()),
            None => arboard::Clipboard::new().ok()?.get_text().ok(),
        }
    }

    // MARK: Mouse

    /// A click in the text: places the caret (twice selects a word, three times a line), Shift
    /// extends the selection, ⌘ (Ctrl elsewhere) adds a caret, and ⌥ (Alt) starts a column selection.
    pub fn pointer_down(&mut self, x: f32, y: f32, add: bool, column: bool, shift: bool) {
        let pos = self.hit(x, y);
        let now = Instant::now();
        let count = match self.last_click {
            Some((at, px, py, n)) if now.duration_since(at) < DOUBLE_CLICK && (px - x).abs() < 5.0 && (py - y).abs() < 5.0 => n % 3 + 1,
            _ => 1,
        };
        self.last_click = Some((now, x, y, count));
        self.touch();
        self.goals = None;
        self.preedit.clear();
        self.buffer().break_undo_coalescing();

        if column && !add && !shift {
            self.column_drag = Some((pos.line, x + self.doc().scroll_x as f32));
            self.drag = None;
            self.doc_mut().cursors = vec![TextSelection::caret(pos)];
            return;
        }
        self.column_drag = None;
        if shift && !add {
            let current = self.doc_mut().cursors.last_mut().map(|last| {
                last.head = pos;
                *last
            });
            if let Some(current) = current {
                self.drag = Some(Drag { origin: TextSelection::caret(current.anchor), unit: 1, current });
            }
            return;
        }
        let selection = self.unit_at(pos, count);
        let doc = self.doc_mut();
        if add {
            let mut cursors = doc.cursors.clone();
            cursors.push(selection);
            doc.cursors = normalize_cursors(cursors);
        } else {
            doc.cursors = vec![selection];
        }
        self.drag = Some(Drag { origin: selection, unit: count, current: selection });
    }

    pub fn pointer_drag(&mut self, x: f32, y: f32) {
        if let Some((line, start_x)) = self.column_drag {
            self.column_select(line, start_x, x, y);
            return;
        }
        let Some(drag) = self.drag.as_ref() else { return };
        let (origin, unit, current) = (drag.origin, drag.unit, drag.current);
        // Dragging past the top or bottom scrolls.
        if y < 0.0 {
            self.scroll_by(0.0, (-y).min(self.line_height * 3.0));
        } else if y > self.height {
            self.scroll_by(0.0, -(y - self.height).min(self.line_height * 3.0));
        }
        let pos = self.hit(x, y);
        let here = self.unit_at(pos, unit);
        let selection = if here.start() < origin.start() {
            TextSelection { anchor: origin.end(), head: here.start() }
        } else {
            TextSelection { anchor: origin.start(), head: here.end().max(origin.end()) }
        };
        let doc = self.doc_mut();
        let mut cursors: Vec<TextSelection> = doc.cursors.iter().filter(|c| **c != current).copied().collect();
        cursors.push(selection);
        let mut merged = normalize_cursors(cursors);
        if let Some(index) = merged.iter().position(|c| *c == selection) {
            let item = merged.remove(index);
            merged.push(item);
        }
        doc.cursors = merged;
        if let Some(drag) = self.drag.as_mut() {
            drag.current = selection;
        }
        self.touch();
    }

    pub fn pointer_up(&mut self) {
        self.drag = None;
        self.column_drag = None;
    }

    /// A rectangular selection from where the drag started: the same columns (by x) on every
    /// line in between. Lines too short to reach the left edge get a caret at their end.
    fn column_select(&mut self, start_line: u64, start_x: f32, x: f32, y: f32) {
        let here = self.hit(x, y);
        let x = x + self.doc().scroll_x as f32;
        let (left, right) = (start_x.min(x), start_x.max(x));
        let lines: Vec<u64> = if here.line >= start_line { (start_line..=here.line).collect() } else { (here.line..=start_line).rev().collect() };
        let mut cursors = Vec::with_capacity(lines.len());
        for line in lines {
            let geometry = self.geometry(line);
            let from = geometry.column_at(0, left) as u64;
            let to = geometry.column_at(0, right) as u64;
            let (anchor, head) = if x >= start_x { (from, to) } else { (to, from) };
            cursors.push(TextSelection { anchor: TextPos::new(line, anchor), head: TextPos::new(line, head) });
        }
        // The caret being dragged is the main one.
        let mut sorted = normalize_cursors(cursors);
        if let Some(index) = sorted.iter().position(|c| c.head.line == here.line) {
            let main = sorted.remove(index);
            sorted.push(main);
        }
        self.doc_mut().cursors = sorted;
        self.touch();
    }

    /// What a click selects: a caret, the word, or the line.
    fn unit_at(&self, pos: TextPos, count: u32) -> TextSelection {
        let buffer = self.buffer();
        match count {
            2 => buffer.word_range_at(pos),
            3 => buffer.select_lines(&[TextSelection::caret(pos)]).first().copied().unwrap_or(TextSelection::caret(pos)),
            _ => TextSelection::caret(pos),
        }
    }
}
