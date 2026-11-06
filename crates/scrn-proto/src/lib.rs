//! Shared API types used by the SCRN server and the desktop client.
//!
//! All endpoints live under [`API_PREFIX`] and authenticate with
//! `Authorization: Bearer scrn_...`.

use serde::{Deserialize, Serialize};

/// Path prefix of the versioned API.
pub const API_PREFIX: &str = "/api/v1";

/// Response of `GET /api/v1/me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeResponse {
    pub username: String,
    pub server_version: String,
}

/// Body of every API error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    /// Stable machine-readable code, e.g. `unauthorized`.
    pub error: String,
    /// Human-readable English description.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn me_response_json_shape() {
        let me = MeResponse {
            username: "alice".into(),
            server_version: "0.1.0".into(),
        };
        let json = serde_json::to_string(&me).unwrap();
        assert_eq!(json, r#"{"username":"alice","server_version":"0.1.0"}"#);
        assert_eq!(serde_json::from_str::<MeResponse>(&json).unwrap(), me);
    }

    #[test]
    fn error_body_json_shape() {
        let body = ApiErrorBody {
            error: "unauthorized".into(),
            message: "missing token".into(),
        };
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"error":"unauthorized","message":"missing token"}"#
        );
    }
}
