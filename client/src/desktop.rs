//! Desktop integration: clipboard, opening links, notifications and
//! autostart.

#[cfg(any(target_os = "linux", test))]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
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
/// Wayland apps. If that fails, GTK is tried as before. On Windows arboard
/// uses the Win32 clipboard.
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
    #[cfg(target_os = "linux")]
    {
        let result = app.run_on_main_thread(move || {
            let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
            clipboard.set_text(&text);
            clipboard.store();
        });
        if let Err(err) = result {
            report::note(format!("copying to the clipboard failed: {err}"));
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = app;
}

/// Opens `url` (http or https only) in the default browser.
pub fn open_url(app: &AppHandle, url: String) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        report::note("refused to open a link that is not http(s)");
        return;
    }
    #[cfg(target_os = "linux")]
    {
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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// No console window for the helper.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = app;
        // The URL handler of Windows; the URL is one argument, so no
        // shell quoting is involved.
        let result = std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", &url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        if let Err(err) = result {
            report::note(format!("opening a link failed: {err}"));
        }
    }
}

/// Shows a desktop notification; failures only go to the log.
pub fn notify(summary: &str, body: &str) {
    let mut notification = notify_rust::Notification::new();
    notification
        .summary(summary)
        .body(body)
        .timeout(notify_rust::Timeout::Milliseconds(6000));
    #[cfg(target_os = "linux")]
    {
        notification.appname("SCRN").icon("scrn");
        let notification = notification.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(err) = notification.show_async().await {
                report::note(format!("notification failed: {err}"));
            }
        });
    }
    #[cfg(windows)]
    {
        // The installer registers this ID with the Start menu shortcut, so
        // the toast shows the SCRN name and icon.
        notification.app_id(APP_ID);
        let notification = notification.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(err) = notification.show() {
                report::note(format!("notification failed: {err}"));
            }
        });
    }
}

/// The identifier from `tauri.conf.json`, used as AppUserModelID.
#[cfg(windows)]
const APP_ID: &str = "de.andreaslink.scrn";

/// True if the Windows taskbar uses the light theme, so the tray needs the
/// dark icon.
#[cfg(windows)]
pub fn taskbar_is_light() -> bool {
    windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|key| key.get_u32("SystemUsesLightTheme"))
        .is_ok_and(|v| v != 0)
}

// --- Autostart ----------------------------------------------------------

/// `$XDG_CONFIG_HOME/autostart/scrn.desktop`.
#[cfg(target_os = "linux")]
fn autostart_path() -> Option<PathBuf> {
    crate::paths::xdg("XDG_CONFIG_HOME", ".config")
        .map(|d| d.join("autostart").join("scrn.desktop"))
}

/// The program to start: the AppImage itself when running from one,
/// otherwise this executable.
#[cfg(target_os = "linux")]
fn program() -> std::io::Result<PathBuf> {
    match std::env::var_os("APPIMAGE") {
        Some(path) if Path::new(&path).is_absolute() => Ok(PathBuf::from(path)),
        _ => std::env::current_exe(),
    }
}

/// Turns autostart on or off.
#[cfg(target_os = "linux")]
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
#[cfg(any(target_os = "linux", test))]
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

/// The key whose values Windows starts at login.
#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Turns autostart on or off through the Run key of the current user.
#[cfg(windows)]
pub fn set_autostart(enabled: bool) -> std::io::Result<()> {
    let key = windows_registry::CURRENT_USER
        .create(RUN_KEY)
        .map_err(std::io::Error::other)?;
    if enabled {
        let program = std::env::current_exe()?;
        let value = format!("\"{}\"", program.display());
        key.set_string("SCRN", value).map_err(std::io::Error::other)
    } else {
        // Not there: nothing to remove.
        if key.get_type("SCRN").is_err() {
            return Ok(());
        }
        key.remove_value("SCRN").map_err(std::io::Error::other)
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn set_autostart(_enabled: bool) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "autostart is not supported on this system yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a desktop session: `cargo test -- --ignored`. A second
    /// clipboard client reads what the first one owns.
    #[cfg(target_os = "linux")]
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
