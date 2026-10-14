//! `TextBuffer`: one open document's text, edits, undo history and file format.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use ropey::{Rope, RopeBuilder};

use crate::encoding::{self, TextEncoding};
use crate::error::{CoreError, CoreResult};
use crate::history::{Change, History};
use crate::search::{self, SearchQuery};
use crate::syntax::{self, HighlightSpan, Syntax};
use crate::types::{EditKind, FileInfo, LineEnding, TextPos, TextSelection};

#[path = "buffer_tools.rs"]
mod tools;
pub use tools::BracketMatch;

#[path = "buffer_column.rs"]
mod column;
pub use column::{ColumnInsert, NumberBase, NumberPadding, column_values};

#[path = "buffer_csv.rs"]
pub(crate) mod csv_tools;

#[path = "buffer_search.rs"]
mod marks;
pub use marks::LineResults;

#[path = "buffer_folds.rs"]
mod folds;
pub use folds::FoldRange;

#[path = "buffer_files.rs"]
mod files;
pub use files::{AppendResult, DiskStatus};

/// The result of a search: matches in document order (anchor = start, head = end).
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SearchResults {
    pub matches: Vec<TextSelection>,
    /// The search stopped at the limit; there are more matches.
    pub truncated: bool,
}

#[derive(uniffi::Object)]
pub struct TextBuffer {
    state: RwLock<State>,
}

struct State {
    rope: Rope,
    path: Option<PathBuf>,
    encoding: TextEncoding,
    line_ending: LineEnding,
    had_decode_errors: bool,
    looks_binary: bool,
    /// Encoding or line-ending changes that need saving but aren't text edits.
    format_changed: bool,
    history: History,
    /// Longest line seen, in bytes. Only grows while editing; used to size the horizontal scroll area.
    max_line_bytes: u64,
    version: u64,
    /// The language's parser and syntax tree, if the document has a language.
    syntax: Option<Syntax>,
    /// The file as it was when the text last matched it.
    disk: Option<files::DiskStamp>,
    /// Bookmarked lines (0-based). They move with edits; lines past the end are ignored.
    bookmarks: std::collections::BTreeSet<usize>,
    /// Folded regions: first line (which stays visible) → last hidden line.
    folds: std::collections::BTreeMap<usize, usize>,
    /// The delimiter, when the document is CSV or TSV (which colors columns instead of parsing).
    csv: Option<char>,
}

#[uniffi::export]
impl TextBuffer {
    /// An empty, untitled document.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Self::with_text(String::new())
    }

    /// An empty document that will be saved in a chosen encoding and line ending
    /// (the defaults from Settings). Starts unmodified.
    #[uniffi::constructor]
    pub fn untitled(encoding: TextEncoding, line_ending: LineEnding) -> Arc<Self> {
        let mut state = State::new("", encoding);
        state.line_ending = line_ending;
        Arc::new(TextBuffer { state: RwLock::new(state) })
    }

    /// An untitled document with initial contents.
    #[uniffi::constructor]
    pub fn with_text(text: String) -> Arc<Self> {
        Arc::new(TextBuffer { state: RwLock::new(State::new(&text, TextEncoding::utf8())) })
    }

    /// Brings back a document from a snapshot written by `write_snapshot`, with the
    /// file, format and modified state it had. Undo history starts fresh.
    #[uniffi::constructor]
    pub fn restore(
        snapshot_path: String,
        file_path: Option<String>,
        encoding: TextEncoding,
        line_ending: LineEnding,
        modified: bool,
    ) -> CoreResult<Arc<Self>> {
        let bytes = fs::read(&snapshot_path).map_err(|e| CoreError::Io { message: format!("{snapshot_path}: {e}") })?;
        let text = String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        let mut state = State::new(&text, encoding);
        state.path = file_path.map(PathBuf::from);
        state.line_ending = line_ending;
        if modified {
            state.history.mark_unsaved();
        }
        state.stamp_disk();
        state.detect_language();
        Ok(Arc::new(TextBuffer { state: RwLock::new(state) }))
    }

    /// Reads a file from disk, detecting its encoding and line endings.
    #[uniffi::constructor]
    pub fn open(path: String) -> CoreResult<Arc<Self>> {
        let buffer = Self::open_without_syntax(path)?;
        buffer.write().detect_language();
        Ok(buffer)
    }

    // MARK: Reading

    /// Number of lines. A trailing line break starts a final, empty line.
    pub fn line_count(&self) -> u64 {
        self.read().rope.len_lines() as u64
    }

    /// One line's text without its line break.
    pub fn line(&self, index: u64) -> String {
        let st = self.read();
        let line = st.clamp_line(index);
        st.line_content(line).to_string()
    }

    /// Several consecutive lines without their line breaks, for drawing.
    pub fn lines(&self, start: u64, count: u64) -> Vec<String> {
        let st = self.read();
        let total = st.rope.len_lines() as u64;
        (start.min(total)..start.saturating_add(count).min(total))
            .map(|line| st.line_content(line as usize).to_string())
            .collect()
    }

    /// Part of a line: up to `count` UTF-16 columns from `start`, without the line break. Drawing
    /// a very long line reads only what's on screen.
    pub fn line_slice(&self, index: u64, start: u64, count: u64) -> String {
        let st = self.read();
        let content = st.line_content(st.clamp_line(index));
        let len = content.len_utf16_cu();
        let from = (start as usize).min(len);
        let to = (start.saturating_add(count) as usize).min(len);
        content.slice(content.utf16_cu_to_char(from)..content.utf16_cu_to_char(to)).to_string()
    }

    /// Length of a line in UTF-16 code units, excluding its line break.
    pub fn line_length(&self, index: u64) -> u64 {
        let st = self.read();
        let line = st.clamp_line(index);
        st.line_content(line).len_utf16_cu() as u64
    }

    /// An upper bound on the longest line's length, in characters.
    pub fn max_line_length(&self) -> u64 {
        self.read().max_line_bytes
    }

    pub fn text(&self) -> String {
        self.read().rope.to_string()
    }

    pub fn text_in(&self, start: TextPos, end: TextPos) -> String {
        let st = self.read();
        let (a, b) = (st.pos_to_char(start), st.pos_to_char(end));
        st.rope.slice(a.min(b)..a.max(b)).to_string()
    }

    /// The first non-blank lines, trimmed, for the sidebar preview.
    pub fn preview(&self, max_chars: u64) -> String {
        let st = self.read();
        let mut preview = String::new();
        for line in st.rope.lines().take(200) {
            // Only the start of each line matters (it may be megabytes long).
            let text: String = line.chars().take(max_chars as usize * 2).collect();
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if !preview.is_empty() {
                preview.push(' ');
            }
            preview.extend(text.chars().take(max_chars as usize - preview.chars().count().min(max_chars as usize)));
            if preview.chars().count() >= max_chars as usize {
                break;
            }
        }
        preview
    }

    pub fn len_utf16(&self) -> u64 {
        self.read().rope.len_utf16_cu() as u64
    }

    /// Offset of a position from the start of the document in UTF-16 code units.
    pub fn utf16_offset(&self, pos: TextPos) -> u64 {
        let st = self.read();
        st.rope.char_to_utf16_cu(st.pos_to_char(pos)) as u64
    }

    pub fn position_at_utf16_offset(&self, offset: u64) -> TextPos {
        let st = self.read();
        let offset = (offset as usize).min(st.rope.len_utf16_cu());
        st.char_to_pos(st.rope.utf16_cu_to_char(offset))
    }

    /// Moves a position onto real text: an existing line, within its length, not inside a surrogate pair.
    pub fn clamp(&self, pos: TextPos) -> TextPos {
        let st = self.read();
        st.char_to_pos(st.pos_to_char(pos))
    }

    // MARK: Editing

    /// Replaces each selection with text: `texts[i]` for selection `i`, or `texts[0]` for all of them.
    /// Overlapping selections merge. Returns carets placed after each insertion, in document order.
    pub fn replace(
        &self,
        selections: Vec<TextSelection>,
        texts: Vec<String>,
        kind: EditKind,
    ) -> Vec<TextSelection> {
        self.edit(selections.clone(), texts, kind, selections)
    }

    /// Like `replace`, but separates what gets replaced (`ranges`) from the cursors the
    /// user sees (`cursors`), which undo restores. Backspace replaces the character
    /// before each caret, yet undoing it should put the caret back, not select that character.
    pub fn edit(
        &self,
        ranges: Vec<TextSelection>,
        texts: Vec<String>,
        kind: EditKind,
        cursors: Vec<TextSelection>,
    ) -> Vec<TextSelection> {
        let mut st = self.write();
        if ranges.is_empty() || texts.is_empty() {
            return cursors;
        }
        let (changes, after) = st.splice(&ranges, &texts);
        if changes.is_empty() {
            return cursors;
        }
        st.reparse();
        st.history.record(changes, cursors, after.clone(), kind);
        st.version += 1;
        after
    }

    /// Reverts the last undo step. Returns the selections to restore, or nil if there was nothing to undo.
    pub fn undo(&self) -> Option<Vec<TextSelection>> {
        let mut st = self.write();
        let transaction = st.history.pop_undo()?;
        for change in transaction.changes.iter().rev() {
            st.revert(change);
        }
        st.reparse();
        let selections = transaction.before.clone();
        st.history.push_redo(transaction);
        st.version += 1;
        Some(selections)
    }

    pub fn redo(&self) -> Option<Vec<TextSelection>> {
        let mut st = self.write();
        let transaction = st.history.pop_redo()?;
        for change in &transaction.changes {
            st.reapply(change);
        }
        st.reparse();
        let selections = transaction.after.clone();
        st.history.push_undo(transaction);
        st.version += 1;
        Some(selections)
    }

    pub fn can_undo(&self) -> bool {
        self.read().history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.read().history.can_redo()
    }

    /// Call when the caret moves for a reason other than editing, so the next keystroke starts a new undo step.
    pub fn break_undo_coalescing(&self) {
        self.write().history.break_coalescing();
    }

    /// Rewrites every line break to `ending` as one undo step, and uses it for new lines.
    pub fn convert_line_endings(&self, ending: LineEnding, selections: Vec<TextSelection>) {
        let mut st = self.write();
        let before = st.rope.clone();
        let mut builder = RopeBuilder::new();
        let mut pending_cr = false;
        let mut out = String::with_capacity(64 * 1024);
        for chunk in before.chunks() {
            for c in chunk.chars() {
                match c {
                    '\r' => {
                        if pending_cr {
                            out.push_str(ending.as_str());
                        }
                        pending_cr = true;
                    }
                    '\n' => {
                        out.push_str(ending.as_str());
                        pending_cr = false;
                    }
                    _ => {
                        if pending_cr {
                            out.push_str(ending.as_str());
                            pending_cr = false;
                        }
                        out.push(c);
                    }
                }
            }
            if out.len() >= 60 * 1024 {
                builder.append(&out);
                out.clear();
            }
        }
        if pending_cr {
            out.push_str(ending.as_str());
        }
        builder.append(&out);
        let after = builder.finish();

        let ending_before = st.line_ending;
        st.rope = after.clone();
        st.line_ending = ending;
        st.text_replaced();
        let change = Change::Replace { before, after, ending_before, ending_after: ending };
        st.history.record(vec![change], selections.clone(), selections, EditKind::Other);
        st.version += 1;
    }

    // MARK: Find and replace

    /// Every match of `query`, up to `limit`.
    pub fn find_all(&self, query: SearchQuery, limit: u64) -> CoreResult<SearchResults> {
        let Some(regex) = search::compile(&query)? else {
            return Ok(SearchResults { matches: Vec::new(), truncated: false });
        };
        // Search a snapshot, so typing isn't held up while a long search runs.
        let rope = self.snapshot();
        let (found, truncated) = search::find(&rope, &regex, &query, None, limit as usize);
        let matches = found
            .iter()
            .map(|f| TextSelection { anchor: rope_char_to_pos(&rope, f.start), head: rope_char_to_pos(&rope, f.end) })
            .collect();
        Ok(SearchResults { matches, truncated })
    }

    /// The text that replacing `range` would insert (expanding `$1` references in regex mode),
    /// or nil if `range` isn't exactly a match anymore.
    pub fn replacement_for(&self, query: SearchQuery, range: TextSelection, replacement: String) -> CoreResult<Option<String>> {
        let Some(regex) = search::compile(&query)? else { return Ok(None) };
        let st = self.read();
        let (start, end) = (st.pos_to_char(range.start()), st.pos_to_char(range.end()));
        // Search the lines around the range so anchors like ^ and \b see their real context.
        let first = st.rope.line_to_char(st.rope.char_to_line(start));
        let last_line = st.rope.char_to_line(end);
        let last = if last_line + 1 < st.rope.len_lines() { st.rope.line_to_char(last_line + 1) } else { st.rope.len_chars() };
        let context = Rope::from(st.rope.slice(first..last));
        let (found, _) = search::find(&context, &regex, &query, Some(&replacement), usize::MAX);
        Ok(found
            .into_iter()
            .find(|f| f.start + first == start && f.end + first == end)
            .and_then(|f| f.replacement))
    }

    /// Replaces every match as one undo step. Returns how many were replaced.
    pub fn replace_all(&self, query: SearchQuery, replacement: String, cursors: Vec<TextSelection>) -> CoreResult<u64> {
        let Some(regex) = search::compile(&query)? else { return Ok(0) };
        let mut st = self.write();
        let (found, _) = search::find(&st.rope, &regex, &query, Some(&replacement), usize::MAX);
        if found.is_empty() {
            return Ok(0);
        }
        let before = st.rope.clone();
        let mut builder = RopeBuilder::new();
        let mut copied = 0;
        for f in &found {
            for chunk in before.slice(copied..f.start).chunks() {
                builder.append(chunk);
            }
            builder.append(f.replacement.as_deref().unwrap_or(""));
            copied = f.end;
        }
        for chunk in before.slice(copied..).chunks() {
            builder.append(chunk);
        }
        let after = builder.finish();
        st.rope = after.clone();
        st.text_replaced();
        st.recount_line_lengths();
        let after_cursors: Vec<TextSelection> = cursors
            .iter()
            .map(|c| TextSelection { anchor: st.clamp_pos(c.anchor), head: st.clamp_pos(c.head) })
            .collect();
        let ending = st.line_ending;
        let change = Change::Replace { before, after, ending_before: ending, ending_after: ending };
        st.history.record(vec![change], cursors, after_cursors, EditKind::Other);
        st.version += 1;
        Ok(found.len() as u64)
    }

    // MARK: Syntax

    /// The language used for highlighting, like "rust" or "python", or nil for plain text.
    pub fn language(&self) -> Option<String> {
        let st = self.read();
        match st.csv {
            Some('\t') => Some("tsv".into()),
            Some(_) => Some("csv".into()),
            None => st.syntax.as_ref().map(|s| s.id().to_string()),
        }
    }

    /// Changes the highlighting language (nil for plain text) and parses the document.
    /// Takes a moment for large files, so call it off the main thread when you can.
    pub fn set_language(&self, id: Option<String>) {
        let mut st = self.write();
        st.csv = match id.as_deref() {
            Some("csv") => Some(csv_tools::detect_delimiter(&st.rope)).filter(|&d| d != '\t').or(Some(',')),
            Some("tsv") => Some('\t'),
            _ => None,
        };
        st.syntax = if st.csv.is_some() { None } else { id.and_then(|id| Syntax::new(&id)) };
        st.reparse();
        st.version += 1;
    }

    /// Colored runs for part of one line, UTF-16 columns `start..start + count` (for very long
    /// lines, where only part is on screen).
    pub fn highlights_in_line(&self, line: u64, start: u64, count: u64) -> Vec<HighlightSpan> {
        let st = self.read();
        if st.csv.is_some() || line >= st.rope.len_lines() as u64 {
            return Vec::new();
        }
        match &st.syntax {
            Some(syntax) => syntax.highlights_in_line(&st.rope, line as usize, start as usize, start.saturating_add(count) as usize),
            None => Vec::new(),
        }
    }

    /// Colored runs for `count` lines starting at `start_line`.
    pub fn highlights(&self, start_line: u64, count: u64) -> Vec<HighlightSpan> {
        let st = self.read();
        if let Some(delimiter) = st.csv {
            return st.csv_highlights(delimiter, start_line as usize, count as usize);
        }
        match &st.syntax {
            Some(syntax) => syntax.highlights(&st.rope, start_line as usize, count as usize),
            None => Vec::new(),
        }
    }

    // MARK: File

    pub fn is_modified(&self) -> bool {
        let st = self.read();
        st.format_changed || st.history.is_modified()
    }

    /// Increases with every change to the text, including undo and redo.
    pub fn version(&self) -> u64 {
        self.read().version
    }

    pub fn path(&self) -> Option<String> {
        self.read().path.as_ref().map(|p| p.to_string_lossy().into_owned())
    }

    pub fn file_info(&self) -> FileInfo {
        let st = self.read();
        FileInfo {
            path: st.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            encoding: st.encoding.clone(),
            line_ending: st.line_ending,
            had_decode_errors: st.had_decode_errors,
            looks_binary: st.looks_binary,
        }
    }

    /// Chooses the encoding used for the next save.
    pub fn set_encoding(&self, encoding: TextEncoding) -> CoreResult<()> {
        encoding.resolve()?;
        let mut st = self.write();
        if st.encoding != encoding {
            st.encoding = encoding;
            st.format_changed = true;
        }
        Ok(())
    }

    pub fn save(&self) -> CoreResult<()> {
        let mut st = self.write();
        let path = st.path.clone().ok_or(CoreError::NoPath)?;
        st.write_to(&path)?;
        st.mark_saved();
        st.stamp_disk();
        Ok(())
    }

    pub fn save_as(&self, path: String) -> CoreResult<()> {
        let mut st = self.write();
        let path = PathBuf::from(path);
        st.write_to(&path)?;
        st.path = Some(path);
        st.mark_saved();
        st.stamp_disk();
        if st.syntax.is_none() {
            st.detect_language();
            st.version += 1;
        }
        Ok(())
    }

    /// Writes the current text as UTF-8 to `path`, for restoring unsaved work after
    /// a relaunch. Doesn't change the document's file, format or saved state.
    pub fn write_snapshot(&self, path: String) -> CoreResult<()> {
        let st = self.read();
        write_atomically(Path::new(&path), st.rope.chunks(), &TextEncoding::utf8())
    }
}

impl TextBuffer {
    /// The text as it is now. Cheap (ropes share their pieces), and lets slow read-only work
    /// run without holding the lock that typing needs.
    pub(crate) fn snapshot(&self) -> Rope {
        self.read().rope.clone()
    }

    /// Reads a file without parsing it for highlighting, for tools that only change its text.
    pub(crate) fn open_without_syntax(path: String) -> CoreResult<Arc<Self>> {
        let bytes = fs::read(&path).map_err(|e| CoreError::Io { message: format!("{path}: {e}") })?;
        let decoded = encoding::decode(bytes);
        let mut state = State::new(&decoded.text, decoded.encoding);
        state.path = Some(PathBuf::from(path));
        state.had_decode_errors = decoded.had_errors;
        state.stamp_disk();
        Ok(Arc::new(TextBuffer { state: RwLock::new(state) }))
    }

    fn read(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }
}

impl State {
    fn new(text: &str, encoding: TextEncoding) -> Self {
        let scan = scan_lines(text);
        State {
            rope: Rope::from_str(text),
            path: None,
            encoding,
            line_ending: scan.line_ending,
            had_decode_errors: false,
            looks_binary: text[..text.len().min(64 * 1024)].contains('\0'),
            format_changed: false,
            history: History::default(),
            max_line_bytes: scan.max_line_bytes,
            version: 0,
            syntax: None,
            disk: None,
            bookmarks: Default::default(),
            folds: Default::default(),
            csv: None,
        }
    }

    /// Picks a language from the file name (or a `#!` line) and parses.
    fn detect_language(&mut self) {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        let extension = name.as_deref().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_lowercase());
        self.csv = match extension.as_deref() {
            Some("csv") => Some(csv_tools::detect_delimiter(&self.rope)),
            Some("tsv" | "tab") => Some('\t'),
            _ => None,
        };
        if self.csv.is_some() {
            self.syntax = None;
            return;
        }
        self.syntax = Syntax::detect(name.as_deref().unwrap_or(""), &self.rope);
        self.reparse();
    }

    fn clamp_pos(&self, pos: TextPos) -> TextPos {
        self.char_to_pos(self.pos_to_char(pos))
    }

    /// Recomputes the longest line after the whole text changed.
    fn recount_line_lengths(&mut self) {
        self.max_line_bytes = self.rope.lines().map(|l| l.len_bytes() as u64).max().unwrap_or(0);
    }

    fn reparse(&mut self) {
        if let Some(syntax) = &mut self.syntax {
            syntax.parse(&self.rope);
        }
    }

    /// After the whole text is swapped out: folds no longer fit, and parsing starts over.
    fn text_replaced(&mut self) {
        self.folds.clear();
        self.reset_syntax();
    }

    /// After the whole text is swapped out, parse from scratch.
    fn reset_syntax(&mut self) {
        if let Some(syntax) = &mut self.syntax {
            syntax.reset();
            syntax.parse(&self.rope);
        }
    }

    /// Replaces the chars `at..end` with `text`, keeping the syntax tree and line lengths in step.
    fn replace_chars(&mut self, at: usize, end: usize, text: &str) {
        let line_edit = (!self.bookmarks.is_empty() || !self.folds.is_empty()).then(|| self.line_edit_start(at, end));
        let edit = self.syntax.is_some().then(|| {
            syntax::edit_start(&self.rope, self.rope.char_to_byte(at), self.rope.char_to_byte(end))
        });
        if at < end {
            self.rope.remove(at..end);
        }
        if !text.is_empty() {
            self.rope.insert(at, text);
        }
        if let (Some(syntax), Some(edit)) = (self.syntax.as_mut(), edit) {
            syntax.edit(&syntax::finish_edit(&self.rope, edit, text.len()));
        }
        let inserted_end = at + text.chars().count();
        if let Some(line_edit) = line_edit {
            self.lines_moved(line_edit, inserted_end);
        }
        self.note_line_lengths(at, inserted_end);
    }

    fn clamp_line(&self, line: u64) -> usize {
        (line as usize).min(self.rope.len_lines() - 1)
    }

    /// A line without its trailing line break.
    fn line_content(&self, line: usize) -> ropey::RopeSlice<'_> {
        let slice = self.rope.line(line);
        let len = slice.len_chars();
        let trim = match (len.checked_sub(2).map(|i| slice.char(i)), len.checked_sub(1).map(|i| slice.char(i))) {
            (Some('\r'), Some('\n')) => 2,
            (_, Some('\n' | '\r')) => 1,
            _ => 0,
        };
        slice.slice(..len - trim)
    }

    fn pos_to_char(&self, pos: TextPos) -> usize {
        let line = self.clamp_line(pos.line);
        let start = self.rope.line_to_char(line);
        let content = self.line_content(line);
        let column = (pos.column as usize).min(content.len_utf16_cu());
        // Rounds down out of the middle of a surrogate pair.
        start + content.utf16_cu_to_char(column)
    }

    fn char_to_pos(&self, index: usize) -> TextPos {
        rope_char_to_pos(&self.rope, index)
    }

    /// Applies one replacement per selection, front to back, shifting later ones by earlier length changes.
    fn splice(&mut self, selections: &[TextSelection], texts: &[String]) -> (Vec<Change>, Vec<TextSelection>) {
        let mut ranges: Vec<(usize, usize, usize)> = selections
            .iter()
            .enumerate()
            .map(|(i, s)| (self.pos_to_char(s.start()), self.pos_to_char(s.end()), i))
            .collect();
        ranges.sort_unstable_by_key(|&(start, end, _)| (start, end));

        let mut merged: Vec<(usize, usize, usize)> = Vec::with_capacity(ranges.len());
        for range in ranges {
            match merged.last_mut() {
                // Overlapping, or a caret touching another selection's edge.
                Some(last) if range.0 < last.1 || (range.0 == last.1 && (last.0 == last.1 || range.0 == range.1)) => {
                    last.1 = last.1.max(range.1);
                }
                _ => merged.push(range),
            }
        }

        let mut changes = Vec::with_capacity(merged.len());
        let mut carets = Vec::with_capacity(merged.len());
        let mut delta: isize = 0;
        for (start, end, index) in merged {
            let text = if texts.len() == 1 { &texts[0] } else { texts.get(index).map_or("", String::as_str) };
            let at = start.checked_add_signed(delta).expect("edit offsets stay in bounds");
            let end = end.checked_add_signed(delta).expect("edit offsets stay in bounds");
            let inserted_chars = text.chars().count();
            if at < end || !text.is_empty() {
                let removed = self.rope.slice(at..end).to_string();
                self.replace_chars(at, end, text);
                changes.push(Change::Splice { at, removed, inserted: text.to_string() });
            }
            carets.push(at + inserted_chars);
            delta += inserted_chars as isize - (end - at) as isize;
        }

        let carets = carets.into_iter().map(|c| TextSelection::caret(self.char_to_pos(c))).collect();
        (changes, carets)
    }

    fn revert(&mut self, change: &Change) {
        match change {
            Change::Splice { at, removed, inserted } => {
                self.replace_chars(*at, *at + inserted.chars().count(), removed);
            }
            Change::Replace { before, ending_before, .. } => {
                self.rope = before.clone();
                self.line_ending = *ending_before;
                self.text_replaced();
            }
        }
    }

    fn reapply(&mut self, change: &Change) {
        match change {
            Change::Splice { at, removed, inserted } => {
                self.replace_chars(*at, *at + removed.chars().count(), inserted);
            }
            Change::Replace { after, ending_after, .. } => {
                self.rope = after.clone();
                self.line_ending = *ending_after;
                self.text_replaced();
            }
        }
    }

    /// Keeps `max_line_bytes` an upper bound after text between two char offsets changed.
    fn note_line_lengths(&mut self, start: usize, end: usize) {
        let first = self.rope.char_to_line(start.min(self.rope.len_chars()));
        let last = self.rope.char_to_line(end.min(self.rope.len_chars()));
        for line in first..=last {
            self.max_line_bytes = self.max_line_bytes.max(self.rope.line(line).len_bytes() as u64);
        }
    }

    fn mark_saved(&mut self) {
        self.history.mark_saved();
        self.format_changed = false;
        self.had_decode_errors = false;
    }

    fn write_to(&self, path: &Path) -> CoreResult<()> {
        write_atomically(path, self.rope.chunks(), &self.encoding)
    }
}

/// A char offset as (line, UTF-16 column).
pub(crate) fn rope_char_to_pos(rope: &Rope, index: usize) -> TextPos {
    let index = index.min(rope.len_chars());
    let line = rope.char_to_line(index);
    let start = rope.line_to_char(line);
    let column = rope.char_to_utf16_cu(index) - rope.char_to_utf16_cu(start);
    TextPos::new(line as u64, column as u64)
}

/// Writes to a temporary file beside the target, then renames it into place,
/// so a failed save never leaves a half-written file. Follows symlinks and keeps
/// the existing file's permissions.
fn write_atomically<'a>(path: &Path, chunks: impl Iterator<Item = &'a str>, encoding: &TextEncoding) -> CoreResult<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let name = target.file_name().ok_or_else(|| CoreError::Io { message: "Invalid file name".into() })?;
    let temp = target.with_file_name(format!(".{}.tuplepad-save", name.to_string_lossy()));

    let result = (|| -> CoreResult<()> {
        let file = File::create(&temp)?;
        let mut out = BufWriter::with_capacity(1 << 20, file);
        encoding::write_encoded(&mut out, chunks, encoding)?;
        out.flush()?;
        out.get_ref().sync_all()?;
        if let Ok(metadata) = fs::metadata(&target) {
            fs::set_permissions(&temp, metadata.permissions())?;
        }
        fs::rename(&temp, &target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

struct LineScan {
    line_ending: LineEnding,
    max_line_bytes: u64,
}

/// Finds the most common line ending and the longest line in one pass.
fn scan_lines(text: &str) -> LineScan {
    let bytes = text.as_bytes();
    let (mut lf, mut crlf, mut cr) = (0usize, 0usize, 0usize);
    let mut line_start = 0;
    let mut max_line = 0;
    let mut skip_lf_at = usize::MAX;
    for i in memchr::memchr2_iter(b'\n', b'\r', bytes) {
        if i == skip_lf_at {
            continue;
        }
        max_line = max_line.max(i - line_start);
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            crlf += 1;
            skip_lf_at = i + 1;
            line_start = i + 2;
        } else {
            if bytes[i] == b'\n' { lf += 1 } else { cr += 1 }
            line_start = i + 1;
        }
    }
    max_line = max_line.max(bytes.len() - line_start);

    let line_ending = if crlf > 0 && crlf >= lf && crlf >= cr {
        LineEnding::CrLf
    } else if cr > lf {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    };
    LineScan { line_ending, max_line_bytes: max_line as u64 }
}
