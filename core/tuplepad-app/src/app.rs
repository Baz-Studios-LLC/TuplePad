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
use crate::document::{Conflict, Document};
use crate::editor::input::Modifiers;
use crate::editor::{Editor, ViewOptions, group};
use crate::settings::Settings;
use crate::{AppWindow, DocItem, MenuEntry, Palette, Theme, fonts, session};

pub struct App {
    /// The app itself, for callbacks created later.
    pub me: RefCell<std::rc::Weak<App>>,
    pub ui: slint::Weak<AppWindow>,
    pub settings_window: RefCell<Option<crate::SettingsWindow>>,
    /// What the command palette searches, and which of it is showing.
    pub palette_entries: RefCell<Vec<crate::palette::Entry>>,
    pub palette_shown: RefCell<Vec<usize>>,
    /// The font choices the Settings window offers (worked out when it first opens).
    pub font_choices: RefCell<Vec<(String, String)>>,
    pub editor: RefCell<Editor>,
    pub settings: RefCell<Settings>,
    /// Where settings are saved; `None` in scripted runs, which never save them.
    settings_path: Option<PathBuf>,
    pub session: Option<SessionStore>,
    pub untitled_count: Cell<u32>,
    docs_model: Rc<VecModel<DocItem>>,
    /// Slint's own palette was set to match a pinned appearance.
    palette_pinned: Cell<bool>,
    shut_down: Cell<bool>,
    /// Sidebar items by document, with the version they show.
    doc_items: RefCell<HashMap<String, (u64, bool, DocItem)>>,
    /// The menu models, by name: "file", "file-2", "file-3" (the File menu's sections around its
    /// submenus), "recent", "reopen", "convert", "line-ending", "edit", "selection" and "view".
    menus: HashMap<&'static str, Rc<VecModel<MenuEntry>>>,
    /// The folder the last Show in Finder showed, for scripted checks (which don't open one).
    pub revealed: RefCell<Option<PathBuf>>,
    /// The page the last Print prepared, for scripted checks (which don't print).
    pub printed: RefCell<Option<PathBuf>>,
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
        let names = ["file", "file-2", "file-3", "recent", "reopen", "convert", "line-ending", "edit", "selection", "view", "view-2", "theme"];
        let menus: HashMap<&'static str, Rc<VecModel<MenuEntry>>> = names.into_iter().map(|n| (n, Rc::new(VecModel::default()))).collect();
        for (name, model) in &menus {
            let model = ModelRc::from(model.clone());
            match *name {
                "file" => ui.set_file_menu(model),
                "file-2" => ui.set_file_menu_2(model),
                "file-3" => ui.set_file_menu_3(model),
                "recent" => ui.set_recent_menu(model),
                "reopen" => ui.set_reopen_menu(model),
                "convert" => ui.set_convert_menu(model),
                "line-ending" => ui.set_line_ending_menu(model),
                "edit" => ui.set_edit_menu(model),
                "selection" => ui.set_selection_menu(model),
                "view-2" => ui.set_view_menu_2(model),
                "theme" => ui.set_theme_menu(model),
                _ => ui.set_view_menu(model),
            }
        }
        let app = Rc::new(App {
            me: RefCell::new(std::rc::Weak::new()),
            ui: ui.as_weak(),
            settings_window: RefCell::new(None),
            font_choices: RefCell::new(Vec::new()),
            palette_entries: RefCell::new(Vec::new()),
            palette_shown: RefCell::new(Vec::new()),
            editor: RefCell::new(editor),
            settings: RefCell::new(settings),
            settings_path,
            session,
            untitled_count: Cell::new(0),
            docs_model,
            palette_pinned: Cell::new(false),
            shut_down: Cell::new(false),
            doc_items: RefCell::new(HashMap::new()),
            menus,
            revealed: RefCell::new(None),
            printed: RefCell::new(None),
            language_names: tuplepad_core::languages().into_iter().map(|l| (l.id, l.name)).collect(),
            open_ms: RefCell::new(Vec::new()),
            scripted_answers: scripted.then(|| RefCell::new(VecDeque::new())),
        });
        *app.me.borrow_mut() = Rc::downgrade(&app);
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
            let theme = crate::settings_window::THEME_IDS.iter().position(|t| *t == settings.theme).unwrap_or(0);
            ui.global::<Theme>().set_theme(theme as i32);
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
            let family = fonts::pick(&mut editor.layouter, &settings.font_family);
            if editor.family != family || editor.font_size != settings.font_size || editor.line_height_factor != settings.line_height {
                editor.set_font(&family, settings.font_size, settings.line_height);
                if let Some(ui) = self.ui.upgrade() {
                    ui.global::<Theme>().set_mono_family(family.into());
                }
            }
        }
        self.refresh_settings_window();
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
        let (banner, banner_text) = match doc.conflict {
            Some(Conflict::Changed) => (1, format!("“{}” changed on disk, and you have unsaved changes here.", doc.name())),
            Some(Conflict::Missing) => (2, format!("“{}” was moved or deleted. Your copy is still open.", doc.name())),
            None => (0, String::new()),
        };
        ui.set_banner(banner);
        if banner != 0 {
            ui.set_banner_text(banner_text.into());
        }
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
        let has_path = doc.path().is_some();
        let (can_undo, can_redo, wrap, follow, modified) = (doc.buffer.can_undo(), doc.buffer.can_redo(), doc.wrap, doc.follow, doc.buffer.is_modified());
        let several = editor.docs.len() > 1;
        let enabled = |id: &str| match id {
            "edit.undo" => can_undo,
            "edit.redo" => can_redo,
            "file.revert" => has_path && modified,
            "file.move" | "file.show" | "view.follow" => has_path,
            "file.close-others" => several,
            _ => true,
        };
        let checked = |id: &str| match id {
            "view.sidebar" => settings.sidebar_visible,
            "view.wrap" => wrap,
            "view.follow" => follow,
            "view.invisibles" => settings.show_invisibles,
            "view.indent-guides" => settings.show_indent_guides,
            "view.current-line" => settings.highlight_current_line,
            "view.appearance-system" => settings.appearance == "system",
            "view.appearance-light" => settings.appearance == "light",
            "view.appearance-dark" => settings.appearance == "dark",

            _ => false,
        };
        let mut file = commands::menu_sections(Menu::File, enabled, checked).into_iter();
        self.set_menu("file", file.next().unwrap_or_default());
        self.set_menu("file-2", file.next().unwrap_or_default());
        self.set_menu("file-3", file.next().unwrap_or_default());
        for (name, menu) in [("edit", Menu::Edit), ("selection", Menu::Selection)] {
            self.set_menu(name, commands::menu_sections(menu, enabled, checked).concat());
        }
        let mut view = commands::menu_sections(Menu::View, enabled, checked).into_iter();
        self.set_menu("view", view.next().unwrap_or_default());
        self.set_menu("view-2", view.next().unwrap_or_default());
        let themes = crate::settings_window::THEME_IDS
            .iter()
            .zip(["TuplePad", "Midnight", "Graphite", "Sepia", "High Contrast"])
            .map(|(id, name)| commands::item_entry(format!("theme:{id}"), name.into(), true, Some(settings.theme == *id)))
            .collect();
        self.set_menu("theme", themes);

        // Open Recent, and the encodings and line endings.
        let mut recent: Vec<MenuEntry> = settings
            .recent_files
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let path = Path::new(path);
                let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                let folder = path.parent().map(tilde).unwrap_or_default();
                commands::item_entry(format!("recent:{i}"), format!("{name}  —  {folder}"), true, None)
            })
            .collect();
        if !recent.is_empty() {
            recent.push(commands::separator_entry());
            recent.push(commands::item_entry("recent.clear".into(), "Clear Menu".into(), true, None));
        }
        self.set_menu("recent", recent);
        let current = (info.encoding.name.clone(), info.encoding.has_bom);
        let reopen = tuplepad_core::supported_encodings()
            .into_iter()
            .map(|name| commands::item_entry(format!("reopen:{name}"), name.clone(), has_path, Some(name == current.0)))
            .collect();
        self.set_menu("reopen", reopen);
        let mut convert: Vec<MenuEntry> = Vec::new();
        for name in tuplepad_core::supported_encodings() {
            let unicode = name.starts_with("UTF");
            convert.push(commands::item_entry(format!("convert:{name}"), name.clone(), true, Some(current == (name.clone(), false))));
            if unicode {
                let title = format!("{name} with BOM");
                convert.push(commands::item_entry(format!("convert:{name}+bom"), title, true, Some(current == (name.clone(), true))));
            }
        }
        self.set_menu("convert", convert);
        let endings = [("lf", "LF (Mac and Linux)", LineEnding::Lf), ("crlf", "CRLF (Windows)", LineEnding::CrLf), ("cr", "CR (classic Mac)", LineEnding::Cr)];
        let line_endings = endings
            .into_iter()
            .map(|(id, title, ending)| commands::item_entry(format!("line-ending:{id}"), title.into(), true, Some(info.line_ending == ending)))
            .collect();
        self.set_menu("line-ending", line_endings);
    }

    /// Updates one menu model, touching only the entries that changed.
    fn set_menu(&self, name: &str, entries: Vec<MenuEntry>) {
        let Some(model) = self.menus.get(name) else { return };
        if model.row_count() != entries.len() {
            model.set_vec(entries);
            return;
        }
        for (index, entry) in entries.into_iter().enumerate() {
            if model.row_data(index).as_ref() != Some(&entry) {
                model.set_row_data(index, entry);
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
        if let Some((kind, value)) = id.split_once(':') {
            self.run_item(kind, value);
            self.refresh();
            return;
        }
        match id {
            "file.new" => self.new_document(),
            "file.save-all" => self.save_all(),
            "file.revert" => self.revert(),
            "file.move" => self.move_current(),
            "file.show" => self.show_in_file_manager(),
            "file.print" => {
                if let Some(ui) = self.ui.upgrade() {
                    let settings = self.settings.borrow();
                    ui.set_print_line_numbers(settings.print_line_numbers);
                    ui.set_print_colors(settings.print_colors);
                    ui.set_print_sheet(true);
                }
            }
            "print.confirm" | "print.cancel" => {
                let Some(ui) = self.ui.upgrade() else { return };
                ui.set_print_sheet(false);
                ui.invoke_focus_editor();
                if id == "print.confirm" {
                    let (numbers, colors) = (ui.get_print_line_numbers(), ui.get_print_colors());
                    self.change_settings(|s| {
                        s.print_line_numbers = numbers;
                        s.print_colors = colors;
                    });
                    self.print();
                }
            }
            "file.install-tool" => self.install_command_line_tool(),
            "file.close-others" => self.close_others(),
            "file.close-all" => self.close_all(),
            "recent.clear" => self.change_settings(|s| s.recent_files.clear()),
            "disk.reload" => self.revert(),
            "disk.keep" => {
                let mut editor = self.editor.borrow_mut();
                editor.buffer().acknowledge_disk_change();
                editor.doc_mut().conflict = None;
            }
            "disk.save" => {
                if self.save_current(false) {
                    self.editor.borrow_mut().doc_mut().conflict = None;
                }
            }
            "view.follow" => {
                let mut editor = self.editor.borrow_mut();
                let doc = editor.doc_mut();
                doc.follow = !doc.follow;
                if doc.follow {
                    let end = doc.buffer.end_position();
                    doc.cursors = vec![tuplepad_core::TextSelection::caret(end)];
                    editor.reveal();
                }
            }
            "file.open" => self.open_dialog(),
            "file.save" => {
                self.save_current(false);
            }
            "file.save-as" => {
                self.save_current(true);
            }
            "file.close" if self.settings_focused() => self.hide_settings(),
            "file.close" => self.close_document(None),
            "app.settings" => self.show_settings(None),
            "view.palette" => self.open_palette(),
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

    /// Items of the generated submenus: `recent:<index>`, `reopen:<encoding>`,
    /// `convert:<encoding>[+bom]` and `line-ending:<lf|crlf|cr>`.
    fn run_item(&self, kind: &str, value: &str) {
        match kind {
            "recent" => {
                let path = value.parse::<usize>().ok().and_then(|i| self.settings.borrow().recent_files.get(i).cloned());
                if let Some(path) = path {
                    if !Path::new(&path).exists() {
                        self.settings.borrow_mut().recent_files.retain(|p| *p != path);
                        self.alert(&format!("“{}” can't be found.", tilde(Path::new(&path))), "It may have been moved, renamed or deleted.");
                        return;
                    }
                    self.open_paths(&[PathBuf::from(path)]);
                }
            }
            "reopen" => {
                let (buffer, cursors, modified) = {
                    let editor = self.editor.borrow();
                    (editor.buffer(), editor.doc().cursors.clone(), editor.doc().buffer.is_modified())
                };
                if modified && !self.confirm("Reopen with a different encoding?", "Your unsaved changes will be lost (Undo brings them back).", "Reopen") {
                    return;
                }
                let encoding = tuplepad_core::TextEncoding { name: value.to_string(), has_bom: false };
                match buffer.reopen_with_encoding(encoding, cursors) {
                    Ok(()) => self.editor.borrow_mut().text_replaced(),
                    Err(error) => self.alert(&format!("TuplePad couldn't reopen the file as {value}."), &error.to_string()),
                }
            }
            "convert" => {
                let (name, bom) = match value.strip_suffix("+bom") {
                    Some(name) => (name, true),
                    None => (value, false),
                };
                let encoding = tuplepad_core::TextEncoding { name: name.to_string(), has_bom: bom };
                let buffer = self.editor.borrow().buffer();
                match buffer.unencodable_character(encoding.clone()) {
                    Ok(Some(character)) => self.alert(
                        &format!("This document can't be saved as {name}."),
                        &format!("“{character}” and maybe other characters have no {name} equivalent. Pick another encoding, or remove them first."),
                    ),
                    Ok(None) => {
                        if let Err(error) = buffer.set_encoding(encoding) {
                            self.alert("TuplePad couldn't change the encoding.", &error.to_string());
                        }
                    }
                    Err(error) => self.alert("TuplePad couldn't change the encoding.", &error.to_string()),
                }
            }
            "theme" => {
                let value = value.to_string();
                self.change_settings(|s| s.theme = value);
            }
            "line-ending" => {
                let ending = match value {
                    "crlf" => LineEnding::CrLf,
                    "cr" => LineEnding::Cr,
                    _ => LineEnding::Lf,
                };
                let mut editor = self.editor.borrow_mut();
                let cursors = editor.doc().cursors.clone();
                editor.buffer().convert_line_endings(ending, cursors);
                editor.text_replaced();
            }
            _ => {}
        }
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
        let buffer = TextBuffer::untitled(settings.new_document_encoding(), settings.new_document_line_ending());
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
                    self.settings.borrow_mut().note_recent(&absolute);
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

    /// Files asked for from outside (Finder, the `tuplepad` command, a drop), at a line and column
    /// if given. Files that don't exist yet open as new documents, created when saved.
    pub fn open_requests(&self, requests: Vec<(PathBuf, Option<u64>, Option<u64>)>) {
        for (path, line, column) in requests {
            let path = crate::platform::absolute(&path);
            if path.exists() {
                self.open_paths(std::slice::from_ref(&path));
            } else {
                let settings = self.settings.borrow().clone();
                let buffer = TextBuffer::untitled(tuplepad_core::TextEncoding::utf8(), LineEnding::Lf);
                buffer.set_path(Some(path.to_string_lossy().into_owned()));
                self.add_document(Document::new(buffer, 0, 0, settings.wrap_by_default));
                self.drop_unused_untitled();
            }
            if let Some(line) = line {
                let mut editor = self.editor.borrow_mut();
                let target = tuplepad_core::TextPos::new(line.saturating_sub(1), column.unwrap_or(1).saturating_sub(1));
                editor.go_to_line(target.line);
                let pos = editor.buffer().clamp(target);
                editor.doc_mut().cursors = vec![tuplepad_core::TextSelection::caret(pos)];
                editor.reveal();
            }
        }
        self.refresh();
    }

    /// A new untitled document holding `text` (standard input, from the `tuplepad` command).
    pub fn new_document_with_text(&self, text: &str) {
        self.new_document();
        let mut editor = self.editor.borrow_mut();
        editor.apply(|buffer, cursors| buffer.paste(cursors, text));
        let start = tuplepad_core::TextSelection::caret(tuplepad_core::TextPos::new(0, 0));
        editor.doc_mut().cursors = vec![start];
        editor.reveal();
    }

    /// Brings the window to the front.
    pub fn activate(&self) {
        if self.is_scripted() {
            return;
        }
        if let Some(ui) = self.ui.upgrade() {
            use slint::winit_030::WinitWindowAccessor;
            let _ = ui.show();
            ui.window().with_winit_window(|window| window.focus_window());
        }
    }

    /// Installs the `tuplepad` terminal command: a link to the one that comes with the app, in a
    /// folder on the PATH. Asks for an administrator password only if no such folder is writable.
    pub fn install_command_line_tool(&self) {
        let Some(tool) = bundled_tool() else {
            self.alert("The tuplepad command isn't included in this copy of TuplePad.", "Build it with scripts/bundle-mac.sh, or download TuplePad again.");
            return;
        };
        if cfg!(windows) {
            self.alert("The tuplepad command is ready.", &format!("It's at {}. Add that folder to your PATH to use it from any terminal.", tool.display()));
            return;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let folders: Vec<PathBuf> = match self.next_scripted_folder() {
            Some(folder) => vec![folder],
            None if cfg!(target_os = "macos") => vec!["/opt/homebrew/bin".into(), "/usr/local/bin".into()],
            None => vec![home.join(".local/bin")],
        };
        for folder in &folders {
            if !cfg!(target_os = "macos") {
                let _ = std::fs::create_dir_all(folder);
            }
            let writable = std::fs::metadata(folder).map(|m| !m.permissions().readonly()).unwrap_or(false);
            let link = folder.join("tuplepad");
            if writable && (std::fs::remove_file(&link).is_ok() || !link.exists()) && symlink(&tool, &link).is_ok() {
                self.alert_info("The tuplepad command is installed.", &command_line_examples(&link));
                return;
            }
        }
        #[cfg(target_os = "macos")]
        if !self.is_scripted() {
            let quoted = tool.to_string_lossy().replace('\'', "'\\\\''");
            let script = format!(
                "do shell script \"mkdir -p /usr/local/bin && ln -sf '{quoted}' /usr/local/bin/tuplepad\" with prompt \"TuplePad wants to install the tuplepad command in /usr/local/bin.\" with administrator privileges"
            );
            let output = std::process::Command::new("osascript").arg("-e").arg(script).output();
            match output {
                Ok(output) if output.status.success() => self.alert_info("The tuplepad command is installed.", &command_line_examples(Path::new("/usr/local/bin/tuplepad"))),
                Ok(output) if String::from_utf8_lossy(&output.stderr).contains("-128") => {}
                Ok(output) => self.alert("Couldn't install the tuplepad command.", String::from_utf8_lossy(&output.stderr).trim()),
                Err(error) => self.alert("Couldn't install the tuplepad command.", &error.to_string()),
            }
            return;
        }
        self.alert("Couldn't install the tuplepad command.", "None of the usual folders for commands can be written to.");
    }

    fn next_scripted_folder(&self) -> Option<PathBuf> {
        if self.is_scripted() { self.next_answer().map(PathBuf::from) } else { None }
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
                    self.settings.borrow_mut().note_recent(&path);
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

    /// Looks for changes made to open files by other apps: reloads files without unsaved
    /// changes (Undo brings the old text back), flags conflicts and missing files, and pulls new
    /// lines into followed logs. Runs every second and when the window comes to the front.
    pub fn check_disk(&self) {
        let Ok(mut editor) = self.editor.try_borrow_mut() else { return };
        let current = editor.current;
        let mut changed_current = false;
        for index in 0..editor.docs.len() {
            let doc = &mut editor.docs[index];
            if doc.path().is_none() {
                continue;
            }
            let buffer = doc.buffer.clone();
            let modified = buffer.is_modified();
            if doc.follow && !modified {
                match buffer.append_from_disk() {
                    Ok(tuplepad_core::AppendResult::Appended { .. }) => {
                        doc.cursors = vec![tuplepad_core::TextSelection::caret(buffer.end_position())];
                        changed_current |= index == current;
                        continue;
                    }
                    Ok(tuplepad_core::AppendResult::NothingNew) => continue,
                    _ => {}
                }
            }
            match buffer.disk_status() {
                tuplepad_core::DiskStatus::Changed if modified => doc.conflict = Some(Conflict::Changed),
                tuplepad_core::DiskStatus::Changed => {
                    if buffer.reload(doc.cursors.clone()).is_ok() {
                        doc.cursors = doc.cursors.iter().map(|c| tuplepad_core::TextSelection { anchor: buffer.clamp(c.anchor), head: buffer.clamp(c.head) }).collect();
                        doc.conflict = None;
                        changed_current |= index == current;
                    }
                }
                tuplepad_core::DiskStatus::Missing => doc.conflict = Some(Conflict::Missing),
                _ => {
                    if doc.conflict == Some(Conflict::Missing) {
                        doc.conflict = None;
                    }
                }
            }
        }
        if changed_current {
            let follow = editor.doc().follow;
            editor.text_replaced();
            if follow {
                editor.reveal();
            }
        }
        drop(editor);
        self.refresh();
    }

    /// Saves every document with unsaved changes; untitled ones ask where.
    pub fn save_all(&self) {
        let count = self.editor.borrow().docs.len();
        let start = self.editor.borrow().current;
        for index in 0..count {
            let modified = self.editor.borrow().docs.get(index).is_some_and(|d| d.buffer.is_modified());
            if modified {
                self.editor.borrow_mut().select_document(index);
                self.refresh();
                self.save_current(false);
            }
        }
        self.editor.borrow_mut().select_document(start);
    }

    /// Goes back to the file on disk. Undo brings the changes back.
    pub fn revert(&self) {
        let mut editor = self.editor.borrow_mut();
        let cursors = editor.doc().cursors.clone();
        let buffer = editor.buffer();
        match buffer.reload(cursors) {
            Ok(()) => {
                editor.doc_mut().conflict = None;
                editor.text_replaced();
            }
            Err(error) => {
                drop(editor);
                self.alert("TuplePad couldn't read the file.", &error.to_string());
            }
        }
    }

    /// Renames or moves the current document's file, keeping it open.
    pub fn move_current(&self) {
        let (buffer, name, path) = {
            let editor = self.editor.borrow();
            (editor.buffer(), editor.doc().name(), editor.doc().path())
        };
        let Some(path) = path else { return };
        let destination = if self.is_scripted() {
            self.next_answer().map(PathBuf::from)
        } else {
            let mut dialog = rfd::FileDialog::new().set_title("Rename or Move").set_file_name(&name);
            if let Some(folder) = path.parent() {
                dialog = dialog.set_directory(folder);
            }
            dialog.save_file()
        };
        let Some(destination) = destination else { return };
        if destination == path {
            return;
        }
        if path.exists()
            && let Err(error) = move_file(&path, &destination)
        {
            self.alert(&format!("TuplePad couldn't move “{name}”."), &error.to_string());
            return;
        }
        buffer.set_path(Some(destination.to_string_lossy().into_owned()));
        self.settings.borrow_mut().note_recent(&destination);
        let mut editor = self.editor.borrow_mut();
        editor.doc_mut().conflict = None;
        editor.invalidate();
    }

    /// Shows the current file in Finder (or the system's file manager).
    pub fn show_in_file_manager(&self) {
        let Some(path) = self.editor.borrow().doc().path() else { return };
        *self.revealed.borrow_mut() = Some(path.clone());
        if !self.is_scripted() {
            reveal(&path);
        }
    }

    pub fn print(&self) {
        crate::print::print(self);
    }

    /// Closes every document but the current one, asking about unsaved changes.
    pub fn close_others(&self) {
        let keep = self.editor.borrow().doc().id.clone();
        self.close_where(|id| id != keep);
    }

    /// Closes every document, asking about unsaved changes, and leaves a new one.
    pub fn close_all(&self) {
        self.close_where(|_| true);
    }

    fn close_where(&self, close: impl Fn(&str) -> bool) {
        loop {
            let next = self.editor.borrow().docs.iter().position(|d| close(&d.id));
            let Some(index) = next else { break };
            let before = self.editor.borrow().docs.len();
            self.close_document(Some(index));
            let after = self.editor.borrow().docs.len();
            let replaced = before == 1 && after == 1 && self.editor.borrow().docs.iter().all(|d| !close(&d.id));
            if after == before && !replaced {
                // Cancelled.
                break;
            }
            if replaced {
                break;
            }
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

    /// Asks a yes-or-no question; `action` names the button that goes ahead.
    pub fn confirm(&self, title: &str, detail: &str, action: &str) -> bool {
        if self.is_scripted() {
            return self.next_answer().is_some_and(|a| a == action);
        }
        let buttons = rfd::MessageButtons::OkCancelCustom(action.into(), "Cancel".into());
        let result = rfd::MessageDialog::new().set_level(rfd::MessageLevel::Warning).set_title(title).set_description(detail).set_buttons(buttons).show();
        matches!(result, rfd::MessageDialogResult::Custom(ref answer) if answer == action) || result == rfd::MessageDialogResult::Ok
    }

    /// A message that isn't a problem.
    pub fn alert_info(&self, title: &str, detail: &str) {
        eprintln!("{title} {detail}");
        if self.is_scripted() {
            return;
        }
        rfd::MessageDialog::new().set_level(rfd::MessageLevel::Info).set_title(title).set_description(detail).show();
    }

    pub fn alert(&self, title: &str, detail: &str) {
        eprintln!("{title} {detail}");
        if self.is_scripted() {
            return;
        }
        rfd::MessageDialog::new().set_level(rfd::MessageLevel::Warning).set_title(title).set_description(detail).show();
    }

    // MARK: Session

    /// The last things to do before quitting (once, however the app quits).
    pub fn shut_down(&self) {
        if self.shut_down.replace(true) {
            return;
        }
        crate::platform::stop_listening();
        self.save_session();
        self.save_settings();
    }

    pub fn save_session(&self) {
        if let Some(store) = &self.session
            && let Ok(mut editor) = self.editor.try_borrow_mut()
        {
            session::save(store, &mut editor, self.untitled_count.get());
        }
    }
}

/// The `tuplepad` command that comes with the app: in the bundle's Helpers folder on a Mac,
/// otherwise next to the app.
fn bundled_tool() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let folder = exe.parent()?;
    let name = if cfg!(windows) { "tuplepad.exe" } else { "tuplepad" };
    [folder.join("../Helpers").join(name), folder.join(name)].into_iter().find(|p| p.exists()).and_then(|p| p.canonicalize().ok())
}

fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(not(unix))]
    return std::fs::copy(target, link).map(|_| ());
}

fn command_line_examples(link: &Path) -> String {
    format!("It's at {}. In a terminal, try:\n\ntuplepad notes.txt\ntuplepad +42 main.rs\ngit log | tuplepad -", link.display())
}

/// Moves a file, copying it across volumes when a rename can't.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
    }
}

/// Opens the system's file manager with a file selected.
fn reveal(path: &Path) {
    use std::process::Command;
    let result = if cfg!(target_os = "macos") {
        Command::new("open").arg("-R").arg(path).spawn()
    } else if cfg!(windows) {
        Command::new("explorer").arg(format!("/select,{}", path.display())).spawn()
    } else {
        // The freedesktop file-manager interface selects the file; failing that, open its folder.
        let uri = format!("array:string:file://{}", path.display());
        Command::new("dbus-send")
            .args(["--session", "--dest=org.freedesktop.FileManager1", "--type=method_call", "/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems", &uri, "string:"])
            .spawn()
            .or_else(|_| Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn())
    };
    if let Err(error) = result {
        eprintln!("couldn't show {}: {error}", path.display());
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
