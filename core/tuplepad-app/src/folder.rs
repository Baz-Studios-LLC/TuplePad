//! The folder in the sidebar (Open Folder…): a file tree that follows changes on disk, and Go to
//! File, which finds any file in it by name.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use notify::Watcher;
use slint::{Model, ModelRc, VecModel};

use crate::FolderRow;
use crate::app::App;
use crate::palette::Entry;

/// Names never shown in the tree.
const HIDDEN: [&str; 5] = [".git", ".DS_Store", ".svn", ".hg", "Thumbs.db"];
/// Files Go to File lists at most.
const QUICK_OPEN_LIMIT: u32 = 50_000;

#[derive(Clone)]
struct Node {
    path: PathBuf,
    name: String,
    folder: bool,
}

pub struct FolderState {
    pub root: PathBuf,
    /// Listings of the folders that have been opened.
    listings: HashMap<PathBuf, Vec<Node>>,
    expanded: HashSet<PathBuf>,
    /// What each row of the tree is.
    rows: Vec<Node>,
    model: Rc<VecModel<FolderRow>>,
    _watcher: Option<notify::RecommendedWatcher>,
    /// Folders changed on disk since the tree was last refreshed.
    changed: HashSet<PathBuf>,
    refresh_timer: slint::Timer,
    /// Every file, relative to the root, once Go to File has listed them.
    files: Option<Vec<String>>,
}

/// A folder's contents: folders first, then files, by name.
fn list(folder: &Path) -> Vec<Node> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut nodes: Vec<Node> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if HIDDEN.contains(&name.as_str()) {
                return None;
            }
            let folder = entry.file_type().map(|t| t.is_dir()).unwrap_or(false) || (entry.path().is_dir());
            Some(Node { path: entry.path(), name, folder })
        })
        .collect();
    nodes.sort_by(|a, b| b.folder.cmp(&a.folder).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    nodes
}

fn badge(name: &str) -> String {
    Path::new(name).extension().map(|e| e.to_string_lossy().to_uppercase().chars().take(4).collect()).unwrap_or_default()
}

impl FolderState {
    fn new(root: PathBuf) -> FolderState {
        let mut listings = HashMap::new();
        listings.insert(root.clone(), list(&root));
        FolderState {
            root,
            listings,
            expanded: HashSet::new(),
            rows: Vec::new(),
            model: Rc::new(VecModel::default()),
            _watcher: None,
            changed: HashSet::new(),
            refresh_timer: slint::Timer::default(),
            files: None,
        }
    }

    /// Marks the document showing in the editor.
    pub fn mark(&mut self, current: Option<&Path>) {
        self.rebuild(current);
    }

    /// The tree's rows: the root's contents, with opened folders' contents under them.
    fn rebuild(&mut self, current: Option<&Path>) {
        let mut rows = Vec::new();
        let mut items = Vec::new();
        // Depth-first, keeping each folder's order.
        fn walk(state: &FolderState, folder: &Path, depth: u32, rows: &mut Vec<Node>, items: &mut Vec<FolderRow>, current: Option<&Path>) {
            for node in state.listings.get(folder).map(Vec::as_slice).unwrap_or(&[]) {
                let expanded = node.folder && state.expanded.contains(&node.path);
                items.push(FolderRow {
                    name: node.name.clone().into(),
                    depth: depth as i32,
                    folder: node.folder,
                    expanded,
                    badge: if node.folder { Default::default() } else { badge(&node.name).into() },
                    current: current == Some(node.path.as_path()),
                });
                rows.push(node.clone());
                if expanded {
                    walk(state, &node.path, depth + 1, rows, items, current);
                }
            }
        }
        walk(self, &self.root.clone(), 0, &mut rows, &mut items, current);
        self.rows = rows;
        if self.model.row_count() != items.len() {
            self.model.set_vec(items);
        } else {
            for (i, item) in items.into_iter().enumerate() {
                if self.model.row_data(i).as_ref() != Some(&item) {
                    self.model.set_row_data(i, item);
                }
            }
        }
    }
}

impl App {
    fn current_path(&self) -> Option<PathBuf> {
        self.editor.try_borrow().ok().and_then(|e| e.doc().path())
    }

    /// File ▸ Open Folder…
    pub fn open_folder_dialog(&self) {
        let folder = if self.is_scripted() { self.next_scripted_folder() } else { rfd::FileDialog::new().set_title("Open Folder").pick_folder() };
        if let Some(folder) = folder {
            self.open_folder(&folder);
        }
    }

    /// Shows a folder in the sidebar and watches it for changes.
    pub fn open_folder(&self, folder: &Path) {
        let root = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
        if !root.is_dir() {
            return;
        }
        let mut state = FolderState::new(root.clone());
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            let folders: Vec<PathBuf> = event.paths.iter().filter_map(|p| p.parent().map(Path::to_path_buf)).collect();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = crate::platform::app() {
                    app.folder_changed(folders);
                }
            });
        });
        if let Ok(mut watcher) = watcher
            && watcher.watch(&root, notify::RecursiveMode::Recursive).is_ok()
        {
            state._watcher = Some(watcher);
        }
        state.rebuild(self.current_path().as_deref());
        if let Some(ui) = self.ui.upgrade() {
            ui.set_folder_rows(ModelRc::from(state.model.clone()));
            ui.set_folder_name(root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned()).into());
            ui.set_sidebar_mode(1);
            ui.set_sidebar_visible(true);
        }
        *self.folder.borrow_mut() = Some(state);
        {
            let mut settings = self.settings.borrow_mut();
            settings.folder = Some(root.to_string_lossy().into_owned());
            settings.sidebar_visible = true;
        }
        self.save_settings();
        // Find in Files starts from this folder.
        self.set_search_folder_path(&root);
    }

    pub fn close_folder(&self) {
        *self.folder.borrow_mut() = None;
        if let Some(ui) = self.ui.upgrade() {
            ui.set_folder_name(Default::default());
            ui.set_folder_rows(ModelRc::default());
            ui.set_sidebar_mode(0);
        }
        self.settings.borrow_mut().folder = None;
        self.save_settings();
    }

    /// A click on a row: a folder opens or closes, a file opens in the editor.
    pub fn activate_folder_row(&self, index: usize) {
        let node = self.folder.borrow().as_ref().and_then(|s| s.rows.get(index).cloned());
        let Some(node) = node else { return };
        if node.folder {
            {
                let mut folder = self.folder.borrow_mut();
                let Some(state) = folder.as_mut() else { return };
                if !state.expanded.remove(&node.path) {
                    state.expanded.insert(node.path.clone());
                    if !state.listings.contains_key(&node.path) {
                        state.listings.insert(node.path.clone(), list(&node.path));
                    }
                }
            }
            self.refresh_folder_tree();
        } else {
            self.open_paths(std::slice::from_ref(&node.path));
            if let Some(ui) = self.ui.upgrade() {
                ui.invoke_focus_editor();
            }
        }
    }

    /// A tree row's context menu.
    pub fn folder_action(&self, index: usize, action: &str) {
        let node = self.folder.borrow().as_ref().and_then(|s| s.rows.get(index).cloned());
        let Some(node) = node else { return };
        match action {
            "open" => self.activate_folder_row(index),
            "find" => {
                self.set_search_folder_path(&node.path);
                self.show_search(1, false);
            }
            "show" => crate::app::reveal(&node.path),
            "copy-path" => {
                self.editor.borrow_mut().write_clipboard(node.path.to_string_lossy().into_owned());
                self.show_toast("Copied the path");
            }
            "trash" => {
                let place = if cfg!(windows) { "Recycle Bin" } else { "Trash" };
                let what = if node.folder { "folder" } else { "file" };
                if !self.confirm(&format!("Move the {what} “{}” to the {place}?", node.name), "You can put it back from there.", crate::commands::MOVE_TO_TRASH) {
                    return;
                }
                // Scripted runs delete their own test files rather than filling the real Trash.
                let result = if self.is_scripted() {
                    if node.folder { std::fs::remove_dir_all(&node.path) } else { std::fs::remove_file(&node.path) }.map_err(|e| e.to_string())
                } else {
                    trash::delete(&node.path).map_err(|e| e.to_string())
                };
                match result {
                    Ok(()) => {
                        if let Some(state) = self.folder.borrow_mut().as_mut()
                            && let Some(parent) = node.path.parent()
                        {
                            state.changed.insert(parent.to_path_buf());
                        }
                        self.reload_changed_folders();
                        self.show_toast(&format!("Moved “{}” to the {place}", node.name));
                    }
                    Err(error) => self.alert(&format!("TuplePad couldn't move “{}” to the {place}.", node.name), &error),
                }
            }
            _ => {}
        }
    }

    /// Marks the current document in the tree (after switching documents).
    pub fn refresh_folder_tree(&self) {
        let current = self.current_path();
        if let Ok(mut folder) = self.folder.try_borrow_mut()
            && let Some(state) = folder.as_mut()
        {
            state.rebuild(current.as_deref());
        }
    }

    /// Something changed on disk: lists the affected folders again, a moment later (changes
    /// come in bursts).
    fn folder_changed(&self, folders: Vec<PathBuf>) {
        let mut folder = self.folder.borrow_mut();
        let Some(state) = folder.as_mut() else { return };
        state.changed.extend(folders);
        state.files = None;
        state.refresh_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(250), || {
            if let Some(app) = crate::platform::app() {
                app.reload_changed_folders();
            }
        });
    }

    fn reload_changed_folders(&self) {
        {
            let mut folder = self.folder.borrow_mut();
            let Some(state) = folder.as_mut() else { return };
            let changed: Vec<PathBuf> = state.changed.drain().collect();
            for path in changed {
                if state.listings.contains_key(&path) {
                    state.listings.insert(path.clone(), list(&path));
                }
            }
            state.expanded.retain(|p| p.is_dir());
        }
        self.refresh_folder_tree();
    }

    /// Go to File (⌘P): every file in the folder (or, without one, open and recent documents),
    /// in the palette.
    pub fn open_quick_open(&self) {
        let root = self.folder.borrow().as_ref().map(|s| s.root.clone());
        let Some(root) = root else {
            let entries: Vec<Entry> = crate::palette::entries(self).into_iter().filter(|e| e.id.starts_with("doc:") || e.id.starts_with("recent:")).collect();
            self.open_palette_list(entries, "Go to an open or recent document (Open Folder… to find any file)");
            return;
        };
        let cached = self.folder.borrow().as_ref().and_then(|s| s.files.clone());
        let files = cached.unwrap_or_else(|| {
            let files = tuplepad_core::list_files(root.to_string_lossy().into_owned(), QUICK_OPEN_LIMIT).unwrap_or_default();
            if let Some(state) = self.folder.borrow_mut().as_mut() {
                state.files = Some(files.clone());
            }
            files
        });
        let entries: Vec<Entry> = files
            .iter()
            .map(|relative| {
                let path = Path::new(relative);
                Entry {
                    id: format!("open-file:{}", root.join(relative).to_string_lossy()),
                    title: path.file_name().map_or_else(|| relative.clone(), |n| n.to_string_lossy().into_owned()),
                    detail: path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                    trailing: String::new(),
                }
            })
            .collect();
        self.open_palette_list(entries, &format!("Go to a file in {}", root.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())));
    }

    /// For scripted runs: the tree as it shows.
    pub fn folder_dump(&self) -> serde_json::Value {
        let folder = self.folder.borrow();
        let Some(state) = folder.as_ref() else { return serde_json::Value::Null };
        let rows: Vec<serde_json::Value> =
            state.model.iter().map(|r| serde_json::json!([r.name.as_str(), r.depth, r.folder, r.expanded, r.current])).collect();
        serde_json::json!({"root": state.root, "rows": rows})
    }
}
