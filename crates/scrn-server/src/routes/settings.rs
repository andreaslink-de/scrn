//! The settings page: language, theme, default link expiry, storage usage
//! and API tokens.

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::auth::CurrentUser;
use crate::crypto::ApiTokenSecret;
use crate::error::AppError;
use crate::html::{self, Page, Usage};
use crate::i18n;
use crate::prefs::{self, Prefs};
use crate::repo::api_tokens::{self, ApiToken};
use crate::repo::{screenshots, users};

/// Token names: 1 to 100 characters, matching the database constraint.
pub const MAX_TOKEN_NAME_CHARS: usize = 100;
/// Longest custom link lifetime in days.
pub const MAX_EXPIRY_DAYS: i64 = 3650;
/// Expiry choices offered directly; anything else is "custom".
const EXPIRY_PRESETS: [i64; 4] = [0, 1, 7, 30];

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsPage {
    page: Page,
    csrf: String,
    saved: bool,
    error: Option<String>,
    expiry: String,
    custom_days: String,
    usage: Usage,
    new_token: Option<String>,
    token_error: Option<String>,
    token_name: String,
    tokens: Vec<ApiToken>,
}

#[derive(Deserialize)]
pub struct ShowQuery {
    saved: Option<String>,
}

#[derive(Deserialize)]
pub struct PrefsForm {
    csrf: String,
    lang: String,
    theme: String,
    expiry: String,
    #[serde(default)]
    custom_days: String,
}

#[derive(Deserialize)]
pub struct TokenForm {
    csrf: String,
    name: String,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
}

/// What to show besides the stored settings.
#[derive(Default)]
struct Extra {
    saved: bool,
    error: Option<String>,
    form: Option<(String, String)>,
    new_token: Option<String>,
    token_error: Option<String>,
    token_name: String,
}

pub async fn show(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(query): Query<ShowQuery>,
) -> Result<Response, AppError> {
    let extra = Extra {
        saved: query.saved.is_some(),
        ..Extra::default()
    };
    render(&state, &user, extra, StatusCode::OK).await
}

pub async fn save(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<PrefsForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    // The choices come from select and radio fields, so anything else is a
    // forged request.
    let lang = prefs::parse_language(&form.lang).ok_or(AppError::BadRequest)?;
    let theme = prefs::parse_theme(&form.theme).ok_or(AppError::BadRequest)?;
    let Some(days) = parse_expiry(&form.expiry, &form.custom_days)? else {
        let extra = Extra {
            error: Some(
                i18n::catalogs()
                    .t(user.user.language, "error.expiry_days")
                    .to_owned(),
            ),
            form: Some((form.expiry, form.custom_days)),
            ..Extra::default()
        };
        return render(&state, &user, extra, StatusCode::UNPROCESSABLE_ENTITY).await;
    };
    users::set_preferences(&state.db, user.user.id, lang, theme, days).await?;
    tracing::info!(user_id = user.user.id, "settings saved");
    let mut response = Redirect::to("/app/settings?saved=1").into_response();
    for cookie in (Prefs { lang, theme }).cookies(state.config.cookie_secure) {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    Ok(response)
}

/// Parses the expiry choice. `Ok(None)` means an invalid custom value.
fn parse_expiry(choice: &str, custom: &str) -> Result<Option<i64>, AppError> {
    if choice == "custom" {
        return Ok(custom
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|d| (1..=MAX_EXPIRY_DAYS).contains(d)));
    }
    choice
        .parse::<i64>()
        .ok()
        .filter(|d| EXPIRY_PRESETS.contains(d))
        .map(Some)
        .ok_or(AppError::BadRequest)
}

/// The values the expiry fields show for a stored setting.
fn expiry_fields(days: i64) -> (String, String) {
    if EXPIRY_PRESETS.contains(&days) {
        (days.to_string(), String::new())
    } else {
        ("custom".to_owned(), days.to_string())
    }
}

/// `GET /app/tokens`: tokens are managed on the settings page.
pub async fn tokens_redirect() -> Redirect {
    Redirect::to("/app/settings#tokens")
}

pub async fn create_token(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<TokenForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    let Some(name) = valid_token_name(&form.name) else {
        let extra = Extra {
            token_error: Some(
                i18n::catalogs()
                    .t(user.user.language, "error.token_name")
                    .to_owned(),
            ),
            token_name: form.name.clone(),
            ..Extra::default()
        };
        return render(&state, &user, extra, StatusCode::UNPROCESSABLE_ENTITY).await;
    };
    let secret = ApiTokenSecret::generate()?;
    let token = api_tokens::create(
        &state.db,
        user.user.id,
        name,
        &secret.hash,
        state.clock.now(),
    )
    .await?;
    tracing::info!(
        token_id = token.id,
        user_id = user.user.id,
        "API token created"
    );
    // Rendered directly instead of redirecting: only this response ever
    // contains the token.
    let extra = Extra {
        new_token: Some(secret.token),
        ..Extra::default()
    };
    render(&state, &user, extra, StatusCode::OK).await
}

pub async fn revoke_token(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    // Tokens of other users and already revoked ones are left untouched;
    // the redirect is the same either way.
    if api_tokens::revoke(&state.db, id, user.user.id, state.clock.now()).await? {
        tracing::info!(token_id = id, user_id = user.user.id, "API token revoked");
    }
    Ok(Redirect::to("/app/settings#tokens").into_response())
}

/// Trims the name and checks its length. Control characters are rejected.
fn valid_token_name(raw: &str) -> Option<&str> {
    let name = raw.trim();
    let chars = name.chars().count();
    ((1..=MAX_TOKEN_NAME_CHARS).contains(&chars) && !name.chars().any(char::is_control))
        .then_some(name)
}

async fn render(
    state: &AppState,
    user: &CurrentUser,
    extra: Extra,
    status: StatusCode,
) -> Result<Response, AppError> {
    let tokens = api_tokens::list_for_user(&state.db, user.user.id).await?;
    let used = screenshots::usage_for_user(&state.db, user.user.id).await?;
    let quota = user.user.effective_quota(state.config.default_quota_bytes);
    let (expiry, custom_days) = extra
        .form
        .unwrap_or_else(|| expiry_fields(user.user.default_expiry_days));
    let page = Page::user(user, "settings.title", "/app/settings");
    html::render(
        status,
        &SettingsPage {
            page,
            csrf: user.csrf.clone(),
            saved: extra.saved,
            error: extra.error,
            expiry,
            custom_days,
            usage: Usage::new(used, quota),
            new_token: extra.new_token,
            token_error: extra.token_error,
            token_name: extra.token_name,
            tokens,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_names() {
        assert_eq!(valid_token_name("  Laptop "), Some("Laptop"));
        assert_eq!(
            valid_token_name(&"ä".repeat(100)),
            Some("ä".repeat(100).as_str())
        );
        assert_eq!(valid_token_name(""), None);
        assert_eq!(valid_token_name("   "), None);
        assert_eq!(valid_token_name(&"x".repeat(101)), None);
        assert_eq!(valid_token_name("a\u{0}b"), None);
    }

    #[test]
    fn expiry_choices() {
        assert_eq!(parse_expiry("0", "").unwrap(), Some(0));
        assert_eq!(parse_expiry("30", "99").unwrap(), Some(30));
        assert_eq!(parse_expiry("custom", " 14 ").unwrap(), Some(14));
        assert_eq!(parse_expiry("custom", "0").unwrap(), None);
        assert_eq!(parse_expiry("custom", "3651").unwrap(), None);
        assert_eq!(parse_expiry("custom", "").unwrap(), None);
        assert!(parse_expiry("14", "").is_err());
        assert!(parse_expiry("never", "").is_err());
        assert_eq!(expiry_fields(7), ("7".to_owned(), String::new()));
        assert_eq!(expiry_fields(14), ("custom".to_owned(), "14".to_owned()));
    }
}
