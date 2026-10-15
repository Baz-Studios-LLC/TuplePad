//! The MCP tools. Each call runs on a blocking thread: the app may wait for its main thread, or
//! for the person to answer the permission question, inside it.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, InitializeRequestParams, InitializeResult, ServerCapabilities, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, Json, RoleServer, ServerHandler, tool, tool_handler, tool_router};

use crate::*;

/// The text tools `run_tool` takes, with what each does.
pub const TEXT_TOOLS: &[(&str, &str)] = &[
    ("json.format", "pretty-print JSON"),
    ("json.minify", "JSON on one line"),
    ("json.sort-keys", "pretty-print JSON with every object's keys sorted"),
    ("json.validate", "check JSON, reporting the first error's line and column"),
    ("xml.format", "pretty-print XML"),
    ("xml.linearize", "XML on one line"),
    ("xml.validate", "check XML"),
    ("lines.sort-ascending", "sort lines"),
    ("lines.sort-descending", "sort lines, Z to A"),
    ("lines.sort-case-insensitive", "sort lines ignoring case"),
    ("lines.sort-numeric", "sort lines by the number they start with"),
    ("lines.reverse", "reverse the order of the lines"),
    ("lines.remove-duplicates", "remove duplicate lines"),
    ("lines.remove-consecutive-duplicates", "remove lines that repeat the one before"),
    ("lines.remove-empty", "remove empty lines"),
    ("lines.remove-blank", "remove lines that are empty or only whitespace"),
    ("lines.join", "join the lines into one"),
    ("case.upper", "UPPERCASE"),
    ("case.lower", "lowercase"),
    ("case.title", "Title Case"),
    ("case.sentence", "Sentence case"),
    ("case.swap", "sWAP cASE"),
    ("whitespace.trim-trailing", "remove whitespace at line ends"),
    ("whitespace.trim-leading", "remove whitespace at line starts"),
    ("whitespace.trim-both", "remove whitespace at both ends of lines"),
    ("whitespace.tabs-to-spaces", "tabs to spaces"),
    ("whitespace.spaces-to-tabs", "leading spaces to tabs"),
    ("encode.base64", "encode as Base64"),
    ("decode.base64", "decode Base64"),
    ("encode.url", "percent-encode for URLs"),
    ("decode.url", "decode percent-encoding"),
    ("encode.html", "escape HTML entities"),
    ("decode.html", "unescape HTML entities"),
    ("encode.hex", "encode as hex"),
    ("decode.hex", "decode hex"),
    ("encode.quoted-printable", "encode as quoted-printable"),
    ("decode.quoted-printable", "decode quoted-printable"),
];

const INSTRUCTIONS: &str = "TuplePad is the text editor the person has open. These tools read and edit the documents \
open in it; edits show up live in front of the person, and each edit_document call is one step they can undo. \
Refer to documents by the id from list_documents (or by path). Lines and columns are 1-based, and columns count \
characters. Prefer edit_document with old_text for small changes; read_document first to see the current text, \
since the person may be editing too.";

#[derive(Clone)]
pub(crate) struct TuplePadServer {
    host: Arc<dyn Host>,
    connection: u64,
    version: String,
    tool_router: ToolRouter<Self>,
}

fn caller_name(context: &RequestContext<RoleServer>) -> String {
    context
        .client_info()
        .map(|info| info.title.filter(|t| !t.trim().is_empty()).unwrap_or(info.name))
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "An MCP client".to_string())
}

impl TuplePadServer {
    pub(crate) fn new(host: Arc<dyn Host>, connection: u64, version: String) -> Self {
        Self { host, connection, version, tool_router: Self::tool_router() }
    }

    /// Runs `work` for the caller on a blocking thread, after checking it's allowed, and records it.
    async fn call<T: Send + 'static>(
        &self,
        context: &RequestContext<RoleServer>,
        tool: &'static str,
        access: Access,
        work: impl FnOnce(&dyn Host) -> Result<T, String> + Send + 'static,
        summary: impl FnOnce(&T) -> String + Send + 'static,
    ) -> Result<T, String> {
        let caller = Caller { name: caller_name(context), connection: self.connection };
        let host = self.host.clone();
        tokio::task::spawn_blocking(move || {
            host.seen(&caller);
            let result = host.authorize(&caller, access).and_then(|()| work(&*host));
            match &result {
                Ok(value) => host.record(&caller, tool, Ok(&summary(value))),
                Err(error) => host.record(&caller, tool, Err(error)),
            }
            result
        })
        .await
        .map_err(|_| "TuplePad couldn't finish that.".to_string())?
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 { format!("1 {one}") } else { format!("{count} {many}") }
}

#[tool_router]
impl TuplePadServer {
    #[tool(
        description = "Lists the documents open in TuplePad: id, name, path, language, whether they have unsaved changes, line count, and which one the person is working in.",
        annotations(title = "List open documents", read_only_hint = true)
    )]
    async fn list_documents(&self, context: RequestContext<RoleServer>) -> Result<Json<DocumentList>, String> {
        self.call(&context, "list_documents", Access::Read, |host| Ok(DocumentList { documents: host.list_documents() }), |list| {
            plural(list.documents.len(), "document", "documents")
        })
        .await
        .map(Json)
    }

    #[tool(
        description = "Reads an open document's current text, including changes not yet saved, with line numbers. Give start_line and end_line to read part of a long document.",
        annotations(title = "Read a document", read_only_hint = true)
    )]
    async fn read_document(&self, Parameters(request): Parameters<ReadRequest>, context: RequestContext<RoleServer>) -> Result<String, String> {
        self.call(&context, "read_document", Access::Read, move |host| host.read_document(&request), |text| {
            text.lines().next().unwrap_or_default().to_string()
        })
        .await
    }

    #[tool(
        description = "Gets what the person has selected in the document they're working in: each selection's position and text (a caret has empty text).",
        annotations(title = "Get the selection", read_only_hint = true)
    )]
    async fn get_selection(&self, context: RequestContext<RoleServer>) -> Result<Json<SelectionInfo>, String> {
        self.call(&context, "get_selection", Access::Read, |host| host.selection(), |info| {
            format!("{} in {}", plural(info.selections.len(), "selection", "selections"), info.document.name)
        })
        .await
        .map(Json)
    }

    #[tool(
        description = "Opens a file in TuplePad (or shows it, if it's already open) and returns the document. Optionally scrolls to a line.",
        annotations(title = "Open a file", read_only_hint = true, open_world_hint = false)
    )]
    async fn open_file(&self, Parameters(request): Parameters<OpenRequest>, context: RequestContext<RoleServer>) -> Result<Json<DocumentInfo>, String> {
        self.call(&context, "open_file", Access::Read, move |host| host.open_file(&request), |doc| doc.name.clone()).await.map(Json)
    }

    #[tool(
        description = "Creates a new, unsaved document in TuplePad with the given text, and shows it.",
        annotations(title = "Create a document", read_only_hint = false, destructive_hint = false)
    )]
    async fn create_document(&self, Parameters(request): Parameters<CreateRequest>, context: RequestContext<RoleServer>) -> Result<Json<DocumentInfo>, String> {
        self.call(&context, "create_document", Access::Write, move |host| host.create_document(&request), |doc| doc.name.clone()).await.map(Json)
    }

    #[tool(
        description = "Changes an open document's text. The edits happen together, live in front of the person, as one step they can undo, and the document isn't saved. Each edit replaces old_text (which must appear exactly once; include enough surrounding text to make it unique) or a range given by start_line, start_column, end_line and end_column (1-based, columns count characters, end exclusive). Ranges refer to the text before any of the edits.",
        annotations(title = "Edit a document", read_only_hint = false, destructive_hint = true, idempotent_hint = false)
    )]
    async fn edit_document(&self, Parameters(request): Parameters<EditRequest>, context: RequestContext<RoleServer>) -> Result<String, String> {
        self.call(&context, "edit_document", Access::Write, move |host| host.edit_document(&request), |summary| summary.clone()).await
    }

    #[tool(
        description = "Finds text in an open document (the one the person is working in, by default): each match's line, column and the line's text. At most 500 matches.",
        annotations(title = "Find in a document", read_only_hint = true)
    )]
    async fn find_in_document(&self, Parameters(request): Parameters<FindRequest>, context: RequestContext<RoleServer>) -> Result<String, String> {
        self.call(&context, "find_in_document", Access::Read, move |host| host.find(&request), |text| {
            text.lines().next().unwrap_or_default().to_string()
        })
        .await
    }

    #[tool(
        description = "Searches the files in a folder (skipping what .gitignore leaves out, binary files and very large files), returning matching lines grouped by file.",
        annotations(title = "Find in files", read_only_hint = true)
    )]
    async fn find_in_files(&self, Parameters(request): Parameters<FindInFilesRequest>, context: RequestContext<RoleServer>) -> Result<String, String> {
        self.call(&context, "find_in_files", Access::Read, move |host| host.find_in_files(&request), |text| {
            text.lines().next().unwrap_or_default().to_string()
        })
        .await
    }

    #[tool(
        description = "Runs one of TuplePad's built-in text tools on a whole document, as one step the person can undo. Tools: json.format, json.minify, json.sort-keys, json.validate, xml.format, xml.linearize, xml.validate, lines.sort-ascending, lines.sort-descending, lines.sort-case-insensitive, lines.sort-numeric, lines.reverse, lines.remove-duplicates, lines.remove-consecutive-duplicates, lines.remove-empty, lines.remove-blank, lines.join, case.upper, case.lower, case.title, case.sentence, case.swap, whitespace.trim-trailing, whitespace.trim-leading, whitespace.trim-both, whitespace.tabs-to-spaces, whitespace.spaces-to-tabs, encode.base64, decode.base64, encode.url, decode.url, encode.html, decode.html, encode.hex, decode.hex, encode.quoted-printable, decode.quoted-printable. The validate tools only check and report.",
        annotations(title = "Run a text tool", read_only_hint = false, destructive_hint = true)
    )]
    async fn run_tool(&self, Parameters(request): Parameters<RunToolRequest>, context: RequestContext<RoleServer>) -> Result<String, String> {
        self.call(&context, "run_tool", Access::Write, move |host| host.run_tool(&request), |summary| summary.clone()).await
    }

    #[tool(
        description = "Saves an open document to its file. A new document needs a path; giving a path for a saved document saves it there instead, like Save As.",
        annotations(title = "Save a document", read_only_hint = false, destructive_hint = true)
    )]
    async fn save_document(&self, Parameters(request): Parameters<SaveRequest>, context: RequestContext<RoleServer>) -> Result<Json<DocumentInfo>, String> {
        self.call(&context, "save_document", Access::Write, move |host| host.save_document(&request), |doc| {
            doc.path.clone().unwrap_or_else(|| doc.name.clone())
        })
        .await
        .map(Json)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TuplePadServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tuplepad", self.version.clone()).with_title("TuplePad"))
            .with_instructions(INSTRUCTIONS)
    }

    async fn initialize(&self, request: InitializeRequestParams, context: RequestContext<RoleServer>) -> Result<InitializeResult, ErrorData> {
        context.peer.set_peer_info(request.clone());
        let caller = Caller { name: caller_name(&context), connection: self.connection };
        self.host.seen(&caller);
        self.negotiate_initialize(&request)
    }
}
