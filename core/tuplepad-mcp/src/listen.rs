//! Where the server listens: a local socket for the `tuplepad mcp` bridge (a Unix socket, or on
//! Windows a localhost port written beside the socket's path), and optionally Streamable HTTP
//! on 127.0.0.1. Both run on a Tokio runtime in a thread of their own.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;

use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio_util::sync::CancellationToken;

use crate::Host;
use crate::server::TuplePadServer;

pub struct Options {
    /// The bridge's socket. On Windows the port goes in this path with the extension `port`.
    pub socket: PathBuf,
    /// Streamable HTTP on 127.0.0.1 at this port (0 picks a free one), or none.
    pub http_port: Option<u16>,
    /// TuplePad's version, as clients see it.
    pub version: String,
}

/// The running server. Dropping it stops it.
pub struct Running {
    stop: CancellationToken,
    thread: Option<JoinHandle<()>>,
    socket: PathBuf,
    /// The HTTP port it listens on.
    pub http_port: Option<u16>,
    /// Why HTTP isn't running, when it was asked for (most likely the port is taken).
    pub http_error: Option<String>,
}

impl Running {
    pub fn http_url(&self) -> Option<String> {
        self.http_port.map(|port| format!("http://127.0.0.1:{port}/mcp"))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(self.socket.with_extension("port"));
    }
}

/// Starts listening. Fails if the bridge's socket can't be made; HTTP failing is reported in
/// [`Running::http_error`] instead, so the bridge still works.
pub fn start(host: Arc<dyn Host>, options: Options) -> std::io::Result<Running> {
    let stop = CancellationToken::new();
    // The listeners are made on the server's own thread (inside its runtime), which reports back.
    let (report, reported) = std::sync::mpsc::channel::<std::io::Result<(Option<u16>, Option<String>)>>();
    let thread = {
        let stop = stop.clone();
        let socket_path = options.socket.clone();
        let (http_port, version) = (options.http_port, options.version.clone());
        std::thread::Builder::new().name("tuplepad-mcp".into()).spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = report.send(Err(error));
                    return;
                }
            };
            runtime.block_on(async move {
                let socket = match bind_socket(&socket_path).await {
                    Ok(socket) => socket,
                    Err(error) => {
                        let _ = report.send(Err(error));
                        return;
                    }
                };
                let (http, http_error) = match http_port {
                    Some(port) => match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                        Ok(listener) => (Some(listener), None),
                        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => (None, Some(format!("Port {port} is already in use."))),
                        Err(error) => (None, Some(error.to_string())),
                    },
                    None => (None, None),
                };
                let port = http.as_ref().and_then(|l| l.local_addr().ok()).map(|a| a.port());
                let _ = report.send(Ok((port, http_error)));
                tokio::spawn(accept_bridges(socket, host.clone(), version.clone(), stop.clone()));
                if let (Some(listener), Some(port)) = (http, port) {
                    tokio::spawn(serve_http(listener, port, host, version, stop.clone()));
                }
                stop.cancelled().await;
            });
            runtime.shutdown_timeout(std::time::Duration::from_millis(500));
        })?
    };
    let (http_port, http_error) = reported.recv().map_err(|_| std::io::Error::other("The MCP server didn't start."))??;
    Ok(Running { stop, thread: Some(thread), socket: options.socket, http_port, http_error })
}

// MARK: The bridge's socket

#[cfg(unix)]
type SocketListener = tokio::net::UnixListener;
#[cfg(windows)]
type SocketListener = tokio::net::TcpListener;

#[cfg(unix)]
async fn bind_socket(path: &std::path::Path) -> std::io::Result<SocketListener> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    // A socket left behind by a TuplePad that didn't quit cleanly.
    if std::os::unix::net::UnixStream::connect(path).is_err() {
        let _ = std::fs::remove_file(path);
    }
    tokio::net::UnixListener::bind(path)
}

#[cfg(windows)]
async fn bind_socket(path: &std::path::Path) -> std::io::Result<SocketListener> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::write(path.with_extension("port"), listener.local_addr()?.port().to_string())?;
    Ok(listener)
}

/// Each `tuplepad mcp` bridge is a connection of its own, serving one client.
async fn accept_bridges(listener: SocketListener, host: Arc<dyn Host>, version: String, stop: CancellationToken) {
    static CONNECTIONS: AtomicU64 = AtomicU64::new(0);
    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(_) => continue,
            },
            () = stop.cancelled() => return,
        };
        let connection = CONNECTIONS.fetch_add(1, Ordering::Relaxed) + 1;
        let host = host.clone();
        let server = TuplePadServer::new(host.clone(), connection, version.clone());
        let stop = stop.clone();
        tokio::spawn(async move {
            if let Ok(running) = server.serve(stream).await {
                let token = running.cancellation_token();
                tokio::select! {
                    _ = running.waiting() => {}
                    () = stop.cancelled() => token.cancel(),
                }
            }
            host.closed(connection);
        });
    }
}

// MARK: Streamable HTTP

async fn serve_http(listener: tokio::net::TcpListener, port: u16, host: Arc<dyn Host>, version: String, stop: CancellationToken) {
    // Only this machine (the default allowed hosts are loopback), and no web pages: a browser's
    // request carries an Origin, which must be TuplePad's own.
    // Sessions for clients on older protocol versions, so a client is known by name after it
    // introduces itself (newer versions name the client in every request).
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(true)
        .with_cancellation_token(stop.child_token())
        .with_allowed_origins([format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")]);
    let service: StreamableHttpService<TuplePadServer, LocalSessionManager> =
        StreamableHttpService::new(move || Ok(TuplePadServer::new(host.clone(), 0, version.clone())), Default::default(), config);
    let router = axum::Router::new().nest_service("/mcp", service);
    let _ = axum::serve(listener, router).with_graceful_shutdown(async move { stop.cancelled_owned().await }).await;
}
