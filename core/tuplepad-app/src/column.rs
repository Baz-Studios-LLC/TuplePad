//! The Column Editor: text or a run of numbers down a column, or at every caret.

use slint::ComponentHandle;
use tuplepad_core::{ColumnInsert, NumberBase, NumberPadding, column_values};

use crate::ColumnBridge;
use crate::app::App;

/// Values shown in the preview.
const PREVIEW: u32 = 8;

impl App {
    fn column_insert(&self) -> Result<ColumnInsert, String> {
        let ui = self.ui.upgrade().ok_or_else(String::new)?;
        let bridge = ui.global::<ColumnBridge>();
        if bridge.get_mode() == 0 {
            return Ok(ColumnInsert::Text { text: bridge.get_text().to_string() });
        }
        let number = |text: slint::SharedString, name: &str| text.trim().parse::<i64>().map_err(|_| format!("{name} needs a whole number"));
        let start = number(bridge.get_start(), "Start")?;
        let step = number(bridge.get_step(), "Step")?;
        let repeat = number(bridge.get_repeat(), "Repeat")?;
        if repeat < 1 {
            return Err("Repeat needs a number from 1 up".into());
        }
        let base = [NumberBase::Decimal, NumberBase::Hex, NumberBase::HexUppercase, NumberBase::Octal, NumberBase::Binary][bridge.get_base().clamp(0, 4) as usize];
        let padding = [NumberPadding::None, NumberPadding::Zeros, NumberPadding::Spaces][bridge.get_padding().clamp(0, 2) as usize];
        Ok(ColumnInsert::Numbers { start, step, repeat: repeat.min(u32::MAX as i64) as u32, base, padding })
    }

    /// How many lines the insert reaches, the way the core counts them.
    fn column_lines(&self) -> u64 {
        let editor = self.editor.borrow();
        let cursors = &editor.doc().cursors;
        match cursors.as_slice() {
            [one] if one.start().line != one.end().line => one.end().line - one.start().line + 1,
            [one] => editor.buffer().line_count() - one.head.line,
            several => several.len() as u64,
        }
    }

    pub fn open_column_editor(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        ui.global::<ColumnBridge>().set_open(true);
        self.column_changed();
    }

    /// Updates the preview as the fields change.
    pub fn column_changed(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let bridge = ui.global::<ColumnBridge>();
        let lines = self.column_lines();
        let several = self.editor.borrow().doc().cursors.len() > 1;
        let target = match (several, lines) {
            (true, n) => format!("At each of the {n} carets."),
            (false, 1) => "On this line.".to_string(),
            (false, n) => format!("Down the column, on {} lines.", crate::editor::group(n)),
        };
        bridge.set_target(target.into());
        match self.column_insert() {
            Ok(insert) => {
                let shown = lines.min(PREVIEW as u64) as usize;
                // Padding depends on the widest number of all, so work them all out (it's quick).
                let all = column_values(insert, lines.min(4_000_000) as u32);
                let mut values = all.into_iter().take(shown).map(|v| if v.is_empty() { "␣".into() } else { v }).collect::<Vec<_>>().join("   ");
                if lines > shown as u64 {
                    values.push_str("   …");
                }
                bridge.set_preview(values.into());
                bridge.set_valid(true);
            }
            Err(message) => {
                bridge.set_preview(message.into());
                bridge.set_valid(false);
            }
        }
    }

    pub fn accept_column_editor(&self) {
        let Ok(insert) = self.column_insert() else { return };
        if matches!(&insert, ColumnInsert::Text { text } if text.is_empty()) {
            return;
        }
        self.close_column_editor();
        {
            let mut editor = self.editor.borrow_mut();
            let tab_width = editor.tab_width() as u32;
            editor.apply(|buffer, cursors| buffer.insert_column(insert, cursors.to_vec(), tab_width));
        }
        self.refresh();
    }

    pub fn close_column_editor(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<ColumnBridge>().set_open(false);
            ui.invoke_focus_editor();
        }
    }
}
