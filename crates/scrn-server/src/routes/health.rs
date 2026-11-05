//! Health check.

use axum::extract::State;
use axum::http::StatusCode;

use crate::AppState;

/// Liveness and readiness check: 200 when the database answers, 503 otherwise.
pub async fn healthz(State(state): State<AppState>) -> (StatusCode, &'static str) {
    match crate::db::ping(&state.db).await {
        Ok(()) => (StatusCode::OK, "ok\n"),
        Err(err) => {
            tracing::warn!(error = %err, "health check: database unavailable");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable\n")
        }
    }
}
