//! Server-rendered HTML: the page context shared by all templates, the
//! embedded static assets and the response headers of HTML pages.
//!
//! Templates live in `crates/scrn-server/templates` (askama, checked at
//! compile time, HTML-escaped by default). Every visible text comes from the
//! translation catalogs through [`Page::t`].

use std::sync::LazyLock;

use askama::Template;
use axum::extract::{Path, Query};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::auth::CurrentUser;
use crate::crypto;
use crate::error::AppError;
use crate::i18n;
use crate::prefs::{self, Prefs};
use crate::repo::users::Language;

/// Policy for HTML pages: only our own scripts, styles and images, no
/// framing, forms only to this origin.
pub const PAGE_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
     img-src 'self' data:; connect-src 'self'; form-action 'self'; \
     frame-ancestors 'none'; base-uri 'none'";

/// Navigation of logged-in pages.
#[derive(Debug, Clone)]
pub struct Nav {
    pub username: String,
    pub is_admin: bool,
    /// CSRF token of the session, for the logout button.
    pub csrf: String,
    /// Path of the active navigation entry.
    pub active: &'static str,
}

/// Context available to every template as `page`.
#[derive(Debug, Clone)]
pub struct Page {
    pub prefs: Prefs,
    pub title_key: &'static str,
    pub nav: Option<Nav>,
    /// CSRF token for the language and theme switch of logged-out pages.
    /// Without it the switch is not shown.
    pub switch_csrf: Option<String>,
    /// Where the switch returns to.
    pub back: String,
    /// Narrow single-column layout for forms like the login.
    pub narrow: bool,
}

impl Page {
    /// A logged-out page.
    pub fn public(
        prefs: Prefs,
        title_key: &'static str,
        switch_csrf: Option<String>,
        back: &str,
    ) -> Self {
        Self {
            prefs,
            title_key,
            nav: None,
            switch_csrf,
            back: back.to_owned(),
            narrow: true,
        }
    }

    /// A page for a logged-in user.
    pub fn user(user: &CurrentUser, title_key: &'static str, active: &'static str) -> Self {
        Self {
            prefs: Prefs::of_user(&user.user),
            title_key,
            nav: Some(Nav {
                username: user.user.username.clone(),
                is_admin: user.is_admin(),
                csrf: user.csrf.clone(),
                active,
            }),
            switch_csrf: None,
            back: active.to_owned(),
            narrow: false,
        }
    }

    pub fn lang(&self) -> Language {
        self.prefs.lang
    }

    /// Translated message (escaped by the template).
    pub fn t<'a>(&self, key: &'a str) -> &'a str {
        i18n::catalogs().t(self.prefs.lang, key)
    }

    /// Translated message with one `{name}` placeholder.
    pub fn tf(&self, key: &str, name: &str, value: &str) -> String {
        i18n::catalogs().tf(self.prefs.lang, key, &[(name, value)])
    }

    /// Translated message with two placeholders.
    pub fn tf2(&self, key: &str, n1: &str, v1: &str, n2: &str, v2: &str) -> String {
        i18n::catalogs().tf(self.prefs.lang, key, &[(n1, v1), (n2, v2)])
    }

    pub fn lang_code(&self) -> &'static str {
        prefs::language_code(self.prefs.lang)
    }

    pub fn theme_code(&self) -> &'static str {
        prefs::theme_code(self.prefs.theme)
    }

    pub fn is_dark(&self) -> bool {
        self.prefs.theme == crate::repo::users::Theme::Dark
    }

    /// URL of an embedded asset, versioned by its content hash.
    pub fn asset(&self, name: &str) -> String {
        asset_url(name)
    }

    pub fn time(&self, ts: impl Int) -> String {
        format_time(ts.int())
    }

    /// The `datetime` value of a `<time>` element; app.js uses it to show
    /// the time in the browser's time zone.
    pub fn iso(&self, ts: impl Int) -> String {
        format_iso(ts.int())
    }

    pub fn bytes(&self, n: impl Int) -> String {
        format_bytes(n.int(), self.prefs.lang)
    }

    pub fn is_active(&self, path: &str) -> bool {
        self.nav.as_ref().is_some_and(|nav| nav.active == path)
    }
}

/// Integers passed from templates, which hand over references.
pub trait Int {
    fn int(&self) -> i64;
}

impl Int for i64 {
    fn int(&self) -> i64 {
        *self
    }
}

impl<T: Int + ?Sized> Int for &T {
    fn int(&self) -> i64 {
        (**self).int()
    }
}

/// Storage usage for the progress bar template (`usage.html`).
#[derive(Debug, Clone, Copy)]
pub struct Usage {
    pub used: i64,
    /// `None` means unlimited.
    pub quota: Option<i64>,
    pub percent: i64,
}

impl Usage {
    pub fn new(used: i64, quota: Option<i64>) -> Self {
        Self {
            used,
            quota,
            percent: quota.map_or(0, |q| percent(used, q)),
        }
    }
}

/// Renders a template into an HTML response.
pub fn render(status: StatusCode, template: &impl Template) -> Result<Response, AppError> {
    let html = template
        .render()
        .map_err(|err| AppError::Internal(format!("template error: {err}")))?;
    Ok(response(status, html))
}

/// Wraps rendered HTML in a response with the security headers of pages.
/// Pages may contain one-time secrets or per-user data, so they are never
/// cached.
pub fn response(status: StatusCode, html: String) -> Response {
    let mut response = (status, html).into_response();
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(PAGE_CSP));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("same-origin"));
    response
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate<'a> {
    page: Page,
    message: &'a str,
}

/// An error page with a translated message. Never fails: if the template
/// cannot be rendered, a plain status response is returned.
pub fn error_page(prefs: Prefs, status: StatusCode, message_key: &str) -> Response {
    let page = Page::public(prefs, "error.title", None, "/");
    let message = i18n::catalogs().t(prefs.lang, message_key);
    match (ErrorTemplate { page, message }).render() {
        Ok(html) => response(status, html),
        Err(err) => {
            tracing::error!(error = %err, "rendering the error page failed");
            status.into_response()
        }
    }
}

// --- Static assets -----------------------------------------------------------

struct Asset {
    name: &'static str,
    content_type: &'static str,
    data: &'static [u8],
}

const ASSETS: &[Asset] = &[
    Asset {
        name: "app.css",
        content_type: "text/css; charset=utf-8",
        data: include_bytes!("../static/app.css"),
    },
    Asset {
        name: "htmx.min.js",
        content_type: "text/javascript; charset=utf-8",
        data: include_bytes!("../static/htmx.min.js"),
    },
    Asset {
        name: "app.js",
        content_type: "text/javascript; charset=utf-8",
        data: include_bytes!("../static/app.js"),
    },
    Asset {
        name: "favicon.svg",
        content_type: "image/svg+xml",
        data: include_bytes!("../static/favicon.svg"),
    },
];

/// Short content hashes, used as cache-busting versions.
static VERSIONS: LazyLock<Vec<String>> = LazyLock::new(|| {
    ASSETS
        .iter()
        .map(|a| crypto::hex_encode(&crypto::sha256(a.data)[..6]))
        .collect()
});

fn find_asset(name: &str) -> Option<(&'static Asset, &'static str)> {
    ASSETS
        .iter()
        .zip(VERSIONS.iter())
        .find(|(a, _)| a.name == name)
        .map(|(a, v)| (a, v.as_str()))
}

pub fn asset_url(name: &str) -> String {
    match find_asset(name) {
        Some((asset, version)) => format!("/static/{}?v={version}", asset.name),
        None => format!("/static/{name}"),
    }
}

#[derive(Deserialize)]
pub struct AssetQuery {
    v: Option<String>,
}

/// `GET /static/{file}`. Requests with the current version may be cached
/// forever; others are revalidated.
pub async fn serve_static(Path(file): Path<String>, Query(query): Query<AssetQuery>) -> Response {
    let Some((asset, version)) = find_asset(&file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let cache = if query.v.as_deref() == Some(version) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut response = asset.data.into_response();
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(asset.content_type));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

// --- Formatting ---------------------------------------------------------------

/// Formats a Unix timestamp as `YYYY-MM-DD HH:MM UTC`.
pub fn format_time(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// Formats a Unix timestamp as `YYYY-MM-DDTHH:MM:SSZ` (RFC 3339, UTC).
pub fn format_iso(ts: i64) -> String {
    let secs = ts.rem_euclid(86_400);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        format_date(ts),
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// Formats the UTC date of a Unix timestamp as `YYYY-MM-DD`, the value
/// format of `<input type="date">`.
pub fn format_date(ts: i64) -> String {
    let (y, m, d) = civil_from_days(ts.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Parses `YYYY-MM-DD` into days since 1970-01-01. Rejects dates that do
/// not exist, such as 2026-02-30.
pub fn parse_date(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let y: i64 = y.parse().ok()?;
    let m: i64 = m.parse().ok()?;
    let d: i64 = d.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let days = days_from_civil(y, m, d);
    // Round trip to reject days beyond the end of the month.
    (civil_from_days(days) == (y, m, d)).then_some(days)
}

/// Inverse of [`civil_from_days`] (Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Converts days since 1970-01-01 to a proleptic Gregorian date
/// (algorithm by Howard Hinnant).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Formats a byte count with binary units and one decimal, using the
/// decimal separator of the language: `1,5 MiB` or `1.5 MiB`.
pub fn format_bytes(bytes: i64, lang: Language) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let bytes = bytes.max(0);
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut unit = 0;
    let mut scaled = bytes;
    while scaled >= 1024 * 1024 && unit < UNITS.len() - 2 {
        scaled /= 1024;
        unit += 1;
    }
    // `scaled` is in units of UNITS[unit]; show it in the next unit with
    // one decimal, rounded.
    let tenths = (scaled * 10 + 512) / 1024;
    let sep = match lang {
        Language::De => ',',
        Language::En => '.',
    };
    format!("{}{sep}{} {}", tenths / 10, tenths % 10, UNITS[unit + 1])
}

/// Percentage for progress bars, clamped to 0..=100.
pub fn percent(used: i64, limit: i64) -> i64 {
    if limit <= 0 {
        return 0;
    }
    (used.max(0).saturating_mul(100) / limit).min(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_timestamps() {
        assert_eq!(format_time(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(format_time(1_793_914_620), "2026-11-05 21:37 UTC");
        assert_eq!(format_time(-60), "1969-12-31 23:59 UTC");
        assert_eq!(format_iso(1_793_914_621), "2026-11-05T21:37:01Z");
        assert_eq!(format_iso(-1), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn dates() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(1_793_914_620), "2026-11-05");
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("2000-02-29"), Some(951_782_400 / 86_400));
        assert_eq!(parse_date("2026-11-05"), Some(1_793_914_620 / 86_400));
        for bad in [
            "",
            "2026-02-30",
            "2025-02-29",
            "2026-13-01",
            "26-01-01",
            "2026-1-01",
            "x",
        ] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
        for days in [-1000, 0, 10_957, 20_397, 100_000] {
            assert_eq!(parse_date(&format_date(days * 86_400)), Some(days));
        }
    }

    #[test]
    fn formats_bytes() {
        assert_eq!(format_bytes(0, Language::De), "0 B");
        assert_eq!(format_bytes(1023, Language::En), "1023 B");
        assert_eq!(format_bytes(1024, Language::En), "1.0 KiB");
        assert_eq!(format_bytes(1536, Language::De), "1,5 KiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024, Language::En), "1.0 GiB");
        assert_eq!(
            format_bytes(5 * 1024 * 1024 + 512 * 1024, Language::En),
            "5.5 MiB"
        );
        assert_eq!(format_bytes(-5, Language::En), "0 B");
        assert_eq!(format_bytes(i64::MAX, Language::En), "8388608.0 TiB");
    }

    #[test]
    fn percentages() {
        assert_eq!(percent(0, 100), 0);
        assert_eq!(percent(50, 100), 50);
        assert_eq!(percent(500, 100), 100);
        assert_eq!(percent(5, 0), 0);
    }

    #[test]
    fn assets_are_versioned() {
        let url = asset_url("app.css");
        assert!(url.starts_with("/static/app.css?v="), "{url}");
        assert_eq!(url.len(), "/static/app.css?v=".len() + 12);
        assert_eq!(asset_url("missing.js"), "/static/missing.js");
    }

    #[test]
    fn error_page_is_translated() {
        let prefs = Prefs {
            lang: Language::En,
            theme: crate::repo::users::Theme::Light,
        };
        let response = error_page(prefs, StatusCode::NOT_FOUND, "error.not_found");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[CONTENT_SECURITY_POLICY], PAGE_CSP);
    }
}
