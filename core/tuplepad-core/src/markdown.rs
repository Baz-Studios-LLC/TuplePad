//! The Markdown preview: HTML with markers that tie blocks to source lines, for synced scrolling,
//! and code blocks colored by TuplePad's own highlighter.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd, html};

use crate::syntax;

/// Markdown (with GitHub-style tables, task lists, strikethrough and footnotes) as HTML. Each
/// block is preceded by `<span class="tp-line" data-line="N"></span>` (0-based source line).
#[uniffi::export]
pub fn markdown_html(text: String) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES;
    // Byte offsets of line starts, to turn an offset into a line number.
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let line_of = |offset: usize| line_starts.partition_point(|&start| start <= offset).saturating_sub(1);

    let mut events: Vec<Event> = Vec::new();
    let mut code: Option<(String, String)> = None; // (language, text)
    let mut table_depth = 0;
    for (event, range) in Parser::new_ext(&text, options).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                if table_depth == 0 {
                    events.push(marker(line_of(range.start)));
                }
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((language, String::new()));
            }
            Event::Text(t) if code.is_some() => {
                if let Some((_, body)) = code.as_mut() {
                    body.push_str(&t);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                let (language, body) = code.take().unwrap_or_default();
                let inner = syntax::highlight_html(&language, &body).unwrap_or_else(|| syntax::escape_html(&body));
                let class = if language.is_empty() { String::new() } else { format!(" class=\"language-{}\"", syntax::escape_html(&language)) };
                events.push(Event::Html(format!("<pre><code{class}>{inner}</code></pre>\n").into()));
            }
            Event::Start(tag) => {
                let is_table = matches!(tag, Tag::Table(_));
                let block = matches!(tag, Tag::Paragraph | Tag::Heading { .. } | Tag::BlockQuote(_) | Tag::List(_)
                    | Tag::Table(_) | Tag::HtmlBlock | Tag::FootnoteDefinition(_));
                if block && table_depth == 0 {
                    events.push(marker(line_of(range.start)));
                }
                if is_table {
                    table_depth += 1;
                }
                events.push(Event::Start(tag));
            }
            Event::End(TagEnd::Table) => {
                table_depth -= 1;
                events.push(Event::End(TagEnd::Table));
            }
            Event::Rule => {
                events.push(marker(line_of(range.start)));
                events.push(Event::Rule);
            }
            other => events.push(other),
        }
    }
    let mut out = String::with_capacity(text.len() * 2);
    html::push_html(&mut out, events.into_iter());
    out
}

fn marker(line: usize) -> Event<'static> {
    Event::Html(format!("<span class=\"tp-line\" data-line=\"{line}\"></span>").into())
}

/// What a block of the native Markdown preview is.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownBlockKind {
    Heading,
    Paragraph,
    /// A fenced or indented code block (`code` and `language`).
    Code,
    ListItem,
    Rule,
    /// A row of a table (`cells`); `header` for the first.
    TableRow,
    /// An image on its own (`url`, with `text` as its description).
    Image,
}

/// One block of the preview. `text` is inline Markdown (emphasis, strong, code, links and
/// strikethrough only), ready for a styled-text renderer.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct MarkdownBlock {
    pub kind: MarkdownBlockKind,
    /// The heading level (1–6), or how deeply a list item is nested (0 for the outermost).
    pub level: u32,
    /// How many block quotes it's inside.
    pub quote: u32,
    pub text: String,
    /// A list item's bullet or number ("•", "3."), or a task's box ("☐", "☑").
    pub marker: String,
    pub code: String,
    pub language: String,
    pub cells: Vec<String>,
    pub header: bool,
    pub url: String,
    /// 0-based line in the source where the block starts.
    pub line: u64,
}

/// Where inline text goes: the current table cell, or the block's text.
fn out<'a>(cell: &'a mut String, inline: &'a mut String, in_table: bool) -> &'a mut String {
    if in_table { cell } else { inline }
}

fn escape_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '#' | '~' | '!' | '|') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The document as blocks for a native preview (headings, paragraphs, lists, quotes, code,
/// tables, rules and images), each with the source line it starts on, for synced scrolling.
#[uniffi::export]
pub fn markdown_blocks(text: String) -> Vec<MarkdownBlock> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES;
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let line_of = |offset: usize| line_starts.partition_point(|&start| start <= offset).saturating_sub(1) as u64;
    let empty = |kind: MarkdownBlockKind, line: u64| MarkdownBlock {
        kind,
        level: 0,
        quote: 0,
        text: String::new(),
        marker: String::new(),
        code: String::new(),
        language: String::new(),
        cells: Vec::new(),
        header: false,
        url: String::new(),
        line,
    };

    let mut blocks: Vec<MarkdownBlock> = Vec::new();
    // The block being filled in, and the inline text going into it.
    let mut current: Option<MarkdownBlock> = None;
    let mut inline = String::new();
    let mut quote = 0u32;
    // Each open list: the next number, or None for bullets.
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut in_code = false;
    let mut table_row: Option<MarkdownBlock> = None;
    let mut in_head = false;
    let mut cell = String::new();
    let mut links: Vec<String> = Vec::new();
    let mut image: Option<(String, String)> = None;

    let flush = |blocks: &mut Vec<MarkdownBlock>, current: &mut Option<MarkdownBlock>, inline: &mut String| {
        if let Some(mut block) = current.take() {
            block.text = std::mem::take(inline).trim().to_string();
            if !block.text.is_empty() || block.kind != MarkdownBlockKind::Paragraph {
                blocks.push(block);
            }
        }
        inline.clear();
    };

    for (event, range) in Parser::new_ext(&text, options).into_offset_iter() {
        let line = line_of(range.start);
        // Inline text goes into a table cell when in one.
        let in_table = table_row.is_some();
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut blocks, &mut current, &mut inline);
                current = Some(MarkdownBlock { level: level as u32, quote, ..empty(MarkdownBlockKind::Heading, line) });
            }
            Event::Start(Tag::Paragraph) => {
                // Paragraphs in a list item belong to it.
                if current.as_ref().is_none_or(|b| b.kind != MarkdownBlockKind::ListItem) {
                    flush(&mut blocks, &mut current, &mut inline);
                    current = Some(MarkdownBlock { quote, ..empty(MarkdownBlockKind::Paragraph, line) });
                } else if !inline.trim().is_empty() {
                    inline.push_str("\n\n");
                }
            }
            Event::End(TagEnd::Heading(_)) | Event::End(TagEnd::Paragraph) => {
                if current.as_ref().is_some_and(|b| b.kind != MarkdownBlockKind::ListItem) {
                    flush(&mut blocks, &mut current, &mut inline);
                }
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut blocks, &mut current, &mut inline);
                quote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut blocks, &mut current, &mut inline);
                quote = quote.saturating_sub(1);
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut blocks, &mut current, &mut inline);
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut blocks, &mut current, &mut inline);
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut blocks, &mut current, &mut inline);
                let marker = match lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}.");
                        *number += 1;
                        marker
                    }
                    _ => "•".to_string(),
                };
                current = Some(MarkdownBlock { level: lists.len().saturating_sub(1) as u32, quote, marker, ..empty(MarkdownBlockKind::ListItem, line) });
            }
            Event::End(TagEnd::Item) => flush(&mut blocks, &mut current, &mut inline),
            Event::TaskListMarker(done) => {
                if let Some(block) = current.as_mut() {
                    block.marker = if done { "☑".into() } else { "☐".into() };
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut blocks, &mut current, &mut inline);
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                current = Some(MarkdownBlock { quote, language, ..empty(MarkdownBlockKind::Code, line) });
                in_code = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code = false;
                if let Some(mut block) = current.take() {
                    block.code = block.code.trim_end_matches('\n').to_string();
                    blocks.push(block);
                }
            }
            Event::Text(t) if in_code => {
                if let Some(block) = current.as_mut() {
                    block.code.push_str(&t);
                }
            }
            Event::Rule => {
                flush(&mut blocks, &mut current, &mut inline);
                blocks.push(MarkdownBlock { quote, ..empty(MarkdownBlockKind::Rule, line) });
            }
            Event::Start(Tag::Table(_)) => flush(&mut blocks, &mut current, &mut inline),
            Event::Start(Tag::TableHead) => {
                in_head = true;
                table_row = Some(MarkdownBlock { header: true, quote, ..empty(MarkdownBlockKind::TableRow, line) });
            }
            Event::Start(Tag::TableRow) => table_row = Some(MarkdownBlock { quote, ..empty(MarkdownBlockKind::TableRow, line) }),
            Event::End(TagEnd::TableCell) => {
                if let Some(row) = table_row.as_mut() {
                    row.cells.push(std::mem::take(&mut cell).trim().to_string());
                }
            }
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                if let Some(row) = table_row.take() {
                    blocks.push(row);
                }
                in_head = false;
            }
            Event::Start(Tag::Image { dest_url, .. }) => image = Some((dest_url.to_string(), String::new())),
            Event::End(TagEnd::Image) => {
                if let Some((url, alt)) = image.take() {
                    // An image alone in its paragraph becomes a block; inside text, its description.
                    let alone = current.as_ref().is_some_and(|b| b.kind == MarkdownBlockKind::Paragraph) && inline.trim().is_empty();
                    if alone {
                        current = None;
                        blocks.push(MarkdownBlock { quote, url, text: alt, ..empty(MarkdownBlockKind::Image, line) });
                    } else {
                        out(&mut cell, &mut inline, in_table).push_str(&format!("*{}*", escape_inline(&alt)));
                    }
                }
            }
            Event::Text(t) if image.is_some() => {
                if let Some((_, alt)) = image.as_mut() {
                    alt.push_str(&t);
                }
            }
            Event::Text(t) => out(&mut cell, &mut inline, in_table).push_str(&escape_inline(&t)),
            Event::Code(c) => {
                out(&mut cell, &mut inline, in_table).push('`');
                out(&mut cell, &mut inline, in_table).push_str(&c.replace('`', "'"));
                out(&mut cell, &mut inline, in_table).push('`');
            }
            Event::SoftBreak => out(&mut cell, &mut inline, in_table).push(' '),
            Event::HardBreak => out(&mut cell, &mut inline, in_table).push('\n'),
            Event::Start(Tag::Emphasis) | Event::End(TagEnd::Emphasis) => out(&mut cell, &mut inline, in_table).push('*'),
            Event::Start(Tag::Strong) | Event::End(TagEnd::Strong) => out(&mut cell, &mut inline, in_table).push_str("**"),
            Event::Start(Tag::Strikethrough) | Event::End(TagEnd::Strikethrough) => out(&mut cell, &mut inline, in_table).push_str("~~"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                links.push(dest_url.to_string());
                out(&mut cell, &mut inline, in_table).push('[');
            }
            Event::End(TagEnd::Link) => {
                let url = links.pop().unwrap_or_default();
                out(&mut cell, &mut inline, in_table).push_str(&format!("]({})", url.replace(['(', ')', ' '], "")));
            }
            Event::Html(h) | Event::InlineHtml(h) => out(&mut cell, &mut inline, in_table).push_str(&escape_inline(h.trim_end_matches('\n'))),
            Event::FootnoteReference(name) => out(&mut cell, &mut inline, in_table).push_str(&format!("[{}]", escape_inline(&name))),
            _ => {}
        }
        let _ = in_head;
    }
    flush(&mut blocks, &mut current, &mut inline);
    blocks
}

/// Syntax colors for a snippet of code in a named language ("rust", "js", …), for code blocks.
#[uniffi::export]
pub fn highlight_code(language: String, code: String) -> Vec<crate::HighlightSpan> {
    syntax::highlight_spans(&language, &code).unwrap_or_default()
}

#[cfg(test)]
mod block_tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(MarkdownBlockKind, String)> {
        markdown_blocks(text.into()).into_iter().map(|b| (b.kind, if b.kind == MarkdownBlockKind::Code { b.code } else { b.text })).collect()
    }

    #[test]
    fn blocks_with_inline_markdown() {
        use MarkdownBlockKind::*;
        let text = "# Title\n\nSome *em* and **strong** with `code` and [a link](https://x.y).\n\n- one\n- [x] done\n\n1. first\n2. second\n\n> quoted\n\n```rust\nfn main() {}\n```\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n![logo](img/logo.png)\n";
        let blocks = markdown_blocks(text.into());
        assert_eq!(
            kinds(text),
            vec![
                (Heading, "Title".into()),
                (Paragraph, "Some *em* and **strong** with `code` and [a link](https://x.y).".into()),
                (ListItem, "one".into()),
                (ListItem, "done".into()),
                (ListItem, "first".into()),
                (ListItem, "second".into()),
                (Paragraph, "quoted".into()),
                (Code, "fn main() {}".into()),
                (Rule, String::new()),
                (TableRow, String::new()),
                (TableRow, String::new()),
                (Image, "logo".into()),
            ]
        );
        assert_eq!((blocks[0].level, blocks[0].line), (1, 0));
        assert_eq!(blocks[3].marker, "☑");
        assert_eq!(blocks[5].marker, "2.");
        assert_eq!((blocks[6].quote, blocks[7].language.as_str()), (1, "rust"));
        assert_eq!((blocks[9].header, blocks[9].cells.clone(), blocks[10].cells.clone()), (true, vec!["a".to_string(), "b".into()], vec!["1".to_string(), "2".into()]));
        assert_eq!(blocks[11].url, "img/logo.png");
        // Markdown's special characters in plain text are escaped.
        assert_eq!(kinds("a * b")[0].1, "a \\* b");
    }

    #[test]
    fn code_is_colored() {
        let spans = highlight_code("rust".into(), "fn main() {}".into());
        assert!(spans.iter().any(|s| s.style == crate::HighlightStyle::Keyword));
        assert!(highlight_code("no-such-language".into(), "x".into()).is_empty());
    }
}
