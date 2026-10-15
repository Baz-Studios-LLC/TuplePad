//! The editor's context menu: for a misspelled word, its suggestions and Learn Spelling and
//! Ignore Spelling; then Cut, Copy, Paste and Select All.

use std::collections::HashSet;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};
use tuplepad_core::{EditKind, TextSelection};

use crate::app::App;
use crate::{ContextItem, EditorBridge};

/// Suggestions offered for a misspelled word.
const SUGGESTIONS: usize = 6;

/// The word a context menu was opened on, and its suggestions.
#[derive(Default)]
pub struct ContextWord {
    range: Option<TextSelection>,
    word: String,
    suggestions: Vec<String>,
}

fn item(id: &str, title: &str, enabled: bool) -> ContextItem {
    ContextItem { id: id.into(), title: title.into(), enabled }
}

fn separator() -> ContextItem {
    ContextItem { id: "".into(), title: "\u{E001}⸺".into(), enabled: true }
}

impl App {
    /// A right-click in the text: the caret moves there (unless it's in the selection), and the
    /// menu is filled in for what's under the pointer.
    pub fn context_requested(&self, pane: usize, x: f32, y: f32) {
        self.focus_pane(pane);
        let (context, has_selection) = {
            let mut editor = self.editor.borrow_mut();
            let pos = editor.hit(x, y);
            let in_selection = editor.doc().cursors.iter().any(|c| c.anchor != c.head && c.start() <= pos && pos <= c.end());
            if !in_selection {
                editor.doc_mut().cursors = vec![TextSelection::caret(pos)];
                editor.touch();
            }
            let has_selection = editor.doc().cursors.iter().any(|c| c.anchor != c.head);
            let buffer = editor.buffer();
            let mut context = ContextWord::default();
            if editor.options.spelling && tuplepad_core::spelling_ready()
                && let Some(range) = buffer.word_at(pos)
            {
                let word = buffer.text_in(range.start(), range.end());
                let misspelled = buffer.misspellings(range.start().line, 1, &editor.spelling_words).iter().any(|w| w.start() == range.start() && w.end() == range.end());
                if misspelled {
                    context = ContextWord { range: Some(range), suggestions: tuplepad_core::spelling_suggestions(&word, SUGGESTIONS), word };
                }
            }
            (context, has_selection)
        };
        let mut items = Vec::new();
        if context.range.is_some() {
            if context.suggestions.is_empty() {
                items.push(item("", "No Suggestions", false));
            }
            for (i, suggestion) in context.suggestions.iter().enumerate() {
                items.push(item(&format!("spell-replace:{i}"), suggestion, true));
            }
            items.push(separator());
            items.push(item("spell-learn", "Learn Spelling", true));
            items.push(item("spell-ignore", "Ignore Spelling", true));
            items.push(separator());
        }
        items.push(item("edit.cut", "Cut", has_selection));
        items.push(item("edit.copy", "Copy", true));
        items.push(item("edit.paste", "Paste", true));
        items.push(separator());
        items.push(item("edit.select-all", "Select All", true));
        *self.context_word.borrow_mut() = context;
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<EditorBridge>().set_context_items(ModelRc::from(Rc::new(VecModel::from(items))));
        }
        self.refresh();
    }

    pub fn context_chosen(&self, id: &str) {
        let (range, word, suggestions) = {
            let context = self.context_word.borrow();
            (context.range, context.word.clone(), context.suggestions.clone())
        };
        if let Some(index) = id.strip_prefix("spell-replace:") {
            let (Some(range), Some(text)) = (range, index.parse::<usize>().ok().and_then(|i| suggestions.get(i).cloned())) else { return };
            self.editor.borrow_mut().apply(|buffer, cursors| buffer.edit(vec![range], vec![text.clone()], EditKind::Other, cursors.to_vec()));
            self.refresh();
            return;
        }
        match id {
            "spell-learn" => {
                self.settings.borrow_mut().spelling_words.push(word);
                self.apply_settings(true);
                self.refresh();
            }
            "spell-ignore" => {
                self.ignored_words.borrow_mut().insert(word);
                self.apply_settings(false);
                self.refresh();
            }
            "" => {}
            other => self.run_command(other),
        }
    }

    /// Words the spell checker accepts: those taught to it, and those ignored this session.
    pub fn accepted_words(&self) -> HashSet<String> {
        let mut words: HashSet<String> = self.settings.borrow().spelling_words.iter().cloned().collect();
        words.extend(self.ignored_words.borrow().iter().cloned());
        words
    }
}
