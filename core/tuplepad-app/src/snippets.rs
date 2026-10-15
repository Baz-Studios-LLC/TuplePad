//! Snippets: a trigger word then Tab expands a body with tab stops (`$1`, `${1:text}`, `$0`), and
//! Tab and Shift-Tab move between the stops. Insert Snippet… lists them in the command palette.
//! The user's snippets are saved as `snippets.json` in the data folder (not by scripted runs).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tuplepad_core::{EditKind, TextPos, TextSelection};

use crate::app::App;
use crate::palette::Entry;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub trigger: String,
    pub name: String,
    /// A language id like "rust", or empty for every document.
    pub language: String,
    pub body: String,
}

fn snippet(trigger: &str, name: &str, language: &str, body: &str) -> Snippet {
    Snippet { trigger: trigger.into(), name: name.into(), language: language.into(), body: body.into() }
}

pub fn examples() -> Vec<Snippet> {
    vec![
        snippet("fn", "Function", "rust", "fn ${1:name}($2) {\n\t$0\n}"),
        snippet("test", "Test", "rust", "#[test]\nfn ${1:name}() {\n\t$0\n}"),
        snippet("def", "Function", "python", "def ${1:name}($2):\n\t${0:pass}"),
        snippet("func", "Function", "swift", "func ${1:name}($2) {\n\t$0\n}"),
        snippet("function", "Function", "javascript", "function ${1:name}($2) {\n\t$0\n}"),
        snippet("link", "Link", "markdown", "[${1:text}](${2:https://})$0"),
        snippet("code", "Code block", "markdown", "```${1:language}\n$0\n```"),
        snippet("todo", "To-do note", "", "TODO: $0"),
    ]
}

pub struct SnippetStore {
    pub snippets: Vec<Snippet>,
    path: Option<PathBuf>,
}

impl SnippetStore {
    /// The saved snippets, or the examples. Without a path (scripted runs) nothing is saved.
    pub fn load(path: Option<PathBuf>) -> Self {
        let saved = path.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|data| serde_json::from_slice(&data).ok());
        SnippetStore { snippets: saved.unwrap_or_else(examples), path }
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        if let Ok(data) = serde_json::to_vec_pretty(&self.snippets) {
            let _ = std::fs::write(path, data);
        }
    }

    /// The snippet a trigger expands to in a language; one for that language wins over one for any.
    pub fn find(&self, trigger: &str, language: Option<&str>) -> Option<&Snippet> {
        let matches: Vec<&Snippet> = self.snippets.iter().filter(|s| s.trigger == trigger && (s.language.is_empty() || Some(s.language.as_str()) == language)).collect();
        matches.iter().find(|s| !s.language.is_empty()).or(matches.first()).copied()
    }

    pub fn available(&self, language: Option<&str>) -> Vec<(usize, &Snippet)> {
        self.snippets.iter().enumerate().filter(|(_, s)| s.language.is_empty() || Some(s.language.as_str()) == language).collect()
    }
}

/// A snippet being filled in: where each stop is, and which is current.
pub struct SnippetSession {
    doc: String,
    stops: Vec<Vec<TextSelection>>,
    index: usize,
    line_count: u64,
}

impl App {
    /// Tab (or Shift-Tab with `back`) for snippets: moves between a snippet's stops, or expands
    /// the trigger word before the caret. Returns whether it used the key.
    pub fn snippet_tab(&self, back: bool) -> bool {
        if self.advance_snippet(if back { -1 } else { 1 }) {
            return true;
        }
        if back {
            return false;
        }
        let found = {
            let editor = self.editor.borrow();
            let doc = editor.doc();
            match doc.cursors.as_slice() {
                [caret] if caret.anchor == caret.head => {
                    let line = doc.buffer.line(caret.head.line);
                    tuplepad_core::snippet_trigger(&line, caret.head.column).and_then(|(start, trigger)| {
                        let language = doc.buffer.language();
                        let store = self.snippets.borrow();
                        store.find(&trigger, language.as_deref()).cloned().map(|s| (s, TextSelection { anchor: TextPos::new(caret.head.line, start), head: caret.head }))
                    })
                }
                _ => None,
            }
        };
        match found {
            Some((snippet, range)) => {
                self.insert_snippet(&snippet, Some(range));
                true
            }
            None => false,
        }
    }

    /// Inserts a snippet at the caret (or in place of `range`) and selects its first stop.
    pub fn insert_snippet(&self, snippet: &Snippet, range: Option<TextSelection>) {
        {
            let mut editor = self.editor.borrow_mut();
            let Some(caret) = editor.doc().cursors.last().copied() else { return };
            let target = range.unwrap_or(TextSelection { anchor: caret.start(), head: caret.end() });
            let line = editor.buffer().line(target.start().line);
            let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            let unit = editor.indentation().unit();
            let expansion = tuplepad_core::expand_snippet(snippet.body.clone(), indent, unit);
            let text = expansion.text.clone();
            editor.apply(|buffer, cursors| buffer.edit(vec![target], vec![text.clone()], EditKind::Other, cursors.to_vec()));
            let base = target.start();
            let stops: Vec<Vec<TextSelection>> = expansion
                .stops
                .iter()
                .map(|stop| {
                    stop.ranges
                        .iter()
                        .map(|r| {
                            let line = base.line + r.line as u64;
                            let offset = if r.line == 0 { base.column } else { 0 };
                            TextSelection { anchor: TextPos::new(line, offset + r.start as u64), head: TextPos::new(line, offset + r.end as u64) }
                        })
                        .collect()
                })
                .collect();
            if let Some(first) = stops.first() {
                editor.doc_mut().cursors = tuplepad_core::normalize_cursors(first.clone());
                editor.reveal();
            }
            *self.snippet_session.borrow_mut() = (stops.len() > 1).then(|| SnippetSession {
                doc: editor.doc().id.clone(),
                stops,
                index: 0,
                line_count: editor.buffer().line_count(),
            });
        }
        self.refresh();
    }

    /// Moves to the next (or previous) stop of the snippet being filled in. Returns false when
    /// there's none (and stops helping once the snippet has been left behind).
    fn advance_snippet(&self, delta: i32) -> bool {
        let mut session = self.snippet_session.borrow_mut();
        let Some(current) = session.as_mut() else { return false };
        let mut editor = self.editor.borrow_mut();
        // Another document, or a line break typed in a stop: the stops are no longer reliable.
        if editor.doc().id != current.doc || editor.buffer().line_count() != current.line_count {
            *session = None;
            return false;
        }
        let carets = editor.doc().cursors.clone();
        current.stops = tuplepad_core::shift_snippet_stops(std::mem::take(&mut current.stops), current.index, &carets);
        let next = current.index as i64 + delta as i64;
        if next < 0 {
            return true;
        }
        let next = next as usize;
        if next >= current.stops.len() {
            *session = None;
            return false;
        }
        current.index = next;
        let stop = current.stops[next].clone();
        let last = next + 1 == current.stops.len();
        editor.doc_mut().cursors = tuplepad_core::normalize_cursors(stop);
        editor.reveal();
        drop(editor);
        if last {
            *session = None;
        }
        drop(session);
        self.refresh();
        true
    }

    /// Insert Snippet…: the snippets for this document's language, in the command palette.
    pub fn open_snippet_list(&self) {
        let language = self.editor.borrow().buffer().language();
        let entries: Vec<Entry> = self
            .snippets
            .borrow()
            .available(language.as_deref())
            .into_iter()
            .map(|(i, s)| Entry {
                id: format!("snippet:{i}"),
                title: s.name.clone(),
                detail: s.body.lines().next().unwrap_or("").to_string(),
                trailing: s.trigger.clone(),
            })
            .collect();
        self.open_palette_list(entries, "Insert a snippet (or type its trigger and press Tab)");
    }

    /// Paste from History…: text copied or cut in TuplePad, newest first.
    pub fn open_clipboard_history(&self) {
        let entries: Vec<Entry> = self
            .editor
            .borrow()
            .clip_history
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let first: String = text.trim().lines().next().unwrap_or("").chars().take(90).collect();
                let lines = text.lines().count();
                Entry {
                    id: format!("clip:{i}"),
                    title: if first.is_empty() { "(spaces)".into() } else { first },
                    detail: if lines > 1 { format!("{lines} lines") } else { format!("{} characters", text.chars().count()) },
                    trailing: String::new(),
                }
            })
            .collect();
        self.open_palette_list(entries, "Paste something you copied earlier");
    }

    /// A palette item from Insert Snippet… or Paste from History….
    pub fn run_list_item(&self, kind: &str, value: &str) -> bool {
        let Ok(index) = value.parse::<usize>() else { return false };
        match kind {
            "snippet" => {
                let snippet = self.snippets.borrow().snippets.get(index).cloned();
                if let Some(snippet) = snippet {
                    self.insert_snippet(&snippet, None);
                }
            }
            "clip" => {
                let text = self.editor.borrow().clip_history.get(index).cloned();
                if let Some(text) = text {
                    self.editor.borrow_mut().paste_text(&text);
                    self.refresh();
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_snippets_win() {
        let mut store = SnippetStore { snippets: examples(), path: None };
        store.snippets.push(snippet("fn", "Any", "", "x"));
        assert_eq!(store.find("fn", Some("rust")).map(|s| s.name.as_str()), Some("Function"));
        assert_eq!(store.find("fn", None).map(|s| s.name.as_str()), Some("Any"));
        assert_eq!(store.find("def", Some("rust")), None);
        assert!(store.available(Some("python")).iter().any(|(_, s)| s.trigger == "todo"));
    }
}
