//! Preferences, saved as JSON in the app's data folder. Scripted runs never save them, and take
//! overrides from `--set key=value`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Empty picks the system's monospace font.
    pub font_family: String,
    pub font_size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    pub tab_width: u32,
    pub indent_with_tabs: bool,
    pub detect_indentation: bool,
    pub auto_close_brackets: bool,
    pub smart_highlight: bool,
    pub wrap_by_default: bool,
    pub show_invisibles: bool,
    pub show_indent_guides: bool,
    pub highlight_current_line: bool,
    /// "system", "light" or "dark".
    pub appearance: String,
    pub sidebar_width: f32,
    pub sidebar_visible: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            font_family: String::new(),
            font_size: 13.0,
            line_height: 1.62,
            tab_width: 4,
            indent_with_tabs: false,
            detect_indentation: true,
            auto_close_brackets: true,
            smart_highlight: true,
            wrap_by_default: false,
            show_invisibles: false,
            show_indent_guides: true,
            highlight_current_line: true,
            appearance: "system".into(),
            sidebar_width: 264.0,
            sidebar_visible: true,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Settings {
        std::fs::read(path).ok().and_then(|data| serde_json::from_slice(&data).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Applies `key=value` overrides (keys as in the JSON file). Returns the keys it didn't know.
    pub fn apply_overrides(&mut self, overrides: &[String]) -> Vec<String> {
        let mut json = serde_json::to_value(&*self).unwrap_or_default();
        let mut unknown = Vec::new();
        for item in overrides {
            let Some((key, value)) = item.split_once('=') else { continue };
            let Some(slot) = json.get_mut(key) else {
                unknown.push(key.to_string());
                continue;
            };
            *slot = serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
        }
        if let Ok(settings) = serde_json::from_value(json) {
            *self = settings;
        }
        unknown
    }

    pub fn indentation(&self) -> tuplepad_core::Indentation {
        if self.indent_with_tabs {
            tuplepad_core::Indentation::Tabs
        } else {
            tuplepad_core::Indentation::Spaces(self.tab_width.max(1))
        }
    }
}

/// Where TuplePad keeps its settings and session: Application Support on a Mac, %APPDATA% on
/// Windows, and the XDG data folder on Linux.
pub fn data_folder() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or(home)
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"))
    };
    base.join("TuplePad")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_and_round_trip() {
        let mut settings = Settings::default();
        let unknown = settings.apply_overrides(&["fontSize=15".into(), "showInvisibles=true".into(), "appearance=dark".into(), "nope=1".into()]);
        assert_eq!(unknown, vec!["nope".to_string()]);
        assert_eq!(settings.font_size, 15.0);
        assert!(settings.show_invisibles);
        assert_eq!(settings.appearance, "dark");
        let folder = std::env::temp_dir().join(format!("tuplepad-settings-{}", std::process::id()));
        let path = folder.join("settings.json");
        settings.save(&path);
        assert_eq!(Settings::load(&path), settings);
        let _ = std::fs::remove_dir_all(folder);
    }
}
