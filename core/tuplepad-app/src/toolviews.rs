//! Views that take the editor's place for a while: the hex view (a file's bytes, read a page at
//! a time as the list asks for them) and Compare (a document side by side with its saved
//! version, the clipboard or another document).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, ModelTracker, VecModel};
use tuplepad_core::{DiffKind, DiffOptions, DiffResult, MatchSpan};

use crate::app::App;
use crate::{CompareLine, HexLine, TitlePart, ToolViewBridge};

/// Rows read from the file at a time.
const PAGE: usize = 256;
/// Characters of a line shown in Compare.
const SHOWN: usize = 400;

/// A file's rows of bytes, read as they're shown.
struct HexModel {
    path: String,
    rows: usize,
    pages: RefCell<HashMap<usize, Rc<Vec<HexLine>>>>,
}

impl Model for HexModel {
    type Data = HexLine;

    fn row_count(&self) -> usize {
        self.rows
    }

    fn row_data(&self, row: usize) -> Option<HexLine> {
        if row >= self.rows {
            return None;
        }
        let page = row / PAGE;
        let lines = {
            let mut pages = self.pages.borrow_mut();
            if pages.len() > 64 {
                pages.clear();
            }
            pages
                .entry(page)
                .or_insert_with(|| {
                    let rows = tuplepad_core::hex_rows(self.path.clone(), (page * PAGE) as u64, PAGE as u64).unwrap_or_default();
                    Rc::new(rows.into_iter().map(|r| HexLine { offset: format!("{:08x}", r.offset).into(), hex: r.hex.into(), ascii: r.ascii.into() }).collect())
                })
                .clone()
        };
        lines.get(row % PAGE).cloned()
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &()
    }
}

/// What a document is compared with.
#[derive(Clone)]
pub enum CompareWith {
    Saved,
    Clipboard,
    Document(usize),
}

#[derive(Default)]
pub struct CompareState {
    left: String,
    right: String,
    changes: Vec<u64>,
    current: Option<usize>,
}

/// A line's text split into parts, the changed ones marked.
fn diff_parts(text: &str, spans: &[MatchSpan]) -> Vec<TitlePart> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut parts = Vec::new();
    let mut column = 0usize;
    let piece = |from: usize, to: usize| String::from_utf16_lossy(&units[from.min(units.len())..to.min(units.len())]).replace('\t', "    ");
    for span in spans {
        let (start, end) = (span.start as usize, span.end as usize);
        if start > column {
            parts.push(TitlePart { text: piece(column, start).into(), matched: false });
        }
        if end > start {
            parts.push(TitlePart { text: piece(start, end).into(), matched: true });
        }
        column = end.max(column);
    }
    if column < units.len() {
        parts.push(TitlePart { text: piece(column, units.len()).into(), matched: false });
    }
    // Very long lines are cut short.
    let mut shown = 0;
    let mut kept = Vec::new();
    for mut part in parts {
        let count = part.text.chars().count();
        if shown + count > SHOWN {
            part.text = part.text.chars().take(SHOWN - shown).chain(std::iter::once('…')).collect::<String>().into();
            kept.push(part);
            break;
        }
        shown += count;
        kept.push(part);
    }
    kept
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{} {many}", crate::editor::group(n)) }
}

impl App {
    fn tool_view<T>(&self, f: impl FnOnce(&ToolViewBridge) -> T) -> Option<T> {
        self.ui.upgrade().map(|ui| f(&ui.global::<ToolViewBridge>()))
    }

    pub fn close_tool_view(&self) {
        self.tool_view(|b| {
            b.set_view(0);
            b.set_hex_lines(ModelRc::default());
            b.set_compare_lines(ModelRc::default());
        });
        if let Some(ui) = self.ui.upgrade() {
            ui.invoke_focus_editor();
        }
    }

    // MARK: Hex

    /// Hex View: the document's file, byte by byte.
    pub fn show_hex(&self) {
        let (path, name) = {
            let editor = self.editor.borrow();
            (editor.doc().path(), editor.doc().name())
        };
        let Some(path) = path else { return };
        let text = path.to_string_lossy().into_owned();
        let size = match tuplepad_core::file_size(text.clone()) {
            Ok(size) => size,
            Err(error) => {
                self.alert(&format!("TuplePad couldn't read “{name}”."), &error.to_string());
                return;
            }
        };
        let rows = size.div_ceil(16) as usize;
        let model = Rc::new(HexModel { path: text, rows, pages: RefCell::new(HashMap::new()) });
        let modified = self.editor.borrow().buffer().is_modified();
        let subtitle = format!(
            "{} · {}{}",
            crate::app::format_size(size),
            plural(size, "byte", "bytes"),
            if modified { " · the file as saved, without your unsaved changes" } else { "" }
        );
        self.tool_view(|b| {
            b.set_hex_lines(ModelRc::from(model));
            b.set_title(format!("{name} — Hex").into());
            b.set_subtitle(subtitle.into());
            b.set_view(1);
        });
    }

    // MARK: Compare

    pub fn compare(&self, with: CompareWith) {
        let (this_name, this_text, path) = {
            let editor = self.editor.borrow();
            (editor.doc().name(), editor.buffer().text(), editor.doc().path())
        };
        let (left_title, left, right_title, right) = match with {
            CompareWith::Saved => {
                let Some(path) = path else { return };
                let saved = match tuplepad_core::TextBuffer::open(path.to_string_lossy().into_owned()) {
                    Ok(buffer) => buffer.text(),
                    Err(error) => {
                        self.alert(&format!("TuplePad couldn't read “{this_name}”."), &error.to_string());
                        return;
                    }
                };
                (format!("{this_name} (saved)"), saved, format!("{this_name} (now)"), this_text)
            }
            CompareWith::Clipboard => {
                let clip = self.editor.borrow_mut().clipboard_text().unwrap_or_default();
                (this_name.clone(), this_text, "The clipboard".to_string(), clip)
            }
            CompareWith::Document(index) => {
                let other = self.editor.borrow().docs.get(index).map(|d| (d.name(), d.buffer.text()));
                let Some((other_name, other_text)) = other else { return };
                (this_name.clone(), this_text, other_name, other_text)
            }
        };
        *self.compare_state.borrow_mut() = CompareState { left, right, changes: Vec::new(), current: None };
        self.tool_view(|b| {
            b.set_left_title(left_title.clone().into());
            b.set_right_title(right_title.clone().into());
            b.set_title(format!("{left_title} compared with {right_title}").into());
        });
        self.recompare();
        self.tool_view(|b| b.set_view(2));
        self.step_change(true);
    }

    /// Works out the differences again (after the options change).
    pub fn recompare(&self) {
        let Some(options) = self.tool_view(|b| DiffOptions { ignore_whitespace: b.get_ignore_whitespace(), ignore_case: b.get_ignore_case() }) else { return };
        let (left, right) = {
            let state = self.compare_state.borrow();
            (state.left.clone(), state.right.clone())
        };
        let result: DiffResult = tuplepad_core::diff_texts(left, right, options);
        let number = |line: Option<u64>| line.map(|l| crate::editor::group(l + 1)).unwrap_or_default();
        let lines: Vec<CompareLine> = result
            .rows
            .iter()
            .map(|row| CompareLine {
                kind: match row.kind {
                    DiffKind::Same => 0,
                    DiffKind::Changed => 1,
                    DiffKind::Added => 2,
                    DiffKind::Removed => 3,
                },
                left_number: number(row.left_line).into(),
                right_number: number(row.right_line).into(),
                left: ModelRc::new(VecModel::from(diff_parts(&row.left_text, &row.left_spans))),
                right: ModelRc::new(VecModel::from(diff_parts(&row.right_text, &row.right_spans))),
            })
            .collect();
        let summary = if result.changes.is_empty() {
            "No differences".to_string()
        } else {
            let mut parts = Vec::new();
            if result.changed > 0 {
                parts.push(plural(result.changed, "changed line", "changed lines"));
            }
            if result.added > 0 {
                parts.push(format!("{} added", plural(result.added, "line", "lines")));
            }
            if result.removed > 0 {
                parts.push(format!("{} removed", plural(result.removed, "line", "lines")));
            }
            format!("{}: {}", plural(result.changes.len() as u64, "change", "changes"), parts.join(", "))
        };
        {
            let mut state = self.compare_state.borrow_mut();
            state.changes = result.changes.clone();
            state.current = None;
        }
        self.tool_view(|b| {
            b.set_compare_lines(ModelRc::from(Rc::new(VecModel::from(lines))));
            b.set_summary(summary.into());
            b.set_change_row(-1);
        });
    }

    /// Next Change and Previous Change, wrapping around.
    pub fn step_change(&self, forward: bool) {
        let row = {
            let mut state = self.compare_state.borrow_mut();
            if state.changes.is_empty() {
                return;
            }
            let count = state.changes.len();
            let next = match (state.current, forward) {
                (None, true) => 0,
                (None, false) => count - 1,
                (Some(i), true) => (i + 1) % count,
                (Some(i), false) => (i + count - 1) % count,
            };
            state.current = Some(next);
            state.changes[next] as i32
        };
        self.tool_view(|b| {
            b.set_change_row(row);
            b.set_scroll_request(b.get_scroll_request() + 1);
        });
    }

    /// For scripted runs.
    pub fn tool_view_dump(&self) -> serde_json::Value {
        let Some(state) = self.tool_view(|b| {
            let text = |parts: ModelRc<TitlePart>| parts.iter().map(|p| p.text.to_string()).collect::<String>();
            let marked = |parts: ModelRc<TitlePart>| parts.iter().filter(|p| p.matched).map(|p| p.text.to_string()).collect::<Vec<_>>();
            let compare: Vec<serde_json::Value> = b
                .get_compare_lines()
                .iter()
                .map(|l| serde_json::json!([l.kind, l.left_number.as_str(), text(l.left.clone()), l.right_number.as_str(), text(l.right.clone()), marked(l.left), marked(l.right)]))
                .collect();
            let hex = b.get_hex_lines();
            let first_hex: Vec<serde_json::Value> = (0..hex.row_count().min(3)).filter_map(|i| hex.row_data(i)).map(|l| serde_json::json!([l.offset.as_str(), l.hex.as_str(), l.ascii.as_str()])).collect();
            serde_json::json!({
                "view": b.get_view(),
                "title": b.get_title().as_str(),
                "subtitle": b.get_subtitle().as_str(),
                "summary": b.get_summary().as_str(),
                "change_row": b.get_change_row(),
                "compare": compare,
                "hex_rows": hex.row_count(),
                "hex": first_hex,
            })
        }) else {
            return serde_json::Value::Null;
        };
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_mark_the_changes() {
        let parts = diff_parts("let a = 1;", &[MatchSpan { start: 8, end: 9 }]);
        let runs: Vec<(String, bool)> = parts.iter().map(|p| (p.text.to_string(), p.matched)).collect();
        assert_eq!(runs, vec![("let a = ".into(), false), ("1".into(), true), (";".into(), false)]);
    }
}
