//! Login and logout.
//!
//! Every failed login, whether the username is unknown, the password wrong
//! or the account disabled, produces exactly the same response, and an
//! argon2 verification runs in every case so the timing is similar too.

use askama::Template;
use axum::Form;
use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use super::with_cookie;
use crate::AppState;
use crate::auth::{self, ClientIp, CurrentUser, MAX_PASSWORD_CHARS, MAX_USERNAME_CHARS};
use crate::error::AppError;
use crate::html::{self, Page};
use crate::i18n;
use crate::prefs::Prefs;
use crate::repo::{sessions, users};

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage {
    page: Page,
    csrf: String,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginForm {
    csrf: String,
    username: String,
    password: String,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
}

pub async fn show(
    State(state): State<AppState>,
    prefs: Prefs,
    headers: HeaderMap,
    user: Option<CurrentUser>,
) -> Result<Response, AppError> {
    if user.is_some() {
        return Ok(Redirect::to("/app").into_response());
    }
    let (csrf, cookie) = auth::form_csrf(&headers, &state)?;
    Ok(with_cookie(
        render(prefs, csrf, None, StatusCode::OK)?,
        cookie,
    ))
}

pub async fn submit(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    prefs: Prefs,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    auth::verify_form_csrf(&headers, &state, &form.csrf)?;
    let message = |key| Some(i18n::catalogs().t(prefs.lang, key).to_owned());
    let now = state.clock.now();
    let ip_key = ip.to_string();
    let username = form.username.trim();
    let user_key = username.to_lowercase();

    if state.login_ip_limiter.is_limited(&ip_key, now)
        || state.login_user_limiter.is_limited(&user_key, now)
    {
        return render(
            prefs,
            form.csrf,
            message("error.too_many_attempts"),
            StatusCode::TOO_MANY_REQUESTS,
        );
    }
    state.login_ip_limiter.record(&ip_key, now);

    let user = if username.len() <= MAX_USERNAME_CHARS {
        users::get_by_username(&state.db, username).await?
    } else {
        None
    };
    let hash = user
        .as_ref()
        .and_then(|u| u.password_hash.clone())
        .unwrap_or_else(|| state.dummy_password_hash.to_string());
    let password_ok = form.password.chars().count() <= MAX_PASSWORD_CHARS
        && auth::verify_password(form.password, hash).await?;

    let user = match user {
        Some(user) if password_ok && user.password_hash.is_some() && !user.is_disabled() => user,
        _ => {
            state.login_user_limiter.record(&user_key, now);
            return render(
                prefs,
                form.csrf,
                message("error.login_failed"),
                StatusCode::UNAUTHORIZED,
            );
        }
    };

    state.login_user_limiter.reset(&user_key);
    sessions::delete_expired(&state.db, now).await?;
    let cookie = auth::start_session(&state, user.id).await?;
    tracing::info!(user_id = user.id, "user logged in");
    let mut response = ([(SET_COOKIE, cookie)], Redirect::to("/app")).into_response();
    // Remember the user's language and theme for logged-out pages too.
    for cookie in Prefs::of_user(&user).cookies(state.config.cookie_secure) {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    Ok(response)
}

pub async fn logout(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    sessions::delete(&state.db, &user.session_hash).await?;
    let cookie = auth::clear_session_cookie(&state)?;
    Ok(([(SET_COOKIE, cookie)], Redirect::to("/login")).into_response())
}

fn render(
    prefs: Prefs,
    csrf: String,
    error: Option<String>,
    status: StatusCode,
) -> Result<Response, AppError> {
    // The username is deliberately not echoed back, so the page is the
    // same for every failed attempt.
    let page = Page::public(prefs, "login.title", Some(csrf.clone()), "/login");
    html::render(status, &LoginPage { page, csrf, error })
}
