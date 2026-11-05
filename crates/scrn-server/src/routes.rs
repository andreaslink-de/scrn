//! HTTP routes.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;

use crate::AppState;

/// Builds the router with all routes.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .with_state(state)
}

/// Liveness and readiness check: 200 when the database answers, 503 otherwise.
async fn healthz(State(state): State<AppState>) -> (StatusCode, &'static str) {
    match crate::db::ping(&state.db).await {
        Ok(()) => (StatusCode::OK, "ok\n"),
        Err(err) => {
            tracing::warn!(error = %err, "health check: database unavailable");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable\n")
        }
    }
}
