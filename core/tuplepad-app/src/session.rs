//! Saving and restoring the open documents between launches. The record format and the session
//! folder are the core's (`SessionStore`); this maps them to the app's documents.

use tuplepad_core::{DocumentRecord, Indentation, LineEnding, SessionRecord, SessionStore, TextBuffer, TextEncoding, TextPos, TextSelection};

use crate::document::Document;
use crate::editor::Editor;

fn line_ending_name(ending: LineEnding) -> &'static str {
    match ending {
        LineEnding::Lf => "lf",
        LineEnding::CrLf => "crlf",
        LineEnding::Cr => "cr",
    }
}

fn line_ending_from(name: &str) -> LineEnding {
    match name {
        "crlf" => LineEnding::CrLf,
        "cr" => LineEnding::Cr,
        _ => LineEnding::Lf,
    }
}

/// Writes the session: snapshots of unsaved text that changed since the last save, then the index.
pub fn save(store: &SessionStore, editor: &mut Editor, untitled_count: u32) {
    let line_height = editor.line_height as f64;
    let mut documents = Vec::with_capacity(editor.docs.len());
    for doc in editor.docs.iter_mut() {
        let buffer = doc.buffer.clone();
        let needs_snapshot = doc.is_untitled() || buffer.is_modified();
        let mut has_snapshot = false;
        if needs_snapshot {
            let version = buffer.version();
            let path = store.snapshot_path(&doc.id);
            if doc.snapshot_version == Some(version) && path.exists() {
                has_snapshot = true;
            } else if std::fs::create_dir_all(store.directory()).is_ok() {
                match buffer.write_snapshot(path.to_string_lossy().into_owned()) {
                    Ok(()) => {
                        doc.snapshot_version = Some(version);
                        has_snapshot = true;
                    }
                    Err(error) => eprintln!("couldn't save the session copy of {}: {error}", doc.name()),
                }
            }
        }
        if !has_snapshot && doc.is_untitled() {
            continue;
        }
        let info = buffer.file_info();
        documents.push(DocumentRecord {
            id: doc.id.clone(),
            path: buffer.path(),
            untitled_number: doc.untitled_number,
            has_snapshot,
            modified: buffer.is_modified(),
            encoding: info.encoding.name,
            has_bom: info.encoding.has_bom,
            line_ending: line_ending_name(info.line_ending).into(),
            cursors: doc.cursors.iter().map(|c| [c.anchor.line, c.anchor.column, c.head.line, c.head.column]).collect(),
            language: buffer.language(),
            word_wrap: doc.wrap,
            indentation: match doc.indentation {
                None => String::new(),
                Some(Indentation::Tabs) => "tabs".into(),
                Some(Indentation::Spaces(width)) => width.to_string(),
            },
            bookmarks: buffer.bookmarks(),
            scroll_x: doc.scroll_x,
            // In rows, so a different font size restores the same place.
            scroll_y: doc.scroll_y / line_height,
            last_edited: doc.last_edited,
        });
    }
    let record = SessionRecord { selected_id: editor.docs.get(editor.current).map(|d| d.id.clone()), untitled_count, documents };
    if let Err(error) = store.save(&record) {
        eprintln!("couldn't save the session: {error}");
    }
}

/// Reopens the documents of the last session. Returns the untitled count it had.
pub fn restore(store: &SessionStore, editor: &mut Editor) -> u32 {
    let Some(record) = store.load() else { return 0 };
    let line_height = editor.line_height as f64;
    let mut selected = 0;
    for r in record.documents {
        let encoding = TextEncoding { name: r.encoding.clone(), has_bom: r.has_bom };
        let buffer = if r.has_snapshot {
            let snapshot = store.snapshot_path(&r.id).to_string_lossy().into_owned();
            TextBuffer::restore(snapshot, r.path.clone(), encoding, line_ending_from(&r.line_ending), r.modified)
        } else if let Some(path) = &r.path {
            TextBuffer::open(path.clone())
        } else {
            continue;
        };
        let buffer = match buffer {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("couldn't reopen {}: {error}", r.path.as_deref().unwrap_or("an untitled document"));
                continue;
            }
        };
        if r.language.is_some() && r.language != buffer.language() {
            buffer.set_language(r.language.clone());
        }
        if !r.bookmarks.is_empty() {
            buffer.set_bookmarks(r.bookmarks.clone());
        }
        let size = r.path.as_ref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
        let mut doc = Document::new(buffer, r.untitled_number, size, r.word_wrap);
        let clamp = |line: u64, column: u64| doc.buffer.clamp(TextPos::new(line, column));
        let cursors: Vec<TextSelection> =
            r.cursors.iter().map(|c| TextSelection { anchor: clamp(c[0], c[1]), head: clamp(c[2], c[3]) }).collect();
        if !cursors.is_empty() {
            doc.cursors = cursors;
        }
        doc.indentation = match r.indentation.as_str() {
            "" => None,
            "tabs" => Some(Indentation::Tabs),
            width => width.parse().ok().map(Indentation::Spaces),
        };
        doc.scroll_x = r.scroll_x;
        doc.scroll_y = r.scroll_y * line_height;
        doc.last_edited = r.last_edited;
        doc.snapshot_version = r.has_snapshot.then(|| doc.buffer.version());
        if record.selected_id.as_deref() == Some(r.id.as_str()) {
            selected = editor.docs.len();
        }
        doc.id = r.id;
        editor.add_document(doc);
    }
    editor.show_document(selected);
    record.untitled_count
}
