//! First-start setup: creates the admin account with the one-time token
//! from the log. Locked as soon as any user exists.

use askama::Template;
use axum::Form;
use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use super::with_cookie;
use crate::AppState;
use crate::auth::{self, MIN_PASSWORD_CHARS};
use crate::error::AppError;
use crate::html::{self, Page};
use crate::i18n;
use crate::prefs::Prefs;
use crate::repo::users;

#[derive(Template)]
#[template(path = "setup.html")]
struct SetupPage<'a> {
    page: Page,
    csrf: String,
    username: &'a str,
    min_password: usize,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct SetupForm {
    csrf: String,
    setup_token: String,
    username: String,
    password: String,
    password_confirm: String,
}

pub async fn show(
    State(state): State<AppState>,
    prefs: Prefs,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if users::count(&state.db).await? > 0 {
        return Err(AppError::NotFound);
    }
    let (csrf, cookie) = auth::form_csrf(&headers, &state)?;
    Ok(with_cookie(
        render(prefs, csrf, "", None, StatusCode::OK)?,
        cookie,
    ))
}

pub async fn submit(
    State(state): State<AppState>,
    prefs: Prefs,
    headers: HeaderMap,
    Form(form): Form<SetupForm>,
) -> Result<Response, AppError> {
    auth::verify_form_csrf(&headers, &state, &form.csrf)?;
    if users::count(&state.db).await? > 0 {
        return Err(AppError::NotFound);
    }
    let username = form.username.trim();

    if !state.check_setup_token(form.setup_token.trim()) {
        let message = i18n::catalogs().t(prefs.lang, "error.setup_token_invalid");
        return render(
            prefs,
            form.csrf,
            username,
            Some(message.to_owned()),
            StatusCode::FORBIDDEN,
        );
    }
    if let Err(err) = auth::validate_username(username)
        .and_then(|()| auth::validate_new_password(&form.password, &form.password_confirm))
    {
        return render(
            prefs,
            form.csrf,
            username,
            Some(err.message(prefs.lang)),
            StatusCode::UNPROCESSABLE_ENTITY,
        );
    }

    let hash = auth::hash_password(form.password).await?;
    let now = state.clock.now();
    let Some(admin) = users::create_first_admin(&state.db, username, &hash, now).await? else {
        // Another request completed the setup in the meantime.
        return Err(AppError::NotFound);
    };
    // Keep the language and theme the admin used during the setup.
    users::set_preferences(&state.db, admin.id, prefs.lang, prefs.theme, 0).await?;
    state.clear_setup_token();
    tracing::info!(username = %admin.username, "admin account created, setup is locked");

    let cookie = auth::start_session(&state, admin.id).await?;
    Ok(([(SET_COOKIE, cookie)], Redirect::to("/admin")).into_response())
}

fn render(
    prefs: Prefs,
    csrf: String,
    username: &str,
    error: Option<String>,
    status: StatusCode,
) -> Result<Response, AppError> {
    let page = Page::public(prefs, "setup.title", Some(csrf.clone()), "/setup");
    html::render(
        status,
        &SetupPage {
            page,
            csrf,
            username,
            min_password: MIN_PASSWORD_CHARS,
            error,
        },
    )
}
