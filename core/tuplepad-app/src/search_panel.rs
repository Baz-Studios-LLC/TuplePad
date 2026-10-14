//! Find and Replace across open documents or a folder: the search panel docked under the editor.
//! Folder searches run in the core's background walker and are polled every 80 ms; results
//! stream into the list as they come and are sorted by path when the search ends. Replace in
//! Files previews every change first, then changes open documents in their buffers (one undo
//! step each) and rewrites other files after keeping a backup, so each file can be undone.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tuplepad_core::{
    FileMatches, FolderSearch, FolderSearchOptions, FolderSearchProgress, LineMatches, ReplacementPreview, SearchQuery, TextBuffer, TextPos, TextSelection,
    preview_file_replacements, replace_in_file,
};

use crate::app::{App, tilde};
use crate::editor::group;
use crate::{FindBridge, PlanRow, ReplacedRow, SearchBridge, SearchRow, TitlePart};

/// Lines kept per open document.
const DOCUMENT_LINE_LIMIT: u64 = 2_000;
/// Stretches of changed lines previewed per file.
const PREVIEW_LIMIT: u64 = 500;
/// Characters of a line shown in a preview.
const PREVIEW_CHARS: usize = 240;

/// A file (or open document) with matches.
struct FileResult {
    /// The open document the matches came from (when searching open documents).
    doc_id: Option<String>,
    path: Option<PathBuf>,
    title: String,
    location: String,
    lines: Vec<LineMatches>,
    match_count: u64,
    truncated: bool,
    collapsed: bool,
}

struct PlanFile {
    result: usize,
    previews: Vec<ReplacementPreview>,
    included: bool,
}

enum Undo {
    /// An open document, changed in its buffer: undone with the buffer's own Undo if nothing
    /// has changed since.
    Document { doc_id: String, version: u64 },
    /// A file on disk, rewritten after copying it to `backup`.
    File { path: PathBuf, backup: PathBuf, modified: Option<SystemTime> },
}

struct ReplacedFile {
    title: String,
    location: String,
    count: u64,
    undo: Undo,
    undone: bool,
    error: Option<String>,
}

#[derive(Default)]
pub struct SearchState {
    results: Vec<FileResult>,
    /// What each row of the list shows: a file, or one of its lines.
    row_map: Vec<(usize, Option<usize>)>,
    rows: Rc<VecModel<SearchRow>>,
    searched: Option<SearchQuery>,
    searched_folder: Option<PathBuf>,
    /// Where the search looked ("in 3 open documents").
    description: String,
    running: Option<Arc<FolderSearch>>,
    timer: Option<slint::Timer>,
    generation: u64,
    searching: bool,
    stopped: bool,
    progress: Option<FolderSearchProgress>,
    error: Option<String>,
    folder: Option<PathBuf>,
    plan: Option<Vec<PlanFile>>,
    preparing: bool,
    /// What the plan replaces with.
    replacement: String,
    replaced: Vec<ReplacedFile>,
}

/// A line's text with its matches marked, for the results list.
fn marked_parts(line: &LineMatches) -> Vec<TitlePart> {
    let matched_at = |column: u32| line.spans.iter().any(|s| column >= s.start && column < s.end);
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut current = false;
    let mut column = line.preview_start;
    for (shown, ch) in line.preview.chars().enumerate() {
        if shown == PREVIEW_CHARS {
            text.push('…');
            break;
        }
        let matched = matched_at(column);
        if matched != current && !text.is_empty() {
            parts.push(TitlePart { text: std::mem::take(&mut text).into(), matched: current });
        }
        current = matched;
        if ch == '\t' {
            text.push_str("    ");
        } else {
            text.push(ch);
        }
        column += ch.len_utf16() as u32;
    }
    if !text.is_empty() {
        parts.push(TitlePart { text: text.into(), matched: current });
    }
    parts
}

/// A stretch of lines for the replace sheet: tabs as spaces, without the indentation the lines
/// share.
fn preview_lines(text: &str) -> Vec<String> {
    text.trim_end_matches(['\n', '\r']).split('\n').map(|line| line.trim_end_matches('\r').replace('\t', "    ")).collect()
}

fn dedent(lines: &mut [Vec<String>]) {
    let indent = lines
        .iter()
        .flatten()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    for line in lines.iter_mut().flatten() {
        *line = line.get(indent..).unwrap_or("").to_string();
    }
}

/// One line before and after replacing, with what changed marked: the text between their common
/// start and common end. A long unchanged start is cut short so the change stays in view.
fn changed_parts(before: &str, after: &str) -> (Vec<TitlePart>, Vec<TitlePart>) {
    let (b, a): (Vec<char>, Vec<char>) = (before.chars().collect(), after.chars().collect());
    let prefix = b.iter().zip(&a).take_while(|(x, y)| x == y).count();
    let suffix = b[prefix..].iter().rev().zip(a[prefix..].iter().rev()).take_while(|(x, y)| x == y).count();
    let lead: String = if prefix > 60 { std::iter::once('…').chain(b[prefix - 40..prefix].iter().copied()).collect() } else { b[..prefix].iter().collect() };
    let parts = |chars: &[char]| {
        let middle: String = chars[prefix..chars.len() - suffix].iter().collect();
        let tail: String = chars[chars.len() - suffix..].iter().take(PREVIEW_CHARS).collect();
        [(lead.clone(), false), (middle, true), (tail, false)]
            .into_iter()
            .filter(|(text, _)| !text.is_empty())
            .map(|(text, matched)| TitlePart { text: text.into(), matched })
            .collect()
    };
    (parts(&b), parts(&a))
}

fn plural(count: u64, one: &str, many: &str) -> String {
    if count == 1 { format!("1 {one}") } else { format!("{} {many}", group(count)) }
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl App {
    fn search_bridge<T>(&self, f: impl FnOnce(&SearchBridge) -> T) -> Option<T> {
        self.ui.upgrade().map(|ui| f(&ui.global::<SearchBridge>()))
    }

    fn panel_query(&self) -> Option<SearchQuery> {
        self.search_bridge(|b| SearchQuery {
            pattern: b.get_query().to_string(),
            case_sensitive: b.get_case_sensitive(),
            whole_word: b.get_whole_word(),
            regex: b.get_regex(),
        })
        .filter(|q| !q.pattern.is_empty())
    }

    /// Find in Open Documents (scope 0) and Find or Replace in Files (scope 1): opens the panel,
    /// starting from the selection, or from the find bar's text.
    pub fn show_search(&self, scope: i32, replace: bool) {
        let Some(ui) = self.ui.upgrade() else { return };
        let bridge = ui.global::<SearchBridge>();
        bridge.set_rows(ModelRc::from(self.search.borrow().rows.clone()));
        let selected = {
            let editor = self.editor.borrow();
            let buffer = editor.buffer();
            editor
                .doc()
                .cursors
                .last()
                .filter(|c| c.anchor != c.head && c.start().line == c.end().line && c.end().column - c.start().column <= 200)
                .map(|c| buffer.text_in(c.start(), c.end()))
        };
        let find = ui.global::<FindBridge>();
        if !bridge.get_open() && selected.is_some() {
            bridge.set_query(selected.unwrap_or_default().into());
        } else if bridge.get_query().is_empty() && !find.get_query().is_empty() {
            bridge.set_query(find.get_query());
            bridge.set_case_sensitive(find.get_case_sensitive());
            bridge.set_whole_word(find.get_whole_word());
            bridge.set_regex(find.get_regex());
        }
        if !bridge.get_open() {
            let settings = self.settings.borrow();
            bridge.set_include(settings.search_include.clone().into());
            bridge.set_exclude(settings.search_exclude.clone().into());
            bridge.set_panel_height(settings.search_panel_height.max(150.0));
        }
        bridge.set_scope(scope);
        if replace {
            bridge.set_show_replace(true);
        }
        bridge.set_open(true);
        bridge.set_focus_request(bridge.get_focus_request() + 1);
        self.search_scope_changed();
        self.show_search_status();
    }

    pub fn hide_search(&self) {
        self.stop_search();
        self.search_bridge(|b| b.set_open(false));
        if let Some(ui) = self.ui.upgrade() {
            ui.invoke_focus_editor();
        }
    }

    /// Choosing Folder for the first time starts from the current document's folder.
    pub fn search_scope_changed(&self) {
        let folder_scope = self.search_bridge(|b| b.get_scope() == 1).unwrap_or(false);
        if folder_scope && self.search.borrow().folder.is_none() {
            let folder = self
                .editor
                .borrow()
                .doc()
                .path()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .or_else(|| self.settings.borrow().search_folders.first().map(PathBuf::from));
            self.set_search_folder(folder);
        } else {
            self.show_search_folder();
        }
    }

    fn set_search_folder(&self, folder: Option<PathBuf>) {
        // The real path, so results match open documents (which are opened by their real paths).
        let folder = folder.map(|f| std::fs::canonicalize(&f).unwrap_or(f));
        if let Some(folder) = &folder {
            let mut settings = self.settings.borrow_mut();
            let text = folder.to_string_lossy().into_owned();
            settings.search_folders.retain(|f| *f != text);
            settings.search_folders.insert(0, text);
            settings.search_folders.truncate(8);
        }
        self.search.borrow_mut().folder = folder;
        self.save_settings();
        self.show_search_folder();
    }

    fn show_search_folder(&self) {
        let folder = self.search.borrow().folder.as_deref().map(tilde).unwrap_or_default();
        let recent: Vec<SharedString> = self.settings.borrow().search_folders.iter().map(|f| tilde(Path::new(f)).into()).collect();
        self.search_bridge(|b| {
            b.set_folder(folder.into());
            b.set_recent_folders(ModelRc::from(Rc::new(VecModel::from(recent))));
        });
    }

    pub fn choose_search_folder(&self) {
        let start = self.search.borrow().folder.clone();
        let folder = if self.is_scripted() {
            self.next_scripted_folder()
        } else {
            let mut dialog = rfd::FileDialog::new().set_title("Choose a folder to search");
            if let Some(start) = start {
                dialog = dialog.set_directory(start);
            }
            dialog.pick_folder()
        };
        if folder.is_some() {
            self.set_search_folder(folder);
        }
    }

    pub fn pick_search_folder(&self, index: usize) {
        let folder = self.settings.borrow().search_folders.get(index).map(PathBuf::from);
        if folder.is_some() {
            self.set_search_folder(folder);
        }
    }

    /// The scripted runs' way to point the search at a folder.
    pub fn set_search_folder_path(&self, folder: &Path) {
        self.set_search_folder(Some(folder.to_path_buf()));
    }

    /// Whether a search or a replace preview is still being worked out.
    pub fn search_busy(&self) -> bool {
        let search = self.search.borrow();
        search.searching || search.preparing
    }

    // MARK: Searching

    pub fn start_search(&self) {
        self.stop_search();
        let Some(ui) = self.ui.upgrade() else { return };
        let bridge = ui.global::<SearchBridge>();
        {
            let mut search = self.search.borrow_mut();
            search.stopped = false;
            search.error = None;
            search.replaced.clear();
            search.results.clear();
            search.row_map.clear();
            search.rows.set_vec(Vec::new());
            search.progress = None;
            search.generation += 1;
            search.searched = None;
        }
        bridge.set_selected(-1);
        bridge.set_replaced(ModelRc::default());
        let Some(query) = self.panel_query() else {
            self.show_search_status();
            return;
        };
        self.search.borrow_mut().searched = Some(query.clone());
        if bridge.get_scope() == 1 {
            let (include, exclude) = (bridge.get_include().to_string(), bridge.get_exclude().to_string());
            {
                let mut settings = self.settings.borrow_mut();
                settings.search_include = include.clone();
                settings.search_exclude = exclude.clone();
            }
            self.save_settings();
            self.search_folder(query, include, exclude, bridge.get_search_everything());
        } else {
            self.search_open_documents(query);
        }
        self.show_search_status();
    }

    fn search_open_documents(&self, query: SearchQuery) {
        let docs: Vec<(String, Arc<TextBuffer>, String, String, Option<PathBuf>)> = self
            .editor
            .borrow()
            .docs
            .iter()
            .map(|d| {
                let location = d.path().and_then(|p| p.parent().map(tilde)).unwrap_or_else(|| "Not saved".into());
                (d.id.clone(), d.buffer.clone(), d.name(), location, d.path())
            })
            .collect();
        let generation = {
            let mut search = self.search.borrow_mut();
            search.description = format!("in {}", plural(docs.len() as u64, "open document", "open documents"));
            search.searching = true;
            search.generation
        };
        std::thread::spawn(move || {
            for (doc_id, buffer, title, location, path) in docs {
                let found = buffer.find_lines(query.clone(), DOCUMENT_LINE_LIMIT);
                let stop = found.is_err();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(app) = crate::platform::app() else { return };
                    if app.search.borrow().generation != generation {
                        return;
                    }
                    match found {
                        Ok(found) if found.match_count > 0 => app.add_search_result(FileResult {
                            doc_id: Some(doc_id),
                            path,
                            title,
                            location,
                            lines: found.lines,
                            match_count: found.match_count,
                            truncated: found.truncated,
                            collapsed: false,
                        }),
                        Ok(_) => {}
                        Err(error) => app.search.borrow_mut().error = Some(error.to_string()),
                    }
                    app.show_search_status();
                });
                if stop {
                    break;
                }
            }
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = crate::platform::app() else { return };
                if app.search.borrow().generation == generation {
                    app.search.borrow_mut().searching = false;
                    app.show_search_status();
                }
            });
        });
    }

    fn search_folder(&self, query: SearchQuery, include: String, exclude: String, search_everything: bool) {
        let Some(folder) = self.search.borrow().folder.clone() else {
            self.search.borrow_mut().error = Some("Choose a folder to search.".into());
            return;
        };
        let options = FolderSearchOptions { folder: folder.to_string_lossy().into_owned(), query, include, exclude, search_everything };
        match FolderSearch::start(options) {
            Ok(running) => {
                let timer = slint::Timer::default();
                let generation = {
                    let mut search = self.search.borrow_mut();
                    search.description = format!("in {}", folder.file_name().map_or_else(|| tilde(&folder), |n| n.to_string_lossy().into_owned()));
                    search.searched_folder = Some(folder);
                    search.running = Some(running);
                    search.searching = true;
                    search.generation
                };
                timer.start(slint::TimerMode::Repeated, Duration::from_millis(80), move || {
                    if let Some(app) = crate::platform::app() {
                        app.poll_search(generation);
                    }
                });
                self.search.borrow_mut().timer = Some(timer);
            }
            Err(error) => self.search.borrow_mut().error = Some(error.to_string()),
        }
    }

    fn poll_search(&self, generation: u64) {
        let Some(running) = self.search.borrow().running.clone().filter(|_| self.search.borrow().generation == generation) else { return };
        let found = running.take_results();
        let progress = running.progress();
        let root = self.search.borrow().searched_folder.clone();
        for file in found {
            self.add_search_result(Self::file_result(file, root.as_deref()));
        }
        let finished = progress.finished;
        self.search.borrow_mut().progress = Some(progress);
        if finished {
            {
                let mut search = self.search.borrow_mut();
                search.running = None;
                search.timer = None;
                search.searching = false;
            }
            self.sort_search_results();
        }
        self.show_search_status();
    }

    fn file_result(file: FileMatches, root: Option<&Path>) -> FileResult {
        let path = PathBuf::from(&file.path);
        let title = path.file_name().map_or_else(|| file.path.clone(), |n| n.to_string_lossy().into_owned());
        let folder = path.parent().unwrap_or(Path::new(""));
        let location = match root.and_then(|r| folder.strip_prefix(r).ok()) {
            Some(relative) => relative.to_string_lossy().into_owned(),
            None => tilde(folder),
        };
        FileResult { doc_id: None, path: Some(path), title, location, lines: file.lines, match_count: file.match_count, truncated: file.truncated, collapsed: false }
    }

    /// Adds a file's rows at the end of the list (sorted when the search ends).
    fn add_search_result(&self, result: FileResult) {
        let mut search = self.search.borrow_mut();
        let index = search.results.len();
        let mut rows = vec![Self::file_row(&result)];
        search.row_map.push((index, None));
        for (line_index, line) in result.lines.iter().enumerate() {
            rows.push(Self::line_row(line));
            search.row_map.push((index, Some(line_index)));
        }
        search.results.push(result);
        search.rows.extend(rows);
    }

    fn file_row(result: &FileResult) -> SearchRow {
        SearchRow {
            kind: 0,
            title: result.title.clone().into(),
            location: result.location.clone().into(),
            count: format!("{}{}", group(result.match_count), if result.truncated { "+" } else { "" }).into(),
            parts: ModelRc::default(),
            collapsed: result.collapsed,
        }
    }

    fn line_row(line: &LineMatches) -> SearchRow {
        SearchRow {
            kind: 1,
            title: SharedString::default(),
            location: SharedString::default(),
            count: group(line.line + 1).into(),
            parts: ModelRc::from(Rc::new(VecModel::from(marked_parts(line)))),
            collapsed: false,
        }
    }

    /// Sorts the files by path, keeping the selected row selected.
    fn sort_search_results(&self) {
        let selected = self.search_bridge(|b| b.get_selected()).unwrap_or(-1);
        let selected_key = {
            let search = self.search.borrow();
            usize::try_from(selected).ok().and_then(|i| search.row_map.get(i)).map(|&(file, line)| (search.results[file].path.clone(), search.results[file].title.clone(), line))
        };
        {
            let mut search = self.search.borrow_mut();
            let key = |r: &FileResult| format!("{}/{}", r.location, r.title).to_lowercase();
            if search.results.windows(2).all(|w| key(&w[0]) <= key(&w[1])) {
                return;
            }
            search.results.sort_by_cached_key(key);
        }
        self.rebuild_search_rows();
        if let Some((path, title, line)) = selected_key {
            let search = self.search.borrow();
            let row = search.row_map.iter().position(|&(file, l)| search.results[file].path == path && search.results[file].title == title && l == line);
            drop(search);
            self.search_bridge(|b| b.set_selected(row.map_or(-1, |r| r as i32)));
        }
    }

    fn rebuild_search_rows(&self) {
        let mut search = self.search.borrow_mut();
        let mut rows = Vec::new();
        let mut map = Vec::new();
        for (index, result) in search.results.iter().enumerate() {
            rows.push(Self::file_row(result));
            map.push((index, None));
            if !result.collapsed {
                for (line_index, line) in result.lines.iter().enumerate() {
                    rows.push(Self::line_row(line));
                    map.push((index, Some(line_index)));
                }
            }
        }
        search.row_map = map;
        search.rows.set_vec(rows);
    }

    pub fn stop_search(&self) {
        let mut search = self.search.borrow_mut();
        if search.searching {
            search.stopped = true;
        }
        if let Some(running) = search.running.take() {
            running.cancel();
        }
        search.timer = None;
        search.searching = false;
        search.generation += 1;
        drop(search);
        self.sort_search_results();
        self.show_search_status();
    }

    /// A row was chosen with the arrow keys (`how` 0), clicked (1) or opened (2, Return or a
    /// double-click): a line shows its first match in the editor, which takes the keyboard when
    /// opened; clicking or opening a file folds or unfolds its lines.
    pub fn activate_search_row(&self, index: i32, how: i32) {
        let focus_editor = how == 2;
        let entry = usize::try_from(index).ok().and_then(|i| self.search.borrow().row_map.get(i).copied());
        let Some((file, line)) = entry else { return };
        let Some(line) = line else {
            if how != 0 {
                {
                    let mut search = self.search.borrow_mut();
                    let result = &mut search.results[file];
                    result.collapsed = !result.collapsed;
                }
                self.rebuild_search_rows();
            }
            return;
        };
        let (doc_id, path, hit) = {
            let search = self.search.borrow();
            let result = &search.results[file];
            (result.doc_id.clone(), result.path.clone(), result.lines[line].clone())
        };
        let index = doc_id.and_then(|id| self.editor.borrow().docs.iter().position(|d| d.id == id));
        match (index, path) {
            (Some(index), _) => self.editor.borrow_mut().select_document(index),
            (None, Some(path)) => {
                if self.open_paths(std::slice::from_ref(&path)) == 0 && !self.editor.borrow().docs.iter().any(|d| d.path().as_deref() == Some(path.as_path())) {
                    return;
                }
            }
            (None, None) => return,
        }
        {
            let mut editor = self.editor.borrow_mut();
            let buffer = editor.buffer();
            let span = hit.spans.first().map_or((0, 0), |s| (s.start as u64, s.end as u64));
            let anchor = buffer.clamp(TextPos::new(hit.line, span.0));
            let head = buffer.clamp(TextPos::new(hit.line, span.1));
            editor.doc_mut().cursors = vec![TextSelection { anchor, head }];
            editor.reveal_centered();
            editor.touch();
        }
        self.refresh();
        if focus_editor && let Some(ui) = self.ui.upgrade() {
            ui.invoke_focus_editor();
        }
    }

    // MARK: Replace in Files

    /// Works out every change Replace in Files would make, for the preview sheet.
    pub fn prepare_replace(&self) {
        let Some(query) = self.search.borrow().searched.clone() else { return };
        if self.search.borrow().results.is_empty() || self.search.borrow().preparing {
            return;
        }
        let replacement = self.search_bridge(|b| b.get_replacement().to_string()).unwrap_or_default();
        enum Source {
            Buffer(Arc<TextBuffer>),
            File(String),
        }
        let sources: Vec<(usize, Source)> = {
            let search = self.search.borrow();
            let editor = self.editor.borrow();
            search
                .results
                .iter()
                .enumerate()
                .filter_map(|(index, result)| {
                    let doc = editor.docs.iter().find(|d| result.doc_id.as_ref() == Some(&d.id) || (result.path.is_some() && d.path() == result.path));
                    match (doc, &result.path) {
                        (Some(doc), _) => Some((index, Source::Buffer(doc.buffer.clone()))),
                        (None, Some(path)) => Some((index, Source::File(path.to_string_lossy().into_owned()))),
                        (None, None) => None,
                    }
                })
                .collect()
        };
        let generation = {
            let mut search = self.search.borrow_mut();
            search.preparing = true;
            search.replacement = replacement.clone();
            search.generation
        };
        self.search_bridge(|b| b.set_preparing(true));
        std::thread::spawn(move || {
            let mut plan = Vec::new();
            let mut errors = Vec::new();
            for (index, source) in sources {
                let previews = match source {
                    Source::Buffer(buffer) => buffer.replacement_preview(query.clone(), replacement.clone(), PREVIEW_LIMIT),
                    Source::File(path) => preview_file_replacements(path, query.clone(), replacement.clone(), PREVIEW_LIMIT),
                };
                match previews {
                    Ok(previews) if !previews.is_empty() => plan.push((index, previews)),
                    Ok(_) => {}
                    Err(error) => errors.push(error.to_string()),
                }
            }
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = crate::platform::app() else { return };
                {
                    let mut search = app.search.borrow_mut();
                    search.preparing = false;
                    if search.generation != generation {
                        return;
                    }
                    search.error = errors.into_iter().next();
                    search.plan = Some(plan.into_iter().map(|(result, previews)| PlanFile { result, previews, included: true }).collect());
                }
                app.search_bridge(|b| b.set_preparing(false));
                app.show_plan();
                app.show_search_status();
            });
        });
    }

    fn show_plan(&self) {
        let search = self.search.borrow();
        let Some(plan) = &search.plan else {
            self.search_bridge(|b| b.set_plan_open(false));
            return;
        };
        let mut rows = Vec::new();
        let (mut files, mut matches) = (0, 0);
        for (index, file) in plan.iter().enumerate() {
            let result = &search.results[file.result];
            if file.included {
                files += 1;
                matches += result.match_count;
            }
            rows.push(PlanRow {
                kind: 0,
                title: result.title.clone().into(),
                location: result.location.clone().into(),
                count: plural(result.match_count, "change", "changes").into(),
                included: file.included,
                line: index.to_string().into(),
                ..Default::default()
            });
            for preview in &file.previews {
                let mut lines = [preview_lines(&preview.before), preview_lines(&preview.after)];
                dedent(&mut lines);
                let [before, after] = lines;
                let count = before.len().max(after.len());
                let (before_parts, after_parts) = if count == 1 {
                    changed_parts(&before[0], &after[0])
                } else {
                    let whole = |lines: &[String]| vec![TitlePart { text: lines.join("\n").into(), matched: false }];
                    (whole(&before), whole(&after))
                };
                rows.push(PlanRow {
                    kind: 1,
                    included: file.included,
                    line: group(preview.line + 1).into(),
                    before: before.join("\n").into(),
                    after: after.join("\n").into(),
                    before_parts: ModelRc::from(Rc::new(VecModel::from(before_parts))),
                    after_parts: ModelRc::from(Rc::new(VecModel::from(after_parts))),
                    lines: count as i32,
                    ..Default::default()
                });
            }
        }
        let query = search.searched.as_ref().map(|q| q.pattern.clone()).unwrap_or_default();
        let title = format!("Replace “{query}” with “{}”", search.replacement);
        let summary = if plan.is_empty() {
            "Nothing would change.".to_string()
        } else {
            format!("{} in {}. Uncheck any file to leave it alone.", plural(matches, "change", "changes"), plural(files, "file", "files"))
        };
        drop(search);
        self.search_bridge(|b| {
            b.set_plan(ModelRc::from(Rc::new(VecModel::from(rows))));
            b.set_plan_title(title.into());
            b.set_plan_summary(summary.into());
            b.set_plan_open(true);
        });
    }

    /// A plan row's checkbox: plan rows for files carry their index in `line`.
    pub fn toggle_plan_file(&self, row: i32) {
        let index = self.search_bridge(|b| b.get_plan().row_data(row as usize)).flatten().filter(|r| r.kind == 0).and_then(|r| r.line.parse::<usize>().ok());
        let Some(index) = index else { return };
        if let Some(file) = self.search.borrow_mut().plan.as_mut().and_then(|p| p.get_mut(index)) {
            file.included = !file.included;
        }
        self.show_plan();
    }

    /// For scripted runs: the plan row of a file, by name.
    pub fn plan_row_of(&self, title: &str) -> Option<i32> {
        self.search_bridge(|b| b.get_plan().iter().position(|r| r.kind == 0 && r.title == title)).flatten().map(|i| i as i32)
    }

    pub fn cancel_plan(&self) {
        self.search.borrow_mut().plan = None;
        self.search_bridge(|b| b.set_plan_open(false));
    }

    /// Replaces in every file left checked.
    pub fn apply_plan(&self) {
        let (plan, query, replacement) = {
            let mut search = self.search.borrow_mut();
            let (Some(plan), Some(query)) = (search.plan.take(), search.searched.clone()) else { return };
            (plan, query, search.replacement.clone())
        };
        self.search_bridge(|b| b.set_plan_open(false));
        let backups = std::env::temp_dir().join(format!("TuplePad Replace {}", tuplepad_core::new_document_id()));
        let mut done = Vec::new();
        let mut failures = Vec::new();
        for file in plan.into_iter().filter(|f| f.included) {
            let (doc_id, path, title, location) = {
                let search = self.search.borrow();
                let r = &search.results[file.result];
                (r.doc_id.clone(), r.path.clone(), r.title.clone(), r.location.clone())
            };
            let doc_index = self.editor.borrow().docs.iter().position(|d| doc_id.as_ref() == Some(&d.id) || (path.is_some() && d.path() == path));
            if let Some(index) = doc_index {
                let mut editor = self.editor.borrow_mut();
                let (buffer, cursors, id) = {
                    let doc = &editor.docs[index];
                    (doc.buffer.clone(), doc.cursors.clone(), doc.id.clone())
                };
                match buffer.replace_all(query.clone(), replacement.clone(), cursors) {
                    Ok(0) => {}
                    Ok(count) => {
                        if index == editor.current {
                            editor.text_replaced();
                        } else {
                            let doc = &mut editor.docs[index];
                            doc.cursors = doc.cursors.iter().map(|c| TextSelection { anchor: buffer.clamp(c.anchor), head: buffer.clamp(c.head) }).collect();
                        }
                        done.push(ReplacedFile { title, location, count, undo: Undo::Document { doc_id: id, version: buffer.version() }, undone: false, error: None });
                    }
                    Err(error) => failures.push(format!("{title}: {error}")),
                }
            } else if let Some(path) = path {
                let backup = backups.join(format!("{}-{title}", done.len() + failures.len()));
                let result = std::fs::create_dir_all(&backups)
                    .map_err(|e| e.to_string())
                    .and_then(|_| replace_in_file(path.to_string_lossy().into_owned(), query.clone(), replacement.clone(), backup.to_string_lossy().into_owned()).map_err(|e| e.to_string()));
                match result {
                    Ok(0) => {}
                    Ok(count) => {
                        let modified = modified_time(&path);
                        done.push(ReplacedFile { title, location, count, undo: Undo::File { path, backup, modified }, undone: false, error: None });
                    }
                    Err(error) => failures.push(format!("{title}: {error}")),
                }
            }
        }
        {
            let mut search = self.search.borrow_mut();
            search.replaced = done;
            search.results.clear();
            search.row_map.clear();
            search.rows.set_vec(Vec::new());
            search.error = (!failures.is_empty()).then(|| format!("Couldn’t replace in {}", failures.join("; ")));
        }
        self.search_bridge(|b| b.set_selected(-1));
        self.show_replaced();
        self.show_search_status();
        self.refresh();
    }

    fn show_replaced(&self) {
        let rows: Vec<ReplacedRow> = self
            .search
            .borrow()
            .replaced
            .iter()
            .map(|f| ReplacedRow {
                title: f.title.clone().into(),
                location: f.location.clone().into(),
                count: plural(f.count, "replacement", "replacements").into(),
                undone: f.undone,
                error: f.error.clone().unwrap_or_default().into(),
            })
            .collect();
        self.search_bridge(|b| b.set_replaced(ModelRc::from(Rc::new(VecModel::from(rows)))));
    }

    /// Puts one replaced file back the way it was, if it hasn't changed since.
    pub fn undo_replaced(&self, index: usize) {
        self.undo_replaced_file(index);
        self.show_replaced();
        self.show_search_status();
        self.refresh();
    }

    fn undo_replaced_file(&self, index: usize) {
        let mut search = self.search.borrow_mut();
        let Some(file) = search.replaced.get_mut(index).filter(|f| !f.undone) else { return };
        let result: Result<(), String> = match &file.undo {
            Undo::Document { doc_id, version } => {
                let mut editor = self.editor.borrow_mut();
                match editor.docs.iter().position(|d| &d.id == doc_id) {
                    Some(i) if editor.docs[i].buffer.version() == *version => {
                        let buffer = editor.docs[i].buffer.clone();
                        match buffer.undo() {
                            Some(cursors) => {
                                editor.docs[i].cursors = cursors;
                                if i == editor.current {
                                    editor.text_replaced();
                                }
                                Ok(())
                            }
                            None => Err("Nothing to undo.".into()),
                        }
                    }
                    Some(_) => Err("Edited since; use Undo in the document.".into()),
                    None => Err("The document was closed.".into()),
                }
            }
            Undo::File { path, backup, modified } => {
                if modified_time(path) != *modified {
                    Err("Changed since, so it was left alone.".into())
                } else {
                    std::fs::copy(backup, path).map(|_| ()).map_err(|e| e.to_string())
                }
            }
        };
        match result {
            Ok(()) => {
                file.undone = true;
                file.error = None;
            }
            Err(error) => file.error = Some(error),
        }
    }

    pub fn undo_all_replaced(&self) {
        let count = self.search.borrow().replaced.len();
        for index in 0..count {
            self.undo_replaced_file(index);
        }
        self.show_replaced();
        self.show_search_status();
        self.refresh();
    }

    // MARK: Status

    fn show_search_status(&self) {
        let search = self.search.borrow();
        let matches: u64 = search.results.iter().map(|r| r.match_count).sum();
        let folder_scope = self.search_bridge(|b| b.get_scope() == 1).unwrap_or(false);
        let (status, error) = if let Some(error) = &search.error {
            (error.clone(), true)
        } else if search.searching {
            match &search.progress {
                Some(p) if folder_scope => (format!("Searching… {}", plural(p.files_searched, "file", "files")), false),
                _ => ("Searching…".into(), false),
            }
        } else if !search.replaced.is_empty() {
            let active: Vec<&ReplacedFile> = search.replaced.iter().filter(|f| !f.undone).collect();
            let count: u64 = active.iter().map(|f| f.count).sum();
            if active.is_empty() {
                ("Undone".into(), false)
            } else {
                (format!("Replaced {} in {}", group(count), plural(active.len() as u64, "file", "files")), false)
            }
        } else if search.searched.is_none() {
            (String::new(), false)
        } else {
            let stopped = if search.stopped { "Stopped. " } else { "" };
            if matches == 0 {
                (format!("{stopped}No results {}", search.description), false)
            } else {
                let more = search.progress.as_ref().is_some_and(|p| p.hit_limit) || search.results.iter().any(|r| r.truncated);
                (
                    format!(
                        "{stopped}{}{} {} in {}",
                        group(matches),
                        if more { "+" } else { "" },
                        if matches == 1 { "result" } else { "results" },
                        plural(search.results.len() as u64, "file", "files")
                    ),
                    false,
                )
            }
        };
        let skipped = match &search.progress {
            Some(p) if folder_scope => {
                let mut parts = Vec::new();
                if p.skipped_binary > 0 {
                    parts.push(format!("{} binary", group(p.skipped_binary)));
                }
                if p.skipped_large > 0 {
                    parts.push(format!("{} large", group(p.skipped_large)));
                }
                if p.unreadable > 0 {
                    parts.push(format!("{} unreadable", group(p.unreadable)));
                }
                let total = p.skipped_binary + p.skipped_large + p.unreadable;
                if parts.is_empty() { String::new() } else { format!("Skipped {} {}", parts.join(", "), if total == 1 { "file" } else { "files" }) }
            }
            _ => String::new(),
        };
        let searching = search.searching;
        drop(search);
        self.search_bridge(|b| {
            b.set_status(status.into());
            b.set_error(error);
            b.set_skipped(skipped.into());
            b.set_searching(searching);
        });
    }

    /// The panel's height, once a drag ends.
    pub fn search_panel_resized(&self) {
        if let Some(height) = self.search_bridge(|b| b.get_panel_height()) {
            self.settings.borrow_mut().search_panel_height = height;
            self.save_settings();
        }
    }

    /// For scripted runs: the panel's state.
    pub fn search_dump(&self) -> serde_json::Value {
        let search = self.search.borrow();
        let bridge_state = self.search_bridge(|b| {
            let rows: Vec<serde_json::Value> = b
                .get_rows()
                .iter()
                .map(|r| {
                    let text: String = r.parts.iter().map(|p| p.text.to_string()).collect();
                    let marked: Vec<String> = r.parts.iter().filter(|p| p.matched).map(|p| p.text.to_string()).collect();
                    serde_json::json!({"kind": r.kind, "title": r.title.as_str(), "location": r.location.as_str(), "count": r.count.as_str(), "text": text, "marked": marked})
                })
                .collect();
            let plan: Vec<serde_json::Value> = b
                .get_plan()
                .iter()
                .map(|r| serde_json::json!({"kind": r.kind, "title": r.title.as_str(), "included": r.included, "line": r.line.as_str(), "before": r.before.as_str(), "after": r.after.as_str()}))
                .collect();
            serde_json::json!({
                "open": b.get_open(),
                "scope": b.get_scope(),
                "folder": b.get_folder().as_str(),
                "status": b.get_status().as_str(),
                "skipped": b.get_skipped().as_str(),
                "error": b.get_error(),
                "searching": b.get_searching(),
                "selected": b.get_selected(),
                "rows": rows,
                "plan_open": b.get_plan_open(),
                "plan": plan,
                "plan_summary": b.get_plan_summary().as_str(),
            })
        });
        let replaced: Vec<serde_json::Value> =
            search.replaced.iter().map(|f| serde_json::json!({"title": f.title, "count": f.count, "undone": f.undone, "error": f.error})).collect();
        let mut state = bridge_state.unwrap_or_default();
        state["replaced"] = serde_json::Value::Array(replaced);
        state
    }
}
