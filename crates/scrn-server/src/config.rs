//! Server configuration read from environment variables.
//!
//! Every variable is documented in `.env.example`.

use std::net::SocketAddr;
use std::path::PathBuf;

use crate::net::{IpNet, TrustedProxies};

const DEFAULT_BASE_URL: &str = "http://localhost:8080";
const DEFAULT_BIND: &str = "0.0.0.0:8080";
const DEFAULT_DATA_DIR: &str = "/data";

/// Errors raised while reading the configuration.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{name} must start with http:// or https:// and contain a host, got {value:?}")]
    InvalidBaseUrl { name: &'static str, value: String },
    #[error("{name} must be a socket address such as 0.0.0.0:8080, got {value:?}")]
    InvalidBind { name: &'static str, value: String },
    #[error("{name} must not be empty")]
    Empty { name: &'static str },
    #[error("{name} must be true or false, got {value:?}")]
    InvalidBool { name: &'static str, value: String },
    #[error("{name} contains an invalid IP address or CIDR range: {value:?}")]
    InvalidProxy { name: &'static str, value: String },
}

/// Runtime configuration of the server.
#[derive(Debug, Clone)]
pub struct Config {
    /// Public base URL without a trailing slash, e.g. `https://scrn.dev`.
    pub base_url: String,
    /// Address the HTTP server listens on.
    pub bind: SocketAddr,
    /// Directory for the SQLite database and the screenshot files.
    pub data_dir: PathBuf,
    /// Whether session cookies get the `Secure` attribute.
    pub cookie_secure: bool,
    /// Reverse proxies whose `X-Forwarded-*` headers are trusted.
    pub trusted_proxies: TrustedProxies,
}

impl Config {
    /// Reads the configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads the configuration through a lookup function, which keeps tests
    /// independent of the process environment.
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        // Treat unset and empty variables the same way.
        let get = |name: &str| {
            lookup(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };

        let base_url = parse_base_url(
            "SCRN_BASE_URL",
            &get("SCRN_BASE_URL").unwrap_or_else(|| DEFAULT_BASE_URL.to_owned()),
        )?;

        let bind_raw = get("SCRN_BIND").unwrap_or_else(|| DEFAULT_BIND.to_owned());
        let bind = bind_raw.parse().map_err(|_| ConfigError::InvalidBind {
            name: "SCRN_BIND",
            value: bind_raw.clone(),
        })?;

        let data_dir =
            PathBuf::from(get("SCRN_DATA_DIR").unwrap_or_else(|| DEFAULT_DATA_DIR.to_owned()));

        let cookie_secure = match get("SCRN_COOKIE_SECURE") {
            None => true,
            Some(value) => parse_bool("SCRN_COOKIE_SECURE", &value)?,
        };

        let trusted_proxies = match get("SCRN_TRUSTED_PROXIES") {
            None => TrustedProxies::default(),
            Some(value) => parse_proxies("SCRN_TRUSTED_PROXIES", &value)?,
        };

        Ok(Self {
            base_url,
            bind,
            data_dir,
            cookie_secure,
            trusted_proxies,
        })
    }

    /// Path of the SQLite database file.
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("scrn.db")
    }

    /// Directory that holds the screenshot files.
    pub fn files_dir(&self) -> PathBuf {
        self.data_dir.join("files")
    }
}

fn parse_base_url(name: &'static str, value: &str) -> Result<String, ConfigError> {
    let trimmed = value.trim_end_matches('/');
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"));
    match rest {
        // Require a host and reject paths, queries and whitespace: short links
        // live directly under the domain.
        Some(host)
            if !host.is_empty()
                && !host.contains(['/', '?', '#', '@'])
                && !host.chars().any(char::is_whitespace) =>
        {
            Ok(trimmed.to_owned())
        }
        _ => Err(ConfigError::InvalidBaseUrl {
            name,
            value: value.to_owned(),
        }),
    }
}

fn parse_bool(name: &'static str, value: &str) -> Result<bool, ConfigError> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::InvalidBool {
            name,
            value: value.to_owned(),
        }),
    }
}

fn parse_proxies(name: &'static str, value: &str) -> Result<TrustedProxies, ConfigError> {
    let mut nets = Vec::new();
    for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let net: IpNet = part.parse().map_err(|_| ConfigError::InvalidProxy {
            name,
            value: part.to_owned(),
        })?;
        nets.push(net);
    }
    if nets.is_empty() {
        return Err(ConfigError::Empty { name });
    }
    Ok(TrustedProxies::new(nets))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Config::from_lookup(|name| map.get(name).cloned())
    }

    #[test]
    fn defaults_apply_when_nothing_is_set() {
        let cfg = config(&[]).unwrap();
        assert_eq!(cfg.base_url, "http://localhost:8080");
        assert_eq!(cfg.bind, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(cfg.data_dir, PathBuf::from("/data"));
        assert!(cfg.cookie_secure);
        assert!(cfg.trusted_proxies.is_empty());
        assert_eq!(cfg.database_path(), PathBuf::from("/data/scrn.db"));
        assert_eq!(cfg.files_dir(), PathBuf::from("/data/files"));
    }

    #[test]
    fn empty_values_count_as_unset() {
        let cfg = config(&[("SCRN_BIND", "  "), ("SCRN_TRUSTED_PROXIES", "")]).unwrap();
        assert_eq!(cfg.bind, "0.0.0.0:8080".parse().unwrap());
        assert!(cfg.trusted_proxies.is_empty());
    }

    #[test]
    fn reads_all_values() {
        let cfg = config(&[
            ("SCRN_BASE_URL", "https://scrn.dev/"),
            ("SCRN_BIND", "127.0.0.1:9000"),
            ("SCRN_DATA_DIR", "/tmp/scrn"),
            ("SCRN_COOKIE_SECURE", "false"),
            ("SCRN_TRUSTED_PROXIES", "172.18.0.0/16, 10.0.0.5"),
        ])
        .unwrap();
        assert_eq!(cfg.base_url, "https://scrn.dev");
        assert_eq!(cfg.bind, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(cfg.data_dir, PathBuf::from("/tmp/scrn"));
        assert!(!cfg.cookie_secure);
        assert!(cfg.trusted_proxies.contains("172.18.3.4".parse().unwrap()));
        assert!(cfg.trusted_proxies.contains("10.0.0.5".parse().unwrap()));
        assert!(!cfg.trusted_proxies.contains("10.0.0.6".parse().unwrap()));
    }

    #[test]
    fn rejects_invalid_base_urls() {
        for value in [
            "scrn.dev",
            "ftp://scrn.dev",
            "https://",
            "https://scrn.dev/x",
            "https://a b",
        ] {
            assert!(
                matches!(
                    config(&[("SCRN_BASE_URL", value)]),
                    Err(ConfigError::InvalidBaseUrl { .. })
                ),
                "{value} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_invalid_bind() {
        assert!(matches!(
            config(&[("SCRN_BIND", "8080")]),
            Err(ConfigError::InvalidBind { .. })
        ));
    }

    #[test]
    fn parses_booleans() {
        for (value, expected) in [("1", true), ("YES", true), ("off", false), ("False", false)] {
            let cfg = config(&[("SCRN_COOKIE_SECURE", value)]).unwrap();
            assert_eq!(cfg.cookie_secure, expected);
        }
        assert!(matches!(
            config(&[("SCRN_COOKIE_SECURE", "maybe")]),
            Err(ConfigError::InvalidBool { .. })
        ));
    }

    #[test]
    fn rejects_invalid_proxies() {
        assert!(matches!(
            config(&[("SCRN_TRUSTED_PROXIES", "10.0.0.0/33")]),
            Err(ConfigError::InvalidProxy { .. })
        ));
        assert!(matches!(
            config(&[("SCRN_TRUSTED_PROXIES", " , ")]),
            Err(ConfigError::Empty { .. })
        ));
    }
}
