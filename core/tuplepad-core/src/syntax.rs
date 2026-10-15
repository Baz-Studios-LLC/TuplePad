//! Syntax highlighting with tree-sitter. Each document keeps a syntax tree that is
//! updated incrementally as it's edited; highlights are computed only for the lines
//! the editor asks for (the ones on screen).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use ropey::Rope;
use tree_sitter::{InputEdit, Language, Node, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};
use tree_sitter_language::LanguageFn;

/// Documents larger than this aren't parsed; highlighting would make editing sluggish.
// A syntax tree takes far more memory than its text (about 57 bytes per byte of minified JSON),
// so bigger files stay plain text, as in other editors.
pub(crate) const MAX_PARSE_BYTES: usize = 8 * 1024 * 1024;
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
        .chain([("csv", "CSV"), ("tsv", "TSV")].map(|(id, name)| LanguageInfo { id: id.into(), name: name.into() }))
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

/// Grammars used only inside other languages (not offered in the language menu).
static INTERNAL: &[LanguageDef] = &[LanguageDef {
    id: "markdown_inline", name: "Markdown (inline)", extensions: &[], file_names: &[], interpreters: &[],
    language: tree_sitter_md::INLINE_LANGUAGE, queries: &[tree_sitter_md::HIGHLIGHT_QUERY_INLINE],
}];

fn grammar(id: &str) -> Option<&'static LanguageDef> {
    definition(id).or_else(|| INTERNAL.iter().find(|l| l.id == id))
}

/// The language for a code fence's info string ("rust", "py", "c++"…).
fn language_for_name(name: &str) -> Option<&'static str> {
    let name = name.trim().to_lowercase();
    let alias = match name.as_str() {
        "bash" | "zsh" | "console" | "shell-session" | "sh" => "shell",
        "c++" => "cpp",
        "c#" | "cs" => "csharp",
        "golang" => "go",
        "js" | "node" => "javascript",
        "ts" => "typescript",
        "py" | "python3" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "yml" => "yaml",
        "md" => "markdown",
        other => other,
    };
    LANGUAGES
        .iter()
        .find(|l| l.id == alias || l.name.to_lowercase() == alias || l.extensions.contains(&alias))
        .map(|l| l.id)
}

/// Where other languages appear inside a language: `@content` with a `@language` capture
/// naming it, or `@content.<id>` for a fixed language.
fn injection_source(id: &str) -> Option<&'static str> {
    Some(match id {
        "markdown" => {
            "(fenced_code_block (info_string (language) @language) (code_fence_content) @content)
             (html_block) @content.html
             (minus_metadata) @content.yaml
             (plus_metadata) @content.toml
             (inline) @content.markdown_inline"
        }
        "html" => {
            "(script_element (raw_text) @content.javascript)
             (style_element (raw_text) @content.css)"
        }
        _ => return None,
    })
}

struct Injections {
    query: Query,
    /// For each capture: whether it's the content, and the fixed language it names, if any.
    content: Vec<(bool, Option<&'static str>)>,
    language_capture: Option<u32>,
}

fn injections(id: &'static str) -> Option<Arc<Injections>> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<Arc<Injections>>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(id)
        .or_insert_with(|| {
            let source = injection_source(id)?;
            let query = Query::new(&compiled(id)?.language, source).ok()?;
            let content = query
                .capture_names()
                .iter()
                .map(|name| match name.strip_prefix("content") {
                    Some("") => (true, None),
                    Some(rest) => (true, grammar(rest.trim_start_matches('.')).map(|l| l.id)),
                    None => (false, None),
                })
                .collect();
            let language_capture = query.capture_index_for_name("language");
            Some(Arc::new(Injections { query, content, language_capture }))
        })
        .clone()
}

/// A parsed snippet of an injected language, keyed by language and text so it's reused while
/// scrolling and typing elsewhere.
fn injected_tree(id: &'static str, text: &str) -> Option<Tree> {
    use std::hash::{Hash, Hasher};
    static CACHE: OnceLock<Mutex<HashMap<(&'static str, u64), Tree>>> = OnceLock::new();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let key = (id, hasher.finish());
    let cache = CACHE.get_or_init(Default::default);
    if let Some(tree) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Some(tree.clone());
    }
    let mut parser = Parser::new();
    parser.set_language(&compiled(id)?.language).ok()?;
    let tree = parser.parse(text, None)?;
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() > 512 {
        cache.clear();
    }
    cache.insert(key, tree.clone());
    Some(tree)
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
            let def = grammar(id)?;
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

/// A copy of a syntax tree for building the outline elsewhere.
pub(crate) struct OutlineSnapshot {
    id: &'static str,
    compiled: Arc<Compiled>,
    tree: Tree,
}

impl OutlineSnapshot {
    pub fn outline(&self, rope: &Rope, limit: usize) -> Vec<crate::outline::OutlineItem> {
        crate::outline::outline(self.id, &self.compiled.language, &self.tree, rope, limit)
    }
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

    /// The byte of the bracket that pairs with the one at `byte`, from the tree: a bracket token's
    /// parent (an array, a block, an argument list) starts with the opening bracket and ends with
    /// the closing one. Much faster than scanning for it, in a long line. None when the tree doesn't
    /// pair it that way; the caller checks the character there.
    pub fn bracket_partner(&self, byte: usize) -> Option<usize> {
        let tree = self.tree.as_ref()?;
        let token = tree.root_node().descendant_for_byte_range(byte, byte + 1)?;
        if token.start_byte() != byte || token.end_byte() != byte + 1 {
            return None;
        }
        let parent = token.parent()?;
        if parent.start_byte() == byte && parent.end_byte() > byte + 1 {
            Some(parent.end_byte() - 1)
        } else if parent.end_byte() == byte + 1 && parent.start_byte() < byte {
            Some(parent.start_byte())
        } else {
            None
        }
    }

    /// Foldable regions starting on lines `first ..= last`, as (first line, last hidden line), or
    /// nil if this language folds better by indentation (or isn't parsed).
    pub fn fold_ranges(&self, rope: &Rope, first: usize, last: usize) -> Option<Vec<(usize, usize)>> {
        if matches!(self.id, "python" | "yaml" | "ruby" | "lua" | "shell" | "make") {
            return None;
        }
        let tree = self.tree.as_ref()?;
        let mut ends: std::collections::BTreeMap<usize, usize> = Default::default();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            let (start_row, end_row) = (node.start_position().row, node.end_position().row);
            if end_row < first || start_row > last {
                continue;
            }
            if end_row > start_row {
                if let Some((start, end)) = fold_of(node, rope) {
                    if (first..=last).contains(&start) && end > start {
                        let entry = ends.entry(start).or_insert(end);
                        *entry = (*entry).max(end);
                    }
                }
                let mut cursor = node.walk();
                stack.extend(node.children(&mut cursor));
            }
        }
        Some(ends.into_iter().collect())
    }

    /// Lines inside Markdown code blocks within a range (none for other languages).
    pub fn code_block_lines(&self, start: usize, count: usize) -> Vec<usize> {
        if self.id != "markdown" {
            return Vec::new();
        }
        let Some(tree) = &self.tree else { return Vec::new() };
        let end = start + count;
        let mut lines = Vec::new();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            let (first, last) = (node.start_position().row, node.end_position().row);
            if last < start || first >= end {
                continue;
            }
            if matches!(node.kind(), "fenced_code_block" | "indented_code_block") {
                lines.extend(first.max(start)..=last.min(end - 1));
                continue;
            }
            let mut cursor = node.walk();
            stack.extend(node.children(&mut cursor));
        }
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    /// The document's functions, types and headings.
    #[cfg(test)]
    pub fn outline(&self, rope: &Rope, limit: usize) -> Vec<crate::outline::OutlineItem> {
        self.outline_snapshot().map(|snapshot| snapshot.outline(rope, limit)).unwrap_or_default()
    }

    /// What the outline needs, copied (cheaply) so it can be built without holding the buffer's lock.
    pub fn outline_snapshot(&self) -> Option<OutlineSnapshot> {
        Some(OutlineSnapshot { id: self.id, compiled: Arc::clone(&self.compiled), tree: self.tree.clone()? })
    }

    /// Forgets the tree so the next parse starts from scratch (after whole-document rewrites).
    pub fn reset(&mut self) {
        self.tree = None;
    }

    /// Highlighted runs for `count` lines starting at `start_line`.
    pub fn highlights(&self, rope: &Rope, start_line: usize, count: usize) -> Vec<HighlightSpan> {
        let total_lines = rope.len_lines();
        if self.tree.is_none() || start_line >= total_lines || count == 0 {
            return Vec::new();
        }
        let end_line = (start_line + count).min(total_lines);
        let start_byte = rope.line_to_byte(start_line);
        let end_byte = rope.line_to_byte(end_line).min(start_byte + MAX_HIGHLIGHT_BYTES);
        self.highlight_bytes(rope, start_line, start_byte, end_byte, 0)
    }

    /// Highlighted runs for part of one line, UTF-16 columns `start_column..end_column` (for very
    /// long lines, where only part is on screen).
    pub fn highlights_in_line(&self, rope: &Rope, line: usize, start_column: usize, end_column: usize) -> Vec<HighlightSpan> {
        if self.tree.is_none() || line >= rope.len_lines() {
            return Vec::new();
        }
        let slice = rope.line(line);
        let units = slice.len_utf16_cu();
        let (start_column, end_column) = (start_column.min(units), end_column.min(units));
        let line_char = rope.line_to_char(line);
        let start_byte = rope.char_to_byte(line_char + slice.utf16_cu_to_char(start_column));
        let end_byte = rope.char_to_byte(line_char + slice.utf16_cu_to_char(end_column)).min(start_byte + MAX_HIGHLIGHT_BYTES);
        self.highlight_bytes(rope, line, start_byte, end_byte, start_column as u64)
    }

    /// Highlighted runs for the bytes `start_byte..end_byte`, which start on `start_line` at
    /// UTF-16 column `start_column`.
    fn highlight_bytes(&self, rope: &Rope, start_line: usize, start_byte: usize, end_byte: usize, start_column: u64) -> Vec<HighlightSpan> {
        let Some(tree) = &self.tree else { return Vec::new() };
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
        let mut paint: Vec<Option<HighlightStyle>> = vec![None; end_byte - start_byte];
        paint_captures(&mut paint, start_byte, captures);

        // Other languages inside this one: code fences, inline Markdown, <script> and <style>.
        for (id, range) in self.injected_ranges(tree, rope, start_byte, end_byte) {
            let snippet = rope.byte_slice(range.clone()).to_string();
            paint_injection(&mut paint, start_byte, end_byte, id, &snippet, range.start, 0);
        }

        // Turn painted bytes into runs per line, measured in UTF-16 columns.
        let mut spans = Vec::new();
        for line in start_line..rope.len_lines() {
            let line_start = rope.line_to_byte(line);
            if line_start >= end_byte {
                break;
            }
            let from = line_start.max(start_byte);
            let line_end = if line + 1 < rope.len_lines() { rope.line_to_byte(line + 1) } else { rope.len_bytes() };
            let slice = rope.byte_slice(from..line_end.max(from));
            let mut byte = from;
            let mut column = if from == line_start { 0 } else { start_column };
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

impl Syntax {
    /// The stretches of other languages that overlap a byte range.
    fn injected_ranges(&self, tree: &Tree, rope: &Rope, start_byte: usize, end_byte: usize) -> Vec<(&'static str, std::ops::Range<usize>)> {
        let Some(rules) = injections(self.id) else { return Vec::new() };
        let text = |node: Node| rope.byte_slice(node.byte_range()).chunks().map(str::as_bytes);
        let source = |node: Node| rope.byte_slice(node.byte_range()).to_string();
        collect_injections(&rules, tree, start_byte, end_byte, text, source)
    }
}

fn collect_injections<'a, T: tree_sitter::TextProvider<I> + 'a, I: AsRef<[u8]>>(
    rules: &Injections,
    tree: &'a Tree,
    start_byte: usize,
    end_byte: usize,
    text: T,
    source: impl Fn(Node) -> std::string::String,
) -> Vec<(&'static str, std::ops::Range<usize>)> {
    let mut found = Vec::new();
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(start_byte..end_byte);
    let mut matches = cursor.matches(&rules.query, tree.root_node(), text);
    while let Some(m) = matches.next() {
        let mut language = None;
        let mut content = None;
        for capture in m.captures() {
            if Some(capture.index) == rules.language_capture {
                language = language_for_name(&source(capture.node));
            } else if let Some((true, fixed)) = rules.content.get(capture.index as usize) {
                content = Some(capture.node.byte_range());
                if fixed.is_some() {
                    language = *fixed;
                }
            }
        }
        if let (Some(id), Some(range)) = (language, content) {
            if range.start < range.end {
                found.push((id, range));
            }
        }
    }
    found
}

/// Paints captures (absolute byte ranges) into `paint`, which starts at `base`. Inner nodes beat
/// the nodes around them, and for the same node the query's first pattern wins.
fn paint_captures(paint: &mut [Option<HighlightStyle>], base: usize, mut captures: Vec<(usize, usize, usize, HighlightStyle)>) {
    captures.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(b.2.cmp(&a.2)));
    for (start, end, _, style) in captures {
        let from = start.max(base).saturating_sub(base).min(paint.len());
        let to = end.saturating_sub(base).min(paint.len());
        if from < to {
            paint[from..to].fill(Some(style));
        }
    }
}

/// Highlights an injected snippet (which starts at byte `offset` of the document) over `paint`.
fn paint_injection(paint: &mut [Option<HighlightStyle>], base: usize, end: usize, id: &'static str, snippet: &str, offset: usize, depth: usize) {
    let (Some(compiled), Some(tree)) = (compiled(id), injected_tree(id, snippet)) else { return };
    let from = base.max(offset);
    let to = end.min(offset + snippet.len());
    if from >= to {
        return;
    }
    // Code takes its colors from its own language; inline Markdown adds to the block styles.
    if id != "markdown_inline" {
        paint[from - base..to - base].fill(None);
    }
    let mut captures = Vec::new();
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(from - offset..to - offset);
    let mut matches = cursor.captures(&compiled.query, tree.root_node(), snippet.as_bytes());
    while let Some((found, index)) = matches.next() {
        let capture = found.captures()[*index];
        if let Some(style) = compiled.styles[capture.index as usize] {
            let range = capture.node.byte_range();
            captures.push((range.start + offset, range.end + offset, found.pattern_index, style));
        }
    }
    paint_captures(paint, base, captures);
    // One more level, for things like <script> inside HTML inside Markdown.
    if depth == 0 {
        if let Some(rules) = injections(id) {
            let nested = collect_injections(&rules, &tree, from - offset, to - offset, snippet.as_bytes(), |node| {
                snippet[node.byte_range()].to_string()
            });
            for (inner, range) in nested {
                paint_injection(paint, base, end, inner, &snippet[range.clone()], offset + range.start, depth + 1);
            }
        }
    }
}

/// The region a multi-line node folds: bracketed blocks keep their closing line visible;
/// comments, Markdown sections and code blocks fold whole.
fn fold_of(node: Node, rope: &Rope) -> Option<(usize, usize)> {
    let kind = node.kind();
    let start_row = node.start_position().row;
    // A node that ends at the very start of a line (like a Markdown section) ends on the line before.
    let end = node.end_position();
    let end_row = if end.column == 0 && end.row > start_row { end.row - 1 } else { end.row };
    // Does the node's last line start with this child (so it can stay visible after folding)?
    let starts_line = |child: Node| {
        let row = child.start_position().row;
        let line_start = rope.line_to_byte(row);
        rope.byte_slice(line_start..child.start_byte()).chars().all(char::is_whitespace)
    };
    if kind.contains("comment") {
        return Some((start_row, end_row));
    }
    if kind == "section" {
        // A Markdown heading's section, without the blank lines before the next heading.
        let mut end = end_row.min(rope.len_lines() - 1);
        while end > start_row && rope.line(end).chars().all(char::is_whitespace) {
            end -= 1;
        }
        return Some((start_row, end));
    }
    if matches!(kind, "element" | "fenced_code_block") {
        let last = node.child(node.child_count().checked_sub(1)?)?;
        let end = if last.start_position().row > start_row && starts_line(last) { last.start_position().row - 1 } else { end_row };
        return Some((start_row, end));
    }
    let open = node.child(0)?;
    let close = node.child(node.child_count().checked_sub(1)?)?;
    let pairs = [("{", "}"), ("[", "]"), ("(", ")")];
    if !pairs.iter().any(|&(o, c)| open.kind() == o && close.kind() == c) {
        return None;
    }
    let close_row = close.start_position().row;
    let end = if starts_line(close) { close_row.checked_sub(1)? } else { close_row };
    Some((open.start_position().row, end))
}

/// Code as HTML with `<span class="tp-…">` around each colored run, for the Markdown preview.
/// Nil if the language isn't known.
/// Highlight spans for a snippet of code in a named language.
pub(crate) fn highlight_spans(language_name: &str, code: &str) -> Option<Vec<crate::HighlightSpan>> {
    let id = language_for_name(language_name)?;
    let mut syntax = Syntax::new(id)?;
    let rope = Rope::from_str(code);
    syntax.parse(&rope);
    Some(syntax.highlights(&rope, 0, rope.len_lines()))
}

pub(crate) fn highlight_html(language_name: &str, code: &str) -> Option<std::string::String> {
    let id = language_for_name(language_name)?;
    let mut syntax = Syntax::new(id)?;
    let rope = Rope::from_str(code);
    syntax.parse(&rope);
    let spans = syntax.highlights(&rope, 0, rope.len_lines());
    let mut html = std::string::String::with_capacity(code.len() * 2);
    let mut spans = spans.into_iter().peekable();
    for (line_index, line) in code.split_inclusive('\n').enumerate() {
        let units: Vec<u16> = line.trim_end_matches(['\n', '\r']).encode_utf16().collect();
        let mut column = 0usize;
        while let Some(span) = spans.next_if(|s| s.line as usize == line_index) {
            let (start, end) = (span.start as usize, (span.end as usize).min(units.len()));
            if start < column || start >= end {
                continue;
            }
            html.push_str(&escape_html(&std::string::String::from_utf16_lossy(&units[column..start])));
            html.push_str(&format!("<span class=\"tp-{}\">", style_class(span.style)));
            html.push_str(&escape_html(&std::string::String::from_utf16_lossy(&units[start..end])));
            html.push_str("</span>");
            column = end;
        }
        html.push_str(&escape_html(&std::string::String::from_utf16_lossy(&units[column.min(units.len())..])));
        if line.ends_with('\n') {
            html.push('\n');
        }
    }
    Some(html)
}

pub(crate) fn escape_html(text: &str) -> std::string::String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The CSS class name for a style, like `keyword`.
pub(crate) fn style_class(style: HighlightStyle) -> &'static str {
    use HighlightStyle::*;
    match style {
        Keyword => "keyword",
        String => "string",
        Escape => "escape",
        Comment => "comment",
        Number => "number",
        Constant => "constant",
        Function => "function",
        Type => "type",
        Variable => "variable",
        Property => "property",
        Operator => "operator",
        Punctuation => "punctuation",
        Tag => "tag",
        Attribute => "attribute",
        Label => "label",
        Namespace => "namespace",
        Heading => "heading",
        Emphasis => "emphasis",
        Strong => "strong",
        Link => "link",
        Code => "code",
        Inserted => "inserted",
        Deleted => "deleted",
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
    fn every_outline_query_compiles() {
        for language in LANGUAGES {
            if let Some(source) = crate::outline::source_for_test(language.id) {
                let language_compiled = compiled(language.id).unwrap();
                let query = Query::new(&language_compiled.language, source);
                assert!(query.is_ok(), "{}: {:?}", language.id, query.err());
            }
        }
    }

    fn outline_of(id: &str, text: &str) -> Vec<(std::string::String, crate::outline::SymbolKind, u64, u32)> {
        let rope = Rope::from_str(text);
        let mut syntax = Syntax::new(id).unwrap();
        syntax.parse(&rope);
        syntax.outline(&rope, 100).into_iter().map(|i| (i.name, i.kind, i.line, i.depth)).collect()
    }

    #[test]
    fn outlines() {
        use crate::outline::SymbolKind::*;
        let rust = outline_of("rust", "struct Point { x: i32 }\n\nimpl Point {\n    fn new() -> Self { todo!() }\n}\n\nfn main() {}\n");
        assert_eq!(rust, vec![
            ("Point".into(), Type, 0, 0),
            ("impl Point".into(), Namespace, 2, 0),
            ("new".into(), Method, 3, 1),
            ("main".into(), Function, 6, 0),
        ]);
        let swift = outline_of("swift", "struct View {\n    func body() {}\n}\nextension View {\n    init() {}\n}\n");
        assert_eq!(swift, vec![
            ("View".into(), Type, 0, 0),
            ("body".into(), Method, 1, 1),
            ("View".into(), Namespace, 3, 0),
            ("init".into(), Method, 4, 1),
        ]);
        let python = outline_of("python", "class A:\n    def run(self):\n        pass\n\ndef main():\n    pass\n");
        assert_eq!(python, vec![("A".into(), Type, 0, 0), ("run".into(), Method, 1, 1), ("main".into(), Function, 4, 0)]);
        let markdown = outline_of("markdown", "# Guide\n\nIntro.\n\n## Install\n\nSteps.\n\n## Use  it\n");
        assert_eq!(markdown, vec![("Guide".into(), Heading, 0, 0), ("Install".into(), Heading, 4, 1), ("Use it".into(), Heading, 8, 1)]);
        let js = outline_of("javascript", "const add = (a, b) => a + b;\nclass Cart {\n  total() {}\n}\n");
        assert_eq!(js, vec![("add".into(), Function, 0, 0), ("Cart".into(), Type, 1, 0), ("total".into(), Method, 2, 1)]);
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
    fn markdown_inline_styles_and_code_fences() {
        let text = "# Title with *emphasis*\n\nSome **bold**, `code` and a [link](https://example.com).\n\n```rust\nfn main() {}\n```\n";
        let spans = spans_for("markdown", text);
        assert!(spans.contains(&(0, "*emphasis*".into(), HighlightStyle::Emphasis)) || spans.iter().any(|s| s.0 == 0 && s.1.contains("emphasis") && s.2 == HighlightStyle::Emphasis), "{spans:?}");
        assert!(spans.iter().any(|s| s.0 == 2 && s.1.contains("bold") && s.2 == HighlightStyle::Strong), "{spans:?}");
        assert!(spans.iter().any(|s| s.0 == 2 && s.1.contains("code") && s.2 == HighlightStyle::Code), "{spans:?}");
        assert!(spans.iter().any(|s| s.0 == 2 && s.1.contains("example.com") && s.2 == HighlightStyle::Link), "{spans:?}");
        // The fenced block is highlighted as Rust.
        assert!(spans.contains(&(5, "fn".into(), HighlightStyle::Keyword)), "{spans:?}");
        assert!(spans.contains(&(5, "main".into(), HighlightStyle::Function)), "{spans:?}");
    }

    #[test]
    fn html_scripts_and_styles_use_their_languages() {
        let text = "<style>\nbody { color: red; }\n</style>\n<script>\nconst x = 42;\n</script>\n";
        let spans = spans_for("html", text);
        assert!(spans.iter().any(|s| s.0 == 1 && s.1 == "color" && s.2 == HighlightStyle::Property), "{spans:?}");
        assert!(spans.contains(&(4, "const".into(), HighlightStyle::Keyword)), "{spans:?}");
        assert!(spans.contains(&(4, "42".into(), HighlightStyle::Number)), "{spans:?}");
    }

    #[test]
    fn fence_names_map_to_languages() {
        assert_eq!(language_for_name("py"), Some("python"));
        assert_eq!(language_for_name("C++"), Some("cpp"));
        assert_eq!(language_for_name("bash"), Some("shell"));
        assert_eq!(language_for_name("TypeScript"), Some("typescript"));
        assert_eq!(language_for_name("jsx"), Some("javascript"));
        assert_eq!(language_for_name("klingon"), None);
    }

    #[test]
    fn multi_line_comments_split_per_line() {
        let spans = spans_for("c", "/* one\n   two */ int x;\n");
        assert!(spans.contains(&(0, "/* one".into(), HighlightStyle::Comment)), "{spans:?}");
        assert!(spans.contains(&(1, "   two */".into(), HighlightStyle::Comment)), "{spans:?}");
        assert!(spans.contains(&(1, "int".into(), HighlightStyle::Type)), "{spans:?}");
    }
}
