//! XML tools: pretty-print, linearize, check well-formedness and run XPath queries.

use quick_xml::events::Event;
use quick_xml::{Reader, Writer};

use crate::error::{CoreError, CoreResult};

/// A byte offset as 1-based (line, column).
fn location(text: &str, offset: usize) -> (u64, u64) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line as u64, column as u64)
}

fn error_at(text: &str, offset: u64, message: impl Into<String>) -> CoreError {
    let (line, column) = location(text, offset as usize);
    let mut message: String = message.into();
    if let Some(first) = message.chars().next() {
        message = first.to_uppercase().chain(message.chars().skip(1)).collect();
    }
    CoreError::Invalid { message, line, column }
}

/// Reads every event, checking the document is well-formed, and writes them with `writer`.
fn rewrite<W: std::io::Write>(text: &str, writer: &mut Writer<W>) -> CoreResult<()> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut open: Vec<String> = Vec::new();
    let mut roots = 0;
    loop {
        let position = reader.buffer_position();
        let event = reader.read_event().map_err(|e| error_at(text, reader.error_position(), e.to_string()))?;
        match &event {
            Event::Start(start) => {
                if open.is_empty() {
                    roots += 1;
                }
                open.push(String::from_utf8_lossy(start.name().as_ref()).into_owned());
            }
            Event::Empty(_) if open.is_empty() => roots += 1,
            Event::End(_) => {
                open.pop();
            }
            Event::Text(t) if open.is_empty() && !t.iter().all(u8::is_ascii_whitespace) => {
                return Err(error_at(text, position, "Text isn't allowed outside the root element"));
            }
            Event::Eof => break,
            _ => {}
        }
        if roots > 1 {
            return Err(error_at(text, position, "There can be only one root element"));
        }
        writer.write_event(event).map_err(|e| CoreError::invalid(e.to_string()))?;
    }
    if let Some(name) = open.last() {
        return Err(error_at(text, text.len() as u64, format!("<{name}> is never closed")));
    }
    if roots == 0 {
        return Err(CoreError::invalid("There's no root element"));
    }
    Ok(())
}

/// Pretty-prints XML, indented by `indent` spaces (0 means tabs).
pub fn xml_format(text: String, indent: u32) -> CoreResult<String> {
    let (unit, size) = if indent == 0 { (b'\t', 1) } else { (b' ', indent as usize) };
    let mut writer = Writer::new_with_indent(Vec::new(), unit, size);
    rewrite(&text, &mut writer)?;
    let mut out = String::from_utf8(writer.into_inner()).map_err(|e| CoreError::invalid(e.to_string()))?;
    out.push('\n');
    Ok(out)
}

/// XML on one line, without the whitespace between tags.
pub fn xml_linearize(text: String) -> CoreResult<String> {
    let mut writer = Writer::new(Vec::new());
    rewrite(&text, &mut writer)?;
    String::from_utf8(writer.into_inner()).map_err(|e| CoreError::invalid(e.to_string()))
}

/// Checks the XML is well-formed; returns nothing when it is.
pub fn xml_validate(text: String) -> Option<CoreError> {
    let mut writer = Writer::new(std::io::sink());
    rewrite(&text, &mut writer).err()
}

/// One XPath result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XPathItem {
    /// "element", "attribute", "text", "comment", "number", "string" or "boolean".
    pub kind: String,
    /// The element or attribute name, if any.
    pub name: String,
    pub value: String,
}

/// Runs an XPath 1.0 expression over the document.
pub fn xml_xpath(text: String, expression: String) -> CoreResult<Vec<XPathItem>> {
    use sxd_xpath::nodeset::Node;
    use sxd_xpath::{Context, Factory, Value};

    let package = sxd_document::parser::parse(&text).map_err(|e| CoreError::invalid(format!("The XML can't be read: {e:?}")))?;
    let document = package.as_document();
    let xpath = Factory::new()
        .build(&expression)
        .map_err(|e| CoreError::InvalidPattern { message: format!("Invalid XPath: {e}") })?
        .ok_or_else(|| CoreError::InvalidPattern { message: "Type an XPath expression.".into() })?;
    let value = xpath
        .evaluate(&Context::new(), document.root())
        .map_err(|e| CoreError::InvalidPattern { message: format!("The XPath couldn't run: {e}") })?;
    let shorten = |text: String| -> String {
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() > 200 { flat.chars().take(199).chain(std::iter::once('…')).collect() } else { flat }
    };
    let item = |kind: &str, name: String, value: String| XPathItem { kind: kind.into(), name, value: shorten(value) };
    Ok(match value {
        Value::Nodeset(nodes) => nodes
            .document_order()
            .into_iter()
            .take(5_000)
            .map(|node| match node {
                Node::Element(e) => item("element", e.name().local_part().to_string(), node.string_value()),
                Node::Attribute(a) => item("attribute", a.name().local_part().to_string(), a.value().to_string()),
                Node::Text(t) => item("text", String::new(), t.text().to_string()),
                Node::Comment(c) => item("comment", String::new(), c.text().to_string()),
                other => item("node", String::new(), other.string_value()),
            })
            .collect(),
        Value::Number(n) => vec![item("number", String::new(), n.to_string())],
        Value::String(s) => vec![item("string", String::new(), s)],
        Value::Boolean(b) => vec![item("boolean", String::new(), b.to_string())],
    })
}
