//! Tools that change or check text (JSON, XML, CSV, encoding and decoding), copying with syntax
//! colors, and the toast that reports quick results.

use std::time::Duration;

use slint::ComponentHandle;
use tuplepad_core::{Codec, CoreError, EditKind, TextPos, TextSelection};

use crate::app::App;

/// How long a toast stays up.
const TOAST: Duration = Duration::from_millis(2200);

pub(crate) fn codec(name: &str) -> Option<Codec> {
    Some(match name {
        "base64" => Codec::Base64,
        "url" => Codec::Url,
        "html" => Codec::HtmlEntities,
        "hex" => Codec::Hex,
        "quoted-printable" => Codec::QuotedPrintable,
        _ => return None,
    })
}

impl App {
    /// Runs a built-in tool command. Returns false if `id` isn't one.
    pub fn run_builtin(&self, id: &str) -> bool {
        let indent = match self.editor.borrow().indentation() {
            tuplepad_core::Indentation::Tabs => 0,
            tuplepad_core::Indentation::Spaces(width) => width,
        };
        if let Some(name) = id.strip_prefix("encode.") {
            let Some(codec) = codec(name) else { return false };
            self.apply_tool("Encode", |text| Ok(tuplepad_core::encode_text(codec, text)));
            return true;
        }
        if let Some(name) = id.strip_prefix("decode.") {
            let Some(codec) = codec(name) else { return false };
            self.apply_tool("Decode", |text| tuplepad_core::decode_text(codec, text));
            return true;
        }
        match id {
            "json.format" => self.apply_tool("Format JSON", |text| tuplepad_core::json_format(text, indent, false)),
            "json.sort-keys" => self.apply_tool("Sort Keys", |text| tuplepad_core::json_format(text, indent, true)),
            "json.minify" => self.apply_tool("Minify JSON", tuplepad_core::json_minify),
            "json.validate" => self.validate("JSON", tuplepad_core::json_validate),
            "json.copy-path" => self.copy_json_path(),
            "json.tree" => {
                self.change_settings(|s| s.show_outline = true);
            }
            "xml.format" => self.apply_tool("Format XML", |text| tuplepad_core::xml_format(text, indent.max(1))),
            "xml.linearize" => self.apply_tool("Linearize XML", tuplepad_core::xml_linearize),
            "xml.validate" => self.validate("XML", tuplepad_core::xml_validate),
            "csv.align" => self.editor.borrow_mut().apply(|buffer, cursors| buffer.csv_align(cursors.to_vec())),
            "csv.shrink" => self.editor.borrow_mut().apply(|buffer, cursors| buffer.csv_shrink(cursors.to_vec())),
            "csv.delimiter-comma" | "csv.delimiter-tab" | "csv.delimiter-semicolon" | "csv.delimiter-pipe" => {
                let delimiter = match id {
                    "csv.delimiter-tab" => "\t",
                    "csv.delimiter-semicolon" => ";",
                    "csv.delimiter-pipe" => "|",
                    _ => ",",
                };
                let mut editor = self.editor.borrow_mut();
                editor.buffer().set_csv_delimiter(delimiter.into());
                editor.invalidate();
            }
            "edit.copy-colors" => self.copy_with_colors(),
            _ => return false,
        }
        true
    }

    /// The ranges a tool works on: every selection, or the whole document when nothing's selected.
    fn tool_ranges(&self) -> Vec<TextSelection> {
        let editor = self.editor.borrow();
        let selected: Vec<TextSelection> = editor.doc().cursors.iter().filter(|c| c.anchor != c.head).map(|c| TextSelection { anchor: c.start(), head: c.end() }).collect();
        if selected.is_empty() {
            vec![TextSelection { anchor: TextPos::new(0, 0), head: editor.buffer().end_position() }]
        } else {
            selected
        }
    }

    /// Runs `transform` over each selection (or the whole document) as one undo step. An error
    /// is shown as an alert, with the caret moved to where the problem is when it has a place.
    pub fn apply_tool(&self, name: &str, transform: impl Fn(String) -> Result<String, CoreError>) {
        let ranges = self.tool_ranges();
        let buffer = self.editor.borrow().buffer();
        let mut texts = Vec::new();
        for range in &ranges {
            let before = buffer.text_in(range.start(), range.end());
            match transform(before.clone()) {
                Ok(after) => texts.push((before, after)),
                Err(error) => {
                    self.show_tool_error(&error, range.start(), &format!("{name} didn’t work"));
                    return;
                }
            }
        }
        if texts.iter().all(|(before, after)| before == after) {
            self.show_toast(&format!("{name}: nothing to change"));
            return;
        }
        let new_texts: Vec<String> = texts.into_iter().map(|(_, after)| after).collect();
        {
            let mut editor = self.editor.borrow_mut();
            editor.apply(|buffer, cursors| buffer.edit(ranges.clone(), new_texts.clone(), EditKind::Other, cursors.to_vec()));
            editor.text_replaced();
        }
        self.refresh();
    }

    fn validate(&self, kind: &str, check: impl Fn(String) -> Option<CoreError>) {
        let ranges = self.tool_ranges();
        let buffer = self.editor.borrow().buffer();
        for range in &ranges {
            if let Some(error) = check(buffer.text_in(range.start(), range.end())) {
                self.show_tool_error(&error, range.start(), &format!("This isn’t valid {kind}"));
                return;
            }
        }
        self.show_toast(&format!("Valid {kind}"));
    }

    /// An alert for a tool's error; when the error has a place, the caret goes there too.
    fn show_tool_error(&self, error: &CoreError, base: TextPos, title: &str) {
        let mut message = error.to_string();
        if let CoreError::Invalid { line, column, .. } = *error
            && line > 0
        {
            let target_line = base.line + line - 1;
            let target_column = if line == 1 { base.column } else { 0 } + column.saturating_sub(1);
            self.go_to(target_line + 1, Some(target_column + 1));
            message.push_str(&format!(" (line {}, column {})", target_line + 1, target_column + 1));
            self.refresh();
        }
        *self.last_tool_error.borrow_mut() = Some(message.clone());
        self.alert(title, &message);
    }

    fn copy_json_path(&self) {
        let (text, caret) = {
            let editor = self.editor.borrow();
            (editor.buffer().text(), editor.doc().cursors.last().map(|c| c.head).unwrap_or(TextPos::new(0, 0)))
        };
        match tuplepad_core::json_path_at(text, caret.line, caret.column) {
            Some(path) => {
                self.editor.borrow_mut().write_clipboard(path.clone());
                self.show_toast(&format!("Copied {path}"));
            }
            None => self.show_toast("No JSON value at the caret"),
        }
    }

    /// Copies the selection (or the whole document) as HTML with its syntax colors, and as plain
    /// text for places that don't take HTML.
    fn copy_with_colors(&self) {
        let ranges = self.tool_ranges();
        let (buffer, tab_width) = {
            let editor = self.editor.borrow();
            (editor.buffer(), editor.tab_width())
        };
        let html: String = ranges.iter().map(|r| crate::print::range_html(&buffer, r.start(), r.end(), tab_width)).collect();
        let plain: Vec<String> = ranges.iter().map(|r| buffer.text_in(r.start(), r.end())).collect();
        let plain = plain.join("\n");
        if self.is_scripted() {
            self.editor.borrow_mut().write_clipboard(plain);
        } else if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_html(html.clone(), Some(plain));
        }
        *self.last_copied_html.borrow_mut() = Some(html);
        self.show_toast("Copied with syntax colors");
    }

    /// A short message at the bottom of the window that goes away by itself.
    pub fn show_toast(&self, text: &str) {
        let Some(ui) = self.ui.upgrade() else { return };
        ui.set_toast(text.into());
        ui.set_toast_shown(true);
        *self.last_toast.borrow_mut() = Some(text.to_string());
        let weak = ui.as_weak();
        self.toast_timer.start(slint::TimerMode::SingleShot, TOAST, move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_toast_shown(false);
            }
        });
    }
}
