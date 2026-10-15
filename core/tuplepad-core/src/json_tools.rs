//! JSON tools: format, minify, validate, sort keys, a tree of the document, and the path to a key.

use serde::Serialize;
use serde_json::Value;
use tree_sitter::{Node, Parser};

use crate::error::{CoreError, CoreResult};

fn parse(text: &str) -> CoreResult<Value> {
    serde_json::from_str(text).map_err(|e| CoreError::Invalid {
        message: describe(&e),
        line: e.line() as u64,
        column: e.column() as u64,
    })
}

/// serde_json's message without its " at line 3 column 7" suffix, capitalized.
fn describe(error: &serde_json::Error) -> String {
    let text = error.to_string();
    let text = text.split(" at line ").next().unwrap_or(&text);
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Invalid JSON".into(),
    }
}

fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// Pretty-prints JSON, indented by `indent` spaces (0 means tabs), keeping key order and
/// number precision unless `sort_keys`.
pub fn json_format(text: String, indent: u32, sort_keys: bool) -> CoreResult<String> {
    let mut value = parse(&text)?;
    if sort_keys {
        value = sorted(value);
    }
    let unit = if indent == 0 { "\t".to_string() } else { " ".repeat(indent as usize) };
    let formatter = serde_json::ser::PrettyFormatter::with_indent(unit.as_bytes());
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    value.serialize(&mut serializer).map_err(|e| CoreError::invalid(e.to_string()))?;
    let mut text = String::from_utf8(out).map_err(|e| CoreError::invalid(e.to_string()))?;
    text.push('\n');
    Ok(text)
}

/// JSON on one line, with no extra spaces.
pub fn json_minify(text: String) -> CoreResult<String> {
    serde_json::to_string(&parse(&text)?).map_err(|e| CoreError::invalid(e.to_string()))
}

/// Checks the JSON; returns nothing when it's valid.
pub fn json_validate(text: String) -> Option<CoreError> {
    parse(&text).err()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonKind {
    Object,
    Array,
    String,
    Number,
    Bool,
    Null,
}

/// One node of the JSON tree view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonTreeItem {
    pub id: u32,
    pub parent: Option<u32>,
    /// The key, or "[3]" for an array element; empty for the root.
    pub key: String,
    pub kind: JsonKind,
    /// The value for scalars, or a count like "3 items" for objects and arrays.
    pub preview: String,
    /// 0-based line where the value starts.
    pub line: u64,
    pub column: u64,
}

fn json_tree_sitter(text: &str) -> Option<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_json::LANGUAGE.into()).ok()?;
    parser.parse(text, None)
}

/// The document as a tree (at most `limit` nodes), from its syntax tree, so it works on JSON
/// with mistakes too.
pub fn json_tree(text: String, limit: u32) -> Vec<JsonTreeItem> {
    let Some(tree) = json_tree_sitter(&text) else { return Vec::new() };
    let mut items = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    let values: Vec<Node> = root.named_children(&mut cursor).filter(|n| n.kind() != "comment").collect();
    for value in values {
        add_value(&text, value, String::new(), None, &mut items, limit as usize);
    }
    items
}

fn add_value(text: &str, node: Node, key: String, parent: Option<u32>, items: &mut Vec<JsonTreeItem>, limit: usize) {
    if items.len() >= limit {
        return;
    }
    let source = &text[node.byte_range()];
    let kind = match node.kind() {
        "object" => JsonKind::Object,
        "array" => JsonKind::Array,
        "string" => JsonKind::String,
        "number" => JsonKind::Number,
        "true" | "false" => JsonKind::Bool,
        "null" => JsonKind::Null,
        _ => return,
    };
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).filter(|n| n.kind() != "comment").collect();
    let preview = match kind {
        JsonKind::Object => count(children.len(), "key", "keys"),
        JsonKind::Array => count(children.len(), "item", "items"),
        _ => {
            let flat: String = source.split_whitespace().collect::<Vec<_>>().join(" ");
            if flat.chars().count() > 120 { flat.chars().take(119).chain(std::iter::once('…')).collect() } else { flat }
        }
    };
    let id = items.len() as u32;
    let position = node.start_position();
    let line_start = text[..node.start_byte()].rfind('\n').map_or(0, |i| i + 1);
    items.push(JsonTreeItem {
        id,
        parent,
        key,
        kind,
        preview,
        line: position.row as u64,
        column: text[line_start..node.start_byte()].encode_utf16().count() as u64,
    });
    match kind {
        JsonKind::Object => {
            for pair in children.into_iter().filter(|n| n.kind() == "pair") {
                let (Some(key), Some(value)) = (pair.child_by_field_name("key"), pair.child_by_field_name("value")) else { continue };
                add_value(text, value, unquote(&text[key.byte_range()]), Some(id), items, limit);
            }
        }
        JsonKind::Array => {
            for (index, child) in children.into_iter().enumerate() {
                add_value(text, child, format!("[{index}]"), Some(id), items, limit);
            }
        }
        _ => {}
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

fn unquote(key: &str) -> String {
    serde_json::from_str::<String>(key).unwrap_or_else(|_| key.trim_matches('"').to_string())
}

/// The path to the value at a byte offset, like `$.store.books[2].title`.
pub fn json_path_at(text: String, line: u64, utf16_column: u64) -> Option<String> {
    let tree = json_tree_sitter(&text)?;
    // Line and UTF-16 column to a byte offset.
    let line_start = text.split_inclusive('\n').take(line as usize).map(str::len).sum::<usize>();
    let line_text = text[line_start..].split('\n').next().unwrap_or("");
    let mut units = 0;
    let mut byte = line_start + line_text.len();
    for (offset, c) in line_text.char_indices() {
        if units >= utf16_column as usize {
            byte = line_start + offset;
            break;
        }
        units += c.len_utf16();
    }
    let mut node = tree.root_node().descendant_for_byte_range(byte, byte)?;
    let mut parts: Vec<String> = Vec::new();
    // On the whitespace inside a pair (between the colon and the value), the pair itself is the
    // smallest node: name its key too.
    if node.kind() == "pair" {
        if let Some(key) = node.child_by_field_name("key") {
            let name = unquote(&text[key.byte_range()]);
            let simple = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit());
            parts.push(if simple { format!(".{name}") } else { format!("[{}]", serde_json::to_string(&name).ok()?) });
        }
    }
    loop {
        let Some(parent) = node.parent() else { break };
        match parent.kind() {
            "pair" => {
                if let Some(key) = parent.child_by_field_name("key") {
                    let name = unquote(&text[key.byte_range()]);
                    let simple = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                        && !name.starts_with(|c: char| c.is_ascii_digit());
                    parts.push(if simple { format!(".{name}") } else { format!("[{}]", serde_json::to_string(&name).ok()?) });
                }
            }
            "array" => {
                let mut cursor = parent.walk();
                let index = parent
                    .named_children(&mut cursor)
                    .filter(|n| n.kind() != "comment")
                    .position(|child| child.byte_range().contains(&node.start_byte()) || child.id() == node.id());
                if let Some(index) = index {
                    parts.push(format!("[{index}]"));
                }
            }
            _ => {}
        }
        node = parent;
    }
    parts.reverse();
    Some(format!("${}", parts.concat()))
}
