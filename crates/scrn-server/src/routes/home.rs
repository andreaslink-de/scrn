//! Start page redirect and the page after login. The gallery replaces the
//! latter in step S7b.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::html::{self, Ctx, Nav};
use crate::repo::users;

/// `/`: setup on first start, otherwise the app or the login page.
pub async fn root(
    State(state): State<AppState>,
    user: Option<CurrentUser>,
) -> Result<Response, AppError> {
    let target = if user.is_some() {
        "/app"
    } else if users::count(&state.db).await? == 0 {
        "/setup"
    } else {
        "/login"
    };
    Ok(Redirect::to(target).into_response())
}

pub async fn app(user: CurrentUser) -> Response {
    let ctx = Ctx::new(user.user.language);
    let body = format!(
        "<p>{}</p>",
        ctx.tf("home.signed_in_as", &[("username", &user.user.username)])
    );
    let nav = Nav {
        is_admin: user.is_admin(),
        csrf: &user.csrf,
    };
    html::response(
        StatusCode::OK,
        html::page(&ctx, "home.title", Some(&nav), &body),
    )
}
