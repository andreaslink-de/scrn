//! The screenshot permission of the desktop portal.
//!
//! GNOME lets an app take screenshots without a dialog only if the portal's
//! permission store says so. A program started from a terminal has no app
//! ID and is allowed by default. Started from the menu, the client runs as
//! the app of its desktop file (`SCRN`), and the portal refuses until the
//! permission is stored. Its own question needs a parent window, which the
//! tray app does not have, so the client asks in its own window and stores
//! the answer itself.

use std::collections::HashMap;

use ashpd::zbus;
use ashpd::zbus::OwnedVariant;

const SERVICE: &str = "org.freedesktop.impl.portal.PermissionStore";
const PATH: &str = "/org/freedesktop/impl/portal/PermissionStore";
const INTERFACE: &str = "org.freedesktop.impl.portal.PermissionStore";
/// Table and entry the screenshot portal reads.
const TABLE: &str = "screenshot";
const ENTRY: &str = "screenshot";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Granted,
    Denied,
    Unknown,
}

/// The app ID the portal sees for this process, from its systemd unit.
/// `None` when started from a terminal or outside a desktop session.
pub fn app_id() -> Option<String> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    app_id_from_cgroup(&cgroup)
}

fn app_id_from_cgroup(cgroup: &str) -> Option<String> {
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    app_id_from_unit(path.rsplit('/').next()?)
}

/// Parses `app-[<launcher>-]<app ID>-<random>.scope` and
/// `app-[<launcher>-]<app ID>[@<random>].service`, the names desktops give
/// the units of started apps. Dashes inside the app ID are escaped.
fn app_id_from_unit(unit: &str) -> Option<String> {
    let rest = unit.strip_prefix("app-")?;
    let name = if let Some(scope) = rest.strip_suffix(".scope") {
        scope.rsplit_once('-')?.0
    } else {
        let service = rest.strip_suffix(".service")?;
        service.split_once('@').map_or(service, |(name, _)| name)
    };
    let id = unescape(name.rsplit('-').next()?)?;
    (!id.is_empty()).then_some(id)
}

/// Undoes the `\xNN` escapes of systemd unit names.
fn unescape(escaped: &str) -> Option<String> {
    let bytes = escaped.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            if bytes.get(i + 1) != Some(&b'x') {
                return None;
            }
            let hex = escaped.get(i + 2..i + 4)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

async fn store() -> zbus::Result<zbus::Proxy<'static>> {
    let connection = zbus::Connection::session().await?;
    zbus::Proxy::new(&connection, SERVICE, PATH, INTERFACE).await
}

/// What the permission store says about `app_id`. Errors, such as a
/// missing table on a fresh system, count as unknown.
pub async fn status(app_id: &str) -> Permission {
    let reply: zbus::Result<(HashMap<String, Vec<String>>, OwnedVariant)> = match store().await {
        Ok(proxy) => proxy.call("Lookup", &(TABLE, ENTRY)).await,
        Err(err) => Err(err),
    };
    match reply {
        Ok((apps, _)) => from_entry(apps.get(app_id)),
        Err(_) => Permission::Unknown,
    }
}

fn from_entry(values: Option<&Vec<String>>) -> Permission {
    match values.and_then(|v| v.first()).map(String::as_str) {
        Some("yes") => Permission::Granted,
        Some("no") => Permission::Denied,
        _ => Permission::Unknown,
    }
}

/// Stores that `app_id` may take screenshots without a dialog.
pub async fn grant(app_id: &str) -> zbus::Result<()> {
    let proxy = store().await?;
    let yes: &[&str] = &["yes"];
    proxy
        .call::<_, _, ()>("SetPermission", &(TABLE, true, ENTRY, app_id, yes))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_ids_from_units() {
        assert_eq!(
            app_id_from_unit("app-gnome-SCRN-3784964.scope").as_deref(),
            Some("SCRN")
        );
        assert_eq!(
            app_id_from_unit("app-org.example.Shot-12.scope").as_deref(),
            Some("org.example.Shot")
        );
        assert_eq!(
            app_id_from_unit("app-gnome-my\\x2dapp-77.scope").as_deref(),
            Some("my-app")
        );
        assert_eq!(
            app_id_from_unit("app-kde-SCRN@a1b2.service").as_deref(),
            Some("SCRN")
        );
        assert_eq!(app_id_from_unit("vte-spawn-a970.scope"), None);
        assert_eq!(app_id_from_unit("app-gnome-SCRN.slice"), None);
        assert_eq!(app_id_from_unit("app-bad\\q-1.scope"), None);
    }

    #[test]
    fn app_id_from_the_cgroup_file() {
        let menu = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-SCRN-3784964.scope\n";
        assert_eq!(app_id_from_cgroup(menu).as_deref(), Some("SCRN"));
        let terminal = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.gnome.Terminal.slice/vte-spawn-a9704f31.scope\n";
        assert_eq!(app_id_from_cgroup(terminal), None);
        assert_eq!(app_id_from_cgroup(""), None);
    }

    #[test]
    fn permission_entries() {
        assert_eq!(from_entry(Some(&vec!["yes".into()])), Permission::Granted);
        assert_eq!(from_entry(Some(&vec!["no".into()])), Permission::Denied);
        assert_eq!(from_entry(Some(&vec![])), Permission::Unknown);
        assert_eq!(from_entry(None), Permission::Unknown);
    }
}

#[cfg(test)]
mod live {
    /// Reads the permission store of the running session; needs a desktop.
    #[tokio::test]
    #[ignore = "needs a desktop session"]
    async fn reads_the_permission_store() {
        let status = super::status("SCRN").await;
        println!("SCRN: {status:?}");
        assert_eq!(
            super::status("no.such.app").await,
            super::Permission::Unknown
        );
    }
}
