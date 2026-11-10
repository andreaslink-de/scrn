//! Where the client keeps its files.
//!
//! | What | Linux | Windows |
//! |------|-------|---------|
//! | settings | `$XDG_CONFIG_HOME/scrn/config.json` | `%APPDATA%\SCRN\config.json` |
//! | history, log | `$XDG_STATE_HOME/scrn/` | `%LOCALAPPDATA%\SCRN\` |
//! | saved images | `SCRN` in `XDG_PICTURES_DIR` (`~/.config/user-dirs.dirs`) | `SCRN` in the Pictures known folder |
//!
//! The pictures folder has a localized name, such as `~/Bilder`, and on
//! Windows it may be moved, e.g. into OneDrive; the system says where.

use std::path::PathBuf;

/// An absolute directory from the environment variable `var`.
fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

#[cfg(not(windows))]
fn home() -> Option<PathBuf> {
    env_dir("HOME")
}

/// `$<var>`, or `$HOME/<fallback>`.
#[cfg(not(windows))]
pub fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    env_dir(var).or_else(|| home().map(|h| h.join(fallback)))
}

#[cfg(not(windows))]
fn config_dir() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config").map(|d| d.join("scrn"))
}

#[cfg(windows)]
fn config_dir() -> Option<PathBuf> {
    env_dir("APPDATA").map(|d| d.join("SCRN"))
}

#[cfg(not(windows))]
fn state_dir() -> Option<PathBuf> {
    xdg("XDG_STATE_HOME", ".local/state").map(|d| d.join("scrn"))
}

#[cfg(windows)]
fn state_dir() -> Option<PathBuf> {
    env_dir("LOCALAPPDATA").map(|d| d.join("SCRN"))
}

pub fn config_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.json"))
}

pub fn history_file() -> Option<PathBuf> {
    state_dir().map(|d| d.join("history.json"))
}

pub fn log_file() -> Option<PathBuf> {
    state_dir().map(|d| d.join("client.log"))
}

/// The folder for saved images, `SCRN` in the pictures folder the system
/// names: on Linux from `XDG_PICTURES_DIR` in `user-dirs.dirs`, on
/// Windows the Pictures known folder. `~/Pictures` only when there is none.
pub fn pictures_dir() -> PathBuf {
    dirs::picture_dir()
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join("Pictures")))
        .unwrap_or_else(std::env::temp_dir)
        .join("SCRN")
}

#[cfg(test)]
mod pictures_tests {
    #[test]
    fn saved_images_go_to_scrn_in_an_absolute_folder() {
        let dir = super::pictures_dir();
        assert!(dir.is_absolute(), "{}", dir.display());
        assert!(dir.ends_with("SCRN"));
        println!("pictures: {}", dir.display());
    }
}
