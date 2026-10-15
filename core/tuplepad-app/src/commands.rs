//! Every command, with its menu, title and shortcut. The menus, the editor's shortcut handling
//! and (later) the command palette are all built from this one list.

use slint::{Keys, SharedString};

use crate::MenuEntry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    File,
    Edit,
    Selection,
    Find,
    Text,
    /// Text ▸ Lines, Case and Whitespace.
    Lines,
    Case,
    Whitespace,
    Tools,
    /// Tools ▸ Compare, JSON, XML, CSV, Encode and Decode.
    Compare,
    Json,
    Xml,
    Csv,
    Encode,
    Decode,
    View,
    /// View ▸ Code Folding.
    Folding,
    Help,
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

/// Where the file manager is called on each system.
pub const SHOW_IN_FILE_MANAGER: &str = if cfg!(target_os = "macos") {
    "Show in Finder"
} else if cfg!(windows) {
    "Show in File Explorer"
} else {
    "Show in Files"
};

/// What deleting a file is called on each system.
pub const MOVE_TO_TRASH: &str = if cfg!(windows) { "Move to Recycle Bin" } else { "Move to Trash" };

/// Where a submenu goes in its menu. Its items are built separately (see `App::refresh`).
const fn submenu(id: &'static str, menu: Menu) -> Command {
    Command { id, title: "", menu, keys: &[], other_keys: None, toggle: false }
}

pub static COMMANDS: &[Command] = &[
    command("file.new", "New", Menu::File, &["Control", "n"]),
    command("file.open", "Open…", Menu::File, &["Control", "o"]),
    command("file.open-folder", "Open Folder…", Menu::File, &["Control", "Shift", "o"]),
    command("file.quick-open", "Go to File…", Menu::File, &["Control", "p"]),
    submenu("submenu.recent", Menu::File),
    separator(Menu::File),
    command("file.save", "Save", Menu::File, &["Control", "s"]),
    command("file.save-as", "Save As…", Menu::File, &["Control", "Shift", "s"]),
    command("file.save-all", "Save All", Menu::File, &["Control", "Alt", "s"]),
    command("file.revert", "Revert to Saved", Menu::File, &[]),
    separator(Menu::File),
    command("file.move", "Rename or Move…", Menu::File, &[]),
    command("file.show", SHOW_IN_FILE_MANAGER, Menu::File, &[]),
    command("file.copy-path", "Copy Path", Menu::File, &[]),
    command("file.trash", MOVE_TO_TRASH, Menu::File, &[]),
    separator(Menu::File),
    submenu("submenu.encoding", Menu::File),
    separator(Menu::File),
    command("file.print", "Print…", Menu::File, &[]),
    separator(Menu::File),
    command("app.settings", "Settings…", Menu::File, &["Control", ","]),
    command("file.install-tool", "Install Command Line Tool…", Menu::File, &[]),
    separator(Menu::File),
    command("file.close", "Close", Menu::File, &["Control", "w"]),
    command("file.close-others", "Close Others", Menu::File, &["Control", "Alt", "w"]),
    command("file.close-all", "Close All", Menu::File, &[]),
    command("edit.undo", "Undo", Menu::Edit, &["Control", "z"]),
    Command { other_keys: Some(&["Control", "y"]), ..command("edit.redo", "Redo", Menu::Edit, &["Control", "Shift", "z"]) },
    separator(Menu::Edit),
    command("edit.cut", "Cut", Menu::Edit, &["Control", "x"]),
    command("edit.copy", "Copy", Menu::Edit, &["Control", "c"]),
    command("edit.paste", "Paste", Menu::Edit, &["Control", "v"]),
    command("edit.select-all", "Select All", Menu::Edit, &["Control", "a"]),
    command("edit.copy-colors", "Copy with Syntax Colors", Menu::Edit, &["Control", "Alt", "Shift", "c"]),
    command("edit.clipboard-history", "Paste from History…", Menu::Edit, &["Control", "Shift", "v"]),
    separator(Menu::Edit),
    toggle("edit.spelling", "Check Spelling", Menu::Edit, &[]),
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
    command("find.show", "Find…", Menu::Find, &["Control", "f"]),
    Command { other_keys: Some(&["Control", "h"]), ..command("find.replace", "Find and Replace…", Menu::Find, &["Control", "Alt", "f"]) },
    Command { other_keys: Some(&["F3"]), ..command("find.next", "Find Next", Menu::Find, &["Control", "g"]) },
    Command { other_keys: Some(&["Shift", "F3"]), ..command("find.previous", "Find Previous", Menu::Find, &["Control", "Shift", "g"]) },
    command("find.use-selection", "Use Selection for Find", Menu::Find, &["Control", "e"]),
    separator(Menu::Find),
    command("find.count", "Count Matches", Menu::Find, &[]),
    Command { other_keys: Some(&["Control", "Shift", "m"]), ..command("find.mark-all", "Mark All", Menu::Find, &["Control", "Meta", "m"]) },
    command("find.clear-marks", "Clear Marks", Menu::Find, &[]),
    separator(Menu::Find),
    command("search.open-documents", "Find in Open Documents…", Menu::Find, &[]),
    command("search.files", "Find in Files…", Menu::Find, &["Control", "Shift", "f"]),
    command("search.replace-files", "Replace in Files…", Menu::Find, &["Control", "Alt", "Shift", "f"]),
    separator(Menu::Find),
    command("bookmark.toggle", "Toggle Bookmark", Menu::Find, &["Control", "F2"]),
    command("bookmark.next", "Next Bookmark", Menu::Find, &["F2"]),
    command("bookmark.previous", "Previous Bookmark", Menu::Find, &["Shift", "F2"]),
    command("find.bookmark-lines", "Bookmark Matching Lines", Menu::Find, &[]),
    command("bookmark.copy", "Copy Bookmarked Lines", Menu::Find, &[]),
    command("bookmark.delete", "Delete Bookmarked Lines", Menu::Find, &[]),
    command("bookmark.clear", "Clear Bookmarks", Menu::Find, &[]),
    Command { other_keys: Some(&["Control", "g"]), ..command("text.goto", "Go to Line…", Menu::Text, &["Control", "l"]) },
    separator(Menu::Text),
    command("text.language", "Set Language…", Menu::None, &[]),
    Command { other_keys: Some(&["Alt", "c"]), ..command("text.column", "Column Editor…", Menu::Text, &["Control", "Alt", "c"]) },
    command("text.comment", "Toggle Comment", Menu::Text, &["Control", "/"]),
    separator(Menu::Text),
    Command { other_keys: Some(&["Alt", "UpArrow"]), ..command("text.move-up", "Move Line Up", Menu::Text, &["Control", "Alt", "UpArrow"]) },
    Command { other_keys: Some(&["Alt", "DownArrow"]), ..command("text.move-down", "Move Line Down", Menu::Text, &["Control", "Alt", "DownArrow"]) },
    command("text.duplicate", "Duplicate Line", Menu::Text, &["Control", "Shift", "d"]),
    command("text.delete-line", "Delete Line", Menu::Text, &["Control", "Shift", "k"]),
    command("lines.sort-ascending", "Sort Ascending", Menu::Lines, &[]),
    command("lines.sort-descending", "Sort Descending", Menu::Lines, &[]),
    command("lines.sort-case-insensitive", "Sort Ignoring Case", Menu::Lines, &[]),
    command("lines.sort-numeric", "Sort by Number", Menu::Lines, &[]),
    command("lines.reverse", "Reverse", Menu::Lines, &[]),
    separator(Menu::Lines),
    command("lines.remove-duplicates", "Remove Duplicate Lines", Menu::Lines, &[]),
    command("lines.remove-consecutive-duplicates", "Remove Consecutive Duplicates", Menu::Lines, &[]),
    command("lines.remove-empty", "Remove Empty Lines", Menu::Lines, &[]),
    command("lines.remove-blank", "Remove Blank Lines", Menu::Lines, &[]),
    separator(Menu::Lines),
    Command { other_keys: Some(&["Control", "j"]), ..command("lines.join", "Join Lines", Menu::Lines, &["Control", "Meta", "j"]) },
    command("lines.split", "Split Lines at Wrap Width", Menu::Lines, &[]),
    Command { other_keys: Some(&["Control", "Shift", "u"]), ..command("case.upper", "UPPERCASE", Menu::Case, &["Control", "Meta", "u"]) },
    Command { other_keys: Some(&["Control", "u"]), ..command("case.lower", "lowercase", Menu::Case, &["Control", "Meta", "l"]) },
    command("case.title", "Title Case", Menu::Case, &[]),
    command("case.sentence", "Sentence case", Menu::Case, &[]),
    command("case.swap", "sWAP cASE", Menu::Case, &[]),
    command("whitespace.trim-trailing", "Trim Trailing Whitespace", Menu::Whitespace, &[]),
    command("whitespace.trim-leading", "Trim Leading Whitespace", Menu::Whitespace, &[]),
    command("whitespace.trim-both", "Trim Leading and Trailing", Menu::Whitespace, &[]),
    separator(Menu::Whitespace),
    command("whitespace.tabs-to-spaces", "Tabs to Spaces", Menu::Whitespace, &[]),
    command("whitespace.spaces-to-tabs", "Leading Spaces to Tabs", Menu::Whitespace, &[]),
    separator(Menu::Whitespace),
    command("whitespace.indent-spaces", "Convert Indentation to Spaces", Menu::Whitespace, &[]),
    command("whitespace.indent-tabs", "Convert Indentation to Tabs", Menu::Whitespace, &[]),
    command("compare.saved", "With Saved Version", Menu::Compare, &[]),
    command("compare.clipboard", "With Clipboard", Menu::Compare, &[]),
    submenu("submenu.compare-documents", Menu::Compare),
    command("json.format", "Format", Menu::Json, &[]),
    command("json.minify", "Minify", Menu::Json, &[]),
    command("json.sort-keys", "Sort Keys", Menu::Json, &[]),
    command("json.validate", "Validate", Menu::Json, &[]),
    separator(Menu::Json),
    command("json.tree", "Show Tree", Menu::Json, &[]),
    command("json.copy-path", "Copy Path", Menu::Json, &[]),
    command("xml.format", "Format", Menu::Xml, &[]),
    command("xml.linearize", "Linearize", Menu::Xml, &[]),
    command("xml.validate", "Validate", Menu::Xml, &[]),
    separator(Menu::Xml),
    command("xml.xpath", "XPath Query…", Menu::Xml, &[]),
    command("csv.align", "Align Columns", Menu::Csv, &[]),
    command("csv.shrink", "Shrink Columns", Menu::Csv, &[]),
    command("csv.sort", "Sort by Column…", Menu::Csv, &[]),
    separator(Menu::Csv),
    toggle("csv.delimiter-comma", "Comma Delimited", Menu::Csv, &[]),
    toggle("csv.delimiter-tab", "Tab Delimited", Menu::Csv, &[]),
    toggle("csv.delimiter-semicolon", "Semicolon Delimited", Menu::Csv, &[]),
    toggle("csv.delimiter-pipe", "Pipe Delimited", Menu::Csv, &[]),
    command("encode.base64", "Base64", Menu::Encode, &[]),
    command("encode.url", "URL", Menu::Encode, &[]),
    command("encode.html", "HTML Entities", Menu::Encode, &[]),
    command("encode.hex", "Hex", Menu::Encode, &[]),
    command("encode.quoted-printable", "Quoted-Printable", Menu::Encode, &[]),
    command("decode.base64", "Base64", Menu::Decode, &[]),
    command("decode.url", "URL", Menu::Decode, &[]),
    command("decode.html", "HTML Entities", Menu::Decode, &[]),
    command("decode.hex", "Hex", Menu::Decode, &[]),
    command("decode.quoted-printable", "Quoted-Printable", Menu::Decode, &[]),
    command("tools.hashes", "Hashes…", Menu::Tools, &[]),
    separator(Menu::Tools),
    toggle("tools.markdown-preview", "Markdown Preview", Menu::Tools, &["Control", "Shift", "m"]),
    command("tools.hex", "Hex View", Menu::Tools, &[]),
    separator(Menu::Tools),
    command("tools.snippet", "Insert Snippet…", Menu::Tools, &["Control", "Alt", "j"]),
    command("tools.edit-snippets", "Edit Snippets…", Menu::Tools, &[]),
    Command { other_keys: Some(&["Control", "Alt", "r"]), ..command("tools.shell", "Run Shell Command…", Menu::Tools, &["Control", "Meta", "r"]) },
    command("view.palette", "Command Palette…", Menu::View, &["Control", "Shift", "p"]),
    separator(Menu::View),
    Command { other_keys: Some(&["Control", "b"]), ..toggle("view.sidebar", "Show Sidebar", Menu::View, &["Control", "Meta", "s"]) },
    Command { other_keys: Some(&["Control", "Shift", "o"]), ..toggle("view.outline", "Show Outline", Menu::View, &["Control", "Meta", "o"]) },
    toggle("view.minimap", "Show Minimap", Menu::View, &[]),
    toggle("view.split-side", "Split Side by Side", Menu::View, &["Control", "\\"]),
    Command { other_keys: Some(&["Control", "Alt", "\\"]), ..toggle("view.split-stacked", "Split Top and Bottom", Menu::View, &["Control", "Meta", "\\"]) },
    separator(Menu::View),
    toggle("view.wrap", "Word Wrap", Menu::View, &["Alt", "z"]),
    toggle("view.invisibles", "Show Invisibles", Menu::View, &[]),
    toggle("view.indent-guides", "Show Indent Guides", Menu::View, &[]),
    toggle("view.current-line", "Highlight Current Line", Menu::View, &[]),
    toggle("view.follow", "Follow End of File", Menu::View, &[]),
    submenu("submenu.folding", Menu::View),
    Command { other_keys: Some(&["Control", "Shift", "["]), ..command("fold.fold", "Fold", Menu::Folding, &["Control", "Alt", "LeftArrow"]) },
    Command { other_keys: Some(&["Control", "Shift", "]"]), ..command("fold.unfold", "Unfold", Menu::Folding, &["Control", "Alt", "RightArrow"]) },
    separator(Menu::Folding),
    Command { other_keys: Some(&["Alt", "0"]), ..command("fold.all", "Fold All", Menu::Folding, &["Control", "Alt", "Shift", "LeftArrow"]) },
    Command { other_keys: Some(&["Alt", "Shift", "0"]), ..command("fold.unfold-all", "Unfold All", Menu::Folding, &["Control", "Alt", "Shift", "RightArrow"]) },
    separator(Menu::View),
    command("view.zoom-in", "Zoom In", Menu::View, &["Control", "Shift?", "="]),
    command("view.zoom-out", "Zoom Out", Menu::View, &["Control", "-"]),
    command("view.zoom-reset", "Actual Size", Menu::View, &["Control", "0"]),
    separator(Menu::View),
    submenu("submenu.theme", Menu::View),
    separator(Menu::View),
    toggle("view.appearance-system", "Match System Appearance", Menu::View, &[]),
    toggle("view.appearance-light", "Light Appearance", Menu::View, &[]),
    toggle("view.appearance-dark", "Dark Appearance", Menu::View, &[]),
    command("view.toggle-theme", "Switch Light and Dark", Menu::None, &[]),
    Command { other_keys: Some(&["F1"]), ..command("help.shortcuts", "Keyboard Shortcuts", Menu::Help, &["Control", "Alt", "/"]) },
];

const MAC: bool = cfg!(target_os = "macos");

impl Command {
    pub fn is_separator(&self) -> bool {
        self.id.is_empty()
    }

    pub fn is_submenu(&self) -> bool {
        self.id.starts_with("submenu.")
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
            "LeftArrow" => '\u{F702}'.to_string(),
            "RightArrow" => '\u{F703}'.to_string(),
            // Function keys: F1 is U+F704, like AppKit's.
            other if other.len() > 1 && other.starts_with('F') && other[1..].parse::<u32>().is_ok() => {
                char::from_u32(0xF703 + other[1..].parse::<u32>().unwrap_or(0)).map_or(String::new(), |c| c.to_string())
            }
            other => other.to_string(),
        };
        let text = text.to_lowercase();
        // With Shift held, "=" arrives as "+".
        text == pressed || (pressed == "=" && text == "+")
    }
}

/// The command a key press is the shortcut for.
pub fn for_key(text: &str, control: bool, alt: bool, shift: bool, meta: bool) -> Option<&'static Command> {
    COMMANDS.iter().filter(|c| !c.is_separator() && !c.is_submenu()).find(|c| c.matches(text, control, alt, shift, meta))
}

/// Slint's title for a separator entry (an internal placeholder it turns into a separator).
const SEPARATOR_TITLE: &str = "\u{E001}⸺";

pub fn separator_entry() -> MenuEntry {
    MenuEntry { title: SEPARATOR_TITLE.into(), enabled: true, ..Default::default() }
}

/// An entry for a submenu's item (recent files, encodings), which isn't in the command list.
pub fn item_entry(id: String, title: String, enabled: bool, checked: Option<bool>) -> MenuEntry {
    MenuEntry {
        id: id.into(),
        title: title.into(),
        enabled,
        checkable: checked.is_some(),
        checked: checked.unwrap_or(false),
        ..Default::default()
    }
}

/// One menu's entries, with what's enabled and checked right now, in sections split where its
/// submenus go.
pub fn menu_sections(menu: Menu, enabled: impl Fn(&str) -> bool, checked: impl Fn(&str) -> bool) -> Vec<Vec<MenuEntry>> {
    let mut sections = vec![Vec::new()];
    for c in COMMANDS.iter().filter(|c| c.menu == menu) {
        if c.is_submenu() {
            sections.push(Vec::new());
            continue;
        }
        let entry = if c.is_separator() {
            separator_entry()
        } else {
            MenuEntry {
                id: SharedString::from(c.id),
                title: SharedString::from(c.title),
                shortcut: c.slint_keys(),
                enabled: enabled(c.id),
                checkable: c.toggle,
                checked: c.toggle && checked(c.id),
            }
        };
        sections.last_mut().unwrap().push(entry);
    }
    sections
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
        assert_eq!(for_key("\u{F705}", false, false, false, false).map(|c| c.id), Some("bookmark.next"));
        assert_eq!(for_key("\u{F705}", false, false, true, false).map(|c| c.id), Some("bookmark.previous"));
        assert!(COMMANDS.iter().filter(|c| !c.is_separator()).all(|c| c.shortcut().is_empty() || Keys::from_parts(c.shortcut().iter().copied()).is_ok()));
        // No two commands share a shortcut.
        let shortcuts: Vec<_> = COMMANDS.iter().filter(|c| !c.shortcut().is_empty()).map(|c| c.shortcut()).collect();
        assert_eq!(menu_sections(Menu::File, |_| true, |_| false).len(), 3);
        for (i, a) in shortcuts.iter().enumerate() {
            assert!(!shortcuts[i + 1..].contains(a), "{a:?} is used twice");
        }
    }
}
