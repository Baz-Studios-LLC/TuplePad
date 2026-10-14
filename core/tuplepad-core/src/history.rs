//! Undo and redo, with typing coalesced into one step per run of keystrokes.

use std::time::{Duration, Instant};

use ropey::Rope;

use crate::types::{EditKind, LineEnding, TextSelection};

const COALESCE_WINDOW: Duration = Duration::from_secs(2);

/// One primitive change. Splices are applied in order, each in the char offsets
/// of the rope as it was at that moment.
#[derive(Clone, Debug)]
pub(crate) enum Change {
    Splice { at: usize, removed: String, inserted: String },
    /// A whole-document rewrite (e.g. converting line endings). Ropes share
    /// structure, so keeping both versions costs no extra copy of the text.
    Replace { before: Rope, after: Rope, ending_before: LineEnding, ending_after: LineEnding },
}

#[derive(Debug)]
pub(crate) struct Transaction {
    /// Identifies the document state this transaction produces, for "is modified" tracking.
    id: u64,
    pub changes: Vec<Change>,
    pub before: Vec<TextSelection>,
    pub after: Vec<TextSelection>,
    kind: EditKind,
    last_change: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    next_id: u64,
    saved_state: u64,
    coalescing_broken: bool,
}

impl History {
    /// The id of the current document state. 0 means "as loaded".
    pub fn state(&self) -> u64 {
        self.undo.last().map_or(0, |t| t.id)
    }

    pub fn mark_saved(&mut self) {
        self.saved_state = self.state();
    }

    /// Marks the current text as different from what's on disk (restored unsaved work).
    pub fn mark_unsaved(&mut self) {
        self.saved_state = u64::MAX;
    }

    pub fn is_modified(&self) -> bool {
        self.state() != self.saved_state
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The next edit starts a new undo step even if it would otherwise merge.
    pub fn break_coalescing(&mut self) {
        self.coalescing_broken = true;
    }

    pub fn record(
        &mut self,
        changes: Vec<Change>,
        before: Vec<TextSelection>,
        after: Vec<TextSelection>,
        kind: EditKind,
    ) {
        self.redo.clear();
        let now = Instant::now();
        let ends_line = kind == EditKind::Typing
            && changes.iter().any(|c| matches!(c, Change::Splice { inserted, .. } if inserted.contains(['\n', '\r'])));
        self.next_id += 1;
        let id = self.next_id;

        let saved_state = self.saved_state;
        let broken = std::mem::take(&mut self.coalescing_broken);
        if let Some(last) = self.undo.last_mut() {
            let continues = match kind {
                EditKind::Composition => last.kind == EditKind::Composition,
                EditKind::Typing | EditKind::Deletion => last.kind == kind && last.after == before,
                EditKind::Other => false,
            };
            if continues && !broken && last.id != saved_state && now - last.last_change < COALESCE_WINDOW {
                last.changes.extend(changes);
                last.after = after;
                last.id = id;
                last.last_change = now;
                self.coalescing_broken = ends_line;
                return;
            }
        }

        self.undo.push(Transaction { id, changes, before, after, kind, last_change: now });
        self.coalescing_broken = ends_line;
    }

    pub fn pop_undo(&mut self) -> Option<Transaction> {
        self.coalescing_broken = true;
        self.undo.pop()
    }

    pub fn push_redo(&mut self, transaction: Transaction) {
        self.redo.push(transaction);
    }

    pub fn pop_redo(&mut self) -> Option<Transaction> {
        self.coalescing_broken = true;
        self.redo.pop()
    }

    pub fn push_undo(&mut self, transaction: Transaction) {
        self.undo.push(transaction);
    }
}
