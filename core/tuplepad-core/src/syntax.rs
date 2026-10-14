//! Syntax highlighting with tree-sitter. Each document keeps a syntax tree that is
//! updated incrementally as it's edited; highlights are computed only for the lines
//! the editor asks for (the ones on screen).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use ropey::Rope;
use tree_sitter::{InputEdit, Language, Node, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};
use tree_sitter_language::LanguageFn;

/// Documents larger than this aren't parsed; highlighting would make editing sluggish.
pub(crate) const MAX_PARSE_BYTES: usize = 16 * 1024 * 1024;
/// At most this much text is highlighted per request (protects against enormous single lines).
const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;

/// The kinds of text the editor colors. Theme colors are chosen on the Swift side.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HighlightStyle {
    Keyword,
    String,
    Escape,
    Comment,
    Number,
    Constant,
    Function,
    Type,
    Variable,
    Property,
    Operator,
    Punctuation,
    Tag,
    Attribute,
    Label,
    Namespace,
    Heading,
    Emphasis,
    Strong,
    Link,
    Code,
    Inserted,
    Deleted,
}

/// A colored run of text within one line. Columns are UTF-16 code units.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub line: u64,
    pub start: u64,
    pub end: u64,
    pub style: HighlightStyle,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct LanguageInfo {
    pub id: String,
    pub name: String,
}

struct LanguageDef {
    id: &'static str,
    name: &'static str,
    extensions: &'static [&'static str],
    file_names: &'static [&'static str],
    interpreters: &'static [&'static str],
    language: LanguageFn,
    queries: &'static [&'static str],
}

static LANGUAGES: &[LanguageDef] = &[
    LanguageDef {
        id: "c", name: "C", extensions: &["c", "h"], file_names: &[], interpreters: &[],
        language: tree_sitter_c::LANGUAGE, queries: &[tree_sitter_c::HIGHLIGHT_QUERY],
    },
    LanguageDef {
        id: "cpp", name: "C++", extensions: &["cc", "cpp", "cxx", "c++", "hpp", "hh", "hxx", "h++", "ino", "ipp", "tpp"],
        file_names: &[], interpreters: &[],
        language: tree_sitter_cpp::LANGUAGE, queries: &[tree_sitter_cpp::HIGHLIGHT_QUERY, tree_sitter_c::HIGHLIGHT_QUERY],
    },
    LanguageDef {
        id: "csharp", name: "C#", extensions: &["cs", "csx"], file_names: &[], interpreters: &[],
        language: tree_sitter_c_sharp::LANGUAGE, queries: &[tree_sitter_c_sharp::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "css", name: "CSS", extensions: &["css"], file_names: &[], interpreters: &[],
        language: tree_sitter_css::LANGUAGE, queries: &[tree_sitter_css::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "diff", name: "Diff", extensions: &["diff", "patch"], file_names: &[], interpreters: &[],
        language: tree_sitter_diff::LANGUAGE, queries: &[tree_sitter_diff::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "go", name: "Go", extensions: &["go"], file_names: &[], interpreters: &[],
        language: tree_sitter_go::LANGUAGE, queries: &[tree_sitter_go::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "html", name: "HTML", extensions: &["html", "htm", "xhtml", "shtml"], file_names: &[], interpreters: &[],
        language: tree_sitter_html::LANGUAGE, queries: &[tree_sitter_html::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "java", name: "Java", extensions: &["java"], file_names: &[], interpreters: &[],
        language: tree_sitter_java::LANGUAGE, queries: &[tree_sitter_java::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "javascript", name: "JavaScript", extensions: &["js", "mjs", "cjs", "jsx"], file_names: &[],
        interpreters: &["node", "deno", "bun"],
        language: tree_sitter_javascript::LANGUAGE,
        queries: &[tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY],
    },
    LanguageDef {
        id: "json", name: "JSON", extensions: &["json", "jsonc", "json5", "geojson", "webmanifest", "har", "ipynb"],
        file_names: &[".prettierrc", ".eslintrc", ".babelrc", "composer.lock", "flake.lock"], interpreters: &[],
        language: tree_sitter_json::LANGUAGE, queries: &[tree_sitter_json::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "lua", name: "Lua", extensions: &["lua"], file_names: &[], interpreters: &["lua"],
        language: tree_sitter_lua::LANGUAGE, queries: &[tree_sitter_lua::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "make", name: "Makefile", extensions: &["mk", "mak", "make"], file_names: &["Makefile", "makefile", "GNUmakefile"],
        interpreters: &["make"],
        language: tree_sitter_make::LANGUAGE, queries: &[tree_sitter_make::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "markdown", name: "Markdown", extensions: &["md", "markdown", "mdown", "mkd", "mdx"], file_names: &[],
        interpreters: &[],
        language: tree_sitter_md::LANGUAGE, queries: &[tree_sitter_md::HIGHLIGHT_QUERY_BLOCK],
    },
    LanguageDef {
        id: "php", name: "PHP", extensions: &["php", "phtml", "php3", "php4", "php5", "phps"], file_names: &[],
        interpreters: &["php"],
        language: tree_sitter_php::LANGUAGE_PHP, queries: &[tree_sitter_php::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "python", name: "Python", extensions: &["py", "pyw", "pyi"], file_names: &["SConstruct", "SConscript"],
        interpreters: &["python", "python2", "python3"],
        language: tree_sitter_python::LANGUAGE, queries: &[tree_sitter_python::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "ruby", name: "Ruby", extensions: &["rb", "rake", "gemspec", "ru"],
        file_names: &["Gemfile", "Rakefile", "Podfile", "Fastfile", "Brewfile"], interpreters: &["ruby"],
        language: tree_sitter_ruby::LANGUAGE, queries: &[tree_sitter_ruby::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "rust", name: "Rust", extensions: &["rs"], file_names: &[], interpreters: &[],
        language: tree_sitter_rust::LANGUAGE, queries: &[tree_sitter_rust::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "shell", name: "Shell", extensions: &["sh", "bash", "zsh", "ksh", "command", "tool"],
        file_names: &[".bashrc", ".bash_profile", ".zshrc", ".zprofile", ".zshenv", ".profile", ".bash_aliases"],
        interpreters: &["sh", "bash", "zsh", "ksh", "dash"],
        language: tree_sitter_bash::LANGUAGE, queries: &[tree_sitter_bash::HIGHLIGHT_QUERY],
    },
    LanguageDef {
        id: "sql", name: "SQL", extensions: &["sql"], file_names: &[], interpreters: &[],
        language: tree_sitter_sequel::LANGUAGE, queries: &[tree_sitter_sequel::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "swift", name: "Swift", extensions: &["swift"], file_names: &[], interpreters: &["swift"],
        language: tree_sitter_swift::LANGUAGE, queries: &[tree_sitter_swift::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "toml", name: "TOML", extensions: &["toml"], file_names: &["Cargo.lock", "Pipfile", "poetry.lock"],
        interpreters: &[],
        language: tree_sitter_toml_ng::LANGUAGE, queries: &[tree_sitter_toml_ng::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "tsx", name: "TSX", extensions: &["tsx"], file_names: &[], interpreters: &[],
        language: tree_sitter_typescript::LANGUAGE_TSX,
        queries: &[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ],
    },
    LanguageDef {
        id: "typescript", name: "TypeScript", extensions: &["ts", "mts", "cts"], file_names: &[], interpreters: &[],
        language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        queries: &[tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY],
    },
    LanguageDef {
        id: "xml", name: "XML", extensions: &["xml", "plist", "svg", "xsd", "xsl", "xslt", "csproj", "xib", "storyboard", "entitlements", "rss", "atom"],
        file_names: &[], interpreters: &[],
        language: tree_sitter_xml::LANGUAGE_XML, queries: &[tree_sitter_xml::XML_HIGHLIGHT_QUERY],
    },
    LanguageDef {
        id: "yaml", name: "YAML", extensions: &["yml", "yaml"], file_names: &[".clang-format", ".clangd"], interpreters: &[],
        language: tree_sitter_yaml::LANGUAGE, queries: &[tree_sitter_yaml::HIGHLIGHTS_QUERY],
    },
];

/// Every language TuplePad can highlight, sorted by name.
#[uniffi::export]
pub fn languages() -> Vec<LanguageInfo> {
    let mut list: Vec<_> = LANGUAGES
        .iter()
        .map(|l| LanguageInfo { id: l.id.to_string(), name: l.name.to_string() })
        .collect();
    list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    list
}

/// Guesses a language from a file name, then from a `#!` line.
#[uniffi::export]
pub fn detect_language(file_name: String, first_line: String) -> Option<String> {
    detect(&file_name, &first_line).map(|l| l.id.to_string())
}

fn detect(file_name: &str, first_line: &str) -> Option<&'static LanguageDef> {
    let name = file_name.rsplit('/').next().unwrap_or(file_name);
    if let Some(language) = LANGUAGES.iter().find(|l| l.file_names.contains(&name)) {
        return Some(language);
    }
    if let Some((_, extension)) = name.rsplit_once('.') {
        let extension = extension.to_lowercase();
        if let Some(language) = LANGUAGES.iter().find(|l| l.extensions.contains(&extension.as_str())) {
            return Some(language);
        }
    }
    let shebang = first_line.strip_prefix("#!")?;
    let mut words = shebang.split_whitespace();
    let mut program = words.next()?.rsplit('/').next()?;
    if program == "env" {
        program = words.find(|w| !w.starts_with('-'))?;
    }
    let program = program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    LANGUAGES.iter().find(|l| l.interpreters.iter().any(|i| i.trim_end_matches(|c: char| c.is_ascii_digit()) == program))
}

fn definition(id: &str) -> Option<&'static LanguageDef> {
    LANGUAGES.iter().find(|l| l.id == id)
}

/// A grammar with its highlight query compiled (compiling takes a few milliseconds, so it's shared).
struct Compiled {
    language: Language,
    query: Query,
    /// The style for each capture in the query, by capture index.
    styles: Vec<Option<HighlightStyle>>,
}

fn compiled(id: &'static str) -> Option<Arc<Compiled>> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<Arc<Compiled>>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(id)
        .or_insert_with(|| {
            let def = definition(id)?;
            let language = Language::new(def.language);
            let source = def.queries.join("\n");
            let query = Query::new(&language, &source).ok()?;
            let styles = query.capture_names().iter().map(|name| style_for_capture(name)).collect();
            Some(Arc::new(Compiled { language, query, styles }))
        })
        .clone()
}

/// Maps a capture name like `keyword.control.import` to a style, trying the most specific match first.
fn style_for_capture(name: &str) -> Option<HighlightStyle> {
    use HighlightStyle::*;
    let mut candidate = name;
    loop {
        let style = match candidate {
            "comment" | "comment.documentation" => Some(Comment),
            "string.escape" | "escape" | "string.special" | "string.regex" | "string.regexp" | "string.special.regex" => Some(Escape),
            "string" | "character" => Some(String),
            "number" | "float" | "constant.numeric" | "number.float" => Some(Number),
            "boolean" | "constant" | "constant.builtin" | "constant.character" => Some(Constant),
            "function" | "method" | "function.method" | "function.macro" | "function.builtin" | "function.call" | "macro" => Some(Function),
            "type" | "type.builtin" | "constructor" | "class" | "struct" | "interface" | "enum" => Some(Type),
            "keyword" | "conditional" | "repeat" | "include" | "exception" | "storageclass" | "modifier"
            | "preproc" | "define" | "keyword.directive" | "variable.builtin" => Some(Keyword),
            "variable" | "variable.parameter" => Some(Variable),
            // JSON and YAML keys read best in the property color, apart from their string values.
            "property" | "field" | "variable.member" | "member" | "property.definition" | "string.special.key" => Some(Property),
            "operator" => Some(Operator),
            "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => Some(Punctuation),
            "punctuation.special" => Some(Operator),
            "tag" | "tag.builtin" => Some(Tag),
            "tag.attribute" | "attribute" | "attribute.builtin" | "decorator" => Some(Attribute),
            "label" => Some(Label),
            "module" | "namespace" => Some(Namespace),
            "text.title" | "markup.heading" | "title" => Some(Heading),
            "text.emphasis" | "markup.italic" | "emphasis" => Some(Emphasis),
            "text.strong" | "markup.bold" | "strong" => Some(Strong),
            "text.uri" | "text.reference" | "markup.link" | "markup.link.url" | "markup.link.label" | "link_uri" | "link_text" => Some(Link),
            "text.literal" | "markup.raw" | "markup.raw.block" | "text.code" => Some(Code),
            "diff.plus" | "diff.addition" | "addition" | "text.diff.add" | "attribute.diff.add" => Some(Inserted),
            "diff.minus" | "diff.deletion" | "deletion" | "text.diff.delete" | "attribute.diff.delete" => Some(Deleted),
            "none" | "embedded" | "spell" | "nospell" | "conceal" => return None,
            _ => None,
        };
        if style.is_some() {
            return style;
        }
        // keyword.control.import → keyword.control → keyword
        candidate = candidate.rsplit_once('.')?.0;
    }
}

/// How a language writes comments: a line prefix and/or a block pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CommentTokens {
    pub line: Option<&'static str>,
    pub block: Option<(&'static str, &'static str)>,
}

pub(crate) fn comment_tokens(id: &str) -> Option<CommentTokens> {
    let c_like = CommentTokens { line: Some("//"), block: Some(("/*", "*/")) };
    let hash = CommentTokens { line: Some("#"), block: None };
    let markup = CommentTokens { line: None, block: Some(("<!--", "-->")) };
    Some(match id {
        "c" | "cpp" | "csharp" | "go" | "java" | "javascript" | "rust" | "swift" | "typescript" | "tsx" | "php" => c_like,
        "css" => CommentTokens { line: None, block: Some(("/*", "*/")) },
        "python" | "ruby" | "shell" | "make" | "yaml" | "toml" => hash,
        "lua" => CommentTokens { line: Some("--"), block: Some(("--[[", "]]")) },
        "sql" => CommentTokens { line: Some("--"), block: Some(("/*", "*/")) },
        "html" | "xml" | "markdown" => markup,
        _ => return None,
    })
}

/// One document's parser and syntax tree.
pub(crate) struct Syntax {
    id: &'static str,
    compiled: Arc<Compiled>,
    parser: Parser,
    tree: Option<Tree>,
}

impl Syntax {
    pub fn new(id: &str) -> Option<Self> {
        let def = definition(id)?;
        let compiled = compiled(def.id)?;
        let mut parser = Parser::new();
        parser.set_language(&compiled.language).ok()?;
        Some(Syntax { id: def.id, compiled, parser, tree: None })
    }

    pub fn detect(file_name: &str, rope: &Rope) -> Option<Self> {
        let first_line = if rope.len_lines() > 0 { rope.line(0).to_string() } else { std::string::String::new() };
        Syntax::new(detect(file_name, &first_line)?.id)
    }

    pub fn id(&self) -> &'static str {
        self.id
    }

    /// Parses the document, reusing the previous tree for the parts that didn't change.
    pub fn parse(&mut self, rope: &Rope) {
        if rope.len_bytes() > MAX_PARSE_BYTES {
            self.tree = None;
            return;
        }
        let mut read = |byte: usize, _: Point| -> &[u8] {
            if byte >= rope.len_bytes() {
                return &[];
            }
            let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - chunk_start..]
        };
        self.tree = self.parser.parse_with_options(&mut read, self.tree.as_ref(), None);
    }

    /// Tells the tree about a change, before the document is reparsed.
    pub fn edit(&mut self, edit: &InputEdit) {
        if let Some(tree) = &mut self.tree {
            tree.edit(edit);
        }
    }

    pub fn comment_tokens(&self) -> Option<CommentTokens> {
        comment_tokens(self.id)
    }

    /// Whether the byte sits inside a string or comment (brackets there don't count when matching).
    pub fn in_string_or_comment(&self, byte: usize) -> bool {
        let Some(tree) = &self.tree else { return false };
        let mut node = tree.root_node().descendant_for_byte_range(byte, byte + 1);
        while let Some(current) = node {
            let kind = current.kind();
            if kind.contains("string") || kind.contains("comment") || kind.contains("char") || kind == "regex" {
                return true;
            }
            node = current.parent();
        }
        false
    }

    /// Forgets the tree so the next parse starts from scratch (after whole-document rewrites).
    pub fn reset(&mut self) {
        self.tree = None;
    }

    /// Highlighted runs for `count` lines starting at `start_line`.
    pub fn highlights(&self, rope: &Rope, start_line: usize, count: usize) -> Vec<HighlightSpan> {
        let Some(tree) = &self.tree else { return Vec::new() };
        let total_lines = rope.len_lines();
        if start_line >= total_lines || count == 0 {
            return Vec::new();
        }
        let end_line = (start_line + count).min(total_lines);
        let start_byte = rope.line_to_byte(start_line);
        let end_byte = rope.line_to_byte(end_line).min(start_byte + MAX_HIGHLIGHT_BYTES);
        if end_byte <= start_byte {
            return Vec::new();
        }

        // Paint each byte with a style. Later (more specific) captures overwrite earlier ones:
        // inner nodes beat the nodes around them, and for the same node the query's first pattern wins.
        let mut captures: Vec<(usize, usize, usize, HighlightStyle)> = Vec::new();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(start_byte..end_byte);
        let text = |node: Node| rope.byte_slice(node.byte_range()).chunks().map(str::as_bytes);
        let mut matches = cursor.captures(&self.compiled.query, tree.root_node(), text);
        while let Some((found, index)) = matches.next() {
            let capture = found.captures()[*index];
            if let Some(style) = self.compiled.styles[capture.index as usize] {
                let range = capture.node.byte_range();
                captures.push((range.start, range.end, found.pattern_index, style));
            }
        }
        captures.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(b.2.cmp(&a.2)));
        let mut paint: Vec<Option<HighlightStyle>> = vec![None; end_byte - start_byte];
        for (start, end, _, style) in captures {
            let from = start.max(start_byte) - start_byte;
            let to = end.min(end_byte).saturating_sub(start_byte);
            if from < to {
                paint[from..to].fill(Some(style));
            }
        }

        // Turn painted bytes into runs per line, measured in UTF-16 columns.
        let mut spans = Vec::new();
        for line in start_line..end_line {
            let line_start = rope.line_to_byte(line);
            if line_start >= end_byte {
                break;
            }
            let slice = rope.line(line);
            let mut byte = line_start;
            let mut column = 0u64;
            let mut current: Option<(u64, HighlightStyle)> = None;
            for chunk in slice.chunks() {
                for c in chunk.chars() {
                    if c == '\n' || c == '\r' || byte >= end_byte {
                        break;
                    }
                    let style = paint[byte - start_byte];
                    match (current, style) {
                        (Some((_, open)), Some(next)) if open == next => {}
                        (Some((from, open)), _) => {
                            spans.push(HighlightSpan { line: line as u64, start: from, end: column, style: open });
                            current = style.map(|s| (column, s));
                        }
                        (None, Some(next)) => current = Some((column, next)),
                        (None, None) => {}
                    }
                    byte += c.len_utf8();
                    column += c.len_utf16() as u64;
                }
            }
            if let Some((from, open)) = current {
                spans.push(HighlightSpan { line: line as u64, start: from, end: column, style: open });
            }
        }
        spans
    }
}

/// Describes replacing the bytes `start..old_end` so a syntax tree can follow along.
/// Call before changing the rope; finish with `new_end` once the new text is in.
pub(crate) fn edit_start(rope: &Rope, start_byte: usize, old_end_byte: usize) -> InputEdit {
    InputEdit {
        start_byte,
        old_end_byte,
        new_end_byte: start_byte,
        start_position: point(rope, start_byte),
        old_end_position: point(rope, old_end_byte),
        new_end_position: point(rope, start_byte),
    }
}

pub(crate) fn finish_edit(rope: &Rope, mut edit: InputEdit, inserted_bytes: usize) -> InputEdit {
    edit.new_end_byte = edit.start_byte + inserted_bytes;
    edit.new_end_position = point(rope, edit.new_end_byte);
    edit
}

fn point(rope: &Rope, byte: usize) -> Point {
    let byte = byte.min(rope.len_bytes());
    let row = rope.byte_to_line(byte);
    Point { row, column: byte - rope.line_to_byte(row) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_compiles() {
        for language in LANGUAGES {
            assert!(compiled(language.id).is_some(), "{} failed to compile its highlight query", language.id);
        }
    }

    #[test]
    fn detects_languages() {
        let cases = [
            ("main.rs", "", Some("rust")),
            ("App.TSX", "", Some("tsx")),
            ("Makefile", "", Some("make")),
            (".zshrc", "", Some("shell")),
            ("deploy", "#!/usr/bin/env python3", Some("python")),
            ("run", "#!/bin/bash -e", Some("shell")),
            ("server", "#!/usr/bin/env -S node --flag", Some("javascript")),
            ("notes.txt", "hello", None),
        ];
        for (name, first, expected) in cases {
            assert_eq!(detect_language(name.into(), first.into()).as_deref(), expected, "{name}");
        }
    }

    #[test]
    fn maps_capture_names() {
        assert_eq!(style_for_capture("keyword.control.import"), Some(HighlightStyle::Keyword));
        assert_eq!(style_for_capture("string.escape"), Some(HighlightStyle::Escape));
        assert_eq!(style_for_capture("variable.builtin"), Some(HighlightStyle::Keyword));
        assert_eq!(style_for_capture("embedded"), None);
    }

    fn spans_for(id: &str, text: &str) -> Vec<(u64, std::string::String, HighlightStyle)> {
        let rope = Rope::from_str(text);
        let mut syntax = Syntax::new(id).unwrap();
        syntax.parse(&rope);
        syntax
            .highlights(&rope, 0, rope.len_lines())
            .into_iter()
            .map(|s| {
                let line = rope.line(s.line as usize).to_string();
                let units: Vec<u16> = line.encode_utf16().collect();
                (s.line, std::string::String::from_utf16_lossy(&units[s.start as usize..s.end as usize]), s.style)
            })
            .collect()
    }

    #[test]
    fn highlights_rust() {
        let spans = spans_for("rust", "fn main() {\n    let s = \"hé👋\"; // hi\n}\n");
        assert!(spans.contains(&(0, "fn".into(), HighlightStyle::Keyword)), "{spans:?}");
        assert!(spans.contains(&(0, "main".into(), HighlightStyle::Function)), "{spans:?}");
        assert!(spans.contains(&(1, "let".into(), HighlightStyle::Keyword)), "{spans:?}");
        // UTF-16 columns line up even after the emoji.
        assert!(spans.contains(&(1, "\"hé👋\"".into(), HighlightStyle::String)), "{spans:?}");
        assert!(spans.contains(&(1, "// hi".into(), HighlightStyle::Comment)), "{spans:?}");
    }

    #[test]
    fn multi_line_comments_split_per_line() {
        let spans = spans_for("c", "/* one\n   two */ int x;\n");
        assert!(spans.contains(&(0, "/* one".into(), HighlightStyle::Comment)), "{spans:?}");
        assert!(spans.contains(&(1, "   two */".into(), HighlightStyle::Comment)), "{spans:?}");
        assert!(spans.contains(&(1, "int".into(), HighlightStyle::Type)), "{spans:?}");
    }
}
