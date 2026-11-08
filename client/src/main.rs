//! SCRN desktop client.
//!
//! A tray icon with a menu, `scrn capture` as command (a second start hands
//! the command to the running instance), capture through the screenshot
//! portal, a fullscreen overlay per monitor to select an area and an editor
//! for arrows, highlights and blurred areas. The finished image is
//! uploaded to the SCRN server and its link copied (`scrn copy-last` copies
//! it again); without a server it is saved under `~/Pictures/SCRN`.
//! Observations go to the client log (see [`report`]).

mod api;
mod capture;
mod config;
mod desktop;
mod history;
mod i18n;
mod paths;
#[cfg(target_os = "linux")]
mod permission;
mod report;
mod secret;
#[cfg(windows)]
mod shortcut;

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Serialize;
use tauri::http::{Response, StatusCode, header::CONTENT_TYPE};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use capture::{Capture, Rect};
use config::Settings;
use i18n::{fill, t};

/// Overlay windows are labelled `overlay-<monitor index>`, one per monitor.
const OVERLAY: &str = "overlay-";

/// Time the tray menu needs to close before the screen is captured.
const MENU_CLOSE_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

/// The editor window.
const EDITOR: &str = "editor";

/// The settings window.
const SETTINGS: &str = "settings";
/// Label of the window that asks for the screenshot permission.
#[cfg(target_os = "linux")]
const PERMISSION: &str = "permission";

/// Toolbar height and margins of the editor window in logical pixels.
const EDITOR_CHROME: (f64, f64) = (32.0, 96.0);

#[derive(Default)]
struct AppState {
    /// The full screen while the overlays are open.
    capture: Mutex<Option<Capture>>,
    /// The selected area while the editor is open.
    edit: Mutex<Option<Capture>>,
    settings: Mutex<Settings>,
    history: Mutex<Vec<history::Entry>>,
    /// Uploads in flight; the tray shows the empty mark meanwhile.
    uploads: AtomicUsize,
    /// Why the keyboard shortcut could not be registered, if it failed.
    shortcut_error: Mutex<Option<String>>,
}

fn lock<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

const USAGE: &str = "usage: scrn [capture | copy-last]

  scrn             start the tray icon
  scrn capture     take a screenshot and select an area
  scrn copy-last   copy the link of the last upload

Both commands hand over to the running instance, if there is one.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Tray,
    Capture,
    CopyLast,
}

fn parse_command(arg: Option<&str>) -> Option<Command> {
    match arg {
        None => Some(Command::Tray),
        Some("capture") => Some(Command::Capture),
        Some("copy-last") => Some(Command::CopyLast),
        Some(_) => None,
    }
}

fn run_command(app: &AppHandle, command: Command, source: &'static str) {
    match command {
        Command::Tray => {}
        Command::Capture => start_capture(app, source),
        Command::CopyLast => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { copy_last(&app).await });
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return;
    }
    let Some(command) = parse_command(args.first().map(String::as_str)) else {
        eprintln!("unknown command {:?}\n\n{USAGE}", args[0]);
        std::process::exit(2);
    };
    report::environment();
    let settings = config::load().unwrap_or_else(|err| {
        report::note(format!(
            "loading the settings failed, using defaults: {err}"
        ));
        Settings::default()
    });
    let state = AppState {
        settings: Mutex::new(settings),
        history: Mutex::new(history::load()),
        ..AppState::default()
    };

    let builder = tauri::Builder::default();
    #[cfg(windows)]
    let builder = builder.plugin(shortcut::plugin());
    let result = builder
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            report::note(format!("second start handed over: args={:?}", &argv[1..]));
            match parse_command(argv.get(1).map(String::as_str)) {
                Some(command) => run_command(app, command, "command (running instance)"),
                None => report::note("unknown command handed over, ignored"),
            }
        }))
        .manage(state)
        .register_uri_scheme_protocol("scrn", |ctx, request| {
            serve(ctx.app_handle(), request.uri().path())
        })
        .invoke_handler(tauri::generate_handler![
            overlay_info,
            finish_selection,
            cancel_selection,
            edit_image,
            save_edit,
            discard_edit,
            editor_config,
            get_settings,
            save_settings,
            test_connection,
            forget_token,
            allow_screenshots,
            note
        ])
        .setup(move |app| {
            build_tray(app.handle())?;
            log_monitors(app.handle());
            #[cfg(windows)]
            shortcut::apply(app.handle());
            run_command(app.handle(), command, "command");
            Ok(())
        })
        .build(tauri::generate_context!());
    let app = match result {
        Ok(app) => app,
        Err(err) => {
            report::note(format!("startup failed: {err}"));
            std::process::exit(1);
        }
    };
    app.run(|_, event| {
        // Keep running in the tray when the overlay closes.
        if let RunEvent::ExitRequested { api, code, .. } = event
            && code.is_none()
        {
            api.prevent_exit();
        }
    });
}

/// The tray icons, normal and while an upload runs (the brackets without
/// the bars). GNOME's top bar is dark in both themes, so Linux takes the
/// green for dark backgrounds; Windows follows the theme of the taskbar;
/// macOS colors a black template itself.
#[cfg(target_os = "linux")]
fn tray_icons() -> (&'static [u8], &'static [u8]) {
    (
        include_bytes!("../icons/tray/tray-on-dark@2x.png"),
        include_bytes!("../icons/tray/empty/tray-on-dark@2x.png"),
    )
}

#[cfg(windows)]
fn tray_icons() -> (&'static [u8], &'static [u8]) {
    if desktop::taskbar_is_light() {
        (
            include_bytes!("../icons/tray/tray-on-light@2x.png"),
            include_bytes!("../icons/tray/empty/tray-on-light@2x.png"),
        )
    } else {
        (
            include_bytes!("../icons/tray/tray-on-dark@2x.png"),
            include_bytes!("../icons/tray/empty/tray-on-dark@2x.png"),
        )
    }
}

#[cfg(target_os = "macos")]
fn tray_icons() -> (&'static [u8], &'static [u8]) {
    (
        include_bytes!("../icons/tray/tray-template@2x.png"),
        include_bytes!("../icons/tray/empty/tray-template@2x.png"),
    )
}

const TRAY: &str = "main";
/// Menu IDs of the recent links: `recent-<index in the history>`.
const RECENT: &str = "recent-";

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = tray_menu(app)?;
    let icon = tauri::image::Image::from_bytes(tray_icons().0)?;
    TrayIconBuilder::with_id(TRAY)
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("SCRN")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .build(app)?;
    report::note("tray icon created");
    Ok(())
}

fn tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let state = app.state::<AppState>();
    let configured = lock(&state.settings).is_configured();
    let entries = lock(&state.history).clone();
    let now = now();
    let item = |id: &str, key: &str, enabled: bool| {
        MenuItem::with_id(app, id, t(key), enabled, None::<&str>)
    };
    let recent = Submenu::with_id(app, "recent", t("tray.recent"), true)?;
    if entries.is_empty() {
        recent.append(&item("recent-empty", "tray.recent_empty", false)?)?;
    }
    for (index, entry) in entries.iter().enumerate() {
        recent.append(&MenuItem::with_id(
            app,
            format!("{RECENT}{index}"),
            recent_label(entry, now),
            true,
            None::<&str>,
        )?)?;
    }
    Menu::with_items(
        app,
        &[
            &item("capture", "tray.capture", true)?,
            &item(
                "copy_last",
                "tray.copy_last",
                configured || !entries.is_empty(),
            )?,
            &recent,
            &item("gallery", "tray.gallery", configured)?,
            &PredefinedMenuItem::separator(app)?,
            &item("settings", "tray.settings", true)?,
            &item("quit", "tray.quit", true)?,
        ],
    )
}

/// `AbCd1234 · vor 5 Min.` for a public link, `privat · …` otherwise.
fn recent_label(entry: &history::Entry, now: i64) -> String {
    let name = if entry.public {
        entry.url.rsplit('/').next().unwrap_or(&entry.url)
    } else {
        t("tray.private")
    };
    format!(
        "{name} · {}",
        i18n::age(i18n::lang(), entry.created_at, now)
    )
}

fn refresh_tray(app: &AppHandle) {
    let result = tray_menu(app).and_then(|menu| match app.tray_by_id(TRAY) {
        Some(tray) => tray.set_menu(Some(menu)),
        None => Ok(()),
    });
    if let Err(err) = result {
        report::note(format!("updating the tray menu failed: {err}"));
    }
}

fn set_busy(app: &AppHandle, busy: bool) {
    let state = app.state::<AppState>();
    let running = if busy {
        state.uploads.fetch_add(1, Ordering::SeqCst) + 1
    } else {
        state
            .uploads
            .fetch_sub(1, Ordering::SeqCst)
            .saturating_sub(1)
    };
    let (normal, busy) = tray_icons();
    let icon = if running > 0 { busy } else { normal };
    if let (Some(tray), Ok(image)) = (app.tray_by_id(TRAY), tauri::image::Image::from_bytes(icon)) {
        let _ = tray.set_icon(Some(image));
        let _ = tray.set_icon_as_template(cfg!(target_os = "macos"));
    }
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "capture" => start_capture(app, "tray menu"),
        "copy_last" => run_command(app, Command::CopyLast, "tray menu"),
        "gallery" => {
            let base = lock(&app.state::<AppState>().settings).server_url.clone();
            if !base.is_empty() {
                desktop::open_url(app, format!("{base}/app"));
            }
        }
        "settings" => {
            if let Err(err) = open_settings(app) {
                report::note(format!("opening the settings failed: {err}"));
            }
        }
        "quit" => {
            report::note("quit from the tray menu");
            app.exit(0);
        }
        other => {
            let entry = other
                .strip_prefix(RECENT)
                .and_then(|i| i.parse::<usize>().ok())
                .and_then(|i| lock(&app.state::<AppState>().history).get(i).cloned());
            if let Some(entry) = entry {
                use_link(app, &entry);
            }
        }
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

// --- Links --------------------------------------------------------------

/// Copies a public link; private screenshots have none, so their page in
/// the gallery opens instead.
fn use_link(app: &AppHandle, entry: &history::Entry) {
    if entry.public {
        desktop::copy_text(app, entry.url.clone());
        desktop::notify(t("notify.copied"), &entry.url);
    } else {
        desktop::open_url(app, entry.url.clone());
    }
}

fn remember(app: &AppHandle, entry: history::Entry) {
    let entries = {
        let state = app.state::<AppState>();
        let mut entries = lock(&state.history);
        history::push(&mut entries, entry);
        entries.clone()
    };
    if let Err(err) = history::save(&entries) {
        report::note(format!("saving the history failed: {err}"));
    }
    refresh_tray(app);
}

fn entry_of(base: &str, info: scrn_proto::ScreenshotInfo) -> history::Entry {
    let public = info.short_url.is_some();
    history::Entry {
        url: info
            .short_url
            .unwrap_or_else(|| api::Api::gallery_url(base, &info.id)),
        id: info.id,
        public,
        created_at: info.created_at,
    }
}

/// Runs blocking work (the keyring talks D-Bus synchronously) off the
/// async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

/// The API client for the configured server, or `None` if there is no
/// server or no token.
async fn client(app: &AppHandle) -> Result<Option<(api::Api, Settings)>, String> {
    let settings = lock(&app.state::<AppState>().settings).clone();
    if !settings.is_configured() {
        return Ok(None);
    }
    let url = settings.server_url.clone();
    let token = blocking(move || secret::token(&url)).await?.map_err(|e| {
        report::note(format!("reading the token failed: {e}"));
        t("error.keyring").to_owned()
    })?;
    match token {
        Some(token) => Ok(Some((
            api::Api::new(&settings.server_url, &token).map_err(|e| e.to_string())?,
            settings,
        ))),
        None => Ok(None),
    }
}

/// `scrn copy-last`: the newest link of the history, or of the server when
/// the history is empty.
async fn copy_last(app: &AppHandle) {
    let first = lock(&app.state::<AppState>().history).first().cloned();
    if let Some(entry) = first {
        use_link(app, &entry);
        return;
    }
    let (api, settings) = match client(app).await {
        Ok(Some(client)) => client,
        Ok(None) => {
            desktop::notify(t("notify.not_configured"), t("notify.not_configured_body"));
            return;
        }
        Err(message) => {
            desktop::notify(t("notify.no_links"), &message);
            return;
        }
    };
    match api.latest().await {
        Ok(Some(info)) => {
            let entry = entry_of(&settings.server_url, info);
            remember(app, entry.clone());
            use_link(app, &entry);
        }
        Ok(None) => desktop::notify(t("notify.no_links"), t("notify.no_links_body")),
        Err(err) => {
            report::note(format!("asking for the last screenshot failed: {err}"));
            desktop::notify(t("notify.no_links"), err.describe());
        }
    }
}

/// Uploads the finished image and copies its link. Without a server, or
/// when the upload fails, the image is saved locally so nothing is lost.
async fn deliver(app: AppHandle, png: Vec<u8>) {
    let save_local = |png: &[u8]| match capture::save(png) {
        Ok(path) => {
            report::note(format!("saved to {} ({} bytes)", path.display(), png.len()));
            Some(path)
        }
        Err(err) => {
            report::note(format!("saving failed: {err}"));
            None
        }
    };
    let kept = |path: Option<std::path::PathBuf>| {
        path.map_or_else(String::new, |p| {
            fill(t("notify.kept"), &p.display().to_string())
        })
    };
    let (api, settings) = match client(&app).await {
        Ok(Some(client)) => client,
        Ok(None) => {
            let path = save_local(&png);
            desktop::notify(t("notify.saved"), &kept(path));
            return;
        }
        Err(message) => {
            let path = save_local(&png);
            desktop::notify(
                t("notify.upload_failed"),
                &format!("{message} {}", kept(path)),
            );
            return;
        }
    };
    set_busy(&app, true);
    let started = std::time::Instant::now();
    let size = png.len();
    let copy = settings.keep_copy.then(|| png.clone());
    let result = api
        .upload(png.clone(), settings.visibility, settings.expires_in_days)
        .await;
    set_busy(&app, false);
    match result {
        Ok(info) => {
            report::note(format!(
                "uploaded {size} bytes in {} ms ({})",
                started.elapsed().as_millis(),
                if info.short_url.is_some() {
                    "public"
                } else {
                    "private"
                }
            ));
            let entry = entry_of(&settings.server_url, info);
            remember(&app, entry.clone());
            if entry.public {
                use_link(&app, &entry);
            } else {
                desktop::notify(t("notify.private"), t("notify.private_body"));
            }
            if let Some(copy) = copy {
                save_local(&copy);
            }
        }
        Err(err) => {
            report::note(format!("upload failed: {err}"));
            let path = save_local(&png);
            desktop::notify(
                t("notify.upload_failed"),
                &format!("{} {}", err.describe(), kept(path)),
            );
        }
    }
}

fn log_monitors(app: &AppHandle) {
    match app.available_monitors() {
        Ok(monitors) => {
            for m in monitors {
                report::note(format!(
                    "monitor {:?}: position={:?} size={:?} scale={}",
                    m.name(),
                    m.position(),
                    m.size(),
                    m.scale_factor()
                ));
            }
        }
        Err(err) => report::note(format!("listing monitors failed: {err}")),
    }
}

/// Takes a screenshot and opens the overlay. `source` says what asked.
fn start_capture(app: &AppHandle, source: &'static str) {
    report::note(format!("capture requested by {source}"));
    let busy = overlays(app)
        .into_iter()
        .next()
        .or_else(|| app.get_webview_window(EDITOR));
    if let Some(window) = busy {
        let _ = window.set_focus();
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if matches!(source, "tray menu" | "permission window") {
            // Otherwise the closing menu or window ends up in the screenshot.
            tokio::time::sleep(MENU_CLOSE_DELAY).await;
        }
        let started = std::time::Instant::now();
        match capture::take().await {
            Ok((shot, how)) => {
                report::note(format!(
                    "screenshot: {}x{} px, {} bytes, {} ms, {how}",
                    shot.width,
                    shot.height,
                    shot.png.len(),
                    started.elapsed().as_millis(),
                ));
                *lock(&app.state::<AppState>().capture) = Some(shot);
                if let Err(err) = open_overlays(&app) {
                    report::note(format!("opening the overlay failed: {err}"));
                    close_overlays(&app);
                }
            }
            Err(err) => {
                report::note(format!("capture failed: {err}"));
                capture_failed(&app, &err).await;
            }
        }
    });
}

/// Explains a failed capture. A refusal of the portal while the client runs
/// as an app without the stored permission opens the permission window;
/// anything else becomes a notification.
async fn capture_failed(app: &AppHandle, err: &capture::CaptureError) {
    #[cfg(target_os = "linux")]
    if err.is_cancelled() {
        return;
    }
    #[cfg(target_os = "linux")]
    if err.is_portal()
        && let Some(id) = permission::app_id()
    {
        let status = permission::status(&id).await;
        report::note(format!("screenshot permission of app {id}: {status:?}"));
        if status != permission::Permission::Granted {
            if let Err(err) = open_permission(app) {
                report::note(format!("opening the permission window failed: {err}"));
            }
            return;
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = app;
    desktop::notify(t("notify.capture_failed"), &err.to_string());
}

#[cfg(target_os = "linux")]
fn open_permission(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(PERMISSION) {
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, PERMISSION, WebviewUrl::App("permission.html".into()))
        .title("SCRN")
        .inner_size(480.0, 360.0)
        .resizable(false)
        .center()
        .focused(true)
        .build()?;
    Ok(())
}

/// Stores the screenshot permission after the user agreed in the
/// permission window, then takes the screenshot that failed.
#[tauri::command]
#[cfg(target_os = "linux")]
async fn allow_screenshots(app: AppHandle) -> Result<(), String> {
    let id = permission::app_id().ok_or_else(|| t("permission.no_app").to_owned())?;
    permission::grant(&id).await.map_err(|err| {
        report::note(format!("storing the screenshot permission failed: {err}"));
        err.to_string()
    })?;
    report::note(format!("screenshot permission stored for app {id}"));
    if let Some(window) = app.get_webview_window(PERMISSION) {
        let _ = window.close();
    }
    start_capture(&app, "permission window");
    Ok(())
}

/// Only Linux asks for a stored screenshot permission.
#[tauri::command]
#[cfg(not(target_os = "linux"))]
fn allow_screenshots() -> Result<(), String> {
    Err("not needed on this system".into())
}

/// Opens one overlay per monitor, each fullscreen on its own monitor.
fn open_overlays(app: &AppHandle) -> tauri::Result<()> {
    let monitors = app.available_monitors()?;
    for index in 0..monitors.len().max(1) {
        let window = WebviewWindowBuilder::new(
            app,
            format!("{OVERLAY}{index}"),
            WebviewUrl::App("overlay.html".into()),
        )
        .title("SCRN")
        .decorations(false)
        .always_on_top(true)
        .resizable(false)
        .skip_taskbar(true)
        .visible(false)
        .build()?;
        show_overlay(app, window, index, monitors.get(index))?;
    }
    Ok(())
}

/// Wayland ignores window positions, so GTK puts the window fullscreen on
/// its monitor (`gtk_window_fullscreen_on_monitor`).
#[cfg(target_os = "linux")]
fn show_overlay(
    app: &AppHandle,
    window: WebviewWindow,
    index: usize,
    _monitor: Option<&tauri::Monitor>,
) -> tauri::Result<()> {
    use gtk::prelude::GtkWindowExt;
    let Ok(monitor) = i32::try_from(index) else {
        return Ok(());
    };
    app.run_on_main_thread(move || {
        let gtk_window = match window.gtk_window() {
            Ok(w) => w,
            Err(err) => {
                report::note(format!("overlay {monitor}: no GTK window: {err}"));
                return;
            }
        };
        let Some(screen) = gtk::gdk::Screen::default() else {
            report::note("no GDK screen");
            return;
        };
        gtk_window.fullscreen_on_monitor(&screen, monitor);
        let _ = window.show();
        if monitor == 0 {
            let _ = window.set_focus();
        }
    })
}

/// Elsewhere the window moves onto its monitor first; fullscreen then
/// covers the monitor the window is on.
#[cfg(not(target_os = "linux"))]
fn show_overlay(
    _app: &AppHandle,
    window: WebviewWindow,
    index: usize,
    monitor: Option<&tauri::Monitor>,
) -> tauri::Result<()> {
    if let Some(m) = monitor {
        window.set_position(*m.position())?;
        window.set_size(*m.size())?;
    }
    window.set_fullscreen(true)?;
    window.show()?;
    if index == 0 {
        window.set_focus()?;
    }
    Ok(())
}

fn overlays(app: &AppHandle) -> Vec<WebviewWindow> {
    app.webview_windows()
        .into_iter()
        .filter(|(label, _)| label.starts_with(OVERLAY))
        .map(|(_, window)| window)
        .collect()
}

fn close_overlays(app: &AppHandle) {
    for window in overlays(app) {
        let _ = window.close();
    }
}

/// Serves the current capture to the overlay as `scrn://localhost/capture.png`.
fn serve(app: &AppHandle, path: &str) -> Response<Vec<u8>> {
    let png = (path == "/capture.png")
        .then(|| {
            lock(&app.state::<AppState>().capture)
                .as_ref()
                .map(|c| c.png.clone())
        })
        .flatten();
    let response = match png {
        Some(png) => Response::builder()
            .header(CONTENT_TYPE, "image/png")
            .body(png),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new()),
    };
    response.unwrap_or_else(|_| Response::new(Vec::new()))
}

#[derive(Clone, Serialize)]
struct MonitorInfo {
    name: Option<String>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
}

#[derive(Serialize)]
struct OverlayInfo {
    image_width: u32,
    image_height: u32,
    /// The part of the image that shows the overlay's monitor.
    source: Rect,
    monitor: Option<MonitorInfo>,
    monitors: Vec<MonitorInfo>,
}

fn monitor_info(m: &tauri::Monitor) -> MonitorInfo {
    MonitorInfo {
        name: m.name().cloned(),
        x: m.position().x,
        y: m.position().y,
        width: m.size().width,
        height: m.size().height,
        scale: m.scale_factor(),
    }
}

/// Image size and the part of the image the overlay should show. The
/// portal image covers the bounding box of all monitors; each overlay
/// shows the part of its own monitor, found by the index in its label.
#[tauri::command]
fn overlay_info(
    window: WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<OverlayInfo, String> {
    let (image_width, image_height) = lock(&state.capture)
        .as_ref()
        .map(|c| (c.width, c.height))
        .ok_or("no capture")?;
    let monitors: Vec<MonitorInfo> = window
        .available_monitors()
        .map_err(|e| e.to_string())?
        .iter()
        .map(monitor_info)
        .collect();
    let index = window
        .label()
        .strip_prefix(OVERLAY)
        .and_then(|i| i.parse::<usize>().ok())
        .unwrap_or(0);
    let source = source_rect(&monitors, index, image_width, image_height);
    report::note(format!(
        "overlay {index}: window inner={:?} scale={:?} current monitor={:?} source={source:?}",
        window.inner_size().ok(),
        window.scale_factor().ok(),
        window
            .current_monitor()
            .ok()
            .flatten()
            .map(|m| m.name().cloned()),
    ));
    Ok(OverlayInfo {
        image_width,
        image_height,
        source,
        monitor: monitors.get(index).cloned(),
        monitors,
    })
}

/// The part of the portal image that shows monitor `index`.
fn source_rect(monitors: &[MonitorInfo], index: usize, width: u32, height: u32) -> Rect {
    let whole = Rect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let Some(m) = monitors.get(index).filter(|_| monitors.len() > 1) else {
        return whole;
    };
    let left = monitors.iter().map(|m| m.x).min().unwrap_or(0);
    let top = monitors.iter().map(|m| m.y).min().unwrap_or(0);
    let right = monitors
        .iter()
        .map(|m| m.x + m.width as i32)
        .max()
        .unwrap_or(1);
    let bottom = monitors
        .iter()
        .map(|m| m.y + m.height as i32)
        .max()
        .unwrap_or(1);
    let fx = f64::from(width) / f64::from((right - left).max(1));
    let fy = f64::from(height) / f64::from((bottom - top).max(1));
    let scale = |v: i32, f: f64| (f64::from(v) * f).round().max(0.0) as u32;
    let x = scale(m.x - left, fx).min(width.saturating_sub(1));
    let y = scale(m.y - top, fy).min(height.saturating_sub(1));
    Rect {
        x,
        y,
        width: scale(m.width as i32, fx).clamp(1, width - x),
        height: scale(m.height as i32, fy).clamp(1, height - y),
    }
}

/// Crops the selection, closes the overlays and opens the editor.
#[tauri::command]
async fn finish_selection(
    window: WebviewWindow,
    state: tauri::State<'_, AppState>,
    rect: Rect,
) -> Result<(), String> {
    let png = {
        let guard = lock(&state.capture);
        let capture = guard.as_ref().ok_or("no capture")?;
        capture::crop(&capture.png, rect).map_err(|e| e.to_string())?
    };
    report::note(format!("selection {rect:?}"));
    let monitor = window.current_monitor().ok().flatten();
    *lock(&state.capture) = None;
    *lock(&state.edit) = Some(Capture {
        png,
        width: rect.width,
        height: rect.height,
    });
    let app = window.app_handle().clone();
    close_overlays(&app);
    if let Err(err) = open_editor(&app, monitor.as_ref(), rect) {
        report::note(format!("opening the editor failed: {err}"));
        *lock(&state.edit) = None;
        return Err(err.to_string());
    }
    Ok(())
}

#[tauri::command]
fn cancel_selection(window: WebviewWindow, state: tauri::State<'_, AppState>) {
    report::note("selection cancelled");
    *lock(&state.capture) = None;
    close_overlays(window.app_handle());
}

/// Opens the editor, as large as the image but at most 90 % of the monitor.
fn open_editor(
    app: &AppHandle,
    monitor: Option<&tauri::Monitor>,
    rect: Rect,
) -> tauri::Result<WebviewWindow> {
    let (scale, screen) = monitor.map_or((1.0, (1280.0, 800.0)), |m| {
        let scale = m.scale_factor();
        let size = m.size().to_logical::<f64>(scale);
        (scale, (size.width, size.height))
    });
    let (width, height) = editor_size(rect, scale, screen);
    WebviewWindowBuilder::new(app, EDITOR, WebviewUrl::App("editor.html".into()))
        .title("SCRN")
        .inner_size(width, height)
        .min_inner_size(560.0, 360.0)
        .center()
        .focused(true)
        .build()
}

/// Logical window size for an image of `rect` physical pixels.
fn editor_size(rect: Rect, scale: f64, screen: (f64, f64)) -> (f64, f64) {
    let width = f64::from(rect.width) / scale + EDITOR_CHROME.0;
    let height = f64::from(rect.height) / scale + EDITOR_CHROME.1;
    (
        width.clamp(560.0, (screen.0 * 0.9).max(560.0)).round(),
        height.clamp(360.0, (screen.1 * 0.9).max(360.0)).round(),
    )
}

/// The selected area as PNG for the editor.
#[tauri::command]
fn edit_image(state: tauri::State<'_, AppState>) -> Result<tauri::ipc::Response, String> {
    lock(&state.edit)
        .as_ref()
        .map(|c| tauri::ipc::Response::new(c.png.clone()))
        .ok_or_else(|| "nothing to edit".to_owned())
}

/// Takes the finished image from the editor (the PNG as request body),
/// closes the editor and uploads or saves the image in the background.
#[tauri::command]
fn save_edit(
    window: WebviewWindow,
    state: tauri::State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<(), String> {
    // Raw bytes, or a JSON array of numbers when the IPC falls back to JSON.
    let png: Vec<u8> = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(),
        tauri::ipc::InvokeBody::Json(value) => serde_json::from_value(value.clone())
            .map_err(|_| "expected the PNG as body".to_owned())?,
    };
    let (width, height) = lock(&state.edit)
        .as_ref()
        .map(|c| (c.width, c.height))
        .ok_or("nothing to edit")?;
    capture::check_edit(&png, width, height).map_err(|e| e.to_string())?;
    *lock(&state.edit) = None;
    let _ = window.close();
    tauri::async_runtime::spawn(deliver(window.app_handle().clone(), png));
    Ok(())
}

#[derive(Serialize)]
struct EditorConfig {
    /// True if a server is set up: the editor then offers "upload".
    upload: bool,
}

#[tauri::command]
fn editor_config(state: tauri::State<'_, AppState>) -> EditorConfig {
    EditorConfig {
        upload: lock(&state.settings).is_configured(),
    }
}

#[tauri::command]
fn discard_edit(window: WebviewWindow, state: tauri::State<'_, AppState>) {
    report::note("edit discarded");
    *lock(&state.edit) = None;
    let _ = window.close();
}

// --- Settings window ----------------------------------------------------

fn open_settings(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(SETTINGS) {
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App("settings.html".into()))
        .title("SCRN")
        .inner_size(560.0, 680.0)
        .min_inner_size(420.0, 480.0)
        .center()
        .focused(true)
        .build()?;
    Ok(())
}

#[derive(Serialize)]
struct SettingsView {
    settings: Settings,
    has_token: bool,
    insecure: bool,
    /// True where the client registers its own keyboard shortcut.
    shortcut_supported: bool,
    shortcut_error: Option<String>,
    config_path: String,
    log_path: String,
}

async fn settings_view(app: &AppHandle, settings: Settings) -> Result<SettingsView, String> {
    let has_token = if settings.is_configured() {
        let url = settings.server_url.clone();
        blocking(move || secret::token(&url))
            .await?
            .map_err(|e| {
                report::note(format!("reading the token failed: {e}"));
                t("error.keyring").to_owned()
            })?
            .is_some()
    } else {
        false
    };
    let path =
        |p: Option<std::path::PathBuf>| p.map_or_else(String::new, |p| p.display().to_string());
    Ok(SettingsView {
        insecure: config::is_insecure(&settings.server_url),
        shortcut_supported: cfg!(windows),
        shortcut_error: lock(&app.state::<AppState>().shortcut_error).clone(),
        settings,
        has_token,
        config_path: path(config::path()),
        log_path: path(report::path()),
    })
}

#[tauri::command]
async fn get_settings(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<SettingsView, String> {
    let settings = lock(&state.settings).clone();
    settings_view(&app, settings).await
}

/// Saves the settings. `token` replaces the stored token when given; when
/// only the address changes, the token moves along with it.
#[tauri::command]
async fn save_settings(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    settings: Settings,
    token: Option<String>,
) -> Result<SettingsView, String> {
    let settings = settings.validated().map_err(|e| e.describe())?;
    let token = token.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    if token
        .as_deref()
        .is_some_and(|t| !secret::looks_like_token(t))
    {
        return Err(t("error.bad_token").to_owned());
    }
    let old = lock(&state.settings).clone();
    let (old_url, new_url) = (old.server_url.clone(), settings.server_url.clone());
    blocking(move || -> Result<(), secret::SecretError> {
        let moved = match (&token, old_url != new_url && !old_url.is_empty()) {
            (None, true) => secret::token(&old_url)?,
            _ => None,
        };
        if let Some(token) = token.or(moved)
            && !new_url.is_empty()
        {
            secret::set_token(&new_url, &token)?;
        }
        if old_url != new_url && !old_url.is_empty() {
            secret::delete_token(&old_url)?;
        }
        Ok(())
    })
    .await?
    .map_err(|e| {
        report::note(format!("storing the token failed: {e}"));
        t("error.keyring").to_owned()
    })?;
    if settings.autostart != old.autostart || settings.autostart {
        desktop::set_autostart(settings.autostart).map_err(|e| e.to_string())?;
    }
    config::save(&settings).map_err(|e| e.describe())?;
    report::note(format!(
        "settings saved: server {}, {:?}, expiry {:?}, keep copy {}, autostart {}, shortcut {:?}",
        if settings.is_configured() {
            "set"
        } else {
            "none"
        },
        settings.visibility,
        settings.expires_in_days,
        settings.keep_copy,
        settings.autostart,
        settings.shortcut
    ));
    *lock(&state.settings) = settings.clone();
    refresh_tray(&app);
    #[cfg(windows)]
    shortcut::apply(&app);
    settings_view(&app, settings).await
}

/// Checks address and token with `GET /me`. Without a token in the form,
/// the stored token of that address is used.
#[tauri::command]
async fn test_connection(server_url: String, token: Option<String>) -> Result<String, String> {
    let url = config::normalize_server_url(&server_url).map_err(|e| e.describe())?;
    let token = match token.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()) {
        Some(token) => token,
        None => {
            let key = url.clone();
            blocking(move || secret::token(&key))
                .await?
                .map_err(|_| t("error.keyring").to_owned())?
                .ok_or_else(|| t("error.no_token").to_owned())?
        }
    };
    let api = api::Api::new(&url, &token).map_err(|e| e.to_string())?;
    match api.me().await {
        Ok(me) => Ok(format!("{} · SCRN {}", me.username, me.server_version)),
        Err(err) => {
            report::note(format!("connection test failed: {err}"));
            Err(err.describe().to_owned())
        }
    }
}

#[tauri::command]
async fn forget_token(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<SettingsView, String> {
    let settings = lock(&state.settings).clone();
    let url = settings.server_url.clone();
    blocking(move || secret::delete_token(&url))
        .await?
        .map_err(|_| t("error.keyring").to_owned())?;
    report::note("token removed");
    settings_view(&app, settings).await
}

/// Lets the overlay and the editor add their own observations to the log.
#[tauri::command]
fn note(line: String) {
    report::note(format!(
        "webview says: {}",
        line.chars().take(500).collect::<String>()
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(x: i32, y: i32, width: u32, height: u32) -> MonitorInfo {
        MonitorInfo {
            name: None,
            x,
            y,
            width,
            height,
            scale: 1.0,
        }
    }

    #[test]
    fn source_rect_picks_each_monitor() {
        // The setup from the C0 test: 3440x1440 left, 1920x1080 right.
        let monitors = [monitor(0, 0, 3440, 1440), monitor(3440, 0, 1920, 1080)];
        let left = source_rect(&monitors, 0, 5360, 1440);
        assert_eq!(
            (left.x, left.y, left.width, left.height),
            (0, 0, 3440, 1440)
        );
        let right = source_rect(&monitors, 1, 5360, 1440);
        assert_eq!(
            (right.x, right.y, right.width, right.height),
            (3440, 0, 1920, 1080)
        );
    }

    #[test]
    fn source_rect_scales_to_the_image() {
        // An image at twice the logical size, e.g. with scale 2.
        let monitors = [monitor(0, 0, 1000, 500), monitor(1000, 0, 1000, 500)];
        let right = source_rect(&monitors, 1, 4000, 1000);
        assert_eq!(
            (right.x, right.y, right.width, right.height),
            (2000, 0, 2000, 1000)
        );
    }

    #[test]
    fn commands_are_parsed() {
        assert!(parse_command(None) == Some(Command::Tray));
        assert!(parse_command(Some("capture")) == Some(Command::Capture));
        assert!(parse_command(Some("copy-last")) == Some(Command::CopyLast));
        assert!(parse_command(Some("upload")).is_none());
    }

    #[test]
    fn history_entries_use_the_short_link_or_the_gallery() {
        let info = |short_url: Option<&str>| scrn_proto::ScreenshotInfo {
            id: "0b6f".into(),
            visibility: if short_url.is_some() {
                scrn_proto::Visibility::Public
            } else {
                scrn_proto::Visibility::Private
            },
            short_url: short_url.map(str::to_owned),
            expires_at: None,
            width: 1,
            height: 1,
            created_at: 7,
            views: 0,
        };
        let public = entry_of("https://scrn.dev", info(Some("https://scrn.dev/AbCd1234")));
        assert!(public.public);
        assert_eq!(public.url, "https://scrn.dev/AbCd1234");
        let private = entry_of("https://scrn.dev", info(None));
        assert!(!private.public);
        assert_eq!(private.url, "https://scrn.dev/app/shots/0b6f");
        assert!(recent_label(&public, 7).starts_with("AbCd1234 · "));
    }

    #[test]
    fn editor_fits_the_image_and_the_screen() {
        let rect = |width, height| Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        // Room to spare: image plus toolbar and margins.
        assert_eq!(
            editor_size(rect(800, 400), 1.0, (1920.0, 1080.0)),
            (832.0, 496.0)
        );
        // Physical pixels at scale 2 need half the logical size.
        assert_eq!(
            editor_size(rect(1600, 800), 2.0, (1920.0, 1080.0)),
            (832.0, 496.0)
        );
        // Large images are capped at 90 % of the screen, tiny ones get the minimum.
        assert_eq!(
            editor_size(rect(5000, 3000), 1.0, (1920.0, 1080.0)),
            (1728.0, 972.0)
        );
        assert_eq!(
            editor_size(rect(20, 10), 1.0, (1920.0, 1080.0)),
            (560.0, 360.0)
        );
    }

    #[test]
    fn source_rect_uses_the_whole_image_for_one_monitor() {
        let monitors = [monitor(0, 0, 1920, 1080)];
        let r = source_rect(&monitors, 0, 1920, 1080);
        assert_eq!((r.x, r.y, r.width, r.height), (0, 0, 1920, 1080));
        let r = source_rect(&[], 3, 800, 600);
        assert_eq!((r.width, r.height), (800, 600));
    }
}
