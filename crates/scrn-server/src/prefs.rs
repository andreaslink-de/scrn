//! Language and theme of a request.
//!
//! Resolution order for the language: the user's setting, then the
//! `scrn_lang` cookie, then `Accept-Language`, then German. The theme comes
//! from the user's setting, then the `scrn_theme` cookie, then dark. The
//! cookies are set on logged-out pages through the footer switch and on
//! login and settings changes, so error pages, which are rendered without a
//! database lookup, also match the user's choice.

use std::convert::Infallible;

use axum::extract::FromRequestParts;
use axum::http::header::ACCEPT_LANGUAGE;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue};

use crate::cookies::{self, CookieSpec, SameSite};
use crate::i18n::DEFAULT_LANGUAGE;
use crate::repo::users::{Language, Theme, User};

pub const LANG_COOKIE: &str = "scrn_lang";
pub const THEME_COOKIE: &str = "scrn_theme";
const COOKIE_MAX_AGE: i64 = 365 * 24 * 60 * 60;
pub const DEFAULT_THEME: Theme = Theme::Dark;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefs {
    pub lang: Language,
    pub theme: Theme,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            lang: DEFAULT_LANGUAGE,
            theme: DEFAULT_THEME,
        }
    }
}

impl Prefs {
    /// Preferences of a logged-in user.
    pub fn of_user(user: &User) -> Self {
        Self {
            lang: user.language,
            theme: user.theme,
        }
    }

    /// Preferences of a logged-out request: cookies, then `Accept-Language`.
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let lang = cookies::get(headers, LANG_COOKIE)
            .and_then(parse_language)
            .or_else(|| {
                headers
                    .get(ACCEPT_LANGUAGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(accept_language)
            })
            .unwrap_or(DEFAULT_LANGUAGE);
        let theme = cookies::get(headers, THEME_COOKIE)
            .and_then(parse_theme)
            .unwrap_or(DEFAULT_THEME);
        Self { lang, theme }
    }

    /// `Set-Cookie` headers that store these preferences in the browser.
    /// They are not secret, but HttpOnly anyway since no script reads them.
    pub fn cookies(&self, secure: bool) -> Vec<HeaderValue> {
        [
            (LANG_COOKIE, language_code(self.lang)),
            (THEME_COOKIE, theme_code(self.theme)),
        ]
        .into_iter()
        .filter_map(|(name, value)| {
            CookieSpec {
                name,
                secure,
                same_site: SameSite::Lax,
                max_age: Some(COOKIE_MAX_AGE),
            }
            .set(value)
        })
        .collect()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Prefs {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Infallible> {
        Ok(Self::from_headers(&parts.headers))
    }
}

pub fn language_code(lang: Language) -> &'static str {
    match lang {
        Language::De => "de",
        Language::En => "en",
    }
}

pub fn theme_code(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => "dark",
        Theme::Light => "light",
    }
}

pub fn parse_language(value: &str) -> Option<Language> {
    match value {
        "de" => Some(Language::De),
        "en" => Some(Language::En),
        _ => None,
    }
}

pub fn parse_theme(value: &str) -> Option<Theme> {
    match value {
        "dark" => Some(Theme::Dark),
        "light" => Some(Theme::Light),
        _ => None,
    }
}

/// Picks the supported language with the highest quality from an
/// `Accept-Language` header (RFC 9110). Ties keep the header order.
fn accept_language(header: &str) -> Option<Language> {
    let mut best: Option<(Language, u16)> = None;
    for item in header.split(',') {
        let mut parts = item.split(';');
        let tag = parts.next().unwrap_or("").trim();
        let quality = parts
            .find_map(|p| p.trim().strip_prefix("q="))
            .map_or(Some(1000), parse_quality);
        let Some(quality) = quality.filter(|q| *q > 0) else {
            continue;
        };
        let primary = tag.split('-').next().unwrap_or("").to_ascii_lowercase();
        let Some(lang) = parse_language(&primary) else {
            continue;
        };
        if best.is_none_or(|(_, q)| quality > q) {
            best = Some((lang, quality));
        }
    }
    best.map(|(lang, _)| lang)
}

/// Parses a q-value (`0`, `0.5`, `1.000`) into thousandths.
fn parse_quality(value: &str) -> Option<u16> {
    let (int, frac) = value.split_once('.').unwrap_or((value, ""));
    if frac.len() > 3 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let int: u16 = match int {
        "0" => 0,
        "1" => 1,
        _ => return None,
    };
    let frac: u16 = format!("{frac:0<3}").parse().ok()?;
    let q = int * 1000 + frac;
    (q <= 1000).then_some(q)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::COOKIE;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(*name, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn accept_language_parsing() {
        assert_eq!(accept_language("en-US,en;q=0.9"), Some(Language::En));
        assert_eq!(accept_language("de-AT"), Some(Language::De));
        assert_eq!(
            accept_language("fr, en;q=0.5, de;q=0.8"),
            Some(Language::De)
        );
        assert_eq!(accept_language("en;q=0.5, de;q=0.5"), Some(Language::En));
        assert_eq!(accept_language("de;q=0, en;q=0.1"), Some(Language::En));
        assert_eq!(accept_language("fr, es"), None);
        assert_eq!(accept_language("*"), None);
        assert_eq!(accept_language("en;q=2"), None);
        assert_eq!(accept_language(""), None);
    }

    #[test]
    fn quality_values() {
        assert_eq!(parse_quality("1"), Some(1000));
        assert_eq!(parse_quality("1.000"), Some(1000));
        assert_eq!(parse_quality("0.5"), Some(500));
        assert_eq!(parse_quality("0.123"), Some(123));
        assert_eq!(parse_quality("1.5"), None);
        assert_eq!(parse_quality("0.1234"), None);
        assert_eq!(parse_quality("x"), None);
    }

    #[test]
    fn resolution_order_for_logged_out_requests() {
        assert_eq!(Prefs::from_headers(&HeaderMap::new()), Prefs::default());
        assert_eq!(Prefs::default().lang, Language::De);
        assert_eq!(Prefs::default().theme, Theme::Dark);

        let h = headers(&[("accept-language", "en")]);
        assert_eq!(Prefs::from_headers(&h).lang, Language::En);

        // The cookie wins over Accept-Language.
        let h = headers(&[
            ("accept-language", "en"),
            ("cookie", "scrn_lang=de; scrn_theme=light"),
        ]);
        assert_eq!(
            Prefs::from_headers(&h),
            Prefs {
                lang: Language::De,
                theme: Theme::Light
            }
        );

        // Invalid cookie values are ignored.
        let h = headers(&[("cookie", "scrn_lang=fr; scrn_theme=pink")]);
        assert_eq!(Prefs::from_headers(&h), Prefs::default());
        let _ = COOKIE;
    }

    #[test]
    fn cookies_round_trip() {
        let prefs = Prefs {
            lang: Language::En,
            theme: Theme::Light,
        };
        let set = prefs.cookies(true);
        assert_eq!(set.len(), 2);
        let first = set[0].to_str().unwrap();
        assert!(first.starts_with("scrn_lang=en;"), "{first}");
        assert!(first.contains("Secure"));
        assert!(set[1].to_str().unwrap().starts_with("scrn_theme=light;"));
    }
}
