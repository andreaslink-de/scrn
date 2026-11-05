//! Typed errors of request handlers and their HTTP responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use crate::crypto::CryptoError;
use crate::html;
use crate::i18n::DEFAULT_LANGUAGE;
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
    #[error("login required")]
    LoginRequired,
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

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, key) = match &self {
            Self::LoginRequired => return Redirect::to("/login").into_response(),
            Self::NotFound => (StatusCode::NOT_FOUND, "error.not_found"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "error.forbidden"),
            Self::BadRequest => (StatusCode::BAD_REQUEST, "error.bad_request"),
            Self::Csrf => (StatusCode::FORBIDDEN, "error.csrf"),
            Self::Internal(detail) => {
                tracing::error!(error = %detail, "request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "error.internal")
            }
        };
        html::error_page(DEFAULT_LANGUAGE, status, key)
    }
}
