//! An open document: its buffer, carets, scroll position and per-document options.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tuplepad_core::{Indentation, SearchQuery, TextBuffer, TextPos, TextSelection};

pub struct Document {
    /// Stable for the life of the document, and the name of its session snapshot.
    pub id: String,
    pub buffer: Arc<TextBuffer>,
    pub untitled_number: u32,
    /// Size on disk when opened, for the sidebar.
    pub size: u64,
    /// Carets and selections in document order; the last one is the main caret.
    pub cursors: Vec<TextSelection>,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub wrap: bool,
    /// What detection found or the user picked; `None` uses the default from Settings.
    pub indentation: Option<Indentation>,
    /// Seconds since 1970 when the text last changed.
    pub last_edited: f64,
    /// The buffer version last written to the session snapshot.
    pub snapshot_version: Option<u64>,
    /// Keep reading new lines from the end of the file, and stay scrolled there (logs).
    pub follow: bool,
    /// The file changed on disk while the document had unsaved changes, or went missing.
    pub conflict: Option<Conflict>,
    /// Mark All: a search whose matches stay highlighted until cleared.
    pub mark: Option<SearchQuery>,
    /// The file looks binary (spelling isn't checked), and whether its notice is still showing.
    pub binary: bool,
    pub binary_notice: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// Changed by something else while this copy has unsaved changes.
    Changed,
    /// Deleted or moved away.
    Missing,
}

impl Document {
    pub fn new(buffer: Arc<TextBuffer>, untitled_number: u32, size: u64, wrap: bool) -> Self {
        Document {
            id: tuplepad_core::new_document_id(),
            buffer,
            untitled_number,
            size,
            cursors: vec![TextSelection::caret(TextPos::new(0, 0))],
            scroll_x: 0.0,
            scroll_y: 0.0,
            wrap,
            indentation: None,
            last_edited: tuplepad_core::now_seconds(),
            snapshot_version: None,
            follow: false,
            conflict: None,
            mark: None,
            binary: false,
            binary_notice: false,
        }
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.buffer.path().map(PathBuf::from)
    }

    pub fn name(&self) -> String {
        match self.buffer.path() {
            Some(path) => Path::new(&path).file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned()),
            None if self.untitled_number > 1 => format!("Untitled {}", self.untitled_number),
            None => "Untitled".into(),
        }
    }

    /// The first line with text in it, for the sidebar.
    pub fn is_untitled(&self) -> bool {
        self.buffer.path().is_none()
    }
}
