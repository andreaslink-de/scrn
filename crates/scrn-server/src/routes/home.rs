//! Start page redirect.

use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};

use crate::AppState;
use crate::auth::CurrentUser;
use crate::error::AppError;
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
