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
//! `{"settings": tab}`, `{"settingsInt": [key, value]}`, `{"settingsBool": [key, value]}` and
//! `{"closeSettings": true}` (the Settings window), `{"menus": path}`, `{"shot": signal path}` and `{"signal": path}` (write the file, then wait
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
use crate::{AppWindow, ColumnBridge, FindBridge, SearchBridge, Theme, ToolBridge};

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
        "move" => {
            // The pointer moving (no button) to a point in window coordinates, for hover states.
            if let Some(p) = arg.as_array().filter(|a| a.len() == 2) {
                let position = slint::LogicalPosition::new(p[0].as_f64().unwrap_or(0.0) as f32, p[1].as_f64().unwrap_or(0.0) as f32);
                ui.window().dispatch_event(WindowEvent::PointerMoved { position });
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
        "settings" => app.show_settings(arg.as_i64().map(|t| t as i32)),
        "closeSettings" => app.hide_settings(),
        "settingsInt" | "settingsBool" => {
            let key = arg.get(0).and_then(Value::as_str).unwrap_or_default();
            if let Some(window) = app.settings_window.borrow().as_ref() {
                if kind == "settingsInt" {
                    window.invoke_set_int(key.into(), arg.get(1).and_then(Value::as_i64).unwrap_or(0) as i32);
                } else {
                    window.invoke_set_bool(key.into(), arg.get(1).and_then(Value::as_bool).unwrap_or(false));
                }
            }
        }
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
        "find" => {
            // {"find": {"query": …, "replacement": …, "case": bool, "word": bool, "regex": bool,
            //  "replaceBar": bool, "action": "next" | "previous" | "replace" | "replaceAll" | "close"}}
            let bridge = ui.global::<FindBridge>();
            let mut changed = false;
            if let Some(query) = arg.get("query").and_then(Value::as_str) {
                if !bridge.get_open() {
                    app.open_find(false);
                }
                bridge.set_query(query.into());
                changed = true;
            }
            if let Some(text) = arg.get("replacement").and_then(Value::as_str) {
                bridge.set_replacement(text.into());
            }
            let flag = |name: &str| arg.get(name).and_then(Value::as_bool);
            if let Some(on) = flag("case") {
                bridge.set_case_sensitive(on);
                changed = true;
            }
            if let Some(on) = flag("word") {
                bridge.set_whole_word(on);
                changed = true;
            }
            if let Some(on) = flag("regex") {
                bridge.set_regex(on);
                changed = true;
            }
            if let Some(on) = arg.get("replaceBar").and_then(Value::as_bool) {
                bridge.set_show_replace(on);
            }
            if changed {
                app.find_changed();
            }
            match arg.get("action").and_then(Value::as_str) {
                Some("next") => app.find_step(true),
                Some("previous") => app.find_step(false),
                Some("replace") => app.replace_one(),
                Some("replaceAll") => app.replace_all_matches(),
                Some("close") => app.close_find(),
                _ => {}
            }
            return later(app, runner, 150);
        }
        "gutterClick" => {
            let pane = app.editor.borrow().focused_pane();
            app.toggle_bookmark_at_row(pane, arg.as_u64().unwrap_or(0));
        }
        "split" => app.run_command(match arg.as_str().unwrap_or("off") {
            "side" => "view.split-side",
            "stacked" => "view.split-stacked",
            _ => "view.split-off",
        }),
        "focusPane" => app.focus_pane(arg.as_u64().unwrap_or(0) as usize),
        "panes" => {
            let state: Vec<Value> = app
                .editor
                .borrow_mut()
                .panes_state()
                .into_iter()
                .map(|(name, cursors)| json!({"doc": name, "cursors": cursors.iter().map(|c| json!([[c.anchor.line, c.anchor.column], [c.head.line, c.head.column]])).collect::<Vec<_>>()}))
                .collect();
            let focused = app.editor.borrow().focused_pane();
            let split = format!("{:?}", app.editor.borrow().split());
            if let Some(path) = arg.as_str() {
                let _ = std::fs::write(path, serde_json::to_string_pretty(&json!({"split": split, "focused": focused, "panes": state})).unwrap_or_default());
            }
        }
        "minimapClick" => app.editor.borrow_mut().minimap_scroll(arg.as_f64().unwrap_or(0.0) as f32),
        "foldClick" => app.editor.borrow_mut().toggle_fold_at_row(arg.as_u64().unwrap_or(0)),
        "search" => {
            // {"search": {"scope": "documents" | "folder", "replace": bool, "folder": …, "query": …,
            //  "replacement": …, "include": …, "exclude": …, "case"/"word"/"regex"/"everything": bool,
            //  "action": "start" | "stop" | "close" | "prepareReplace" | "applyPlan" | "cancelPlan" | "undoAll",
            //  "activate": row, "open": row, "togglePlan": row, "undo": index}}
            let bridge = ui.global::<SearchBridge>();
            if let Some(scope) = arg.get("scope").and_then(Value::as_str) {
                let replace = arg.get("replace").and_then(Value::as_bool).unwrap_or(false);
                app.show_search(if scope == "folder" { 1 } else { 0 }, replace);
            }
            if let Some(folder) = arg.get("folder").and_then(Value::as_str) {
                app.set_search_folder_path(std::path::Path::new(folder));
            }
            let text = |name: &str| arg.get(name).and_then(Value::as_str).map(slint::SharedString::from);
            let flag = |name: &str| arg.get(name).and_then(Value::as_bool);
            if let Some(t) = text("query") {
                bridge.set_query(t);
            }
            if let Some(t) = text("replacement") {
                bridge.set_replacement(t);
            }
            if let Some(t) = text("include") {
                bridge.set_include(t);
            }
            if let Some(t) = text("exclude") {
                bridge.set_exclude(t);
            }
            if let Some(on) = flag("case") {
                bridge.set_case_sensitive(on);
            }
            if let Some(on) = flag("word") {
                bridge.set_whole_word(on);
            }
            if let Some(on) = flag("regex") {
                bridge.set_regex(on);
            }
            if let Some(on) = flag("everything") {
                bridge.set_search_everything(on);
            }
            let row = |name: &str| arg.get(name).and_then(Value::as_i64).map(|n| n as i32);
            if let Some(n) = row("activate") {
                bridge.set_selected(n);
                app.activate_search_row(n, 1);
            }
            if let Some(n) = row("open") {
                bridge.set_selected(n);
                app.activate_search_row(n, 2);
            }
            if let Some(n) = row("togglePlan").or_else(|| arg.get("togglePlan").and_then(Value::as_str).and_then(|t| app.plan_row_of(t))) {
                app.toggle_plan_file(n);
            }
            if let Some(n) = row("undo") {
                app.undo_replaced(n.max(0) as usize);
            }
            match arg.get("action").and_then(Value::as_str) {
                Some("start") => app.start_search(),
                Some("stop") => app.stop_search(),
                Some("close") => app.hide_search(),
                Some("prepareReplace") => app.prepare_replace(),
                Some("applyPlan") => app.apply_plan(),
                Some("cancelPlan") => app.cancel_plan(),
                Some("undoAll") => app.undo_all_replaced(),
                _ => {}
            }
        }
        "column" => {
            // {"column": {"mode": "text" | "numbers", "text", "start", "step", "repeat", "base": n,
            //  "padding": n, "action": "accept" | "cancel"}}; opens the Column Editor first.
            let bridge = ui.global::<ColumnBridge>();
            if !bridge.get_open() {
                app.open_column_editor();
            }
            if let Some(mode) = arg.get("mode").and_then(Value::as_str) {
                bridge.set_mode(if mode == "numbers" { 1 } else { 0 });
            }
            let text = |name: &str| arg.get(name).and_then(Value::as_str).map(slint::SharedString::from);
            if let Some(t) = text("text") {
                bridge.set_text(t);
            }
            if let Some(t) = text("start") {
                bridge.set_start(t);
            }
            if let Some(t) = text("step") {
                bridge.set_step(t);
            }
            if let Some(t) = text("repeat") {
                bridge.set_repeat(t);
            }
            if let Some(n) = arg.get("base").and_then(Value::as_i64) {
                bridge.set_base(n as i32);
            }
            if let Some(n) = arg.get("padding").and_then(Value::as_i64) {
                bridge.set_padding(n as i32);
            }
            app.column_changed();
            match arg.get("action").and_then(Value::as_str) {
                Some("accept") => app.accept_column_editor(),
                Some("cancel") => app.close_column_editor(),
                _ => {}
            }
        }
        "outline" => {
            if let Some(path) = arg.as_str() {
                let _ = std::fs::write(path, serde_json::to_string_pretty(&app.outline_dump()).unwrap_or_default());
            }
        }
        "outlineGo" => app.activate_outline_item(arg.as_u64().unwrap_or(0) as usize),
        "sheet" => {
            // {"sheet": {"hashCheck", "hashFile", "xpath", "csvColumn", "csvDescending", "csvNumeric",
            //  "csvHeader", "shell", "shellMode", "action": "accept" | "cancel" | "xpath"}}
            let bridge = ui.global::<ToolBridge>();
            let text = |name: &str| arg.get(name).and_then(Value::as_str).map(str::to_string);
            let flag = |name: &str| arg.get(name).and_then(Value::as_bool);
            if let Some(t) = text("hashCheck") {
                bridge.set_hash_check(t.into());
                app.hash_check_changed();
            }
            if let Some(t) = text("xpath") {
                bridge.set_xpath(t.into());
            }
            if let Some(n) = arg.get("csvColumn").and_then(Value::as_i64) {
                bridge.set_csv_column(n as i32);
            }
            if let Some(on) = flag("csvDescending") {
                bridge.set_csv_descending(on);
            }
            if let Some(on) = flag("csvNumeric") {
                bridge.set_csv_numeric(on);
            }
            if let Some(on) = flag("csvHeader") {
                bridge.set_csv_header(on);
            }
            if let Some(t) = text("shell") {
                bridge.set_shell_command(t.into());
            }
            if let Some(n) = arg.get("shellMode").and_then(Value::as_i64) {
                bridge.set_shell_mode(n as i32);
            }
            if let Some(path) = text("hashFile") {
                app.hash_path(PathBuf::from(path));
            }
            match arg.get("action").and_then(Value::as_str) {
                Some("accept") => app.accept_sheet(),
                Some("cancel") => app.close_sheet(),
                Some("xpath") => app.run_xpath(),
                _ => {}
            }
            return later(app, runner, 250);
        }
        "snippetEdit" => {
            // {"snippetEdit": {"add": true, "select": n, "trigger", "name", "language": n, "body", "remove": true}}
            if arg.get("add").and_then(Value::as_bool) == Some(true) {
                app.add_snippet();
            }
            if let Some(n) = arg.get("select").and_then(Value::as_i64) {
                app.select_snippet(n as i32);
            }
            let text = |name: &str| arg.get(name).and_then(Value::as_str);
            app.edit_snippet_fields(text("trigger"), text("name"), arg.get("language").and_then(Value::as_i64).map(|n| n as i32), text("body"));
            if arg.get("remove").and_then(Value::as_bool) == Some(true) {
                app.remove_snippet();
            }
        }
        "folder" => {
            // {"folder": path} opens a folder in the sidebar; {"folder": {"row": n}} clicks a row.
            match arg.as_str() {
                Some(path) => app.open_folder(std::path::Path::new(path)),
                None => {
                    if let Some(n) = arg.get("row").and_then(Value::as_u64) {
                        app.activate_folder_row(n as usize);
                    }
                }
            }
        }
        "folderState" => {
            if let Some(path) = arg.as_str() {
                let _ = std::fs::write(path, serde_json::to_string_pretty(&app.folder_dump()).unwrap_or_default());
            }
        }
        "waitSearch" => return wait_until(app, runner, |app| !app.search_busy(), Instant::now()),
        "searchStatus" => {
            let state = app.search_dump();
            if let Some(path) = arg.as_str() {
                let _ = std::fs::write(path, serde_json::to_string_pretty(&state).unwrap_or_default());
            }
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
            // In front, so the system draws it (macOS stops redrawing windows behind others).
            if kind == "shot" {
                use slint::winit_030::WinitWindowAccessor;
                ui.window().with_winit_window(|window| window.focus_window());
                ui.window().request_redraw();
            }
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

/// Carries on once `done` holds (or after 30 seconds).
fn wait_until(app: Rc<App>, runner: Shared, done: fn(&App) -> bool, since: Instant) {
    if done(&app) || since.elapsed() > Duration::from_secs(30) {
        return later(app, runner, 60);
    }
    slint::Timer::single_shot(Duration::from_millis(40), move || wait_until(app, runner, done, since));
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
    let sheet_state = {
        use slint::Model;
        let b = ui.global::<ToolBridge>();
        json!({
            "sheet": b.get_sheet(),
            "hash_source": b.get_hash_source().as_str(),
            "hashes": b.get_hashes().iter().map(|r| json!([r.name.as_str(), r.value.as_str(), r.matches])).collect::<Vec<_>>(),
            "xpath": b.get_xpath_results().iter().map(|r| json!([r.kind.as_str(), r.name.as_str(), r.value.as_str()])).collect::<Vec<_>>(),
            "xpath_status": b.get_xpath_status().as_str(),
            "csv_columns": b.get_csv_columns().iter().map(|c| c.to_string()).collect::<Vec<_>>(),
            "shell_input": b.get_shell_input().as_str(),
        })
    };
    let column_state = {
        let bridge = ui.global::<ColumnBridge>();
        json!({"open": bridge.get_open(), "preview": bridge.get_preview().as_str(), "target": bridge.get_target().as_str(), "valid": bridge.get_valid()})
    };
    let find_state = {
        use slint::Model;
        let bridge = ui.global::<FindBridge>();
        let editor_bridge = ui.global::<crate::EditorBridge>();
        json!({
            "open": bridge.get_open(),
            "replace_bar": bridge.get_show_replace(),
            "query": bridge.get_query().as_str(),
            "status": bridge.get_status().as_str(),
            "error": bridge.get_error(),
            "bands": editor_bridge.get_find_matches().row_count(),
            "marks": editor_bridge.get_marks().row_count(),
            "mark": doc.mark.as_ref().map(|q| q.pattern.clone()),
        })
    };
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
        "theme": ui.global::<Theme>().get_theme(),
        "settings_open": app.settings_window.borrow().as_ref().is_some_and(|w| w.window().is_visible()),
        "palette_open": ui.get_palette_open(),
        "palette": palette_titles(&ui),
        "palette_selected": ui.get_palette_selected(),
        "sidebar": ui.get_sidebar_visible(),
        "sidebar_mode": ui.get_sidebar_mode(),
        "outline_visible": ui.get_outline_visible(),
        "minimap": ui.global::<crate::EditorBridge>().get_minimap_width() > 0.0,
        "minimap_view": [ui.global::<crate::EditorBridge>().get_minimap_view_y(), ui.global::<crate::EditorBridge>().get_minimap_view_height()],
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
        "find": find_state,
        "column": column_state,
        "tool_sheet": sheet_state,
        "bookmarks": doc.buffer.bookmarks(),
        "completion": editor.completion.as_ref().map(|c| json!({"items": c.items, "selected": c.selected, "prefix": c.prefix})),
        "toast": app.last_toast.borrow().clone(),
        "tool_error": app.last_tool_error.borrow().clone(),
        "copied_html": app.last_copied_html.borrow().clone(),
        "folds": doc.buffer.folds().iter().map(|f| [f.start, f.end]).collect::<Vec<_>>(),
    })
}

fn palette_titles(ui: &AppWindow) -> Vec<Value> {
    use slint::Model;
    ui.get_palette_items()
        .iter()
        .map(|item| {
            let title: String = item.parts.iter().map(|p| p.text.to_string()).collect();
            let matched: String = item.parts.iter().filter(|p| p.matched).map(|p| p.text.to_string()).collect();
            json!({"title": title, "matched": matched, "trailing": item.trailing.as_str()})
        })
        .collect()
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
    let view: Vec<Value> = [ui.get_view_menu(), ui.get_view_menu_2(), ui.get_view_menu_3()].into_iter().flat_map(menu).collect();
    json!({
        "File": file,
        "Open Recent": menu(ui.get_recent_menu()),
        "Reopen with Encoding": menu(ui.get_reopen_menu()),
        "Convert to Encoding": menu(ui.get_convert_menu()),
        "Line Endings": menu(ui.get_line_ending_menu()),
        "Edit": menu(ui.get_edit_menu()),
        "Selection": menu(ui.get_selection_menu()),
        "Find": menu(ui.get_find_menu()),
        "Text": menu(ui.get_text_menu()),
        "Lines": menu(ui.get_lines_menu()),
        "Case": menu(ui.get_case_menu()),
        "Whitespace": menu(ui.get_whitespace_menu()),
        "View": view,
        "Code Folding": menu(ui.get_folding_menu()),
        "Language": menu(ui.get_language_menu()),
        "Tools": menu(ui.get_tools_menu()),
        "Compare": menu(ui.get_compare_menu()),
        "Compare With": menu(ui.get_compare_documents_menu()),
        "JSON": menu(ui.get_json_menu()),
        "XML": menu(ui.get_xml_menu()),
        "CSV": menu(ui.get_csv_menu()),
        "Encode": menu(ui.get_encode_menu()),
        "Decode": menu(ui.get_decode_menu()),
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
