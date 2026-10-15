//! The app's side of the built-in MCP server (`tuplepad-mcp`): starting and stopping it with
//! Settings, answering its tools (on the main thread, where the documents live), asking the
//! person whether a client may read or edit, highlighting what AI tools change, the activity log
//! in Settings, and the status bar's indicator.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::ComponentHandle;
use tuplepad_core::{CaseChange, LineOperation, SearchQuery, TextBuffer, TextPos, TextSelection};
use tuplepad_mcp::{Access, Caller, DocumentInfo, SelectedText, SelectionInfo};

use crate::app::App;

/// How long an HTTP client counts as connected after it was last heard from (HTTP has no
/// connection to watch, unlike the bridge's socket).
const HTTP_PRESENCE: Duration = Duration::from_secs(120);

/// One line of the activity log.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub time: String,
    pub client: String,
    pub tool: String,
    pub summary: String,
    pub failed: bool,
}

/// What the server's threads share with the app.
#[derive(Default)]
pub struct Shared {
    /// Newest first.
    pub log: Mutex<VecDeque<LogEntry>>,
    /// Open bridge connections, and the client on each.
    pub bridges: Mutex<HashMap<u64, String>>,
    /// When each client was last heard from, and when it last used a tool.
    pub seen: Mutex<HashMap<String, (Instant, Option<Instant>)>>,
    /// Bumped whenever anything above changes.
    pub generation: AtomicU64,
}

impl Shared {
    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// The clients connected now, by name.
    pub fn connected(&self) -> Vec<String> {
        let mut names: Vec<String> = self.bridges.lock().map(|b| b.values().cloned().collect()).unwrap_or_default();
        if let Ok(seen) = self.seen.lock() {
            names.extend(seen.iter().filter(|(_, (at, _))| at.elapsed() < HTTP_PRESENCE).map(|(name, _)| name.clone()));
        }
        names.sort();
        names.dedup();
        names
    }

    /// Whether a client used a tool in the last few seconds.
    pub fn busy(&self) -> bool {
        self.seen.lock().map(|s| s.values().any(|(_, used)| used.is_some_and(|u| u.elapsed() < Duration::from_secs(3)))).unwrap_or(false)
    }
}

#[derive(Default)]
pub struct McpState {
    pub running: Option<tuplepad_mcp::Running>,
    pub shared: Arc<Shared>,
    /// Why it isn't running although it's on.
    pub error: Option<String>,
    /// What it was started with: (socket, HTTP port).
    started_with: Option<(PathBuf, Option<u16>)>,
    /// The generation last shown in the window.
    pub shown: u64,
    /// Counts highlights, so an older one's timer doesn't clear a newer one.
    flash_generation: u64,
}

/// Where the `tuplepad mcp` bridge connects. Debug builds use their own socket, so they never
/// take requests meant for the app; tests give theirs with `TUPLEPAD_MCP_SOCKET`.
pub fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("TUPLEPAD_MCP_SOCKET") {
        return PathBuf::from(path);
    }
    let name = if cfg!(debug_assertions) { "mcp-debug" } else { "mcp" };
    crate::settings::data_folder().join("Slint").join(format!("{name}.sock"))
}

/// The `tuplepad` command that bridges a client's stdio to the app: the one inside the app
/// (Contents/Helpers on a Mac), or beside the app's binary.
pub fn bridge_path() -> PathBuf {
    let name = if cfg!(windows) { "tuplepad.exe" } else { "tuplepad" };
    let exe = std::env::current_exe().unwrap_or_default();
    let folder = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let helpers = folder.parent().map(|contents| contents.join("Helpers").join(name));
    match helpers {
        Some(path) if cfg!(target_os = "macos") && path.exists() => path,
        _ => folder.join(name),
    }
}

/// What adds TuplePad to Claude Code.
fn claude_code_command() -> String {
    let path = bridge_path().to_string_lossy().into_owned();
    let quoted = if path.contains([' ', '\'', '"', '(', ')']) { format!("'{}'", path.replace('\'', "'\\''")) } else { path };
    format!("claude mcp add tuplepad -- {quoted} mcp")
}

/// Runs `f` on the main thread with the app, and waits for its answer.
fn on_main<T: Send + 'static>(f: impl FnOnce(&Rc<App>) -> T + Send + 'static) -> Result<T, String> {
    let (send, receive) = std::sync::mpsc::channel();
    slint::invoke_from_event_loop(move || {
        if let Some(app) = crate::platform::app() {
            let _ = send.send(f(&app));
        }
    })
    .map_err(|_| "TuplePad is closing.".to_string())?;
    receive.recv().map_err(|_| "TuplePad is closing.".to_string())
}

/// The server's view of the app.
struct AppHost {
    shared: Arc<Shared>,
}

impl tuplepad_mcp::Host for AppHost {
    fn authorize(&self, caller: &Caller, access: Access) -> Result<(), String> {
        let name = caller.name.clone();
        on_main(move |app| app.mcp_authorize(&name, access))?
    }

    fn seen(&self, caller: &Caller) {
        if caller.connection > 0 {
            if let Ok(mut bridges) = self.shared.bridges.lock() {
                bridges.insert(caller.connection, caller.name.clone());
            }
        } else if let Ok(mut seen) = self.shared.seen.lock() {
            let used = seen.get(&caller.name).and_then(|(_, used)| *used);
            seen.insert(caller.name.clone(), (Instant::now(), used));
        }
        self.shared.bump();
    }

    fn closed(&self, connection: u64) {
        if let Ok(mut bridges) = self.shared.bridges.lock() {
            bridges.remove(&connection);
        }
        self.shared.bump();
    }

    fn record(&self, caller: &Caller, tool: &str, outcome: Result<&str, &str>) {
        let entry = LogEntry {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            client: caller.name.clone(),
            tool: tool.to_string(),
            summary: outcome.unwrap_or_else(|error| error).lines().next().unwrap_or_default().chars().take(160).collect(),
            failed: outcome.is_err(),
        };
        if let Ok(mut log) = self.shared.log.lock() {
            log.push_front(entry);
            log.truncate(300);
        }
        if let Ok(mut seen) = self.shared.seen.lock() {
            let at = if caller.connection > 0 { seen.get(&caller.name).map_or(Instant::now() - HTTP_PRESENCE, |(at, _)| *at) } else { Instant::now() };
            seen.insert(caller.name.clone(), (at, Some(Instant::now())));
        }
        self.shared.bump();
    }

    fn list_documents(&self) -> Vec<DocumentInfo> {
        on_main(|app| app.mcp_documents()).unwrap_or_default()
    }

    fn read_document(&self, request: &tuplepad_mcp::ReadRequest) -> Result<String, String> {
        let document = request.document.clone();
        let (buffer, info) = on_main(move |app| app.mcp_document(&document).map(|i| (app.editor.borrow().docs[i].buffer.clone(), app.mcp_info(i))))??;
        Ok(read_lines(&buffer, &info, request.start_line, request.end_line))
    }

    fn selection(&self) -> Result<SelectionInfo, String> {
        on_main(|app| app.mcp_selection())?
    }

    fn open_file(&self, request: &tuplepad_mcp::OpenRequest) -> Result<DocumentInfo, String> {
        let request = request.clone();
        on_main(move |app| app.mcp_open(&request))?
    }

    fn create_document(&self, request: &tuplepad_mcp::CreateRequest) -> Result<DocumentInfo, String> {
        let request = request.clone();
        on_main(move |app| app.mcp_create(&request))?
    }

    fn edit_document(&self, request: &tuplepad_mcp::EditRequest) -> Result<String, String> {
        let request = request.clone();
        on_main(move |app| app.mcp_edit(&request))?
    }

    fn find(&self, request: &tuplepad_mcp::FindRequest) -> Result<String, String> {
        let document = request.document.clone();
        let (buffer, info) = on_main(move |app| {
            let index = match &document {
                Some(document) => app.mcp_document(document)?,
                None => app.editor.borrow().current,
            };
            Ok::<_, String>((app.editor.borrow().docs[index].buffer.clone(), app.mcp_info(index)))
        })??;
        find_in_buffer(&buffer, &info, request)
    }

    fn find_in_files(&self, request: &tuplepad_mcp::FindInFilesRequest) -> Result<String, String> {
        // Open documents with unsaved changes are searched as they are in TuplePad, not on disk.
        let unsaved = on_main(|app| {
            app.editor
                .borrow()
                .docs
                .iter()
                .filter(|d| d.buffer.is_modified())
                .filter_map(|d| d.path().map(|p| (std::fs::canonicalize(&p).unwrap_or(p), d.buffer.clone())))
                .collect::<Vec<_>>()
        })?;
        find_in_folder(request, &unsaved)
    }

    fn run_tool(&self, request: &tuplepad_mcp::RunToolRequest) -> Result<String, String> {
        let request = request.clone();
        on_main(move |app| app.mcp_run_tool(&request))?
    }

    fn save_document(&self, request: &tuplepad_mcp::SaveRequest) -> Result<DocumentInfo, String> {
        let request = request.clone();
        on_main(move |app| app.mcp_save(&request))?
    }
}

// MARK: Positions as people count them

/// The 1-based character column of a UTF-16 position in `line`.
fn char_column(line: &str, utf16: u64) -> u64 {
    let mut units = 0u64;
    let mut chars = 0u64;
    for c in line.chars() {
        if units >= utf16 {
            break;
        }
        units += c.len_utf16() as u64;
        chars += 1;
    }
    chars + 1
}

fn line_list(lines: &[u64]) -> String {
    let mut lines: Vec<u64> = lines.to_vec();
    lines.sort_unstable();
    lines.dedup();
    let mut runs: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let start = lines[i];
        while i + 1 < lines.len() && lines[i + 1] == lines[i] + 1 {
            i += 1;
        }
        runs.push(if lines[i] == start { format!("{start}") } else { format!("{start}–{}", lines[i]) });
        i += 1;
    }
    let shown = runs.len().min(8);
    let more = if runs.len() > shown { format!(" and {} more", runs.len() - shown) } else { String::new() };
    format!("{}{more}", runs[..shown].join(", "))
}

/// A document's lines, numbered, under a heading that says which and how many.
fn read_lines(buffer: &TextBuffer, info: &DocumentInfo, start: Option<u64>, end: Option<u64>) -> String {
    const MOST: u64 = 2_000;
    const LONGEST: usize = 2_000;
    let count = buffer.line_count();
    let first = start.unwrap_or(1).clamp(1, count);
    let last = end.unwrap_or(first + MOST - 1).clamp(first, count).min(first + MOST - 1);
    let lines = buffer.lines(first - 1, last - first + 1);
    let width = last.to_string().len().max(4);
    let unsaved = if info.unsaved { ", with unsaved changes" } else { "" };
    let mut text = format!("{} ({}{unsaved}): lines {first}–{last} of {count}", info.name, info.language);
    if last < count {
        text.push_str(&format!(" (read more with start_line {})", last + 1));
    }
    text.push('\n');
    for (i, line) in lines.iter().enumerate() {
        let shown: String = line.chars().take(LONGEST).collect();
        let cut = line.chars().count().saturating_sub(LONGEST);
        let more = if cut > 0 { format!(" … ({cut} more characters)") } else { String::new() };
        text.push_str(&format!("{:>width$}\t{shown}{more}\n", first + i as u64));
    }
    text
}

fn search_query(text: &str, regex: bool, match_case: bool, whole_word: bool) -> SearchQuery {
    SearchQuery { pattern: text.to_string(), case_sensitive: match_case, whole_word, regex }
}

fn find_in_buffer(buffer: &TextBuffer, info: &DocumentInfo, request: &tuplepad_mcp::FindRequest) -> Result<String, String> {
    const MOST: u64 = 500;
    let query = search_query(&request.query, request.regex, request.match_case, request.whole_word);
    let found = buffer.find_all(query, MOST).map_err(|e| e.to_string())?;
    if found.matches.is_empty() {
        return Ok(format!("No matches in {}.", info.name));
    }
    let more = if found.truncated { " (stopped at 500)" } else { "" };
    let mut text = format!("{} in {}{more}:\n", plural(found.matches.len() as u64, "match", "matches"), info.name);
    for m in &found.matches {
        let line = buffer.line(m.start().line);
        let shown: String = line.trim_start().chars().take(300).collect();
        text.push_str(&format!("{}:{}: {shown}\n", m.start().line + 1, char_column(&line, m.start().column)));
    }
    Ok(text)
}

fn find_in_folder(request: &tuplepad_mcp::FindInFilesRequest, unsaved: &[(PathBuf, Arc<TextBuffer>)]) -> Result<String, String> {
    let most = request.max_results.unwrap_or(200).clamp(1, 5_000) as usize;
    let folder = crate::platform::absolute(Path::new(&request.folder));
    let folder = std::fs::canonicalize(&folder).unwrap_or(folder);
    let options = tuplepad_core::FolderSearchOptions {
        folder: folder.to_string_lossy().into_owned(),
        query: search_query(&request.query, request.regex, request.match_case, request.whole_word),
        include: request.include.clone().unwrap_or_default(),
        exclude: request.exclude.clone().unwrap_or_default(),
        search_everything: false,
    };
    let search = tuplepad_core::FolderSearch::start(options).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut files: Vec<tuplepad_core::FileMatches> = Vec::new();
    let mut lines = 0;
    loop {
        let finished = search.progress().finished;
        for file in search.take_results() {
            lines += file.lines.len();
            files.push(file);
        }
        if finished || lines >= most || started.elapsed() > Duration::from_secs(30) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    search.cancel();
    let progress = search.progress();
    // Unsaved documents in the folder replace what's on disk.
    let query = search_query(&request.query, request.regex, request.match_case, request.whole_word);
    let open: Vec<&(PathBuf, Arc<TextBuffer>)> = unsaved.iter().filter(|(path, _)| path.starts_with(&folder)).collect();
    files.retain(|f| !open.iter().any(|(path, _)| std::fs::canonicalize(&f.path).ok().as_ref() == Some(path)));
    for (path, buffer) in open {
        let found = buffer.find_lines(query.clone(), most as u64).map_err(|e| e.to_string())?;
        if !found.lines.is_empty() {
            files.push(tuplepad_core::FileMatches { path: path.to_string_lossy().into_owned(), lines: found.lines, match_count: found.match_count, truncated: found.truncated });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    if files.is_empty() {
        return Ok(format!("No matches in {} files searched.", progress.files_searched));
    }
    let complete = progress.finished && lines <= most && !progress.hit_limit;
    let mut text = format!(
        "{} in {}{}:\n",
        plural(files.iter().map(|f| f.match_count).sum(), "match", "matches"),
        plural(files.len() as u64, "file", "files"),
        if complete { String::new() } else { format!(" (stopped after {most} lines; narrow the search to see more)") }
    );
    let mut shown = 0;
    for file in &files {
        let path = Path::new(&file.path);
        let relative = path.strip_prefix(&folder).unwrap_or(path);
        text.push_str(&format!("\n{}\n", relative.display()));
        for line in &file.lines {
            if shown >= most {
                break;
            }
            text.push_str(&format!("  {}: {}\n", line.line + 1, line.preview.chars().take(300).collect::<String>()));
            shown += 1;
        }
    }
    Ok(text)
}

fn plural(count: u64, one: &str, many: &str) -> String {
    if count == 1 { format!("1 {one}") } else { format!("{} {many}", crate::editor::group(count)) }
}

/// A text tool's result on the whole text: the new text, or a message for tools that only check.
enum ToolResult {
    Text(String),
    Checked(String),
}

fn run_text_tool(tool: &str, text: String, indent: u32, tab_width: u32) -> Result<ToolResult, String> {
    use LineOperation::*;
    let error = |e: tuplepad_core::CoreError| match e {
        tuplepad_core::CoreError::Invalid { message, line, column } if line > 0 => format!("{message} (line {line}, column {column})"),
        other => other.to_string(),
    };
    let line_operation = match tool {
        "lines.sort-ascending" => Some(SortAscending),
        "lines.sort-descending" => Some(SortDescending),
        "lines.sort-case-insensitive" => Some(SortCaseInsensitive),
        "lines.sort-numeric" => Some(SortNumeric),
        "lines.reverse" => Some(Reverse),
        "lines.remove-duplicates" => Some(RemoveDuplicates),
        "lines.remove-consecutive-duplicates" => Some(RemoveConsecutiveDuplicates),
        "lines.remove-empty" => Some(RemoveEmpty),
        "lines.remove-blank" => Some(RemoveBlank),
        "lines.join" => Some(Join),
        "whitespace.trim-trailing" => Some(TrimTrailing),
        "whitespace.trim-leading" => Some(TrimLeading),
        "whitespace.trim-both" => Some(TrimBoth),
        "whitespace.tabs-to-spaces" => Some(TabsToSpaces),
        "whitespace.spaces-to-tabs" => Some(LeadingSpacesToTabs),
        _ => None,
    };
    let case = match tool {
        "case.upper" => Some(CaseChange::Upper),
        "case.lower" => Some(CaseChange::Lower),
        "case.title" => Some(CaseChange::Title),
        "case.sentence" => Some(CaseChange::Sentence),
        "case.swap" => Some(CaseChange::Swap),
        _ => None,
    };
    if line_operation.is_some() || case.is_some() {
        // The Text menu's tools work on a selection: run them on a copy with everything selected.
        let scratch = TextBuffer::with_text(text);
        let all = vec![TextSelection { anchor: TextPos::new(0, 0), head: scratch.end_position() }];
        match (line_operation, case) {
            (Some(operation), _) => {
                scratch.transform_lines(operation, all, tab_width);
            }
            (_, Some(change)) => {
                scratch.change_case(change, all);
            }
            _ => {}
        }
        return Ok(ToolResult::Text(scratch.text()));
    }
    if let Some(name) = tool.strip_prefix("encode.") {
        let codec = crate::builtin::codec(name).ok_or_else(|| unknown_tool(tool))?;
        return Ok(ToolResult::Text(tuplepad_core::encode_text(codec, text)));
    }
    if let Some(name) = tool.strip_prefix("decode.") {
        let codec = crate::builtin::codec(name).ok_or_else(|| unknown_tool(tool))?;
        return tuplepad_core::decode_text(codec, text).map(ToolResult::Text).map_err(error);
    }
    match tool {
        "json.format" => tuplepad_core::json_format(text, indent, false).map(ToolResult::Text).map_err(error),
        "json.sort-keys" => tuplepad_core::json_format(text, indent, true).map(ToolResult::Text).map_err(error),
        "json.minify" => tuplepad_core::json_minify(text).map(ToolResult::Text).map_err(error),
        "json.validate" => match tuplepad_core::json_validate(text) {
            None => Ok(ToolResult::Checked("The JSON is valid.".into())),
            Some(e) => Err(error(e)),
        },
        "xml.format" => tuplepad_core::xml_format(text, indent.max(1)).map(ToolResult::Text).map_err(error),
        "xml.linearize" => tuplepad_core::xml_linearize(text).map(ToolResult::Text).map_err(error),
        "xml.validate" => match tuplepad_core::xml_validate(text) {
            None => Ok(ToolResult::Checked("The XML is well formed.".into())),
            Some(e) => Err(error(e)),
        },
        _ => Err(unknown_tool(tool)),
    }
}

fn unknown_tool(tool: &str) -> String {
    let names: Vec<&str> = tuplepad_mcp::TEXT_TOOLS.iter().map(|(id, _)| *id).collect();
    format!("TuplePad has no text tool “{tool}”. The tools are: {}.", names.join(", "))
}

/// The one range that changes `old` into `new` (the text between their common start and end),
/// as char offsets in `old` and the replacement.
fn changed_part(old: &str, new: &str) -> (usize, usize, String) {
    let (old_chars, new_chars): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let prefix = old_chars.iter().zip(&new_chars).take_while(|(a, b)| a == b).count();
    let suffix = old_chars[prefix..].iter().rev().zip(new_chars[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    (prefix, old_chars.len() - suffix, new_chars[prefix..new_chars.len() - suffix].iter().collect())
}

/// A char offset in `text` as a position.
fn position_of_char(text: &str, offset: usize) -> TextPos {
    let mut line = 0u64;
    let mut column = 0u64;
    for c in text.chars().take(offset) {
        if c == '\n' {
            line += 1;
            column = 0;
        } else {
            column += c.len_utf16() as u64;
        }
    }
    TextPos::new(line, column)
}

impl App {
    // MARK: Starting and stopping

    /// Starts, stops or restarts the server to match Settings.
    pub fn update_mcp(&self) {
        // Scripted runs serve only on a socket of their own (TUPLEPAD_MCP_SOCKET), for tests.
        let allowed = !self.is_scripted() || std::env::var_os("TUPLEPAD_MCP_SOCKET").is_some();
        let (enabled, http, port) = {
            let settings = self.settings.borrow();
            (settings.mcp_enabled && allowed, settings.mcp_http, settings.mcp_port)
        };
        let wanted = enabled.then(|| (socket_path(), http.then_some(port)));
        let mut state = self.mcp.borrow_mut();
        if state.started_with == wanted && (state.running.is_some() || wanted.is_none() || state.error.is_some()) {
            return;
        }
        state.running = None;
        state.error = None;
        state.started_with = wanted.clone();
        if let Some((socket, http_port)) = wanted {
            let host = Arc::new(AppHost { shared: state.shared.clone() });
            let options = tuplepad_mcp::Options { socket, http_port, version: crate::VERSION.trim().to_string() };
            match tuplepad_mcp::start(host, options) {
                Ok(running) => state.running = Some(running),
                Err(error) => state.error = Some(format!("TuplePad couldn't start its MCP server: {error}")),
            }
        }
        state.shared.bump();
    }

    pub fn stop_mcp(&self) {
        self.mcp.borrow_mut().running = None;
    }

    // MARK: Permission

    /// Whether a client may use a tool of this kind, asking the person the first time.
    fn mcp_authorize(&self, name: &str, access: Access) -> Result<(), String> {
        let decided = self.settings.borrow().mcp_clients.get(name).cloned();
        let decision = match decided {
            Some(decision) => decision,
            None => {
                let decision = self.ask_mcp_permission(name);
                if let Some(decision) = &decision {
                    self.settings.borrow_mut().mcp_clients.insert(name.to_string(), decision.clone());
                    self.save_settings();
                    self.mcp.borrow().shared.bump();
                }
                decision.unwrap_or_else(|| "none".into())
            }
        };
        match (decision.as_str(), access) {
            ("edit", _) | ("read", Access::Read) => Ok(()),
            ("read", Access::Write) => Err(format!("“{name}” may only read documents in TuplePad. The person can allow editing in TuplePad's Settings ▸ AI.")),
            _ => Err(format!("The person hasn't allowed “{name}” to use TuplePad. They can change that in TuplePad's Settings ▸ AI.")),
        }
    }

    /// Asks whether a client may read and edit, only read, or not use TuplePad. None if the
    /// question was dismissed without an answer.
    fn ask_mcp_permission(&self, name: &str) -> Option<String> {
        const EDIT: &str = "Allow Editing";
        const READ: &str = "Read Only";
        const DENY: &str = "Don't Allow";
        let answer = if self.is_scripted() {
            self.next_answer()
        } else {
            let result = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Info)
                .set_title(format!("Let “{name}” use TuplePad?"))
                .set_description(format!(
                    "“{name}” wants to work with your open documents through TuplePad's MCP server.\n\n\
                     Allow Editing lets it read and change them; each change shows up as it happens and can be undone. \
                     Read Only lets it read them. You can change this later in Settings ▸ AI."
                ))
                .set_buttons(rfd::MessageButtons::YesNoCancelCustom(EDIT.into(), READ.into(), DENY.into()))
                .show();
            match result {
                rfd::MessageDialogResult::Custom(answer) => Some(answer),
                rfd::MessageDialogResult::Yes => Some(EDIT.into()),
                rfd::MessageDialogResult::No => Some(READ.into()),
                rfd::MessageDialogResult::Cancel => Some(DENY.into()),
                _ => None,
            }
        };
        match answer.as_deref() {
            Some(EDIT) => Some("edit".into()),
            Some(READ) => Some("read".into()),
            Some(DENY) => Some("none".into()),
            _ => None,
        }
    }

    /// Settings ▸ AI: change what a client may do ("edit", "read" or "none"), or forget it (None).
    pub fn set_mcp_client(&self, name: &str, decision: Option<&str>) {
        {
            let mut settings = self.settings.borrow_mut();
            match decision {
                Some(decision) => settings.mcp_clients.insert(name.to_string(), decision.to_string()),
                None => settings.mcp_clients.remove(name),
            };
        }
        self.save_settings();
        self.mcp.borrow().shared.bump();
    }

    // MARK: Documents

    pub fn mcp_info(&self, index: usize) -> DocumentInfo {
        let editor = self.editor.borrow();
        let doc = &editor.docs[index];
        DocumentInfo {
            id: doc.id.clone(),
            name: doc.name(),
            path: doc.path().map(|p| p.to_string_lossy().into_owned()),
            language: self.language_name(doc),
            unsaved: doc.buffer.is_modified(),
            lines: doc.buffer.line_count(),
            active: index == editor.current,
        }
    }

    fn mcp_documents(&self) -> Vec<DocumentInfo> {
        let count = self.editor.borrow().docs.len();
        (0..count).map(|i| self.mcp_info(i)).collect()
    }

    /// The open document a client means: by id, by path, or by name if only one has it.
    fn mcp_document(&self, document: &str) -> Result<usize, String> {
        let editor = self.editor.borrow();
        let wanted = document.trim();
        if let Some(i) = editor.docs.iter().position(|d| d.id == wanted) {
            return Ok(i);
        }
        let as_path = crate::platform::absolute(Path::new(wanted));
        let canonical = std::fs::canonicalize(&as_path).unwrap_or(as_path.clone());
        if let Some(i) = editor.docs.iter().position(|d| d.path().is_some_and(|p| p == as_path || std::fs::canonicalize(&p).ok() == Some(canonical.clone()))) {
            return Ok(i);
        }
        let named: Vec<usize> = editor.docs.iter().enumerate().filter(|(_, d)| d.name() == wanted).map(|(i, _)| i).collect();
        match named.as_slice() {
            [i] => Ok(*i),
            [] => Err(format!("No open document is “{wanted}”. list_documents shows the open ones, and open_file opens a file.")),
            _ => Err(format!("More than one open document is called “{wanted}”; use its id from list_documents.")),
        }
    }

    fn mcp_selection(&self) -> Result<SelectionInfo, String> {
        let index = self.editor.borrow().current;
        let info = self.mcp_info(index);
        let editor = self.editor.borrow();
        let doc = &editor.docs[index];
        let selections = doc
            .cursors
            .iter()
            .map(|c| {
                let (start, end) = (c.start(), c.end());
                SelectedText {
                    start_line: start.line + 1,
                    start_column: char_column(&doc.buffer.line(start.line), start.column),
                    end_line: end.line + 1,
                    end_column: char_column(&doc.buffer.line(end.line), end.column),
                    text: doc.buffer.text_in(start, end),
                }
            })
            .collect();
        Ok(SelectionInfo { document: info, selections })
    }

    fn mcp_open(&self, request: &tuplepad_mcp::OpenRequest) -> Result<DocumentInfo, String> {
        let path = crate::platform::absolute(Path::new(&request.path));
        if path.is_dir() {
            return Err(format!("“{}” is a folder; open_file opens files.", path.display()));
        }
        self.open_requests(vec![(path.clone(), request.line, None)]);
        let index = self.mcp_document(&path.to_string_lossy()).map_err(|_| format!("TuplePad couldn't open “{}”.", path.display()))?;
        Ok(self.mcp_info(index))
    }

    fn mcp_create(&self, request: &tuplepad_mcp::CreateRequest) -> Result<DocumentInfo, String> {
        let language = match &request.language {
            Some(wanted) if !wanted.trim().is_empty() => {
                let wanted = wanted.trim().to_lowercase();
                let found = tuplepad_core::languages().into_iter().find(|l| l.id == wanted || l.name.to_lowercase() == wanted);
                Some(found.map(|l| l.id).ok_or_else(|| format!("TuplePad doesn't know the language “{wanted}”."))?)
            }
            _ => None,
        };
        self.new_document_with_text(&request.text);
        let index = self.editor.borrow().current;
        if let Some(language) = language {
            self.editor.borrow().docs[index].buffer.set_language(Some(language));
            self.editor.borrow_mut().invalidate();
        }
        self.refresh();
        Ok(self.mcp_info(index))
    }

    /// Applies a client's edits as one undo step, highlighted as they land.
    fn mcp_edit(&self, request: &tuplepad_mcp::EditRequest) -> Result<String, String> {
        let index = self.mcp_document(&request.document)?;
        if request.edits.is_empty() {
            return Err("There are no edits in the request.".into());
        }
        let (buffer, name) = {
            let editor = self.editor.borrow();
            (editor.docs[index].buffer.clone(), editor.docs[index].name())
        };
        let mut ranges = Vec::new();
        let mut texts = Vec::new();
        for (n, edit) in request.edits.iter().enumerate() {
            let which = if request.edits.len() > 1 { format!(" (edit {})", n + 1) } else { String::new() };
            let range = match &edit.old_text {
                Some(old) if !old.is_empty() => {
                    let found = buffer.find_all(search_query(old, false, true, false), 2).map_err(|e| e.to_string())?;
                    match found.matches.as_slice() {
                        [only] => *only,
                        [] => return Err(format!("old_text wasn't found in {name}{which}. Read the document again: it may have changed.")),
                        _ => {
                            let count = buffer.find_all(search_query(old, false, true, false), 10_000).map(|f| f.matches.len()).unwrap_or(2);
                            return Err(format!("old_text appears {count} times in {name}{which}; include more of the text around it so it's unique."));
                        }
                    }
                }
                _ => {
                    let (Some(start_line), Some(start_column)) = (edit.start_line, edit.start_column) else {
                        return Err(format!("Each edit needs old_text, or start_line and start_column{which}."));
                    };
                    let (end_line, end_column) = (edit.end_line.unwrap_or(start_line), edit.end_column.unwrap_or(start_column));
                    let place = |line: u64, column: u64| {
                        buffer.position_for_character(line, column).ok_or_else(|| {
                            format!("Line {line}, column {column} is outside {name}{which}, which has {} lines.", buffer.line_count())
                        })
                    };
                    let (start, end) = (place(start_line, start_column)?, place(end_line, end_column)?);
                    if end < start {
                        return Err(format!("The range ends before it starts{which}."));
                    }
                    TextSelection { anchor: start, head: end }
                }
            };
            ranges.push(range);
            texts.push(edit.new_text.clone());
        }
        let cursors = self.editor.borrow().docs[index].cursors.clone();
        let result = buffer.edit_from_outside(ranges, texts, cursors).map_err(|e| e.to_string())?;
        let lines: Vec<u64> = result.inserted.iter().flat_map(|r| (r.start().line + 1)..=(r.end().line + 1).max(r.start().line + 1)).collect();
        self.mcp_changed(index, result.cursors, result.inserted);
        Ok(format!(
            "Made {} to {name} (now {} lines). Changed lines: {}. It isn't saved.",
            plural(request.edits.len() as u64, "edit", "edits"),
            buffer.line_count(),
            line_list(&lines)
        ))
    }

    /// After a client changed a document: the person's carets, the highlight, and the window.
    fn mcp_changed(&self, index: usize, cursors: Vec<TextSelection>, inserted: Vec<TextSelection>) {
        let id = {
            let mut editor = self.editor.borrow_mut();
            let doc = &mut editor.docs[index];
            doc.cursors = cursors;
            doc.last_edited = tuplepad_core::now_seconds();
            let id = doc.id.clone();
            editor.flashes.insert(id.clone(), inserted);
            editor.invalidate();
            id
        };
        let generation = {
            let mut state = self.mcp.borrow_mut();
            state.flash_generation += 1;
            state.flash_generation
        };
        if let Some(ui) = self.ui.upgrade() {
            use slint::ComponentHandle;
            ui.global::<crate::EditorBridge>().set_flash_on(true);
        }
        // The highlight shows for a moment, fades, and then goes.
        let app = self.me.borrow().clone();
        slint::Timer::single_shot(Duration::from_millis(900), move || {
            if let Some(app) = app.upgrade()
                && app.mcp.borrow().flash_generation == generation
                && let Some(ui) = app.ui.upgrade()
            {
                use slint::ComponentHandle;
                ui.global::<crate::EditorBridge>().set_flash_on(false);
            }
        });
        let app = self.me.borrow().clone();
        slint::Timer::single_shot(Duration::from_millis(2_700), move || {
            if let Some(app) = app.upgrade() {
                if app.mcp.borrow().flash_generation == generation {
                    app.editor.borrow_mut().flashes.clear();
                } else {
                    app.editor.borrow_mut().flashes.remove(&id);
                }
                app.refresh();
            }
        });
        self.refresh();
    }

    fn mcp_run_tool(&self, request: &tuplepad_mcp::RunToolRequest) -> Result<String, String> {
        let index = self.mcp_document(&request.document)?;
        let tool = request.tool.trim();
        if !tuplepad_mcp::TEXT_TOOLS.iter().any(|(id, _)| *id == tool) {
            return Err(unknown_tool(tool));
        }
        let (buffer, name, indent, tab_width) = {
            let editor = self.editor.borrow();
            let indent = match editor.docs[index].indentation.unwrap_or_else(|| editor.indentation()) {
                tuplepad_core::Indentation::Tabs => 0,
                tuplepad_core::Indentation::Spaces(width) => width,
            };
            (editor.docs[index].buffer.clone(), editor.docs[index].name(), indent, editor.tab_width() as u32)
        };
        let old = buffer.text();
        let new = match run_text_tool(tool, old.clone(), indent, tab_width)? {
            ToolResult::Checked(message) => return Ok(format!("{message} ({name})")),
            ToolResult::Text(new) => new,
        };
        if new == old {
            return Ok(format!("{tool} made no changes to {name}."));
        }
        let (start, end, replacement) = changed_part(&old, &new);
        let range = TextSelection { anchor: position_of_char(&old, start), head: position_of_char(&old, end) };
        let cursors = self.editor.borrow().docs[index].cursors.clone();
        let result = buffer.edit_from_outside(vec![range], vec![replacement], cursors).map_err(|e| e.to_string())?;
        let lines: Vec<u64> = result.inserted.iter().flat_map(|r| (r.start().line + 1)..=(r.end().line + 1)).collect();
        self.mcp_changed(index, result.cursors, result.inserted);
        Ok(format!("Ran {tool} on {name} (now {} lines). Changed lines: {}. It isn't saved.", buffer.line_count(), line_list(&lines)))
    }

    fn mcp_save(&self, request: &tuplepad_mcp::SaveRequest) -> Result<DocumentInfo, String> {
        let index = self.mcp_document(&request.document)?;
        let (buffer, name, conflict) = {
            let editor = self.editor.borrow();
            let doc = &editor.docs[index];
            (doc.buffer.clone(), doc.name(), doc.conflict.is_some())
        };
        let target = request.path.as_deref().filter(|p| !p.trim().is_empty()).map(|p| crate::platform::absolute(Path::new(p)));
        let result = match &target {
            Some(path) => {
                if path.is_dir() {
                    return Err(format!("“{}” is a folder; give the path of the file to save.", path.display()));
                }
                buffer.save_as(path.to_string_lossy().into_owned())
            }
            None if buffer.path().is_none() => return Err(format!("{name} hasn't been saved before, so save_document needs a path for it.")),
            None if conflict => {
                return Err(format!("{name} changed on disk (or went missing) since TuplePad opened it. The person needs to choose Reload or Keep Mine first."));
            }
            None => buffer.save(),
        };
        result.map_err(|e| format!("TuplePad couldn't save {name}: {e}"))?;
        if let Some(path) = &target {
            self.settings.borrow_mut().note_recent(path);
        }
        {
            let mut editor = self.editor.borrow_mut();
            let doc = &mut editor.docs[index];
            if let Some(path) = doc.path() {
                doc.size = std::fs::metadata(&path).map_or(0, |m| m.len());
            }
            editor.invalidate();
        }
        self.refresh();
        Ok(self.mcp_info(index))
    }

    // MARK: Showing it

    /// From the 60 ms tick: the status bar's indicator, and Settings when something changed.
    pub fn mcp_tick(&self) {
        let (text, busy) = self.mcp_indicator();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_mcp_indicator(text.into());
            ui.set_mcp_busy(busy);
        }
        let generation = self.mcp.borrow().shared.generation.load(Ordering::Acquire);
        if self.mcp.borrow().shown != generation {
            self.mcp.borrow_mut().shown = generation;
            self.refresh_settings_window();
        }
    }

    /// What the status bar shows: the connected clients (empty when none), and whether one is working.
    pub fn mcp_indicator(&self) -> (String, bool) {
        let state = self.mcp.borrow();
        if state.running.is_none() {
            return (String::new(), false);
        }
        let connected = state.shared.connected();
        let text = match connected.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            many => format!("{} AI clients", many.len()),
        };
        (text, state.shared.busy())
    }

    /// Settings ▸ AI: whether the server runs, how to connect, the AI tools that asked, and the log.
    pub fn show_mcp_settings(&self, window: &crate::SettingsWindow) {
        use slint::{ModelRc, VecModel};
        let settings = self.settings.borrow().clone();
        let state = self.mcp.borrow();
        let connected = state.shared.connected();
        let status = if !settings.mcp_enabled {
            "Claude Code, Claude Desktop and other MCP clients can read and edit your open documents. TuplePad asks you first.".to_string()
        } else if let Some(error) = &state.error {
            error.clone()
        } else {
            match connected.as_slice() {
                [] => "On. No AI tool is connected right now.".to_string(),
                [one] => format!("On. {one} is connected."),
                many => format!("On. {} AI tools are connected.", many.len()),
            }
        };
        let url = format!("http://127.0.0.1:{}/mcp", settings.mcp_port);
        let http_detail = match state.running.as_ref() {
            Some(running) if settings.mcp_http => match (&running.http_error, running.http_url()) {
                (Some(error), _) => format!("{error} Change mcpPort in the settings file to use another."),
                (None, Some(url)) => format!("At {url}"),
                _ => String::new(),
            },
            _ => format!("At {url}, for tools that connect over HTTP"),
        };
        let shown_url = state.running.as_ref().filter(|_| settings.mcp_http).and_then(|r| r.http_url()).unwrap_or_default();
        let clients: Vec<crate::McpClientRow> = settings
            .mcp_clients
            .iter()
            .map(|(name, decision)| crate::McpClientRow {
                name: name.into(),
                access: match decision.as_str() {
                    "edit" => 0,
                    "read" => 1,
                    _ => 2,
                },
                connected: connected.contains(name),
            })
            .collect();
        let log: Vec<crate::McpLogRow> = state
            .shared
            .log
            .lock()
            .map(|log| {
                log.iter()
                    .map(|e| crate::McpLogRow { time: e.time.clone().into(), client: e.client.clone().into(), tool: e.tool.clone().into(), summary: e.summary.clone().into(), failed: e.failed })
                    .collect()
            })
            .unwrap_or_default();
        window.set_mcp_status(status.into());
        window.set_mcp_http_detail(http_detail.into());
        window.set_mcp_command(claude_code_command().into());
        window.set_mcp_json(format!("\"command\": \"{}\", \"args\": [\"mcp\"]", bridge_path().display()).into());
        window.set_mcp_url(shown_url.into());
        window.set_mcp_clients(ModelRc::new(VecModel::from(clients)));
        window.set_mcp_log(ModelRc::new(VecModel::from(log)));
    }

    /// A copy button in Settings ▸ AI: the Claude Code command, the JSON other apps take, or the URL.
    pub fn copy_mcp_setup(&self, what: &str) {
        let text = match what {
            "command" => claude_code_command(),
            "json" => serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {"tuplepad": {"command": bridge_path().to_string_lossy(), "args": ["mcp"]}}
            }))
            .unwrap_or_default(),
            _ => self.mcp.borrow().running.as_ref().and_then(|r| r.http_url()).unwrap_or_default(),
        };
        self.editor.borrow_mut().write_clipboard(text);
        let Some(window) = self.settings_window.borrow().as_ref().map(|w| w.clone_strong()) else { return };
        window.set_copied(what.into());
        let weak = window.as_weak();
        let what = what.to_string();
        slint::Timer::single_shot(Duration::from_millis(1_500), move || {
            if let Some(window) = weak.upgrade()
                && window.get_copied() == what.as_str()
            {
                window.set_copied("".into());
            }
        });
    }

    /// For scripted runs.
    pub fn mcp_dump(&self) -> serde_json::Value {
        let state = self.mcp.borrow();
        let log: Vec<serde_json::Value> = state
            .shared
            .log
            .lock()
            .map(|log| log.iter().map(|e| serde_json::json!({"client": e.client, "tool": e.tool, "summary": e.summary, "failed": e.failed})).collect())
            .unwrap_or_default();
        serde_json::json!({
            "running": state.running.is_some(),
            "http": state.running.as_ref().and_then(|r| r.http_url()),
            "error": state.error,
            "connected": state.shared.connected(),
            "clients": self.settings.borrow().mcp_clients,
            "log": log,
            "flashes": self.editor.borrow().flashes.values().map(|v| v.len()).sum::<usize>(),
        })
    }
}
