//! `tuplepad`: opens files in TuplePad from the terminal.
//!
//!     tuplepad notes.txt src/main.rs     open files
//!     tuplepad +42 main.rs               open at line 42 (also main.rs:42 or main.rs:42:7)
//!     git log | tuplepad -               open standard input as a new document
//!
//! It hands everything to the app through `tuplepad://` URLs, so it works whether or not
//! TuplePad is already running.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const BUNDLE_ID: &str = "com.bazstudios.TuplePad";

const HELP: &str = "\
Open files in TuplePad.

Usage:
  tuplepad [+LINE[:COLUMN]] FILE[:LINE[:COLUMN]]...
  tuplepad -                 Read standard input into a new document

Files that don't exist yet open as new documents and are created when you save.

Options:
  -h, --help                 Show this help
  -v, --version              Show the version
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return open_url("tuplepad://activate");
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
                println!("tuplepad {}", env!("CARGO_PKG_VERSION"));
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
    for url in urls {
        let status = open_url(&url);
        if status != ExitCode::SUCCESS {
            return status;
        }
    }
    ExitCode::SUCCESS
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

fn open_url(url: &str) -> ExitCode {
    match Command::new("/usr/bin/open").args(["-b", BUNDLE_ID, url]).status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => fail("couldn't open TuplePad. Is it installed?"),
        Err(e) => fail(&format!("couldn't run /usr/bin/open: {e}")),
    }
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
