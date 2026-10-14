//! What TuplePad remembers between launches: which documents were open, where their carets and
//! scroll positions were, and the text of anything not saved to a file (in snapshot files next
//! to the session index).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    /// The document that was showing.
    pub selected_id: Option<String>,
    /// How many untitled documents have been numbered so far.
    pub untitled_count: u32,
    pub documents: Vec<DocumentRecord>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentRecord {
    pub id: String,
    pub path: Option<String>,
    /// "Untitled 3" is 3; 0 for documents with a file.
    pub untitled_number: u32,
    /// The text is in a snapshot file (untitled documents and files with unsaved changes).
    pub has_snapshot: bool,
    pub modified: bool,
    pub encoding: String,
    pub has_bom: bool,
    /// "lf", "crlf" or "cr".
    pub line_ending: String,
    /// [anchor line, anchor column, head line, head column] per caret.
    pub cursors: Vec<[u64; 4]>,
    /// The highlighting language, if one was chosen or detected.
    pub language: Option<String>,
    pub word_wrap: bool,
    /// "tabs", or the number of spaces; empty for the default.
    #[serde(default)]
    pub indentation: String,
    #[serde(default)]
    pub bookmarks: Vec<u64>,
    pub scroll_x: f64,
    pub scroll_y: f64,
    /// Seconds since 1970 when the text last changed.
    pub last_edited: f64,
}

/// Reads and writes a session folder: `session.json` plus one `<id>.txt` snapshot per
/// document with unsaved text.
pub struct SessionStore {
    directory: PathBuf,
}

impl SessionStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        SessionStore { directory: directory.into() }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn index_path(&self) -> PathBuf {
        self.directory.join("session.json")
    }

    pub fn snapshot_path(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.txt"))
    }

    /// The saved session, if there is a readable one.
    pub fn load(&self) -> Option<SessionRecord> {
        let data = fs::read(self.index_path()).ok()?;
        serde_json::from_slice(&data).ok()
    }

    /// Writes the index (atomically) and deletes snapshots no document uses any more.
    pub fn save(&self, record: &SessionRecord) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        let json = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
        let temporary = self.directory.join("session.json.tmp");
        fs::write(&temporary, json)?;
        fs::rename(&temporary, self.index_path())?;
        let keep: Vec<String> = record.documents.iter().filter(|d| d.has_snapshot).map(|d| format!("{}.txt", d.id)).collect();
        for entry in fs::read_dir(&self.directory)?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".txt") && !keep.contains(&name) {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}

/// A new document id: unique on this machine, and safe as a file name.
pub fn new_document_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:016x}-{:04x}-{:08x}", nanos as u64, std::process::id() & 0xffff, count)
}

/// Seconds since 1970, for `last_edited`.
pub fn now_seconds() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_loads_and_cleans_up_snapshots() {
        let folder = tempfile::tempdir().unwrap();
        let store = SessionStore::new(folder.path().join("Session"));
        assert_eq!(store.load(), None);
        let kept = DocumentRecord { id: new_document_id(), has_snapshot: true, untitled_number: 1, cursors: vec![[0, 1, 0, 3]], ..Default::default() };
        let stale = new_document_id();
        assert_ne!(kept.id, stale);
        fs::create_dir_all(store.directory()).unwrap();
        fs::write(store.snapshot_path(&kept.id), "keep me").unwrap();
        fs::write(store.snapshot_path(&stale), "closed").unwrap();
        let record = SessionRecord { selected_id: Some(kept.id.clone()), untitled_count: 1, documents: vec![kept.clone()] };
        store.save(&record).unwrap();
        assert_eq!(store.load(), Some(record));
        assert!(store.snapshot_path(&kept.id).exists());
        assert!(!store.snapshot_path(&stale).exists());
    }
}
