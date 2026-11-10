//! The keyboard shortcut for a screenshot on Windows (`PrintScreen` by
//! default). Linux desktops call `scrn capture` from their own shortcut
//! settings instead; GNOME on Wayland gives apps no global shortcuts.

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::{AppState, lock, report};

pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                crate::start_capture(app, "shortcut");
            }
        })
        .build()
}

/// Unregisters the shortcut until [`apply`] runs again.
pub fn release(app: &AppHandle) {
    if let Err(err) = app.global_shortcut().unregister_all() {
        report::note(format!("releasing the shortcut failed: {err}"));
    }
}

/// Registers the shortcut from the settings in place of the old one and
/// keeps the error, if any, for the settings window.
pub fn apply(app: &AppHandle) {
    let state = app.state::<AppState>();
    let wanted = lock(&state.settings).shortcut.clone();
    let shortcuts = app.global_shortcut();
    if let Err(err) = shortcuts.unregister_all() {
        report::note(format!("releasing the shortcut failed: {err}"));
    }
    let result = if wanted.is_empty() {
        Ok(())
    } else {
        wanted
            .parse::<Shortcut>()
            .map_err(|e| e.to_string())
            .and_then(|s| shortcuts.register(s).map_err(|e| e.to_string()))
    };
    match &result {
        Ok(()) => report::note(format!("shortcut {wanted:?} registered")),
        Err(err) => report::note(format!("shortcut {wanted:?} failed: {err}")),
    }
    *lock(&state.shortcut_error) = result.err();
}
