//! The Settings window: it shows the current settings and changes them as you click, live.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::app::App;
use crate::settings::Settings;
use crate::{SettingsValues, SettingsWindow, SnippetRow, Theme};

/// Theme ids, in the order of `Themes.ids` in theme.slint.
pub const THEME_IDS: [&str; 5] = ["tuplepad", "midnight", "graphite", "sepia", "high-contrast"];
const LINE_HEIGHTS: [f32; 3] = [1.4, 1.62, 1.85];
const LINE_ENDINGS: [&str; 3] = ["lf", "crlf", "cr"];
const APPEARANCES: [&str; 3] = ["system", "light", "dark"];

/// The font choices: shown name and the family saved in settings ("" for the system's own).
pub fn font_choices(app: &App) -> Vec<(String, String)> {
    let mut families = app.editor.borrow_mut().layouter.monospace_families();
    families.retain(|f| f != crate::fonts::MAC_SYSTEM_MONO);
    let mut choices = vec![("System Default".to_string(), String::new())];
    if cfg!(target_os = "macos") {
        choices.push(("SF Mono".to_string(), crate::fonts::MAC_SYSTEM_MONO.to_string()));
    }
    choices.extend(families.into_iter().map(|f| (f.clone(), f)));
    choices
}

pub fn values(settings: &Settings, fonts: &[(String, String)]) -> SettingsValues {
    let encodings = tuplepad_core::supported_encodings();
    let nearest = |value: f32, options: &[f32]| {
        options.iter().enumerate().min_by(|a, b| (a.1 - value).abs().total_cmp(&(b.1 - value).abs())).map_or(0, |(i, _)| i)
    };
    SettingsValues {
        font_names: ModelRc::new(VecModel::from(fonts.iter().map(|(name, _)| SharedString::from(name.as_str())).collect::<Vec<_>>())),
        font_index: fonts.iter().position(|(_, value)| *value == settings.font_family).unwrap_or(0) as i32,
        font_size: settings.font_size.round() as i32,
        line_height: nearest(settings.line_height, &LINE_HEIGHTS) as i32,
        tab_width: settings.tab_width as i32,
        use_tabs: settings.indent_with_tabs,
        detect_indentation: settings.detect_indentation,
        auto_close: settings.auto_close_brackets,
        smart_highlight: settings.smart_highlight,
        wrap: settings.wrap_by_default,
        invisibles: settings.show_invisibles,
        indent_guides: settings.show_indent_guides,
        current_line: settings.highlight_current_line,
        minimap: settings.show_minimap,
        word_completion: settings.word_completion,
        encoding_index: encodings.iter().position(|e| *e == settings.new_document_encoding).unwrap_or(0) as i32,
        encodings: ModelRc::new(VecModel::from(encodings.into_iter().map(SharedString::from).collect::<Vec<_>>())),
        bom: settings.new_document_bom,
        line_ending: LINE_ENDINGS.iter().position(|e| *e == settings.new_document_line_ending).unwrap_or(0) as i32,
        appearance: APPEARANCES.iter().position(|a| *a == settings.appearance).unwrap_or(0) as i32,
        theme: THEME_IDS.iter().position(|t| *t == settings.theme).unwrap_or(0) as i32,
        mcp: settings.mcp_enabled,
        mcp_http: settings.mcp_http,
        check_updates: settings.check_updates,
    }
}

/// Applies one change from the window.
fn set_bool(settings: &mut Settings, key: &str, value: bool) {
    match key {
        "bom" => settings.new_document_bom = value,
        "use-tabs" => settings.indent_with_tabs = value,
        "detect-indentation" => settings.detect_indentation = value,
        "auto-close" => settings.auto_close_brackets = value,
        "smart-highlight" => settings.smart_highlight = value,
        "wrap" => settings.wrap_by_default = value,
        "invisibles" => settings.show_invisibles = value,
        "indent-guides" => settings.show_indent_guides = value,
        "current-line" => settings.highlight_current_line = value,
        "minimap" => settings.show_minimap = value,
        "word-completion" => settings.word_completion = value,
        "mcp" => settings.mcp_enabled = value,
        "mcp-http" => settings.mcp_http = value,
        "check-updates" => settings.check_updates = value,
        _ => eprintln!("unknown setting {key}"),
    }
}

fn set_int(settings: &mut Settings, fonts: &[(String, String)], key: &str, value: i32) {
    let index = value.max(0) as usize;
    match key {
        "encoding" => {
            if let Some(name) = tuplepad_core::supported_encodings().get(index) {
                settings.new_document_encoding = name.clone();
            }
        }
        "line-ending" => settings.new_document_line_ending = LINE_ENDINGS.get(index).unwrap_or(&"lf").to_string(),
        "tab-width" => settings.tab_width = value.clamp(1, 16) as u32,
        "font" => {
            if let Some((_, family)) = fonts.get(index) {
                settings.font_family = family.clone();
            }
        }
        "font-size" => settings.font_size = value.clamp(8, 48) as f32,
        "line-height" => settings.line_height = LINE_HEIGHTS.get(index).copied().unwrap_or(1.62),
        "appearance" => settings.appearance = APPEARANCES.get(index).unwrap_or(&"system").to_string(),
        "theme" => settings.theme = THEME_IDS.get(index).unwrap_or(&"tuplepad").to_string(),
        _ => eprintln!("unknown setting {key}"),
    }
}

impl App {
    /// Opens the Settings window (creating it the first time), optionally at a tab.
    pub fn show_settings(&self, tab: Option<i32>) {
        let Some(app) = self.me.borrow().upgrade() else { return };
        if self.settings_window.borrow().is_none() {
            let Ok(window) = SettingsWindow::new() else { return };
            window.set_version(crate::VERSION.trim().into());
            {
                let app = app.clone();
                window.on_check_updates(move || app.check_for_updates(true));
            }
            if cfg!(target_os = "macos") {
                window.set_titlebar_height(28.0);
            }
            let fonts = Rc::new(font_choices(&app));
            *self.font_choices.borrow_mut() = fonts.to_vec();
            {
                let app = app.clone();
                window.on_set_bool(move |key, value| {
                    set_bool(&mut app.settings.borrow_mut(), &key, value);
                    app.apply_settings(true);
                    app.refresh();
                });
            }
            {
                let app = app.clone();
                let fonts = fonts.clone();
                window.on_set_int(move |key, value| {
                    set_int(&mut app.settings.borrow_mut(), &fonts, &key, value);
                    app.apply_settings(true);
                    app.refresh();
                });
            }
            {
                let weak = window.as_weak();
                window.on_drag_window(move || {
                    use slint::winit_030::WinitWindowAccessor;
                    if let Some(window) = weak.upgrade() {
                        window.window().with_winit_window(|w| {
                            let _ = w.drag_window();
                        });
                    }
                });
            }
            {
                let app = app.clone();
                window.on_snippet_select(move |index| app.select_snippet(index));
            }
            {
                let app = app.clone();
                window.on_mcp_client_access(move |name, access| {
                    let decision = ["edit", "read", "none"].get(access.max(0) as usize).copied().unwrap_or("none");
                    app.set_mcp_client(&name, Some(decision));
                });
            }
            {
                let app = app.clone();
                window.on_mcp_client_forget(move |name| app.set_mcp_client(&name, None));
            }
            {
                let app = app.clone();
                window.on_copy_text(move |what| app.copy_mcp_setup(&what));
            }
            {
                let app = app.clone();
                window.on_snippet_edited(move || app.snippet_edited());
            }
            {
                let app = app.clone();
                window.on_snippet_add(move || app.add_snippet());
            }
            {
                let app = app.clone();
                window.on_snippet_remove(move || app.remove_snippet());
            }
            let languages: Vec<SharedString> =
                std::iter::once("Any language".to_string()).chain(tuplepad_core::languages().into_iter().map(|l| l.name)).map(SharedString::from).collect();
            window.set_snippet_languages(ModelRc::new(VecModel::from(languages)));
            *self.settings_window.borrow_mut() = Some(window);
            self.show_snippet_list();
            self.select_snippet(0);
        }
        self.apply_settings(false);
        if let Some(window) = self.settings_window.borrow().as_ref() {
            if let Some(tab) = tab {
                window.set_tab(tab);
            }
            let _ = window.show();
            use slint::winit_030::WinitWindowAccessor;
            window.window().with_winit_window(|w| w.focus_window());
        }
    }

    // MARK: Snippets

    fn show_snippet_list(&self) {
        let Some(window) = self.settings_window.borrow().as_ref().map(|w| w.clone_strong()) else { return };
        let names: std::collections::HashMap<String, String> = tuplepad_core::languages().into_iter().map(|l| (l.id, l.name)).collect();
        let rows: Vec<SnippetRow> = self
            .snippets
            .borrow()
            .snippets
            .iter()
            .map(|s| {
                let language = if s.language.is_empty() { "any language".to_string() } else { names.get(&s.language).cloned().unwrap_or_else(|| s.language.clone()) };
                SnippetRow { name: if s.name.is_empty() { "Untitled snippet".into() } else { s.name.clone().into() }, detail: format!("{} · {language}", s.trigger).into() }
            })
            .collect();
        window.set_snippets(ModelRc::new(VecModel::from(rows)));
    }

    pub fn select_snippet(&self, index: i32) {
        let Some(window) = self.settings_window.borrow().as_ref().map(|w| w.clone_strong()) else { return };
        let snippet = usize::try_from(index).ok().and_then(|i| self.snippets.borrow().snippets.get(i).cloned());
        let Some(snippet) = snippet else {
            window.set_snippet_index(-1);
            return;
        };
        let language = tuplepad_core::languages().iter().position(|l| l.id == snippet.language).map_or(0, |i| i + 1);
        window.set_snippet_trigger(snippet.trigger.into());
        window.set_snippet_name(snippet.name.into());
        window.set_snippet_language(language as i32);
        window.set_snippet_body(snippet.body.into());
        window.set_snippet_index(index);
    }

    /// A field changed: the snippet follows, and is saved.
    pub fn snippet_edited(&self) {
        let Some(window) = self.settings_window.borrow().as_ref().map(|w| w.clone_strong()) else { return };
        let Ok(index) = usize::try_from(window.get_snippet_index()) else { return };
        let language = usize::try_from(window.get_snippet_language() - 1).ok().and_then(|i| tuplepad_core::languages().get(i).map(|l| l.id.clone())).unwrap_or_default();
        {
            let mut store = self.snippets.borrow_mut();
            let Some(snippet) = store.snippets.get_mut(index) else { return };
            snippet.trigger = window.get_snippet_trigger().trim().to_string();
            snippet.name = window.get_snippet_name().to_string();
            snippet.language = language;
            snippet.body = window.get_snippet_body().to_string();
            store.save();
        }
        self.show_snippet_list();
    }

    /// For scripted runs: fills in the chosen snippet's fields as if typed.
    pub fn edit_snippet_fields(&self, trigger: Option<&str>, name: Option<&str>, language: Option<i32>, body: Option<&str>) {
        let Some(window) = self.settings_window.borrow().as_ref().map(|w| w.clone_strong()) else { return };
        if let Some(t) = trigger {
            window.set_snippet_trigger(t.into());
        }
        if let Some(n) = name {
            window.set_snippet_name(n.into());
        }
        if let Some(l) = language {
            window.set_snippet_language(l);
        }
        if let Some(b) = body {
            window.set_snippet_body(b.into());
        }
        self.snippet_edited();
    }

    pub fn add_snippet(&self) {
        let index = {
            let mut store = self.snippets.borrow_mut();
            store.snippets.push(crate::snippets::Snippet { trigger: String::new(), name: "New snippet".into(), language: String::new(), body: "$0".into() });
            store.save();
            store.snippets.len() - 1
        };
        self.show_snippet_list();
        self.select_snippet(index as i32);
    }

    pub fn remove_snippet(&self) {
        let Some(window) = self.settings_window.borrow().as_ref().map(|w| w.clone_strong()) else { return };
        let Ok(index) = usize::try_from(window.get_snippet_index()) else { return };
        let count = {
            let mut store = self.snippets.borrow_mut();
            if index < store.snippets.len() {
                store.snippets.remove(index);
                store.save();
            }
            store.snippets.len()
        };
        self.show_snippet_list();
        self.select_snippet(if count == 0 { -1 } else { index.min(count - 1) as i32 });
    }

    pub fn hide_settings(&self) {
        if let Some(window) = self.settings_window.borrow().as_ref() {
            let _ = window.hide();
        }
    }

    /// Whether the Settings window is open and in front.
    pub fn settings_focused(&self) -> bool {
        use slint::winit_030::WinitWindowAccessor;
        self.settings_window.borrow().as_ref().is_some_and(|w| {
            w.window().is_visible() && w.window().with_winit_window(|winit| winit.has_focus()).unwrap_or(false)
        })
    }

    /// Brings the Settings window's controls and colors up to date.
    pub fn refresh_settings_window(&self) {
        let Some(window) = self.settings_window.borrow().as_ref().map(|w| w.clone_strong()) else { return };
        let settings = self.settings.borrow().clone();
        window.set_values(values(&settings, &self.font_choices.borrow()));
        let theme = window.global::<Theme>();
        if let Some(ui) = self.ui.upgrade() {
            let main = ui.global::<Theme>();
            theme.set_appearance(main.get_appearance());
            theme.set_theme(main.get_theme());
            theme.set_mono_family(main.get_mono_family());
        }
        self.show_mcp_settings(&window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_from_the_window() {
        let mut settings = Settings::default();
        let fonts = vec![("System Default".to_string(), String::new()), ("Menlo".to_string(), "Menlo".to_string())];
        set_int(&mut settings, &fonts, "font", 1);
        set_int(&mut settings, &fonts, "line-height", 2);
        set_int(&mut settings, &fonts, "theme", 3);
        set_int(&mut settings, &fonts, "line-ending", 1);
        set_bool(&mut settings, "use-tabs", true);
        assert_eq!((settings.font_family.as_str(), settings.line_height, settings.theme.as_str()), ("Menlo", 1.85, "sepia"));
        assert_eq!((settings.new_document_line_ending.as_str(), settings.indent_with_tabs), ("crlf", true));
        let shown = values(&settings, &fonts);
        assert_eq!((shown.font_index, shown.line_height, shown.theme, shown.line_ending), (1, 2, 3, 1));
    }
}
