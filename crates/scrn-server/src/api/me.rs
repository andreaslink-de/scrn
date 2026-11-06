//! `GET /api/v1/me`: who the token belongs to. The client uses it to test
//! its connection settings.

use axum::Json;
use scrn_proto::MeResponse;

use super::ApiUser;

pub async fn me(api_user: ApiUser) -> Json<MeResponse> {
    Json(MeResponse {
        username: api_user.user.username,
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
    })
}
