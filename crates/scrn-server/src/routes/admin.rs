//! Administration: invitations and user accounts.

use std::collections::HashMap;

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::auth::{AdminUser, CurrentUser};
use crate::crypto::SecretToken;
use crate::error::AppError;
use crate::html::{self, Page, Usage};
use crate::i18n;
use crate::repo::invites;
use crate::repo::users::{self, Role};
use crate::repo::{screenshots, sessions};

const MIB: i64 = 1024 * 1024;

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminPage {
    page: Page,
    csrf: String,
    invite_link: Option<String>,
    error: Option<String>,
    default_days: i64,
    default_quota: String,
    users: Vec<UserRow>,
    invites: Vec<InviteRow>,
}

struct UserRow {
    id: i64,
    username: String,
    is_self: bool,
    role_key: &'static str,
    disabled: bool,
    usage: Usage,
    /// The quota field: empty for the server default, else MiB.
    quota_mb: String,
}

struct InviteRow {
    id: i64,
    created_at: i64,
    expires_at: i64,
    status_key: &'static str,
    used_by: Option<String>,
    open: bool,
}

#[derive(Deserialize)]
pub struct QuotaForm {
    csrf: String,
    #[serde(default)]
    quota_mb: String,
}

/// Allowed invitation validity in days.
const INVITE_DAYS: std::ops::RangeInclusive<i64> = 1..=30;
const DEFAULT_INVITE_DAYS: i64 = 7;

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
}

#[derive(Deserialize)]
pub struct InviteForm {
    csrf: String,
    days: String,
}

/// Something to show above the lists after an action.
enum Notice {
    None,
    InviteLink(String),
    Error(&'static str),
}

pub async fn show(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
) -> Result<Response, AppError> {
    render(&state, &admin, Notice::None, StatusCode::OK).await
}

pub async fn create_invite(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Form(form): Form<InviteForm>,
) -> Result<Response, AppError> {
    admin.verify_csrf(&form.csrf)?;
    let Some(days) = form
        .days
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|d| INVITE_DAYS.contains(d))
    else {
        return render(
            &state,
            &admin,
            Notice::Error("error.invite_days"),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    };
    let token = SecretToken::generate()?;
    let now = state.clock.now();
    let invite = invites::create(
        &state.db,
        &token.hash,
        admin.user.id,
        now,
        now + days * 86_400,
    )
    .await?;
    tracing::info!(
        invite_id = invite.id,
        created_by = admin.user.id,
        days,
        "invitation created"
    );
    // The link is rendered directly instead of redirecting, because only
    // this response ever contains the token.
    let link = format!("{}/invite/{}", state.config.base_url, token.token);
    render(&state, &admin, Notice::InviteLink(link), StatusCode::OK).await
}

pub async fn delete_invite(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    admin.verify_csrf(&form.csrf)?;
    if invites::delete_unused(&state.db, id).await? {
        tracing::info!(invite_id = id, "invitation revoked");
    }
    Ok(Redirect::to("/admin").into_response())
}

pub async fn disable_user(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    admin.verify_csrf(&form.csrf)?;
    if id == admin.user.id {
        return render(
            &state,
            &admin,
            Notice::Error("error.cannot_disable_self"),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let now = state.clock.now();
    let mut tx = state.db.begin().await?;
    if !users::set_disabled(&mut *tx, id, Some(now)).await? {
        return Err(AppError::NotFound);
    }
    sessions::delete_for_user(&mut *tx, id).await?;
    tx.commit().await?;
    tracing::info!(user_id = id, by = admin.user.id, "user disabled");
    Ok(Redirect::to("/admin").into_response())
}

/// Sets a user's quota. Empty means the server default, 0 unlimited,
/// otherwise MiB. Takes effect with the next upload; nothing is deleted.
pub async fn set_quota(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
    Form(form): Form<QuotaForm>,
) -> Result<Response, AppError> {
    admin.verify_csrf(&form.csrf)?;
    let Some(quota) = parse_quota_mb(&form.quota_mb) else {
        return render(
            &state,
            &admin,
            Notice::Error("error.quota_invalid"),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    };
    if !users::set_quota(&state.db, id, quota).await? {
        return Err(AppError::NotFound);
    }
    tracing::info!(user_id = id, by = admin.user.id, quota_bytes = ?quota, "quota changed");
    Ok(Redirect::to("/admin").into_response())
}

/// `Some(None)` for the default, `Some(Some(bytes))` for a value, `None`
/// for invalid input.
fn parse_quota_mb(raw: &str) -> Option<Option<i64>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(None);
    }
    raw.parse::<i64>()
        .ok()
        .filter(|mb| *mb >= 0)
        .and_then(|mb| mb.checked_mul(MIB))
        .map(Some)
}

pub async fn enable_user(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    admin.verify_csrf(&form.csrf)?;
    if !users::set_disabled(&state.db, id, None).await? {
        return Err(AppError::NotFound);
    }
    tracing::info!(user_id = id, by = admin.user.id, "user enabled");
    Ok(Redirect::to("/admin").into_response())
}

async fn render(
    state: &AppState,
    admin: &CurrentUser,
    notice: Notice,
    status: StatusCode,
) -> Result<Response, AppError> {
    let lang = admin.user.language;
    let all_users = users::list(&state.db).await?;
    let all_invites = invites::list(&state.db).await?;
    let usage = screenshots::usage_by_user(&state.db).await?;
    let now = state.clock.now();
    let default_quota_bytes = state.config.default_quota_bytes;

    let names: HashMap<i64, &str> = all_users
        .iter()
        .map(|u| (u.id, u.username.as_str()))
        .collect();
    let invites = all_invites
        .iter()
        .map(|invite| {
            let used_by = invite
                .used_by
                .map(|id| names.get(&id).copied().unwrap_or("?").to_owned());
            let status_key = if used_by.is_some() {
                "invite.status.used"
            } else if invite.is_usable(now) {
                "invite.status.open"
            } else {
                "invite.status.expired"
            };
            InviteRow {
                id: invite.id,
                created_at: invite.created_at,
                expires_at: invite.expires_at,
                status_key,
                open: used_by.is_none(),
                used_by,
            }
        })
        .collect();
    let users = all_users
        .iter()
        .map(|user| UserRow {
            id: user.id,
            username: user.username.clone(),
            is_self: user.id == admin.user.id,
            role_key: match user.role {
                Role::Admin => "role.admin",
                Role::User => "role.user",
            },
            disabled: user.is_disabled(),
            usage: Usage::new(
                usage.get(&user.id).copied().unwrap_or(0),
                user.effective_quota(default_quota_bytes),
            ),
            quota_mb: user
                .quota_bytes
                .map_or_else(String::new, |q| (q / MIB).to_string()),
        })
        .collect();

    let (invite_link, error) = match notice {
        Notice::None => (None, None),
        Notice::InviteLink(link) => (Some(link), None),
        Notice::Error(key) => (None, Some(i18n::catalogs().t(lang, key).to_owned())),
    };
    let default_quota = if default_quota_bytes == 0 {
        i18n::catalogs().t(lang, "storage.unlimited").to_owned()
    } else {
        html::format_bytes(default_quota_bytes, lang)
    };
    html::render(
        status,
        &AdminPage {
            page: Page::user(admin, "admin.title", "/admin"),
            csrf: admin.csrf.clone(),
            invite_link,
            error,
            default_days: DEFAULT_INVITE_DAYS,
            default_quota,
            users,
            invites,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_input() {
        assert_eq!(parse_quota_mb(""), Some(None));
        assert_eq!(parse_quota_mb("  "), Some(None));
        assert_eq!(parse_quota_mb("0"), Some(Some(0)));
        assert_eq!(parse_quota_mb(" 5 "), Some(Some(5 * MIB)));
        for bad in ["-1", "1.5", "x", "9223372036854775807"] {
            assert_eq!(parse_quota_mb(bad), None, "{bad}");
        }
    }
}
