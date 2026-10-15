//! The Markdown preview, drawn by Slint itself: the core splits the document into blocks
//! (headings, paragraphs, lists, quotes, code, tables, rules, images), each with its source
//! line; inline text is shown as styled text and code with TuplePad's syntax colors. The preview
//! scrolls with the editor: blocks report where they were laid out, and the editor's top line is
//! matched to them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, StyledText, VecModel};
use tuplepad_core::{MarkdownBlock, MarkdownBlockKind};

use crate::app::App;
use crate::{CodeLine, CodePart, PreviewBlock, PreviewBridge};

#[derive(Default)]
pub struct PreviewState {
    /// The document and version the blocks are for.
    key: Option<(String, u64)>,
    /// Each block's source line and laid-out position.
    lines: Vec<u64>,
    positions: Vec<f32>,
    /// The editor's top line when the preview last followed it.
    synced: Option<(String, u64)>,
    images: HashMap<PathBuf, Option<slint::Image>>,
}

fn styled(text: &str) -> StyledText {
    StyledText::from_markdown(text).unwrap_or_else(|_| StyledText::from_plain_text(text))
}

/// A code block's lines in syntax colors.
fn code_lines(language: &str, code: &str) -> Vec<CodeLine> {
    let spans = if language.is_empty() { Vec::new() } else { tuplepad_core::highlight_code(language.to_string(), code.to_string()) };
    code.split('\n')
        .enumerate()
        .map(|(index, line)| {
            let units: Vec<u16> = line.encode_utf16().collect();
            let text = |from: usize, to: usize| String::from_utf16_lossy(&units[from.min(units.len())..to.min(units.len())]).replace('\t', "    ");
            let mut parts = Vec::new();
            let mut column = 0;
            for span in spans.iter().filter(|s| s.line == index as u64) {
                let (start, end) = (span.start as usize, (span.end as usize).min(units.len()));
                if start < column || start >= end {
                    continue;
                }
                if start > column {
                    parts.push(CodePart { text: text(column, start).into(), style: 0 });
                }
                parts.push(CodePart { text: text(start, end).into(), style: crate::editor::style_index(span.style) });
                column = end;
            }
            if column < units.len() || parts.is_empty() {
                parts.push(CodePart { text: text(column, units.len()).into(), style: 0 });
            }
            CodeLine { parts: ModelRc::new(VecModel::from(parts)) }
        })
        .collect()
}

impl App {
    fn preview_bridge<T>(&self, f: impl FnOnce(&PreviewBridge) -> T) -> Option<T> {
        self.ui.upgrade().map(|ui| f(&ui.global::<PreviewBridge>()))
    }

    pub fn toggle_preview(&self) {
        let open = !self.preview_bridge(|b| b.get_open()).unwrap_or(false);
        self.preview_bridge(|b| b.set_open(open));
        *self.preview.borrow_mut() = PreviewState::default();
        self.refresh_preview();
    }

    pub fn preview_open(&self) -> bool {
        self.preview_bridge(|b| b.get_open()).unwrap_or(false)
    }

    /// Brings the preview up to date with the document (from the tick), and keeps it scrolled
    /// with the editor.
    pub fn refresh_preview(&self) {
        if !self.preview_open() {
            return;
        }
        let Ok(editor) = self.editor.try_borrow() else { return };
        let doc = editor.doc();
        let key = (doc.id.clone(), doc.buffer.version());
        let markdown = doc.buffer.language().as_deref() == Some("markdown");
        let folder = doc.path().and_then(|p| p.parent().map(Path::to_path_buf));
        let top = editor.top_line();
        let buffer = doc.buffer.clone();
        drop(editor);
        if self.preview.borrow().key.as_ref() != Some(&key) {
            if markdown {
                let blocks = tuplepad_core::markdown_blocks(buffer.text());
                self.show_blocks(&blocks, folder.as_deref());
            } else {
                self.preview_bridge(|b| {
                    b.set_blocks(ModelRc::default());
                    b.set_empty_text("The preview shows Markdown documents.".into());
                });
                let mut state = self.preview.borrow_mut();
                state.lines.clear();
                state.positions.clear();
            }
            let mut state = self.preview.borrow_mut();
            state.key = Some(key.clone());
            state.synced = None;
        }
        self.sync_preview_scroll(&key.0, top);
    }

    fn show_blocks(&self, blocks: &[MarkdownBlock], folder: Option<&Path>) {
        let rows: Vec<PreviewBlock> = blocks
            .iter()
            .enumerate()
            .map(|(index, block)| {
                let next = blocks.get(index + 1);
                let mut row = PreviewBlock {
                    kind: match block.kind {
                        MarkdownBlockKind::Heading => 0,
                        MarkdownBlockKind::Paragraph => 1,
                        MarkdownBlockKind::Code => 2,
                        MarkdownBlockKind::ListItem => 3,
                        MarkdownBlockKind::Rule => 4,
                        MarkdownBlockKind::TableRow => 5,
                        MarkdownBlockKind::Image => 6,
                    },
                    level: block.level as i32,
                    quote: block.quote as i32,
                    marker: block.marker.clone().into(),
                    alt: block.text.clone().into(),
                    header: block.header,
                    last: next.is_none_or(|n| n.kind != block.kind || n.quote != block.quote),
                    ..Default::default()
                };
                match block.kind {
                    // Headings are bold through their markup.
                    MarkdownBlockKind::Heading => row.text = styled(&format!("**{}**", block.text)),
                    MarkdownBlockKind::Code => row.code = ModelRc::new(VecModel::from(code_lines(&block.language, &block.code))),
                    MarkdownBlockKind::TableRow => {
                        let cells: Vec<StyledText> = block.cells.iter().map(|c| styled(&if block.header && !c.is_empty() { format!("**{c}**") } else { c.clone() })).collect();
                        row.cells = ModelRc::new(VecModel::from(cells));
                    }
                    MarkdownBlockKind::Image => {
                        if let Some(image) = self.load_image(&block.url, folder) {
                            row.image = image;
                            row.has_image = true;
                        }
                    }
                    _ => row.text = styled(&block.text),
                }
                row
            })
            .collect();
        {
            let mut state = self.preview.borrow_mut();
            state.lines = blocks.iter().map(|b| b.line).collect();
            state.positions = vec![-1.0; blocks.len()];
        }
        self.preview_bridge(|b| {
            b.set_blocks(ModelRc::from(Rc::new(VecModel::from(rows))));
            b.set_empty_text(if blocks.is_empty() { "Nothing to preview yet.".into() } else { Default::default() });
        });
    }

    /// A local image (relative to the document's folder), loaded once.
    fn load_image(&self, url: &str, folder: Option<&Path>) -> Option<slint::Image> {
        if url.contains("://") {
            return None;
        }
        let path = folder.map_or_else(|| PathBuf::from(url), |f| f.join(url));
        let mut state = self.preview.borrow_mut();
        state.images.entry(path.clone()).or_insert_with(|| slint::Image::load_from_path(&path).ok()).clone()
    }

    /// Where a block ended up in the preview's layout.
    pub fn preview_block_moved(&self, index: usize, y: f32) {
        let mut state = self.preview.borrow_mut();
        if let Some(position) = state.positions.get_mut(index) {
            *position = y;
        }
        state.synced = None;
    }

    /// Scrolls the preview to the block at the editor's top line (between blocks, in proportion).
    fn sync_preview_scroll(&self, doc: &str, top: u64) {
        let target = {
            let mut state = self.preview.borrow_mut();
            if state.synced.as_ref() == Some(&(doc.to_string(), top)) || state.lines.is_empty() || state.positions.iter().any(|&p| p < 0.0) {
                return;
            }
            state.synced = Some((doc.to_string(), top));
            // Measured from the first block, so the preview keeps its top margin.
            let base = state.positions.first().copied().unwrap_or(0.0);
            let after = state.lines.partition_point(|&line| line <= top);
            let y = if after == 0 {
                base
            } else {
                let i = after - 1;
                let (line, y) = (state.lines[i], state.positions[i]);
                match (state.lines.get(i + 1), state.positions.get(i + 1)) {
                    (Some(&next_line), Some(&next_y)) if next_line > line => y + (next_y - y) * ((top - line) as f32 / (next_line - line) as f32),
                    _ => y,
                }
            };
            (y - base).max(0.0)
        };
        self.preview_bridge(|b| {
            b.set_scroll_y(target);
            b.set_scroll_request(b.get_scroll_request() + 1);
        });
    }

    /// A link in the preview: web links open in the browser.
    pub fn preview_link(&self, link: &str) {
        if link.starts_with("http://") || link.starts_with("https://") || link.starts_with("mailto:") {
            crate::platform::open_url(link);
        }
    }

    /// For scripted runs.
    pub fn preview_dump(&self) -> serde_json::Value {
        let state = self.preview.borrow();
        let open = self.preview_open();
        let (count, empty, scroll) = self.preview_bridge(|b| {
            use slint::Model;
            (b.get_blocks().row_count(), b.get_empty_text().to_string(), b.get_scroll_y())
        }).unwrap_or_default();
        serde_json::json!({"open": open, "blocks": count, "empty": empty, "lines": state.lines, "positions": state.positions, "synced": state.synced.as_ref().map(|s| s.1), "scroll_y": scroll, "laid_out": state.positions.iter().all(|&p| p >= 0.0)})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_keeps_its_text() {
        let lines = code_lines("rust", "fn main() {\n\tlet x = 1;\n}");
        use slint::Model;
        let text: Vec<String> = lines.iter().map(|l| l.parts.iter().map(|p| p.text.to_string()).collect()).collect();
        assert_eq!(text, ["fn main() {", "    let x = 1;", "}"]);
        assert!(lines[0].parts.iter().any(|p| p.style == 1));
    }
}
