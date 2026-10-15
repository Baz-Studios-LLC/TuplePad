//! `tuplepad`: opens files in TuplePad from the terminal.
//!
//!     tuplepad notes.txt src/main.rs     open files
//!     tuplepad +42 main.rs               open at line 42 (also main.rs:42 or main.rs:42:7)
//!     git log | tuplepad -               open standard input as a new document
//!     tuplepad mcp                       connect an AI tool to TuplePad (MCP over stdio)
//!
//! It hands everything to the app as `tuplepad://` links: over a local socket when TuplePad is
//! running, or by launching the TuplePad it came with (on a Mac, through Launch Services).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const BUNDLE_ID: &str = "com.bazstudios.TuplePad";

const HELP: &str = "\
Open files in TuplePad.

Usage:
  tuplepad [+LINE[:COLUMN]] FILE[:LINE[:COLUMN]]...
  tuplepad -                 Read standard input into a new document
  tuplepad mcp               Connect an AI tool to TuplePad's MCP server (turn it on in
                             Settings ▸ AI): claude mcp add tuplepad -- tuplepad mcp

Files that don't exist yet open as new documents and are created when you save.

Options:
  -h, --help                 Show this help
  -v, --version              Show the version
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return deliver(&["tuplepad://activate".to_string()]);
    }
    if args.len() == 1 && args[0] == "mcp" {
        return mcp_bridge();
    }
    let mut line: Option<(u64, Option<u64>)> = None;
    let mut urls = Vec::new();
    for arg in &args {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "-v" | "--version" => {
                // TuplePad's own version name (v001, …), from VERSION at the repository's root.
                println!("tuplepad {}", include_str!("../../../VERSION").trim());
                return ExitCode::SUCCESS;
            }
            "-" => match stdin_document() {
                Ok(url) => urls.push(url),
                Err(message) => return fail(&message),
            },
            _ if arg.starts_with('+') => match parse_position(&arg[1..]) {
                Some(position) => line = Some(position),
                None => return fail(&format!("“{arg}” isn't a line number, like +42 or +42:7")),
            },
            _ if arg.starts_with('-') => return fail(&format!("Unknown option “{arg}”. Try tuplepad --help.")),
            _ => {
                let (path, position) = split_position(arg);
                urls.push(open_document_url(&absolute(path), position.or(line.take())));
            }
        }
    }
    deliver(&urls)
}

/// TuplePad's data folder (as in the app's `settings::data_folder`).
fn data_folder() -> PathBuf {
    if let Some(folder) = std::env::var_os("TUPLEPAD_DATA_DIR") {
        return PathBuf::from(folder);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or(home)
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"))
    };
    base.join("TuplePad")
}

/// Where a running TuplePad listens (as in the app's `platform::socket_path`).
fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("TUPLEPAD_SOCKET") {
        return PathBuf::from(path);
    }
    data_folder().join("Slint/tuplepad.sock")
}

/// Where TuplePad's MCP server takes bridges (as in the app's `mcp::socket_path`).
fn mcp_socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("TUPLEPAD_MCP_SOCKET") {
        return PathBuf::from(path);
    }
    data_folder().join("Slint/mcp.sock")
}

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Stream = std::net::TcpStream;

fn connect(socket: &Path) -> Option<Stream> {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::connect(socket).ok()
    }
    #[cfg(windows)]
    {
        let port: u16 = std::fs::read_to_string(socket.with_extension("port")).ok()?.trim().parse().ok()?;
        std::net::TcpStream::connect(("127.0.0.1", port)).ok()
    }
}

/// `tuplepad mcp`: an AI tool starts this as its MCP server and talks to it over stdin and
/// stdout; it passes everything through to the running TuplePad, starting TuplePad (in the
/// background) if it isn't running.
fn mcp_bridge() -> ExitCode {
    const OFF: &str = "TuplePad's MCP server is off. Turn on “Let AI tools use TuplePad” in TuplePad's Settings ▸ AI.";
    let socket = mcp_socket_path();
    let stream = match connect(&socket) {
        Some(stream) => stream,
        // Pointed at a socket of its own (tests): never start a real TuplePad instead.
        None if std::env::var_os("TUPLEPAD_MCP_SOCKET").is_some() => return fail("no TuplePad MCP server is listening at TUPLEPAD_MCP_SOCKET."),
        None if connect(&socket_path()).is_some() => return fail(OFF),
        None => {
            if start_app(&[], true) != ExitCode::SUCCESS {
                return ExitCode::FAILURE;
            }
            // Wait for it to start; once it's up without its MCP socket, the server is off.
            let started = std::time::Instant::now();
            let mut running_since = None;
            loop {
                if let Some(stream) = connect(&socket) {
                    break stream;
                }
                if running_since.is_none() && connect(&socket_path()).is_some() {
                    running_since = Some(std::time::Instant::now());
                }
                if running_since.is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(3)) {
                    return fail(OFF);
                }
                if started.elapsed() > std::time::Duration::from_secs(30) {
                    return fail("TuplePad didn't start.");
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    };
    let Ok(mut to_app) = stream.try_clone() else { return fail("couldn't talk to TuplePad.") };
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut to_app);
        let _ = to_app.shutdown(std::net::Shutdown::Write);
    });
    let mut from_app = stream;
    let mut stdout = std::io::stdout().lock();
    let _ = std::io::copy(&mut from_app, &mut stdout);
    let _ = std::io::Write::flush(&mut stdout);
    ExitCode::SUCCESS
}

/// Sends the links to a running TuplePad. Returns false if none is listening.
fn send_to_running(urls: &[String]) -> bool {
    use std::io::Write;
    let message: String = urls.iter().map(|u| format!("{u}\n")).collect();
    #[cfg(unix)]
    {
        let Ok(mut stream) = std::os::unix::net::UnixStream::connect(socket_path()) else { return false };
        stream.write_all(message.as_bytes()).is_ok()
    }
    #[cfg(windows)]
    {
        let Ok(port) = std::fs::read_to_string(socket_path().with_extension("port")) else { return false };
        let Ok(port) = port.trim().parse::<u16>() else { return false };
        let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) else { return false };
        stream.write_all(message.as_bytes()).is_ok()
    }
}

/// Hands the links to TuplePad: the running one, or else the one this command came with.
fn deliver(urls: &[String]) -> ExitCode {
    if send_to_running(urls) {
        return ExitCode::SUCCESS;
    }
    // Pointed at a socket of its own (tests): never start a real TuplePad instead.
    if std::env::var_os("TUPLEPAD_SOCKET").is_some() {
        return fail("no TuplePad is listening at TUPLEPAD_SOCKET.");
    }
    start_app(urls, false)
}

/// Starts the TuplePad this command came with (on a Mac, through Launch Services), handing it
/// the links; `background` keeps it from coming to the front.
fn start_app(urls: &[String], background: bool) -> ExitCode {
    let exe = std::env::current_exe().and_then(|e| e.canonicalize()).ok();
    if cfg!(target_os = "macos") {
        // Inside TuplePad.app/Contents/Helpers: open that app; otherwise whichever is installed.
        let bundle = exe.as_deref().and_then(|e| e.ancestors().find(|a| a.extension().is_some_and(|x| x == "app")));
        let mut command = Command::new("/usr/bin/open");
        if background {
            command.arg("-g");
        }
        match bundle {
            Some(app) => command.arg("-a").arg(app),
            None => command.args(["-b", BUNDLE_ID]),
        };
        return match command.args(urls).status() {
            Ok(status) if status.success() => ExitCode::SUCCESS,
            Ok(_) => fail("couldn't open TuplePad. Is it installed?"),
            Err(e) => fail(&format!("couldn't run /usr/bin/open: {e}")),
        };
    }
    // Windows and Linux: start the TuplePad next to this command (or, as the Windows installer
    // lays it out, the TuplePad.exe above its bin folder).
    let folder = exe.as_deref().and_then(Path::parent);
    let candidates = match folder {
        Some(folder) if cfg!(windows) => vec![folder.join("tuplepad-app.exe"), folder.join("..").join("TuplePad.exe")],
        Some(folder) => vec![folder.join("tuplepad-app")],
        None => Vec::new(),
    };
    let Some(app) = candidates.into_iter().find(|p| p.exists()) else {
        return fail("couldn't find TuplePad next to this command.");
    };
    match Command::new(app).args(urls).spawn() {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => fail(&format!("couldn't start TuplePad: {e}")),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("tuplepad: {message}");
    ExitCode::FAILURE
}

/// "42" or "42:7".
fn parse_position(text: &str) -> Option<(u64, Option<u64>)> {
    let mut parts = text.splitn(2, ':');
    let line = parts.next()?.parse().ok()?;
    let column = match parts.next() {
        Some(column) => Some(column.parse().ok()?),
        None => None,
    };
    Some((line, column))
}

/// "file.rs:42:7" → ("file.rs", (42, 7)), but only when the file doesn't literally have that name.
fn split_position(arg: &str) -> (&str, Option<(u64, Option<u64>)>) {
    if Path::new(arg).exists() {
        return (arg, None);
    }
    let mut pieces = arg.rsplitn(3, ':');
    let last = pieces.next();
    let middle = pieces.next();
    let first = pieces.next();
    match (first, middle, last) {
        (Some(path), Some(line), Some(column)) if line.parse::<u64>().is_ok() && column.parse::<u64>().is_ok() => {
            (path, Some((line.parse().unwrap(), Some(column.parse().unwrap()))))
        }
        _ => match arg.rsplit_once(':') {
            Some((path, line)) if line.parse::<u64>().is_ok() && !path.is_empty() => (path, Some((line.parse().unwrap(), None))),
            _ => (arg, None),
        },
    }
}

fn absolute(path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|dir| dir.join(path)).unwrap_or_else(|_| path.to_path_buf())
    }
}

fn open_document_url(path: &Path, position: Option<(u64, Option<u64>)>) -> String {
    let mut url = format!("tuplepad://open?path={}", encode(&path.to_string_lossy()));
    if let Some((line, column)) = position {
        url.push_str(&format!("&line={line}"));
        if let Some(column) = column {
            url.push_str(&format!("&column={column}"));
        }
    }
    url
}

/// Saves standard input to a temporary file the app reads (and deletes) as a new document.
fn stdin_document() -> Result<String, String> {
    let mut text = Vec::new();
    std::io::stdin().read_to_end(&mut text).map_err(|e| format!("couldn't read standard input: {e}"))?;
    let path = std::env::temp_dir().join(format!("tuplepad-stdin-{}-{}.txt", std::process::id(), text.len()));
    std::fs::write(&path, &text).map_err(|e| format!("couldn't save standard input: {e}"))?;
    Ok(format!("tuplepad://new?from={}", encode(&path.to_string_lossy())))
}

/// Percent-encodes everything but unreserved characters and slashes.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_positions() {
        assert_eq!(parse_position("42"), Some((42, None)));
        assert_eq!(parse_position("42:7"), Some((42, Some(7))));
        assert_eq!(parse_position("x"), None);
        assert_eq!(split_position("/nope/main.rs:10"), ("/nope/main.rs", Some((10, None))));
        assert_eq!(split_position("/nope/main.rs:10:3"), ("/nope/main.rs", Some((10, Some(3)))));
        assert_eq!(split_position("/nope/plain.rs"), ("/nope/plain.rs", None));
    }

    #[test]
    fn builds_urls() {
        assert_eq!(
            open_document_url(Path::new("/tmp/a b/ü.txt"), Some((3, Some(4)))),
            "tuplepad://open?path=/tmp/a%20b/%C3%BC.txt&line=3&column=4"
        );
    }
}
