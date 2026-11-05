//! Registration through a single-use invitation link.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use super::with_cookie;
use crate::AppState;
use crate::auth::{self, MIN_PASSWORD_CHARS};
use crate::crypto;
use crate::error::AppError;
use crate::html::{self, Ctx, Field, csrf_field};
use crate::i18n::DEFAULT_LANGUAGE;
use crate::repo::users::{NewUser, Role};
use crate::repo::{RepoError, invites, users};

#[derive(Deserialize)]
pub struct RegisterForm {
    csrf: String,
    username: String,
    password: String,
    password_confirm: String,
}

/// The same page for unknown, expired and used invitations.
fn invalid_invite() -> Response {
    html::error_page(
        DEFAULT_LANGUAGE,
        StatusCode::NOT_FOUND,
        "error.invite_invalid",
    )
}

pub async fn show(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let Some(hash) = crypto::hash_hex_token(&token) else {
        return Ok(invalid_invite());
    };
    let now = state.clock.now();
    if invites::get_usable_by_token_hash(&state.db, &hash, now)
        .await?
        .is_none()
    {
        return Ok(invalid_invite());
    }
    let (csrf, cookie) = auth::form_csrf(&headers, &state)?;
    let ctx = Ctx::new(DEFAULT_LANGUAGE);
    Ok(with_cookie(
        render(&ctx, &token, &csrf, "", None, StatusCode::OK),
        cookie,
    ))
}

pub async fn submit(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Form(form): Form<RegisterForm>,
) -> Result<Response, AppError> {
    auth::verify_form_csrf(&headers, &state, &form.csrf)?;
    let Some(hash) = crypto::hash_hex_token(&token) else {
        return Ok(invalid_invite());
    };
    let ctx = Ctx::new(DEFAULT_LANGUAGE);
    let username = form.username.trim();
    let now = state.clock.now();

    // Check the invite before the expensive password hash.
    if invites::get_usable_by_token_hash(&state.db, &hash, now)
        .await?
        .is_none()
    {
        return Ok(invalid_invite());
    }
    if let Err(err) = auth::validate_username(username)
        .and_then(|()| auth::validate_new_password(&form.password, &form.password_confirm))
    {
        let message = err.message(&ctx);
        return Ok(render(
            &ctx,
            &token,
            &form.csrf,
            username,
            Some(&message),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }
    let password_hash = auth::hash_password(form.password).await?;

    // Creating the user and consuming the invite happen atomically. The
    // immediate transaction takes the write lock up front, so concurrent
    // registrations with the same link are serialized.
    let now = state.clock.now();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let Some(invite) = invites::get_usable_by_token_hash(&mut *tx, &hash, now).await? else {
        return Ok(invalid_invite());
    };
    let created = users::create(
        &mut *tx,
        &NewUser {
            username,
            password_hash: Some(&password_hash),
            role: Role::User,
        },
        now,
    )
    .await;
    let user = match created {
        Ok(user) => user,
        Err(RepoError::Conflict) => {
            let message = ctx.t("error.username_taken");
            return Ok(render(
                &ctx,
                &token,
                &form.csrf,
                username,
                Some(&message),
                StatusCode::CONFLICT,
            ));
        }
        Err(err) => return Err(err.into()),
    };
    if !invites::mark_used(&mut *tx, invite.id, user.id, now).await? {
        return Ok(invalid_invite());
    }
    tx.commit().await?;
    tracing::info!(
        user_id = user.id,
        invite_id = invite.id,
        "user registered via invitation"
    );

    let cookie = auth::start_session(&state, user.id).await?;
    Ok(([(SET_COOKIE, cookie)], Redirect::to("/app")).into_response())
}

fn render(
    ctx: &Ctx<'_>,
    token: &str,
    csrf: &str,
    username: &str,
    error: Option<&str>,
    status: StatusCode,
) -> Response {
    let min = MIN_PASSWORD_CHARS.to_string();
    let min_attr = format!(r#" minlength="{MIN_PASSWORD_CHARS}""#);
    let fields = [
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
<form method="post" action="/invite/{token}">
{csrf}
{fields}
<p><button type="submit">{submit}</button></p>
</form>"#,
        intro = ctx.t("invite.intro"),
        alert = html::alert(error),
        token = html::esc(token),
        csrf = csrf_field(csrf),
        submit = ctx.t("invite.submit"),
    );
    html::response(status, html::page(ctx, "invite.title", None, &body))
}
