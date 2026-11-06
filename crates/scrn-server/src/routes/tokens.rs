//! API token management for the logged-in user.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::AppState;
use crate::auth::CurrentUser;
use crate::crypto::ApiTokenSecret;
use crate::error::AppError;
use crate::html::{self, Ctx, Nav, csrf_field, esc, format_time};
use crate::repo::api_tokens::{self, ApiToken};

/// Token names: 1 to 100 characters, matching the database constraint.
pub const MAX_TOKEN_NAME_CHARS: usize = 100;

#[derive(Deserialize)]
pub struct CreateForm {
    csrf: String,
    name: String,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
}

enum Notice {
    None,
    NewToken(String),
    Error(&'static str),
}

pub async fn show(State(state): State<AppState>, user: CurrentUser) -> Result<Response, AppError> {
    render(&state, &user, Notice::None, "", StatusCode::OK).await
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<CreateForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    let Some(name) = valid_name(&form.name) else {
        return render(
            &state,
            &user,
            Notice::Error("error.token_name"),
            &form.name,
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    };
    let secret = ApiTokenSecret::generate()?;
    let token = api_tokens::create(
        &state.db,
        user.user.id,
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
    render(
        &state,
        &user,
        Notice::NewToken(secret.token),
        "",
        StatusCode::OK,
    )
    .await
}

pub async fn revoke(
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
    Ok(Redirect::to("/app/tokens").into_response())
}

/// Trims the name and checks its length. Control characters are rejected.
fn valid_name(raw: &str) -> Option<&str> {
    let name = raw.trim();
    let chars = name.chars().count();
    ((1..=MAX_TOKEN_NAME_CHARS).contains(&chars) && !name.chars().any(char::is_control))
        .then_some(name)
}

async fn render(
    state: &AppState,
    user: &CurrentUser,
    notice: Notice,
    name_value: &str,
    status: StatusCode,
) -> Result<Response, AppError> {
    let ctx = Ctx::new(user.user.language);
    let tokens = api_tokens::list_for_user(&state.db, user.user.id).await?;

    let notice = match notice {
        Notice::None => String::new(),
        Notice::NewToken(token) => format!(
            r#"<p role="status">{}<br><input type="text" readonly value="{}" size="60" aria-label="{}"></p>"#,
            ctx.t("tokens.created"),
            esc(&token),
            ctx.t("tokens.title"),
        ),
        Notice::Error(key) => html::alert(Some(&ctx.t(key))),
    };

    let body = format!(
        r#"{notice}
<p>{intro}</p>
<form method="post" action="/app/tokens">
{csrf}
<p><label for="name">{name_label}</label><br><input id="name" name="name" type="text" maxlength="{max}" value="{name_value}" autocomplete="off" required></p>
<p><button type="submit">{create}</button></p>
</form>
{table}"#,
        intro = ctx.t("tokens.intro"),
        csrf = csrf_field(&user.csrf),
        name_label = ctx.t("tokens.name"),
        max = MAX_TOKEN_NAME_CHARS,
        name_value = esc(name_value),
        create = ctx.t("tokens.create"),
        table = tokens_table(&ctx, user, &tokens),
    );
    let nav = Nav {
        is_admin: user.is_admin(),
        csrf: &user.csrf,
    };
    Ok(html::response(
        status,
        html::page(&ctx, "tokens.title", Some(&nav), &body),
    ))
}

fn tokens_table(ctx: &Ctx<'_>, user: &CurrentUser, tokens: &[ApiToken]) -> String {
    if tokens.is_empty() {
        return format!("<p>{}</p>", ctx.t("tokens.none"));
    }
    let rows: String = tokens
        .iter()
        .map(|token| {
            let last_used = token
                .last_used_at
                .map_or_else(|| ctx.t("tokens.never"), format_time);
            let (status, action) = if token.revoked_at.is_some() {
                (ctx.t("status.revoked"), String::new())
            } else {
                (
                    ctx.t("status.active"),
                    format!(
                        r#"<form method="post" action="/app/tokens/{id}/revoke">{csrf}<button type="submit">{label}</button></form>"#,
                        id = token.id,
                        csrf = csrf_field(&user.csrf),
                        label = ctx.t("tokens.revoke"),
                    ),
                )
            };
            format!(
                "<tr><td>{name}</td><td>{created}</td><td>{last_used}</td><td>{status}</td><td>{action}</td></tr>\n",
                name = esc(&token.name),
                created = format_time(token.created_at),
            )
        })
        .collect();
    format!(
        "<table>\n<thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead>\n<tbody>\n{rows}</tbody>\n</table>",
        ctx.t("tokens.col.name"),
        ctx.t("admin.col.created"),
        ctx.t("tokens.col.last_used"),
        ctx.t("admin.col.status"),
        ctx.t("admin.col.actions"),
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
}
