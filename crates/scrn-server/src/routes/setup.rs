//! First-start setup: creates the admin account with the one-time token
//! from the log. Locked as soon as any user exists.

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
use crate::html::{self, Ctx, Field, csrf_field};
use crate::i18n::DEFAULT_LANGUAGE;
use crate::repo::users;

#[derive(Deserialize)]
pub struct SetupForm {
    csrf: String,
    setup_token: String,
    username: String,
    password: String,
    password_confirm: String,
}

pub async fn show(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, AppError> {
    if users::count(&state.db).await? > 0 {
        return Err(AppError::NotFound);
    }
    let (csrf, cookie) = auth::form_csrf(&headers, &state)?;
    let ctx = Ctx::new(DEFAULT_LANGUAGE);
    Ok(with_cookie(
        render(&ctx, &csrf, "", None, StatusCode::OK),
        cookie,
    ))
}

pub async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SetupForm>,
) -> Result<Response, AppError> {
    auth::verify_form_csrf(&headers, &state, &form.csrf)?;
    if users::count(&state.db).await? > 0 {
        return Err(AppError::NotFound);
    }
    let ctx = Ctx::new(DEFAULT_LANGUAGE);
    let username = form.username.trim();

    if !state.check_setup_token(form.setup_token.trim()) {
        let message = ctx.t("error.setup_token_invalid");
        return Ok(render(
            &ctx,
            &form.csrf,
            username,
            Some(&message),
            StatusCode::FORBIDDEN,
        ));
    }
    if let Err(err) = auth::validate_username(username)
        .and_then(|()| auth::validate_new_password(&form.password, &form.password_confirm))
    {
        let message = err.message(&ctx);
        return Ok(render(
            &ctx,
            &form.csrf,
            username,
            Some(&message),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }

    let hash = auth::hash_password(form.password).await?;
    let now = state.clock.now();
    let Some(admin) = users::create_first_admin(&state.db, username, &hash, now).await? else {
        // Another request completed the setup in the meantime.
        return Err(AppError::NotFound);
    };
    state.clear_setup_token();
    tracing::info!(username = %admin.username, "admin account created, setup is locked");

    let cookie = auth::start_session(&state, admin.id).await?;
    Ok(([(SET_COOKIE, cookie)], Redirect::to("/admin")).into_response())
}

fn render(
    ctx: &Ctx<'_>,
    csrf: &str,
    username: &str,
    error: Option<&str>,
    status: StatusCode,
) -> Response {
    let min = MIN_PASSWORD_CHARS.to_string();
    let min_attr = format!(r#" minlength="{MIN_PASSWORD_CHARS}""#);
    let fields = [
        Field {
            name: "setup_token",
            label: ctx.t("setup.token"),
            kind: "password",
            value: "",
            autocomplete: "off",
            hint: None,
            extra: "",
        },
        Field {
            name: "username",
            label: ctx.t("field.username"),
            kind: "text",
            value: username,
            autocomplete: "username",
            hint: Some(ctx.t("field.username_hint")),
            extra: r#" maxlength="64""#,
        },
        Field {
            name: "password",
            label: ctx.t("field.password"),
            kind: "password",
            value: "",
            autocomplete: "new-password",
            hint: Some(ctx.tf("field.password_hint", &[("min", &min)])),
            extra: &min_attr,
        },
        Field {
            name: "password_confirm",
            label: ctx.t("field.password_confirm"),
            kind: "password",
            value: "",
            autocomplete: "new-password",
            hint: None,
            extra: &min_attr,
        },
    ];
    let fields: String = fields.iter().map(Field::render).collect();
    let body = format!(
        r#"<p>{intro}</p>
{alert}
<form method="post" action="/setup">
{csrf}
{fields}
<p><button type="submit">{submit}</button></p>
</form>"#,
        intro = ctx.t("setup.intro"),
        alert = html::alert(error),
        csrf = csrf_field(csrf),
        submit = ctx.t("setup.submit"),
    );
    html::response(status, html::page(ctx, "setup.title", None, &body))
}
