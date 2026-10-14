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
