//! SCRN desktop client.
//!
//! A tray icon with a menu, `scrn capture` as command (a second start hands
//! the command to the running instance), capture through the screenshot
//! portal, a fullscreen overlay per monitor to select an area and an editor
//! for arrows, highlights and blurred areas. The finished image is saved as
//! PNG under `~/Pictures/SCRN`. Observations go to the client log (see
//! [`report`]).

mod capture;
mod report;

use std::sync::Mutex;

use gtk::prelude::GtkWindowExt;
use serde::Serialize;
use tauri::http::{Response, StatusCode, header::CONTENT_TYPE};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use capture::{Capture, Rect};

/// Overlay windows are labelled `overlay-<monitor index>`, one per monitor.
const OVERLAY: &str = "overlay-";

/// Time the tray menu needs to close before the screen is captured.
const MENU_CLOSE_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

/// The editor window.
const EDITOR: &str = "editor";

/// Toolbar height and margins of the editor window in logical pixels.
const EDITOR_CHROME: (f64, f64) = (32.0, 96.0);

#[derive(Default)]
struct AppState {
    /// The full screen while the overlays are open.
    capture: Mutex<Option<Capture>>,
    /// The selected area while the editor is open.
    edit: Mutex<Option<Capture>>,
}

fn lock(slot: &Mutex<Option<Capture>>) -> std::sync::MutexGuard<'_, Option<Capture>> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
            edit_image,
            save_edit,
            discard_edit,
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
        if source == "tray menu" {
            // Otherwise the closing menu ends up in the screenshot.
            tokio::time::sleep(MENU_CLOSE_DELAY).await;
        }
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
                *lock(&app.state::<AppState>().capture) = Some(shot);
                if let Err(err) = open_overlays(&app) {
                    report::note(format!("opening the overlay failed: {err}"));
                    close_overlays(&app);
                }
            }
            Err(err) => report::note(format!("capture failed: {err}")),
        }
    });
}

/// Opens one overlay per monitor. Wayland ignores window positions, so
/// each window is put fullscreen on its monitor through GTK.
fn open_overlays(app: &AppHandle) -> tauri::Result<()> {
    let count = app.available_monitors()?.len().max(1);
    for index in 0..count {
        let window = WebviewWindowBuilder::new(
            app,
            format!("{OVERLAY}{index}"),
            WebviewUrl::App("overlay.html".into()),
        )
        .title("SCRN")
        .decorations(false)
        .always_on_top(true)
        .resizable(false)
        .visible(false)
        .build()?;
        let Ok(monitor) = i32::try_from(index) else {
            continue;
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
        })?;
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

/// Takes the finished image from the editor (the raw PNG as request body),
/// saves it and closes the editor. Returns the path.
#[tauri::command]
fn save_edit(
    window: WebviewWindow,
    state: tauri::State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<String, String> {
    let tauri::ipc::InvokeBody::Raw(png) = request.body() else {
        return Err("expected the PNG as raw body".to_owned());
    };
    let (width, height) = lock(&state.edit)
        .as_ref()
        .map(|c| (c.width, c.height))
        .ok_or("nothing to edit")?;
    capture::check_edit(png, width, height).map_err(|e| e.to_string())?;
    let path = capture::save(png).map_err(|e| e.to_string())?;
    report::note(format!(
        "edit saved to {} ({} bytes)",
        path.display(),
        png.len()
    ));
    *lock(&state.edit) = None;
    let _ = window.close();
    Ok(path.display().to_string())
}

#[tauri::command]
fn discard_edit(window: WebviewWindow, state: tauri::State<'_, AppState>) {
    report::note("edit discarded");
    *lock(&state.edit) = None;
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
