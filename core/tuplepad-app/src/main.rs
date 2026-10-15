//! TuplePad: a fast, beautiful text editor for Mac, Windows and Linux.
//!
//! `tuplepad-app [files…] [--dark|--light] [--renderer skia|femtovg] [--size WxH]
//!           [--script steps.json] [--session-dir DIR] [--set key=value]…`

// The scripted runs' state dump is one large `json!`.
#![recursion_limit = "256"]

slint::include_modules!();

mod app;
mod column;
mod commands;
mod completion;
mod document;
mod editor;
mod find;
mod fonts;
mod outline;
mod palette;
mod platform;
mod print;
mod script;
mod search_panel;
mod session;
mod settings;
mod settings_window;
mod tools;

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, SharedString};
use tuplepad_core::SessionStore;

use app::App;
use settings::Settings;

struct Options {
    files: Vec<PathBuf>,
    appearance: Option<&'static str>,
    renderer: Option<String>,
    size: Option<(f32, f32)>,
    script: Option<String>,
    session_dir: Option<PathBuf>,
    overrides: Vec<String>,
}

fn parse_args() -> Options {
    let mut options =
        Options { files: Vec::new(), appearance: None, renderer: None, size: None, script: None, session_dir: None, overrides: Vec::new() };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dark" => options.appearance = Some("dark"),
            "--light" => options.appearance = Some("light"),
            "--renderer" => options.renderer = args.next(),
            "--script" => options.script = args.next(),
            "--session-dir" => options.session_dir = args.next().map(PathBuf::from),
            "--set" => options.overrides.extend(args.next()),
            "--size" => {
                options.size = args.next().and_then(|s| {
                    let (w, h) = s.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                })
            }
            // macOS adds this when an app is opened from Finder.
            _ if arg.starts_with("-psn_") => {}
            _ => options.files.push(PathBuf::from(arg)),
        }
    }
    options
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_args();
    let scripted = options.script.is_some();

    // Files and links on the command line: if TuplePad is already running, it takes them.
    let links: Vec<String> = options
        .files
        .iter()
        .map(|f| {
            let text = f.to_string_lossy();
            if text.starts_with("tuplepad://") { text.into_owned() } else { platform::absolute(f).to_string_lossy().into_owned() }
        })
        .collect();
    if !scripted && platform::send_to_running(&links) {
        return Ok(());
    }

    let mut backend = slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_custom_application_handler(platform::WindowEvents);
    if let Some(renderer) = &options.renderer {
        backend = backend.renderer_name(renderer.clone());
    }
    #[cfg(target_os = "macos")]
    {
        use slint::winit_030::winit::platform::macos::WindowAttributesExtMacOS;
        backend = backend.with_winit_window_attributes_hook(|attributes| {
            attributes.with_titlebar_transparent(true).with_fullsize_content_view(true).with_title_hidden(true)
        });
    }
    backend.select()?;
    fonts::register_system_fonts();

    // Settings and the session live in the app's data folder; scripted runs use neither unless
    // given a session folder of their own.
    let data = settings::data_folder().join("Slint");
    let settings_path = (!scripted).then(|| data.join("settings.json"));
    let mut settings = settings_path.as_deref().map(Settings::load).unwrap_or_default();
    let unknown = settings.apply_overrides(&options.overrides);
    if !unknown.is_empty() {
        eprintln!("unknown settings: {unknown:?}");
    }
    if let Some(appearance) = options.appearance {
        settings.appearance = appearance.into();
    }
    let session_folder = match &options.session_dir {
        Some(folder) => Some(folder.clone()),
        None if scripted => None,
        None => Some(data.join(if cfg!(debug_assertions) { "Session-Debug" } else { "Session" })),
    };

    let ui = AppWindow::new()?;
    if cfg!(target_os = "macos") {
        ui.set_titlebar_height(28.0);
    }
    if let Some((width, height)) = options.size {
        ui.window().set_size(slint::LogicalSize::new(width, height));
    }

    let app = App::new(&ui, settings, settings_path, session_folder.map(SessionStore::new), scripted);
    platform::set_app(&app);
    platform::install_open_handler();
    // Scripted runs listen only on a socket of their own (TUPLEPAD_SOCKET), for tests.
    if !scripted || std::env::var_os("TUPLEPAD_SOCKET").is_some() {
        platform::listen();
    }
    if let Some(store) = &app.session {
        let count = session::restore(store, &mut app.editor.borrow_mut());
        app.untitled_count.set(count);
    }
    let requests: Vec<_> = links.iter().filter_map(|link| platform::request_for(link)).collect();
    for request in requests {
        match request {
            platform::Request::Open { path, line, column } => app.open_requests(vec![(path, line, column)]),
            platform::Request::NewWithText(text) => app.new_document_with_text(&text),
            _ => {}
        }
    }
    if app.editor.borrow().docs.is_empty() {
        app.new_document();
    }
    connect(&ui, &app);
    app.refresh();

    // Caret blinking, scrollbar fading and smart highlighting.
    let ticker = slint::Timer::default();
    {
        let app = app.clone();
        ticker.start(slint::TimerMode::Repeated, Duration::from_millis(60), move || app.tick());
    }
    // Open files are checked for changes made by other apps every second.
    let watcher = slint::Timer::default();
    {
        let app = app.clone();
        watcher.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || app.check_disk());
    }
    // The session is saved every two seconds, and on quit.
    let saver = slint::Timer::default();
    if app.session.is_some() {
        let app = app.clone();
        saver.start(slint::TimerMode::Repeated, Duration::from_secs(2), move || {
            app.save_session();
            app.save_settings();
        });
    }

    if let Some(path) = &options.script {
        script::start(app.clone(), path)?;
    }
    ui.run()?;
    app.shut_down();
    Ok(())
}

/// Wires the window's callbacks to the app.
fn connect(ui: &AppWindow, app: &Rc<App>) {
    let bridge = ui.global::<EditorBridge>();

    // Runs `f` on the editor with a pane's view in place, then redraws.
    let edit = {
        let app = app.clone();
        Rc::new(move |pane: i32, f: &dyn Fn(&mut editor::Editor)| {
            if let Ok(mut editor) = app.editor.try_borrow_mut() {
                editor.with_pane(pane.max(0) as usize, |e| f(e));
            }
            app.refresh();
        })
    };

    {
        let app = app.clone();
        bridge.on_key(move |pane, event| {
            app.focus_pane(pane.max(0) as usize);
            let m = event.modifiers;
            app.key(&event.text, m.control, m.alt, m.shift, m.meta)
        });
    }
    {
        let app = app.clone();
        bridge.on_text(move |pane, text: SharedString| {
            app.focus_pane(pane.max(0) as usize);
            app.typed(&text);
        });
    }
    {
        let edit = edit.clone();
        bridge.on_minimap_pointer(move |pane, y| edit(pane, &|e| e.minimap_scroll(y)));
    }
    {
        let app = app.clone();
        bridge.on_completion_chosen(move |_, index| app.accept_completion(index.max(0) as usize));
    }
    {
        let app = app.clone();
        bridge.on_preedit(move |pane, text: SharedString| {
            app.focus_pane(pane.max(0) as usize);
            if let Ok(mut editor) = app.editor.try_borrow_mut() {
                editor.set_preedit(&text);
            }
            app.refresh();
        });
    }
    {
        let app = app.clone();
        bridge.on_pointer_down(move |pane, x, y, m| {
            app.focus_pane(pane.max(0) as usize);
            if let Ok(mut editor) = app.editor.try_borrow_mut() {
                editor.completion = None;
                editor.pointer_down(x, y, m.control, m.alt, m.shift);
            }
            app.refresh();
        });
    }
    {
        let edit = edit.clone();
        bridge.on_pointer_drag(move |pane, x, y| edit(pane, &|e| e.pointer_drag(x, y)));
    }
    {
        let edit = edit.clone();
        bridge.on_pointer_up(move |pane, m| edit(pane, &|e| e.pointer_up(m.alt)));
    }
    {
        let edit = edit.clone();
        bridge.on_scrolled(move |pane, dx, dy| edit(pane, &|e| e.scroll_by(dx, dy)));
    }
    {
        let edit = edit.clone();
        bridge.on_scrollbar_drag(move |pane, fraction| edit(pane, &|e| e.scroll_to_fraction(fraction)));
    }
    {
        let edit = edit.clone();
        bridge.on_scrollbar_hover(move |pane, inside| edit(pane, &|e| e.set_scrollbar_hover(inside)));
    }
    {
        let app = app.clone();
        bridge.on_focus_changed(move |pane, focused| {
            if focused {
                app.focus_pane(pane.max(0) as usize);
            }
            if let Ok(mut editor) = app.editor.try_borrow_mut() {
                editor.touch();
            }
            app.refresh();
        });
    }
    {
        let edit = edit.clone();
        let weak = ui.as_weak();
        bridge.on_resized(move |pane, width, height| {
            let scale = weak.upgrade().map_or(1.0, |ui| ui.window().scale_factor());
            edit(pane, &|e| e.resize(width, height, scale));
        });
    }
    {
        let app = app.clone();
        ui.on_command(move |id| app.run_command(&id));
    }
    {
        // Closing the main window closes Settings too, so the app quits.
        let app = app.clone();
        ui.window().on_close_requested(move || {
            app.hide_settings();
            slint::CloseRequestResponse::HideWindow
        });
    }
    {
        let app = app.clone();
        ui.on_select_doc(move |index| app.select_document(index as usize));
    }
    {
        let app = app.clone();
        ui.on_palette_query(move |query| app.palette_query(&query));
    }
    {
        let find = ui.global::<FindBridge>();
        let a = app.clone();
        find.on_changed(move || a.find_changed());
        let a = app.clone();
        find.on_next(move || a.find_step(true));
        let a = app.clone();
        find.on_previous(move || a.find_step(false));
        let a = app.clone();
        find.on_replace(move || a.replace_one());
        let a = app.clone();
        find.on_replace_all(move || a.replace_all_matches());
        let a = app.clone();
        find.on_close(move || a.close_find());
        let a = app.clone();
        find.on_action(move |id| a.run_command(&id));
    }
    {
        let app = app.clone();
        bridge.on_gutter_clicked(move |pane, row| app.toggle_bookmark_at_row(pane.max(0) as usize, row.max(0) as u64));
    }
    {
        let app = app.clone();
        bridge.on_fold_clicked(move |pane, row| {
            app.editor.borrow_mut().with_pane(pane.max(0) as usize, |e| e.toggle_fold_at_row(row.max(0) as u64));
            app.refresh();
        });
    }
    {
        let app = app.clone();
        ui.global::<OutlineBridge>().on_activate(move |index| app.activate_outline_item(index.max(0) as usize));
    }
    {
        let column = ui.global::<ColumnBridge>();
        let a = app.clone();
        column.on_changed(move || a.column_changed());
        let a = app.clone();
        column.on_accept(move || a.accept_column_editor());
        let a = app.clone();
        column.on_cancel(move || a.close_column_editor());
    }
    {
        let search = ui.global::<SearchBridge>();
        let a = app.clone();
        search.on_start(move || a.start_search());
        let a = app.clone();
        search.on_stop(move || a.stop_search());
        let a = app.clone();
        search.on_close(move || a.hide_search());
        let a = app.clone();
        search.on_scope_changed(move || a.search_scope_changed());
        let a = app.clone();
        search.on_choose_folder(move || a.choose_search_folder());
        let a = app.clone();
        search.on_pick_folder(move |i| a.pick_search_folder(i.max(0) as usize));
        let a = app.clone();
        search.on_activate(move |i, how| a.activate_search_row(i, how));
        let a = app.clone();
        search.on_prepare_replace(move || a.prepare_replace());
        let a = app.clone();
        search.on_apply_plan(move || a.apply_plan());
        let a = app.clone();
        search.on_cancel_plan(move || a.cancel_plan());
        let a = app.clone();
        search.on_plan_toggle(move |i| a.toggle_plan_file(i));
        let a = app.clone();
        search.on_undo(move |i| a.undo_replaced(i.max(0) as usize));
        let a = app.clone();
        search.on_undo_all(move || a.undo_all_replaced());
        let a = app.clone();
        search.on_resized(move || a.search_panel_resized());
    }
    {
        let app = app.clone();
        ui.on_palette_run(move |index| app.palette_run(index.max(0) as usize));
    }
    {
        let app = app.clone();
        ui.on_palette_close(move || app.close_palette());
    }
    {
        let app = app.clone();
        ui.on_sidebar_resized(move |width| app.set_sidebar_width(width));
    }
    {
        let weak = ui.as_weak();
        ui.on_drag_window(move || {
            use slint::winit_030::WinitWindowAccessor;
            if let Some(ui) = weak.upgrade() {
                ui.window().with_winit_window(|window| {
                    let _ = window.drag_window();
                });
            }
        });
    }
}
