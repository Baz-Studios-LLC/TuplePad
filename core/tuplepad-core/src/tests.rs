use crate::*;

fn pos(line: u64, column: u64) -> TextPos {
    TextPos::new(line, column)
}

fn caret(line: u64, column: u64) -> TextSelection {
    TextSelection::caret(pos(line, column))
}

fn range(a: (u64, u64), b: (u64, u64)) -> TextSelection {
    TextSelection { anchor: pos(a.0, a.1), head: pos(b.0, b.1) }
}

fn type_text(buffer: &TextBuffer, selections: Vec<TextSelection>, text: &str) -> Vec<TextSelection> {
    buffer.replace(selections, vec![text.to_string()], EditKind::Typing)
}

#[test]
fn lines_exclude_their_breaks() {
    let buffer = TextBuffer::with_text("one\r\ntwo\nthree\rfour".into());
    assert_eq!(buffer.line_count(), 4);
    assert_eq!(buffer.lines(0, 10), vec!["one", "two", "three", "four"]);
    assert_eq!(buffer.line_length(0), 3);

    let trailing = TextBuffer::with_text("a\n".into());
    assert_eq!(trailing.line_count(), 2);
    assert_eq!(trailing.line(1), "");
}

#[test]
fn columns_count_utf16_units() {
    let buffer = TextBuffer::with_text("a👋b\nzz".into());
    // 👋 is two UTF-16 code units.
    assert_eq!(buffer.line_length(0), 4);
    assert_eq!(buffer.text_in(pos(0, 1), pos(0, 3)), "👋");
    // A column inside the surrogate pair rounds down to before the emoji.
    assert_eq!(buffer.clamp(pos(0, 2)), pos(0, 1));
    assert_eq!(buffer.utf16_offset(pos(1, 1)), 6);
    assert_eq!(buffer.position_at_utf16_offset(6), pos(1, 1));
}

#[test]
fn positions_clamp_to_real_text() {
    let buffer = TextBuffer::with_text("hello\r\nworld".into());
    assert_eq!(buffer.clamp(pos(0, 99)), pos(0, 5));
    assert_eq!(buffer.clamp(pos(9, 0)), pos(1, 0));
}

#[test]
fn deleting_a_line_break_removes_crlf_whole() {
    let buffer = TextBuffer::with_text("ab\r\ncd".into());
    let after = buffer.replace(vec![range((0, 2), (1, 0))], vec![String::new()], EditKind::Deletion);
    assert_eq!(buffer.text(), "abcd");
    assert_eq!(after, vec![caret(0, 2)]);
}

#[test]
fn multi_cursor_typing_shifts_later_carets() {
    let buffer = TextBuffer::with_text("ab\nab\nab".into());
    let after = type_text(&buffer, vec![caret(2, 1), caret(0, 1), caret(1, 1)], "XY");
    assert_eq!(buffer.text(), "aXYb\naXYb\naXYb");
    assert_eq!(after, vec![caret(0, 3), caret(1, 3), caret(2, 3)]);

    // Two carets on the same line.
    let buffer = TextBuffer::with_text("abc".into());
    let after = type_text(&buffer, vec![caret(0, 0), caret(0, 2)], "-");
    assert_eq!(buffer.text(), "-ab-c");
    assert_eq!(after, vec![caret(0, 1), caret(0, 4)]);
}

#[test]
fn per_selection_texts_follow_original_order() {
    let buffer = TextBuffer::with_text("1\n2".into());
    buffer.replace(vec![caret(1, 1), caret(0, 1)], vec!["b".into(), "a".into()], EditKind::Other);
    assert_eq!(buffer.text(), "1a\n2b");
}

#[test]
fn overlapping_selections_merge() {
    let buffer = TextBuffer::with_text("abcdef".into());
    let after = type_text(&buffer, vec![range((0, 1), (0, 4)), range((0, 3), (0, 5)), caret(0, 1)], "_");
    assert_eq!(buffer.text(), "a_f");
    assert_eq!(after, vec![caret(0, 2)]);
}

#[test]
fn typing_run_undoes_as_one_step() {
    let buffer = TextBuffer::new();
    let mut sel = vec![caret(0, 0)];
    for c in ["h", "e", "y"] {
        sel = type_text(&buffer, sel, c);
    }
    assert_eq!(buffer.text(), "hey");
    assert!(buffer.is_modified());
    assert_eq!(buffer.undo(), Some(vec![caret(0, 0)]));
    assert_eq!(buffer.text(), "");
    assert!(!buffer.is_modified());
    assert_eq!(buffer.redo(), Some(vec![caret(0, 3)]));
    assert_eq!(buffer.text(), "hey");
}

#[test]
fn moving_the_caret_starts_a_new_undo_step() {
    let buffer = TextBuffer::with_text("ab".into());
    type_text(&buffer, vec![caret(0, 2)], "c");
    type_text(&buffer, vec![caret(0, 0)], "z"); // non-contiguous: new step
    assert_eq!(buffer.text(), "zabc");
    buffer.undo();
    assert_eq!(buffer.text(), "abc");

    let sel = type_text(&buffer, vec![caret(0, 3)], "d");
    buffer.break_undo_coalescing();
    type_text(&buffer, sel, "e");
    buffer.undo();
    assert_eq!(buffer.text(), "abcd");
}

#[test]
fn backspacing_undoes_as_one_step_and_restores_the_caret() {
    let buffer = TextBuffer::with_text("hello".into());
    let mut cursors = vec![caret(0, 5)];
    for _ in 0..3 {
        let head = cursors[0].head;
        let range = range((0, head.column - 1), (0, head.column));
        cursors = buffer.edit(vec![range], vec![String::new()], EditKind::Deletion, cursors);
    }
    assert_eq!(buffer.text(), "he");
    assert_eq!(buffer.undo(), Some(vec![caret(0, 5)]));
    assert_eq!(buffer.text(), "hello");
}

#[test]
fn newline_ends_an_undo_step() {
    let buffer = TextBuffer::new();
    let sel = type_text(&buffer, vec![caret(0, 0)], "a");
    let sel = type_text(&buffer, sel, "\n");
    type_text(&buffer, sel, "b");
    buffer.undo();
    assert_eq!(buffer.text(), "a\n");
}

#[test]
fn undo_reverts_multi_cursor_edits_exactly() {
    let original = "fn a() {}\nfn b() {}\nfn c() {}";
    let buffer = TextBuffer::with_text(original.into());
    buffer.replace(
        vec![range((0, 0), (0, 2)), range((1, 0), (1, 2)), range((2, 0), (2, 2))],
        vec!["pub fn".into()],
        EditKind::Other,
    );
    assert_eq!(buffer.text(), "pub fn a() {}\npub fn b() {}\npub fn c() {}");
    buffer.undo();
    assert_eq!(buffer.text(), original);
    buffer.redo();
    buffer.undo();
    assert_eq!(buffer.text(), original);
}

#[test]
fn modified_tracks_the_saved_state() {
    let buffer = TextBuffer::with_text("x".into());
    assert!(!buffer.is_modified());
    type_text(&buffer, vec![caret(0, 1)], "y");
    assert!(buffer.is_modified());
    buffer.undo();
    assert!(!buffer.is_modified());
    buffer.redo();
    assert!(buffer.is_modified());
}

#[test]
fn converts_line_endings_as_one_undo_step() {
    let buffer = TextBuffer::with_text("a\r\nb\nc\rd".into());
    assert_eq!(buffer.file_info().line_ending, LineEnding::CrLf);
    buffer.convert_line_endings(LineEnding::Lf, vec![caret(0, 0)]);
    assert_eq!(buffer.text(), "a\nb\nc\nd");
    assert_eq!(buffer.file_info().line_ending, LineEnding::Lf);
    assert_eq!(buffer.line_count(), 4);
    buffer.undo();
    assert_eq!(buffer.text(), "a\r\nb\nc\rd");
    assert_eq!(buffer.file_info().line_ending, LineEnding::CrLf);
}

#[test]
fn detects_line_endings() {
    assert_eq!(TextBuffer::with_text("a\nb\r\nc\n".into()).file_info().line_ending, LineEnding::Lf);
    assert_eq!(TextBuffer::with_text("a\r\nb\r\n".into()).file_info().line_ending, LineEnding::CrLf);
    assert_eq!(TextBuffer::with_text("a\rb\r".into()).file_info().line_ending, LineEnding::Cr);
    assert_eq!(TextBuffer::with_text("one line".into()).file_info().line_ending, LineEnding::Lf);
}

#[test]
fn preview_skips_blank_lines() {
    let buffer = TextBuffer::with_text("\n\n  Shopping list  \n- eggs\n".into());
    assert_eq!(buffer.preview(40), "Shopping list - eggs");
    assert_eq!(buffer.preview(8), "Shopping");
}

#[test]
fn saves_and_reopens_in_the_original_encoding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.txt");
    std::fs::write(&path, b"caf\xe9 cr\xe8me, d\xe9j\xe0 vu\r\n").unwrap();

    let buffer = TextBuffer::open(path.to_string_lossy().into()).unwrap();
    assert_eq!(buffer.file_info().encoding.name, "windows-1252");
    assert_eq!(buffer.line(0), "café crème, déjà vu");

    type_text(&buffer, vec![caret(0, 0)], "¡");
    buffer.save().unwrap();
    assert!(!buffer.is_modified());
    assert_eq!(std::fs::read(&path).unwrap(), b"\xa1caf\xe9 cr\xe8me, d\xe9j\xe0 vu\r\n");
}

#[test]
fn refuses_to_save_unrepresentable_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.txt");
    std::fs::write(&path, b"caf\xe9 cr\xe8me").unwrap();
    let buffer = TextBuffer::open(path.to_string_lossy().into()).unwrap();
    type_text(&buffer, vec![caret(0, 0)], "👋");
    assert!(matches!(buffer.save(), Err(CoreError::Unmappable { .. })));
    // The original file is untouched, and no temporary file is left behind.
    assert_eq!(std::fs::read(&path).unwrap(), b"caf\xe9 cr\xe8me");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

    buffer.set_encoding(TextEncoding::utf8()).unwrap();
    buffer.save().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "👋café crème");
}

#[test]
fn save_as_follows_symlinks_and_keeps_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("real.sh");
    std::fs::write(&target, "echo hi\n").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
    let link = dir.path().join("link.sh");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let buffer = TextBuffer::open(link.to_string_lossy().into()).unwrap();
    type_text(&buffer, vec![caret(0, 7)], "!");
    buffer.save().unwrap();
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "echo hi!\n");
    assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o755);
}

#[test]
fn snapshots_restore_unsaved_work() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, b"caf\xe9\r\n").unwrap();
    let buffer = TextBuffer::open(file.to_string_lossy().into()).unwrap();
    type_text(&buffer, vec![caret(0, 4)], "!");

    let snapshot = dir.path().join("snapshot.txt");
    buffer.write_snapshot(snapshot.to_string_lossy().into()).unwrap();
    // The snapshot doesn't count as saving.
    assert!(buffer.is_modified());
    assert_eq!(std::fs::read(&file).unwrap(), b"caf\xe9\r\n");

    let info = buffer.file_info();
    let restored = TextBuffer::restore(
        snapshot.to_string_lossy().into(),
        info.path.clone(),
        info.encoding.clone(),
        info.line_ending,
        true,
    )
    .unwrap();
    assert_eq!(restored.text(), "café!\r\n");
    assert!(restored.is_modified());
    assert!(!restored.can_undo());
    assert_eq!(restored.file_info(), info);
    restored.save().unwrap();
    assert!(!restored.is_modified());
    assert_eq!(std::fs::read(&file).unwrap(), b"caf\xe9!\r\n");
}

#[test]
fn detects_the_language_when_opening_and_saving() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.rs");
    std::fs::write(&path, "fn a() {}\n").unwrap();
    let buffer = TextBuffer::open(path.to_string_lossy().into()).unwrap();
    assert_eq!(buffer.language().as_deref(), Some("rust"));
    assert!(buffer.highlights(0, 1).iter().any(|s| s.style == HighlightStyle::Keyword));

    let untitled = TextBuffer::with_text("print('hi')\n".into());
    assert_eq!(untitled.language(), None);
    assert!(untitled.highlights(0, 1).is_empty());
    untitled.save_as(dir.path().join("hi.py").to_string_lossy().into()).unwrap();
    assert_eq!(untitled.language().as_deref(), Some("python"));
}

/// After any sequence of edits, the incrementally updated tree must match a fresh parse.
#[test]
fn incremental_highlighting_matches_a_full_parse() {
    let source = "fn main() {\n    let s = \"héllo 👋\";\n    /* note */ call(s, 42);\n}\n\nstruct P { x: i32 }\n";
    let buffer = TextBuffer::with_text(source.into());
    buffer.set_language(Some("rust".into()));

    let check = |label: &str| {
        let fresh = TextBuffer::with_text(buffer.text());
        fresh.set_language(Some("rust".into()));
        let lines = buffer.line_count();
        assert_eq!(buffer.highlights(0, lines), fresh.highlights(0, lines), "after {label}: {:?}", buffer.text());
    };

    type_text(&buffer, vec![caret(0, 0)], "// ");
    check("commenting out a line");
    buffer.replace(vec![range((1, 12), (1, 17))], vec!["wörld\n    more".into()], EditKind::Other);
    check("a multi-line replacement");
    buffer.replace(vec![caret(2, 4), caret(3, 4), caret(4, 0)], vec!["/*".into(), "*/".into(), "x".into()], EditKind::Other);
    check("multi-cursor edits that open and close a comment");
    buffer.replace(vec![range((0, 0), (2, 0))], vec![String::new()], EditKind::Deletion);
    check("deleting lines");
    buffer.undo();
    check("undo");
    buffer.undo();
    check("a second undo");
    buffer.redo();
    check("redo");
    buffer.convert_line_endings(LineEnding::CrLf, vec![caret(0, 0)]);
    check("converting line endings");
    buffer.undo();
    check("undoing the conversion");
}

fn search(pattern: &str, regex: bool) -> SearchQuery {
    SearchQuery { pattern: pattern.into(), case_sensitive: true, whole_word: false, regex }
}

#[test]
fn finds_matches_as_utf16_positions() {
    let buffer = TextBuffer::with_text("👋 cat\r\nconcat cat".into());
    let results = buffer.find_all(search("cat", false), 100).unwrap();
    assert_eq!(results.matches, vec![range((0, 3), (0, 6)), range((1, 3), (1, 6)), range((1, 7), (1, 10))]);
    assert!(!results.truncated);
    let limited = buffer.find_all(search("cat", false), 2).unwrap();
    assert_eq!(limited.matches.len(), 2);
    assert!(limited.truncated);
}

#[test]
fn replace_all_expands_groups_and_undoes_in_one_step() {
    let original = "let a = 1;\nlet bb = 22;\n";
    let buffer = TextBuffer::with_text(original.into());
    let count = buffer.replace_all(search(r"let (\w+) = (\d+);", true), "const $1: i32 = $2;".into(), vec![caret(1, 3)]).unwrap();
    assert_eq!(count, 2);
    assert_eq!(buffer.text(), "const a: i32 = 1;\nconst bb: i32 = 22;\n");
    assert_eq!(buffer.undo(), Some(vec![caret(1, 3)]));
    assert_eq!(buffer.text(), original);

    // Literal mode leaves $ alone.
    let buffer = TextBuffer::with_text("price".into());
    buffer.replace_all(search("price", false), "$5".into(), vec![]).unwrap();
    assert_eq!(buffer.text(), "$5");
}

#[test]
fn single_replacement_checks_the_range_is_still_a_match() {
    let buffer = TextBuffer::with_text("id=7 id=42".into());
    let query = search(r"id=(\d+)", true);
    let second = range((0, 5), (0, 10));
    assert_eq!(buffer.replacement_for(query.clone(), second, "n$1".into()).unwrap().as_deref(), Some("n42"));
    // A range that isn't exactly a match gets nothing.
    assert_eq!(buffer.replacement_for(query.clone(), range((0, 6), (0, 10)), "n$1".into()).unwrap(), None);
    assert!(matches!(buffer.find_all(search("(", true), 10), Err(CoreError::InvalidPattern { .. })));
}

#[test]
fn new_documents_use_the_chosen_format() {
    let utf16 = TextEncoding { name: "UTF-16LE".into(), has_bom: true };
    let buffer = TextBuffer::untitled(utf16.clone(), LineEnding::CrLf);
    assert!(!buffer.is_modified());
    assert_eq!(buffer.file_info().encoding, utf16);
    assert_eq!(buffer.file_info().line_ending, LineEnding::CrLf);
}

#[test]
fn opening_a_missing_file_is_an_error() {
    assert!(matches!(TextBuffer::open("/definitely/not/here.txt".into()), Err(CoreError::Io { .. })));
}

fn ranked(query: &str, candidates: &[&str]) -> Vec<String> {
    fuzzy_rank(query.into(), candidates.iter().map(|c| c.to_string()).collect(), 50)
        .into_iter()
        .map(|hit| candidates[hit.index as usize].to_string())
        .collect()
}

#[test]
fn fuzzy_matching_needs_the_letters_in_order() {
    let commands = ["Duplicate Line", "Delete Line", "Sort Ascending", "Save As…"];
    assert_eq!(ranked("dl", &commands)[..2], ["Delete Line", "Duplicate Line"]);
    assert_eq!(ranked("sortasc", &commands), ["Sort Ascending"]);
    assert_eq!(ranked("sort asc", &commands), ["Sort Ascending"]);
    assert!(ranked("xyz", &commands).is_empty());
    assert!(ranked("enil", &commands).is_empty());
    // An empty query keeps everything, in order.
    assert_eq!(ranked("", &commands), commands);
}

#[test]
fn fuzzy_matching_prefers_word_starts_and_runs() {
    // A whole-word match beats the same letters later on.
    assert_eq!(ranked("save", &["Use Selection for Find and Save", "Save"])[0], "Save");
    // Word starts beat letters buried in a word.
    assert_eq!(ranked("gl", &["Toggle Line", "Go to Line…"])[0], "Go to Line…");
    // camelCase and path segments count as word starts.
    assert_eq!(ranked("tbr", &["tabular.rs", "src/text_buffer.rs"])[0], "src/text_buffer.rs");
    assert_eq!(ranked("tb", &["Settings", "TextBuffer"])[0], "TextBuffer");
    // A contiguous match beats a scattered one.
    assert_eq!(ranked("find", &["Fold Inside Nested Docs", "Find Next"])[0], "Find Next");
    // Letters picked out of the middle of words are noise next to a real match…
    assert_eq!(ranked("tog", &["Chinese Traditional (Big 5)", "Toggle Comment", "Sort Ignoring Case"]), ["Toggle Comment"]);
    // …but still match when nothing better does.
    assert_eq!(ranked("tog", &["Chinese Traditional (Big 5)"]), ["Chinese Traditional (Big 5)"]);
}

#[test]
fn fuzzy_positions_are_utf16_offsets_for_highlighting() {
    let hits = fuzzy_rank("gl".into(), vec!["Go to Line".into()], 10);
    assert_eq!(hits[0].positions, vec![0, 6]);
    // Characters outside the BMP take two UTF-16 units.
    let hits = fuzzy_rank("ab".into(), vec!["😀 a b".into()], 10);
    assert_eq!(hits[0].positions, vec![3, 5]);
    // Matching ignores case.
    let hits = fuzzy_rank("UTF".into(), vec!["utf-8".into()], 10);
    assert_eq!(hits[0].positions, vec![0, 1, 2]);
    assert!(fuzzy_rank("x".into(), vec!["a".into(); 3], 10).is_empty());
    assert_eq!(fuzzy_rank("".into(), vec!["a".into(); 5], 2).len(), 2);
}
