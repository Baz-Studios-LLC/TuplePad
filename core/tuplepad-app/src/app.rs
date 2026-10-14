//! The app: the window, the editor, the documents, settings and session, and the commands that
//! tie them together.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tuplepad_core::{Indentation, LineEnding, SessionStore, TextBuffer};

use crate::commands::{self, Menu};
use crate::document::Document;
use crate::editor::input::Modifiers;
use crate::editor::{Editor, ViewOptions, group};
use crate::settings::Settings;
use crate::{AppWindow, DocItem, MenuEntry, Palette, Theme, fonts, session};

pub struct App {
    pub ui: slint::Weak<AppWindow>,
    pub editor: RefCell<Editor>,
    pub settings: RefCell<Settings>,
    /// Where settings are saved; `None` in scripted runs, which never save them.
    settings_path: Option<PathBuf>,
    pub session: Option<SessionStore>,
    pub untitled_count: Cell<u32>,
    docs_model: Rc<VecModel<DocItem>>,
    /// Slint's own palette was set to match a pinned appearance.
    palette_pinned: Cell<bool>,
    /// Sidebar items by document, with the version they show.
    doc_items: RefCell<HashMap<String, (u64, bool, DocItem)>>,
    menus: Vec<(Menu, Rc<VecModel<MenuEntry>>)>,
    language_names: HashMap<String, String>,
    pub open_ms: RefCell<Vec<(String, f64)>>,
    /// Answers for dialogs in scripted runs (they never show real ones).
    pub scripted_answers: Option<RefCell<VecDeque<String>>>,
}

pub fn view_options(settings: &Settings) -> ViewOptions {
    ViewOptions {
        show_invisibles: settings.show_invisibles,
        show_indent_guides: settings.show_indent_guides,
        highlight_current_line: settings.highlight_current_line,
        smart_highlight: settings.smart_highlight,
        auto_close: settings.auto_close_brackets,
        tab_width: settings.tab_width.max(1),
        default_indentation: settings.indentation(),
    }
}

impl App {
    pub fn new(ui: &AppWindow, settings: Settings, settings_path: Option<PathBuf>, session: Option<SessionStore>, scripted: bool) -> Rc<App> {
        let mut editor = Editor::new(ui, "Menlo", settings.font_size, settings.line_height, view_options(&settings));
        let family = fonts::pick(&mut editor.layouter, &settings.font_family);
        editor.set_font(&family, settings.font_size, settings.line_height);
        ui.global::<Theme>().set_mono_family(family.into());
        if scripted {
            editor.private_clipboard = Some(String::new());
        }
        let docs_model = Rc::new(VecModel::default());
        ui.set_docs(ModelRc::from(docs_model.clone()));
        let menus: Vec<(Menu, Rc<VecModel<MenuEntry>>)> =
            [Menu::File, Menu::Edit, Menu::Selection, Menu::View].into_iter().map(|m| (m, Rc::new(VecModel::default()))).collect();
        for (menu, model) in &menus {
            let model = ModelRc::from(model.clone());
            match menu {
                Menu::File => ui.set_file_menu(model),
                Menu::Edit => ui.set_edit_menu(model),
                Menu::Selection => ui.set_selection_menu(model),
                Menu::View => ui.set_view_menu(model),
                Menu::None => {}
            }
        }
        let app = Rc::new(App {
            ui: ui.as_weak(),
            editor: RefCell::new(editor),
            settings: RefCell::new(settings),
            settings_path,
            session,
            untitled_count: Cell::new(0),
            docs_model,
            palette_pinned: Cell::new(false),
            doc_items: RefCell::new(HashMap::new()),
            menus,
            language_names: tuplepad_core::languages().into_iter().map(|l| (l.id, l.name)).collect(),
            open_ms: RefCell::new(Vec::new()),
            scripted_answers: scripted.then(|| RefCell::new(VecDeque::new())),
        });
        app.apply_settings(false);
        app
    }

    pub fn is_scripted(&self) -> bool {
        self.scripted_answers.is_some()
    }

    // MARK: Settings

    /// Pushes settings to the editor and window, and saves them unless `save` is false.
    pub fn apply_settings(&self, save: bool) {
        let settings = self.settings.borrow().clone();
        if let Some(ui) = self.ui.upgrade() {
            let appearance = match settings.appearance.as_str() {
                "light" => 1,
                "dark" => 2,
                _ => 0,
            };
            ui.global::<Theme>().set_appearance(appearance);
            // Where Slint draws the menu bar itself, its colors follow TuplePad's appearance.
            if !cfg!(target_os = "macos") || std::env::var_os("SLINT_NO_MUDA").is_some() {
                use slint::language::ColorScheme;
                // Following the system needs nothing, unless an earlier pin has to be undone.
                let dark = match appearance {
                    1 => Some(false),
                    2 => Some(true),
                    _ if self.palette_pinned.get() => Some(system_is_dark(&ui)),
                    _ => None,
                };
                if let Some(dark) = dark {
                    ui.global::<Palette>().set_color_scheme(if dark { ColorScheme::Dark } else { ColorScheme::Light });
                    self.palette_pinned.set(appearance != 0);
                }
            }
            ui.set_sidebar_visible(settings.sidebar_visible);
            ui.set_sidebar_width(settings.sidebar_width);
        }
        if let Ok(mut editor) = self.editor.try_borrow_mut() {
            editor.set_options(view_options(&settings));
            if editor.font_size != settings.font_size || editor.line_height_factor != settings.line_height {
                let family = editor.family.clone();
                editor.set_font(&family, settings.font_size, settings.line_height);
            }
        }
        if save && let Some(path) = &self.settings_path {
            settings.save(path);
        }
    }

    fn change_settings(&self, change: impl FnOnce(&mut Settings)) {
        change(&mut self.settings.borrow_mut());
        self.apply_settings(true);
    }

    // MARK: Drawing

    /// Redraws everything that depends on the documents and the editor.
    pub fn refresh(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let Ok(mut editor) = self.editor.try_borrow_mut() else { return };
        editor.sync(&ui);

        let items: Vec<DocItem> = {
            let mut cache = self.doc_items.borrow_mut();
            let items = editor
                .docs
                .iter()
                .map(|doc| {
                    let stamp = (doc.buffer.version(), doc.buffer.is_modified());
                    match cache.get(&doc.id) {
                        Some((version, modified, item)) if (*version, *modified) == stamp && doc.path().is_some() == !item.folder.starts_with("Not saved") => item.clone(),
                        _ => {
                            let item = self.doc_item(doc);
                            cache.insert(doc.id.clone(), (stamp.0, stamp.1, item.clone()));
                            item
                        }
                    }
                })
                .collect();
            cache.retain(|id, _| editor.docs.iter().any(|d| &d.id == id));
            items
        };
        if self.docs_model.row_count() != items.len() {
            self.docs_model.set_vec(items);
        } else {
            for (index, item) in items.into_iter().enumerate() {
                if self.docs_model.row_data(index).as_ref() != Some(&item) {
                    self.docs_model.set_row_data(index, item);
                }
            }
        }

        let doc = editor.doc();
        ui.set_current_doc(editor.current as i32);
        let name = doc.name();
        ui.set_window_title(name.clone().into());
        ui.set_doc_title(name.into());
        let mut subtitle = doc.path().and_then(|p| p.parent().map(|f| tilde(f))).unwrap_or_else(|| "Not saved".into());
        if doc.buffer.is_modified() {
            subtitle.push_str("  —  Edited");
        }
        ui.set_doc_subtitle(subtitle.into());
        ui.set_wraps(doc.wrap);
        ui.set_status_left(editor.status_left().into());
        let info = doc.buffer.file_info();
        let ending = match info.line_ending {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
            LineEnding::Cr => "CR",
        };
        let indentation = match editor.indentation() {
            Indentation::Tabs => "Tabs".to_string(),
            Indentation::Spaces(width) => format!("Spaces: {width}"),
        };
        let lines = doc.buffer.line_count();
        ui.set_status_right(
            format!(
                "{}   ·   {indentation}   ·   {}{}   ·   {ending}   ·   {} {}",
                self.language_name(doc),
                info.encoding.name,
                if info.encoding.has_bom { " with BOM" } else { "" },
                group(lines),
                if lines == 1 { "line" } else { "lines" }
            )
            .into(),
        );

        // What the menus show as enabled and checked.
        let settings = self.settings.borrow();
        let (can_undo, can_redo, wrap) = (doc.buffer.can_undo(), doc.buffer.can_redo(), doc.wrap);
        let enabled = |id: &str| match id {
            "edit.undo" => can_undo,
            "edit.redo" => can_redo,
            _ => true,
        };
        let checked = |id: &str| match id {
            "view.sidebar" => settings.sidebar_visible,
            "view.wrap" => wrap,
            "view.invisibles" => settings.show_invisibles,
            "view.indent-guides" => settings.show_indent_guides,
            "view.current-line" => settings.highlight_current_line,
            "view.appearance-system" => settings.appearance == "system",
            "view.appearance-light" => settings.appearance == "light",
            "view.appearance-dark" => settings.appearance == "dark",
            _ => false,
        };
        for (menu, model) in &self.menus {
            let entries = commands::menu_entries(*menu, enabled, checked);
            if model.row_count() != entries.len() {
                model.set_vec(entries);
                continue;
            }
            for (index, entry) in entries.into_iter().enumerate() {
                if model.row_data(index).as_ref() != Some(&entry) {
                    model.set_row_data(index, entry);
                }
            }
        }
    }

    /// Called by a timer: blinks the caret and catches up on smart highlighting.
    pub fn tick(&self) {
        let needs_sync = match (self.ui.upgrade(), self.editor.try_borrow_mut()) {
            (Some(ui), Ok(mut editor)) => editor.tick(&ui),
            _ => false,
        };
        if needs_sync {
            self.refresh();
        }
    }

    fn language_name(&self, doc: &Document) -> String {
        doc.buffer.language().and_then(|id| self.language_names.get(&id).cloned()).unwrap_or_else(|| "Plain Text".into())
    }

    fn doc_item(&self, doc: &Document) -> DocItem {
        let name = doc.name();
        let extension = match Path::new(&name).extension() {
            Some(e) if doc.path().is_some() => e.to_string_lossy().to_uppercase(),
            _ => "TXT".into(),
        };
        let folder = doc.path().and_then(|p| p.parent().map(|f| tilde(f))).unwrap_or_else(|| "Not saved".into());
        let size = if doc.path().is_some() { format!("  ·  {}", format_size(doc.size)) } else { String::new() };
        DocItem {
            name: name.into(),
            folder: folder.clone().into(),
            detail: format!("{folder}{size}").into(),
            preview: doc.preview().into(),
            badge: extension.chars().take(4).collect::<String>().into(),
            tint: tint_for(&extension),
            modified: doc.buffer.is_modified(),
        }
    }

    // MARK: Commands

    /// A key press in the editor: a command's shortcut, or an editing key.
    pub fn key(&self, text: &str, control: bool, alt: bool, shift: bool, meta: bool) -> bool {
        if let Some(command) = commands::for_key(text, control, alt, shift, meta) {
            self.run_command(command.id);
            return true;
        }
        let handled = match self.editor.try_borrow_mut() {
            Ok(mut editor) => editor.key(text, Modifiers { command: control, control: meta, alt, shift }),
            Err(_) => false,
        };
        if handled {
            self.refresh();
        }
        handled
    }

    pub fn run_command(&self, id: &str) {
        {
            let mut editor = self.editor.borrow_mut();
            editor.touch();
            let indentation = editor.indentation();
            match id {
                "edit.undo" => editor.undo(),
                "edit.redo" => editor.redo(),
                "edit.cut" => editor.cut(),
                "edit.copy" => editor.copy(),
                "edit.paste" => editor.paste(),
                "edit.select-all" => editor.select_all(),
                "edit.indent" => editor.apply(|b, c| b.indent_lines(c, indentation)),
                "edit.outdent" => editor.apply(|b, c| b.outdent_lines(c, indentation)),
                "selection.line" => editor.select_lines(),
                "selection.next" => editor.select_next_occurrence(),
                "selection.caret-above" => editor.add_caret_vertically(false),
                "selection.caret-below" => editor.add_caret_vertically(true),
                "selection.single" => editor.single_cursor(),
                "view.wrap" => {
                    let doc = editor.doc_mut();
                    doc.wrap = !doc.wrap;
                    doc.scroll_x = 0.0;
                    editor.reveal();
                }
                _ => {}
            }
        }
        match id {
            "file.new" => self.new_document(),
            "file.open" => self.open_dialog(),
            "file.save" => {
                self.save_current(false);
            }
            "file.save-as" => {
                self.save_current(true);
            }
            "file.close" => self.close_document(None),
            "view.sidebar" => self.change_settings(|s| s.sidebar_visible = !s.sidebar_visible),
            "view.invisibles" => self.change_settings(|s| s.show_invisibles = !s.show_invisibles),
            "view.indent-guides" => self.change_settings(|s| s.show_indent_guides = !s.show_indent_guides),
            "view.current-line" => self.change_settings(|s| s.highlight_current_line = !s.highlight_current_line),
            "view.zoom-in" => self.change_settings(|s| s.font_size = (s.font_size + 1.0).min(48.0)),
            "view.zoom-out" => self.change_settings(|s| s.font_size = (s.font_size - 1.0).max(8.0)),
            "view.zoom-reset" => self.change_settings(|s| s.font_size = Settings::default().font_size),
            "view.appearance-system" => self.change_settings(|s| s.appearance = "system".into()),
            "view.appearance-light" => self.change_settings(|s| s.appearance = "light".into()),
            "view.appearance-dark" => self.change_settings(|s| s.appearance = "dark".into()),
            "view.toggle-theme" => {
                let dark = self.ui.upgrade().is_some_and(|ui| ui.global::<Theme>().get_dark());
                self.change_settings(|s| s.appearance = if dark { "light".into() } else { "dark".into() });
            }
            _ => {}
        }
        self.refresh();
    }

    pub fn select_document(&self, index: usize) {
        self.editor.borrow_mut().select_document(index);
        self.refresh();
    }

    pub fn set_sidebar_width(&self, width: f32) {
        let width = width.clamp(200.0, 420.0);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_sidebar_width(width);
        }
        self.settings.borrow_mut().sidebar_width = width;
    }

    /// Saves settings that change continuously (like the sidebar width) once they settle.
    pub fn save_settings(&self) {
        if let Some(path) = &self.settings_path {
            self.settings.borrow().save(path);
        }
    }

    // MARK: Documents

    fn next_untitled(&self) -> u32 {
        let next = self.untitled_count.get() + 1;
        self.untitled_count.set(next);
        next
    }

    pub fn add_document(&self, doc: Document) {
        let mut editor = self.editor.borrow_mut();
        let index = editor.add_document(doc);
        editor.select_document(index);
    }

    pub fn new_document(&self) {
        let settings = self.settings.borrow().clone();
        let buffer = TextBuffer::untitled(tuplepad_core::TextEncoding::utf8(), LineEnding::Lf);
        let doc = Document::new(buffer, self.next_untitled(), 0, settings.wrap_by_default);
        self.add_document(doc);
    }

    /// Opens files (or shows them if they're already open). Returns how many it opened.
    pub fn open_paths(&self, paths: &[PathBuf]) -> usize {
        let mut opened = 0;
        for path in paths {
            let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let existing = self.editor.borrow().docs.iter().position(|d| d.path().as_deref() == Some(absolute.as_path()));
            if let Some(index) = existing {
                self.editor.borrow_mut().select_document(index);
                continue;
            }
            let started = Instant::now();
            match TextBuffer::open(absolute.to_string_lossy().into_owned()) {
                Ok(buffer) => {
                    let ms = started.elapsed().as_secs_f64() * 1000.0;
                    let settings = self.settings.borrow().clone();
                    let size = std::fs::metadata(&absolute).map_or(0, |m| m.len());
                    let mut doc = Document::new(buffer, 0, size, settings.wrap_by_default);
                    if settings.detect_indentation {
                        doc.indentation = Indentation::detect(&doc.buffer);
                    }
                    self.open_ms.borrow_mut().push((doc.name(), ms));
                    self.add_document(doc);
                    opened += 1;
                }
                Err(error) => self.alert(&format!("TuplePad couldn't open “{}”.", path.display()), &error.to_string()),
            }
        }
        // An empty untitled document someone hasn't used gives way to what was opened.
        if opened > 0 {
            self.drop_unused_untitled();
        }
        self.refresh();
        opened
    }

    fn drop_unused_untitled(&self) {
        let mut editor = self.editor.borrow_mut();
        if editor.docs.len() < 2 {
            return;
        }
        let unused = editor.docs.iter().position(|d| d.is_untitled() && !d.buffer.is_modified() && d.buffer.len_utf16() == 0);
        if let Some(index) = unused {
            let current = editor.current;
            editor.remove_document(index);
            let target = if current > index { current - 1 } else { current };
            editor.show_document(target);
        }
    }

    pub fn open_dialog(&self) {
        let paths = if self.is_scripted() {
            self.next_answer().map(|p| vec![PathBuf::from(p)]).unwrap_or_default()
        } else {
            rfd::FileDialog::new().set_title("Open").pick_files().unwrap_or_default()
        };
        self.open_paths(&paths);
    }

    /// Saves the current document (asking where for untitled ones, or always with `ask`).
    /// Returns false if it wasn't saved.
    pub fn save_current(&self, ask: bool) -> bool {
        let (buffer, name, path) = {
            let editor = self.editor.borrow();
            let doc = editor.doc();
            (doc.buffer.clone(), doc.name(), doc.path())
        };
        let target = match path {
            Some(path) if !ask => Some((path, false)),
            _ => {
                let chosen = if self.is_scripted() {
                    self.next_answer().map(PathBuf::from)
                } else {
                    let mut dialog = rfd::FileDialog::new().set_title("Save As").set_file_name(&name);
                    if let Some(folder) = buffer.path().and_then(|p| Path::new(&p).parent().map(Path::to_path_buf)) {
                        dialog = dialog.set_directory(folder);
                    }
                    dialog.save_file()
                };
                chosen.map(|p| (p, true))
            }
        };
        let Some((path, new_path)) = target else { return false };
        let result = if new_path { buffer.save_as(path.to_string_lossy().into_owned()) } else { buffer.save() };
        match result {
            Ok(()) => {
                if new_path {
                    // A newly saved document may highlight differently.
                    let mut editor = self.editor.borrow_mut();
                    editor.doc_mut().size = std::fs::metadata(&path).map_or(0, |m| m.len());
                    editor.invalidate();
                }
                self.refresh();
                true
            }
            Err(error) => {
                self.alert(&format!("TuplePad couldn't save “{name}”."), &error.to_string());
                false
            }
        }
    }

    /// Closes a document (the current one by default), asking first about unsaved changes.
    pub fn close_document(&self, index: Option<usize>) {
        let index = index.unwrap_or_else(|| self.editor.borrow().current);
        let (modified, name) = {
            let editor = self.editor.borrow();
            let Some(doc) = editor.docs.get(index) else { return };
            (doc.buffer.is_modified(), doc.name())
        };
        if modified {
            self.editor.borrow_mut().select_document(index);
            self.refresh();
            match self.ask_to_save(&name).as_str() {
                "Save" => {
                    if !self.save_current(false) {
                        return;
                    }
                }
                "Don't Save" => {}
                _ => return,
            }
        }
        let empty = {
            let mut editor = self.editor.borrow_mut();
            editor.remove_document(index);
            editor.docs.is_empty()
        };
        if empty {
            self.new_document();
        } else {
            let mut editor = self.editor.borrow_mut();
            let current = editor.current;
            editor.show_document(current);
        }
        self.refresh();
    }

    // MARK: Dialogs

    fn next_answer(&self) -> Option<String> {
        self.scripted_answers.as_ref()?.borrow_mut().pop_front()
    }

    fn ask_to_save(&self, name: &str) -> String {
        if self.is_scripted() {
            return self.next_answer().unwrap_or_else(|| "Cancel".into());
        }
        let buttons = rfd::MessageButtons::YesNoCancelCustom("Save".into(), "Don't Save".into(), "Cancel".into());
        let result = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Save changes?")
            .set_description(format!("Do you want to save the changes you made to “{name}”?\n\nYour changes will be lost if you don't save them."))
            .set_buttons(buttons)
            .show();
        match result {
            rfd::MessageDialogResult::Custom(answer) => answer,
            rfd::MessageDialogResult::Yes => "Save".into(),
            rfd::MessageDialogResult::No => "Don't Save".into(),
            _ => "Cancel".into(),
        }
    }

    pub fn alert(&self, title: &str, detail: &str) {
        eprintln!("{title} {detail}");
        if self.is_scripted() {
            return;
        }
        rfd::MessageDialog::new().set_level(rfd::MessageLevel::Warning).set_title(title).set_description(detail).show();
    }

    // MARK: Session

    pub fn save_session(&self) {
        if let Some(store) = &self.session
            && let Ok(mut editor) = self.editor.try_borrow_mut()
        {
            session::save(store, &mut editor, self.untitled_count.get());
        }
    }
}

/// Whether the system is in dark mode.
fn system_is_dark(ui: &AppWindow) -> bool {
    use slint::winit_030::{WinitWindowAccessor, winit::window::Theme as WinitTheme};
    ui.window().with_winit_window(|window| window.theme() == Some(WinitTheme::Dark)).unwrap_or(false)
}

pub fn tilde(path: &Path) -> String {
    let path = path.to_string_lossy();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.into_owned(),
    }
}

fn format_size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} bytes"),
        1_000..1_000_000 => format!("{:.0} KB", bytes as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", bytes as f64 / 1e6),
        _ => format!("{:.2} GB", bytes as f64 / 1e9),
    }
}

/// A color for each kind of file's badge. (Never green: that's Fountage's.)
fn tint_for(extension: &str) -> slint::Color {
    let rgb: u32 = match extension {
        "RS" => 0xC0571F,
        "SWIFT" => 0xE0533A,
        "LOG" => 0x5B6F99,
        "MD" | "MARKDOWN" => 0x3D5A9E,
        "JSON" | "TOML" | "YAML" | "YML" => 0xB7791F,
        "PY" => 0x3572A5,
        "JS" | "TS" | "TSX" | "JSX" | "MJS" => 0x2F6FB7,
        "HTML" | "XML" | "SVG" => 0xC2273D,
        "CSS" | "SCSS" => 0x6A4FB3,
        "C" | "H" | "CPP" | "HPP" | "CC" => 0x5D6B8A,
        "SH" | "ZSH" | "BASH" => 0x7A6A55,
        _ => 0x8C826F,
    };
    slint::Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}
