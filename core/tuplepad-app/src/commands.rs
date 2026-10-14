//! Every command, with its menu, title and shortcut. The menus, the editor's shortcut handling
//! and (later) the command palette are all built from this one list.

use slint::{Keys, SharedString};

use crate::MenuEntry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    File,
    Edit,
    Selection,
    View,
    /// Commands without a menu item (buttons, scripts).
    None,
}

pub struct Command {
    pub id: &'static str,
    pub title: &'static str,
    pub menu: Menu,
    /// The shortcut, in Slint's key parts: `Control` is ⌘ on a Mac and Ctrl elsewhere, and
    /// `Meta` is the Mac's Control key.
    pub keys: &'static [&'static str],
    /// A different shortcut on Windows and Linux, if any.
    pub other_keys: Option<&'static [&'static str]>,
    /// Shown with a checkmark when on.
    pub toggle: bool,
}

const fn command(id: &'static str, title: &'static str, menu: Menu, keys: &'static [&'static str]) -> Command {
    Command { id, title, menu, keys, other_keys: None, toggle: false }
}

const fn toggle(id: &'static str, title: &'static str, menu: Menu, keys: &'static [&'static str]) -> Command {
    Command { id, title, menu, keys, other_keys: None, toggle: true }
}

const fn separator(menu: Menu) -> Command {
    Command { id: "", title: "", menu, keys: &[], other_keys: None, toggle: false }
}

pub static COMMANDS: &[Command] = &[
    command("file.new", "New", Menu::File, &["Control", "n"]),
    command("file.open", "Open…", Menu::File, &["Control", "o"]),
    separator(Menu::File),
    command("file.save", "Save", Menu::File, &["Control", "s"]),
    command("file.save-as", "Save As…", Menu::File, &["Control", "Shift", "s"]),
    separator(Menu::File),
    command("file.close", "Close", Menu::File, &["Control", "w"]),
    command("edit.undo", "Undo", Menu::Edit, &["Control", "z"]),
    Command { other_keys: Some(&["Control", "y"]), ..command("edit.redo", "Redo", Menu::Edit, &["Control", "Shift", "z"]) },
    separator(Menu::Edit),
    command("edit.cut", "Cut", Menu::Edit, &["Control", "x"]),
    command("edit.copy", "Copy", Menu::Edit, &["Control", "c"]),
    command("edit.paste", "Paste", Menu::Edit, &["Control", "v"]),
    command("edit.select-all", "Select All", Menu::Edit, &["Control", "a"]),
    separator(Menu::Edit),
    command("edit.indent", "Indent", Menu::Edit, &["Control", "]"]),
    command("edit.outdent", "Outdent", Menu::Edit, &["Control", "["]),
    command("selection.line", "Select Line", Menu::Selection, &["Control", "Shift", "l"]),
    command("selection.next", "Select Next Occurrence", Menu::Selection, &["Control", "d"]),
    separator(Menu::Selection),
    Command {
        other_keys: Some(&["Control", "Alt", "UpArrow"]),
        ..command("selection.caret-above", "Add Caret Above", Menu::Selection, &["Meta", "Shift", "UpArrow"])
    },
    Command {
        other_keys: Some(&["Control", "Alt", "DownArrow"]),
        ..command("selection.caret-below", "Add Caret Below", Menu::Selection, &["Meta", "Shift", "DownArrow"])
    },
    command("selection.single", "Single Caret", Menu::Selection, &[]),
    Command { other_keys: Some(&["Control", "b"]), ..toggle("view.sidebar", "Show Sidebar", Menu::View, &["Control", "Meta", "s"]) },
    separator(Menu::View),
    toggle("view.wrap", "Word Wrap", Menu::View, &["Alt", "z"]),
    toggle("view.invisibles", "Show Invisibles", Menu::View, &[]),
    toggle("view.indent-guides", "Show Indent Guides", Menu::View, &[]),
    toggle("view.current-line", "Highlight Current Line", Menu::View, &[]),
    separator(Menu::View),
    command("view.zoom-in", "Zoom In", Menu::View, &["Control", "Shift?", "="]),
    command("view.zoom-out", "Zoom Out", Menu::View, &["Control", "-"]),
    command("view.zoom-reset", "Actual Size", Menu::View, &["Control", "0"]),
    separator(Menu::View),
    toggle("view.appearance-system", "Match System Appearance", Menu::View, &[]),
    toggle("view.appearance-light", "Light Appearance", Menu::View, &[]),
    toggle("view.appearance-dark", "Dark Appearance", Menu::View, &[]),
    command("view.toggle-theme", "Switch Light and Dark", Menu::None, &[]),
];

const MAC: bool = cfg!(target_os = "macos");

impl Command {
    pub fn is_separator(&self) -> bool {
        self.id.is_empty()
    }

    /// The shortcut on this system.
    pub fn shortcut(&self) -> &'static [&'static str] {
        match self.other_keys {
            Some(other) if !MAC => other,
            _ => self.keys,
        }
    }

    fn slint_keys(&self) -> Keys {
        Keys::from_parts(self.shortcut().iter().copied()).unwrap_or_default()
    }

    /// Whether a key press is this command's shortcut.
    pub fn matches(&self, text: &str, control: bool, alt: bool, shift: bool, meta: bool) -> bool {
        let parts = self.shortcut();
        let Some(&key) = parts.iter().find(|p| !matches!(**p, "Control" | "Alt" | "Shift" | "Meta" | "Shift?" | "Alt?")) else {
            return false;
        };
        let has = |name: &str| parts.contains(&name);
        let shift_ok = has("Shift?") || has("Shift") == shift;
        if has("Control") != control || has("Meta") != meta || (!has("Alt?") && has("Alt") != alt) || !shift_ok {
            return false;
        }
        let pressed = match key {
            "UpArrow" => '\u{F700}'.to_string(),
            "DownArrow" => '\u{F701}'.to_string(),
            other => other.to_string(),
        };
        let text = text.to_lowercase();
        // With Shift held, "=" arrives as "+".
        text == pressed || (pressed == "=" && text == "+")
    }
}

/// The command a key press is the shortcut for.
pub fn for_key(text: &str, control: bool, alt: bool, shift: bool, meta: bool) -> Option<&'static Command> {
    COMMANDS.iter().filter(|c| !c.is_separator()).find(|c| c.matches(text, control, alt, shift, meta))
}

/// Slint's title for a separator entry (an internal placeholder it turns into a separator).
const SEPARATOR_TITLE: &str = "\u{E001}⸺";

/// One menu's entries, with what's enabled and checked right now.
pub fn menu_entries(menu: Menu, enabled: impl Fn(&str) -> bool, checked: impl Fn(&str) -> bool) -> Vec<MenuEntry> {
    COMMANDS
        .iter()
        .filter(|c| c.menu == menu)
        .map(|c| {
            if c.is_separator() {
                return MenuEntry { title: SEPARATOR_TITLE.into(), enabled: true, ..Default::default() };
            }
            MenuEntry {
                id: SharedString::from(c.id),
                title: SharedString::from(c.title),
                shortcut: c.slint_keys(),
                enabled: enabled(c.id),
                checkable: c.toggle,
                checked: c.toggle && checked(c.id),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_match_key_presses() {
        assert_eq!(for_key("z", true, false, false, false).map(|c| c.id), Some("edit.undo"));
        assert_eq!(for_key("Z", true, false, true, false).map(|c| c.id), Some("edit.redo"));
        assert_eq!(for_key("+", true, false, true, false).map(|c| c.id), Some("view.zoom-in"));
        assert_eq!(for_key("=", true, false, false, false).map(|c| c.id), Some("view.zoom-in"));
        assert_eq!(for_key("z", false, false, false, false).map(|c| c.id), None);
        assert!(COMMANDS.iter().filter(|c| !c.is_separator()).all(|c| c.shortcut().is_empty() || Keys::from_parts(c.shortcut().iter().copied()).is_ok()));
        // No two commands share a shortcut.
        let shortcuts: Vec<_> = COMMANDS.iter().filter(|c| !c.shortcut().is_empty()).map(|c| c.shortcut()).collect();
        for (i, a) in shortcuts.iter().enumerate() {
            assert!(!shortcuts[i + 1..].contains(a), "{a:?} is used twice");
        }
    }
}
