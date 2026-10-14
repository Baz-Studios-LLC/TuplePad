//! The find bar: matches found on a background thread (so typing a query into a huge file stays
//! smooth), stepping through them, replacing one or all, counting, Mark All and bookmarking.

use slint::ComponentHandle;
use tuplepad_core::{EditKind, SearchQuery, SearchResults, TextSelection};

use crate::app::App;
use crate::editor::group;
use crate::FindBridge;

/// Matches kept for stepping through and counting in the bar.
const LIMIT: u64 = 100_000;

#[derive(Clone, Copy, PartialEq)]
enum Step {
    /// Select the first match from the caret on (as you type).
    First,
    Next,
    Previous,
    /// Just refresh the matches (after an edit).
    None,
}

#[derive(Default)]
pub struct FindState {
    matches: Vec<TextSelection>,
    truncated: bool,
    current: Option<usize>,
    /// What the matches were found for: document, buffer version and query.
    found_for: Option<(String, u64, String)>,
    generation: u64,
    searching: bool,
    error: Option<String>,
    /// Shown in place of the count until the next search (like "Replaced 12").
    message: Option<String>,
}

fn key(query: &SearchQuery) -> String {
    format!("{}\0{}{}{}", query.pattern, query.case_sensitive, query.whole_word, query.regex)
}

impl App {
    /// The find bar's query, if it has one.
    pub fn find_query(&self) -> Option<SearchQuery> {
        let ui = self.ui.upgrade()?;
        let bridge = ui.global::<FindBridge>();
        let pattern = bridge.get_query().to_string();
        (!pattern.is_empty()).then(|| SearchQuery {
            pattern,
            case_sensitive: bridge.get_case_sensitive(),
            whole_word: bridge.get_whole_word(),
            regex: bridge.get_regex(),
        })
    }

    /// Find… (and Find and Replace…): opens the bar, starting from the selection if there's one.
    pub fn open_find(&self, replace: bool) {
        let Some(ui) = self.ui.upgrade() else { return };
        let bridge = ui.global::<FindBridge>();
        let selected = {
            let editor = self.editor.borrow();
            let buffer = editor.buffer();
            editor
                .doc()
                .cursors
                .last()
                .filter(|c| c.anchor != c.head && c.start().line == c.end().line && c.end().column - c.start().column <= 200)
                .map(|c| buffer.text_in(c.start(), c.end()))
        };
        if let Some(text) = selected {
            bridge.set_query(text.into());
        }
        if replace {
            bridge.set_show_replace(true);
        }
        let was_open = bridge.get_open();
        bridge.set_open(true);
        bridge.set_focus_request(bridge.get_focus_request() + 1);
        if !was_open || self.find.borrow().found_for.is_none() {
            self.find_changed();
        }
    }

    pub fn close_find(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<FindBridge>().set_open(false);
            ui.invoke_focus_editor();
        }
        self.editor.borrow_mut().find_query = None;
        self.refresh();
    }

    /// The query or options changed: search again and select the first match from the caret.
    pub fn find_changed(&self) {
        self.find.borrow_mut().message = None;
        self.start_search(Step::First);
    }

    fn start_search(&self, step: Step) {
        let query = self.find_query();
        let (buffer, doc_id) = {
            let editor = self.editor.borrow();
            (editor.buffer(), editor.doc().id.clone())
        };
        self.editor.borrow_mut().find_query = query.clone();
        let generation = {
            let mut find = self.find.borrow_mut();
            find.generation += 1;
            find.error = None;
            find.current = None;
            if query.is_none() {
                find.matches.clear();
                find.found_for = None;
                find.searching = false;
            } else {
                find.searching = true;
            }
            find.generation
        };
        self.show_find_status();
        let Some(query) = query else {
            self.refresh();
            return;
        };
        let version = buffer.version();
        let found_for = (doc_id, version, key(&query));
        std::thread::spawn(move || {
            let result = buffer.find_all(query, LIMIT);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = crate::platform::app() {
                    app.search_done(generation, found_for, result, step);
                }
            });
        });
    }

    fn search_done(&self, generation: u64, found_for: (String, u64, String), result: Result<SearchResults, tuplepad_core::CoreError>, step: Step) {
        {
            let mut find = self.find.borrow_mut();
            if find.generation != generation {
                return;
            }
            find.searching = false;
            match result {
                Ok(results) => {
                    find.matches = results.matches;
                    find.truncated = results.truncated;
                    find.found_for = Some(found_for);
                }
                Err(error) => {
                    find.matches.clear();
                    find.found_for = None;
                    find.error = Some(error.to_string());
                }
            }
        }
        match step {
            Step::First => self.select_match(true, true),
            Step::Next => self.select_match(true, false),
            Step::Previous => self.select_match(false, false),
            Step::None => {
                // Keep pointing at the match the caret is on, if it still is one.
                let caret = self.editor.borrow().doc().cursors.last().copied();
                let mut find = self.find.borrow_mut();
                find.current = caret.and_then(|c| find.matches.iter().position(|m| m.start() == c.start() && m.end() == c.end()));
            }
        }
        self.show_find_status();
        self.refresh();
    }

    /// Whether the matches are still for this document, text and query.
    fn matches_current(&self) -> bool {
        let Some(query) = self.find_query() else { return false };
        let editor = self.editor.borrow();
        let doc = editor.doc();
        self.find.borrow().found_for == Some((doc.id.clone(), doc.buffer.version(), key(&query)))
    }

    /// Selects the next (or previous) match from the caret, wrapping around. With `inclusive`,
    /// a match starting at the caret counts (for search-as-you-type).
    fn select_match(&self, forward: bool, inclusive: bool) {
        let Some(caret) = self.editor.borrow().doc().cursors.last().copied() else { return };
        let index = {
            let find = self.find.borrow();
            let matches = &find.matches;
            if matches.is_empty() {
                None
            } else if forward {
                let from = if inclusive { caret.start() } else { caret.end() };
                matches.iter().position(|m| if inclusive { m.start() >= from } else { m.start() >= from && *m != caret }).or(Some(0))
            } else {
                matches.iter().rposition(|m| m.end() <= caret.start() && *m != caret).or(Some(matches.len() - 1))
            }
        };
        self.find.borrow_mut().current = index;
        if let Some(index) = index {
            let found = self.find.borrow().matches[index];
            let mut editor = self.editor.borrow_mut();
            editor.doc_mut().cursors = vec![found];
            editor.buffer().break_undo_coalescing();
            editor.reveal_centered();
        }
    }

    /// Find Next and Find Previous (and Return in the bar).
    pub fn find_step(&self, forward: bool) {
        if self.find_query().is_none() {
            self.open_find(false);
            return;
        }
        self.find.borrow_mut().message = None;
        if self.matches_current() {
            self.select_match(forward, false);
            self.show_find_status();
            self.refresh();
        } else {
            self.start_search(if forward { Step::Next } else { Step::Previous });
        }
    }

    /// Replaces the selected match and moves on to the next.
    pub fn replace_one(&self) {
        let (Some(query), Some(ui)) = (self.find_query(), self.ui.upgrade()) else { return };
        let replacement = ui.global::<FindBridge>().get_replacement().to_string();
        {
            let mut editor = self.editor.borrow_mut();
            let buffer = editor.buffer();
            let current = editor.doc().cursors.last().copied();
            if let Some((range, text)) = current.and_then(|c| buffer.replacement_for(query, c, replacement).ok().flatten().map(|text| (c, text))) {
                editor.apply(|buffer, cursors| buffer.edit(vec![range], vec![text], EditKind::Other, cursors.to_vec()));
            }
        }
        self.start_search(Step::Next);
    }

    /// Replaces every match in the document as one undo step.
    pub fn replace_all_matches(&self) {
        let (Some(query), Some(ui)) = (self.find_query(), self.ui.upgrade()) else { return };
        let replacement = ui.global::<FindBridge>().get_replacement().to_string();
        let result = {
            let mut editor = self.editor.borrow_mut();
            let cursors = editor.doc().cursors.clone();
            let result = editor.buffer().replace_all(query, replacement, cursors);
            if matches!(result, Ok(n) if n > 0) {
                editor.text_replaced();
            }
            result
        };
        let message = match result {
            Ok(0) => "Nothing to replace".to_string(),
            Ok(1) => "Replaced 1".to_string(),
            Ok(n) => format!("Replaced {}", group(n)),
            Err(error) => error.to_string(),
        };
        self.start_search(Step::None);
        self.find.borrow_mut().message = Some(message);
        self.show_find_status();
    }

    /// Counts every match (beyond the bar's limit too), in the background.
    pub fn count_matches(&self) {
        let Some(query) = self.find_query() else {
            self.open_find(false);
            return;
        };
        let buffer = self.editor.borrow().buffer();
        let generation = self.find.borrow().generation;
        std::thread::spawn(move || {
            let result = buffer.count_matches(query);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = crate::platform::app() else { return };
                if app.find.borrow().generation != generation {
                    return;
                }
                app.find.borrow_mut().message = Some(match result {
                    Ok(1) => "1 match".into(),
                    Ok(n) => format!("{} matches", group(n)),
                    Err(error) => error.to_string(),
                });
                app.show_find_status();
            });
        });
    }

    /// Mark All: keeps every match highlighted (until Clear Marks).
    pub fn mark_all(&self) {
        let Some(query) = self.find_query() else {
            self.open_find(false);
            return;
        };
        self.editor.borrow_mut().doc_mut().mark = Some(query);
    }

    pub fn clear_marks(&self) {
        self.editor.borrow_mut().doc_mut().mark = None;
    }

    /// Bookmarks every line with a match.
    pub fn bookmark_matching_lines(&self) {
        let Some(query) = self.find_query() else {
            self.open_find(false);
            return;
        };
        let result = self.editor.borrow().buffer().bookmark_matching_lines(query);
        self.find.borrow_mut().message = Some(match result {
            Ok(1) => "Bookmarked 1 line".into(),
            Ok(n) => format!("Bookmarked {} lines", group(n)),
            Err(error) => error.to_string(),
        });
        self.show_find_status();
    }

    /// Use Selection for Find (⌘E): the selected text becomes the query, without opening the bar.
    pub fn use_selection_for_find(&self) {
        let selected = {
            let editor = self.editor.borrow();
            let buffer = editor.buffer();
            editor.doc().cursors.last().filter(|c| c.anchor != c.head).map(|c| buffer.text_in(c.start(), c.end()))
        };
        if let (Some(text), Some(ui)) = (selected, self.ui.upgrade()) {
            ui.global::<FindBridge>().set_query(text.into());
            self.find.borrow_mut().found_for = None;
            if ui.global::<FindBridge>().get_open() {
                self.start_search(Step::None);
            }
        }
    }

    /// After edits or switching documents, finds again (keeping the place) if the bar is open.
    pub fn refresh_find(&self) {
        let open = self.ui.upgrade().is_some_and(|ui| ui.global::<FindBridge>().get_open());
        if !open || self.find_query().is_none() || self.find.borrow().searching || self.matches_current() {
            return;
        }
        self.find.borrow_mut().message = None;
        self.start_search(Step::None);
    }

    // MARK: Bookmarks

    /// A click on a line number.
    pub fn toggle_bookmark_at_row(&self, row: u64) {
        {
            let mut editor = self.editor.borrow_mut();
            let line = editor.line_at_row(row);
            editor.buffer().toggle_bookmark(line);
            editor.touch();
        }
        self.refresh();
    }

    pub fn toggle_bookmark(&self) {
        let mut editor = self.editor.borrow_mut();
        let buffer = editor.buffer();
        let mut lines: Vec<u64> = editor.doc().cursors.iter().map(|c| c.head.line).collect();
        lines.dedup();
        for line in lines {
            buffer.toggle_bookmark(line);
        }
        editor.touch();
    }

    /// Next Bookmark and Previous Bookmark, wrapping around.
    pub fn step_bookmark(&self, forward: bool) {
        let mut editor = self.editor.borrow_mut();
        let Some(line) = editor.doc().cursors.last().map(|c| c.head.line) else { return };
        if let Some(target) = editor.buffer().next_bookmark(line, forward) {
            editor.jump_to_line(target);
        }
    }

    pub fn copy_bookmarked_lines(&self) {
        let mut editor = self.editor.borrow_mut();
        let text = editor.buffer().bookmarked_text();
        if !text.is_empty() {
            editor.write_clipboard(text);
        }
    }

    pub fn delete_bookmarked_lines(&self) {
        let mut editor = self.editor.borrow_mut();
        if editor.buffer().bookmarks().is_empty() {
            return;
        }
        editor.apply(|buffer, cursors| buffer.delete_bookmarked_lines(cursors.to_vec()));
    }

    pub fn clear_bookmarks(&self) {
        let mut editor = self.editor.borrow_mut();
        editor.buffer().clear_bookmarks();
        editor.touch();
    }

    fn show_find_status(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let bridge = ui.global::<FindBridge>();
        let find = self.find.borrow();
        let (status, error) = if let Some(error) = &find.error {
            (if bridge.get_regex() { "Invalid pattern".to_string() } else { error.clone() }, true)
        } else if let Some(message) = &find.message {
            (message.clone(), false)
        } else if bridge.get_query().is_empty() {
            (String::new(), false)
        } else if find.searching {
            ("…".to_string(), false)
        } else if find.matches.is_empty() {
            ("No matches".to_string(), false)
        } else {
            let total = format!("{}{}", group(find.matches.len() as u64), if find.truncated { "+" } else { "" });
            match find.current {
                Some(i) => (format!("{} of {total}", group(i as u64 + 1)), false),
                None => (format!("{total} found"), false),
            }
        };
        bridge.set_status(status.into());
        bridge.set_error(error);
    }
}
