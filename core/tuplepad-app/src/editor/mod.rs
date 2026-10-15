//! The editor view: turns the current document into the rows, carets and highlights Slint draws,
//! and maps the pointer back to positions. Editing itself lives in the core
//! (`tuplepad_core::editing`); keys and the mouse are in `input`.

pub mod display;
pub mod folding;
pub mod minimap;
pub mod panes;
pub mod geometry;
pub mod input;
pub mod layout;
pub mod rows;

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tuplepad_core::{FoldRange, HighlightStyle, Indentation, SearchQuery, TextBuffer, TextPos, TextSelection, WrappedLine};

use crate::document::Document;
use crate::{AppWindow, Band, Caret, CompletionItem, EditorBridge, Row};
use display::DisplayLine;
use geometry::{LONG_LINE, LineGeometry};
use layout::Layouter;
use rows::RowMap;

const BLINK: Duration = Duration::from_millis(530);
const SCROLLBAR_LINGER: Duration = Duration::from_millis(900);
/// How long the caret rests before other occurrences of its word light up.
const SMART_HIGHLIGHT_DELAY: Duration = Duration::from_millis(350);

/// Display options from Settings (and the View menu).
#[derive(Clone, Debug, PartialEq)]
pub struct ViewOptions {
    pub show_invisibles: bool,
    pub show_indent_guides: bool,
    pub highlight_current_line: bool,
    pub smart_highlight: bool,
    pub auto_close: bool,
    pub tab_width: u32,
    pub default_indentation: Indentation,
    pub minimap: bool,
    pub spelling: bool,
}

/// What a mouse drag extends: the caret, word or line first clicked.
pub(crate) struct Drag {
    origin: TextSelection,
    unit: u32,
    /// The selection the drag currently owns.
    current: TextSelection,
}

/// Selected text being dragged: it moves once the pointer has gone a few pixels.
pub(crate) struct TextDrag {
    selection: TextSelection,
    origin: (f32, f32),
    started: bool,
    target: Option<TextPos>,
}

/// The lines changed by the last edit, so wrapped row counts can be updated without
/// re-measuring the whole document.
enum Dirty {
    Lines { start: u64, end_new: u64, old_count: u64 },
    All,
}

#[derive(Clone, PartialEq)]
struct RowsKey {
    doc: String,
    wrap: bool,
    cells: u32,
    tab_width: u32,
}

/// Sets one of the bridge's per-pane properties: `set_x` for the first pane, `set_x_2` for the
/// second.
macro_rules! put {
    ($bridge:expr, $pane:expr, $set:ident, $set2:ident, $value:expr) => {
        if $pane == 0 { $bridge.$set($value) } else { $bridge.$set2($value) }
    };
}

pub struct Editor {
    pub docs: Vec<Document>,
    pub current: usize,
    pub layouter: Layouter,
    pub family: String,
    pub font_size: f32,
    pub line_height_factor: f32,
    pub line_height: f32,
    pub options: ViewOptions,
    scale: f32,
    width: f32,
    height: f32,
    gutter: f32,
    /// The width of a space, used for tab stops, wrapping and indent guides.
    cell: f32,
    rows: RowMap,
    rows_key: Option<RowsKey>,
    rows_version: u64,
    /// The folds the row map hides.
    rows_folds: Vec<FoldRange>,
    /// Lines on screen where a region could fold, for the gutter's controls.
    foldable: Option<((String, u64, u64, u64, usize), Vec<u64>)>,
    dirty: Option<Dirty>,
    /// Bumped whenever every row must be rebuilt (another document, font or options).
    epoch: u64,
    row_model: Rc<VecModel<Row>>,
    slot_keys: Vec<u64>,
    caret_model: Rc<VecModel<Caret>>,
    selection_model: Rc<VecModel<Band>>,
    occurrence_model: Rc<VecModel<Band>>,
    bracket_model: Rc<VecModel<Band>>,
    underline_model: Rc<VecModel<Band>>,
    active_model: Rc<VecModel<i32>>,
    /// The x each caret aims for while moving up and down.
    goals: Option<Vec<f32>>,
    preedit: String,
    composed: bool,
    drag: Option<Drag>,
    /// A column selection in progress: the line and x where it started.
    column_drag: Option<(u64, f32)>,
    /// Selected text being dragged to another place.
    text_drag: Option<TextDrag>,
    last_click: Option<(Instant, f32, f32, u32)>,
    last_activity: Instant,
    scrollbar_until: Instant,
    scrollbar_hover: bool,
    caret_shown: bool,
    /// Where very long lines wrap (slow to work out), by line: the buffer version, width and
    /// tab width it was worked out for.
    wrap_cache: std::collections::HashMap<u64, (u64, u32, u32, WrappedLine)>,
    /// The find bar's search, highlighted on screen while the bar is open.
    pub find_query: Option<SearchQuery>,
    /// Word completion's list, while it's showing.
    pub completion: Option<crate::completion::Completion>,
    minimap: minimap::Minimap,
    /// The first line the minimap shows.
    minimap_first: u64,
    /// The list as last shown, with its row and x.
    completion_shown: Option<(crate::completion::Completion, i32, f32)>,
    find_model: Rc<VecModel<Band>>,
    /// The find bar's match that is selected, drawn stronger than the others.
    current_match_model: Rc<VecModel<Band>>,
    /// Misspelled words on screen, and what they were found for.
    misspelled_model: Rc<VecModel<Band>>,
    spell_cache: Option<((String, u64, u64, u64, u64), Vec<TextSelection>)>,
    /// Words the user taught the spell checker, and a count of changes to them.
    pub spelling_words: std::sync::Arc<std::collections::HashSet<String>>,
    pub spelling_generation: u64,
    mark_model: Rc<VecModel<Band>>,
    /// What AI tools just changed (over MCP), by document id, highlighted while it fades.
    pub flashes: HashMap<String, Vec<TextSelection>>,
    flash_model: Rc<VecModel<Band>>,
    /// What smart highlighting shows, and the caret state it was found for.
    smart: Option<SearchQuery>,
    smart_for: Option<(u64, Vec<TextSelection>)>,
    /// Scripted runs keep copy and paste away from the real clipboard.
    pub private_clipboard: Option<String>,
    /// Text copied or cut in TuplePad, newest first (Paste from History).
    pub clip_history: Vec<String>,
    /// Milliseconds the last `sync` took, for benchmarks.
    pub last_sync_ms: f64,
    /// Which pane the view fields above belong to (the focused one), the other pane's view,
    /// and how the panes are split.
    pane: usize,
    other: Option<Box<panes::PaneView>>,
    split: panes::Split,
}

impl Editor {
    pub fn new(ui: &AppWindow, family: &str, font_size: f32, line_height_factor: f32, options: ViewOptions) -> Self {
        let scale = ui.window().scale_factor();
        let bridge = ui.global::<EditorBridge>();
        let row_model = Rc::new(VecModel::default());
        let caret_model = Rc::new(VecModel::default());
        let selection_model = Rc::new(VecModel::default());
        let occurrence_model = Rc::new(VecModel::default());
        let bracket_model = Rc::new(VecModel::default());
        let underline_model = Rc::new(VecModel::default());
        let active_model = Rc::new(VecModel::default());
        let find_model = Rc::new(VecModel::default());
        let current_match_model = Rc::new(VecModel::default());
        let misspelled_model = Rc::new(VecModel::default());
        bridge.set_misspelled(ModelRc::from(misspelled_model.clone()));
        let mark_model = Rc::new(VecModel::default());
        bridge.set_find_matches(ModelRc::from(find_model.clone()));
        bridge.set_current_matches(ModelRc::from(current_match_model.clone()));
        bridge.set_marks(ModelRc::from(mark_model.clone()));
        let flash_model = Rc::new(VecModel::default());
        bridge.set_flashes(ModelRc::from(flash_model.clone()));
        bridge.set_rows(ModelRc::from(row_model.clone()));
        bridge.set_carets(ModelRc::from(caret_model.clone()));
        bridge.set_selections(ModelRc::from(selection_model.clone()));
        bridge.set_occurrences(ModelRc::from(occurrence_model.clone()));
        bridge.set_brackets(ModelRc::from(bracket_model.clone()));
        bridge.set_preedit_underlines(ModelRc::from(underline_model.clone()));
        bridge.set_active_rows(ModelRc::from(active_model.clone()));
        let mut layouter = Layouter::new(family, font_size, scale);
        let cell = layouter.advance(" ");
        let now = Instant::now();
        Editor {
            docs: Vec::new(),
            current: 0,
            layouter,
            family: family.to_string(),
            font_size,
            line_height_factor,
            line_height: (font_size * line_height_factor).round(),
            options,
            scale,
            width: 800.0,
            height: 600.0,
            gutter: 64.0,
            cell,
            rows: RowMap::plain(1),
            rows_folds: Vec::new(),
            foldable: None,
            rows_key: None,
            rows_version: 0,
            dirty: None,
            epoch: 0,
            row_model,
            slot_keys: Vec::new(),
            caret_model,
            selection_model,
            occurrence_model,
            bracket_model,
            underline_model,
            active_model,
            goals: None,
            preedit: String::new(),
            composed: false,
            drag: None,
            column_drag: None,
            text_drag: None,
            last_click: None,
            last_activity: now,
            scrollbar_until: now,
            scrollbar_hover: false,
            caret_shown: true,
            wrap_cache: std::collections::HashMap::new(),
            find_query: None,
            completion: None,
            completion_shown: None,
            minimap: Default::default(),
            minimap_first: 0,
            find_model,
            current_match_model,
            misspelled_model,
            spell_cache: None,
            spelling_words: Default::default(),
            spelling_generation: 0,
            mark_model,
            flashes: HashMap::new(),
            flash_model,
            smart: None,
            smart_for: None,
            private_clipboard: None,
            clip_history: Vec::new(),
            last_sync_ms: 0.0,
            pane: 0,
            other: Some(panes::PaneView::second(&bridge)),
            split: panes::Split::Off,
        }
    }

    pub fn doc(&self) -> &Document {
        &self.docs[self.current]
    }

    pub fn doc_mut(&mut self) -> &mut Document {
        &mut self.docs[self.current]
    }

    pub fn buffer(&self) -> std::sync::Arc<TextBuffer> {
        self.doc().buffer.clone()
    }

    pub fn indentation(&self) -> Indentation {
        self.doc().indentation.unwrap_or(self.options.default_indentation)
    }

    /// Whether an input method is composing text.
    pub fn composing(&self) -> bool {
        !self.preedit.is_empty()
    }

    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Everything changed: rebuild every row on the next sync.
    pub fn invalidate(&mut self) {
        self.epoch += 1;
        self.rows_key = None;
        self.wrap_cache.clear();
    }

    /// Where a line wraps, if the document wraps.
    fn wrap_of(&mut self, buffer: &TextBuffer, line: u64) -> Option<WrappedLine> {
        if !self.doc().wrap {
            return None;
        }
        let (cells, tab) = (self.wrap_cells(), self.options.tab_width);
        if buffer.line_length(line) as usize <= LONG_LINE {
            return buffer.wrap_layout(cells, tab, line, 1).into_iter().next();
        }
        let version = buffer.version();
        if let Some((v, c, t, wrap)) = self.wrap_cache.get(&line)
            && (*v, *c, *t) == (version, cells, tab)
        {
            return Some(wrap.clone());
        }
        let wrap = buffer.wrap_layout(cells, tab, line, 1).into_iter().next()?;
        if self.wrap_cache.len() > 64 {
            self.wrap_cache.clear();
        }
        self.wrap_cache.insert(line, (version, cells, tab, wrap.clone()));
        Some(wrap)
    }

    // MARK: Documents

    pub fn add_document(&mut self, doc: Document) -> usize {
        self.docs.push(doc);
        self.docs.len() - 1
    }

    pub fn select_document(&mut self, index: usize) {
        if index < self.docs.len() && index != self.current {
            self.current = index;
            self.completion = None;
            self.preedit.clear();
            self.goals = None;
            self.drag = None;
            self.smart = None;
            self.smart_for = None;
            self.invalidate();
            self.touch();
        }
    }

    /// Shows a document even if it's already the current one (after the list changed).
    pub fn show_document(&mut self, index: usize) {
        if self.docs.is_empty() {
            return;
        }
        self.current = usize::MAX;
        self.select_document(index.min(self.docs.len() - 1));
    }

    /// Removes a document (the app has already asked about unsaved changes).
    pub fn remove_document(&mut self, index: usize) -> Option<Document> {
        if index >= self.docs.len() {
            return None;
        }
        let doc = self.docs.remove(index);
        if self.current > index || self.current >= self.docs.len() {
            self.current = self.current.saturating_sub(1);
        }
        self.invalidate();
        Some(doc)
    }

    // MARK: Metrics

    pub fn resize(&mut self, width: f32, height: f32, scale: f32) {
        self.width = width;
        self.height = height;
        if scale != self.scale {
            self.scale = scale;
            self.layouter.configure(&self.family, self.font_size, scale);
            self.cell = self.layouter.advance(" ");
            self.invalidate();
        }
    }

    pub fn set_font(&mut self, family: &str, size: f32, line_height_factor: f32) {
        let size = size.clamp(8.0, 48.0);
        let line_height = (size * line_height_factor).round();
        let ratio = line_height as f64 / self.line_height as f64;
        self.family = family.to_string();
        self.font_size = size;
        self.line_height_factor = line_height_factor;
        self.line_height = line_height;
        self.layouter.configure(family, size, self.scale);
        self.cell = self.layouter.advance(" ");
        for doc in &mut self.docs {
            doc.scroll_y *= ratio;
        }
        self.invalidate();
    }

    pub fn set_options(&mut self, options: ViewOptions) {
        if options != self.options {
            self.options = options;
            self.invalidate();
        }
    }

    fn visible_rows(&self) -> u64 {
        ((self.height / self.line_height).floor() as u64).max(1)
    }

    fn text_width(&self) -> f32 {
        (self.width - self.gutter - self.minimap_width()).max(1.0)
    }

    /// Room the minimap (and the scrollbar strip beside it) takes, when it's on.
    fn minimap_width(&self) -> f32 {
        if self.options.minimap && self.width > 500.0 { minimap::WIDTH + minimap::SCROLLBAR } else { 0.0 }
    }

    /// A click or drag on the minimap: centers the line under the pointer.
    pub fn minimap_scroll(&mut self, y: f32) {
        self.ensure_rows();
        let line = (self.minimap_first + (y.max(0.0) / minimap::LINE) as u64).min(self.doc().buffer.line_count().saturating_sub(1));
        let row = self.rows.first_row(line) as f64;
        let lh = self.line_height as f64;
        self.doc_mut().scroll_y = row * lh - (self.height as f64 - lh) / 2.0;
        self.clamp_scroll();
        self.touch();
    }

    /// How many cells fit on a row when wrapping.
    pub fn wrap_cells(&self) -> u32 {
        (((self.text_width() - 28.0) / self.cell).floor() as u32).max(20)
    }

    // MARK: Rows

    /// Brings the row map up to date with the document, its wrapping and the view's width.
    fn ensure_rows(&mut self) {
        let doc = &self.docs[self.current];
        let buffer = doc.buffer.clone();
        let key = RowsKey { doc: doc.id.clone(), wrap: doc.wrap, cells: if doc.wrap { self.wrap_cells() } else { 0 }, tab_width: self.options.tab_width };
        let version = buffer.version();
        let line_count = buffer.line_count();
        let folds = buffer.folds();
        let rebuild = |key: &RowsKey| {
            let mut map = if key.wrap {
                RowMap::wrapped(buffer.wrap_rows(key.cells, key.tab_width, 0, line_count))
            } else {
                RowMap::plain(line_count)
            };
            map.hide(&folds);
            map
        };
        if self.rows_key.as_ref() != Some(&key) || self.rows_folds != folds {
            // Keep the same line at the top when the wrapping changes.
            let top_line = if self.rows_key.as_ref().is_some_and(|k| k.doc == key.doc) {
                let row = (self.doc().scroll_y / self.line_height as f64).floor() as u64;
                Some(self.rows.line_at(row).0)
            } else {
                None
            };
            self.rows = rebuild(&key);
            if let Some(line) = top_line {
                let row = self.rows.first_row(line);
                self.docs[self.current].scroll_y = row as f64 * self.line_height as f64;
            }
            self.rows_key = Some(key);
            self.rows_version = version;
            self.rows_folds = folds;
            self.dirty = None;
            return;
        }
        if version == self.rows_version {
            return;
        }
        self.rows_version = version;
        let dirty = self.dirty.take();
        if !key.wrap || !folds.is_empty() {
            self.rows = rebuild(&key);
            return;
        }
        match dirty {
            Some(Dirty::Lines { start, end_new, old_count }) => {
                let end_new = end_new.min(line_count - 1);
                let end_old = end_new as i64 - (line_count as i64 - old_count as i64);
                if end_old < start as i64 - 1 || start > end_new {
                    self.rows = rebuild(&key);
                } else {
                    let new_rows = buffer.wrap_rows(key.cells, key.tab_width, start, end_new - start + 1);
                    self.rows.splice(start, (end_old - start as i64 + 1).max(0) as u64, new_rows);
                    if self.rows.line_count() != line_count {
                        self.rows = rebuild(&key);
                    }
                }
            }
            _ => self.rows = rebuild(&key),
        }
    }

    /// Notes which lines an edit touched (`before` and `after` are the cursors around it).
    fn note_edit(&mut self, old_count: u64, before: &[TextSelection], after: &[TextSelection]) {
        let start = before.iter().chain(after).map(|c| c.start().line).min().unwrap_or(0);
        let end_new = after.iter().map(|c| c.end().line).max().unwrap_or(start).max(start);
        self.dirty = match self.dirty {
            None => Some(Dirty::Lines { start, end_new, old_count }),
            Some(_) => Some(Dirty::All),
        };
    }

    /// The editor changed text some other way (undo, reload): re-measure every line.
    fn note_any_edit(&mut self) {
        self.dirty = Some(Dirty::All);
    }

    // MARK: Geometry

    pub fn tab_width(&self) -> usize {
        self.options.tab_width as usize
    }

    /// How a line is drawn, with composing text when it's the main caret's line. For a very long
    /// line only `window` (source columns) is read from the buffer.
    fn build_geometry(&mut self, buffer: &TextBuffer, line: u64, wrap: Option<&WrappedLine>, window: Option<(usize, usize)>) -> LineGeometry {
        let len = buffer.line_length(line) as usize;
        let (breaks, indent_x) = match wrap {
            Some(w) => (w.breaks.iter().map(|&b| b as usize).collect(), w.indent as f32 * self.cell),
            None => (Vec::new(), 0.0),
        };
        if len > LONG_LINE {
            let (from, to) = window.unwrap_or((0, 0));
            let text = if to > from { buffer.line_slice(line, from as u64, (to - from) as u64) } else { String::new() };
            return LineGeometry::grid(self.cell, from, text, breaks, indent_x, len);
        }
        let text = buffer.line(line);
        let primary = self.doc().cursors.last().map(|c| c.head);
        let preedit = match primary {
            Some(head) if !self.preedit.is_empty() && head.line == line => Some((head.column as usize, self.preedit.as_str())),
            _ => None,
        };
        let display = DisplayLine::new(&text, preedit, self.tab_width());
        let layout = self.layouter.line(&display.text);
        let preedit_at = preedit.map(|(at, _)| at);
        LineGeometry::laid(display, layout, breaks, indent_x, len, preedit_at)
    }

    pub fn geometry(&mut self, line: u64) -> LineGeometry {
        let buffer = self.buffer();
        let wrap = self.wrap_of(&buffer, line);
        self.build_geometry(&buffer, line, wrap.as_ref(), None)
    }

    /// The visual row and x (in text coordinates, before horizontal scrolling) of a position.
    pub fn locate(&mut self, pos: TextPos) -> (u64, f32) {
        self.ensure_rows();
        let geometry = self.geometry(pos.line);
        let (sub, x) = geometry.locate(pos.column as usize);
        (self.rows.first_row(pos.line) + sub as u64, x)
    }

    /// The position on a visual row nearest to `x`.
    pub fn position_on_row(&mut self, row: u64, x: f32) -> TextPos {
        self.ensure_rows();
        let (line, sub) = self.rows.line_at(row);
        let geometry = self.geometry(line);
        TextPos::new(line, geometry.column_at(sub as usize, x) as u64)
    }

    /// The document position under a point in the text area.
    pub fn hit(&mut self, x: f32, y: f32) -> TextPos {
        self.ensure_rows();
        let (scroll_x, scroll_y) = (self.doc().scroll_x, self.doc().scroll_y);
        let row = ((scroll_y + y as f64) / self.line_height as f64).floor();
        if row < 0.0 {
            return TextPos::new(0, 0);
        }
        if row as u64 >= self.rows.total_rows() {
            return self.buffer().end_position();
        }
        self.position_on_row(row as u64, x + scroll_x as f32)
    }

    // MARK: Scrolling

    fn max_scroll_y(&mut self) -> f64 {
        let content = self.rows.total_rows() as f64 * self.line_height as f64;
        let view = self.height as f64;
        if content <= view { 0.0 } else { content - view * 0.6 }
    }

    fn max_scroll_x(&mut self) -> f64 {
        if self.doc().wrap {
            return 0.0;
        }
        let chars = self.doc().buffer.max_line_length() as f64;
        let widest = chars * self.cell as f64 + 80.0;
        (widest - self.text_width() as f64).max(0.0)
    }

    fn clamp_scroll(&mut self) {
        let max_y = self.max_scroll_y();
        let max_x = self.max_scroll_x();
        let doc = self.doc_mut();
        doc.scroll_y = doc.scroll_y.clamp(0.0, max_y);
        doc.scroll_x = doc.scroll_x.clamp(0.0, max_x);
    }

    /// Scrolls just enough to show the main caret.
    /// Like `reveal`, but a caret out of view lands mid-view, with context around it (for
    /// jumping to search matches).
    pub fn reveal_centered(&mut self) {
        let Some(head) = self.doc().cursors.last().map(|c| c.head) else { return };
        let (row, _) = self.locate(head);
        let (lh, view) = (self.line_height as f64, self.height as f64);
        let top = row as f64 * lh;
        let doc = self.doc_mut();
        if top < doc.scroll_y || top + lh > doc.scroll_y + view {
            doc.scroll_y = (top - (view - lh) / 2.0).max(0.0);
        }
        self.reveal();
    }

    pub fn reveal(&mut self) {
        let Some(head) = self.doc().cursors.last().map(|c| c.head) else { return };
        let (row, x) = self.locate(head);
        let lh = self.line_height as f64;
        let top = row as f64 * lh;
        let view = self.height as f64;
        let text_width = self.text_width() as f64;
        let (x, wrap) = (x as f64, self.doc().wrap);
        let doc = self.doc_mut();
        if top < doc.scroll_y {
            doc.scroll_y = top;
        } else if top + lh > doc.scroll_y + view {
            doc.scroll_y = top + lh - view;
        }
        if !wrap {
            if x < doc.scroll_x + 8.0 {
                doc.scroll_x = (x - 48.0).max(0.0);
            } else if x > doc.scroll_x + text_width - 24.0 {
                doc.scroll_x = x - text_width + 72.0;
            }
        }
        self.clamp_scroll();
    }

    /// Puts a line near the top third of the view, with the caret at its start.
    pub fn go_to_line(&mut self, line: u64) {
        let line = line.min(self.doc().buffer.line_count().saturating_sub(1));
        self.doc_mut().cursors = vec![TextSelection::caret(TextPos::new(line, 0))];
        self.ensure_rows();
        let row = self.rows.first_row(line);
        let (lh, view) = (self.line_height as f64, self.height as f64);
        self.doc_mut().scroll_y = (row as f64 * lh - view / 3.0).max(0.0);
        self.clamp_scroll();
        self.touch();
    }

    /// Moves the caret to a line's start, scrolling only if the line is out of view.
    pub fn jump_to_line(&mut self, line: u64) {
        self.ensure_rows();
        let row = self.rows.first_row(line) as f64;
        let (lh, top, view) = (self.line_height as f64, self.doc().scroll_y, self.height as f64);
        if row * lh >= top && (row + 1.0) * lh <= top + view {
            self.doc_mut().cursors = vec![TextSelection::caret(TextPos::new(line, 0))];
            self.touch();
        } else {
            self.go_to_line(line);
        }
    }

    /// The line at the top of the view.
    pub fn top_line(&self) -> u64 {
        let row = (self.doc().scroll_y / self.line_height as f64).floor().max(0.0) as u64;
        if self.rows.is_wrapped() {
            // Without &mut access to refresh the map, the stale map is close enough here.
            return self.rows.line_at_stale(row);
        }
        row.min(self.doc().buffer.line_count().saturating_sub(1))
    }

    /// The line shown on a visual row.
    pub fn line_at_row(&mut self, row: u64) -> u64 {
        self.ensure_rows();
        self.rows.line_at(row).0
    }

    pub fn scroll_by(&mut self, dx: f32, dy: f32) {
        let doc = self.doc_mut();
        doc.scroll_y -= dy as f64;
        doc.scroll_x -= dx as f64;
        self.ensure_rows();
        self.clamp_scroll();
        self.scrollbar_until = Instant::now() + SCROLLBAR_LINGER;
    }

    pub fn scroll_to_fraction(&mut self, fraction: f32) {
        self.ensure_rows();
        let max = self.max_scroll_y();
        self.doc_mut().scroll_y = max * fraction.clamp(0.0, 1.0) as f64;
        self.scrollbar_until = Instant::now() + SCROLLBAR_LINGER;
    }

    pub fn set_scrollbar_hover(&mut self, inside: bool) {
        self.scrollbar_hover = inside;
    }

    // MARK: Drawing

    /// Brings every model the view draws up to date. Cheap when little changed: rows are only
    /// rebuilt when their content, colors or decorations changed.
    pub fn sync(&mut self, ui: &AppWindow) {
        let started = Instant::now();
        self.sync_view(ui);
        if self.split != panes::Split::Off {
            let other = 1 - self.pane;
            self.with_pane(other, |e| e.sync_view(ui));
        }
        let _ = started;
    }

    fn sync_view(&mut self, ui: &AppWindow) {
        let started = Instant::now();
        self.update_smart_highlight(started.duration_since(self.last_activity));
        self.reveal_folded_carets();
        self.ensure_rows();
        self.clamp_scroll();
        let bridge = ui.global::<EditorBridge>();
        let lh = self.line_height;
        let buffer = self.buffer();
        let line_count = buffer.line_count();
        let total_rows = self.rows.total_rows();
        let scroll_y = self.doc().scroll_y;
        let top_row = ((scroll_y / lh as f64).floor() as u64).min(total_rows.saturating_sub(1));
        let offset = (scroll_y - top_row as f64 * lh as f64) as f32;
        let slots = (self.height / lh).ceil() as usize + 2;

        // The gutter fits the longest line number.
        let digits = line_count.to_string().len().max(3);
        self.gutter = (self.layouter.advance("0") * digits as f32 + 36.0).round();

        if self.row_model.row_count() != slots {
            self.row_model.set_vec(vec![Row::default(); slots]);
            self.slot_keys = vec![u64::MAX; slots];
        }

        // The lines on screen.
        let (first_line, first_sub) = self.rows.line_at(top_row);
        let mut lines = Vec::new();
        let mut covered: i64 = -(first_sub as i64);
        let mut line = first_line;
        while covered < slots as i64 && line < line_count {
            let rows = self.rows.rows_of(line);
            // Lines inside a fold have no rows.
            if rows > 0 {
                lines.push(line);
                covered += rows as i64;
            }
            line += 1;
        }
        let count = lines.len() as u64;
        let wraps: Vec<Option<WrappedLine>> = lines.iter().map(|&line| self.wrap_of(&buffer, line)).collect();
        let long: Vec<bool> = lines.iter().map(|&line| buffer.line_length(line) as usize > LONG_LINE).collect();
        let first_rows: Vec<u64> = lines.iter().map(|&l| self.rows.first_row(l)).collect();
        // What of each line is on screen, for very long lines (which only read that part).
        let (scroll_x, cell, text_width) = (self.doc().scroll_x as f32, self.cell, self.text_width());
        let windows: Vec<(usize, usize)> = (0..lines.len())
            .map(|i| match &wraps[i] {
                Some(wrap) => {
                    let rows = wrap.breaks.len() as u64 + 1;
                    let first = top_row.saturating_sub(first_rows[i]).min(rows - 1) as usize;
                    let last = ((top_row + slots as u64).saturating_sub(first_rows[i])).min(rows - 1) as usize;
                    let start = if first == 0 { 0 } else { wrap.breaks[first - 1] as usize };
                    let end = wrap.breaks.get(last).map_or(usize::MAX / 2, |&b| b as usize);
                    (start, end)
                }
                None => (((scroll_x / cell) as usize).saturating_sub(64), ((scroll_x + text_width) / cell) as usize + 64),
            })
            .collect();
        let geometries: Vec<LineGeometry> =
            (0..lines.len()).map(|i| self.build_geometry(&buffer, lines[i], wraps[i].as_ref(), Some(windows[i]))).collect();

        // Colors: very long lines only for the part on screen.
        let mut colors: Vec<Vec<(usize, usize, i32)>> = vec![Vec::new(); lines.len()];
        let mut add = |spans: Vec<tuplepad_core::HighlightSpan>| {
            for span in spans {
                if let Ok(i) = lines.binary_search(&span.line) {
                    colors[i].push((span.start as usize, span.end as usize, style_index(span.style)));
                }
            }
        };
        if long.iter().any(|&l| l) {
            for (i, &line) in lines.iter().enumerate() {
                let (from, to) = windows[i];
                add(if long[i] { buffer.highlights_in_line(line, from as u64, to.saturating_sub(from) as u64) } else { buffer.highlights(line, 1) });
            }
        } else {
            // Runs of consecutive lines (folds leave gaps).
            let mut i = 0;
            while i < lines.len() {
                let start = i;
                while i + 1 < lines.len() && lines[i + 1] == lines[i] + 1 {
                    i += 1;
                }
                add(buffer.highlights(lines[start], (i - start + 1) as u64));
                i += 1;
            }
        }
        let _ = count;

        // Where regions can fold (in the gutter), and which are folded.
        let last_line = lines.last().copied().unwrap_or(first_line);
        let fold_key = (self.doc().id.clone(), buffer.version(), first_line, last_line, self.tab_width());
        if self.foldable.as_ref().is_none_or(|(key, _)| *key != fold_key) {
            let starts = buffer.fold_ranges(first_line, last_line - first_line + 1, self.tab_width() as u32).into_iter().map(|f| f.start).collect();
            self.foldable = Some((fold_key, starts));
        }
        let foldable = self.foldable.as_ref().map(|(_, starts)| starts.clone()).unwrap_or_default();
        let folded: Vec<u64> = self.rows_folds.iter().map(|f| f.start).collect();

        // Indent guides: blank lines take the indentation of the lines around them.
        let unit_cells = match self.indentation() {
            Indentation::Spaces(width) => width as usize,
            Indentation::Tabs => self.tab_width(),
        };
        let guide_cells: Vec<usize> = if self.options.show_indent_guides {
            (0..lines.len())
                .map(|i| {
                    if !geometries[i].blank {
                        return geometries[i].indent_cells;
                    }
                    let before = geometries[..i].iter().rev().find(|g| !g.blank).map(|g| g.indent_cells);
                    let after = geometries[i + 1..].iter().find(|g| !g.blank).map(|g| g.indent_cells);
                    before.unwrap_or(0).min(after.unwrap_or(0))
                })
                .collect()
        } else {
            vec![0; lines.len()]
        };

        let cursors = self.doc().cursors.clone();
        let active_lines: Vec<u64> = cursors.iter().map(|c| c.head.line).collect();
        let bookmarks = buffer.bookmarks();
        let invisibles = self.options.show_invisibles;

        // Rows, in fixed slots.
        let mut index = 0;
        let mut sub = first_sub as usize;
        for r in top_row..top_row + slots as u64 {
            let slot = (r % slots as u64) as usize;
            if r >= total_rows || index >= lines.len() {
                let key = hash_of(&(self.epoch, r, "past the end"));
                if self.slot_keys[slot] != key {
                    self.slot_keys[slot] = key;
                    self.row_model.set_row_data(slot, Row { row: r as i32, line: -1, ..Row::default() });
                }
                continue;
            }
            let line = lines[index];
            let geometry = &geometries[index];
            let (row_start, row_end, _) = geometry.row_range(sub);
            let current = active_lines.contains(&line);
            let guides = display::guides(guide_cells[index], unit_cells, self.cell);
            let line_break = line + 1 < line_count;
            let bookmarked = sub == 0 && bookmarks.binary_search(&line).is_ok();
            let is_folded = folded.contains(&line);
            let fold = if sub > 0 { 0 } else if is_folded { 2 } else if foldable.contains(&line) { 1 } else { 0 };
            // A folded line ends with a pill standing for what's hidden.
            let folded_end = is_folded && sub + 1 == geometry.rows();
            let folded_x = if folded_end { geometry.locate(buffer.line_length(line) as usize).1 } else { 0.0 };
            let key = hash_of(&(
                (self.epoch, r, line, sub, geometry.content_key(), &colors[index], current, row_start, row_end),
                (guide_cells[index], line_break, bookmarked, fold, folded_end, folded_x.to_bits()),
            ));
            if self.slot_keys[slot] != key {
                self.slot_keys[slot] = key;
                let spans = geometry.row_spans(sub, &colors[index], invisibles, line_break);
                self.row_model.set_row_data(
                    slot,
                    Row {
                        row: r as i32,
                        line: line as i32,
                        number: if sub == 0 { SharedString::from((line + 1).to_string()) } else { SharedString::new() },
                        spans: ModelRc::new(VecModel::from(spans)),
                        current,
                        bookmarked,
                        fold,
                        folded_end,
                        folded_x,
                        guides: ModelRc::new(VecModel::from(guides)),
                    },
                );
            }
            sub += 1;
            if sub >= geometry.rows() {
                sub = 0;
                index += 1;
            }
        }

        // Carets, selections and other bands, on the lines that are showing.
        let visible = top_row..top_row + slots as u64;
        let space = self.cell;
        let position_of = |line: u64| lines.binary_search(&line).ok();
        let primary = cursors.last().copied();
        let mut carets = Vec::new();
        let mut bands = Vec::new();
        let mut underlines = Vec::new();
        let mut active_rows = Vec::new();
        let mut ime = (top_row, 0.0);
        for cursor in &cursors {
            let head = cursor.head;
            if let Some(i) = position_of(head.line) {
                let geometry = &geometries[i];
                let is_primary = Some(*cursor) == primary;
                let composing = if is_primary { geometry.locate_after_preedit() } else { None };
                if composing.is_some() {
                    underlines.extend(geometry.preedit_band(first_rows[i]));
                }
                let (sub, x) = composing.unwrap_or_else(|| geometry.locate(head.column as usize));
                let row = first_rows[i] + sub as u64;
                if visible.contains(&row) {
                    carets.push(Caret { row: row as i32, x });
                }
                if is_primary {
                    ime = (row, x);
                }
                for sub in 0..geometry.rows() as u64 {
                    let row = first_rows[i] + sub;
                    if visible.contains(&row) && !active_rows.contains(&(row as i32)) {
                        active_rows.push(row as i32);
                    }
                }
            }
            if cursor.anchor == cursor.head {
                continue;
            }
            let (start, end) = (cursor.start(), cursor.end());
            for (i, &line) in lines.iter().enumerate() {
                if line < start.line || line > end.line {
                    continue;
                }
                let from = if line == start.line { start.column as usize } else { 0 };
                let to = if line == end.line { end.column as usize } else { usize::MAX };
                bands.extend(geometries[i].bands(first_rows[i], from, to, line < end.line, space * 0.6));
            }
        }

        // Other occurrences of the selected word, the find bar's matches, Mark All, and the
        // bracket pair at the caret. Very long lines are left out (searching them each frame is slow).
        // Bands for the matches, and separately for matches that are exactly a selection.
        let match_bands = |query: &SearchQuery| -> (Vec<Band>, Vec<Band>) {
            let mut matches = Vec::new();
            let mut i = 0;
            while i < lines.len() {
                if long[i] {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < lines.len() && !long[i] {
                    i += 1;
                }
                matches.extend(buffer.find_in_lines(query.clone(), lines[start], (i - start) as u64, 2_000).unwrap_or_default());
            }
            let (mut bands, mut selected) = (Vec::new(), Vec::new());
            for m in matches {
                let is_selected = cursors.iter().any(|c| c.start() == m.start() && c.end() == m.end());
                for line in m.start().line..=m.end().line {
                    let Some(i) = position_of(line) else { continue };
                    let from = if line == m.start().line { m.start().column as usize } else { 0 };
                    let to = if line == m.end().line { m.end().column as usize } else { usize::MAX };
                    let row_bands = geometries[i].bands(first_rows[i], from, to, false, 0.0);
                    if is_selected { selected.extend(row_bands) } else { bands.extend(row_bands) }
                }
            }
            (bands, selected)
        };
        let occurrences = match (&self.smart, &self.find_query) {
            (Some(query), None) => match_bands(query).0,
            _ => Vec::new(),
        };
        let (found, current_match) = self.find_query.as_ref().map(|q| match_bands(q)).unwrap_or_default();
        let marked = self.doc().mark.as_ref().map(|q| match_bands(q)).map(|(mut a, b)| {
            a.extend(b);
            a
        }).unwrap_or_default();
        let mut brackets = Vec::new();
        if let Some(primary) = primary
            && primary.anchor == primary.head
            && self.preedit.is_empty()
            && let Some(pair) = buffer.matching_bracket(primary.head)
        {
            for pos in [pair.open, pair.close] {
                let Some(i) = position_of(pos.line) else { continue };
                let col = pos.column as usize;
                brackets.extend(geometries[i].bands(first_rows[i], col, col + 1, false, 0.0));
            }
        }

        replace_if_changed(&self.caret_model, carets);
        replace_if_changed(&self.selection_model, bands);
        replace_if_changed(&self.occurrence_model, occurrences);
        replace_if_changed(&self.find_model, found);
        replace_if_changed(&self.current_match_model, current_match);

        // Misspelled words in the lines on screen (but not the one being typed at the caret).
        let misspelled = if self.options.spelling && tuplepad_core::spelling_ready() && !lines.is_empty() {
            let last = *lines.last().unwrap_or(&first_line);
            let key = (self.doc().id.clone(), buffer.version(), first_line, last, self.spelling_generation);
            if self.spell_cache.as_ref().is_none_or(|(k, _)| *k != key) {
                let found = buffer.misspellings(first_line, last - first_line + 1, &self.spelling_words);
                self.spell_cache = Some((key, found));
            }
            let found = self.spell_cache.as_ref().map(|(_, f)| f.clone()).unwrap_or_default();
            let typing = cursors.last().filter(|c| c.anchor == c.head).map(|c| c.head);
            let mut bands = Vec::new();
            for word in found.iter().filter(|w| Some(w.end()) != typing) {
                if let Some(i) = position_of(word.start().line) {
                    bands.extend(geometries[i].bands(first_rows[i], word.start().column as usize, word.end().column as usize, false, 0.0));
                }
            }
            bands
        } else {
            Vec::new()
        };
        replace_if_changed(&self.misspelled_model, misspelled);
        replace_if_changed(&self.mark_model, marked);
        let mut flashed = Vec::new();
        for range in self.flashes.get(&self.doc().id).into_iter().flatten() {
            for line in range.start().line..=range.end().line {
                let Some(i) = position_of(line) else { continue };
                let from = if line == range.start().line { range.start().column as usize } else { 0 };
                let to = if line == range.end().line { range.end().column as usize } else { usize::MAX };
                flashed.extend(geometries[i].bands(first_rows[i], from, to, line < range.end().line, space * 0.6));
            }
        }
        replace_if_changed(&self.flash_model, flashed);
        replace_if_changed(&self.bracket_model, brackets);
        replace_if_changed(&self.underline_model, underlines);
        replace_if_changed(&self.active_model, active_rows);

        // The minimap, and where the view is in it.
        let minimap_width = self.minimap_width();
        put!(bridge, self.pane, set_minimap_width, set_minimap_width_2, minimap_width);
        if minimap_width > 0.0 {
            let fits = (self.height / minimap::LINE).floor() as u64;
            let max_y = self.max_scroll_y();
            let fraction = if max_y > 0.0 { self.doc().scroll_y / max_y } else { 0.0 };
            let first = minimap::first_line(line_count, fits, fraction);
            self.minimap_first = first;
            let theme = ui.global::<crate::Theme>();
            let colors: Vec<slint::Color> = theme.get_syntax().iter().collect();
            let request = minimap::Request {
                buffer: &buffer,
                doc: &self.docs[self.current].id,
                first_line: first,
                lines: fits,
                height: self.height,
                scale: self.scale,
                tab_width: self.tab_width(),
                colors: &colors,
                dark: theme.get_dark(),
            };
            if self.minimap.update(&request) {
                put!(bridge, self.pane, set_minimap, set_minimap_2, self.minimap.image.clone());
            }
            let last_visible = lines.last().copied().unwrap_or(first_line);
            put!(bridge, self.pane, set_minimap_view_y, set_minimap_view_y_2, (first_line as f32 - first as f32) * minimap::LINE);
            put!(bridge, self.pane, set_minimap_view_height, set_minimap_view_height_2, ((last_visible + 1 - first_line) as f32 * minimap::LINE).max(minimap::LINE * 4.0));
        }

        // Word completion's list, under the start of the word being completed.
        let placed = self.completion.as_ref().and_then(|c| {
            let i = position_of(c.range.start().line)?;
            let (sub, x) = geometries[i].locate(c.range.start().column as usize);
            let row = first_rows[i] + sub as u64;
            visible.contains(&row).then(|| (c.clone(), row as i32, x))
        });
        if placed.is_none() {
            self.completion = None;
        }
        if placed != self.completion_shown {
            match &placed {
                Some((c, row, x)) => {
                    let chars = c.prefix.chars().count();
                    let items: Vec<CompletionItem> = c
                        .items
                        .iter()
                        .map(|word| {
                            let split = word.char_indices().nth(chars).map_or(word.len(), |(b, _)| b);
                            CompletionItem { head: word[..split].into(), tail: word[split..].into() }
                        })
                        .collect();
                    put!(bridge, self.pane, set_completions, set_completions_2, ModelRc::new(VecModel::from(items)));
                    put!(bridge, self.pane, set_completion_selected, set_completion_selected_2, c.selected as i32);
                    put!(bridge, self.pane, set_completion_row, set_completion_row_2, *row);
                    put!(bridge, self.pane, set_completion_x, set_completion_x_2, *x);
                }
                None => put!(bridge, self.pane, set_completions, set_completions_2, ModelRc::default()),
            }
            self.completion_shown = placed;
        }

        let max_y = self.max_scroll_y();
        let content = max_y + self.height as f64;
        let (scroll_x, scroll_y) = (self.doc().scroll_x, self.doc().scroll_y);
        put!(bridge, self.pane, set_top_row, set_top_row_2, top_row as i32);
        put!(bridge, self.pane, set_scroll_offset, set_scroll_offset_2, offset);
        put!(bridge, self.pane, set_scroll_x, set_scroll_x_2, scroll_x as f32);
        bridge.set_line_height(lh);
        bridge.set_font_size(self.font_size);
        put!(bridge, self.pane, set_gutter_width, set_gutter_width_2, self.gutter);
        bridge.set_highlight_current_line(self.options.highlight_current_line);
        let drop = self.text_drag.as_ref().filter(|d| d.started).and_then(|d| d.target);
        match drop {
            Some(target) => {
                let (row, x) = self.locate(target);
                put!(bridge, self.pane, set_drop_row, set_drop_row_2, row as i32);
                put!(bridge, self.pane, set_drop_x, set_drop_x_2, x);
                put!(bridge, self.pane, set_drop_visible, set_drop_visible_2, true);
            }
            None => put!(bridge, self.pane, set_drop_visible, set_drop_visible_2, false),
        }
        put!(bridge, self.pane, set_ime_row, set_ime_row_2, ime.0 as i32);
        put!(bridge, self.pane, set_ime_x, set_ime_x_2, ime.1);
        put!(bridge, self.pane, set_thumb_size, set_thumb_size_2, (self.height as f64 / content).min(1.0) as f32);
        put!(bridge, self.pane, set_thumb_start, set_thumb_start_2, if max_y > 0.0 { (scroll_y / max_y) as f32 } else { 0.0 });
        self.blink(ui);
        self.last_sync_ms = started.elapsed().as_secs_f64() * 1000.0;
    }

    /// Blinks the caret, fades the scrollbar and looks for occurrences to highlight; called by a
    /// timer. Returns true when the view needs a sync.
    pub fn tick(&mut self, ui: &AppWindow) -> bool {
        self.blink(ui);
        if self.split != panes::Split::Off {
            let other = 1 - self.pane;
            self.with_pane(other, |e| e.blink(ui));
        }
        self.update_smart_highlight(self.last_activity.elapsed())
    }

    fn blink(&mut self, ui: &AppWindow) {
        let bridge = ui.global::<EditorBridge>();
        let now = Instant::now();
        let idle = now.duration_since(self.last_activity);
        let caret = idle < BLINK || (idle.as_millis() / BLINK.as_millis()) % 2 == 0;
        if caret != self.caret_shown {
            self.caret_shown = caret;
            put!(bridge, self.pane, set_caret_visible, set_caret_visible_2, caret);
        }
        let scrollbar = now < self.scrollbar_until || self.scrollbar_hover;
        if (if self.pane == 0 { bridge.get_scrollbar_shown() } else { bridge.get_scrollbar_shown_2() }) != scrollbar {
            put!(bridge, self.pane, set_scrollbar_shown, set_scrollbar_shown_2, scrollbar);
        }
    }

    /// Smart highlighting: after a pause, the selected text (or the word at the caret) lights
    /// up wherever else it appears on screen.
    fn update_smart_highlight(&mut self, idle: Duration) -> bool {
        let state = (self.doc().buffer.version(), self.doc().cursors.clone());
        if self.smart_for.as_ref() == Some(&state) {
            return false;
        }
        if self.smart.is_some() {
            // Clear right away when things change; look again after the pause.
            self.smart = None;
            return true;
        }
        if idle < SMART_HIGHLIGHT_DELAY || !self.options.smart_highlight || !self.preedit.is_empty() {
            return false;
        }
        let buffer = self.buffer();
        let query = match state.1.as_slice() {
            [only] if only.anchor != only.head => {
                let (start, end) = (only.start(), only.end());
                let text = if start.line == end.line && end.column - start.column <= 200 { buffer.text_in(start, end) } else { String::new() };
                (!text.trim().is_empty()).then(|| SearchQuery { pattern: text, case_sensitive: true, whole_word: false, regex: false })
            }
            [only] => buffer.word_at(only.head).map(|word| SearchQuery {
                pattern: buffer.text_in(word.start(), word.end()),
                case_sensitive: true,
                whole_word: true,
                regex: false,
            }),
            _ => None,
        };
        self.smart_for = Some(state);
        self.smart = query;
        self.smart.is_some()
    }

    /// Width of a space and height of a row, and how many rows the document takes, for scripts.
    pub fn metrics(&mut self) -> (f32, f32, u64) {
        self.ensure_rows();
        (self.cell, self.line_height, self.rows.total_rows())
    }

    pub fn status_left(&self) -> String {
        let doc = self.doc();
        let Some(primary) = doc.cursors.last() else { return String::new() };
        let mut status = format!("Ln {}, Col {}", group(primary.head.line + 1), primary.head.column + 1);
        if doc.cursors.len() > 1 {
            status.push_str(&format!("   ·   {} cursors", doc.cursors.len()));
        } else if primary.anchor != primary.head {
            let chars = doc.buffer.utf16_offset(primary.end()) - doc.buffer.utf16_offset(primary.start());
            status.push_str(&format!("   ·   {} selected", group(chars)));
        }
        if !self.preedit.is_empty() {
            status.push_str("   ·   composing");
        }
        status
    }
}

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn replace_if_changed<T: Clone + PartialEq + 'static>(model: &VecModel<T>, items: Vec<T>) {
    let same = model.row_count() == items.len() && items.iter().enumerate().all(|(i, item)| model.row_data(i).as_ref() == Some(item));
    if !same {
        model.set_vec(items);
    }
}

/// The index of a highlight style in `Theme.syntax`.
pub fn style_index(style: HighlightStyle) -> i32 {
    match style {
        HighlightStyle::Keyword => 1,
        HighlightStyle::String => 2,
        HighlightStyle::Escape => 3,
        HighlightStyle::Comment => 4,
        HighlightStyle::Number => 5,
        HighlightStyle::Constant => 6,
        HighlightStyle::Function => 7,
        HighlightStyle::Type => 8,
        HighlightStyle::Variable => 9,
        HighlightStyle::Property => 10,
        HighlightStyle::Operator => 11,
        HighlightStyle::Punctuation => 12,
        HighlightStyle::Tag => 13,
        HighlightStyle::Attribute => 14,
        HighlightStyle::Label => 15,
        HighlightStyle::Namespace => 16,
        HighlightStyle::Heading => 17,
        HighlightStyle::Emphasis => 18,
        HighlightStyle::Strong => 19,
        HighlightStyle::Link => 20,
        HighlightStyle::Code => 21,
        HighlightStyle::Inserted => 22,
        HighlightStyle::Deleted => 23,
    }
}

/// 1234567 → "1,234,567".
pub fn group(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
