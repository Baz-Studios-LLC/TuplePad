//! Find and Replace in Files: searching a folder in the background, the way `.gitignore` says.

use std::fs;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ignore::overrides::OverrideBuilder;
use ignore::{WalkBuilder, WalkParallel, WalkState};
use regex::Regex;
use ropey::Rope;

use crate::buffer::TextBuffer;
use crate::encoding;
use crate::error::{CoreError, CoreResult};
use crate::search::{self, LineMatches, ReplacementPreview, SearchQuery};

/// What and where to search.
#[derive(uniffi::Record, Clone, Debug)]
pub struct FolderSearchOptions {
    pub folder: String,
    pub query: SearchQuery,
    /// Globs a file must match, separated by commas or spaces, like "*.rs, *.swift". Empty means every file.
    pub include: String,
    /// Globs to skip, like "build, *.min.js".
    pub exclude: String,
    /// Also search files that `.gitignore` excludes, hidden files and very large files.
    pub search_everything: bool,
}

/// The matches in one file.
#[derive(uniffi::Record, Clone, Debug)]
pub struct FileMatches {
    pub path: String,
    pub lines: Vec<LineMatches>,
    pub match_count: u64,
    /// The file has more matches than were kept.
    pub truncated: bool,
}

/// How far a folder search has got.
#[derive(uniffi::Record, Clone, Debug, Default)]
pub struct FolderSearchProgress {
    pub files_searched: u64,
    pub files_matched: u64,
    pub matches: u64,
    pub skipped_binary: u64,
    pub skipped_large: u64,
    pub unreadable: u64,
    pub finished: bool,
    /// The search stopped early because it found too many matches.
    pub hit_limit: bool,
}

/// Files bigger than this are skipped unless searching everything.
const LARGE_FILE: u64 = 32 * 1024 * 1024;
/// Matches kept per file, and in all.
const FILE_MATCH_LIMIT: usize = 1_000;
const TOTAL_MATCH_LIMIT: u64 = 50_000;

#[derive(Default)]
struct Shared {
    cancelled: AtomicBool,
    finished: AtomicBool,
    hit_limit: AtomicBool,
    files_searched: AtomicU64,
    files_matched: AtomicU64,
    matches: AtomicU64,
    skipped_binary: AtomicU64,
    skipped_large: AtomicU64,
    unreadable: AtomicU64,
    results: Mutex<Vec<FileMatches>>,
}

/// A search running in the background. Poll `progress` and `take_results` until it's finished.
#[derive(uniffi::Object)]
pub struct FolderSearch {
    shared: Arc<Shared>,
}

#[uniffi::export]
impl FolderSearch {
    /// Starts searching. Fails right away for an empty or invalid pattern, a bad glob or a missing folder.
    #[uniffi::constructor]
    pub fn start(options: FolderSearchOptions) -> CoreResult<Arc<Self>> {
        let regex = search::compile(&options.query)?
            .ok_or_else(|| CoreError::InvalidPattern { message: "Type something to search for.".into() })?;
        let walker = walker(&options)?;
        let shared = Arc::new(Shared::default());
        let worker = Arc::clone(&shared);
        std::thread::spawn(move || {
            run(walker, &regex, &options, &worker);
            worker.finished.store(true, Ordering::Release);
        });
        Ok(Arc::new(FolderSearch { shared }))
    }

    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Release);
    }

    pub fn progress(&self) -> FolderSearchProgress {
        let s = &self.shared;
        let load = |n: &AtomicU64| n.load(Ordering::Acquire);
        FolderSearchProgress {
            files_searched: load(&s.files_searched),
            files_matched: load(&s.files_matched),
            matches: load(&s.matches),
            skipped_binary: load(&s.skipped_binary),
            skipped_large: load(&s.skipped_large),
            unreadable: load(&s.unreadable),
            finished: s.finished.load(Ordering::Acquire),
            hit_limit: s.hit_limit.load(Ordering::Acquire),
        }
    }

    /// Files with matches found since the last call.
    pub fn take_results(&self) -> Vec<FileMatches> {
        std::mem::take(&mut *self.shared.results.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

fn globs(list: &str) -> impl Iterator<Item = &str> {
    list.split(|c: char| c == ',' || c.is_whitespace()).filter(|glob| !glob.is_empty())
}

fn walker(options: &FolderSearchOptions) -> CoreResult<WalkParallel> {
    let folder = std::path::Path::new(&options.folder);
    if !folder.is_dir() {
        return Err(CoreError::Io { message: format!("“{}” isn’t a folder.", options.folder) });
    }
    let mut overrides = OverrideBuilder::new(folder);
    let bad_glob = |error: ignore::Error| CoreError::InvalidPattern { message: format!("Invalid file pattern: {error}") };
    for glob in globs(&options.include) {
        overrides.add(glob).map_err(bad_glob)?;
    }
    for glob in globs(&options.exclude) {
        overrides.add(&format!("!{glob}")).map_err(bad_glob)?;
    }
    let overrides = overrides.build().map_err(bad_glob)?;
    let mut builder = WalkBuilder::new(folder);
    builder
        .standard_filters(!options.search_everything)
        // .gitignore files count even outside a Git repository.
        .require_git(false)
        .overrides(overrides)
        .filter_entry(|entry| entry.file_name() != ".git")
        .threads(std::thread::available_parallelism().map_or(4, |n| n.get()).min(12));
    Ok(builder.build_parallel())
}

fn run(walker: WalkParallel, regex: &Regex, options: &FolderSearchOptions, shared: &Arc<Shared>) {
    walker.run(|| {
        Box::new(|entry| {
            if shared.cancelled.load(Ordering::Acquire) {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            if let Some(found) = search_file(entry.path(), regex, options, shared) {
                shared.files_matched.fetch_add(1, Ordering::AcqRel);
                let total = shared.matches.fetch_add(found.match_count, Ordering::AcqRel) + found.match_count;
                shared.results.lock().unwrap_or_else(|e| e.into_inner()).push(found);
                if total >= TOTAL_MATCH_LIMIT {
                    shared.hit_limit.store(true, Ordering::Release);
                    shared.cancelled.store(true, Ordering::Release);
                    return WalkState::Quit;
                }
            }
            WalkState::Continue
        })
    });
}

/// Searches one file, unless it's binary, too large or unreadable.
fn search_file(path: &std::path::Path, regex: &Regex, options: &FolderSearchOptions, shared: &Shared) -> Option<FileMatches> {
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size > LARGE_FILE && !options.search_everything {
        shared.skipped_large.fetch_add(1, Ordering::AcqRel);
        return None;
    }
    let Ok(bytes) = read_text_bytes(path) else {
        shared.unreadable.fetch_add(1, Ordering::AcqRel);
        return None;
    };
    shared.files_searched.fetch_add(1, Ordering::AcqRel);
    let Some(bytes) = bytes else {
        shared.skipped_binary.fetch_add(1, Ordering::AcqRel);
        return None;
    };
    let text = String::from_utf8(bytes).unwrap_or_else(|error| encoding::decode(error.into_bytes()).text);
    // Huge files are searched in chunks, so Stop takes effect without waiting for them.
    let cancelled = || shared.cancelled.load(Ordering::Acquire);
    if !search::has_match(&text, regex, &cancelled)? {
        return None;
    }
    let rope = Rope::from_str(&text);
    drop(text);
    let (found, truncated) = search::find_cancellable(&rope, regex, &options.query, FILE_MATCH_LIMIT, &cancelled)?;
    if found.is_empty() {
        return None;
    }
    let lines = search::group_by_line(&rope, &found, FILE_MATCH_LIMIT);
    Some(FileMatches { path: path.to_string_lossy().into_owned(), lines, match_count: found.len() as u64, truncated })
}

/// A file's bytes, or nil if it looks binary (a zero byte near the start, unless it's UTF-16).
/// Only the start of a binary file is read.
fn read_text_bytes(path: &std::path::Path) -> std::io::Result<Option<Vec<u8>>> {
    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(8192);
    (&mut file).take(8192).read_to_end(&mut bytes)?;
    let utf16_bom = bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]);
    if !utf16_bom && memchr::memchr(0, &bytes).is_some() {
        return Ok(None);
    }
    file.read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// Replaces every match in a file, keeping its encoding and line endings. The original file is
/// copied to `backup` first, so the change can be undone. Returns how many matches were replaced.
#[uniffi::export]
pub fn replace_in_file(path: String, query: SearchQuery, replacement: String, backup: String) -> CoreResult<u64> {
    let buffer = TextBuffer::open_without_syntax(path.clone())?;
    if buffer.file_info().had_decode_errors {
        return Err(CoreError::Io { message: format!("{path} isn’t valid text in its encoding, so it was left unchanged.") });
    }
    let count = buffer.replace_all(query, replacement, Vec::new())?;
    if count > 0 {
        fs::copy(&path, &backup)?;
        buffer.save()?;
    }
    Ok(count)
}

/// What replacing every match in a file would change (at most `max_groups` stretches of lines).
#[uniffi::export]
pub fn preview_file_replacements(path: String, query: SearchQuery, replacement: String, max_groups: u64) -> CoreResult<Vec<ReplacementPreview>> {
    TextBuffer::open_without_syntax(path)?.replacement_preview(query, replacement, max_groups)
}

/// Files in a folder for Go to File: paths relative to the folder, sorted, at most `limit`.
/// Skips what `.gitignore` excludes, hidden files and `.git`.
#[uniffi::export]
pub fn list_files(folder: String, limit: u32) -> CoreResult<Vec<String>> {
    let root = std::path::Path::new(&folder);
    if !root.is_dir() {
        return Err(CoreError::Io { message: format!("“{folder}” isn’t a folder.") });
    }
    let mut files = Vec::new();
    for entry in WalkBuilder::new(root).require_git(false).filter_entry(|e| e.file_name() != ".git").build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Ok(relative) = entry.path().strip_prefix(root) {
            files.push(relative.to_string_lossy().into_owned());
        }
        if files.len() >= limit as usize {
            break;
        }
    }
    files.sort_by_key(|path| path.to_lowercase());
    Ok(files)
}
