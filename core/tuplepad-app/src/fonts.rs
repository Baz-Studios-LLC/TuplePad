//! Choosing the editor's font from what the system has.

use crate::editor::layout::Layouter;

/// The family name SF Mono gets here. macOS lists its upright and italic files under one hidden
/// family with the same style, so the italic one can win; registering them again under our own
/// name, with the right styles, makes the choice reliable.
pub const MAC_SYSTEM_MONO: &str = "TuplePad System Mono";

/// Makes the system's own monospace font available where it needs help (macOS).
pub fn register_system_fonts() {
    if !cfg!(target_os = "macos") {
        return;
    }
    use slint::fontique_011::fontique::{Blob, FontInfoOverride, FontStyle};
    let faces = [
        ("/System/Library/Fonts/SFNSMono.ttf", FontStyle::Normal),
        ("/System/Library/Fonts/SFNSMonoItalic.ttf", FontStyle::Italic),
    ];
    let mut collection = slint::fontique_011::shared_collection();
    for (path, style) in faces {
        let Ok(data) = std::fs::read(path) else { continue };
        let info = FontInfoOverride { family_name: Some(MAC_SYSTEM_MONO), style: Some(style), ..Default::default() };
        collection.register_fonts(Blob::new(std::sync::Arc::new(data)), Some(info));
    }
}

/// The font to use: the one in Settings if the system has it, else the system's monospace font.
pub fn pick(layouter: &mut Layouter, preferred: &str) -> String {
    if !preferred.is_empty() && layouter.has_family(preferred) {
        return preferred.to_string();
    }
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &[MAC_SYSTEM_MONO, "Menlo"]
    } else if cfg!(windows) {
        &["Cascadia Mono", "Consolas", "Courier New"]
    } else {
        &["DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono", "Ubuntu Mono"]
    };
    candidates.iter().find(|name| layouter.has_family(name)).map_or("monospace".into(), |name| name.to_string())
}
