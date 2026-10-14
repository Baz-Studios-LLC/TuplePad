//! Scripted runs for checks, screenshots and timings: `--script steps.json`. Scripted runs never
//! save settings, never touch the real clipboard, and only keep a session with `--session-dir`.
//!
//! Each step is a one-key object, run in order on the event loop:
//! `{"wait": ms}`, `{"type": "text"}` (real key events, through the text input),
//! `{"key": "left", "mods": ["shift", "cmd", "alt", "ctrl"]}` (`cmd` is ⌘ on a Mac and Ctrl
//! elsewhere), `{"command": "edit.undo"}` (as the menu would), `{"click": [x, y], "mods": [...]}`
//! and `{"drag": [x0, y0, x1, y1]}` (editor coordinates), `{"scroll": dy}`, `{"goto": line}`,
//! `{"pointer": [[x, y], …]}` (real pointer events in window coordinates: press, move, release),
//! `{"select": doc}`, `{"open": path}`, `{"answer": "Save"}` (the next dialog's answer, or a
//! path for Open and Save As), `{"set": {"showInvisibles": true}}` (settings), `{"resize": [w, h]}`,
//! `{"preedit": "にほんご"}` and `{"commit": "日本語"}` (stand-ins for an input method, which
//! can't be scripted), `{"hover": true}` and `{"drop": [paths]}` (stand-ins for files dragged from
//! another app), `{"clipboard": "text"}`, `{"saveSession": true}`, `{"checkDisk": true}`, `{"dump": path}`,
//! `{"menus": path}`, `{"shot": signal path}` and `{"signal": path}` (write the file, then wait
//! for `<path>.done`: a harness takes a screenshot or changes files on disk meanwhile),
//! `{"benchScroll": {"name", "frames", "dy"}}`, `{"benchType": {"name", "text"}}`,
//! `{"benchJump": {"name", "count"}}`, `{"write": path}` and `{"quit": true}`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, RenderingState};

use crate::app::App;
use crate::{AppWindow, Theme};

#[derive(Default)]
struct FrameClock {
    before: Option<Instant>,
    /// Called (on the next turn of the event loop) after the next frame is drawn, with when it
    /// finished and how long drawing took.
    waiter: Option<Box<dyn FnOnce(Instant, f64)>>,
}

struct Runner {
    steps: Vec<Value>,
    next: usize,
    results: Map<String, Value>,
    clock: Rc<RefCell<FrameClock>>,
}

type Shared = Rc<RefCell<Runner>>;

pub fn start(app: Rc<App>, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let steps: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let clock = Rc::new(RefCell::new(FrameClock::default()));
    let ui = app.ui.upgrade().ok_or("no window")?;
    {
        let clock = clock.clone();
        let installed = ui.window().set_rendering_notifier(move |state, _| match state {
            RenderingState::BeforeRendering => clock.borrow_mut().before = Some(Instant::now()),
            RenderingState::AfterRendering => {
                let now = Instant::now();
                let mut clock = clock.borrow_mut();
                let drawing = clock.before.map_or(0.0, |b| ms(now - b));
                if let Some(waiter) = clock.waiter.take() {
                    slint::Timer::single_shot(Duration::ZERO, move || waiter(now, drawing));
                }
            }
            _ => {}
        });
        if let Err(error) = installed {
            eprintln!("frame timing unavailable: {error:?}");
        }
    }
    let mut results = Map::new();
    let opened: Map<String, Value> = app.open_ms.borrow().iter().map(|(name, ms)| (name.clone(), json!(round(*ms)))).collect();
    results.insert("open_ms".into(), Value::Object(opened));
    let runner = Rc::new(RefCell::new(Runner { steps, next: 0, results, clock }));
    later(app, runner, 600);
    Ok(())
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn later(app: Rc<App>, runner: Shared, delay_ms: u64) {
    slint::Timer::single_shot(Duration::from_millis(delay_ms), move || run_next(app, runner));
}

/// Calls `then` once the next frame has been drawn (or after a timeout, if nothing needed drawing).
fn after_frame(app: &Rc<App>, runner: &Shared, then: impl FnOnce(Instant, f64) + 'static) {
    let clock = runner.borrow().clock.clone();
    let done = Rc::new(RefCell::new(Some(Box::new(then) as Box<dyn FnOnce(Instant, f64)>)));
    {
        let done = done.clone();
        clock.borrow_mut().waiter = Some(Box::new(move |at, drawing| {
            if let Some(then) = done.borrow_mut().take() {
                then(at, drawing);
            }
        }));
    }
    if let Some(ui) = app.ui.upgrade() {
        ui.window().request_redraw();
    }
    slint::Timer::single_shot(Duration::from_millis(250), move || {
        if let Some(then) = done.borrow_mut().take() {
            clock.borrow_mut().waiter = None;
            then(Instant::now(), f64::NAN);
        }
    });
}

fn key_char(name: &str) -> Option<char> {
    let key = match name {
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "return" => Key::Return,
        "tab" => Key::Tab,
        "escape" => Key::Escape,
        other => return other.chars().next(),
    };
    Some(key.into())
}

fn modifier_keys(step: &Value) -> Vec<Key> {
    let names = step.get("mods").and_then(Value::as_array).cloned().unwrap_or_default();
    names
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|name| match name {
            "shift" => Some(Key::Shift),
            // Slint reports ⌘ as Control on a Mac, and the Control key as Meta.
            "cmd" => Some(Key::Control),
            "ctrl" => Some(Key::Meta),
            "alt" => Some(Key::Alt),
            _ => None,
        })
        .collect()
}

fn has_mod(step: &Value, name: &str) -> bool {
    step.get("mods").and_then(Value::as_array).is_some_and(|m| m.iter().any(|v| v == name))
}

fn press(ui: &AppWindow, ch: char, modifiers: &[Key]) {
    let window = ui.window();
    for modifier in modifiers {
        window.dispatch_event(WindowEvent::KeyPressed { text: (*modifier).into() });
    }
    window.dispatch_event(WindowEvent::KeyPressed { text: ch.into() });
    window.dispatch_event(WindowEvent::KeyReleased { text: ch.into() });
    for modifier in modifiers.iter().rev() {
        window.dispatch_event(WindowEvent::KeyReleased { text: (*modifier).into() });
    }
}

fn run_next(app: Rc<App>, runner: Shared) {
    let step = {
        let mut r = runner.borrow_mut();
        let step = r.steps.get(r.next).cloned();
        r.next += 1;
        step
    };
    let Some(step) = step else {
        slint::quit_event_loop().ok();
        return;
    };
    let Some(ui) = app.ui.upgrade() else { return };
    let Some((kind, arg)) = step.as_object().and_then(|o| o.iter().next()).map(|(k, v)| (k.clone(), v.clone())) else {
        return later(app, runner, 0);
    };
    let pause = 40;
    match kind.as_str() {
        "wait" => return later(app, runner, arg.as_u64().unwrap_or(100)),
        "type" => {
            for ch in arg.as_str().unwrap_or_default().chars() {
                press(&ui, ch, &[]);
            }
        }
        "key" => {
            if let Some(ch) = arg.as_str().and_then(key_char) {
                press(&ui, ch, &modifier_keys(&step));
            }
        }
        "click" | "drag" => {
            let n: Vec<f32> = arg.as_array().map(|a| a.iter().filter_map(Value::as_f64).map(|v| v as f32).collect()).unwrap_or_default();
            if n.len() >= 2 {
                let mut editor = app.editor.borrow_mut();
                editor.pointer_down(n[0], n[1], has_mod(&step, "cmd"), has_mod(&step, "alt"), has_mod(&step, "shift"));
                if n.len() >= 4 {
                    // Through a point between, as a real drag would.
                    editor.pointer_drag((n[0] + n[2]) / 2.0 + 3.0, (n[1] + n[3]) / 2.0 + 3.0);
                    editor.pointer_drag(n[2], n[3]);
                }
                editor.pointer_up(has_mod(&step, "copy"));
            }
        }
        "pointer" => {
            // Real pointer events in window coordinates: press at the first point, move through
            // the rest, release at the last.
            use slint::platform::PointerEventButton;
            let points: Vec<slint::LogicalPosition> = arg
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|p| Some(slint::LogicalPosition::new(p.get(0)?.as_f64()? as f32, p.get(1)?.as_f64()? as f32)))
                        .collect()
                })
                .unwrap_or_default();
            let window = ui.window();
            if let (Some(first), Some(last)) = (points.first(), points.last()) {
                window.dispatch_event(WindowEvent::PointerMoved { position: *first });
                window.dispatch_event(WindowEvent::PointerPressed { position: *first, button: PointerEventButton::Left });
                for point in &points[1..] {
                    window.dispatch_event(WindowEvent::PointerMoved { position: *point });
                }
                window.dispatch_event(WindowEvent::PointerReleased { position: *last, button: PointerEventButton::Left });
            }
        }
        "scroll" => app.editor.borrow_mut().scroll_by(0.0, -(arg.as_f64().unwrap_or(0.0) as f32)),
        "goto" => app.editor.borrow_mut().go_to_line(arg.as_u64().unwrap_or(0)),
        "select" => app.select_document(arg.as_u64().unwrap_or(0) as usize),
        "command" => app.run_command(arg.as_str().unwrap_or_default()),
        "open" => {
            app.open_paths(&[PathBuf::from(arg.as_str().unwrap_or_default())]);
        }
        "answer" => {
            if let Some(answers) = &app.scripted_answers {
                answers.borrow_mut().push_back(arg.as_str().unwrap_or_default().to_string());
            }
        }
        "set" => {
            let overrides: Vec<String> =
                arg.as_object().map(|o| o.iter().map(|(k, v)| format!("{k}={v}")).collect()).unwrap_or_default();
            let unknown = app.settings.borrow_mut().apply_overrides(&overrides);
            if !unknown.is_empty() {
                eprintln!("unknown settings: {unknown:?}");
            }
            app.apply_settings(false);
        }
        "resize" => {
            let size: Vec<f32> = arg.as_array().map(|a| a.iter().filter_map(Value::as_f64).map(|v| v as f32).collect()).unwrap_or_default();
            if size.len() == 2 {
                ui.window().set_size(slint::LogicalSize::new(size[0], size[1]));
            }
            return later(app, runner, 300);
        }
        "theme" => app.run_command(if arg == "dark" { "view.appearance-dark" } else { "view.appearance-light" }),
        "preedit" => app.editor.borrow_mut().set_preedit(arg.as_str().unwrap_or_default()),
        "commit" => app.editor.borrow_mut().insert_text(arg.as_str().unwrap_or_default()),
        "clipboard" => app.editor.borrow_mut().private_clipboard = Some(arg.as_str().unwrap_or_default().to_string()),
        "saveSession" => app.save_session(),
        "checkDisk" => app.check_disk(),
        // Stand-ins for files dragged from another app (winit events can't be scripted).
        "hover" => crate::platform::post(crate::platform::Request::Hovering(arg.as_bool().unwrap_or(true))),
        "drop" => {
            crate::platform::post(crate::platform::Request::Hovering(false));
            for path in arg.as_array().cloned().unwrap_or_default() {
                let path = PathBuf::from(path.as_str().unwrap_or_default());
                crate::platform::post(crate::platform::Request::Open { path, line: None, column: None });
            }
            return later(app, runner, 200);
        }
        "menus" => {
            let menus = dump_menus(&ui);
            if let Some(path) = arg.as_str() {
                let _ = std::fs::write(path, serde_json::to_string_pretty(&menus).unwrap_or_default());
            }
        }
        "dump" => {
            app.refresh();
            let state = dump(&app, &ui);
            if let Some(path) = arg.as_str() {
                let _ = std::fs::write(path, serde_json::to_string_pretty(&state).unwrap_or_default());
            }
        }
        "shot" | "signal" => {
            // A solid caret, not one caught mid-blink.
            app.editor.borrow_mut().touch();
            app.refresh();
            let signal = PathBuf::from(arg.as_str().unwrap_or("shot"));
            let _ = std::fs::write(&signal, "");
            let done = signal.with_extension("done");
            return wait_for(app, runner, done, Instant::now());
        }
        "benchScroll" => return bench_scroll(app, runner, arg),
        "benchJump" => return bench_jump(app, runner, arg),
        "benchType" => return bench_type(app, runner, arg),
        "write" => {
            let results = Value::Object(runner.borrow().results.clone());
            if let Some(path) = arg.as_str() {
                let _ = std::fs::write(path, serde_json::to_string_pretty(&results).unwrap_or_default());
            }
        }
        "quit" => {
            slint::quit_event_loop().ok();
            return;
        }
        other => eprintln!("unknown step {other}"),
    }
    app.refresh();
    later(app, runner, pause);
}

fn wait_for(app: Rc<App>, runner: Shared, path: PathBuf, since: Instant) {
    if path.exists() || since.elapsed() > Duration::from_secs(10) {
        return later(app, runner, 0);
    }
    slint::Timer::single_shot(Duration::from_millis(100), move || wait_for(app, runner, path, since));
}

fn dump(app: &App, ui: &AppWindow) -> Value {
    let (cell, line_height, rows) = app.editor.borrow_mut().metrics();
    let editor = app.editor.borrow();
    let doc = editor.doc();
    let cursors: Vec<Value> = doc
        .cursors
        .iter()
        .map(|c| json!([[c.anchor.line, c.anchor.column], [c.head.line, c.head.column]]))
        .collect();
    let docs: Vec<Value> = editor
        .docs
        .iter()
        .map(|d| json!({"name": d.name(), "modified": d.buffer.is_modified(), "path": d.buffer.path(), "wrap": d.wrap}))
        .collect();
    json!({
        "doc": doc.name(),
        "current": editor.current,
        "docs": docs,
        "cursors": cursors,
        "first_lines": doc.buffer.lines(0, 12),
        "text": if doc.buffer.len_utf16() < 20_000 { Value::String(doc.buffer.text()) } else { Value::Null },
        "line_count": doc.buffer.line_count(),
        "modified": doc.buffer.is_modified(),
        "wrap": doc.wrap,
        "scroll_y": doc.scroll_y,
        "status_left": ui.get_status_left().to_string(),
        "status_right": ui.get_status_right().to_string(),
        "title": ui.get_doc_title().to_string(),
        "subtitle": ui.get_doc_subtitle().to_string(),
        "dark": ui.global::<Theme>().get_dark(),
        "sidebar": ui.get_sidebar_visible(),
        "banner": ui.get_banner(),
        "dropping": ui.get_dropping(),
        "follow": doc.follow,
        "revealed": app.revealed.borrow().clone(),
        "printed": app.printed.borrow().clone(),
        "font": editor.family,
        "cell": cell,
        "line_height": line_height,
        "rows": rows,
        "scroll_x": doc.scroll_x,
        "font_size": editor.font_size,
        "clipboard": editor.private_clipboard,
        "settings": serde_json::to_value(&*app.settings.borrow()).unwrap_or_default(),
    })
}

fn dump_menus(ui: &AppWindow) -> Value {
    use slint::Model;
    let menu = |model: slint::ModelRc<crate::MenuEntry>| -> Vec<Value> {
        model
            .iter()
            .map(|e| json!({"id": e.id.as_str(), "title": e.title.as_str(), "enabled": e.enabled, "checked": e.checked, "shortcut": format!("{}", e.shortcut)}))
            .collect()
    };
    let file: Vec<Value> = [ui.get_file_menu(), ui.get_file_menu_2(), ui.get_file_menu_3()].into_iter().flat_map(menu).collect();
    json!({
        "File": file,
        "Open Recent": menu(ui.get_recent_menu()),
        "Reopen with Encoding": menu(ui.get_reopen_menu()),
        "Convert to Encoding": menu(ui.get_convert_menu()),
        "Line Endings": menu(ui.get_line_ending_menu()),
        "Edit": menu(ui.get_edit_menu()),
        "Selection": menu(ui.get_selection_menu()),
        "View": menu(ui.get_view_menu()),
    })
}

struct Samples {
    name: String,
    /// Time between drawn frames.
    frame: Vec<f64>,
    /// Time Slint spent drawing each frame.
    drawing: Vec<f64>,
    /// Time the editor spent updating its models.
    update: Vec<f64>,
    /// From key press to the frame that shows it.
    latency: Vec<f64>,
}

impl Samples {
    fn new(name: &str) -> Self {
        Samples { name: name.into(), frame: vec![], drawing: vec![], update: vec![], latency: vec![] }
    }

    fn summary(values: &[f64]) -> Value {
        let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
        if sorted.is_empty() {
            return Value::Null;
        }
        sorted.sort_by(f64::total_cmp);
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
        json!({ "n": sorted.len(), "mean": round(mean), "p50": round(at(0.5)), "p95": round(at(0.95)), "max": round(at(1.0)) })
    }

    fn store(self, runner: &Shared) {
        let mut entry = Map::new();
        for (key, values) in [("frame_ms", &self.frame), ("drawing_ms", &self.drawing), ("update_ms", &self.update), ("latency_ms", &self.latency)] {
            if !values.is_empty() {
                entry.insert(key.into(), Self::summary(values));
            }
        }
        eprintln!("{}: {}", self.name, Value::Object(entry.clone()));
        runner.borrow_mut().results.insert(self.name, Value::Object(entry));
    }
}

/// Scrolls a fixed distance every frame, as fast as frames are drawn.
fn bench_scroll(app: Rc<App>, runner: Shared, arg: Value) {
    let name = arg["name"].as_str().unwrap_or("scroll").to_string();
    let frames = arg["frames"].as_u64().unwrap_or(240) as usize;
    let dy = arg["dy"].as_f64().unwrap_or(40.0) as f32;
    let samples = Rc::new(RefCell::new(Samples::new(&name)));
    scroll_frame(app, runner, samples, frames, dy, None);
}

fn scroll_frame(app: Rc<App>, runner: Shared, samples: Rc<RefCell<Samples>>, left: usize, dy: f32, last: Option<Instant>) {
    if left == 0 {
        let samples = Rc::try_unwrap(samples).ok().map(RefCell::into_inner);
        if let Some(samples) = samples {
            samples.store(&runner);
        }
        return later(app, runner, 50);
    }
    app.editor.borrow_mut().scroll_by(0.0, -dy);
    app.refresh();
    samples.borrow_mut().update.push(app.editor.borrow().last_sync_ms);
    let (app2, runner2) = (app.clone(), runner.clone());
    after_frame(&app, &runner, move |at, drawing| {
        {
            let mut s = samples.borrow_mut();
            if let Some(last) = last {
                s.frame.push(ms(at - last));
            }
            s.drawing.push(drawing);
        }
        scroll_frame(app2, runner2, samples, left - 1, dy, Some(at));
    });
}

/// Jumps to random places in the document (like dragging the scrollbar), one per frame.
fn bench_jump(app: Rc<App>, runner: Shared, arg: Value) {
    let name = arg["name"].as_str().unwrap_or("jump").to_string();
    let count = arg["count"].as_u64().unwrap_or(60) as usize;
    let samples = Rc::new(RefCell::new(Samples::new(&name)));
    jump_frame(app, runner, samples, count, 0x2545F4914F6CDD1D);
}

fn jump_frame(app: Rc<App>, runner: Shared, samples: Rc<RefCell<Samples>>, left: usize, seed: u64) {
    if left == 0 {
        if let Some(samples) = Rc::try_unwrap(samples).ok().map(RefCell::into_inner) {
            samples.store(&runner);
        }
        return later(app, runner, 50);
    }
    let seed = seed ^ (seed << 13) ^ (seed >> 7) ^ (seed << 17);
    let fraction = (seed % 10_000) as f32 / 10_000.0;
    let started = Instant::now();
    app.editor.borrow_mut().scroll_to_fraction(fraction);
    app.refresh();
    samples.borrow_mut().update.push(app.editor.borrow().last_sync_ms);
    let (app2, runner2) = (app.clone(), runner.clone());
    after_frame(&app, &runner, move |at, drawing| {
        {
            let mut s = samples.borrow_mut();
            s.latency.push(ms(at - started));
            s.drawing.push(drawing);
        }
        jump_frame(app2, runner2, samples, left - 1, seed);
    });
}

/// Types text one key at a time through real key events, timing each key until its frame is drawn.
fn bench_type(app: Rc<App>, runner: Shared, arg: Value) {
    let name = arg["name"].as_str().unwrap_or("typing").to_string();
    let text: Vec<char> = arg["text"].as_str().unwrap_or("hello").chars().collect();
    let samples = Rc::new(RefCell::new(Samples::new(&name)));
    type_key(app, runner, samples, text, 0);
}

fn type_key(app: Rc<App>, runner: Shared, samples: Rc<RefCell<Samples>>, text: Vec<char>, index: usize) {
    let Some(&ch) = text.get(index) else {
        if let Some(samples) = Rc::try_unwrap(samples).ok().map(RefCell::into_inner) {
            samples.store(&runner);
        }
        return later(app, runner, 50);
    };
    let Some(ui) = app.ui.upgrade() else { return };
    let started = Instant::now();
    let ch = if ch == '\n' { char::from(Key::Return) } else { ch };
    press(&ui, ch, &[]);
    samples.borrow_mut().update.push(app.editor.borrow().last_sync_ms);
    let (app2, runner2) = (app.clone(), runner.clone());
    after_frame(&app, &runner, move |at, drawing| {
        {
            let mut s = samples.borrow_mut();
            s.latency.push(ms(at - started));
            s.drawing.push(drawing);
        }
        // A short pause between keys, like quick typing.
        slint::Timer::single_shot(Duration::from_millis(16), move || type_key(app2, runner2, samples, text, index + 1));
    });
}
