//! The command palette: fuzzy search over every command, open document, theme and recent file.
//! Ranking is the core's `fuzzy_rank`, which also says which letters matched.

use slint::{ModelRc, SharedString, VecModel};

use crate::app::App;
use crate::commands::{COMMANDS, Menu};
use crate::{PaletteItem, TitlePart};

/// How many results show at once.
const SHOWN: u32 = 12;

#[derive(Clone, Debug)]
pub struct Entry {
    /// A command id, or `doc:<index>`, `recent:<index>`, `theme:<id>`.
    pub id: String,
    pub title: String,
    pub detail: String,
    pub trailing: String,
}

/// A shortcut for showing: ⌃⌥⇧⌘K on a Mac, Ctrl+Alt+Shift+K elsewhere.
pub fn format_shortcut(parts: &[&str]) -> String {
    let has = |name: &str| parts.contains(&name);
    let key = parts.iter().find(|p| !matches!(**p, "Control" | "Alt" | "Shift" | "Meta" | "Shift?" | "Alt?")).copied().unwrap_or("");
    let key = match key {
        "UpArrow" => "↑".to_string(),
        "DownArrow" => "↓".to_string(),
        "LeftArrow" => "←".to_string(),
        "RightArrow" => "→".to_string(),
        other => other.to_uppercase(),
    };
    if key.is_empty() {
        return String::new();
    }
    if cfg!(target_os = "macos") {
        let mut out = String::new();
        for (name, symbol) in [("Meta", "⌃"), ("Alt", "⌥"), ("Shift", "⇧"), ("Control", "⌘")] {
            if has(name) {
                out.push_str(symbol);
            }
        }
        out + &key
    } else {
        let mut out = Vec::new();
        for (name, label) in [("Control", "Ctrl"), ("Meta", "Win"), ("Alt", "Alt"), ("Shift", "Shift")] {
            if has(name) {
                out.push(label.to_string());
            }
        }
        out.push(key);
        out.join("+")
    }
}

fn menu_name(menu: Menu) -> &'static str {
    match menu {
        Menu::File => "File",
        Menu::Edit => "Edit",
        Menu::Selection => "Selection",
        Menu::Find => "Find",
        Menu::View => "View",
        Menu::None => "",
    }
}

/// Everything the palette can find: open documents, commands, themes and recent files.
pub fn entries(app: &App) -> Vec<Entry> {
    let mut entries = Vec::new();
    let editor = app.editor.borrow();
    for (index, doc) in editor.docs.iter().enumerate() {
        if index == editor.current {
            continue;
        }
        let folder = doc.path().and_then(|p| p.parent().map(crate::app::tilde)).unwrap_or_else(|| "Not saved".into());
        entries.push(Entry { id: format!("doc:{index}"), title: doc.name(), detail: folder, trailing: "Document".into() });
    }
    for command in COMMANDS.iter().filter(|c| !c.is_separator() && !c.is_submenu()) {
        entries.push(Entry {
            id: command.id.into(),
            title: command.title.trim_end_matches('…').into(),
            detail: menu_name(command.menu).into(),
            trailing: format_shortcut(command.shortcut()),
        });
    }
    for (id, name) in crate::settings_window::THEME_IDS.iter().zip(["TuplePad", "Midnight", "Graphite", "Sepia", "High Contrast"]) {
        entries.push(Entry { id: format!("theme:{id}"), title: format!("Theme: {name}"), detail: "View".into(), trailing: String::new() });
    }
    let open: Vec<_> = editor.docs.iter().filter_map(|d| d.path()).collect();
    for (index, path) in app.settings.borrow().recent_files.iter().enumerate() {
        let path = std::path::Path::new(path);
        if open.iter().any(|p| p == path) {
            continue;
        }
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let folder = path.parent().map(crate::app::tilde).unwrap_or_default();
        entries.push(Entry { id: format!("recent:{index}"), title: name, detail: folder, trailing: "Recent".into() });
    }
    entries
}

/// A title split into runs of matched and unmatched letters (`positions` are UTF-16 offsets).
pub fn title_parts(title: &str, positions: &[u32]) -> Vec<TitlePart> {
    let mut parts: Vec<TitlePart> = Vec::new();
    let mut offset = 0u32;
    for ch in title.chars() {
        let matched = positions.contains(&offset);
        match parts.last_mut() {
            Some(last) if last.matched == matched => {
                let mut text = last.text.to_string();
                text.push(ch);
                last.text = text.into();
            }
            _ => parts.push(TitlePart { text: SharedString::from(ch.to_string()), matched }),
        }
        offset += ch.len_utf16() as u32;
    }
    parts
}

impl App {
    pub fn open_palette(&self) {
        *self.palette_entries.borrow_mut() = entries(self);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_palette_open(true);
        }
        self.palette_query("");
    }

    pub fn close_palette(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_palette_open(false);
            ui.invoke_focus_editor();
        }
    }

    /// Ranks everything against what's typed and shows the best.
    pub fn palette_query(&self, query: &str) {
        let entries = self.palette_entries.borrow();
        let titles: Vec<String> = entries.iter().map(|e| e.title.clone()).collect();
        let hits = tuplepad_core::fuzzy_rank(query.to_string(), titles, SHOWN);
        let shown: Vec<usize> = hits.iter().map(|h| h.index as usize).collect();
        let items: Vec<PaletteItem> = hits
            .iter()
            .map(|hit| {
                let entry = &entries[hit.index as usize];
                PaletteItem {
                    parts: ModelRc::new(VecModel::from(title_parts(&entry.title, &hit.positions))),
                    detail: entry.detail.clone().into(),
                    trailing: entry.trailing.clone().into(),
                }
            })
            .collect();
        *self.palette_shown.borrow_mut() = shown;
        if let Some(ui) = self.ui.upgrade() {
            ui.set_palette_items(ModelRc::new(VecModel::from(items)));
            ui.set_palette_selected(0);
        }
    }

    /// Runs the chosen result.
    pub fn palette_run(&self, index: usize) {
        let id = {
            let shown = self.palette_shown.borrow();
            let entries = self.palette_entries.borrow();
            shown.get(index).and_then(|&i| entries.get(i)).map(|e| e.id.clone())
        };
        self.close_palette();
        let Some(id) = id else { return };
        if let Some(index) = id.strip_prefix("doc:") {
            self.select_document(index.parse().unwrap_or(0));
        } else {
            self.run_command(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_and_matched_parts() {
        let shown = format_shortcut(&["Control", "Shift", "p"]);
        if cfg!(target_os = "macos") {
            assert_eq!(shown, "⇧⌘P");
            assert_eq!(format_shortcut(&["Meta", "Shift", "UpArrow"]), "⌃⇧↑");
        } else {
            assert_eq!(shown, "Ctrl+Shift+P");
        }
        let parts = title_parts("Word Wrap", &[0, 5]);
        let runs: Vec<(String, bool)> = parts.iter().map(|p| (p.text.to_string(), p.matched)).collect();
        assert_eq!(runs, vec![("W".into(), true), ("ord ".into(), false), ("W".into(), true), ("rap".into(), false)]);
    }
}
