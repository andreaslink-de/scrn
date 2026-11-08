//! Keyboard shortcuts through the GlobalShortcuts portal, on desktops
//! that offer it: KDE Plasma, Hyprland and GNOME 48 or newer. Elsewhere,
//! such as on GNOME 46 or Sway, `scrn capture` is bound in the desktop's
//! own shortcut settings instead.
//!
//! The client suggests Print for a screenshot; the desktop decides, and
//! the user can change both shortcuts in its settings.

use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use tauri::AppHandle;

use crate::i18n::t;
use crate::{Command, parse_command, report, run_command};

/// The shortcut IDs are the command names of `scrn <command>`.
const SHORTCUTS: [(&str, &str, Option<&str>); 2] = [
    ("capture", "tray.capture", Some("Print")),
    ("copy-last", "tray.copy_last", None),
];

/// Binds the shortcuts and runs their commands until the app quits.
/// Returns at once, without an error, where the portal is missing.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = listen(&app).await {
            report::note(format!("portal shortcuts unavailable: {err}"));
        }
    });
}

async fn listen(app: &AppHandle) -> ashpd::Result<()> {
    let portal = GlobalShortcuts::new().await?;
    // Subscribe before binding, so no early press is lost.
    let mut activated = portal.receive_activated().await?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await?;
    let shortcuts: Vec<NewShortcut> = SHORTCUTS
        .iter()
        .map(|(id, label, trigger)| NewShortcut::new(*id, t(label)).preferred_trigger(*trigger))
        .collect();
    let bound = portal
        .bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default())
        .await?
        .response()?;
    for shortcut in bound.shortcuts() {
        report::note(format!(
            "portal shortcut {:?} bound to {:?}",
            shortcut.id(),
            shortcut.trigger_description()
        ));
    }
    // The client opens a single session, so every press is one of ours.
    while let Some(event) = activated.next().await {
        match command_for(event.shortcut_id()) {
            Some(command) => run_command(app, command, "portal shortcut"),
            None => report::note(format!("unknown portal shortcut {:?}", event.shortcut_id())),
        }
    }
    // The stream ends when the bus connection closes. The session lives
    // until then: dropping it would release the shortcuts.
    drop(session);
    Ok(())
}

fn command_for(id: &str) -> Option<Command> {
    SHORTCUTS
        .iter()
        .any(|(known, _, _)| *known == id)
        .then(|| parse_command(Some(id)))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_ids_are_commands() {
        assert!(matches!(command_for("capture"), Some(Command::Capture)));
        assert!(matches!(command_for("copy-last"), Some(Command::CopyLast)));
        assert!(command_for("").is_none());
        assert!(command_for("quit").is_none());
    }

    #[test]
    fn shortcut_descriptions_are_translated() {
        for (_, label, _) in SHORTCUTS {
            assert_ne!(t(label), "", "{label}");
        }
    }
}

#[cfg(test)]
mod live {
    /// Asks the running session's portal; needs a desktop. GNOME 46 has
    /// no GlobalShortcuts portal, so this only reports what is there.
    #[tokio::test]
    #[ignore = "needs a desktop session"]
    async fn reports_whether_the_portal_exists() {
        match ashpd::desktop::global_shortcuts::GlobalShortcuts::new().await {
            Ok(portal) => println!("GlobalShortcuts version {}", portal.version()),
            Err(err) => println!("GlobalShortcuts missing: {err}"),
        }
    }
}
