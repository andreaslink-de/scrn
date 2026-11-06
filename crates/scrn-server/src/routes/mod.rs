//! HTTP routes.

mod admin;
mod gallery;
mod health;
mod home;
mod invite;
mod login;
mod passkeys;
mod prefs;
mod public;
mod settings;
mod setup;

use axum::Router;
use axum::http::HeaderValue;
use axum::http::header::SET_COOKIE;
use axum::response::Response;
use axum::routing::{get, post};

pub use gallery::PAGE_SIZE;
pub use public::{MAX_CACHE_SECONDS, PUBLIC_REQUESTS_PER_IP, PUBLIC_WINDOW};

use axum::extract::DefaultBodyLimit;

use crate::error::AppError;
use crate::{AppState, hardening};

/// Builds the router with all routes.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/", get(home::root))
        .route("/app", get(gallery::gallery))
        .route("/app/shots/{id}", get(gallery::detail))
        .route("/app/shots/{id}/image", get(gallery::image))
        .route("/app/shots/{id}/thumb", get(gallery::thumb))
        .route("/app/shots/{id}/visibility", post(gallery::set_visibility))
        .route("/app/shots/{id}/expiry", post(gallery::set_expiry))
        .route("/app/shots/{id}/delete", post(gallery::delete))
        .route("/app/settings", get(settings::show).post(settings::save))
        .route(
            "/app/tokens",
            get(settings::tokens_redirect).post(settings::create_token),
        )
        .route("/app/tokens/{id}/revoke", post(settings::revoke_token))
        .route("/prefs", post(prefs::switch))
        .route("/login/passkey/start", post(passkeys::login_start))
        .route("/login/passkey/finish", post(passkeys::login_finish))
        .route(
            "/app/passkeys/register/start",
            post(passkeys::register_start),
        )
        .route(
            "/app/passkeys/register/finish",
            post(passkeys::register_finish),
        )
        .route("/app/passkeys/{id}/rename", post(passkeys::rename))
        .route("/app/passkeys/{id}/delete", post(passkeys::delete))
        .route("/static/{file}", get(crate::html::serve_static))
        .route("/favicon.ico", get(crate::html::favicon))
        .route("/setup", get(setup::show).post(setup::submit))
        .route("/login", get(login::show).post(login::submit))
        .route("/logout", post(login::logout))
        .route("/invite/{token}", get(invite::show).post(invite::submit))
        .route("/admin", get(admin::show))
        .route("/admin/backup", post(admin::download_backup))
        .route("/admin/invites", post(admin::create_invite))
        .route("/admin/invites/{id}/delete", post(admin::delete_invite))
        .route("/admin/users/{id}/disable", post(admin::disable_user))
        .route("/admin/users/{id}/enable", post(admin::enable_user))
        .route("/admin/users/{id}/quota", post(admin::set_quota))
        // Fixed routes always win over this capture in axum's router.
        .route("/{name}", get(public::show))
        .nest(scrn_proto::API_PREFIX, crate::api::router(state.clone()))
        .fallback(not_found)
        .layer(axum::middleware::from_fn(crate::error::localize_errors))
        .layer(DefaultBodyLimit::max(hardening::FORM_BODY_LIMIT))
        .layer(axum::middleware::from_fn(hardening::timeout))
        .layer(axum::middleware::from_fn(hardening::catch_panic))
        // Outermost, so 408 and 500 responses get the headers too.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            hardening::security_headers,
        ))
        .with_state(state)
}

async fn not_found() -> AppError {
    AppError::NotFound
}

/// Adds a `Set-Cookie` header if one is given.
fn with_cookie(mut response: Response, cookie: Option<HeaderValue>) -> Response {
    if let Some(cookie) = cookie {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}
