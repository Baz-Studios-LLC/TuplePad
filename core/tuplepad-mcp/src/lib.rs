//! TuplePad's built-in MCP server. AI tools (Claude Code, Claude Desktop and other MCP clients)
//! read and edit the documents open in TuplePad, and the person watches the edits happen.
//!
//! Built on the official `rmcp` crate. The app supplies the documents through [`Host`]; this
//! crate does the protocol and the transports: a local socket for the `tuplepad mcp` bridge
//! (which clients start as a stdio server), and Streamable HTTP on localhost.

mod listen;
mod server;

pub use listen::{Options, Running, start};
pub use server::TEXT_TOOLS;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether a tool only reads or also changes things.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// Who's calling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    /// The client's name as it introduces itself (its title, like "Claude Code", when it gives one).
    pub name: String,
    /// The socket connection it came on (each `tuplepad mcp` bridge is one), or 0 for HTTP.
    pub connection: u64,
}

/// What the app does for the server. Every method is called on a thread of its own, never the
/// app's main thread, so the app can wait for its main thread (or for the person) inside them.
pub trait Host: Send + Sync + 'static {
    /// Whether `caller` may use a tool of this kind. The first time, this asks the person.
    fn authorize(&self, caller: &Caller, access: Access) -> Result<(), String>;
    /// A client was heard from (for the status bar's indicator). Must not wait.
    fn seen(&self, caller: &Caller);
    /// A socket connection closed. Must not wait.
    fn closed(&self, connection: u64);
    /// A tool was used, for the activity log: its summary, or the error. Must not wait.
    fn record(&self, caller: &Caller, tool: &str, outcome: Result<&str, &str>);

    fn list_documents(&self) -> Vec<DocumentInfo>;
    fn read_document(&self, request: &ReadRequest) -> Result<String, String>;
    fn selection(&self) -> Result<SelectionInfo, String>;
    fn open_file(&self, request: &OpenRequest) -> Result<DocumentInfo, String>;
    fn create_document(&self, request: &CreateRequest) -> Result<DocumentInfo, String>;
    fn edit_document(&self, request: &EditRequest) -> Result<String, String>;
    fn find(&self, request: &FindRequest) -> Result<String, String>;
    fn find_in_files(&self, request: &FindInFilesRequest) -> Result<String, String>;
    fn run_tool(&self, request: &RunToolRequest) -> Result<String, String>;
    fn save_document(&self, request: &SaveRequest) -> Result<DocumentInfo, String>;
}

// MARK: What tools return

/// An open document.
#[derive(Serialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct DocumentInfo {
    /// Refers to this document in the other tools (a path works too, for saved documents).
    pub id: String,
    pub name: String,
    /// The file it's saved as; missing for a new document that hasn't been saved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The language it's highlighted as ("Plain Text" if none).
    pub language: String,
    /// Whether it has changes that aren't saved yet.
    pub unsaved: bool,
    pub lines: u64,
    /// Whether it's the document the person is working in.
    pub active: bool,
}

#[derive(Serialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct DocumentList {
    pub documents: Vec<DocumentInfo>,
}

/// A selection (or a caret, when start and end are the same). Lines and columns are 1-based;
/// columns count characters, and the end is just after the last selected character.
#[derive(Serialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SelectedText {
    pub start_line: u64,
    pub start_column: u64,
    pub end_line: u64,
    pub end_column: u64,
    pub text: String,
}

#[derive(Serialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SelectionInfo {
    pub document: DocumentInfo,
    pub selections: Vec<SelectedText>,
}

// MARK: What tools take

#[derive(Deserialize, JsonSchema, Clone, Debug, Default)]
pub struct ReadRequest {
    /// The document's id (from list_documents) or path.
    pub document: String,
    /// The first line to read (1-based). Default: the first.
    #[serde(default)]
    pub start_line: Option<u64>,
    /// The last line to read, inclusive. Default: the last.
    #[serde(default)]
    pub end_line: Option<u64>,
}

#[derive(Deserialize, JsonSchema, Clone, Debug, Default)]
pub struct OpenRequest {
    /// The file's absolute path. A file that doesn't exist yet opens as a new document that saves there.
    pub path: String,
    /// A line to show (1-based).
    #[serde(default)]
    pub line: Option<u64>,
}

#[derive(Deserialize, JsonSchema, Clone, Debug, Default)]
pub struct CreateRequest {
    /// The new document's text.
    #[serde(default)]
    pub text: String,
    /// A language to highlight it as, like "rust", "python", "markdown" or "json".
    #[serde(default)]
    pub language: Option<String>,
}

/// One change. Give either `old_text` (which must appear exactly once in the document) or a range:
/// `start_line`, `start_column`, `end_line` and `end_column`, 1-based, with columns counting
/// characters and the end just after the last character replaced. A range whose start and end
/// are the same inserts. Lines and columns refer to the document before any of the edits.
#[derive(Deserialize, JsonSchema, Clone, Debug, Default)]
pub struct TextEdit {
    #[serde(default)]
    pub old_text: Option<String>,
    #[serde(default)]
    pub start_line: Option<u64>,
    #[serde(default)]
    pub start_column: Option<u64>,
    #[serde(default)]
    pub end_line: Option<u64>,
    #[serde(default)]
    pub end_column: Option<u64>,
    /// The text to put there (empty to delete).
    pub new_text: String,
}

#[derive(Deserialize, JsonSchema, Clone, Debug, Default)]
pub struct EditRequest {
    /// The document's id (from list_documents) or path.
    pub document: String,
    /// The changes, made together as one step the person can undo. They must not overlap.
    pub edits: Vec<TextEdit>,
}

#[derive(Deserialize, JsonSchema, Clone, Debug, Default)]
pub struct FindRequest {
    /// The document's id or path. Default: the one the person is working in.
    #[serde(default)]
    pub document: Option<String>,
    /// What to look for.
    pub query: String,
    /// Treat the query as a regular expression (Rust regex syntax).
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub match_case: bool,
    #[serde(default)]
    pub whole_word: bool,
}

#[derive(Deserialize, JsonSchema, Clone, Debug, Default)]
pub struct FindInFilesRequest {
    /// The folder to search (absolute path). Files that .gitignore leaves out are skipped.
    pub folder: String,
    pub query: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub match_case: bool,
    #[serde(default)]
    pub whole_word: bool,
    /// Only files matching these patterns, comma-separated, like "*.rs, *.toml".
    #[serde(default)]
    pub include: Option<String>,
    /// Skip files and folders matching these patterns, comma-separated, like "build, *.min.js".
    #[serde(default)]
    pub exclude: Option<String>,
    /// At most this many matching lines (default 200).
    #[serde(default)]
    pub max_results: Option<u32>,
}

#[derive(Deserialize, JsonSchema, Clone, Debug, Default)]
pub struct RunToolRequest {
    /// The document's id or path.
    pub document: String,
    /// The tool's id, from the list in this tool's description.
    pub tool: String,
}

#[derive(Deserialize, JsonSchema, Clone, Debug, Default)]
pub struct SaveRequest {
    /// The document's id or path.
    pub document: String,
    /// Where to save it (absolute path). Needed for a new document; for a saved one, saves a copy
    /// there and the document becomes that file, like Save As.
    #[serde(default)]
    pub path: Option<String>,
}
