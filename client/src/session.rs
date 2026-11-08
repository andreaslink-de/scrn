//! Which Linux desktop the client runs on. Capture and the screenshot
//! permission differ between them.

/// The desktop of the session, from `XDG_CURRENT_DESKTOP`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Gnome,
    Kde,
    /// A compositor built on wlroots, such as Sway or Hyprland. Their
    /// portals may lack the screenshot interface; `grim` fills in.
    Wlroots,
    Other,
}

impl Desktop {
    pub fn current() -> Self {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        Self::detect(&desktop, wayland)
    }

    /// `desktop` is a colon-separated list such as `ubuntu:GNOME`.
    fn detect(desktop: &str, wayland: bool) -> Self {
        let names: Vec<String> = desktop.split(':').map(str::to_ascii_lowercase).collect();
        let has = |name: &str| names.iter().any(|n| n == name);
        if has("gnome") {
            Self::Gnome
        } else if has("kde") {
            Self::Kde
        } else if wayland
            && ["sway", "hyprland", "river", "wayfire", "labwc", "niri"]
                .iter()
                .any(|n| has(n))
        {
            Self::Wlroots
        } else {
            Self::Other
        }
    }

    /// GNOME keeps the screenshot permission in the permission store,
    /// which the client can fill in after asking.
    pub fn has_permission_store(self) -> bool {
        self == Self::Gnome
    }

    /// Whether `grim` may capture when the portal fails. It needs the
    /// wlroots screencopy protocol, which GNOME and KDE do not offer.
    pub fn may_use_grim(self) -> bool {
        matches!(self, Self::Wlroots | Self::Other)
    }
}

#[cfg(test)]
mod tests {
    use super::Desktop;

    #[test]
    fn desktops_from_the_environment() {
        assert_eq!(Desktop::detect("ubuntu:GNOME", true), Desktop::Gnome);
        assert_eq!(Desktop::detect("GNOME", false), Desktop::Gnome);
        assert_eq!(Desktop::detect("KDE", true), Desktop::Kde);
        assert_eq!(Desktop::detect("sway", true), Desktop::Wlroots);
        assert_eq!(Desktop::detect("Hyprland", true), Desktop::Wlroots);
        // Without Wayland grim cannot work.
        assert_eq!(Desktop::detect("sway", false), Desktop::Other);
        assert_eq!(Desktop::detect("XFCE", false), Desktop::Other);
        assert_eq!(Desktop::detect("", false), Desktop::Other);
    }

    #[test]
    fn permission_store_and_grim() {
        assert!(Desktop::Gnome.has_permission_store());
        assert!(!Desktop::Kde.has_permission_store());
        assert!(!Desktop::Wlroots.has_permission_store());
        assert!(Desktop::Wlroots.may_use_grim());
        assert!(!Desktop::Gnome.may_use_grim());
        assert!(!Desktop::Kde.may_use_grim());
    }
}
