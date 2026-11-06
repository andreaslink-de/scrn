//! Typed errors of request handlers and their HTTP responses.
//!
//! Handlers do not know the visitor's language when an error happens, so an
//! error first becomes a bare response that carries the message key in an
//! extension. The [`localize_errors`] middleware, which sees the request
//! headers, then renders the page in the right language and theme.

use axum::extract::Request;
use axum::http::header::RETRY_AFTER;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};

use crate::crypto::CryptoError;
use crate::html;
use crate::prefs::Prefs;
use crate::repo::RepoError;

/// Errors a handler can return. Messages shown to users come from the
/// catalogs; internal details are logged, never sent to the client.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("bad request")]
    BadRequest,
    #[error("CSRF token missing or invalid")]
    Csrf,
    #[error("too many requests")]
    TooManyRequests { retry_after: i64 },
    #[error("login required")]
    LoginRequired,
    /// Any status with a specific catalog message.
    #[error("{key}")]
    Message {
        status: StatusCode,
        key: &'static str,
    },
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<RepoError> for AppError {
    fn from(err: RepoError) -> Self {
        Self::Internal(err.to_string())
    }
}

impl From<CryptoError> for AppError {
    fn from(err: CryptoError) -> Self {
        Self::Internal(err.to_string())
    }
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        Self::Internal(format!("database error: {err}"))
    }
}

impl From<tokio::task::JoinError> for AppError {
    fn from(err: tokio::task::JoinError) -> Self {
        Self::Internal(format!("blocking task failed: {err}"))
    }
}

/// Marks a response as an error page still to be rendered.
#[derive(Debug, Clone, Copy)]
struct ErrorKey(&'static str);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, key) = match &self {
            Self::LoginRequired => return Redirect::to("/login").into_response(),
            Self::NotFound => (StatusCode::NOT_FOUND, "error.not_found"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "error.forbidden"),
            Self::BadRequest => (StatusCode::BAD_REQUEST, "error.bad_request"),
            Self::Csrf => (StatusCode::FORBIDDEN, "error.csrf"),
            Self::TooManyRequests { .. } => {
                (StatusCode::TOO_MANY_REQUESTS, "error.too_many_requests")
            }
            Self::Message { status, key } => (*status, *key),
            Self::Internal(detail) => {
                tracing::error!(error = %detail, "request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "error.internal")
            }
        };
        let mut response = status.into_response();
        response.extensions_mut().insert(ErrorKey(key));
        if let Self::TooManyRequests { retry_after } = self
            && let Ok(value) = HeaderValue::from_str(&retry_after.to_string())
        {
            response.headers_mut().insert(RETRY_AFTER, value);
        }
        response
    }
}

/// Middleware: renders error responses as translated pages.
pub async fn localize_errors(request: Request, next: Next) -> Response {
    let prefs = Prefs::from_headers(request.headers());
    let response = next.run(request).await;
    let Some(ErrorKey(key)) = response.extensions().get::<ErrorKey>().copied() else {
        return response;
    };
    let mut page = html::error_page(prefs, response.status(), key);
    if let Some(retry) = response.headers().get(RETRY_AFTER) {
        page.headers_mut().insert(RETRY_AFTER, retry.clone());
    }
    page
}
