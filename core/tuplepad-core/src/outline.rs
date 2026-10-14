//! The outline: a document's functions, types and headings, from its syntax tree.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use ropey::Rope;
use tree_sitter::{Query, QueryCursor, StreamingIterator, Tree};

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    /// A function inside a type.
    Method,
    Type,
    /// Modules, namespaces, impl blocks, extensions and sections of config files.
    Namespace,
    Heading,
    Constant,
    Property,
}

/// One entry in the outline.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    pub name: String,
    pub kind: SymbolKind,
    /// Where the name is (0-based line, UTF-16 column).
    pub line: u64,
    pub column: u64,
    /// The last line of the whole item, for finding the item around the caret.
    pub end_line: u64,
    /// How deeply it's nested inside other items.
    pub depth: u32,
}

/// Symbol queries: `@name` is the name; the item itself is captured as its kind.
fn source(id: &str) -> Option<&'static str> {
    Some(match id {
        "rust" => r#"
            (function_item name: (_) @name) @function
            (function_signature_item name: (_) @name) @function
            (struct_item name: (_) @name) @type
            (enum_item name: (_) @name) @type
            (union_item name: (_) @name) @type
            (trait_item name: (_) @name) @type
            (type_item name: (_) @name) @type
            (impl_item) @namespace
            (mod_item name: (_) @name) @namespace
            (macro_definition name: (_) @name) @function
            (const_item name: (_) @name) @constant
            (static_item name: (_) @name) @constant"#,
        "swift" => r#"
            (class_declaration declaration_kind: "extension" name: (_) @name) @namespace
            (class_declaration declaration_kind: ["class" "struct" "enum" "actor"] name: (_) @name) @type
            (protocol_declaration name: (_) @name) @type
            (function_declaration name: (_) @name) @function
            (protocol_function_declaration name: (_) @name) @function
            (init_declaration name: _ @name) @function
            (typealias_declaration name: (_) @name) @type"#,
        "python" => r#"
            (function_definition name: (_) @name) @function
            (class_definition name: (_) @name) @type"#,
        "javascript" | "typescript" | "tsx" => r#"
            (function_declaration name: (_) @name) @function
            (generator_function_declaration name: (_) @name) @function
            (class_declaration name: (_) @name) @type
            (method_definition name: (_) @name) @function
            (lexical_declaration (variable_declarator name: (identifier) @name value: [(arrow_function) (function_expression)])) @function"#,
        "go" => r#"
            (function_declaration name: (_) @name) @function
            (method_declaration name: (_) @name) @function
            (type_spec name: (_) @name) @type"#,
        "c" => r#"
            (function_definition declarator: (_ declarator: (_) @name)) @function
            (struct_specifier name: (_) @name body: (_)) @type
            (enum_specifier name: (_) @name body: (_)) @type
            (union_specifier name: (_) @name body: (_)) @type"#,
        "cpp" => r#"
            (function_definition declarator: (_ declarator: (_) @name)) @function
            (struct_specifier name: (_) @name body: (_)) @type
            (class_specifier name: (_) @name body: (_)) @type
            (enum_specifier name: (_) @name body: (_)) @type
            (namespace_definition name: (_) @name) @namespace"#,
        "java" => r#"
            (class_declaration name: (_) @name) @type
            (interface_declaration name: (_) @name) @type
            (enum_declaration name: (_) @name) @type
            (record_declaration name: (_) @name) @type
            (method_declaration name: (_) @name) @function
            (constructor_declaration name: (_) @name) @function"#,
        "csharp" => r#"
            (namespace_declaration name: (_) @name) @namespace
            (class_declaration name: (_) @name) @type
            (struct_declaration name: (_) @name) @type
            (interface_declaration name: (_) @name) @type
            (enum_declaration name: (_) @name) @type
            (record_declaration name: (_) @name) @type
            (method_declaration name: (_) @name) @function
            (constructor_declaration name: (_) @name) @function"#,
        "ruby" => r#"
            (method name: (_) @name) @function
            (singleton_method name: (_) @name) @function
            (class name: (_) @name) @type
            (module name: (_) @name) @namespace"#,
        "php" => r#"
            (function_definition name: (_) @name) @function
            (class_declaration name: (_) @name) @type
            (interface_declaration name: (_) @name) @type
            (trait_declaration name: (_) @name) @type
            (method_declaration name: (_) @name) @function"#,
        "lua" => "(function_declaration name: (_) @name) @function",
        "shell" => "(function_definition name: (_) @name) @function",
        "make" => "(rule (targets) @name) @function",
        "css" => r#"
            (rule_set (selectors) @name) @type
            (media_statement) @namespace
            (keyframes_statement (keyframes_name) @name) @namespace"#,
        "markdown" => r#"
            (section (atx_heading (inline) @name)) @heading
            (section (setext_heading (paragraph (inline) @name))) @heading"#,
        "toml" => r#"
            (table . (_) @name) @namespace
            (table_array_element . (_) @name) @namespace"#,
        "yaml" => "(stream (document (block_node (block_mapping (block_mapping_pair key: (_) @name) @property))))",
        "json" => "(document (object (pair key: (string (string_content) @name)) @property))",
        _ => return None,
    })
}

struct Compiled {
    query: Query,
    name_capture: Option<u32>,
    kinds: Vec<Option<SymbolKind>>,
}

fn compiled(id: &'static str, language: &tree_sitter::Language) -> Option<Arc<Compiled>> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<Arc<Compiled>>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(id)
        .or_insert_with(|| {
            let query = Query::new(language, source(id)?).ok()?;
            let kinds = query
                .capture_names()
                .iter()
                .map(|name| match *name {
                    "function" => Some(SymbolKind::Function),
                    "type" => Some(SymbolKind::Type),
                    "namespace" => Some(SymbolKind::Namespace),
                    "heading" => Some(SymbolKind::Heading),
                    "constant" => Some(SymbolKind::Constant),
                    "property" => Some(SymbolKind::Property),
                    _ => None,
                })
                .collect();
            let name_capture = query.capture_index_for_name("name");
            Some(Arc::new(Compiled { query, name_capture, kinds }))
        })
        .clone()
}

/// The outline of a parsed document, in document order, at most `limit` items.
pub(crate) fn outline(id: &'static str, language: &tree_sitter::Language, tree: &Tree, rope: &Rope, limit: usize) -> Vec<OutlineItem> {
    let Some(compiled) = compiled(id, language) else { return Vec::new() };
    let text = |node: tree_sitter::Node| rope.byte_slice(node.byte_range()).chunks().map(str::as_bytes);
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&compiled.query, tree.root_node(), text);
    // (item start byte, item end byte, item)
    let mut found: Vec<(usize, usize, OutlineItem)> = Vec::new();
    while let Some(m) = matches.next() {
        let mut item_node = None;
        let mut name_node = None;
        let mut kind = None;
        for capture in m.captures() {
            if Some(capture.index) == compiled.name_capture {
                name_node = Some(capture.node);
            } else if let Some(Some(found_kind)) = compiled.kinds.get(capture.index as usize) {
                item_node = Some(capture.node);
                kind = Some(*found_kind);
            }
        }
        let (Some(item), Some(kind)) = (item_node, kind) else { continue };
        if found.iter().any(|(start, end, _)| *start == item.start_byte() && *end == item.end_byte()) {
            continue; // matched by two patterns
        }
        // Without a name capture (impl blocks, @media), the item's first line names it.
        let name_node = name_node.unwrap_or(item);
        let mut name = rope.byte_slice(name_node.byte_range()).to_string();
        if name_node == item {
            name = name.lines().next().unwrap_or("").trim().trim_end_matches('{').trim().to_string();
        }
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() {
            continue;
        }
        let start = name_node.start_position();
        let line_start = rope.line_to_byte(start.row);
        let column = rope.byte_slice(line_start..line_start + start.column).len_utf16_cu() as u64;
        found.push((item.start_byte(), item.end_byte(), OutlineItem {
            name: truncate(&name, 120),
            kind,
            line: start.row as u64,
            column,
            end_line: item.end_position().row as u64,
            depth: 0,
        }));
        if found.len() >= limit {
            break;
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    // Nesting: count the items each one sits inside. Functions inside types are methods.
    let mut open: Vec<(usize, SymbolKind)> = Vec::new();
    let mut items = Vec::with_capacity(found.len());
    for (start, end, mut item) in found {
        while open.last().is_some_and(|(open_end, _)| *open_end <= start) {
            open.pop();
        }
        item.depth = open.len() as u32;
        if item.kind == SymbolKind::Function && open.last().is_some_and(|(_, k)| matches!(k, SymbolKind::Type | SymbolKind::Namespace)) {
            item.kind = SymbolKind::Method;
        }
        open.push((end, item.kind));
        items.push(item);
    }
    items
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max - 1).chain(std::iter::once('…')).collect()
}

#[cfg(test)]
pub(crate) fn source_for_test(id: &str) -> Option<&'static str> {
    source(id)
}
