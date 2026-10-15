//! Code folding in the view: folds live in the core (they move with edits); here the commands,
//! and keeping carets out of what a fold hides.

use tuplepad_core::{TextPos, TextSelection, normalize_cursors};

use super::Editor;

impl Editor {
    /// A click on a fold control or a folded line's "⋯".
    pub fn toggle_fold_at_row(&mut self, row: u64) {
        let line = self.line_at_row(row);
        self.buffer().toggle_fold(line, self.tab_width() as u32);
        self.carets_out_of_folds();
    }

    /// Fold: the innermost region around each caret.
    pub fn fold_at_carets(&mut self) {
        let buffer = self.buffer();
        let mut lines: Vec<u64> = self.doc().cursors.iter().map(|c| c.head.line).collect();
        lines.dedup();
        for line in lines {
            buffer.fold(line, self.tab_width() as u32);
        }
        self.carets_out_of_folds();
    }

    /// Unfold: whatever folds at or around each caret.
    pub fn unfold_at_carets(&mut self) {
        let buffer = self.buffer();
        for cursor in self.doc().cursors.clone() {
            buffer.unfold(cursor.head.line);
        }
        self.touch();
    }

    pub fn fold_all(&mut self) {
        self.buffer().fold_all(self.tab_width() as u32);
        self.carets_out_of_folds();
    }

    pub fn unfold_all(&mut self) {
        self.buffer().unfold_all();
        self.touch();
    }

    /// Carets a new fold hides move to the end of its first line (the outermost fold's).
    fn carets_out_of_folds(&mut self) {
        let buffer = self.buffer();
        let folds = buffer.folds();
        let cursors: Vec<TextSelection> = self
            .doc()
            .cursors
            .iter()
            .map(|c| {
                let hiding = folds.iter().filter(|f| f.start < c.head.line && c.head.line <= f.end).min_by_key(|f| f.start);
                match hiding {
                    Some(f) => TextSelection::caret(TextPos::new(f.start, buffer.line_length(f.start))),
                    None => *c,
                }
            })
            .collect();
        self.doc_mut().cursors = normalize_cursors(cursors);
        self.touch();
    }

    /// A caret that gets inside a fold some other way (moving, typing, finding) opens it.
    pub(super) fn reveal_folded_carets(&mut self) {
        let buffer = self.buffer();
        if buffer.folds().is_empty() {
            return;
        }
        let lines: Vec<u64> = self.doc().cursors.iter().map(|c| c.head.line).collect();
        buffer.reveal_lines(lines);
    }
}
