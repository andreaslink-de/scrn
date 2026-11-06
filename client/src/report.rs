//! The spike report: what the spike observed on this desktop, appended to
//! `$XDG_STATE_HOME/scrn/spike.log` (default `~/.local/state/scrn`) and
//! printed to stderr. It never contains image data.

use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(base.join("scrn").join("spike.log"))
}

/// Writes one line to the report.
pub fn note(line: impl AsRef<str>) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let line = format!("[{secs}] {}", line.as_ref());
    eprintln!("{line}");
    let Some(path) = path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Facts about the session, written once at startup.
pub fn environment() {
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| "-".to_owned());
    note(format!(
        "start: scrn {} session={} desktop={} wayland_display={} gdk_backend={} scale_env={}",
        env!("CARGO_PKG_VERSION"),
        var("XDG_SESSION_TYPE"),
        var("XDG_CURRENT_DESKTOP"),
        var("WAYLAND_DISPLAY"),
        var("GDK_BACKEND"),
        var("GDK_SCALE"),
    ));
}
