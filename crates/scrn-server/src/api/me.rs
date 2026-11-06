//! `GET /api/v1/me`: who the token belongs to and how much storage is used.
//! The client uses it to test its connection settings.

use axum::Json;
use axum::extract::State;
use scrn_proto::MeResponse;

use super::{ApiError, ApiUser};
use crate::AppState;
use crate::repo::screenshots;

pub async fn me(
    State(state): State<AppState>,
    api_user: ApiUser,
) -> Result<Json<MeResponse>, ApiError> {
    let used = screenshots::usage_for_user(&state.db, api_user.user.id).await?;
    let quota = api_user
        .user
        .effective_quota(state.config.default_quota_bytes);
    Ok(Json(MeResponse {
        username: api_user.user.username,
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        used_bytes: u64::try_from(used).unwrap_or(0),
        quota_bytes: quota.and_then(|q| u64::try_from(q).ok()),
    }))
}
