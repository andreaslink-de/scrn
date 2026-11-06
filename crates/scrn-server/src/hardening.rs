//! Protections that apply to every request: security headers, time limits
//! and turning a panicking handler into a 500 response instead of a dropped
//! connection.

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::header::{
    CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY, STRICT_TRANSPORT_SECURITY,
    X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures_util::FutureExt;

use crate::AppState;
use crate::html::PAGE_CSP;

/// Time limit for a whole request, including reading the body.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Uploads may take longer: 20 MiB over a slow uplink plus processing.
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Largest body for forms and JSON. Uploads read their body themselves
/// with their own limit.
pub const FORM_BODY_LIMIT: usize = 64 * 1024;

/// Browser features no page of SCRN uses. Passkeys (publickey-credentials)
/// keep their default of "same origin only".
pub const PERMISSIONS_POLICY: &str = "accelerometer=(), camera=(), geolocation=(), gyroscope=(), \
     magnetometer=(), microphone=(), payment=(), usb=(), interest-cohort=()";
/// Policy for responses that did not set their own (JSON, plain text).
pub const FALLBACK_CSP: &str = "default-src 'none'; frame-ancestors 'none'";
/// One year. Subdomains are left out on purpose: they may host other
/// services of the same domain; add `includeSubDomains` in the proxy if
/// that is wanted.
pub const HSTS: &str = "max-age=31536000";

const PERMISSIONS_POLICY_HEADER: HeaderName = HeaderName::from_static("permissions-policy");
const COOP: HeaderName = HeaderName::from_static("cross-origin-opener-policy");

/// Middleware: adds security headers without overriding ones a handler set
/// on purpose (for example the sandbox policy of images).
pub async fn security_headers(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    apply_headers(response.headers_mut(), is_https(&state.config.base_url));
    response
}

pub fn is_https(base_url: &str) -> bool {
    base_url.starts_with("https://")
}

fn apply_headers(headers: &mut HeaderMap, https: bool) {
    let html = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    let set = |headers: &mut HeaderMap, name: HeaderName, value: &'static str| {
        headers
            .entry(name)
            .or_insert(HeaderValue::from_static(value));
    };
    set(headers, X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(headers, PERMISSIONS_POLICY_HEADER, PERMISSIONS_POLICY);
    set(headers, REFERRER_POLICY, "same-origin");
    if html {
        set(headers, CONTENT_SECURITY_POLICY, PAGE_CSP);
        set(headers, X_FRAME_OPTIONS, "DENY");
        set(headers, COOP, "same-origin");
    } else {
        set(headers, CONTENT_SECURITY_POLICY, FALLBACK_CSP);
    }
    if https {
        set(headers, STRICT_TRANSPORT_SECURITY, HSTS);
    }
}

/// Middleware: answers 408 when a request takes longer than its limit.
pub async fn timeout(request: Request, next: Next) -> Response {
    let limit = if is_upload(request.method(), request.uri().path()) {
        UPLOAD_TIMEOUT
    } else {
        REQUEST_TIMEOUT
    };
    let path = request.uri().path().to_owned();
    match tokio::time::timeout(limit, next.run(request)).await {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!(path = %path, seconds = limit.as_secs(), "request timed out");
            (StatusCode::REQUEST_TIMEOUT, "request timeout\n").into_response()
        }
    }
}

fn is_upload(method: &Method, path: &str) -> bool {
    method == Method::POST && path == "/api/v1/screenshots"
}

/// Middleware: a panic in a handler becomes a logged 500 response. Handlers
/// are written not to panic; this is the safety net.
pub async fn catch_panic(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    match AssertUnwindSafe(next.run(request)).catch_unwind().await {
        Ok(response) => response,
        Err(_) => {
            tracing::error!(path = %path, "request handler panicked");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error\n").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_gets_the_page_policy_and_others_the_fallback() {
        let mut html = HeaderMap::new();
        html.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        apply_headers(&mut html, true);
        assert_eq!(html[CONTENT_SECURITY_POLICY], PAGE_CSP);
        assert_eq!(html[X_FRAME_OPTIONS], "DENY");
        assert_eq!(html[STRICT_TRANSPORT_SECURITY], HSTS);
        assert_eq!(html["permissions-policy"], PERMISSIONS_POLICY);
        assert_eq!(html["cross-origin-opener-policy"], "same-origin");

        let mut json = HeaderMap::new();
        json.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        apply_headers(&mut json, false);
        assert_eq!(json[CONTENT_SECURITY_POLICY], FALLBACK_CSP);
        assert_eq!(json[X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert!(!json.contains_key(STRICT_TRANSPORT_SECURITY));
        assert!(!json.contains_key(X_FRAME_OPTIONS));
    }

    #[test]
    fn handler_headers_are_kept() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("image/png"));
        headers.insert(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; sandbox"),
        );
        apply_headers(&mut headers, false);
        assert_eq!(
            headers[CONTENT_SECURITY_POLICY],
            "default-src 'none'; sandbox"
        );
    }

    #[test]
    fn uploads_get_the_long_timeout() {
        assert!(is_upload(&Method::POST, "/api/v1/screenshots"));
        assert!(!is_upload(&Method::GET, "/api/v1/screenshots"));
        assert!(!is_upload(&Method::POST, "/login"));
    }
}
