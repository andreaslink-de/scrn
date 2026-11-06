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
    /// Storage used by images and thumbnails.
    pub used_bytes: u64,
    /// Storage quota that applies to the user; `None` means unlimited.
    pub quota_bytes: Option<u64>,
}

/// Body of every API error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    /// Stable machine-readable code, e.g. `unauthorized` or `quota_exceeded`.
    pub error: String,
    /// Human-readable English description.
    pub message: String,
    /// Set for `quota_exceeded`: storage currently used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_bytes: Option<u64>,
    /// Set for `quota_exceeded`: the quota that applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_bytes: Option<u64>,
}

impl ApiErrorBody {
    pub fn new(error: &str, message: &str) -> Self {
        Self {
            error: error.to_owned(),
            message: message.to_owned(),
            used_bytes: None,
            quota_bytes: None,
        }
    }
}

/// Who can see a screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Private,
    Public,
}

/// A screenshot as returned by the upload and list endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotInfo {
    /// UUID of the screenshot.
    pub id: String,
    pub visibility: Visibility,
    /// The public link; only present for public screenshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_url: Option<String>,
    /// Unix seconds when the public link expires; `None` means never.
    pub expires_at: Option<i64>,
    pub width: u32,
    pub height: u32,
    /// Unix seconds of the upload.
    pub created_at: i64,
}

/// Response of `GET /api/v1/screenshots`, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotList {
    pub screenshots: Vec<ScreenshotInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn me_response_json_shape() {
        let me = MeResponse {
            username: "alice".into(),
            server_version: "0.1.0".into(),
            used_bytes: 10,
            quota_bytes: None,
        };
        let json = serde_json::to_string(&me).unwrap();
        assert_eq!(
            json,
            r#"{"username":"alice","server_version":"0.1.0","used_bytes":10,"quota_bytes":null}"#
        );
        assert_eq!(serde_json::from_str::<MeResponse>(&json).unwrap(), me);
    }

    #[test]
    fn error_body_json_shape() {
        let body = ApiErrorBody::new("unauthorized", "missing token");
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"error":"unauthorized","message":"missing token"}"#
        );
        let quota = ApiErrorBody {
            used_bytes: Some(5),
            quota_bytes: Some(4),
            ..ApiErrorBody::new("quota_exceeded", "full")
        };
        assert_eq!(
            serde_json::to_string(&quota).unwrap(),
            r#"{"error":"quota_exceeded","message":"full","used_bytes":5,"quota_bytes":4}"#
        );
    }

    #[test]
    fn screenshot_json_shape() {
        let private = ScreenshotInfo {
            id: "id".into(),
            visibility: Visibility::Private,
            short_url: None,
            expires_at: None,
            width: 2,
            height: 1,
            created_at: 7,
        };
        assert_eq!(
            serde_json::to_string(&private).unwrap(),
            r#"{"id":"id","visibility":"private","expires_at":null,"width":2,"height":1,"created_at":7}"#
        );
        let public = ScreenshotInfo {
            visibility: Visibility::Public,
            short_url: Some("https://scrn.dev/abcdefgh".into()),
            ..private
        };
        let json = serde_json::to_string(&public).unwrap();
        assert!(json.contains(r#""visibility":"public","short_url":"https://scrn.dev/abcdefgh""#));
        assert_eq!(
            serde_json::from_str::<ScreenshotInfo>(&json).unwrap(),
            public
        );
    }
}
