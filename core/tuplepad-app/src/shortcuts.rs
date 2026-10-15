//! Help ▸ Keyboard Shortcuts: every command with a shortcut, straight from the command list (so
//! it's never out of date), plus the editing keys that aren't commands, filtered as you type.

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::App;
use crate::commands::{COMMANDS, Menu};
use crate::palette::format_shortcut;
use crate::{ShortcutRow, ShortcutSection, ShortcutsBridge};

const MAC: bool = cfg!(target_os = "macos");

/// The sheet's sections, in menu order, and the menus each gathers.
const SECTIONS: &[(&str, &[Menu])] = &[
    ("FILE", &[Menu::File]),
    ("EDIT", &[Menu::Edit]),
    ("SELECTION", &[Menu::Selection]),
    ("FIND", &[Menu::Find]),
    ("TEXT", &[Menu::Text, Menu::Lines, Menu::Case, Menu::Whitespace]),
    ("TOOLS", &[Menu::Tools, Menu::Compare, Menu::Json, Menu::Xml, Menu::Csv, Menu::Encode, Menu::Decode]),
    ("VIEW", &[Menu::View, Menu::Folding]),
    ("HELP", &[Menu::Help]),
];

/// Keys that work in the editor but aren't menu commands.
fn editing_keys() -> Vec<(&'static str, &'static str)> {
    if MAC {
        vec![
            ("Add a caret", "⌘-click"),
            ("Select a column", "⌥-drag"),
            ("Copy dragged text instead of moving it", "hold ⌥"),
            ("Move a word at a time", "⌥← ⌥→"),
            ("Start or end of the line", "⌘← ⌘→"),
            ("Start or end of the document", "⌘↑ ⌘↓"),
            ("Delete the word before", "⌥⌫"),
            ("Delete to the start of the line", "⌘⌫"),
            ("Indent or outdent", "⇥ ⇧⇥"),
            ("Accept a suggestion", "⇥ ↩"),
            ("Expand a snippet", "trigger ⇥"),
        ]
    } else {
        vec![
            ("Add a caret", "Ctrl+click"),
            ("Select a column", "Alt+drag"),
            ("Copy dragged text instead of moving it", "hold Alt"),
            ("Move a word at a time", "Ctrl+← Ctrl+→"),
            ("Start or end of the line", "Home End"),
            ("Start or end of the document", "Ctrl+Home Ctrl+End"),
            ("Delete the word before", "Ctrl+Backspace"),
            ("Indent or outdent", "Tab Shift+Tab"),
            ("Accept a suggestion", "Tab Enter"),
            ("Expand a snippet", "trigger Tab"),
        ]
    }
}

/// The sections, keeping only rows that match `filter` (in the title, keys or section name).
pub fn sections(filter: &str) -> Vec<(String, Vec<(String, String)>)> {
    let filter = filter.trim().to_lowercase();
    let wanted = |section: &str, title: &str, keys: &str| {
        filter.is_empty() || [section, title, keys].iter().any(|text| text.to_lowercase().contains(&filter))
    };
    let mut sections = Vec::new();
    for (name, menus) in SECTIONS {
        let rows: Vec<(String, String)> = COMMANDS
            .iter()
            .filter(|c| menus.contains(&c.menu) && !c.is_separator() && !c.is_submenu() && !c.shortcut().is_empty())
            .map(|c| (c.title.trim_end_matches('…').to_string(), format_shortcut(c.shortcut())))
            .filter(|(title, keys)| wanted(name, title, keys))
            .collect();
        if !rows.is_empty() {
            sections.push((name.to_string(), rows));
        }
    }
    let editing: Vec<(String, String)> =
        editing_keys().into_iter().filter(|(title, keys)| wanted("EDITING", title, keys)).map(|(t, k)| (t.to_string(), k.to_string())).collect();
    if !editing.is_empty() {
        sections.insert(sections.len().min(1), ("EDITING".to_string(), editing));
    }
    sections
}

/// Splits sections into two columns of about the same height, keeping their order.
fn columns<T: Clone>(sections: &[(T, Vec<(String, String)>)]) -> (Vec<(T, Vec<(String, String)>)>, Vec<(T, Vec<(String, String)>)>) {
    let height = |s: &(T, Vec<(String, String)>)| s.1.len() + 2;
    let total: usize = sections.iter().map(height).sum();
    let mut left_height = 0;
    let mut split = sections.len();
    for (i, section) in sections.iter().enumerate() {
        if left_height + height(section) / 2 >= total / 2 {
            split = i;
            break;
        }
        left_height += height(section);
    }
    let split = split.max(1).min(sections.len());
    (sections[..split].to_vec(), sections[split..].to_vec())
}

impl App {
    fn shortcuts_bridge<T>(&self, f: impl FnOnce(&ShortcutsBridge) -> T) -> Option<T> {
        self.ui.upgrade().map(|ui| f(&ui.global::<ShortcutsBridge>()))
    }

    pub fn show_shortcuts(&self) {
        self.shortcuts_bridge(|b| {
            b.set_filter("".into());
            b.set_open(true);
        });
        self.filter_shortcuts();
    }

    pub fn close_shortcuts(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<ShortcutsBridge>().set_open(false);
            ui.invoke_focus_editor();
        }
    }

    /// The search field changed: show the shortcuts that match.
    pub fn filter_shortcuts(&self) {
        let filter = self.shortcuts_bridge(|b| b.get_filter().to_string()).unwrap_or_default();
        let (left, right) = columns(&sections(&filter));
        let model = |sections: Vec<(String, Vec<(String, String)>)>| {
            let sections: Vec<ShortcutSection> = sections
                .into_iter()
                .map(|(title, rows)| ShortcutSection {
                    title: title.into(),
                    rows: ModelRc::new(VecModel::from(rows.into_iter().map(|(title, keys)| ShortcutRow { title: title.into(), keys: keys.into() }).collect::<Vec<_>>())),
                })
                .collect();
            ModelRc::new(VecModel::from(sections))
        };
        self.shortcuts_bridge(|b| {
            b.set_left(model(left));
            b.set_right(model(right));
        });
    }

    /// For scripted runs.
    pub fn shortcuts_dump(&self) -> serde_json::Value {
        use slint::Model;
        self.shortcuts_bridge(|b| {
            let titles = |m: ModelRc<ShortcutSection>| m.iter().map(|s| s.title.to_string()).collect::<Vec<_>>();
            let rows = |m: ModelRc<ShortcutSection>| m.iter().map(|s| s.rows.row_count()).sum::<usize>();
            let sections = [titles(b.get_left()), titles(b.get_right())].concat();
            serde_json::json!({
                "open": b.get_open(),
                "sections": sections,
                "rows": rows(b.get_left()) + rows(b.get_right()),
            })
        })
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shortcut_is_listed_once() {
        let all = sections("");
        let listed: usize = all.iter().map(|(_, rows)| rows.len()).sum();
        let with_keys = COMMANDS.iter().filter(|c| !c.is_separator() && !c.is_submenu() && c.menu != Menu::None && !c.shortcut().is_empty()).count();
        assert_eq!(listed, with_keys + editing_keys().len());
        assert_eq!(all[1].0, "EDITING");
        // Searching finds by name or by key, and a section's name shows its rows.
        let found = sections("palette");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1[0].0, "Command Palette");
        assert!(sections("find").iter().any(|(name, rows)| name == "FIND" && rows.len() > 5));
        assert!(sections("nothing like this").is_empty());
        // Two columns of about the same height.
        let (left, right) = columns(&all);
        let height = |c: &[(String, Vec<(String, String)>)]| c.iter().map(|s| s.1.len() + 2).sum::<usize>() as i64;
        assert!((height(&left) - height(&right)).abs() < 20, "{} vs {}", height(&left), height(&right));
    }
}
