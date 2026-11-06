//! Public short links: `/<short_id>` and `/<short_id>.png` serve the image of
//! a public, unexpired screenshot. Unknown, private and expired IDs all get
//! the same 404 page, so a link reveals nothing once it is dead.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG, IF_NONE_MATCH,
    REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::auth::ClientIp;
use crate::error::AppError;
use crate::repo::screenshots;
use crate::{AppState, short_id, shots, storage};

/// Requests per client IP within [`PUBLIC_WINDOW`].
pub const PUBLIC_REQUESTS_PER_IP: u32 = 300;
/// Rate limit window for public links in seconds.
pub const PUBLIC_WINDOW: i64 = 60;
/// Longest time browsers may cache an image. Short, because the owner can
/// make a screenshot private or delete it at any time.
pub const MAX_CACHE_SECONDS: i64 = 300;

pub async fn show(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let now = state.clock.now();
    let key = ip.to_string();
    if state.public_ip_limiter.is_limited(&key, now) {
        return Err(AppError::TooManyRequests {
            retry_after: PUBLIC_WINDOW,
        });
    }
    state.public_ip_limiter.record(&key, now);

    let id = name.strip_suffix(".png").unwrap_or(&name);
    if !short_id::is_well_formed(id) {
        return Err(AppError::NotFound);
    }
    let Some(shot) = screenshots::get_by_short_id(&state.db, id).await? else {
        return Err(AppError::NotFound);
    };
    // Only public screenshots have a short ID; this is a second guard.
    let Some(link) = shot.public.as_ref() else {
        return Err(AppError::NotFound);
    };
    let remaining = link.expires_at.map(|at| at - now);
    if remaining.is_some_and(|r| r <= 0) {
        return Err(AppError::NotFound);
    }

    let etag = shots::etag(&shot.sha256);
    let max_age = remaining.map_or(MAX_CACHE_SECONDS, |r| r.min(MAX_CACHE_SECONDS));
    let not_modified = headers
        .get(IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|tag| tag.trim() == etag || tag.trim() == "*")
        });

    let mut response = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let path = storage::image_path(&state.config.files_dir(), shot.id);
        let data = match tokio::fs::read(&path).await {
            Ok(data) => data,
            Err(err) => {
                tracing::warn!(screenshot_id = %shot.id, error = %err, "screenshot file missing");
                return Err(AppError::NotFound);
            }
        };
        let mut response = Body::from(data).into_response();
        let h = response.headers_mut();
        h.insert(CONTENT_TYPE, HeaderValue::from_static("image/png"));
        h.insert(
            CONTENT_DISPOSITION,
            header_value(&format!("inline; filename=\"{id}.png\"")),
        );
        response
    };
    let h = response.headers_mut();
    h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    h.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow, noarchive"),
    );
    h.insert(ETAG, header_value(&etag));
    h.insert(
        CACHE_CONTROL,
        header_value(&format!("public, max-age={max_age}")),
    );
    Ok(response)
}

/// The values passed here are built from hex digits and base62 IDs only.
fn header_value(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).unwrap_or_else(|_| HeaderValue::from_static(""))
}
