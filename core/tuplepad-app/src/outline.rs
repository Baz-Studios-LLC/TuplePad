//! The outline panel: the document's functions, types and headings from its syntax tree (for
//! JSON, its whole tree of keys and values), worked out on a background thread whenever the
//! document changes, with the item around the caret highlighted. A click goes to the item.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};
use tuplepad_core::{JsonKind, JsonTreeItem, OutlineItem, SymbolKind, TextPos, TextSelection, TextBuffer};

use crate::app::App;
use crate::{OutlineBridge, OutlineRow};

#[derive(Default)]
pub struct OutlineState {
    /// The document and version the items are for.
    key: Option<(String, u64)>,
    building: Option<(String, u64)>,
    items: Vec<OutlineItem>,
    current: i32,
}

/// An item ready for the panel: where it is, its badge and (for JSON) its value.
pub struct Entry {
    item: OutlineItem,
    kind: i32,
    detail: String,
}

/// JSON nodes that are too many for the panel are left out.
const JSON_LIMIT: u32 = 5_000;

fn json_kind(kind: JsonKind) -> i32 {
    match kind {
        JsonKind::Object => 7,
        JsonKind::Array => 8,
        JsonKind::String => 9,
        JsonKind::Number => 10,
        JsonKind::Bool => 11,
        JsonKind::Null => 12,
    }
}

/// A JSON document's tree as outline entries: each node's depth from its parents, and its end
/// where the next node at its depth or shallower begins.
fn json_entries(nodes: Vec<JsonTreeItem>, line_count: u64) -> Vec<Entry> {
    let mut depths: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut entries: Vec<Entry> = Vec::new();
    for node in nodes {
        let depth = node.parent.and_then(|p| depths.get(&p).map(|d| d + 1)).unwrap_or(0);
        depths.insert(node.id, depth);
        // The root object or array isn't worth a row of its own.
        if node.parent.is_none() && node.key.is_empty() {
            depths.insert(node.id, 0);
            continue;
        }
        let depth = depth.saturating_sub(1);
        entries.push(Entry {
            item: OutlineItem { name: node.key.clone(), kind: SymbolKind::Property, line: node.line, column: node.column, end_line: line_count, depth },
            kind: json_kind(node.kind),
            detail: node.preview,
        });
    }
    for i in 0..entries.len() {
        let depth = entries[i].item.depth;
        if let Some(next) = entries[i + 1..].iter().find(|e| e.item.depth <= depth) {
            entries[i].item.end_line = next.item.line.saturating_sub(1).max(entries[i].item.line);
        }
    }
    entries
}

/// The outline of a document, on a background thread.
fn build(buffer: &TextBuffer) -> Vec<Entry> {
    if buffer.language().as_deref() == Some("json") {
        return json_entries(tuplepad_core::json_tree(buffer.text(), JSON_LIMIT), buffer.line_count());
    }
    buffer.outline().into_iter().map(|item| Entry { kind: kind_index(item.kind), detail: String::new(), item }).collect()
}

fn kind_index(kind: SymbolKind) -> i32 {
    match kind {
        SymbolKind::Function => 0,
        SymbolKind::Method => 1,
        SymbolKind::Type => 2,
        SymbolKind::Namespace => 3,
        SymbolKind::Heading => 4,
        SymbolKind::Constant => 5,
        SymbolKind::Property => 6,
    }
}

/// The innermost item around a line.
fn item_at(items: &[OutlineItem], line: u64) -> Option<usize> {
    items.iter().enumerate().filter(|(_, item)| item.line <= line && line <= item.end_line.max(item.line)).max_by_key(|(_, item)| (item.depth, item.line)).map(|(i, _)| i)
}

impl App {
    /// Keeps the outline up to date while it's showing (from the 60 ms tick).
    pub fn refresh_outline(&self) {
        if !self.settings.borrow().show_outline {
            return;
        }
        let Ok(editor) = self.editor.try_borrow() else { return };
        let doc = editor.doc();
        let key = (doc.id.clone(), doc.buffer.version());
        let caret_line = doc.cursors.last().map_or(0, |c| c.head.line);
        let buffer = doc.buffer.clone();
        let has_language = buffer.language().is_some();
        drop(editor);
        let mut state = self.outline.borrow_mut();
        if state.key.as_ref() != Some(&key) && state.building.as_ref() != Some(&key) {
            state.building = Some(key.clone());
            std::thread::spawn(move || {
                let entries = build(&buffer);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = crate::platform::app() {
                        app.outline_built(key, entries, has_language);
                    }
                });
            });
        }
        let current = item_at(&state.items, caret_line).map_or(-1, |i| i as i32);
        if current != state.current && state.key.is_some() {
            state.current = current;
            if let Some(ui) = self.ui.upgrade() {
                ui.global::<OutlineBridge>().set_current(current);
            }
        }
    }

    fn outline_built(&self, key: (String, u64), entries: Vec<Entry>, has_language: bool) {
        let rows: Vec<OutlineRow> = entries
            .iter()
            .map(|e| OutlineRow { name: e.item.name.clone().into(), kind: e.kind, depth: e.item.depth as i32, detail: e.detail.clone().into() })
            .collect();
        let items: Vec<OutlineItem> = entries.into_iter().map(|e| e.item).collect();
        {
            let mut state = self.outline.borrow_mut();
            if state.building.as_ref() == Some(&key) {
                state.building = None;
            }
            state.key = Some(key);
            state.items = items;
            state.current = -2;
        }
        if let Some(ui) = self.ui.upgrade() {
            let bridge = ui.global::<OutlineBridge>();
            bridge.set_items(ModelRc::from(Rc::new(VecModel::from(rows))));
            bridge.set_empty_text(if has_language { "Nothing to outline in this document." } else { "The outline shows functions, types and headings in code and Markdown." }.into());
        }
        self.refresh_outline();
    }

    /// A click on an item: its name, selected, in view.
    pub fn activate_outline_item(&self, index: usize) {
        let item = self.outline.borrow().items.get(index).cloned();
        let Some(item) = item else { return };
        {
            let mut editor = self.editor.borrow_mut();
            let start = TextPos::new(item.line, item.column);
            let end = TextPos::new(item.line, item.column + item.name.encode_utf16().count() as u64);
            let buffer = editor.buffer();
            editor.doc_mut().cursors = vec![TextSelection { anchor: buffer.clamp(start), head: buffer.clamp(end) }];
            editor.reveal_centered();
            editor.touch();
        }
        self.refresh();
        if let Some(ui) = self.ui.upgrade() {
            ui.invoke_focus_editor();
        }
    }

    /// For scripted runs.
    pub fn outline_dump(&self) -> serde_json::Value {
        let state = self.outline.borrow();
        let items: Vec<serde_json::Value> = state.items.iter().map(|i| serde_json::json!([i.name, kind_index(i.kind), i.depth, i.line])).collect();
        let _ = &items;
        serde_json::json!({"items": items, "current": state.current})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, line: u64, end_line: u64, depth: u32) -> OutlineItem {
        OutlineItem { name: name.into(), kind: SymbolKind::Function, line, column: 0, end_line, depth }
    }

    #[test]
    fn innermost_item_around_a_line() {
        let items = vec![item("Outer", 0, 10, 0), item("inner", 2, 4, 1), item("after", 12, 14, 0)];
        assert_eq!(item_at(&items, 3), Some(1));
        assert_eq!(item_at(&items, 6), Some(0));
        assert_eq!(item_at(&items, 11), None);
        assert_eq!(item_at(&items, 12), Some(2));
    }
}
