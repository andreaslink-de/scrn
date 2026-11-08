//! The settings page: language, theme, default link expiry, storage usage,
//! password, passkeys, device groups and API tokens.

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::auth::{self, CurrentUser, MIN_PASSWORD_CHARS};
use crate::crypto::ApiTokenSecret;
use crate::error::AppError;
use crate::html::{self, Page, Usage};
use crate::i18n;
use crate::prefs::{self, Prefs};
use crate::repo::api_tokens::{self, ApiToken};
use crate::repo::device_groups::{self, DeviceGroup};
use crate::repo::passkeys::{self, Passkey};
use crate::repo::{RepoError, screenshots, sessions, users};

/// Token names: 1 to 100 characters, matching the database constraint.
pub const MAX_TOKEN_NAME_CHARS: usize = 100;
/// Group names: 1 to 50 characters, matching the database constraint.
pub const MAX_GROUP_NAME_CHARS: usize = 50;
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
    groups: Vec<GroupRow>,
    group_error: Option<String>,
    group_name: String,
    passkeys_enabled: bool,
    passkeys: Vec<Passkey>,
    passkey_added: bool,
    passkey_error: Option<String>,
    /// First visit after the account was created: suggest a passkey.
    welcome: bool,
    username: String,
    has_password: bool,
    /// The current password is not needed: a fresh passkey confirmation.
    passkey_confirmed: bool,
    password_notice: Option<&'static str>,
    password_error: Option<String>,
    min_password: usize,
}

/// A device group as the settings page shows it.
struct GroupRow {
    id: i64,
    name: String,
    is_default: bool,
    tokens: i64,
}

#[derive(Deserialize)]
pub struct ShowQuery {
    saved: Option<String>,
    passkey: Option<String>,
    welcome: Option<String>,
    password: Option<String>,
    confirmed: Option<String>,
}

#[derive(Deserialize)]
pub struct PasswordForm {
    csrf: String,
    #[serde(default)]
    current_password: String,
    password: String,
    password_confirm: String,
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
    /// Missing while the user has only the default group.
    group: Option<i64>,
}

#[derive(Deserialize)]
pub struct GroupForm {
    csrf: String,
    name: String,
}

#[derive(Deserialize)]
pub struct TokenGroupForm {
    csrf: String,
    group: i64,
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
    group_error: Option<String>,
    group_name: String,
    passkey_added: bool,
    passkey_error: Option<String>,
    welcome: bool,
    password_notice: Option<&'static str>,
    password_error: Option<String>,
}

pub async fn show(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(query): Query<ShowQuery>,
) -> Result<Response, AppError> {
    let extra = Extra {
        saved: query.saved.is_some(),
        passkey_added: query.passkey.as_deref() == Some("added"),
        welcome: query.welcome.is_some(),
        password_notice: if query.password.as_deref() == Some("changed") {
            Some("password.changed")
        } else if query.confirmed.is_some() {
            Some("password.confirmed")
        } else {
            None
        },
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

/// Sets a new password. Accounts with a password need the current one,
/// unless the user confirmed with a passkey a few minutes ago. All other
/// sessions of the account end.
pub async fn change_password(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<PasswordForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    let lang = user.user.language;
    let now = state.clock.now();
    let fail = |key: &str, status| {
        let extra = Extra {
            password_error: Some(i18n::catalogs().t(lang, key).to_owned()),
            ..Extra::default()
        };
        render(&state, &user, extra, status)
    };

    if let Some(current_hash) = user.user.password_hash.clone()
        && !user.passkey_confirmed(now)
    {
        // Guessing the current password from a stolen session costs the
        // same budget as guessing it on the login page.
        let user_key = user.user.username.to_lowercase();
        if state.login_user_limiter.is_limited(&user_key, now) {
            return fail("error.too_many_attempts", StatusCode::TOO_MANY_REQUESTS).await;
        }
        let ok = form.current_password.chars().count() <= auth::MAX_PASSWORD_CHARS
            && auth::verify_password(form.current_password, current_hash).await?;
        if !ok {
            state.login_user_limiter.record(&user_key, now);
            return fail("error.current_password", StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }
    if let Err(err) = auth::validate_new_password(&form.password, &form.password_confirm) {
        let extra = Extra {
            password_error: Some(err.message(lang)),
            ..Extra::default()
        };
        return render(&state, &user, extra, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
    let hash = auth::hash_password(form.password).await?;
    let mut tx = state.db.begin().await?;
    if !users::set_password_hash(&mut *tx, user.user.id, &hash).await? {
        return Err(AppError::LoginRequired);
    }
    let ended =
        sessions::delete_others_for_user(&mut *tx, user.user.id, &user.session_hash).await?;
    tx.commit().await?;
    tracing::info!(
        user_id = user.user.id,
        other_sessions_ended = ended,
        "password changed"
    );
    Ok(Redirect::to("/app/settings?password=changed#password").into_response())
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
    let Some(name) = valid_name(&form.name) else {
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
    if let Some(group) = form.group
        && device_groups::get_for_user(&state.db, group, user.user.id)
            .await?
            .is_none()
    {
        return Err(AppError::BadRequest);
    }
    let secret = ApiTokenSecret::generate()?;
    let token = api_tokens::create(
        &state.db,
        user.user.id,
        form.group,
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

/// `POST /app/tokens/{id}/group`: moves a token, and with it the
/// screenshots it uploaded, into another group.
pub async fn set_token_group(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Form(form): Form<TokenGroupForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    if !api_tokens::set_group(&state.db, id, user.user.id, form.group).await? {
        return Err(AppError::NotFound);
    }
    tracing::info!(
        token_id = id,
        group_id = form.group,
        user_id = user.user.id,
        "API token moved to another group"
    );
    Ok(Redirect::to("/app/settings#tokens").into_response())
}

/// `POST /app/groups`: creates a device group.
pub async fn create_group(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<GroupForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    let Some(name) = valid_group_name(&form.name) else {
        return group_error(&state, &user, "error.group_name", &form.name).await;
    };
    match device_groups::create(&state.db, user.user.id, name, state.clock.now()).await {
        Ok(id) => {
            tracing::info!(
                group_id = id,
                user_id = user.user.id,
                "device group created"
            );
            Ok(Redirect::to("/app/settings#groups").into_response())
        }
        Err(RepoError::Conflict) => {
            group_error(&state, &user, "error.group_taken", &form.name).await
        }
        Err(err) => Err(err.into()),
    }
}

/// `POST /app/groups/{id}/rename`.
pub async fn rename_group(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Form(form): Form<GroupForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    let Some(name) = valid_group_name(&form.name) else {
        return group_error(&state, &user, "error.group_name", "").await;
    };
    match device_groups::rename(&state.db, id, user.user.id, name).await {
        Ok(true) => Ok(Redirect::to("/app/settings#groups").into_response()),
        Ok(false) => Err(AppError::NotFound),
        Err(RepoError::Conflict) => group_error(&state, &user, "error.group_taken", "").await,
        Err(err) => Err(err.into()),
    }
}

/// `POST /app/groups/{id}/delete`: deletes a group other than the default
/// group. Its tokens move to the default group.
pub async fn delete_group(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if !device_groups::delete(&mut tx, id, user.user.id).await? {
        return Err(AppError::NotFound);
    }
    tx.commit().await?;
    tracing::info!(
        group_id = id,
        user_id = user.user.id,
        "device group deleted"
    );
    Ok(Redirect::to("/app/settings#groups").into_response())
}

async fn group_error(
    state: &AppState,
    user: &CurrentUser,
    key: &str,
    name: &str,
) -> Result<Response, AppError> {
    let extra = Extra {
        group_error: Some(i18n::catalogs().t(user.user.language, key).to_owned()),
        group_name: name.to_owned(),
        ..Extra::default()
    };
    let status = if key == "error.group_taken" {
        StatusCode::CONFLICT
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    render(state, user, extra, status).await
}

/// Trims a group name and checks its length (1 to 50 characters). Control
/// characters are rejected.
pub fn valid_group_name(raw: &str) -> Option<&str> {
    valid_name(raw).filter(|name| name.chars().count() <= MAX_GROUP_NAME_CHARS)
}

/// Trims a token or passkey name and checks its length (1 to 100
/// characters). Control characters are rejected.
pub fn valid_name(raw: &str) -> Option<&str> {
    let name = raw.trim();
    let chars = name.chars().count();
    ((1..=MAX_TOKEN_NAME_CHARS).contains(&chars) && !name.chars().any(char::is_control))
        .then_some(name)
}

/// The settings page with an error in the passkey section.
pub async fn render_with_passkey_error(
    state: &AppState,
    user: &CurrentUser,
    key: &str,
) -> Result<Response, AppError> {
    let extra = Extra {
        passkey_error: Some(i18n::catalogs().t(user.user.language, key).to_owned()),
        ..Extra::default()
    };
    render(state, user, extra, StatusCode::UNPROCESSABLE_ENTITY).await
}

async fn render(
    state: &AppState,
    user: &CurrentUser,
    extra: Extra,
    status: StatusCode,
) -> Result<Response, AppError> {
    let tokens = api_tokens::list_for_user(&state.db, user.user.id).await?;
    let lang = user.user.language;
    let groups = device_groups::list_for_user(&state.db, user.user.id)
        .await?
        .iter()
        .map(|g: &DeviceGroup| GroupRow {
            id: g.id,
            name: super::gallery::group_name(g, lang),
            is_default: g.is_default,
            tokens: g.tokens,
        })
        .collect();
    let passkey_rows = passkeys::list_for_user(&state.db, user.user.id).await?;
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
            groups,
            group_error: extra.group_error,
            group_name: extra.group_name,
            passkeys_enabled: state.passkeys.is_some(),
            passkeys: passkey_rows,
            passkey_added: extra.passkey_added,
            passkey_error: extra.passkey_error,
            welcome: extra.welcome,
            username: user.user.username.clone(),
            has_password: user.user.password_hash.is_some(),
            passkey_confirmed: user.passkey_confirmed(state.clock.now()),
            password_notice: extra.password_notice,
            password_error: extra.password_error,
            min_password: MIN_PASSWORD_CHARS,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_names() {
        assert_eq!(valid_name("  Laptop "), Some("Laptop"));
        assert_eq!(valid_name(&"ä".repeat(100)), Some("ä".repeat(100).as_str()));
        assert_eq!(valid_name(""), None);
        assert_eq!(valid_name("   "), None);
        assert_eq!(valid_name(&"x".repeat(101)), None);
        assert_eq!(valid_name("a\u{0}b"), None);
    }

    #[test]
    fn group_names() {
        assert_eq!(valid_group_name(" Work "), Some("Work"));
        assert_eq!(valid_group_name(&"x".repeat(50)).map(str::len), Some(50));
        assert_eq!(valid_group_name(&"x".repeat(51)), None);
        assert_eq!(valid_group_name(""), None);
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
