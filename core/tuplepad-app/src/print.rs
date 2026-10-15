//! Printing: the document as HTML, with its syntax colors and (if wanted) line numbers, handed to
//! the system's print dialog. On a Mac that's AppKit's print panel; elsewhere the page opens in the
//! browser, which prints it or saves a PDF.

use std::fmt::Write as _;

use tuplepad_core::{HighlightSpan, TextBuffer};

use crate::app::App;

/// Printing stops after this many lines (that's already thousands of pages).
const MAX_PRINTED_LINES: u64 = 50_000;

/// Light-theme syntax colors (as in `Theme.syntax`), indexed by highlight style.
const COLORS: [&str; 24] = [
    "#1B2A4A", "#6B3FA0", "#9A5B13", "#B4441B", "#9A917F", "#B4441B", "#B4441B", "#1F4FA8", "#0F6E7A", "#1B2A4A", "#3D5878",
    "#2C6E99", "#6D6457", "#B02A44", "#8C5A00", "#6B3FA0", "#3D5878", "#103474", "#1B2A4A", "#1B2A4A", "#2C6E99", "#9A5B13",
    "#3D7A3A", "#B02A44",
];

fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

/// The document as a printable HTML page.
pub fn document_html(buffer: &TextBuffer, title: &str, line_numbers: bool, colors: bool, tab_width: usize) -> String {
    let count = buffer.line_count().min(MAX_PRINTED_LINES);
    let digits = count.to_string().len();
    let mut html = String::with_capacity(buffer.len_utf16() as usize * 2);
    html.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>");
    escape(title, &mut html);
    html.push_str(
        "</title><style>\
         @page { margin: 16mm 14mm; }\
         body { margin: 0; color: #1B2A4A; background: white; }\
         .code { font-family: \"SF Mono\", Menlo, Consolas, \"DejaVu Sans Mono\", monospace; font-size: 9.5pt; line-height: 1.45; }\
         .code div { white-space: pre-wrap; margin: 0; }\
         .n { color: #B3A891; }\
         </style></head><body><div class=\"code\">",
    );
    // Wrapped lines hang past the line number (a monospace character is about 0.6 em).
    let hang = if line_numbers { (digits + 2) as f32 * 0.6 } else { 0.0 };
    let line_style = format!("padding-left: {hang:.2}em; text-indent: -{hang:.2}em;");
    let lines = buffer.lines(0, count);
    let spans = if colors { buffer.highlights(0, count) } else { Vec::new() };
    let mut spans = spans.into_iter().peekable();
    for (index, line) in lines.iter().enumerate() {
        let _ = write!(html, "<div style=\"{line_style}\">");
        if line_numbers {
            let _ = write!(html, "<span class=\"n\">{:>digits$}  </span>", index + 1);
        }
        let line_spans: Vec<HighlightSpan> = std::iter::from_fn(|| spans.next_if(|s| s.line == index as u64)).collect();
        write_line(&mut html, line, &line_spans, tab_width);
        if line.is_empty() {
            html.push_str("&nbsp;");
        }
        html.push_str("</div>");
    }
    if buffer.line_count() > count {
        let _ = write!(html, "<div class=\"n\">… {} more lines not printed</div>", buffer.line_count() - count);
    }
    html.push_str("</div></body></html>");
    html
}

/// Part of a document (from `start` to `end`) as colored HTML, for copying with syntax colors.
pub fn range_html(buffer: &TextBuffer, start: tuplepad_core::TextPos, end: tuplepad_core::TextPos, tab_width: usize) -> String {
    let mut html = String::from(
        "<meta charset=\"utf-8\"><pre style=\"font-family: 'SF Mono', Menlo, Consolas, 'DejaVu Sans Mono', monospace; font-size: 12px; color: #1B2A4A;\">",
    );
    let last = end.line.min(buffer.line_count().saturating_sub(1));
    for index in start.line..=last {
        let line = buffer.line(index);
        let length = line.encode_utf16().count() as u64;
        let from = if index == start.line { start.column.min(length) } else { 0 };
        let to = if index == end.line { end.column.min(length) } else { length };
        let units: Vec<u16> = line.encode_utf16().collect();
        let part = String::from_utf16_lossy(&units[from as usize..to.max(from) as usize]);
        let spans: Vec<HighlightSpan> = buffer
            .highlights(index, 1)
            .into_iter()
            .filter(|s| s.end > from && s.start < to)
            .map(|s| HighlightSpan { start: s.start.max(from) - from, end: s.end.min(to) - from, ..s })
            .collect();
        write_line(&mut html, &part, &spans, tab_width);
        if index < last {
            html.push('\n');
        }
    }
    html.push_str("</pre>");
    html
}

/// One line with its colored runs. Columns are UTF-16 units of the original line (before tabs
/// are expanded).
fn write_line(html: &mut String, line: &str, spans: &[HighlightSpan], tab_width: usize) {
    let units: Vec<u16> = line.encode_utf16().collect();
    let text = |from: usize, to: usize| String::from_utf16_lossy(&units[from.min(units.len())..to.min(units.len())]).replace('\t', &" ".repeat(tab_width.max(1)));
    let mut column = 0usize;
    for span in spans {
        let (start, end) = (span.start as usize, (span.end as usize).min(units.len()));
        if start < column || start >= end {
            continue;
        }
        escape(&text(column, start), html);
        let color = COLORS[style_index(span.style)];
        let weight = if matches!(span.style, tuplepad_core::HighlightStyle::Strong | tuplepad_core::HighlightStyle::Heading) { ";font-weight:bold" } else { "" };
        let italic = if span.style == tuplepad_core::HighlightStyle::Emphasis { ";font-style:italic" } else { "" };
        let _ = write!(html, "<span style=\"color:{color}{weight}{italic}\">");
        escape(&text(start, end), html);
        html.push_str("</span>");
        column = end;
    }
    escape(&text(column, units.len()), html);
}

fn style_index(style: tuplepad_core::HighlightStyle) -> usize {
    use tuplepad_core::HighlightStyle::*;
    match style {
        Keyword => 1,
        String => 2,
        Escape => 3,
        Comment => 4,
        Number => 5,
        Constant => 6,
        Function => 7,
        Type => 8,
        Variable => 9,
        Property => 10,
        Operator => 11,
        Punctuation => 12,
        Tag => 13,
        Attribute => 14,
        Label => 15,
        Namespace => 16,
        Heading => 17,
        Emphasis => 18,
        Strong => 19,
        Link => 20,
        Code => 21,
        Inserted => 22,
        Deleted => 23,
    }
}

/// Prints the current document: builds the page and hands it to the system.
pub fn print(app: &App) {
    let (buffer, name) = {
        let editor = app.editor.borrow();
        (editor.buffer(), editor.doc().name())
    };
    let settings = app.settings.borrow().clone();
    let html = document_html(&buffer, &name, settings.print_line_numbers, settings.print_colors, settings.tab_width as usize);
    let folder = std::env::temp_dir().join("TuplePad Print");
    let _ = std::fs::create_dir_all(&folder);
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    let path = folder.join(format!("{safe}.html"));
    if let Err(error) = std::fs::write(&path, &html) {
        app.alert("TuplePad couldn't prepare the document for printing.", &error.to_string());
        return;
    }
    *app.printed.borrow_mut() = Some(path.clone());
    if app.is_scripted() {
        // Scripted runs never print; they can check the page AppKit would print as a PDF.
        #[cfg(target_os = "macos")]
        if let Some(pdf) = std::env::var_os("TUPLEPAD_PRINT_PDF") {
            mac::pdf(&html, std::path::Path::new(&pdf));
        }
        return;
    }
    #[cfg(target_os = "macos")]
    if mac::print(&html, &name) {
        return;
    }
    print_page(&path);
}

/// AppKit's print panel (with its preview and Save as PDF), printing the page as styled text.
#[cfg(target_os = "macos")]
mod mac {
    use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSAttributedStringAppKitDocumentFormats, NSPrintInfo, NSPrintOperation, NSPrintingPaginationMode, NSTextView};
    use objc2_foundation::{NSAttributedString, NSData, NSPoint, NSRect, NSSize, NSString};

    /// The page as styled text in a text view as wide as the paper allows.
    fn page_view(html: &str) -> Option<(objc2::rc::Retained<NSTextView>, objc2::rc::Retained<NSPrintInfo>)> {
        let mtm = MainThreadMarker::new()?;
        let data = NSData::with_bytes(html.as_bytes());
        let text = unsafe { NSAttributedString::initWithHTML_documentAttributes(NSAttributedString::alloc(), &data, None) }?;
        let info = NSPrintInfo::sharedPrintInfo();
        info.setHorizontalPagination(NSPrintingPaginationMode::Fit);
        info.setVerticallyCentered(false);
        let paper = info.paperSize();
        let width = paper.width - info.leftMargin() - info.rightMargin();
        let view = NSTextView::initWithFrame(NSTextView::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, 100.0)));
        if let Some(storage) = unsafe { view.textStorage() } {
            storage.setAttributedString(&text);
        }
        view.setVerticallyResizable(true);
        view.sizeToFit();
        Some((view, info))
    }

    /// Writes the page as a PDF (for checking what printing produces).
    pub fn pdf(html: &str, path: &std::path::Path) {
        use objc2_foundation::NSMutableData;
        let Some((view, info)) = page_view(html) else { return };
        let data = NSMutableData::new();
        let operation = NSPrintOperation::PDFOperationWithView_insideRect_toData_printInfo(&view, view.bounds(), &data, &info);
        operation.runOperation();
        let _ = std::fs::write(path, data.to_vec());
    }

    /// Returns false if AppKit couldn't read the page (then it opens in the browser instead).
    pub fn print(html: &str, title: &str) -> bool {
        let Some((view, info)) = page_view(html) else { return false };
        let operation = NSPrintOperation::printOperationWithView_printInfo(&view, &info);
        operation.setJobTitle(Some(&NSString::from_str(title)));
        operation.setShowsPrintPanel(true);
        operation.runOperation();
        true
    }
}

/// Opens the page in the browser with its print dialog.
fn print_page(path: &std::path::Path) {
    let page = path.with_extension("print.html");
    let wrapped = std::fs::read_to_string(path)
        .unwrap_or_default()
        .replacen("</body>", "<script>window.addEventListener('load', () => window.print());</script></body>", 1);
    let _ = std::fs::write(&page, wrapped);
    open_with_system(&page);
}

fn open_with_system(path: &std::path::Path) {
    use std::process::Command;
    let result = if cfg!(target_os = "macos") {
        Command::new("open").arg(path).spawn()
    } else if cfg!(windows) {
        Command::new("cmd").args(["/C", "start", ""]).arg(path).spawn()
    } else {
        Command::new("xdg-open").arg(path).spawn()
    };
    if let Err(error) = result {
        eprintln!("couldn't open {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_have_colors_numbers_and_escaped_text() {
        let buffer = TextBuffer::with_text("fn main() {\n\tlet s = \"<b>\";\n}\n".to_string());
        buffer.set_language(Some("rust".into()));
        let html = document_html(&buffer, "main.rs", true, true, 4);
        assert!(html.contains("<span class=\"n\">1  </span>"), "{html}");
        assert!(html.contains("color:#6B3FA0\">fn</span>"), "{html}");
        assert!(html.contains("&quot;&lt;b&gt;&quot;"), "{html}");
        assert!(html.contains("    <span style=\"color:#6B3FA0\">let</span>"), "{html}");
        let plain = document_html(&buffer, "main.rs", false, false, 4);
        assert!(!plain.contains("class=\"n\"") && !plain.contains("color:#6B3FA0"));
    }
}
