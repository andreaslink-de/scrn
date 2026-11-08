//! Client settings in `config.json` (see [`crate::paths`]). The API token
//! is not stored here but in the system keyring (see [`crate::secret`]).

use std::path::{Path, PathBuf};

use scrn_proto::Visibility;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("the server address must start with https:// or http://")]
    Scheme,
    #[error("the server address is no valid URL")]
    Url,
    #[error("the server address must not contain a query or fragment")]
    Extra,
    #[error("an expiry of more than 3650 days is not allowed")]
    Expiry,
    #[error("reading or writing the settings failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("the settings file is damaged: {0}")]
    Json(#[from] serde_json::Error),
}

impl ConfigError {
    /// A sentence for the user in the system language.
    pub fn describe(&self) -> String {
        let key = match self {
            Self::Scheme => "error.url_scheme",
            Self::Url => "error.url",
            Self::Extra => "error.url_extra",
            Self::Expiry => "error.expiry",
            Self::Io(_) | Self::Json(_) => return self.to_string(),
        };
        crate::i18n::t(key).to_owned()
    }
}

/// Longest link lifetime the server accepts.
pub const MAX_EXPIRY_DAYS: u32 = 3650;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Base URL of the server without trailing slash; empty until set up.
    pub server_url: String,
    /// Visibility of new uploads.
    pub visibility: Visibility,
    /// Link lifetime of public uploads in days, 0 for never; `None` uses
    /// the default of the account on the server.
    pub expires_in_days: Option<u32>,
    /// Also save every upload under `~/Pictures/SCRN`.
    pub keep_copy: bool,
    /// Start the client with the desktop session.
    pub autostart: bool,
    /// Keyboard shortcut for a screenshot, e.g. `PrintScreen` or
    /// `Ctrl+Shift+S`; empty for none. Only used where the client
    /// registers shortcuts itself (Windows); on Linux the desktop's own
    /// shortcut settings call `scrn capture`.
    pub shortcut: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            visibility: Visibility::Private,
            expires_in_days: None,
            keep_copy: false,
            autostart: false,
            shortcut: DEFAULT_SHORTCUT.to_owned(),
        }
    }
}

pub const DEFAULT_SHORTCUT: &str = "PrintScreen";

impl Settings {
    pub fn is_configured(&self) -> bool {
        !self.server_url.is_empty()
    }

    /// Checks and normalizes the values entered in the settings window.
    pub fn validated(mut self) -> Result<Self, ConfigError> {
        self.server_url = if self.server_url.trim().is_empty() {
            String::new()
        } else {
            normalize_server_url(&self.server_url)?
        };
        if self.expires_in_days.is_some_and(|d| d > MAX_EXPIRY_DAYS) {
            return Err(ConfigError::Expiry);
        }
        self.shortcut = self.shortcut.trim().chars().take(64).collect();
        Ok(self)
    }
}

/// Trims the input, checks scheme and host and drops trailing slashes.
pub fn normalize_server_url(input: &str) -> Result<String, ConfigError> {
    let input = input.trim();
    let lower = input.to_ascii_lowercase();
    if !lower.starts_with("https://") && !lower.starts_with("http://") {
        return Err(ConfigError::Scheme);
    }
    let url = reqwest::Url::parse(input).map_err(|_| ConfigError::Url)?;
    if url.host_str().is_none_or(str::is_empty) || !url.username().is_empty() {
        return Err(ConfigError::Url);
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(ConfigError::Extra);
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// True if the token would travel unencrypted: plain HTTP to a host other
/// than this machine.
pub fn is_insecure(server_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(server_url) else {
        return false;
    };
    let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
    let local = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    url.scheme() == "http" && !local
}

pub fn path() -> Option<PathBuf> {
    crate::paths::config_file()
}

/// Loads the settings; a missing file gives the defaults.
pub fn load() -> Result<Settings, ConfigError> {
    match path() {
        Some(path) => load_from(&path),
        None => Ok(Settings::default()),
    }
}

pub fn load_from(path: &Path) -> Result<Settings, ConfigError> {
    match std::fs::read(path) {
        Ok(data) => Ok(serde_json::from_slice(&data)?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(err) => Err(err.into()),
    }
}

pub fn save(settings: &Settings) -> Result<(), ConfigError> {
    let path = path().ok_or_else(|| std::io::Error::other("no home directory"))?;
    save_to(&path, settings)
}

/// Writes through a temporary file, so a crash never leaves half a file.
pub fn save_to(path: &Path, settings: &Settings) -> Result<(), ConfigError> {
    write_atomic(path, &serde_json::to_vec_pretty(settings)?)
}

pub fn write_atomic(path: &Path, data: &[u8]) -> Result<(), ConfigError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn server_urls_are_normalized() {
        for (input, expected) in [
            ("https://scrn.dev", "https://scrn.dev"),
            ("  https://scrn.dev/  ", "https://scrn.dev"),
            ("HTTPS://Scrn.Dev//", "https://scrn.dev"),
            ("http://192.168.42.10:8080/", "http://192.168.42.10:8080"),
            ("https://example.org/scrn/", "https://example.org/scrn"),
        ] {
            assert_eq!(normalize_server_url(input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn bad_server_urls_are_rejected() {
        assert!(matches!(
            normalize_server_url("scrn.dev"),
            Err(ConfigError::Scheme)
        ));
        assert!(matches!(
            normalize_server_url("ftp://scrn.dev"),
            Err(ConfigError::Scheme)
        ));
        assert!(matches!(
            normalize_server_url("https://"),
            Err(ConfigError::Url)
        ));
        assert!(matches!(
            normalize_server_url("https://a:b@scrn.dev"),
            Err(ConfigError::Url)
        ));
        assert!(matches!(
            normalize_server_url("https://scrn.dev/?x=1"),
            Err(ConfigError::Extra)
        ));
        assert!(matches!(
            normalize_server_url("https://scrn.dev/#a"),
            Err(ConfigError::Extra)
        ));
    }

    #[test]
    fn plain_http_is_insecure_except_on_this_machine() {
        assert!(is_insecure("http://192.168.42.10:8080"));
        assert!(is_insecure("http://scrn.dev"));
        assert!(!is_insecure("https://scrn.dev"));
        assert!(!is_insecure("http://localhost:8081"));
        assert!(!is_insecure("http://127.0.0.1:8081"));
        assert!(!is_insecure("http://[::1]:8081"));
    }

    #[test]
    fn settings_are_validated() {
        let ok = Settings {
            server_url: " https://scrn.dev/ ".into(),
            expires_in_days: Some(7),
            ..Settings::default()
        };
        assert_eq!(ok.validated().unwrap().server_url, "https://scrn.dev");
        let empty = Settings {
            server_url: "   ".into(),
            ..Settings::default()
        };
        assert!(!empty.validated().unwrap().is_configured());
        let long = Settings {
            expires_in_days: Some(MAX_EXPIRY_DAYS + 1),
            ..Settings::default()
        };
        assert!(matches!(long.validated(), Err(ConfigError::Expiry)));
    }

    #[test]
    fn settings_round_trip_and_default_when_missing() {
        let dir = tempdir();
        let path = dir.join("scrn/config.json");
        assert_eq!(load_from(&path).unwrap(), Settings::default());
        let settings = Settings {
            server_url: "https://scrn.dev".into(),
            visibility: Visibility::Public,
            expires_in_days: Some(0),
            keep_copy: true,
            autostart: true,
            shortcut: "Ctrl+Shift+S".into(),
        };
        save_to(&path, &settings).unwrap();
        assert_eq!(load_from(&path).unwrap(), settings);
        // Older files without newer fields still load.
        std::fs::write(&path, r#"{"server_url":"https://a.example"}"#).unwrap();
        let old = load_from(&path).unwrap();
        assert_eq!(old.server_url, "https://a.example");
        assert_eq!(old.visibility, Visibility::Private);
        assert_eq!(old.shortcut, DEFAULT_SHORTCUT);
        std::fs::remove_dir_all(dir).unwrap();
    }

    pub(crate) fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "scrn-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
