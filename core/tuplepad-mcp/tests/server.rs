//! The server over its real socket, with a stand-in for the app.

use std::sync::{Arc, Mutex};

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation};
use tuplepad_mcp::*;

#[derive(Default)]
struct FakeApp {
    log: Mutex<Vec<String>>,
}

impl Host for FakeApp {
    fn authorize(&self, caller: &Caller, access: Access) -> Result<(), String> {
        match (caller.name.as_str(), access) {
            (_, Access::Read) | ("Writer", Access::Write) => Ok(()),
            _ => Err(format!("{} may only read in TuplePad.", caller.name)),
        }
    }
    fn seen(&self, caller: &Caller) {
        self.log.lock().unwrap().push(format!("seen {}", caller.name));
    }
    fn closed(&self, _connection: u64) {
        self.log.lock().unwrap().push("closed".into());
    }
    fn record(&self, caller: &Caller, tool: &str, outcome: Result<&str, &str>) {
        self.log.lock().unwrap().push(format!("{} {tool}: {outcome:?}", caller.name));
    }
    fn list_documents(&self) -> Vec<DocumentInfo> {
        vec![DocumentInfo { id: "d1".into(), name: "notes.md".into(), path: None, language: "Markdown".into(), unsaved: true, lines: 3, active: true }]
    }
    fn read_document(&self, request: &ReadRequest) -> Result<String, String> {
        Ok(format!("{} (lines {:?}–{:?})\n1\thello", request.document, request.start_line, request.end_line))
    }
    fn selection(&self) -> Result<SelectionInfo, String> {
        Err("No document is open.".into())
    }
    fn open_file(&self, _: &OpenRequest) -> Result<DocumentInfo, String> {
        Err("unused".into())
    }
    fn create_document(&self, _: &CreateRequest) -> Result<DocumentInfo, String> {
        Err("unused".into())
    }
    fn edit_document(&self, request: &EditRequest) -> Result<String, String> {
        Ok(format!("{} edits to {}", request.edits.len(), request.document))
    }
    fn find(&self, _: &FindRequest) -> Result<String, String> {
        Err("unused".into())
    }
    fn find_in_files(&self, _: &FindInFilesRequest) -> Result<String, String> {
        Err("unused".into())
    }
    fn run_tool(&self, _: &RunToolRequest) -> Result<String, String> {
        Err("unused".into())
    }
    fn save_document(&self, _: &SaveRequest) -> Result<DocumentInfo, String> {
        Err("unused".into())
    }
}

fn text(result: &rmcp::model::CallToolResult) -> String {
    result.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_over_the_bridge_socket() {
    let folder = std::env::temp_dir().join(format!("tuplepad-mcp-test-{}", std::process::id()));
    let socket = folder.join("mcp.sock");
    let app = Arc::new(FakeApp::default());
    let running = start(app.clone(), Options { socket: socket.clone(), http_port: Some(0), version: "v001".into() }).unwrap();
    assert!(running.http_url().is_some_and(|url| url.starts_with("http://127.0.0.1:")));

    let connect = |name: &'static str| {
        let socket = socket.clone();
        async move {
            #[cfg(unix)]
            let stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
            #[cfg(windows)]
            let stream = {
                let port: u16 = std::fs::read_to_string(socket.with_extension("port")).unwrap().trim().parse().unwrap();
                tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap()
            };
            ClientConfig::new(ClientCapabilities::default(), Implementation::new(name, "1.0").with_title(name)).serve(stream).await.unwrap()
        }
    };

    // Every tool is offered, with the server's name and version.
    let writer = connect("Writer").await;
    let info = writer.peer_info().unwrap();
    let server = info.server_info.as_ref().unwrap();
    assert_eq!((server.name.as_str(), server.version.as_str()), ("tuplepad", "v001"));
    let mut names: Vec<String> = writer.list_all_tools().await.unwrap().into_iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        ["create_document", "edit_document", "find_in_document", "find_in_files", "get_selection", "list_documents", "open_file", "read_document", "run_tool", "save_document"]
    );

    // Structured results, and arguments reaching the app.
    let listed = writer.call_tool(CallToolRequestParams::new("list_documents")).await.unwrap();
    assert_eq!(listed.structured_content.unwrap()["documents"][0]["name"], "notes.md");
    let arguments = serde_json::json!({"document": "d1", "edits": [{"old_text": "a", "new_text": "b"}, {"start_line": 1, "start_column": 1, "end_line": 1, "end_column": 1, "new_text": "x"}]});
    let edited = writer.call_tool(CallToolRequestParams::new("edit_document").with_arguments(arguments.as_object().unwrap().clone())).await.unwrap();
    assert_eq!(text(&edited), "2 edits to d1");
    assert_ne!(edited.is_error, Some(true));

    // A client allowed only to read gets an error from tools that change things.
    let reader = connect("Reader").await;
    let refused = reader.call_tool(CallToolRequestParams::new("edit_document").with_arguments(arguments.as_object().unwrap().clone())).await.unwrap();
    assert_eq!((text(&refused).as_str(), refused.is_error), ("Reader may only read in TuplePad.", Some(true)));
    // An app error comes back as a tool error, not a protocol error.
    let failed = reader.call_tool(CallToolRequestParams::new("get_selection")).await.unwrap();
    assert_eq!(failed.is_error, Some(true));

    // Every tool use is recorded for the activity log; closing a bridge is noticed.
    reader.cancel().await.unwrap();
    writer.cancel().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let log = app.log.lock().unwrap().clone();
    assert!(log.contains(&"Writer edit_document: Ok(\"2 edits to d1\")".to_string()), "{log:?}");
    assert!(log.contains(&"Reader edit_document: Err(\"Reader may only read in TuplePad.\")".to_string()), "{log:?}");
    assert_eq!(log.iter().filter(|l| *l == "closed").count(), 2, "{log:?}");

    // The run_tool description lists every text tool.
    let tools = rmcp::ServiceExt::serve(ClientConfig::default(), {
        #[cfg(unix)]
        { tokio::net::UnixStream::connect(&socket).await.unwrap() }
        #[cfg(windows)]
        { tokio::net::TcpStream::connect(("127.0.0.1", std::fs::read_to_string(socket.with_extension("port")).unwrap().trim().parse::<u16>().unwrap())).await.unwrap() }
    })
    .await
    .unwrap()
    .list_all_tools()
    .await
    .unwrap();
    let run_tool = tools.iter().find(|t| t.name == "run_tool").unwrap().description.clone().unwrap_or_default();
    for (id, _) in TEXT_TOOLS {
        assert!(run_tool.contains(id), "run_tool's description is missing {id}");
    }

    drop(running);
    assert!(!socket.exists(), "the socket is removed when the server stops");
}
