//! Registration through a single-use invitation link.

use askama::Template;
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
use crate::html::{self, Page};
use crate::i18n;
use crate::prefs::Prefs;
use crate::repo::users::{NewUser, Role};

#[derive(Template)]
#[template(path = "invite.html")]
struct InvitePage<'a> {
    page: Page,
    token: &'a str,
    csrf: String,
    username: &'a str,
    min_password: usize,
    error: Option<String>,
}
use crate::repo::{RepoError, invites, users};

#[derive(Deserialize)]
pub struct RegisterForm {
    csrf: String,
    username: String,
    password: String,
    password_confirm: String,
}

/// The same page for unknown, expired and used invitations.
fn invalid_invite() -> Result<Response, AppError> {
    Err(AppError::Message {
        status: StatusCode::NOT_FOUND,
        key: "error.invite_invalid",
    })
}

pub async fn show(
    State(state): State<AppState>,
    Path(token): Path<String>,
    prefs: Prefs,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let Some(hash) = crypto::hash_hex_token(&token) else {
        return invalid_invite();
    };
    let now = state.clock.now();
    if invites::get_usable_by_token_hash(&state.db, &hash, now)
        .await?
        .is_none()
    {
        return invalid_invite();
    }
    let (csrf, cookie) = auth::form_csrf(&headers, &state)?;
    Ok(with_cookie(
        render(prefs, &token, csrf, "", None, StatusCode::OK)?,
        cookie,
    ))
}

pub async fn submit(
    State(state): State<AppState>,
    Path(token): Path<String>,
    prefs: Prefs,
    headers: HeaderMap,
    Form(form): Form<RegisterForm>,
) -> Result<Response, AppError> {
    auth::verify_form_csrf(&headers, &state, &form.csrf)?;
    let Some(hash) = crypto::hash_hex_token(&token) else {
        return invalid_invite();
    };
    let username = form.username.trim();
    let now = state.clock.now();

    // Check the invite before the expensive password hash.
    if invites::get_usable_by_token_hash(&state.db, &hash, now)
        .await?
        .is_none()
    {
        return invalid_invite();
    }
    if let Err(err) = auth::validate_username(username)
        .and_then(|()| auth::validate_new_password(&form.password, &form.password_confirm))
    {
        return render(
            prefs,
            &token,
            form.csrf,
            username,
            Some(err.message(prefs.lang)),
            StatusCode::UNPROCESSABLE_ENTITY,
        );
    }
    let password_hash = auth::hash_password(form.password).await?;

    // Creating the user and consuming the invite happen atomically. The
    // immediate transaction takes the write lock up front, so concurrent
    // registrations with the same link are serialized.
    let now = state.clock.now();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let Some(invite) = invites::get_usable_by_token_hash(&mut *tx, &hash, now).await? else {
        return invalid_invite();
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
            let message = i18n::catalogs().t(prefs.lang, "error.username_taken");
            return render(
                prefs,
                &token,
                form.csrf,
                username,
                Some(message.to_owned()),
                StatusCode::CONFLICT,
            );
        }
        Err(err) => return Err(err.into()),
    };
    if !invites::mark_used(&mut *tx, invite.id, user.id, now).await? {
        return invalid_invite();
    }
    // The new account starts with the language and theme of the page.
    users::set_preferences(&mut *tx, user.id, prefs.lang, prefs.theme, 0).await?;
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
    prefs: Prefs,
    token: &str,
    csrf: String,
    username: &str,
    error: Option<String>,
    status: StatusCode,
) -> Result<Response, AppError> {
    let back = format!("/invite/{token}");
    let page = Page::public(prefs, "invite.title", Some(csrf.clone()), &back);
    html::render(
        status,
        &InvitePage {
            page,
            token,
            csrf,
            username,
            min_password: MIN_PASSWORD_CHARS,
            error,
        },
    )
}
