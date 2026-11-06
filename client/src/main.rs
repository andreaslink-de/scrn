//! SCRN desktop client, C0 spike.
//!
//! Checks whether Tauri carries the client on Linux with Wayland: a tray
//! icon with a menu, `scrn capture` as command (a second start hands the
//! command to the running instance), capture through the screenshot portal
//! and a fullscreen overlay to select an area. The selection is saved as
//! PNG under `~/Pictures/SCRN`. Everything the spike observes goes to the
//! spike report (see [`report`]).

mod capture;
mod report;

use std::sync::Mutex;

use serde::Serialize;
use tauri::http::{Response, StatusCode, header::CONTENT_TYPE};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use capture::{Capture, Rect};

const OVERLAY: &str = "overlay";

#[derive(Default)]
struct AppState {
    capture: Mutex<Option<Capture>>,
}

impl AppState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Capture>> {
        self.capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

const USAGE: &str = "usage: scrn [capture]

  scrn           start the tray icon
  scrn capture   take a screenshot and select an area
                 (hands over to the running instance, if there is one)";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let capture_now = match args.first().map(String::as_str) {
        None => false,
        Some("capture") => true,
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return;
        }
        Some(other) => {
            eprintln!("unknown command {other:?}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    report::environment();

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            report::note(format!("second start handed over: args={:?}", &argv[1..]));
            if argv.get(1).is_some_and(|a| a == "capture") {
                start_capture(app, "command (running instance)");
            }
        }))
        .manage(AppState::default())
        .register_uri_scheme_protocol("scrn", |ctx, request| {
            serve(ctx.app_handle(), request.uri().path())
        })
        .invoke_handler(tauri::generate_handler![
            overlay_info,
            finish_selection,
            cancel_selection,
            note
        ])
        .setup(move |app| {
            build_tray(app.handle())?;
            log_monitors(app.handle());
            if capture_now {
                start_capture(app.handle(), "command");
            }
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

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let capture = MenuItem::with_id(app, "capture", "Screenshot aufnehmen", true, None::<&str>)?;
    let log = report::path().map_or_else(|| "-".to_owned(), |p| p.display().to_string());
    let info = MenuItem::with_id(app, "info", format!("Bericht: {log}"), false, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&capture, &info, &separator, &quit])?;
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("SCRN")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "capture" => start_capture(app, "tray menu"),
            "quit" => {
                report::note("quit from the tray menu");
                app.exit(0);
            }
            _ => {}
        })
        // On Linux the tray is an AppIndicator, which reports no clicks;
        // the spike records whether any arrive.
        .on_tray_icon_event(|_, event| report::note(format!("tray icon event: {event:?}")))
        .build(app)?;
    report::note("tray icon created");
    Ok(())
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
    if let Some(window) = app.get_webview_window(OVERLAY) {
        let _ = window.set_focus();
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let started = std::time::Instant::now();
        match capture::take().await {
            Ok((shot, portal_path)) => {
                report::note(format!(
                    "portal screenshot: {}x{} px, {} bytes, {} ms, file {} (removed)",
                    shot.width,
                    shot.height,
                    shot.png.len(),
                    started.elapsed().as_millis(),
                    portal_path.display()
                ));
                *app.state::<AppState>().lock() = Some(shot);
                if let Err(err) = open_overlay(&app) {
                    report::note(format!("opening the overlay failed: {err}"));
                }
            }
            Err(err) => report::note(format!("capture failed: {err}")),
        }
    });
}

fn open_overlay(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, OVERLAY, WebviewUrl::App("overlay.html".into()))
        .title("SCRN")
        .fullscreen(true)
        .decorations(false)
        .always_on_top(true)
        .resizable(false)
        .focused(true)
        .build()
}

/// Serves the current capture to the overlay as `scrn://localhost/capture.png`.
fn serve(app: &AppHandle, path: &str) -> Response<Vec<u8>> {
    let png = (path == "/capture.png")
        .then(|| {
            app.state::<AppState>()
                .lock()
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

#[derive(Serialize)]
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
/// portal image covers the bounding box of all monitors; with a single
/// monitor this is the whole image.
#[tauri::command]
fn overlay_info(
    window: WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<OverlayInfo, String> {
    let (image_width, image_height) = state
        .lock()
        .as_ref()
        .map(|c| (c.width, c.height))
        .ok_or("no capture")?;
    let monitors: Vec<MonitorInfo> = window
        .available_monitors()
        .map_err(|e| e.to_string())?
        .iter()
        .map(monitor_info)
        .collect();
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .map(|m| monitor_info(&m));
    let source = match (&monitor, monitors.len()) {
        (Some(m), n) if n > 1 => {
            let left = monitors.iter().map(|m| m.x).min().unwrap_or(0);
            let top = monitors.iter().map(|m| m.y).min().unwrap_or(0);
            let right = monitors
                .iter()
                .map(|m| m.x + m.width as i32)
                .max()
                .unwrap_or(1);
            let factor = f64::from(image_width) / f64::from((right - left).max(1));
            let scale = |v: f64| (v * factor).round().max(0.0) as u32;
            Rect {
                x: scale(f64::from(m.x - left)),
                y: scale(f64::from(m.y - top)),
                width: scale(f64::from(m.width)).min(image_width),
                height: scale(f64::from(m.height)).min(image_height),
            }
        }
        _ => Rect {
            x: 0,
            y: 0,
            width: image_width,
            height: image_height,
        },
    };
    report::note(format!(
        "overlay: window inner={:?} scale={:?} monitor={:?} source={source:?}",
        window.inner_size().ok(),
        window.scale_factor().ok(),
        monitor
            .as_ref()
            .map(|m| (&m.name, m.x, m.y, m.width, m.height, m.scale)),
    ));
    Ok(OverlayInfo {
        image_width,
        image_height,
        source,
        monitor,
        monitors,
    })
}

/// Crops the selection, saves it and closes the overlay. Returns the path.
#[tauri::command]
fn finish_selection(
    window: WebviewWindow,
    state: tauri::State<'_, AppState>,
    rect: Rect,
) -> Result<String, String> {
    let png = {
        let guard = state.lock();
        let capture = guard.as_ref().ok_or("no capture")?;
        capture::crop(&capture.png, rect).map_err(|e| e.to_string())?
    };
    let dir = capture::output_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let path = dir.join(format!("scrn-spike-{secs}.png"));
    std::fs::write(&path, &png).map_err(|e| e.to_string())?;
    report::note(format!(
        "selection {rect:?} saved to {} ({} bytes)",
        path.display(),
        png.len()
    ));
    *state.lock() = None;
    let _ = window.close();
    Ok(path.display().to_string())
}

#[tauri::command]
fn cancel_selection(window: WebviewWindow, state: tauri::State<'_, AppState>) {
    report::note("selection cancelled");
    *state.lock() = None;
    let _ = window.close();
}

/// Lets the overlay add its own observations to the report.
#[tauri::command]
fn note(line: String) {
    report::note(format!(
        "overlay says: {}",
        line.chars().take(500).collect::<String>()
    ));
}
