//! Administration: invitations and user accounts.

use std::collections::HashMap;

use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::auth::{AdminUser, CurrentUser};
use crate::crypto::SecretToken;
use crate::error::AppError;
use crate::html::{self, Ctx, Nav, csrf_field, esc, format_time};
use crate::repo::invites::{self, Invite};
use crate::repo::sessions;
use crate::repo::users::{self, Role, User};

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
    let ctx = Ctx::new(admin.user.language);
    let all_users = users::list(&state.db).await?;
    let all_invites = invites::list(&state.db).await?;
    let now = state.clock.now();

    let notice = match notice {
        Notice::None => String::new(),
        Notice::InviteLink(link) => format!(
            r#"<p role="status">{}<br><input type="text" readonly value="{}" size="80" aria-label="{}"></p>"#,
            ctx.t("admin.invite.created"),
            esc(&link),
            ctx.t("admin.invites"),
        ),
        Notice::Error(key) => html::alert(Some(&ctx.t(key))),
    };

    let body = format!(
        r#"{notice}
<h2>{users_title}</h2>
{users}
<h2>{invites_title}</h2>
<form method="post" action="/admin/invites">
{csrf}
<p><label for="days">{days_label}</label><br><input id="days" name="days" type="number" min="1" max="30" value="{default_days}" required></p>
<p><button type="submit">{create}</button></p>
</form>
{invites}"#,
        users_title = ctx.t("admin.users"),
        users = users_table(&ctx, admin, &all_users),
        invites_title = ctx.t("admin.invites"),
        csrf = csrf_field(&admin.csrf),
        days_label = ctx.t("admin.invite.days"),
        default_days = DEFAULT_INVITE_DAYS,
        create = ctx.t("admin.invite.create"),
        invites = invites_table(&ctx, admin, &all_invites, &all_users, now),
    );
    let nav = Nav {
        is_admin: true,
        csrf: &admin.csrf,
    };
    Ok(html::response(
        status,
        html::page(&ctx, "admin.title", Some(&nav), &body),
    ))
}

fn users_table(ctx: &Ctx<'_>, admin: &CurrentUser, users: &[User]) -> String {
    let rows: String = users
        .iter()
        .map(|user| {
            let you = if user.id == admin.user.id {
                format!(" {}", ctx.t("admin.you"))
            } else {
                String::new()
            };
            let role = ctx.t(match user.role {
                Role::Admin => "role.admin",
                Role::User => "role.user",
            });
            let status = ctx.t(if user.is_disabled() {
                "status.disabled"
            } else {
                "status.active"
            });
            let action = if user.id == admin.user.id {
                String::new()
            } else {
                let (path, label) = if user.is_disabled() {
                    ("enable", "admin.enable")
                } else {
                    ("disable", "admin.disable")
                };
                format!(
                    r#"<form method="post" action="/admin/users/{id}/{path}">{csrf}<button type="submit">{label}</button></form>"#,
                    id = user.id,
                    csrf = csrf_field(&admin.csrf),
                    label = ctx.t(label),
                )
            };
            format!(
                "<tr><td>{name}{you}</td><td>{role}</td><td>{status}</td><td>{created}</td><td>{action}</td></tr>\n",
                name = esc(&user.username),
                created = format_time(user.created_at),
            )
        })
        .collect();
    format!(
        "<table>\n<thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead>\n<tbody>\n{rows}</tbody>\n</table>",
        ctx.t("admin.col.username"),
        ctx.t("admin.col.role"),
        ctx.t("admin.col.status"),
        ctx.t("admin.col.created"),
        ctx.t("admin.col.actions"),
    )
}

fn invites_table(
    ctx: &Ctx<'_>,
    admin: &CurrentUser,
    invites: &[Invite],
    users: &[User],
    now: i64,
) -> String {
    if invites.is_empty() {
        return format!("<p>{}</p>", ctx.t("admin.invite.none"));
    }
    let names: HashMap<i64, &str> = users.iter().map(|u| (u.id, u.username.as_str())).collect();
    let rows: String = invites
        .iter()
        .map(|invite| {
            let (status, action) = match invite.used_by {
                Some(user_id) => (
                    format!(
                        "{}: {}",
                        ctx.t("invite.status.used"),
                        esc(names.get(&user_id).copied().unwrap_or("?"))
                    ),
                    String::new(),
                ),
                None => {
                    let status = ctx.t(if invite.is_usable(now) {
                        "invite.status.open"
                    } else {
                        "invite.status.expired"
                    });
                    let action = format!(
                        r#"<form method="post" action="/admin/invites/{id}/delete">{csrf}<button type="submit">{label}</button></form>"#,
                        id = invite.id,
                        csrf = csrf_field(&admin.csrf),
                        label = ctx.t("admin.invite.revoke"),
                    );
                    (status, action)
                }
            };
            format!(
                "<tr><td>{created}</td><td>{expires}</td><td>{status}</td><td>{action}</td></tr>\n",
                created = format_time(invite.created_at),
                expires = format_time(invite.expires_at),
            )
        })
        .collect();
    format!(
        "<table>\n<thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead>\n<tbody>\n{rows}</tbody>\n</table>",
        ctx.t("admin.col.created"),
        ctx.t("admin.col.expires"),
        ctx.t("admin.col.status"),
        ctx.t("admin.col.actions"),
    )
}
