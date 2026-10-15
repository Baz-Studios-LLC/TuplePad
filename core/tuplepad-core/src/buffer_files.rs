//! Keeping a document in step with its file: noticing changes on disk, reloading (as an
//! undoable step), following a growing log, and reopening with a different encoding.

use std::io::{Read, Seek, SeekFrom};
use std::time::SystemTime;

use super::*;

/// What a file on disk looked like when the document last matched it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DiskStamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

impl DiskStamp {
    pub fn of(path: &Path) -> Option<DiskStamp> {
        let metadata = fs::metadata(path).ok()?;
        Some(DiskStamp { len: metadata.len(), modified: metadata.modified().ok() })
    }
}

/// How the file on disk compares to the last time the document matched it.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskStatus {
    /// The document has never been saved to a file.
    NoFile,
    Unchanged,
    /// Something else changed the file.
    Changed,
    /// The file was deleted or moved away.
    Missing,
}

/// The result of pulling new lines from a growing file.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppendResult {
    /// This many lines were added at the end.
    Appended { lines: u64 },
    NothingNew,
    /// The file changed in a way that isn't just new lines at the end (or the document has edits).
    NeedsReload,
}

/// Larger appends are read with a full reload instead.
const MAX_APPEND_BYTES: u64 = 256 * 1024 * 1024;

impl State {
    pub(super) fn stamp_disk(&mut self) {
        self.disk = self.path.as_deref().and_then(DiskStamp::of);
    }

    /// Replaces the whole text with what's on disk, as one undo step, and marks it saved.
    fn reload_from(&mut self, text: String, encoding: TextEncoding, had_errors: bool, cursors: Vec<TextSelection>) {
        let before = self.rope.clone();
        let ending_before = self.line_ending;
        let scan = scan_lines(&text);
        let after = Rope::from_str(&text);
        self.looks_binary = text.as_bytes()[..text.len().min(64 * 1024)].contains(&0);
        self.rope = after.clone();
        self.line_ending = scan.line_ending;
        self.max_line_bytes = scan.max_line_bytes;
        self.encoding = encoding;
        self.text_replaced();
        let restored: Vec<TextSelection> =
            cursors.iter().map(|c| TextSelection { anchor: self.clamp_pos(c.anchor), head: self.clamp_pos(c.head) }).collect();
        let change = Change::Replace { before, after, ending_before, ending_after: self.line_ending };
        self.history.record(vec![change], cursors, restored, EditKind::Other);
        self.mark_saved();
        self.had_decode_errors = had_errors;
        self.stamp_disk();
        self.version += 1;
    }
}

/// Decodes bytes in a chosen encoding, skipping a matching byte-order mark.
fn decode_as(bytes: &[u8], encoding: &TextEncoding) -> CoreResult<(String, TextEncoding, bool)> {
    let resolved = encoding.resolve()?;
    let (bom, body) = match encoding_rs::Encoding::for_bom(bytes) {
        Some((found, length)) if found == resolved => (true, &bytes[length..]),
        _ => (false, bytes),
    };
    let (text, had_errors) = resolved.decode_without_bom_handling(body);
    Ok((text.into_owned(), TextEncoding { name: resolved.name().to_string(), has_bom: bom }, had_errors))
}

#[uniffi::export]
impl TextBuffer {
    /// Compares the file on disk with the last time the document matched it.
    pub fn disk_status(&self) -> DiskStatus {
        let st = self.read();
        let Some(path) = &st.path else { return DiskStatus::NoFile };
        match DiskStamp::of(path) {
            None => DiskStatus::Missing,
            Some(now) if st.disk.is_some_and(|then| then != now) => DiskStatus::Changed,
            Some(_) => DiskStatus::Unchanged,
        }
    }

    /// Points the document at a different file without writing it: for a new file that doesn't
    /// exist yet, or after the file was renamed or moved. Picks a language if it has none.
    pub fn set_path(&self, path: Option<String>) {
        let mut st = self.write();
        st.path = path.map(PathBuf::from);
        st.stamp_disk();
        if st.syntax.is_none() && st.path.is_some() {
            st.detect_language();
        }
        st.version += 1;
    }

    /// "Keep my version": stop reporting the current change on disk.
    pub fn acknowledge_disk_change(&self) {
        self.write().stamp_disk();
    }

    /// Re-reads the file, detecting its encoding again. Undo brings back the previous text.
    pub fn reload(&self, cursors: Vec<TextSelection>) -> CoreResult<()> {
        let mut st = self.write();
        let path = st.path.clone().ok_or(CoreError::NoPath)?;
        let bytes = fs::read(&path)?;
        let decoded = encoding::decode(bytes);
        st.reload_from(decoded.text, decoded.encoding, decoded.had_errors, cursors);
        Ok(())
    }

    /// Re-reads the file's bytes as another encoding (for files that were guessed wrong).
    pub fn reopen_with_encoding(&self, encoding: TextEncoding, cursors: Vec<TextSelection>) -> CoreResult<()> {
        let mut st = self.write();
        let path = st.path.clone().ok_or(CoreError::NoPath)?;
        let bytes = fs::read(&path)?;
        let (text, encoding, had_errors) = decode_as(&bytes, &encoding)?;
        st.reload_from(text, encoding, had_errors, cursors);
        Ok(())
    }

    /// For follow mode: adds lines written to the end of the file since it was read.
    /// Only whole lines are taken, so a half-written line waits for the next check.
    pub fn append_from_disk(&self) -> CoreResult<AppendResult> {
        let mut st = self.write();
        let (Some(path), Some(stamp)) = (st.path.clone(), st.disk) else { return Ok(AppendResult::NeedsReload) };
        // The fast path compares raw bytes, so it's only for UTF-8 files.
        if st.history.is_modified() || st.format_changed || st.encoding.name != "UTF-8" || st.had_decode_errors {
            return Ok(AppendResult::NeedsReload);
        }
        let Some(now) = DiskStamp::of(&path) else { return Ok(AppendResult::NeedsReload) };
        if now.len < stamp.len || now.len - stamp.len > MAX_APPEND_BYTES {
            return Ok(AppendResult::NeedsReload);
        }
        if now.len == stamp.len {
            return Ok(if now == stamp { AppendResult::NothingNew } else { AppendResult::NeedsReload });
        }
        let mut file = File::open(&path)?;
        // Make sure the file grew rather than being rewritten: the bytes just before the old end
        // must still match the end of the document.
        let check = (st.rope.len_bytes() as u64).min(stamp.len).min(4096);
        if check > 0 {
            let mut before = vec![0u8; check as usize];
            file.seek(SeekFrom::Start(stamp.len - check))?;
            file.read_exact(&mut before)?;
            let tail_start = st.rope.len_bytes() - check as usize;
            let tail: Vec<u8> = st.rope.byte_slice(tail_start..).bytes().collect();
            if before != tail {
                return Ok(AppendResult::NeedsReload);
            }
        }
        file.seek(SeekFrom::Start(stamp.len))?;
        let mut bytes = Vec::with_capacity((now.len - stamp.len) as usize);
        file.take(now.len - stamp.len).read_to_end(&mut bytes)?;
        let Some(last_break) = bytes.iter().rposition(|&b| b == b'\n') else { return Ok(AppendResult::NothingNew) };
        let complete = &bytes[..=last_break];
        let (text, _, _) = decode_as(complete, &st.encoding)?;
        let lines = memchr::memchr_iter(b'\n', complete).count() as u64;

        // Mirror the file: the appended text isn't an edit, so it doesn't make the document modified.
        let at = st.rope.len_chars();
        st.replace_chars(at, at, &text);
        st.reparse();
        st.disk = Some(DiskStamp { len: stamp.len + complete.len() as u64, modified: now.modified });
        st.version += 1;
        Ok(AppendResult::Appended { lines })
    }

    /// The first character the encoding can't represent, or nil if the whole text fits.
    pub fn unencodable_character(&self, encoding: TextEncoding) -> CoreResult<Option<String>> {
        let st = self.read();
        match encoding::write_encoded(&mut std::io::sink(), st.rope.chunks(), &encoding) {
            Ok(()) => Ok(None),
            Err(CoreError::Unmappable { character, .. }) => Ok(Some(character)),
            Err(other) => Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(path: &Path) -> Arc<TextBuffer> {
        TextBuffer::open(path.to_string_lossy().into()).unwrap()
    }

    fn caret(line: u64, column: u64) -> TextSelection {
        TextSelection::caret(TextPos::new(line, column))
    }

    #[test]
    fn notices_changes_and_reloads_undoably() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "one\n").unwrap();
        let buffer = open(&path);
        assert_eq!(buffer.disk_status(), DiskStatus::Unchanged);

        fs::write(&path, "one\ntwo, written elsewhere\n").unwrap();
        assert_eq!(buffer.disk_status(), DiskStatus::Changed);
        buffer.reload(vec![caret(0, 3)]).unwrap();
        assert_eq!(buffer.text(), "one\ntwo, written elsewhere\n");
        assert_eq!(buffer.disk_status(), DiskStatus::Unchanged);
        assert!(!buffer.is_modified());

        // Undo brings back what we had, which no longer matches the file.
        assert_eq!(buffer.undo(), Some(vec![caret(0, 3)]));
        assert_eq!(buffer.text(), "one\n");
        assert!(buffer.is_modified());

        fs::remove_file(&path).unwrap();
        assert_eq!(buffer.disk_status(), DiskStatus::Missing);
        assert_eq!(TextBuffer::new().disk_status(), DiskStatus::NoFile);
    }

    #[test]
    fn moves_to_a_new_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("later.rs");
        let buffer = TextBuffer::with_text("fn a() {}".into());
        buffer.set_path(Some(path.to_string_lossy().into()));
        assert_eq!(buffer.language().as_deref(), Some("rust"));
        assert_eq!(buffer.disk_status(), DiskStatus::Missing);
        buffer.save().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "fn a() {}");
        assert_eq!(buffer.disk_status(), DiskStatus::Unchanged);
    }

    #[test]
    fn keeping_my_version_stops_the_reminder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "one").unwrap();
        let buffer = open(&path);
        fs::write(&path, "changed elsewhere").unwrap();
        assert_eq!(buffer.disk_status(), DiskStatus::Changed);
        buffer.acknowledge_disk_change();
        assert_eq!(buffer.disk_status(), DiskStatus::Unchanged);
        assert_eq!(buffer.text(), "one");
    }

    #[test]
    fn saving_updates_the_disk_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "one").unwrap();
        let buffer = open(&path);
        buffer.replace(vec![caret(0, 3)], vec!["!".into()], EditKind::Typing);
        buffer.save().unwrap();
        assert_eq!(buffer.disk_status(), DiskStatus::Unchanged);
    }

    #[test]
    fn follows_a_growing_log_one_whole_line_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        fs::write(&path, "first\n").unwrap();
        let buffer = open(&path);
        assert_eq!(buffer.append_from_disk().unwrap(), AppendResult::NothingNew);

        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"second\nthird, half writ").unwrap();
        file.flush().unwrap();
        assert_eq!(buffer.append_from_disk().unwrap(), AppendResult::Appended { lines: 1 });
        assert_eq!(buffer.text(), "first\nsecond\n");
        assert!(!buffer.is_modified());

        file.write_all("ten 👋\n".as_bytes()).unwrap();
        file.flush().unwrap();
        assert_eq!(buffer.append_from_disk().unwrap(), AppendResult::Appended { lines: 1 });
        assert_eq!(buffer.text(), "first\nsecond\nthird, half written 👋\n");

        // Truncation (log rotation) needs a full reload.
        fs::write(&path, "new log\n").unwrap();
        assert_eq!(buffer.append_from_disk().unwrap(), AppendResult::NeedsReload);
    }

    #[test]
    fn a_rewritten_file_is_reloaded_not_appended_to() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        fs::write(&path, "original\n").unwrap();
        let buffer = open(&path);
        // Longer, but not an append.
        fs::write(&path, "changed elsewhere\n").unwrap();
        assert_eq!(buffer.append_from_disk().unwrap(), AppendResult::NeedsReload);
        assert_eq!(buffer.text(), "original\n");
    }

    #[test]
    fn edits_block_following() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        fs::write(&path, "first\n").unwrap();
        let buffer = open(&path);
        buffer.replace(vec![caret(0, 0)], vec!["x".into()], EditKind::Typing);
        assert_eq!(buffer.append_from_disk().unwrap(), AppendResult::NeedsReload);
    }

    #[test]
    fn reopens_with_another_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latin.txt");
        // "naïve" in Mac Roman (ï is 0x95).
        fs::write(&path, b"na\x95ve").unwrap();
        let buffer = open(&path);
        assert_ne!(buffer.text(), "naïve");
        buffer
            .reopen_with_encoding(TextEncoding { name: "macintosh".into(), has_bom: false }, vec![])
            .unwrap();
        assert_eq!(buffer.text(), "naïve");
        assert_eq!(buffer.file_info().encoding.name, "macintosh");
        assert!(!buffer.is_modified());
    }

    #[test]
    fn checks_whether_text_fits_an_encoding() {
        let buffer = TextBuffer::with_text("café 👋".into());
        let latin = TextEncoding { name: "windows-1252".into(), has_bom: false };
        assert_eq!(buffer.unencodable_character(latin).unwrap().as_deref(), Some("👋"));
        assert_eq!(buffer.unencodable_character(TextEncoding::utf8()).unwrap(), None);
    }
}
