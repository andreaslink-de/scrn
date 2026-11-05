//! Login and logout.
//!
//! Every failed login, whether the username is unknown, the password wrong
//! or the account disabled, produces exactly the same response, and an
//! argon2 verification runs in every case so the timing is similar too.

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
use crate::html::{self, Ctx, Field, csrf_field};
use crate::i18n::DEFAULT_LANGUAGE;
use crate::repo::{sessions, users};

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
    headers: HeaderMap,
    user: Option<CurrentUser>,
) -> Result<Response, AppError> {
    if user.is_some() {
        return Ok(Redirect::to("/app").into_response());
    }
    let (csrf, cookie) = auth::form_csrf(&headers, &state)?;
    let ctx = Ctx::new(DEFAULT_LANGUAGE);
    Ok(with_cookie(
        render(&ctx, &csrf, None, StatusCode::OK),
        cookie,
    ))
}

pub async fn submit(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    auth::verify_form_csrf(&headers, &state, &form.csrf)?;
    let ctx = Ctx::new(DEFAULT_LANGUAGE);
    let now = state.clock.now();
    let ip_key = ip.to_string();
    let username = form.username.trim();
    let user_key = username.to_lowercase();

    if state.login_ip_limiter.is_limited(&ip_key, now)
        || state.login_user_limiter.is_limited(&user_key, now)
    {
        let message = ctx.t("error.too_many_attempts");
        return Ok(render(
            &ctx,
            &form.csrf,
            Some(&message),
            StatusCode::TOO_MANY_REQUESTS,
        ));
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
            let message = ctx.t("error.login_failed");
            return Ok(render(
                &ctx,
                &form.csrf,
                Some(&message),
                StatusCode::UNAUTHORIZED,
            ));
        }
    };

    state.login_user_limiter.reset(&user_key);
    sessions::delete_expired(&state.db, now).await?;
    let cookie = auth::start_session(&state, user.id).await?;
    tracing::info!(user_id = user.id, "user logged in");
    Ok(([(SET_COOKIE, cookie)], Redirect::to("/app")).into_response())
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

fn render(ctx: &Ctx<'_>, csrf: &str, error: Option<&str>, status: StatusCode) -> Response {
    // The username is deliberately not echoed back, so the page is the
    // same for every failed attempt.
    let fields = [
        Field {
            name: "username",
            label: ctx.t("field.username"),
            kind: "text",
            value: "",
            autocomplete: "username",
            hint: None,
            extra: r#" autofocus"#,
        },
        Field {
            name: "password",
            label: ctx.t("field.password"),
            kind: "password",
            value: "",
            autocomplete: "current-password",
            hint: None,
            extra: "",
        },
    ];
    let fields: String = fields.iter().map(Field::render).collect();
    let body = format!(
        r#"{alert}
<form method="post" action="/login">
{csrf}
{fields}
<p><button type="submit">{submit}</button></p>
</form>"#,
        alert = html::alert(error),
        csrf = csrf_field(csrf),
        submit = ctx.t("login.submit"),
    );
    html::response(status, html::page(ctx, "login.title", None, &body))
}
