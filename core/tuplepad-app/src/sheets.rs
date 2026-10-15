//! The tools that ask first or show a result in a sheet: Hashes, XPath Query, Sort CSV by
//! Column, and Run Shell Command (whose output replaces the text, follows it, or opens as a new
//! document).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tuplepad_core::{EditKind, HashAlgorithm, TextPos, TextSelection};

use crate::app::App;
use crate::{HashRow, ToolBridge, XPathRow};

const ALGORITHMS: [(HashAlgorithm, &str); 4] =
    [(HashAlgorithm::Md5, "MD5"), (HashAlgorithm::Sha1, "SHA-1"), (HashAlgorithm::Sha256, "SHA-256"), (HashAlgorithm::Sha512, "SHA-512")];

/// Sheets that are open.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sheet {
    Hashes = 1,
    XPath = 2,
    CsvSort = 3,
    Shell = 4,
}

impl App {
    fn tool_bridge<T>(&self, f: impl FnOnce(&ToolBridge) -> T) -> Option<T> {
        self.ui.upgrade().map(|ui| f(&ui.global::<ToolBridge>()))
    }

    fn open_sheet(&self, sheet: Sheet) {
        self.tool_bridge(|b| b.set_sheet(sheet as i32));
    }

    pub fn close_sheet(&self) {
        self.tool_bridge(|b| b.set_sheet(0));
        if let Some(ui) = self.ui.upgrade() {
            ui.invoke_focus_editor();
        }
    }

    /// Runs a command that opens a sheet. Returns false if `id` isn't one.
    pub fn run_sheet_command(&self, id: &str) -> bool {
        match id {
            "tools.hashes" => self.open_hashes(),
            "xml.xpath" => {
                self.tool_bridge(|b| {
                    b.set_xpath_results(ModelRc::default());
                    b.set_xpath_status(SharedString::default());
                });
                self.open_sheet(Sheet::XPath);
            }
            "csv.sort" => self.open_csv_sort(),
            "tools.shell" => self.open_shell(),
            _ => return false,
        }
        true
    }

    /// The selection (or the document when nothing's selected), and which it is.
    fn selection_or_document(&self) -> (TextSelection, String, bool) {
        let editor = self.editor.borrow();
        let buffer = editor.buffer();
        match editor.doc().cursors.last().filter(|c| c.anchor != c.head) {
            Some(c) => {
                let range = TextSelection { anchor: c.start(), head: c.end() };
                (range, buffer.text_in(range.start(), range.end()), true)
            }
            None => {
                let range = TextSelection { anchor: TextPos::new(0, 0), head: buffer.end_position() };
                (range, buffer.text(), false)
            }
        }
    }

    // MARK: Hashes

    fn open_hashes(&self) {
        let (_, text, selected) = self.selection_or_document();
        let source = if selected { "Of the selection, as UTF-8".to_string() } else { format!("Of “{}”, as UTF-8", self.editor.borrow().doc().name()) };
        let rows: Vec<(String, String)> = ALGORITHMS.iter().map(|(algorithm, name)| (name.to_string(), tuplepad_core::hash_text(*algorithm, text.clone()))).collect();
        self.show_hashes(&source, rows);
        self.open_sheet(Sheet::Hashes);
    }

    fn show_hashes(&self, source: &str, rows: Vec<(String, String)>) {
        let check = self.tool_bridge(|b| b.get_hash_check().trim().to_lowercase()).unwrap_or_default();
        let rows: Vec<HashRow> =
            rows.into_iter().map(|(name, value)| HashRow { matches: !check.is_empty() && value == check, name: name.into(), value: value.into() }).collect();
        self.tool_bridge(|b| {
            b.set_hash_source(source.into());
            b.set_hashes(ModelRc::from(Rc::new(VecModel::from(rows))));
        });
    }

    pub fn hash_check_changed(&self) {
        let Some((source, rows)) = self.tool_bridge(|b| {
            use slint::Model;
            (b.get_hash_source().to_string(), b.get_hashes().iter().map(|r| (r.name.to_string(), r.value.to_string())).collect::<Vec<_>>())
        }) else {
            return;
        };
        self.show_hashes(&source, rows);
    }

    /// Hashes a file chosen from disk, on a background thread (files can be big).
    pub fn hash_file(&self) {
        let path = if self.is_scripted() { self.next_scripted_folder() } else { rfd::FileDialog::new().set_title("Choose a file to hash").pick_file() };
        let Some(path) = path else { return };
        self.hash_path(path);
    }

    pub fn hash_path(&self, path: PathBuf) {
        self.tool_bridge(|b| b.set_hashing(true));
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        std::thread::spawn(move || {
            let text_path = path.to_string_lossy().into_owned();
            let rows: Result<Vec<(String, String)>, String> = ALGORITHMS
                .iter()
                .map(|(algorithm, label)| tuplepad_core::hash_file(*algorithm, text_path.clone()).map(|h| (label.to_string(), h)).map_err(|e| e.to_string()))
                .collect();
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = crate::platform::app() else { return };
                app.tool_bridge(|b| b.set_hashing(false));
                match rows {
                    Ok(rows) => app.show_hashes(&format!("Of “{name}” ({})", crate::app::format_size(size)), rows),
                    Err(error) => app.alert(&format!("TuplePad couldn't read “{name}”."), &error),
                }
            });
        });
    }

    // MARK: XPath

    pub fn run_xpath(&self) {
        let expression = self.tool_bridge(|b| b.get_xpath().to_string()).unwrap_or_default();
        let text = self.editor.borrow().buffer().text();
        let (rows, status, error) = match tuplepad_core::xml_xpath(text, expression) {
            Ok(items) => {
                let status = match items.len() {
                    0 => "Nothing matches.".to_string(),
                    1 => "1 result. Click one to copy it.".to_string(),
                    n => format!("{} results. Click one to copy it.", crate::editor::group(n as u64)),
                };
                let rows: Vec<XPathRow> = items.into_iter().map(|i| XPathRow { kind: i.kind.into(), name: i.name.into(), value: i.value.replace('\n', " ").into() }).collect();
                (rows, status, false)
            }
            Err(error) => (Vec::new(), error.to_string(), true),
        };
        *self.last_toast.borrow_mut() = Some(status.clone());
        self.tool_bridge(|b| {
            b.set_xpath_results(ModelRc::from(Rc::new(VecModel::from(rows))));
            b.set_xpath_status(status.into());
            b.set_xpath_error(error);
        });
    }

    // MARK: CSV

    fn open_csv_sort(&self) {
        let header = self.editor.borrow().buffer().csv_header();
        let columns: Vec<SharedString> =
            header.iter().enumerate().map(|(i, name)| if name.trim().is_empty() { format!("Column {}", i + 1) } else { format!("{} — {}", i + 1, name.trim()) }.into()).collect();
        self.tool_bridge(|b| {
            b.set_csv_columns(ModelRc::from(Rc::new(VecModel::from(columns))));
            b.set_csv_column(0);
        });
        self.open_sheet(Sheet::CsvSort);
    }

    fn sort_csv(&self) {
        let Some((column, descending, numeric, header)) =
            self.tool_bridge(|b| (b.get_csv_column().max(0) as u32, b.get_csv_descending(), b.get_csv_numeric(), b.get_csv_header()))
        else {
            return;
        };
        self.editor.borrow_mut().apply(|buffer, cursors| buffer.csv_sort(column, descending, numeric, header, cursors.to_vec()));
    }

    // MARK: Shell

    fn open_shell(&self) {
        let (_, text, selected) = self.selection_or_document();
        let lines = text.lines().count().max(1) as u64;
        let input = format!(
            "The {} ({}) goes to the command's standard input.",
            if selected { "selection" } else { "whole document" },
            crate::editor::group(lines) + if lines == 1 { " line" } else { " lines" }
        );
        self.tool_bridge(|b| {
            b.set_shell_input(input.into());
            b.set_shell_running(false);
        });
        self.open_sheet(Sheet::Shell);
    }

    fn run_shell(&self) {
        let Some((command, mode)) = self.tool_bridge(|b| (b.get_shell_command().to_string(), b.get_shell_mode())) else { return };
        if command.trim().is_empty() {
            return;
        }
        let (range, input, _) = self.selection_or_document();
        let folder = self.editor.borrow().doc().path().and_then(|p| p.parent().map(|p| p.to_path_buf()));
        self.tool_bridge(|b| b.set_shell_running(true));
        let generation = self.editor.borrow().buffer().version();
        std::thread::spawn(move || {
            let result = run_command(&command, &input, folder);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = crate::platform::app() else { return };
                app.tool_bridge(|b| b.set_shell_running(false));
                match result {
                    Ok(output) => {
                        app.close_sheet();
                        app.shell_output(range, mode, output, generation);
                    }
                    Err(error) => app.alert("The command didn’t work", &error),
                }
            });
        });
    }

    fn shell_output(&self, range: TextSelection, mode: i32, output: String, version: u64) {
        if mode == 2 {
            self.new_document_with_text(&output);
            self.refresh();
            return;
        }
        {
            let mut editor = self.editor.borrow_mut();
            if editor.buffer().version() != version {
                drop(editor);
                self.alert("The document changed while the command ran", "Its output wasn't inserted.");
                return;
            }
            let (target, text) = if mode == 1 {
                let mut text = output;
                if !range.end().column.eq(&0) && !text.starts_with('\n') {
                    text.insert(0, '\n');
                }
                (TextSelection::caret(range.end()), text)
            } else {
                (range, output)
            };
            editor.apply(|buffer, cursors| buffer.edit(vec![target], vec![text.clone()], EditKind::Other, cursors.to_vec()));
            editor.text_replaced();
        }
        self.refresh();
    }

    // MARK: Accepting

    pub fn accept_sheet(&self) {
        let sheet = self.tool_bridge(|b| b.get_sheet()).unwrap_or(0);
        match sheet {
            s if s == Sheet::CsvSort as i32 => {
                self.close_sheet();
                self.sort_csv();
                self.refresh();
            }
            s if s == Sheet::Shell as i32 => self.run_shell(),
            _ => self.close_sheet(),
        }
    }

    pub fn copy_text(&self, text: &str) {
        self.editor.borrow_mut().write_clipboard(text.to_string());
        self.show_toast("Copied");
    }
}

/// Runs a command line through the system's shell with `input` on its standard input. Returns
/// its standard output, or what went wrong (its error output).
fn run_command(command: &str, input: &str, folder: Option<PathBuf>) -> Result<String, String> {
    let mut process = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    } else {
        let mut c = Command::new(std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into()));
        c.arg("-c").arg(command);
        c
    };
    if let Some(folder) = folder {
        process.current_dir(folder);
    }
    let mut child = process.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("No input")?;
    let input = input.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if error.is_empty() { format!("It stopped with {}.", output.status) } else { error })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_commands_with_input() {
        if cfg!(windows) {
            return;
        }
        assert_eq!(run_command("sort", "b\na\n", None), Ok("a\nb\n".to_string()));
        assert!(run_command("exit 3", "", None).is_err());
        assert_eq!(run_command("echo oops >&2; exit 1", "", None), Err("oops".to_string()));
    }
}
