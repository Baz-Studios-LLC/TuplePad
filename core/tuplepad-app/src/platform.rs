//! What the windowing layer doesn't do for us: files dropped on the window, the window coming to
//! the front, files and `tuplepad://` links opened by the system (Finder on a Mac), and requests
//! from the `tuplepad` command or a second launch, which arrive on a local socket.
//!
//! Everything ends up as a `Request`, queued and handled on the event loop.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{CustomApplicationHandler, EventResult, winit};

use crate::app::App;

pub enum Request {
    /// Open a file, at a line and column if given (1-based).
    Open { path: PathBuf, line: Option<u64>, column: Option<u64> },
    /// A new document with this text (standard input, via the `tuplepad` command).
    NewWithText(String),
    /// Bring the window to the front.
    Activate,
    /// Files are being dragged over the window (or not any more).
    Hovering(bool),
    /// Look at the open files on disk again.
    CheckDisk,
}

thread_local! {
    static APP: RefCell<Weak<App>> = RefCell::new(Weak::new());
    static PENDING: RefCell<Vec<Request>> = const { RefCell::new(Vec::new()) };
}

pub fn set_app(app: &Rc<App>) {
    APP.with(|a| *a.borrow_mut() = Rc::downgrade(app));
}

/// Queues a request for the event loop (call on the event loop's thread).
pub fn post(request: Request) {
    let first = PENDING.with(|p| {
        let mut pending = p.borrow_mut();
        pending.push(request);
        pending.len() == 1
    });
    if first {
        slint::Timer::single_shot(Duration::ZERO, handle_pending);
    }
}

fn handle_pending() {
    let requests = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
    let Some(app) = APP.with(|a| a.borrow().upgrade()) else {
        // Not ready yet: try again shortly.
        PENDING.with(|p| p.borrow_mut().extend(requests));
        slint::Timer::single_shot(Duration::from_millis(50), handle_pending);
        return;
    };
    let mut opens = Vec::new();
    for request in requests {
        match request {
            Request::Open { path, line, column } => opens.push((path, line, column)),
            Request::NewWithText(text) => app.new_document_with_text(&text),
            Request::Activate => app.activate(),
            Request::Hovering(on) => {
                if let Some(ui) = app.ui.upgrade() {
                    ui.set_dropping(on);
                }
            }
            Request::CheckDisk => app.check_disk(),
        }
    }
    if !opens.is_empty() {
        app.open_requests(opens);
        app.activate();
    }
    app.refresh();
}

/// Turns a `tuplepad://` link (or a plain path) into a request.
pub fn request_for(link: &str) -> Option<Request> {
    let Some(rest) = link.strip_prefix("tuplepad://") else {
        return Some(Request::Open { path: PathBuf::from(link), line: None, column: None });
    };
    let (action, query) = rest.split_once('?').unwrap_or((rest, ""));
    let value = |name: &str| {
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then(|| decode(value))
        })
    };
    match action.trim_end_matches('/') {
        "open" => Some(Request::Open {
            path: PathBuf::from(value("path")?),
            line: value("line").and_then(|l| l.parse().ok()),
            column: value("column").and_then(|c| c.parse().ok()),
        }),
        "new" => {
            // Standard input, saved to a temporary file by the command.
            let path = value("from")?;
            let bytes = std::fs::read(&path).ok()?;
            let _ = std::fs::remove_file(&path);
            Some(Request::NewWithText(String::from_utf8_lossy(&bytes).into_owned()))
        }
        "activate" => Some(Request::Activate),
        _ => None,
    }
}

/// Undoes percent-encoding.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// MARK: Window events

/// Sees the winit events Slint doesn't handle: dropped files and the window coming to the front.
pub struct WindowEvents;

impl CustomApplicationHandler for WindowEvents {
    fn window_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _winit_window: Option<&winit::window::Window>,
        _slint_window: Option<&slint::Window>,
        event: &WindowEvent,
    ) -> EventResult {
        match event {
            WindowEvent::HoveredFile(_) => post(Request::Hovering(true)),
            WindowEvent::HoveredFileCancelled => post(Request::Hovering(false)),
            WindowEvent::DroppedFile(path) => {
                post(Request::Hovering(false));
                post(Request::Open { path: path.clone(), line: None, column: None });
            }
            WindowEvent::Focused(true) => post(Request::CheckDisk),
            _ => {}
        }
        EventResult::Propagate
    }

    /// The app is quitting. On a Mac, ⌘Q ends the process without returning from the event loop,
    /// so this is where the session is saved for the last time.
    fn exiting(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) -> EventResult {
        if let Some(app) = APP.with(|a| a.borrow().upgrade()) {
            app.shut_down();
        }
        EventResult::Propagate
    }
}

// MARK: The local socket

/// Where a running TuplePad listens for the `tuplepad` command and second launches. Debug
/// builds use their own, so they never take requests meant for the app.
pub fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("TUPLEPAD_SOCKET") {
        return PathBuf::from(path);
    }
    let name = if cfg!(debug_assertions) { "tuplepad-debug" } else { "tuplepad" };
    crate::settings::data_folder().join("Slint").join(format!("{name}.sock"))
}

/// Hands links to a TuplePad that's already running. Returns false if none is.
pub fn send_to_running(links: &[String]) -> bool {
    #[cfg(unix)]
    {
        let Ok(mut stream) = std::os::unix::net::UnixStream::connect(socket_path()) else { return false };
        let message: String = links.iter().map(|l| format!("{l}\n")).collect();
        let message = if message.is_empty() { "tuplepad://activate\n".to_string() } else { message };
        stream.write_all(message.as_bytes()).is_ok()
    }
    #[cfg(windows)]
    {
        let Ok(port) = std::fs::read_to_string(socket_path().with_extension("port")) else { return false };
        let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port.trim().parse::<u16>().unwrap_or(0))) else { return false };
        let message: String = links.iter().map(|l| format!("{l}\n")).collect();
        let message = if message.is_empty() { "tuplepad://activate\n".to_string() } else { message };
        stream.write_all(message.as_bytes()).is_ok()
    }
}

/// Starts listening for requests on a background thread; each line is a link or a path.
pub fn listen() {
    #[cfg(unix)]
    {
        let path = socket_path();
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        // A socket file left by an app that didn't quit cleanly.
        let _ = std::fs::remove_file(&path);
        let listener = match std::os::unix::net::UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("couldn't listen for the tuplepad command: {error}");
                return;
            }
        };
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                forward_lines(stream);
            }
        });
    }
    #[cfg(windows)]
    {
        let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", 0)) else { return };
        if let Ok(address) = listener.local_addr() {
            let _ = std::fs::create_dir_all(socket_path().parent().unwrap_or(Path::new(".")));
            let _ = std::fs::write(socket_path().with_extension("port"), address.port().to_string());
        }
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                forward_lines(stream);
            }
        });
    }
}

/// Removes the socket when the app quits.
pub fn stop_listening() {
    #[cfg(unix)]
    let _ = std::fs::remove_file(socket_path());
    #[cfg(windows)]
    let _ = std::fs::remove_file(socket_path().with_extension("port"));
}

fn forward_lines(stream: impl std::io::Read + Send + 'static) {
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(request) = request_for(&line) {
                post(request);
            }
        });
    }
}

/// Links and paths given on the command line.
pub fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|dir| dir.join(path)).unwrap_or_else(|_| path.to_path_buf())
    }
}

// MARK: macOS

/// Files and links the system asks TuplePad to open (Finder, `open`, the `tuplepad` command)
/// arrive through the app delegate's `application:openURLs:`. winit's delegate doesn't have
/// one, so it's added here, before the app finishes launching.
#[cfg(target_os = "macos")]
pub fn install_open_handler() {
    use objc2::runtime::{AnyObject, Imp, Sel};
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{NSArray, NSURL};

    unsafe extern "C-unwind" fn open_urls(_this: *mut AnyObject, _sel: Sel, _app: *mut AnyObject, urls: *mut NSArray<NSURL>) {
        let Some(urls) = (unsafe { urls.as_ref() }) else { return };
        for url in urls.iter() {
            if url.isFileURL() {
                if let Some(path) = url.path() {
                    post(Request::Open { path: PathBuf::from(path.to_string()), line: None, column: None });
                }
            } else if let Some(link) = url.absoluteString()
                && let Some(request) = request_for(&link.to_string())
            {
                post(request);
            }
        }
    }

    let Some(mtm) = MainThreadMarker::new() else { return };
    let application = NSApplication::sharedApplication(mtm);
    let Some(delegate) = application.delegate() else {
        eprintln!("couldn't add the open-files handler: no app delegate yet");
        return;
    };
    let object: &AnyObject = delegate.as_ref();
    let class = object.class();
    let imp: Imp = unsafe { std::mem::transmute(open_urls as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSURL>)) };
    unsafe {
        objc2::ffi::class_addMethod(class as *const _ as *mut _, sel!(application:openURLs:), imp, c"v@:@@".as_ptr());
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install_open_handler() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_become_requests() {
        match request_for("tuplepad://open?path=/tmp/a%20b/%C3%BC.txt&line=3&column=4") {
            Some(Request::Open { path, line, column }) => {
                assert_eq!(path, PathBuf::from("/tmp/a b/ü.txt"));
                assert_eq!((line, column), (Some(3), Some(4)));
            }
            _ => panic!("not an open request"),
        }
        assert!(matches!(request_for("tuplepad://activate"), Some(Request::Activate)));
        assert!(matches!(request_for("/plain/path.txt"), Some(Request::Open { .. })));
        assert!(request_for("tuplepad://nonsense").is_none());
    }
}
