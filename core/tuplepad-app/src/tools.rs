//! The Text menu: line tools, case, whitespace, comments, moving and duplicating lines. Each is
//! one call into the core, which applies it as one undo step and maps the carets through it.

use tuplepad_core::{CaseChange, Indentation, LineOperation, TextSelection};

use crate::app::App;

impl App {
    /// Runs a Text menu command. Returns false if `id` isn't one.
    pub fn run_text_tool(&self, id: &str) -> bool {
        use LineOperation::*;
        let operation = match id {
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
        let case = match id {
            "case.upper" => Some(CaseChange::Upper),
            "case.lower" => Some(CaseChange::Lower),
            "case.title" => Some(CaseChange::Title),
            "case.sentence" => Some(CaseChange::Sentence),
            "case.swap" => Some(CaseChange::Swap),
            _ => None,
        };
        let mut editor = self.editor.borrow_mut();
        let tab_width = editor.tab_width() as u32;
        if let Some(operation) = operation {
            editor.apply(|buffer, cursors| buffer.transform_lines(operation, cursors.to_vec(), tab_width));
            return true;
        }
        if let Some(change) = case {
            editor.apply(|buffer, cursors| buffer.change_case(change, cursors.to_vec()));
            return true;
        }
        match id {
            "lines.split" => {
                let width = if editor.doc().wrap { editor.wrap_cells() } else { 80 };
                editor.apply(|buffer, cursors| buffer.split_lines(width, tab_width, cursors.to_vec()));
            }
            "text.comment" => editor.apply(|buffer, cursors| buffer.toggle_comment(cursors.to_vec())),
            "text.move-up" => editor.apply(|buffer, cursors| buffer.move_lines(true, cursors.to_vec())),
            "text.move-down" => editor.apply(|buffer, cursors| buffer.move_lines(false, cursors.to_vec())),
            "text.duplicate" => editor.apply(|buffer, cursors| buffer.duplicate_lines(cursors.to_vec())),
            "text.delete-line" => editor.apply(|buffer, cursors| buffer.delete_lines(cursors.to_vec())),
            "whitespace.indent-spaces" | "whitespace.indent-tabs" => {
                // The whole document, whatever is selected; the document's indentation follows.
                let to_tabs = id == "whitespace.indent-tabs";
                let operation = if to_tabs { LeadingSpacesToTabs } else { TabsToSpaces };
                editor.apply(|buffer, cursors| {
                    let carets: Vec<TextSelection> = cursors.iter().map(|c| TextSelection::caret(c.head)).collect();
                    buffer.transform_lines(operation, carets, tab_width)
                });
                editor.doc_mut().indentation = Some(if to_tabs { Indentation::Tabs } else { Indentation::Spaces(tab_width) });
            }
            _ => return false,
        }
        true
    }
}
