//! Split view: two panes over the same documents. The editor's own fields are always the
//! focused pane's view; the other pane's view (its rows, models, carets and scroll position for
//! each document, and so on) waits in a `PaneView`. Drawing or handling events in the other pane
//! swaps the two for the duration, and focusing it swaps them for good. Documents (text, undo,
//! folds, bookmarks) are shared.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use slint::{ModelRc, VecModel};
use tuplepad_core::TextSelection;

use super::{Editor, RowMap};
use crate::EditorBridge;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    Off,
    SideBySide,
    Stacked,
}

/// Defines the view state that belongs to a pane, and how to trade it with the editor's.
macro_rules! pane_view {
    ($($field:ident: $ty:ty = $init:expr),* $(,)?) => {
        pub struct PaneView {
            /// Which pane this is (0 or 1): which of the bridge's properties it draws into.
            pane: usize,
            /// The document it shows.
            current: Option<String>,
            /// Its carets and scroll position in each document.
            views: HashMap<String, (Vec<TextSelection>, f64, f64)>,
            $($field: $ty),*
        }

        impl PaneView {
            fn fields(pane: usize) -> Self {
                PaneView { pane, current: None, views: HashMap::new(), $($field: $init),* }
            }
        }

        impl Editor {
            fn trade_fields(&mut self, other: &mut PaneView) {
                $(std::mem::swap(&mut self.$field, &mut other.$field);)*
            }
        }
    };
}

pane_view! {
    width: f32 = 0.0,
    height: f32 = 0.0,
    gutter: f32 = 64.0,
    rows: RowMap = RowMap::plain(1),
    rows_key: Option<super::RowsKey> = None,
    rows_version: u64 = 0,
    rows_folds: Vec<tuplepad_core::FoldRange> = Vec::new(),
    foldable: Option<((String, u64, u64, u64, usize), Vec<u64>)> = None,
    dirty: Option<super::Dirty> = None,
    row_model: Rc<VecModel<crate::Row>> = Rc::new(VecModel::default()),
    slot_keys: Vec<u64> = Vec::new(),
    caret_model: Rc<VecModel<crate::Caret>> = Rc::new(VecModel::default()),
    selection_model: Rc<VecModel<crate::Band>> = Rc::new(VecModel::default()),
    occurrence_model: Rc<VecModel<crate::Band>> = Rc::new(VecModel::default()),
    bracket_model: Rc<VecModel<crate::Band>> = Rc::new(VecModel::default()),
    underline_model: Rc<VecModel<crate::Band>> = Rc::new(VecModel::default()),
    active_model: Rc<VecModel<i32>> = Rc::new(VecModel::default()),
    find_model: Rc<VecModel<crate::Band>> = Rc::new(VecModel::default()),
    current_match_model: Rc<VecModel<crate::Band>> = Rc::new(VecModel::default()),
    misspelled_model: Rc<VecModel<crate::Band>> = Rc::new(VecModel::default()),
    spell_cache: Option<((String, u64, u64, u64, u64), Vec<TextSelection>)> = None,
    mark_model: Rc<VecModel<crate::Band>> = Rc::new(VecModel::default()),
    flash_model: Rc<VecModel<crate::Band>> = Rc::new(VecModel::default()),
    control_model: Rc<VecModel<crate::Band>> = Rc::new(VecModel::default()),
    goals: Option<Vec<f32>> = None,
    preedit: String = String::new(),
    composed: bool = false,
    mirror: (String, i32) = (String::new(), -1),
    mirror_sync: i32 = 0,
    mirror_window: (String, u64, u64) = (String::new(), 0, 0),
    drag: Option<super::Drag> = None,
    column_drag: Option<(u64, f32)> = None,
    text_drag: Option<super::TextDrag> = None,
    last_click: Option<(Instant, f32, f32, u32)> = None,
    scrollbar_until: Instant = Instant::now(),
    scrollbar_hover: bool = false,
    caret_shown: bool = true,
    wrap_cache: HashMap<u64, (u64, u32, u32, tuplepad_core::WrappedLine)> = HashMap::new(),
    completion: Option<crate::completion::Completion> = None,
    completion_shown: Option<(crate::completion::Completion, i32, f32)> = None,
    minimap: super::minimap::Minimap = Default::default(),
    minimap_first: u64 = 0,
    smart: Option<tuplepad_core::SearchQuery> = None,
    smart_for: Option<(u64, Vec<TextSelection>)> = None,
}

impl PaneView {
    /// The second pane, drawing into the bridge's `-2` properties.
    pub fn second(bridge: &EditorBridge) -> Box<PaneView> {
        let view = PaneView::fields(1);
        bridge.set_rows_2(ModelRc::from(view.row_model.clone()));
        bridge.set_carets_2(ModelRc::from(view.caret_model.clone()));
        bridge.set_selections_2(ModelRc::from(view.selection_model.clone()));
        bridge.set_occurrences_2(ModelRc::from(view.occurrence_model.clone()));
        bridge.set_brackets_2(ModelRc::from(view.bracket_model.clone()));
        bridge.set_preedit_underlines_2(ModelRc::from(view.underline_model.clone()));
        bridge.set_active_rows_2(ModelRc::from(view.active_model.clone()));
        bridge.set_find_matches_2(ModelRc::from(view.find_model.clone()));
        bridge.set_current_matches_2(ModelRc::from(view.current_match_model.clone()));
        bridge.set_marks_2(ModelRc::from(view.mark_model.clone()));
        bridge.set_misspelled_2(ModelRc::from(view.misspelled_model.clone()));
        bridge.set_flashes_2(ModelRc::from(view.flash_model.clone()));
        bridge.set_controls_2(ModelRc::from(view.control_model.clone()));
        Box::new(view)
    }
}

impl Editor {
    /// Trades the focused pane's view for the other one's.
    fn swap_panes(&mut self) {
        let Some(mut other) = self.other.take() else { return };
        self.trade_fields(&mut other);
        std::mem::swap(&mut self.pane, &mut other.pane);
        // Each pane's document, by id (the list may have changed meanwhile).
        let mine = self.docs.get(self.current).map(|d| d.id.clone());
        let theirs = other.current.take().and_then(|id| self.docs.iter().position(|d| d.id == id));
        other.current = mine;
        if let Some(index) = theirs {
            self.current = index;
        }
        // Carets and scroll positions, document by document. A document the incoming pane hasn't
        // shown starts where the other pane was.
        for doc in &mut self.docs {
            let outgoing = (doc.cursors.clone(), doc.scroll_x, doc.scroll_y);
            if let Some((cursors, x, y)) = other.views.remove(&doc.id) {
                // The other pane may have edited it meanwhile.
                let buffer = &doc.buffer;
                doc.cursors = cursors.into_iter().map(|c| TextSelection { anchor: buffer.clamp(c.anchor), head: buffer.clamp(c.head) }).collect();
                doc.scroll_x = x;
                doc.scroll_y = y;
            }
            other.views.insert(doc.id.clone(), outgoing);
        }
        other.views.retain(|id, _| self.docs.iter().any(|d| &d.id == id));
        self.other = Some(other);
    }

    /// Runs `f` with `pane`'s view in place (the focused one is always in place).
    pub fn with_pane<R>(&mut self, pane: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        if pane == self.pane || self.other.is_none() {
            return f(self);
        }
        self.swap_panes();
        let result = f(self);
        self.swap_panes();
        result
    }

    /// The pane with the keyboard.
    pub fn focused_pane(&self) -> usize {
        self.pane
    }

    pub fn focus_pane(&mut self, pane: usize) {
        if pane != self.pane && self.split != Split::Off {
            self.swap_panes();
            self.touch();
        }
    }

    pub fn split(&self) -> Split {
        self.split
    }

    /// Splits the view (the new pane starts on the same document, at the same place), or
    /// joins it again, keeping the focused pane's document.
    pub fn set_split(&mut self, split: Split) {
        if split == self.split {
            return;
        }
        if split == Split::Off {
            let showing = self.docs.get(self.current).map(|d| d.id.clone());
            if self.pane != 0 {
                self.swap_panes();
            }
            if let Some(index) = showing.and_then(|id| self.docs.iter().position(|d| d.id == id)) {
                self.select_document(index);
            }
        } else if self.split == Split::Off {
            let doc = &self.docs[self.current];
            let (id, view) = (doc.id.clone(), (doc.cursors.clone(), doc.scroll_x, doc.scroll_y));
            if let Some(other) = self.other.as_mut() {
                other.current = Some(id.clone());
                other.views.insert(id, view);
                // Rebuild its rows for whatever it shows now.
                other.rows_key = None;
                other.slot_keys.clear();
            }
        }
        self.split = split;
        self.invalidate();
        self.touch();
    }

    /// For scripted runs: each pane's document and carets.
    pub fn panes_state(&mut self) -> Vec<(String, Vec<TextSelection>)> {
        let panes = if self.split == Split::Off { 1 } else { 2 };
        (0..panes).map(|pane| self.with_pane(pane, |e| (e.doc().name(), e.doc().cursors.clone()))).collect()
    }
}
