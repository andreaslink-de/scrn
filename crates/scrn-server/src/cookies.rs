//! Minimal cookie handling. All values we set are hex strings, so no
//! escaping is needed.

use axum::http::header::COOKIE;
use axum::http::{HeaderMap, HeaderValue};

/// Returns the value of the first cookie called `name`.
pub fn get<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

/// `SameSite` attribute.
#[derive(Debug, Clone, Copy)]
pub enum SameSite {
    Strict,
    Lax,
}

/// Attributes of a cookie we set. Cookies are always `HttpOnly` and scoped
/// to `/`.
#[derive(Debug, Clone, Copy)]
pub struct CookieSpec<'a> {
    pub name: &'a str,
    pub secure: bool,
    pub same_site: SameSite,
    /// `None` creates a cookie that lives until the browser is closed.
    pub max_age: Option<i64>,
}

impl CookieSpec<'_> {
    /// Builds a `Set-Cookie` value. Returns `None` if `value` contains
    /// characters that are not allowed in a header.
    pub fn set(&self, value: &str) -> Option<HeaderValue> {
        let mut cookie = format!("{}={value}; Path=/; HttpOnly", self.name);
        cookie.push_str(match self.same_site {
            SameSite::Strict => "; SameSite=Strict",
            SameSite::Lax => "; SameSite=Lax",
        });
        if self.secure {
            cookie.push_str("; Secure");
        }
        if let Some(max_age) = self.max_age {
            cookie.push_str(&format!("; Max-Age={max_age}"));
        }
        HeaderValue::from_str(&cookie).ok()
    }

    /// Builds a `Set-Cookie` value that deletes the cookie.
    pub fn clear(&self) -> Option<HeaderValue> {
        Self {
            max_age: Some(0),
            ..*self
        }
        .set("")
    }
}

/// Cookie name with the `__Host-` prefix when cookies are secure. Browsers
/// only accept such cookies over HTTPS, without a domain and for path `/`,
/// which stops sibling subdomains from planting them.
pub fn name(base: &str, secure: bool) -> String {
    if secure {
        format!("__Host-{base}")
    } else {
        base.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cookies_from_all_headers() {
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, HeaderValue::from_static("a=1; b=2"));
        headers.append(COOKIE, HeaderValue::from_static("c=3"));
        assert_eq!(get(&headers, "a"), Some("1"));
        assert_eq!(get(&headers, "b"), Some("2"));
        assert_eq!(get(&headers, "c"), Some("3"));
        assert_eq!(get(&headers, "d"), None);
    }

    #[test]
    fn builds_set_cookie_values() {
        let spec = CookieSpec {
            name: "__Host-s",
            secure: true,
            same_site: SameSite::Lax,
            max_age: Some(60),
        };
        assert_eq!(
            spec.set("abc").unwrap(),
            "__Host-s=abc; Path=/; HttpOnly; SameSite=Lax; Secure; Max-Age=60"
        );
        assert_eq!(
            spec.clear().unwrap(),
            "__Host-s=; Path=/; HttpOnly; SameSite=Lax; Secure; Max-Age=0"
        );
        let insecure = CookieSpec {
            name: "c",
            secure: false,
            same_site: SameSite::Strict,
            max_age: None,
        };
        assert_eq!(
            insecure.set("x").unwrap(),
            "c=x; Path=/; HttpOnly; SameSite=Strict"
        );
        assert!(insecure.set("bad\nvalue").is_none());
    }

    #[test]
    fn host_prefix_only_when_secure() {
        assert_eq!(name("scrn_session", true), "__Host-scrn_session");
        assert_eq!(name("scrn_session", false), "scrn_session");
    }
}
