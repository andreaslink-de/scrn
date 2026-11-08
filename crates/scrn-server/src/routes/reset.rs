//! Setting a new password through a single-use reset link from an admin.

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use super::with_cookie;
use crate::AppState;
use crate::auth::{self, ClientIp, MIN_PASSWORD_CHARS};
use crate::crypto;
use crate::error::AppError;
use crate::html::{self, Page};
use crate::prefs::Prefs;
use crate::repo::{password_resets, sessions, users};

#[derive(Template)]
#[template(path = "reset.html")]
struct ResetPage<'a> {
    page: Page,
    token: &'a str,
    csrf: String,
    username: &'a str,
    min_password: usize,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct ResetForm {
    csrf: String,
    password: String,
    password_confirm: String,
}

/// The same page for unknown, expired and used links.
fn invalid_link() -> Result<Response, AppError> {
    Err(AppError::Message {
        status: StatusCode::NOT_FOUND,
        key: "error.reset_invalid",
    })
}

/// The user a usable link belongs to.
async fn username_of(
    state: &AppState,
    hash: &crate::repo::Sha256,
) -> Result<Option<String>, AppError> {
    let now = state.clock.now();
    let Some(reset) = password_resets::get_usable_by_token_hash(&state.db, hash, now).await? else {
        return Ok(None);
    };
    Ok(users::get_by_id(&state.db, reset.user_id)
        .await?
        .map(|u| u.username))
}

pub async fn show(
    State(state): State<AppState>,
    Path(token): Path<String>,
    prefs: Prefs,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let Some(hash) = crypto::hash_hex_token(&token) else {
        return invalid_link();
    };
    let Some(username) = username_of(&state, &hash).await? else {
        return invalid_link();
    };
    let (csrf, cookie) = auth::form_csrf(&headers, &state)?;
    Ok(with_cookie(
        render(prefs, &token, csrf, &username, None, StatusCode::OK)?,
        cookie,
    ))
}

pub async fn submit(
    State(state): State<AppState>,
    Path(token): Path<String>,
    ClientIp(ip): ClientIp,
    prefs: Prefs,
    headers: HeaderMap,
    Form(form): Form<ResetForm>,
) -> Result<Response, AppError> {
    auth::verify_form_csrf(&headers, &state, &form.csrf)?;
    // Each attempt may cost an argon2 hash, so it shares the login budget.
    let ip_key = ip.to_string();
    let now = state.clock.now();
    if state.login_ip_limiter.is_limited(&ip_key, now) {
        return Err(AppError::TooManyRequests {
            retry_after: crate::LOGIN_WINDOW,
        });
    }
    state.login_ip_limiter.record(&ip_key, now);
    let Some(hash) = crypto::hash_hex_token(&token) else {
        return invalid_link();
    };
    // Check the link before the expensive password hash.
    let Some(username) = username_of(&state, &hash).await? else {
        return invalid_link();
    };
    if let Err(err) = auth::validate_new_password(&form.password, &form.password_confirm) {
        return render(
            prefs,
            &token,
            form.csrf,
            &username,
            Some(err.message(prefs.lang)),
            StatusCode::UNPROCESSABLE_ENTITY,
        );
    }
    let password_hash = auth::hash_password(form.password).await?;

    // Setting the password, using up the link and ending all sessions of
    // the account happen together. The immediate transaction serializes
    // concurrent submissions of the same link.
    let now = state.clock.now();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let Some(reset) = password_resets::get_usable_by_token_hash(&mut *tx, &hash, now).await? else {
        return invalid_link();
    };
    if !password_resets::mark_used(&mut *tx, reset.id, now).await?
        || !users::set_password_hash(&mut *tx, reset.user_id, &password_hash).await?
    {
        return invalid_link();
    }
    let ended = sessions::delete_for_user(&mut *tx, reset.user_id).await?;
    tx.commit().await?;
    tracing::info!(
        user_id = reset.user_id,
        reset_id = reset.id,
        sessions_ended = ended,
        "password set via reset link"
    );
    Ok(Redirect::to("/login?reset=1").into_response())
}

fn render(
    prefs: Prefs,
    token: &str,
    csrf: String,
    username: &str,
    error: Option<String>,
    status: StatusCode,
) -> Result<Response, AppError> {
    let back = format!("/reset/{token}");
    let page = Page::public(prefs, "reset.title", Some(csrf.clone()), &back);
    html::render(
        status,
        &ResetPage {
            page,
            token,
            csrf,
            username,
            min_password: MIN_PASSWORD_CHARS,
            error,
        },
    )
}
