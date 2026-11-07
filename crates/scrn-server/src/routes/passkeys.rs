//! Passkey ceremonies (JSON, driven by `static/app.js`) and the passkey
//! management forms of the settings page.
//!
//! The JSON endpoints are protected against CSRF with the `X-CSRF-Token`
//! header: the session token for logged-in users, the double-submit cookie
//! token on the login page. A JSON body with that header cannot be sent
//! cross-site without a CORS preflight, which this server never allows.

use axum::Form;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};
use webauthn_rs::prelude::{Passkey, PublicKeyCredential, RegisterPublicKeyCredential};

use super::settings;
use crate::AppState;
use crate::auth::{self, ClientIp, CurrentUser};
use crate::error::AppError;
use crate::i18n;
use crate::passkey::{self, Authentication, Passkeys, Registration};
use crate::prefs::Prefs;
use crate::repo::passkeys::{self as repo, NewPasskey};
use crate::repo::users::{self, Language};
use crate::repo::{RepoError, sessions};

const CSRF_HEADER: &str = "x-csrf-token";

/// Error of a JSON endpoint: a status and a translated message that the
/// page shows as is.
#[derive(Debug)]
pub struct JsonError {
    status: StatusCode,
    message: String,
}

impl JsonError {
    fn new(status: StatusCode, lang: Language, key: &str) -> Self {
        Self {
            status,
            message: i18n::catalogs().t(lang, key).to_owned(),
        }
    }

    fn internal(lang: Language, detail: impl std::fmt::Display) -> Self {
        tracing::error!(error = %detail, "passkey request failed");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, lang, "error.internal")
    }
}

impl IntoResponse for JsonError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct Body {
            message: String,
        }
        (
            self.status,
            Json(Body {
                message: self.message,
            }),
        )
            .into_response()
    }
}

#[derive(Serialize)]
pub struct Started<T> {
    ceremony: String,
    options: T,
}

#[derive(Serialize)]
pub struct Done {
    redirect: &'static str,
}

fn csrf_header(headers: &HeaderMap) -> &str {
    headers
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

fn enabled(state: &AppState, lang: Language) -> Result<&Passkeys, JsonError> {
    state
        .passkeys
        .as_deref()
        .ok_or_else(|| JsonError::new(StatusCode::NOT_FOUND, lang, "passkey.unavailable"))
}

fn parse_stored(row: &repo::Passkey) -> Result<Passkey, serde_json::Error> {
    serde_json::from_str(&row.credential)
}

// --- Registration (logged in) ---------------------------------------------

pub async fn register_start(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
) -> Result<Json<Started<impl Serialize>>, JsonError> {
    let lang = user.user.language;
    user.verify_csrf(csrf_header(&headers))
        .map_err(|_| JsonError::new(StatusCode::FORBIDDEN, lang, "error.csrf"))?;
    let passkeys = enabled(&state, lang)?;
    // Authenticators refuse to register a second credential for this site
    // if they already hold one of these.
    let existing = repo::list_for_user(&state.db, user.user.id)
        .await
        .map_err(|e| JsonError::internal(lang, e))?;
    let exclude = existing
        .iter()
        .filter_map(|row| parse_stored(row).ok())
        .map(|pk| pk.cred_id().clone())
        .collect();
    let name = &user.user.username;
    let (options, reg_state) = passkeys
        .webauthn
        .start_passkey_registration(
            passkey::user_handle(user.user.id),
            name,
            name,
            Some(exclude),
        )
        .map_err(|e| JsonError::internal(lang, e))?;
    let ceremony = passkeys
        .registrations
        .insert(
            Registration {
                user_id: user.user.id,
                state: reg_state,
            },
            state.clock.now(),
        )
        .map_err(|_| JsonError::new(StatusCode::SERVICE_UNAVAILABLE, lang, "passkey.busy"))?;
    Ok(Json(Started { ceremony, options }))
}

#[derive(Deserialize)]
pub struct RegisterFinish {
    ceremony: String,
    name: String,
    credential: RegisterPublicKeyCredential,
}

pub async fn register_finish(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    Json(body): Json<RegisterFinish>,
) -> Result<Json<Done>, JsonError> {
    let lang = user.user.language;
    user.verify_csrf(csrf_header(&headers))
        .map_err(|_| JsonError::new(StatusCode::FORBIDDEN, lang, "error.csrf"))?;
    let passkeys = enabled(&state, lang)?;
    let name = settings::valid_name(&body.name).ok_or_else(|| {
        JsonError::new(StatusCode::UNPROCESSABLE_ENTITY, lang, "error.passkey_name")
    })?;
    let pending = passkeys
        .registrations
        .take(&body.ceremony, state.clock.now())
        .filter(|reg| reg.user_id == user.user.id)
        .ok_or_else(|| JsonError::new(StatusCode::BAD_REQUEST, lang, "passkey.expired"))?;
    let passkey = passkeys
        .webauthn
        .finish_passkey_registration(&body.credential, &pending.state)
        .map_err(|err| {
            tracing::info!(user_id = user.user.id, error = %err, "passkey registration rejected");
            JsonError::new(StatusCode::BAD_REQUEST, lang, "passkey.failed")
        })?;
    let credential = serde_json::to_string(&passkey).map_err(|e| JsonError::internal(lang, e))?;
    let created = repo::create(
        &state.db,
        &NewPasskey {
            user_id: user.user.id,
            name,
            credential_id: passkey.cred_id().as_ref(),
            credential: &credential,
        },
        state.clock.now(),
    )
    .await;
    match created {
        Ok(row) => {
            tracing::info!(
                passkey_id = row.id,
                user_id = user.user.id,
                "passkey registered"
            );
            Ok(Json(Done {
                redirect: "/app/settings?passkey=added#passkeys",
            }))
        }
        Err(RepoError::Conflict) => Err(JsonError::new(
            StatusCode::CONFLICT,
            lang,
            "passkey.duplicate",
        )),
        Err(err) => Err(JsonError::internal(lang, err)),
    }
}

// --- Login ---------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginStart {
    username: String,
}

/// Starts a passkey login for a username. Unknown and disabled users and
/// users without passkeys get the same answer.
pub async fn login_start(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    prefs: Prefs,
    headers: HeaderMap,
    Json(body): Json<LoginStart>,
) -> Result<Json<Started<impl Serialize>>, JsonError> {
    let lang = prefs.lang;
    auth::verify_form_csrf(&headers, &state, csrf_header(&headers))
        .map_err(|_| JsonError::new(StatusCode::FORBIDDEN, lang, "error.csrf"))?;
    let passkeys = enabled(&state, lang)?;
    let now = state.clock.now();
    let ip_key = ip.to_string();
    if state.login_ip_limiter.is_limited(&ip_key, now) {
        return Err(JsonError::new(
            StatusCode::TOO_MANY_REQUESTS,
            lang,
            "error.too_many_attempts",
        ));
    }
    state.login_ip_limiter.record(&ip_key, now);

    let unavailable = || JsonError::new(StatusCode::BAD_REQUEST, lang, "login.passkey_unavailable");
    let username = body.username.trim();
    if username.is_empty() || username.len() > auth::MAX_USERNAME_CHARS {
        return Err(unavailable());
    }
    let user = users::get_by_username(&state.db, username)
        .await
        .map_err(|e| JsonError::internal(lang, e))?
        .filter(|u| !u.is_disabled())
        .ok_or_else(unavailable)?;
    let credentials: Vec<Passkey> = repo::list_for_user(&state.db, user.id)
        .await
        .map_err(|e| JsonError::internal(lang, e))?
        .iter()
        .filter_map(|row| parse_stored(row).ok())
        .collect();
    if credentials.is_empty() {
        return Err(unavailable());
    }
    let (options, auth_state) = passkeys
        .webauthn
        .start_passkey_authentication(&credentials)
        .map_err(|e| JsonError::internal(lang, e))?;
    let ceremony = passkeys
        .authentications
        .insert(
            Authentication {
                user_id: user.id,
                state: auth_state,
            },
            now,
        )
        .map_err(|_| JsonError::new(StatusCode::SERVICE_UNAVAILABLE, lang, "passkey.busy"))?;
    Ok(Json(Started { ceremony, options }))
}

#[derive(Deserialize)]
pub struct LoginFinish {
    ceremony: String,
    credential: PublicKeyCredential,
}

pub async fn login_finish(
    State(state): State<AppState>,
    prefs: Prefs,
    headers: HeaderMap,
    Json(body): Json<LoginFinish>,
) -> Result<Response, JsonError> {
    let lang = prefs.lang;
    auth::verify_form_csrf(&headers, &state, csrf_header(&headers))
        .map_err(|_| JsonError::new(StatusCode::FORBIDDEN, lang, "error.csrf"))?;
    let passkeys = enabled(&state, lang)?;
    let now = state.clock.now();
    let failed = || JsonError::new(StatusCode::UNAUTHORIZED, lang, "login.passkey_failed");

    let pending = passkeys
        .authentications
        .take(&body.ceremony, now)
        .ok_or_else(|| JsonError::new(StatusCode::BAD_REQUEST, lang, "passkey.expired"))?;
    let row = verify_assertion(&state, passkeys, &pending, &body.credential, lang)
        .await?
        .ok_or_else(failed)?;
    let user = users::get_by_id(&state.db, pending.user_id)
        .await
        .map_err(|e| JsonError::internal(lang, e))?
        .filter(|u| !u.is_disabled())
        .ok_or_else(failed)?;

    let cookie = auth::start_session(&state, user.id, auth::SignIn::Passkey)
        .await
        .map_err(|e| JsonError::internal(lang, e))?;
    tracing::info!(
        user_id = user.id,
        passkey_id = row.id,
        "user logged in with passkey"
    );
    let mut response = Json(Done { redirect: "/app" }).into_response();
    response.headers_mut().append(SET_COOKIE, cookie);
    for cookie in Prefs::of_user(&user).cookies(state.config.cookie_secure) {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    Ok(response)
}

/// Checks a passkey response against a pending ceremony and stores the new
/// signature counter. `Ok(None)` means the response was rejected.
async fn verify_assertion(
    state: &AppState,
    passkeys: &Passkeys,
    pending: &Authentication,
    credential: &PublicKeyCredential,
    lang: Language,
) -> Result<Option<repo::Passkey>, JsonError> {
    let result = match passkeys
        .webauthn
        .finish_passkey_authentication(credential, &pending.state)
    {
        Ok(result) => result,
        Err(err) => {
            tracing::info!(user_id = pending.user_id, error = %err, "passkey response rejected");
            return Ok(None);
        }
    };
    let Some(row) = repo::get_by_credential_id(&state.db, result.cred_id().as_ref())
        .await
        .map_err(|e| JsonError::internal(lang, e))?
        .filter(|row| row.user_id == pending.user_id)
    else {
        return Ok(None);
    };
    // Store the new signature counter and backup state.
    let mut stored = parse_stored(&row).map_err(|e| JsonError::internal(lang, e))?;
    stored.update_credential(&result);
    let updated = serde_json::to_string(&stored).map_err(|e| JsonError::internal(lang, e))?;
    repo::record_use(&state.db, row.id, &updated, state.clock.now())
        .await
        .map_err(|e| JsonError::internal(lang, e))?;
    Ok(Some(row))
}

// --- Confirmation (logged in) ------------------------------------------------
//
// A logged-in user proves again that they hold one of their passkeys. The
// session remembers the time, and for a few minutes this replaces the
// current password when setting a new one.

pub async fn confirm_start(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
) -> Result<Json<Started<impl Serialize>>, JsonError> {
    let lang = user.user.language;
    user.verify_csrf(csrf_header(&headers))
        .map_err(|_| JsonError::new(StatusCode::FORBIDDEN, lang, "error.csrf"))?;
    let passkeys = enabled(&state, lang)?;
    let credentials: Vec<Passkey> = repo::list_for_user(&state.db, user.user.id)
        .await
        .map_err(|e| JsonError::internal(lang, e))?
        .iter()
        .filter_map(|row| parse_stored(row).ok())
        .collect();
    if credentials.is_empty() {
        return Err(JsonError::new(
            StatusCode::BAD_REQUEST,
            lang,
            "password.no_passkey",
        ));
    }
    let (options, auth_state) = passkeys
        .webauthn
        .start_passkey_authentication(&credentials)
        .map_err(|e| JsonError::internal(lang, e))?;
    let ceremony = passkeys
        .authentications
        .insert(
            Authentication {
                user_id: user.user.id,
                state: auth_state,
            },
            state.clock.now(),
        )
        .map_err(|_| JsonError::new(StatusCode::SERVICE_UNAVAILABLE, lang, "passkey.busy"))?;
    Ok(Json(Started { ceremony, options }))
}

pub async fn confirm_finish(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    Json(body): Json<LoginFinish>,
) -> Result<Json<Done>, JsonError> {
    let lang = user.user.language;
    user.verify_csrf(csrf_header(&headers))
        .map_err(|_| JsonError::new(StatusCode::FORBIDDEN, lang, "error.csrf"))?;
    let passkeys = enabled(&state, lang)?;
    let pending = passkeys
        .authentications
        .take(&body.ceremony, state.clock.now())
        .filter(|auth| auth.user_id == user.user.id)
        .ok_or_else(|| JsonError::new(StatusCode::BAD_REQUEST, lang, "passkey.expired"))?;
    let row = verify_assertion(&state, passkeys, &pending, &body.credential, lang)
        .await?
        .ok_or_else(|| JsonError::new(StatusCode::UNAUTHORIZED, lang, "passkey.failed"))?;
    sessions::set_passkey_at(&state.db, &user.session_hash, state.clock.now())
        .await
        .map_err(|e| JsonError::internal(lang, e))?;
    tracing::info!(
        user_id = user.user.id,
        passkey_id = row.id,
        "user confirmed with passkey"
    );
    Ok(Json(Done {
        redirect: "/app/settings?confirmed=1#password",
    }))
}

// --- Management forms --------------------------------------------------------

#[derive(Deserialize)]
pub struct RenameForm {
    csrf: String,
    name: String,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
}

pub async fn rename(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Form(form): Form<RenameForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    let Some(name) = settings::valid_name(&form.name) else {
        return settings::render_with_passkey_error(&state, &user, "error.passkey_name").await;
    };
    if !repo::rename(&state.db, id, user.user.id, name).await? {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to("/app/settings#passkeys").into_response())
}

/// Deletes a passkey unless it is the user's last way to sign in.
pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    // The write lock serializes concurrent deletes, so two requests cannot
    // remove the last two passkeys of an account without a password.
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let account = users::get_by_id(&mut *tx, user.user.id)
        .await?
        .ok_or(AppError::LoginRequired)?;
    let count = repo::count_for_user(&mut *tx, user.user.id).await?;
    if account.password_hash.is_none() && count <= 1 {
        drop(tx);
        return settings::render_with_passkey_error(&state, &user, "error.passkey_last").await;
    }
    if !repo::delete(&mut *tx, id, user.user.id).await? {
        return Err(AppError::NotFound);
    }
    tx.commit().await?;
    tracing::info!(passkey_id = id, user_id = user.user.id, "passkey deleted");
    Ok(Redirect::to("/app/settings#passkeys").into_response())
}
