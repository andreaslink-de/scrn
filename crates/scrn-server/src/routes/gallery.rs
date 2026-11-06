//! The gallery at `/app` and the detail view of a screenshot. Every route
//! here only ever touches screenshots of the logged-in user; anything else
//! is a 404, exactly as if the ID did not exist.

use askama::Template;
use axum::Form;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::html::{self, Page};
use crate::i18n;
use crate::image;
use crate::repo::screenshots::{self, Screenshot, Visibility};
use crate::{AppState, shots, storage};

/// Screenshots per gallery page.
pub const PAGE_SIZE: i64 = 24;
/// Longest link lifetime that can be set in the gallery, in days.
pub const MAX_EXPIRY_DAYS: i64 = 3650;
const DAY: i64 = 86_400;
const EXPIRY_PRESETS: [i64; 4] = [0, 1, 7, 30];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Public,
    Private,
}

impl Filter {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("public") => Self::Public,
            Some("private") => Self::Private,
            _ => Self::All,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Public => "public",
            Self::Private => "private",
        }
    }

    fn visibility(self) -> Option<Visibility> {
        match self {
            Self::All => None,
            Self::Public => Some(Visibility::Public),
            Self::Private => Some(Visibility::Private),
        }
    }
}

/// What a gallery card shows.
struct Card {
    id: String,
    thumb_width: u32,
    thumb_height: u32,
    created_at: i64,
    public: bool,
    expired: bool,
    /// Only for public, unexpired screenshots.
    short_url: Option<String>,
}

impl Card {
    fn new(state: &AppState, shot: &Screenshot, now: i64) -> Self {
        let width = u32::try_from(shot.width).unwrap_or(1);
        let height = u32::try_from(shot.height).unwrap_or(1);
        let (thumb_width, thumb_height) = image::thumb_size(width, height);
        let expired = is_expired(shot, now);
        Self {
            id: shot.id.to_string(),
            thumb_width,
            thumb_height,
            created_at: shot.created_at,
            public: shot.public.is_some(),
            expired,
            short_url: short_url(state, shot).filter(|_| !expired),
        }
    }
}

fn is_expired(shot: &Screenshot, now: i64) -> bool {
    shot.public
        .as_ref()
        .and_then(|link| link.expires_at)
        .is_some_and(|at| at <= now)
}

fn short_url(state: &AppState, shot: &Screenshot) -> Option<String> {
    shot.public
        .as_ref()
        .map(|link| format!("{}/{}", state.config.base_url, link.short_id))
}

#[derive(Template)]
#[template(path = "gallery.html")]
struct GalleryPage {
    page: Page,
    filter: &'static str,
    cards: Vec<Card>,
    next: Option<String>,
}

#[derive(Template)]
#[template(path = "gallery_cards.html")]
struct GalleryCards {
    page: Page,
    cards: Vec<Card>,
    next: Option<String>,
}

#[derive(Deserialize)]
pub struct GalleryQuery {
    filter: Option<String>,
    page: Option<String>,
}

/// `GET /app`. htmx requests (`HX-Request`) get only the next cards.
pub async fn gallery(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(query): Query<GalleryQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let filter = Filter::parse(query.filter.as_deref());
    let page_number = query
        .page
        .as_deref()
        .and_then(|p| p.parse::<i64>().ok())
        .filter(|p| (1..=100_000).contains(p))
        .unwrap_or(1);
    // One extra row tells whether there is a next page.
    let mut shots = screenshots::list_for_user(
        &state.db,
        user.user.id,
        filter.visibility(),
        PAGE_SIZE + 1,
        (page_number - 1) * PAGE_SIZE,
    )
    .await?;
    let has_more = shots.len() > PAGE_SIZE as usize;
    shots.truncate(PAGE_SIZE as usize);
    let now = state.clock.now();
    let cards = shots.iter().map(|s| Card::new(&state, s, now)).collect();
    let next = has_more.then(|| format!("/app?filter={}&page={}", filter.code(), page_number + 1));

    let page = Page::user(&user, "gallery.title", "/app");
    if headers.contains_key("hx-request") {
        return html::render(StatusCode::OK, &GalleryCards { page, cards, next });
    }
    html::render(
        StatusCode::OK,
        &GalleryPage {
            page,
            filter: filter.code(),
            cards,
            next,
        },
    )
}

#[derive(Template)]
#[template(path = "shot.html")]
struct ShotPage {
    page: Page,
    csrf: String,
    id: String,
    width: i64,
    height: i64,
    bytes: i64,
    created_at: i64,
    public: bool,
    expired: bool,
    short_url: Option<String>,
    expires_at: Option<i64>,
    expiry: String,
    custom_date: String,
    min_date: String,
    max_date: String,
    error: Option<String>,
}

async fn load(state: &AppState, user: &CurrentUser, id: &str) -> Result<Screenshot, AppError> {
    let id = Uuid::parse_str(id).map_err(|_| AppError::NotFound)?;
    screenshots::get_for_user(&state.db, id, user.user.id)
        .await?
        .ok_or(AppError::NotFound)
}

/// `GET /app/shots/{id}`.
pub async fn detail(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let shot = load(&state, &user, &id).await?;
    render_detail(&state, &user, &shot, None, StatusCode::OK)
}

fn render_detail(
    state: &AppState,
    user: &CurrentUser,
    shot: &Screenshot,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, AppError> {
    let now = state.clock.now();
    let expires_at = shot.public.as_ref().and_then(|l| l.expires_at);
    // The form shows the current expiry: "never" or the date it ends on.
    let (expiry, custom_date) = match expires_at {
        None => ("0".to_owned(), String::new()),
        Some(at) => ("custom".to_owned(), html::format_date(at - 1)),
    };
    let expired = is_expired(shot, now);
    let page = Page::user(user, "shot.title", "/app");
    html::render(
        status,
        &ShotPage {
            page,
            csrf: user.csrf.clone(),
            id: shot.id.to_string(),
            width: shot.width,
            height: shot.height,
            bytes: shot.bytes,
            created_at: shot.created_at,
            public: shot.public.is_some(),
            expired,
            short_url: short_url(state, shot).filter(|_| !expired),
            expires_at,
            expiry,
            custom_date,
            min_date: html::format_date(now),
            max_date: html::format_date(now + MAX_EXPIRY_DAYS * DAY),
            error: error.map(|key| i18n::catalogs().t(user.user.language, key).to_owned()),
        },
    )
}

/// `GET /app/shots/{id}/image` and `/thumb`: the files, for the owner only.
pub async fn image(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    serve_file(&state, &user, &id, false).await
}

pub async fn thumb(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    serve_file(&state, &user, &id, true).await
}

async fn serve_file(
    state: &AppState,
    user: &CurrentUser,
    id: &str,
    thumb: bool,
) -> Result<Response, AppError> {
    let shot = load(state, user, id).await?;
    let files = state.config.files_dir();
    let path = if thumb {
        storage::thumb_path(&files, shot.user_id, shot.id)
    } else {
        storage::image_path(&files, shot.user_id, shot.id)
    };
    let data = tokio::fs::read(&path).await.map_err(|err| {
        tracing::warn!(screenshot_id = %shot.id, error = %err, "screenshot file missing");
        AppError::NotFound
    })?;
    let mut response = Body::from(data).into_response();
    let h = response.headers_mut();
    h.insert(CONTENT_TYPE, HeaderValue::from_static("image/png"));
    h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    // Only the owner's browser may keep a copy, and only briefly.
    h.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=300"),
    );
    if let Ok(etag) = HeaderValue::from_str(&shots::etag(&shot.sha256)) {
        h.insert(ETAG, etag);
    }
    Ok(response)
}

#[derive(Deserialize)]
pub struct VisibilityForm {
    csrf: String,
    visibility: String,
}

/// `POST /app/shots/{id}/visibility`. Making an image public always creates
/// a new short ID with the user's default expiry; making it private deletes
/// the short ID, so the old link is dead for good.
pub async fn set_visibility(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Form(form): Form<VisibilityForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    let shot = load(&state, &user, &id).await?;
    match form.visibility.as_str() {
        "public" => {
            let days = user.user.default_expiry_days;
            let expires_at = (days > 0).then(|| state.clock.now() + days * DAY);
            shots::make_public(&state.db, shot.id, user.user.id, expires_at)
                .await
                .map_err(|err| AppError::Internal(err.to_string()))?
                .ok_or(AppError::NotFound)?;
            tracing::info!(screenshot_id = %shot.id, user_id = user.user.id, "screenshot made public");
        }
        "private" => {
            if !screenshots::set_private(&state.db, shot.id, user.user.id).await? {
                return Err(AppError::NotFound);
            }
            tracing::info!(screenshot_id = %shot.id, user_id = user.user.id, "screenshot made private");
        }
        _ => return Err(AppError::BadRequest),
    }
    Ok(Redirect::to(&format!("/app/shots/{}", shot.id)).into_response())
}

#[derive(Deserialize)]
pub struct ExpiryForm {
    csrf: String,
    expiry: String,
    #[serde(default)]
    custom_date: String,
}

/// `POST /app/shots/{id}/expiry`: never, 1, 7 or 30 days from now, or the
/// end (UTC) of a chosen date.
pub async fn set_expiry(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Form(form): Form<ExpiryForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    let shot = load(&state, &user, &id).await?;
    if shot.public.is_none() {
        return render_detail(
            &state,
            &user,
            &shot,
            Some("error.expiry_private"),
            StatusCode::CONFLICT,
        );
    }
    let now = state.clock.now();
    let expires_at = match form.expiry.as_str() {
        "custom" => {
            let end = html::parse_date(&form.custom_date).map(|days| (days + 1) * DAY);
            match end.filter(|at| *at > now && *at <= now + (MAX_EXPIRY_DAYS + 1) * DAY) {
                Some(at) => Some(at),
                None => {
                    return render_detail(
                        &state,
                        &user,
                        &shot,
                        Some("error.expiry_date"),
                        StatusCode::UNPROCESSABLE_ENTITY,
                    );
                }
            }
        }
        choice => {
            let days = choice
                .parse::<i64>()
                .ok()
                .filter(|d| EXPIRY_PRESETS.contains(d))
                .ok_or(AppError::BadRequest)?;
            (days > 0).then(|| now + days * DAY)
        }
    };
    if !screenshots::set_expiry(&state.db, shot.id, user.user.id, expires_at).await? {
        return Err(AppError::NotFound);
    }
    tracing::info!(screenshot_id = %shot.id, user_id = user.user.id, expires_at = ?expires_at, "link expiry changed");
    Ok(Redirect::to(&format!("/app/shots/{}", shot.id)).into_response())
}

#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
}

/// `POST /app/shots/{id}/delete`: removes the row, the image and the
/// thumbnail.
pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    user.verify_csrf(&form.csrf)?;
    let shot = load(&state, &user, &id).await?;
    if !shots::delete(&state.db, &state.config.files_dir(), shot.id, user.user.id).await? {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to("/app").into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        assert_eq!(Filter::parse(None), Filter::All);
        assert_eq!(Filter::parse(Some("public")), Filter::Public);
        assert_eq!(Filter::parse(Some("private")), Filter::Private);
        assert_eq!(Filter::parse(Some("other")), Filter::All);
        assert_eq!(Filter::Public.visibility(), Some(Visibility::Public));
    }
}
