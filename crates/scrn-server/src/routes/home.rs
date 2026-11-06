//! Start page redirect and the page after login. The gallery replaces the
//! latter in step S7b.

use askama::Template;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::html::{self, Page};
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

#[derive(Template)]
#[template(path = "home.html")]
struct HomePage<'a> {
    page: Page,
    username: &'a str,
}

pub async fn app(user: CurrentUser) -> Result<Response, AppError> {
    let page = Page::user(&user, "home.title", "/app");
    html::render(
        StatusCode::OK,
        &HomePage {
            page,
            username: &user.user.username,
        },
    )
}
