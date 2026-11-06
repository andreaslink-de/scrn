//! The JSON API for the desktop client under `/api/v1`. Every route needs a
//! valid API token in `Authorization: Bearer ...`; session cookies are not
//! accepted here, so the API needs no CSRF protection.

mod me;
mod screenshots;

use axum::Json;
use axum::Router;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, WWW_AUTHENTICATE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use scrn_proto::ApiErrorBody;

use crate::AppState;
use crate::crypto;
use crate::repo::users::{self, User};
use crate::repo::{RepoError, api_tokens};

/// `last_used_at` is written at most this often per token, in seconds.
pub const TOKEN_TOUCH_INTERVAL: i64 = 60;

/// Builds the router for everything below `/api/v1`.
pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/me", get(me::me))
        .route(
            "/screenshots",
            get(screenshots::list).post(screenshots::upload),
        )
        .route("/screenshots/{id}", delete(screenshots::delete))
        .fallback(not_found)
        // `layer` (not `route_layer`) so that unknown paths need a token too
        // and do not reveal which endpoints exist.
        .layer(middleware::from_fn_with_state(state, require_token))
}

async fn not_found() -> ApiError {
    ApiError::NotFound
}

/// Errors of API handlers, sent as JSON with a stable code.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("missing or invalid API token")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(&'static str),
    #[error("the body must be a PNG image (Content-Type: image/png)")]
    UnsupportedMediaType,
    #[error("the request body is too large")]
    PayloadTooLarge,
    #[error("the image exceeds the size limits")]
    ImageTooLarge,
    #[error("the body is not a valid PNG image")]
    InvalidImage,
    #[error("the storage quota is exceeded")]
    QuotaExceeded { used: i64, limit: i64 },
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<RepoError> for ApiError {
    fn from(err: RepoError) -> Self {
        Self::Internal(err.to_string())
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        Self::Internal(format!("database error: {err}"))
    }
}

impl From<crypto::CryptoError> for ApiError {
    fn from(err: crypto::CryptoError) -> Self {
        Self::Internal(err.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            Self::UnsupportedMediaType => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type")
            }
            Self::PayloadTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
            Self::ImageTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "image_too_large"),
            Self::InvalidImage => (StatusCode::UNPROCESSABLE_ENTITY, "invalid_image"),
            Self::QuotaExceeded { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "quota_exceeded"),
            Self::Internal(detail) => {
                tracing::error!(error = %detail, "API request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal")
            }
        };
        let message = match &self {
            Self::Internal(_) => "internal server error".to_owned(),
            other => other.to_string(),
        };
        let mut body = ApiErrorBody::new(code, &message);
        if let Self::QuotaExceeded { used, limit } = self {
            body.used_bytes = u64::try_from(used).ok();
            body.quota_bytes = u64::try_from(limit).ok();
        }
        let mut response = (status, Json(body)).into_response();
        if status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

/// The user an API request authenticated as.
#[derive(Debug, Clone)]
pub struct ApiUser {
    pub user: User,
    pub token_id: i64,
}

impl FromRequestParts<AppState> for ApiUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, ApiError> {
        // Set by `require_token`; missing only if a route escaped the layer.
        parts
            .extensions
            .get::<ApiUser>()
            .cloned()
            .ok_or(ApiError::Unauthorized)
    }
}

/// Middleware: authenticates the bearer token and marks every API response
/// as not cacheable.
async fn require_token(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let mut response = match authenticate(&state, req.headers()).await {
        Ok(api_user) => {
            req.extensions_mut().insert(api_user);
            next.run(req).await
        }
        Err(err) => err.into_response(),
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<ApiUser, ApiError> {
    let token = bearer_token(headers).ok_or(ApiError::Unauthorized)?;
    let hash = crypto::hash_api_token(token).ok_or(ApiError::Unauthorized)?;
    let api_token = api_tokens::get_active_by_hash(&state.db, &hash)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    let user = users::get_by_id(&state.db, api_token.user_id)
        .await?
        .filter(|user| !user.is_disabled())
        .ok_or(ApiError::Unauthorized)?;
    let now = state.clock.now();
    let due = api_token
        .last_used_at
        .is_none_or(|last| now - last >= TOKEN_TOUCH_INTERVAL);
    if due {
        api_tokens::touch_last_used(&state.db, api_token.id, now).await?;
    }
    Ok(ApiUser {
        user,
        token_id: api_token.id,
    })
}

/// Extracts the token from `Authorization: Bearer <token>`. The scheme is
/// case-insensitive (RFC 9110).
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn parses_bearer_tokens() {
        assert_eq!(bearer_token(&headers("Bearer abc")), Some("abc"));
        assert_eq!(bearer_token(&headers("bearer  abc ")), Some("abc"));
        assert_eq!(bearer_token(&headers("Basic abc")), None);
        assert_eq!(bearer_token(&headers("Bearer")), None);
        assert_eq!(bearer_token(&headers("Bearer  ")), None);
        assert_eq!(bearer_token(&HeaderMap::new()), None);
    }
}
