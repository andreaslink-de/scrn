//! Authentication: passwords, sessions, CSRF protection and the request
//! extractors for the current user and the client IP.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts, OptionalFromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue};

use crate::AppState;
use crate::cookies::{self, CookieSpec, SameSite};
use crate::crypto::{self, SecretToken};
use crate::error::AppError;
use crate::repo::users::{Role, User};
use crate::repo::{Sha256, sessions, users};

/// Sessions expire after this many seconds without activity.
pub const SESSION_TTL: i64 = 14 * 24 * 60 * 60;
/// Activity is written to the database at most this often per session.
const SESSION_TOUCH_INTERVAL: i64 = 5 * 60;

pub const MIN_PASSWORD_CHARS: usize = 12;
/// Upper bound that keeps argon2 work per request predictable.
pub const MAX_PASSWORD_CHARS: usize = 1024;
pub const MAX_USERNAME_CHARS: usize = 64;

const SESSION_COOKIE: &str = "scrn_session";
const CSRF_COOKIE: &str = "scrn_csrf";

// --- Validation -------------------------------------------------------------

/// A validation problem in a form, mapped to a catalog message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldError {
    UsernameInvalid,
    PasswordTooShort,
    PasswordTooLong,
    PasswordMismatch,
}

impl FieldError {
    /// Translated, escaped message.
    pub fn message(self, ctx: &crate::html::Ctx<'_>) -> String {
        let min = MIN_PASSWORD_CHARS.to_string();
        let max = MAX_PASSWORD_CHARS.to_string();
        match self {
            Self::UsernameInvalid => ctx.t("error.username_invalid"),
            Self::PasswordTooShort => ctx.tf("error.password_too_short", &[("min", &min)]),
            Self::PasswordTooLong => ctx.tf("error.password_too_long", &[("max", &max)]),
            Self::PasswordMismatch => ctx.t("error.password_mismatch"),
        }
    }
}

/// Usernames: 1 to 64 ASCII letters, digits, `.`, `-` and `_`.
pub fn validate_username(username: &str) -> Result<(), FieldError> {
    let valid = !username.is_empty()
        && username.len() <= MAX_USERNAME_CHARS
        && username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    if valid {
        Ok(())
    } else {
        Err(FieldError::UsernameInvalid)
    }
}

pub fn validate_new_password(password: &str, confirm: &str) -> Result<(), FieldError> {
    let chars = password.chars().count();
    if chars < MIN_PASSWORD_CHARS {
        return Err(FieldError::PasswordTooShort);
    }
    if chars > MAX_PASSWORD_CHARS {
        return Err(FieldError::PasswordTooLong);
    }
    if password != confirm {
        return Err(FieldError::PasswordMismatch);
    }
    Ok(())
}

// --- Passwords --------------------------------------------------------------

/// Hashes a password on the blocking thread pool.
pub async fn hash_password(password: String) -> Result<String, AppError> {
    Ok(tokio::task::spawn_blocking(move || crypto::hash_password(&password)).await??)
}

/// Verifies a password on the blocking thread pool.
pub async fn verify_password(password: String, phc: String) -> Result<bool, AppError> {
    Ok(tokio::task::spawn_blocking(move || crypto::verify_password(&password, &phc)).await?)
}

// --- Sessions ---------------------------------------------------------------

fn session_cookie(state: &AppState) -> (String, bool) {
    let secure = state.config.cookie_secure;
    (cookies::name(SESSION_COOKIE, secure), secure)
}

/// Creates a session for `user_id` and returns the `Set-Cookie` header.
pub async fn start_session(state: &AppState, user_id: i64) -> Result<HeaderValue, AppError> {
    let now = state.clock.now();
    let token = SecretToken::generate()?;
    sessions::create(&state.db, &token.hash, user_id, now, now + SESSION_TTL).await?;
    let (name, secure) = session_cookie(state);
    CookieSpec {
        name: &name,
        secure,
        same_site: SameSite::Lax,
        max_age: Some(SESSION_TTL),
    }
    .set(&token.token)
    .ok_or_else(|| AppError::Internal("invalid session cookie value".into()))
}

/// `Set-Cookie` header that removes the session cookie.
pub fn clear_session_cookie(state: &AppState) -> Result<HeaderValue, AppError> {
    let (name, secure) = session_cookie(state);
    CookieSpec {
        name: &name,
        secure,
        same_site: SameSite::Lax,
        max_age: None,
    }
    .clear()
    .ok_or_else(|| AppError::Internal("invalid cookie".into()))
}

/// The logged-in user of a request.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub user: User,
    pub session_hash: Sha256,
    /// CSRF token for forms of this session.
    pub csrf: String,
}

impl CurrentUser {
    pub fn is_admin(&self) -> bool {
        self.user.role == Role::Admin
    }

    /// Checks the CSRF token of a submitted form.
    pub fn verify_csrf(&self, submitted: &str) -> Result<(), AppError> {
        if crypto::constant_time_eq(self.csrf.as_bytes(), submitted.as_bytes()) {
            Ok(())
        } else {
            Err(AppError::Csrf)
        }
    }
}

/// Derives the CSRF token of a session from its secret ID. It cannot be
/// computed without the session cookie, and it differs from the stored hash.
fn session_csrf(session_token: &str) -> String {
    crypto::hex_encode(&crypto::sha256(
        format!("scrn-csrf-v1:{session_token}").as_bytes(),
    ))
}

async fn load_current_user(
    parts: &Parts,
    state: &AppState,
) -> Result<Option<CurrentUser>, AppError> {
    let (name, _) = session_cookie(state);
    let Some(token) = cookies::get(&parts.headers, &name) else {
        return Ok(None);
    };
    let Some(hash) = crypto::hash_hex_token(token) else {
        return Ok(None);
    };
    let now = state.clock.now();
    let Some(session) = sessions::get_valid(&state.db, &hash, now).await? else {
        return Ok(None);
    };
    let Some(user) = users::get_by_id(&state.db, session.user_id).await? else {
        return Ok(None);
    };
    if user.is_disabled() {
        sessions::delete(&state.db, &hash).await?;
        return Ok(None);
    }
    if now - session.last_seen_at >= SESSION_TOUCH_INTERVAL {
        sessions::touch(&state.db, &hash, now, now + SESSION_TTL).await?;
    }
    Ok(Some(CurrentUser {
        user,
        session_hash: hash,
        csrf: session_csrf(token),
    }))
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        load_current_user(parts, state)
            .await?
            .ok_or(AppError::LoginRequired)
    }
}

impl OptionalFromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Option<Self>, AppError> {
        load_current_user(parts, state).await
    }
}

/// A logged-in admin. Other users get 403.
#[derive(Debug, Clone)]
pub struct AdminUser(pub CurrentUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let user =
            <CurrentUser as FromRequestParts<AppState>>::from_request_parts(parts, state).await?;
        if user.is_admin() {
            Ok(Self(user))
        } else {
            Err(AppError::Forbidden)
        }
    }
}

// --- CSRF for logged-out forms ---------------------------------------------

/// CSRF token for forms shown before login (setup, login, registration),
/// using the double-submit cookie pattern. Returns the token and, if the
/// cookie has to be created, the `Set-Cookie` header.
pub fn form_csrf(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<(String, Option<HeaderValue>), AppError> {
    let secure = state.config.cookie_secure;
    let name = cookies::name(CSRF_COOKIE, secure);
    if let Some(existing) =
        cookies::get(headers, &name).filter(|t| crypto::hash_hex_token(t).is_some())
    {
        return Ok((existing.to_owned(), None));
    }
    let token = SecretToken::generate()?.token;
    let cookie = CookieSpec {
        name: &name,
        secure,
        same_site: SameSite::Strict,
        max_age: None,
    }
    .set(&token)
    .ok_or_else(|| AppError::Internal("invalid CSRF cookie value".into()))?;
    Ok((token, Some(cookie)))
}

/// Checks a logged-out form submission against the CSRF cookie.
pub fn verify_form_csrf(
    headers: &HeaderMap,
    state: &AppState,
    submitted: &str,
) -> Result<(), AppError> {
    let name = cookies::name(CSRF_COOKIE, state.config.cookie_secure);
    match cookies::get(headers, &name) {
        Some(cookie)
            if crypto::hash_hex_token(cookie).is_some()
                && crypto::constant_time_eq(cookie.as_bytes(), submitted.as_bytes()) =>
        {
            Ok(())
        }
        _ => Err(AppError::Csrf),
    }
}

// --- Client IP --------------------------------------------------------------

/// The client IP, taking trusted proxies into account (see
/// [`crate::net::client_ip`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Use the extractor rather than reading the extension directly, so
        // `MockConnectInfo` works in tests. Without connection info (which
        // `serve` always provides) all clients share one rate limit key.
        let peer = ConnectInfo::<SocketAddr>::from_request_parts(parts, state)
            .await
            .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |ci| ci.0.ip());
        let forwarded = parts
            .headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok());
        Ok(Self(crate::net::client_ip(
            peer,
            forwarded,
            &state.config.trusted_proxies,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_rules() {
        for ok in ["a", "alice", "Alice.Smith", "a-b_c", &"x".repeat(64)] {
            assert_eq!(validate_username(ok), Ok(()), "{ok}");
        }
        for bad in ["", "a b", "älice", "a/b", "<x>", &"x".repeat(65)] {
            assert_eq!(
                validate_username(bad),
                Err(FieldError::UsernameInvalid),
                "{bad}"
            );
        }
    }

    #[test]
    fn password_rules() {
        assert_eq!(
            validate_new_password("short", "short"),
            Err(FieldError::PasswordTooShort)
        );
        // Characters, not bytes: 12 umlauts are long enough.
        let umlauts = "ä".repeat(12);
        assert_eq!(validate_new_password(&umlauts, &umlauts), Ok(()));
        let long = "x".repeat(MAX_PASSWORD_CHARS + 1);
        assert_eq!(
            validate_new_password(&long, &long),
            Err(FieldError::PasswordTooLong)
        );
        assert_eq!(
            validate_new_password("123456789012", "123456789013"),
            Err(FieldError::PasswordMismatch)
        );
    }

    #[test]
    fn session_csrf_is_stable_and_differs_from_the_stored_hash() {
        let token = "a".repeat(64);
        assert_eq!(session_csrf(&token), session_csrf(&token));
        assert_ne!(session_csrf(&token), session_csrf(&"b".repeat(64)));
        assert_ne!(
            session_csrf(&token),
            crypto::hex_encode(&crypto::sha256(token.as_bytes()))
        );
    }
}
