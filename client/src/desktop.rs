//! Desktop integration: clipboard, opening links, notifications and
//! autostart.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::AppHandle;

use crate::report;

/// The clipboard owner. It has to live as long as the copied text should
/// be pasteable, so it is kept for the whole run of the tray app.
static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

/// Puts `text` on the clipboard.
///
/// On Wayland a client may only set the clipboard while one of its windows
/// has the keyboard focus, and a tray app usually has none: GNOME drops
/// such requests without an error. arboard owns the X11 clipboard through
/// XWayland instead, which needs no focus, and GNOME passes it on to
/// Wayland apps. If that fails, GTK is tried as before.
pub fn copy_text(app: &AppHandle, text: String) {
    let copied = {
        let mut slot = CLIPBOARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_none() {
            match arboard::Clipboard::new() {
                Ok(clipboard) => *slot = Some(clipboard),
                Err(err) => report::note(format!("opening the clipboard failed: {err}")),
            }
        }
        match slot.as_mut().map(|c| c.set_text(text.clone())) {
            Some(Ok(())) => true,
            Some(Err(err)) => {
                report::note(format!("copying to the clipboard failed: {err}"));
                // A broken connection is opened again next time.
                *slot = None;
                false
            }
            None => false,
        }
    };
    if copied {
        return;
    }
    let result = app.run_on_main_thread(move || {
        let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
        clipboard.set_text(&text);
        clipboard.store();
    });
    if let Err(err) = result {
        report::note(format!("copying to the clipboard failed: {err}"));
    }
}

/// Opens `url` in the default browser.
pub fn open_url(app: &AppHandle, url: String) {
    let result = app.run_on_main_thread(move || {
        if let Err(err) =
            gtk::show_uri_on_window(None::<&gtk::Window>, &url, gtk::current_event_time())
        {
            report::note(format!("opening a link failed: {err}"));
        }
    });
    if let Err(err) = result {
        report::note(format!("opening a link failed: {err}"));
    }
}

/// Shows a desktop notification; failures only go to the log.
pub fn notify(summary: &str, body: &str) {
    let mut notification = notify_rust::Notification::new();
    notification
        .appname("SCRN")
        .summary(summary)
        .body(body)
        .icon("scrn")
        .timeout(notify_rust::Timeout::Milliseconds(6000));
    let notification = notification.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = notification.show_async().await {
            report::note(format!("notification failed: {err}"));
        }
    });
}

// --- Autostart ----------------------------------------------------------

/// `$XDG_CONFIG_HOME/autostart/scrn.desktop`.
fn autostart_path() -> Option<PathBuf> {
    crate::config::base_dir("XDG_CONFIG_HOME", ".config")
        .map(|d| d.join("autostart").join("scrn.desktop"))
}

/// The program to start: the AppImage itself when running from one,
/// otherwise this executable.
fn program() -> std::io::Result<PathBuf> {
    match std::env::var_os("APPIMAGE") {
        Some(path) if Path::new(&path).is_absolute() => Ok(PathBuf::from(path)),
        _ => std::env::current_exe(),
    }
}

/// Turns autostart on or off.
pub fn set_autostart(enabled: bool) -> std::io::Result<()> {
    let path = autostart_path().ok_or_else(|| std::io::Error::other("no home directory"))?;
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    let entry = desktop_entry(&program()?)
        .ok_or_else(|| std::io::Error::other("the program path is not valid UTF-8"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, entry)
}

/// A desktop entry that starts `program` in the tray.
fn desktop_entry(program: &Path) -> Option<String> {
    let program = program.to_str()?;
    if program.contains(['\n', '\r']) {
        return None;
    }
    // Quoting rules of the Desktop Entry spec for the Exec key; the `\`
    // escapes are themselves escaped once more for the string value.
    let mut quoted = String::from("\"");
    for c in program.chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    Some(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=SCRN\n\
         Comment=Screenshots to your own server\n\
         Exec={quoted}\n\
         Icon=scrn\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a desktop session: `cargo test -- --ignored`. A second
    /// clipboard client reads what the first one owns.
    #[test]
    #[ignore = "needs a desktop session"]
    fn copied_text_can_be_pasted_elsewhere() {
        let text = format!("scrn clipboard test {}", std::process::id());
        let mut owner = arboard::Clipboard::new().unwrap();
        owner.set_text(text.clone()).unwrap();
        let mut reader = arboard::Clipboard::new().unwrap();
        assert_eq!(reader.get_text().unwrap(), text);
        // A separate X11 client, if Python with Tk is there.
        let tk = std::process::Command::new("python3")
            .args([
                "-c",
                "import tkinter; r = tkinter.Tk(); r.withdraw(); print(r.clipboard_get(), end='')",
            ])
            .output();
        if let Some(out) = tk.ok().filter(|o| o.status.success()) {
            assert_eq!(String::from_utf8_lossy(&out.stdout), text);
        }
    }

    #[test]
    fn desktop_entries_quote_the_program() {
        let entry = desktop_entry(Path::new("/opt/SCRN/scrn")).unwrap();
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("\nExec=\"/opt/SCRN/scrn\"\n"));
        assert!(entry.contains("\nType=Application\n"));
        let odd = desktop_entry(Path::new("/home/a b/$x/100%/\"q\"")).unwrap();
        assert!(
            odd.contains("Exec=\"/home/a b/\\\\$x/100%%/\\\\\"q\\\\\"\"\n"),
            "{odd}"
        );
        assert!(desktop_entry(Path::new("/tmp/a\nExec=evil")).is_none());
    }
}
