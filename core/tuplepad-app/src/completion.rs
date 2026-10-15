//! Word completion: after three letters of a word, words from the open documents that would
//! finish it, in a small list under the caret. Tab or Return accepts, ↑ and ↓ choose, and Esc
//! (or anything else) closes it. Each document's words are gathered on a background thread and
//! kept until it changes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tuplepad_core::{EditKind, TextBuffer, TextSelection, rank_completions};

use crate::app::App;

/// Letters typed before suggestions appear.
const MIN_PREFIX: usize = 3;
/// Suggestions shown.
const SHOWN: usize = 8;
/// A document's words are gathered again at most this often while it's being edited.
const REFRESH: Duration = Duration::from_secs(2);

/// The list showing under the caret.
#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    /// The word fragment being completed.
    pub range: TextSelection,
    pub prefix: String,
    pub items: Vec<String>,
    pub selected: usize,
}

struct Entry {
    version: u64,
    words: Arc<Vec<String>>,
    built: Instant,
}

/// Each open document's words, most frequent first.
#[derive(Default)]
pub struct WordIndex {
    entries: HashMap<String, Entry>,
    building: HashSet<String>,
}

impl App {
    /// Text typed into the editor: inserts it, then offers to finish the word being typed.
    pub fn typed(&self, text: &str) {
        if let Ok(mut editor) = self.editor.try_borrow_mut() {
            editor.insert_text(text);
        }
        self.update_completion(text);
        self.refresh();
    }

    fn update_completion(&self, typed: &str) {
        let wanted = self.settings.borrow().word_completion
            && typed.chars().count() == 1
            && typed.chars().all(|c| c.is_alphanumeric() || c == '_');
        let prefix = {
            let editor = self.editor.borrow();
            let cursors = &editor.doc().cursors;
            match cursors.as_slice() {
                [caret] if wanted && caret.anchor == caret.head && !editor.composing() => editor.buffer().completion_prefix(caret.head, MIN_PREFIX),
                _ => None,
            }
        };
        let Some((range, prefix)) = prefix else {
            self.editor.borrow_mut().completion = None;
            return;
        };
        let lists = self.word_lists();
        let refs: Vec<&[String]> = lists.iter().map(|l| l.as_slice()).collect();
        let items = rank_completions(&prefix, &refs, SHOWN);
        self.editor.borrow_mut().completion = (!items.is_empty()).then_some(Completion { range, prefix, items, selected: 0 });
    }

    /// Every open document's words, the current one first (gathering any that are out of date
    /// in the background, for next time).
    fn word_lists(&self) -> Vec<Arc<Vec<String>>> {
        let docs: Vec<(String, Arc<TextBuffer>)> = {
            let editor = self.editor.borrow();
            let current = editor.doc().id.clone();
            let mut docs: Vec<_> = editor.docs.iter().map(|d| (d.id.clone(), d.buffer.clone())).collect();
            docs.sort_by_key(|(id, _)| *id != current);
            docs
        };
        let mut index = self.word_index.borrow_mut();
        index.entries.retain(|id, _| docs.iter().any(|(d, _)| d == id));
        let mut lists = Vec::new();
        for (id, buffer) in docs {
            let version = buffer.version();
            let stale = match index.entries.get(&id) {
                Some(entry) => {
                    lists.push(entry.words.clone());
                    entry.version != version && entry.built.elapsed() >= REFRESH
                }
                None => true,
            };
            if stale && index.building.insert(id.clone()) {
                std::thread::spawn(move || {
                    let words = Arc::new(buffer.words(MIN_PREFIX as u32, 20_000));
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(app) = crate::platform::app() else { return };
                        let mut index = app.word_index.borrow_mut();
                        index.building.remove(&id);
                        index.entries.insert(id, Entry { version, words, built: Instant::now() });
                    });
                });
            }
        }
        lists
    }

    /// Gathers the current document's words ahead of time, so the first suggestions are ready.
    pub fn prime_word_index(&self) {
        if !self.settings.borrow().word_completion {
            return;
        }
        let known = {
            let Ok(editor) = self.editor.try_borrow() else { return };
            let id = &editor.doc().id;
            let index = self.word_index.borrow();
            index.entries.contains_key(id) || index.building.contains(id)
        };
        if !known {
            self.word_lists();
        }
    }

    /// Keys while the list is showing. Returns whether the key was used.
    pub fn completion_key(&self, text: &str) -> bool {
        let Ok(mut editor) = self.editor.try_borrow_mut() else { return false };
        let Some(completion) = editor.completion.as_mut() else { return false };
        let count = completion.items.len();
        match text {
            "\t" | "\n" | "\r" => {
                let chosen = completion.selected;
                drop(editor);
                self.accept_completion(chosen);
                return true;
            }
            "\u{F700}" => completion.selected = (completion.selected + count - 1) % count,
            "\u{F701}" => completion.selected = (completion.selected + 1) % count,
            "\u{1b}" => editor.completion = None,
            _ => {
                editor.completion = None;
                return false;
            }
        }
        drop(editor);
        self.refresh();
        true
    }

    /// Replaces the word fragment with a suggestion (a click, Tab or Return).
    pub fn accept_completion(&self, index: usize) {
        {
            let mut editor = self.editor.borrow_mut();
            let Some(completion) = editor.completion.take() else { return };
            let Some(word) = completion.items.get(index).cloned() else { return };
            let range = completion.range;
            editor.apply(|buffer, cursors| buffer.edit(vec![range], vec![word], EditKind::Typing, cursors.to_vec()));
        }
        self.refresh();
    }
}
