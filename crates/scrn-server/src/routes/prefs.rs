//! `POST /prefs`: the language and theme switch of logged-out pages. The
//! choice is stored in cookies only.

use axum::Form;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::auth;
use crate::error::AppError;
use crate::prefs::{self, Prefs};

#[derive(Deserialize)]
pub struct SwitchForm {
    csrf: String,
    lang: String,
    theme: String,
    #[serde(default)]
    back: String,
}

pub async fn switch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SwitchForm>,
) -> Result<Response, AppError> {
    auth::verify_form_csrf(&headers, &state, &form.csrf)?;
    let lang = prefs::parse_language(&form.lang).ok_or(AppError::BadRequest)?;
    let theme = prefs::parse_theme(&form.theme).ok_or(AppError::BadRequest)?;
    let mut response = Redirect::to(safe_back(&form.back)).into_response();
    for cookie in (Prefs { lang, theme }).cookies(state.config.cookie_secure) {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    Ok(response)
}

/// Only local paths with harmless characters; everything else goes to `/`.
/// This prevents open redirects (`//evil.example`) and header injection.
fn safe_back(back: &str) -> &str {
    let ok = back.starts_with('/')
        && !back.starts_with("//")
        && back.len() <= 200
        && back
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b));
    if ok { back } else { "/" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_paths() {
        assert_eq!(safe_back("/login"), "/login");
        assert_eq!(safe_back("/invite/abc123"), "/invite/abc123");
        for bad in [
            "",
            "login",
            "//evil.example",
            "https://evil.example",
            "/a\\b",
            "/a?x=1",
            "/a\r\nSet-Cookie: x",
            "/\u{e4}",
        ] {
            assert_eq!(safe_back(bad), "/", "{bad:?}");
        }
    }
}
