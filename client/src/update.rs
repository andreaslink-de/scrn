//! Looks for a newer client release on GitHub: at start and then once a
//! day, unless the user turned it off in the settings. A newer version
//! shows as an entry in the tray menu and once as a notification; the
//! entry opens the release page. Nothing is downloaded or installed.

use std::time::Duration;

use serde::Deserialize;
use tauri::{AppHandle, Manager};

use crate::i18n::t;
use crate::{AppState, PROJECT_URL, VERSION, desktop, lock, refresh_tray, report};

/// The public list of releases; server and client share the repository.
const RELEASES_API: &str = "https://api.github.com/repos/andreaslink-de/scrn/releases?per_page=30";
/// Client releases are tagged `client-vX.Y.Z`.
const TAG_PREFIX: &str = "client-v";
const FIRST_CHECK: Duration = Duration::from_secs(30);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// A newer client release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub version: String,
    /// The release page; always below the project's releases.
    pub url: String,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Debug, thiserror::Error)]
enum UpdateError {
    #[error("network: {0}")]
    Network(String),
    #[error("GitHub answered {0}")]
    Status(u16),
}

impl From<reqwest::Error> for UpdateError {
    fn from(err: reqwest::Error) -> Self {
        Self::Network(err.without_url().to_string())
    }
}

/// Starts the daily check in the background.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            check(&app).await;
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

/// Checks once now, for example right after the user turned it on.
pub fn check_soon(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { check(&app).await });
}

async fn check(app: &AppHandle) {
    let state = app.state::<AppState>();
    if !lock(&state.settings).check_updates {
        return;
    }
    let releases = match fetch().await {
        Ok(releases) => releases,
        Err(err) => {
            report::note(format!("update check failed: {err}"));
            return;
        }
    };
    let Some(found) = newest(&releases, VERSION) else {
        report::note(format!("update check: {VERSION} is current"));
        return;
    };
    let known = lock(&state.update).replace(found.clone());
    if known.as_ref() != Some(&found) {
        report::note(format!("update check: {} is available", found.version));
        refresh_tray(app);
        desktop::notify(
            &t("notify.update").replace("{0}", &found.version),
            t("notify.update_body"),
        );
    }
}

async fn fetch() -> Result<Vec<Release>, UpdateError> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("scrn-client/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()?;
    let response = client
        .get(RELEASES_API)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(UpdateError::Status(response.status().as_u16()));
    }
    Ok(response.json().await?)
}

/// The newest published client release above `current`, if any.
fn newest(releases: &[Release], current: &str) -> Option<Available> {
    let current = parse(current)?;
    releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter(|r| r.html_url.starts_with(&format!("{PROJECT_URL}/releases/")))
        .filter_map(|r| {
            let version = parse(r.tag_name.strip_prefix(TAG_PREFIX)?)?;
            Some((version, r))
        })
        .filter(|(version, _)| *version > current)
        .max_by_key(|(version, _)| *version)
        .map(|((major, minor, patch), r)| Available {
            version: format!("{major}.{minor}.{patch}"),
            url: r.html_url.clone(),
        })
}

/// `1.2.3` as numbers; anything else, such as `1.2.3-rc1`, is skipped.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> Release {
        Release {
            tag_name: tag.to_owned(),
            html_url: format!("{PROJECT_URL}/releases/tag/{tag}"),
            draft: false,
            prerelease: false,
        }
    }

    #[test]
    fn versions_are_parsed_strictly() {
        assert_eq!(parse("0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse("10.0.12"), Some((10, 0, 12)));
        assert_eq!(parse("0.2"), None);
        assert_eq!(parse("0.2.0.1"), None);
        assert_eq!(parse("0.3.0-rc1"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn only_newer_client_releases_count() {
        let releases = vec![
            release("v0.9.0"),
            release("client-v0.1.1"),
            release("client-v0.2.0"),
            release("client-v0.10.0"),
            release("client-v0.3.0"),
        ];
        let found = newest(&releases, "0.2.0").unwrap();
        assert_eq!(found.version, "0.10.0");
        assert!(found.url.ends_with("/client-v0.10.0"));
        assert!(newest(&releases, "0.10.0").is_none());
        assert!(newest(&releases, "1.0.0").is_none());
    }

    #[test]
    fn drafts_prereleases_and_foreign_links_are_skipped() {
        let mut draft = release("client-v0.4.0");
        draft.draft = true;
        let mut pre = release("client-v0.5.0");
        pre.prerelease = true;
        let mut foreign = release("client-v0.6.0");
        foreign.html_url = "https://example.com/releases/client-v0.6.0".to_owned();
        assert!(newest(&[draft, pre, foreign], "0.2.0").is_none());
    }

    #[test]
    fn github_answer_is_read() {
        let json = r#"[{"tag_name":"client-v0.3.0","html_url":"https://github.com/andreaslink-de/scrn/releases/tag/client-v0.3.0","draft":false,"prerelease":false,"assets":[]}]"#;
        let releases: Vec<Release> = serde_json::from_str(json).unwrap();
        assert_eq!(newest(&releases, "0.2.0").unwrap().version, "0.3.0");
    }

    #[test]
    fn texts_exist() {
        assert!(t("notify.update").contains("{0}"));
        assert!(t("tray.update").contains("{0}"));
        assert_ne!(t("notify.update_body"), "");
    }
}

#[cfg(test)]
mod live {
    /// Asks GitHub; needs the network.
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn reads_the_real_releases() {
        let releases = super::fetch().await.unwrap();
        println!(
            "{} releases, newest client: {:?}",
            releases.len(),
            super::newest(&releases, "0.0.1")
        );
    }
}
