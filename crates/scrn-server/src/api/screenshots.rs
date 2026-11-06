//! Upload, list and delete screenshots.

use axum::Json;
use axum::body::Body;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use scrn_proto::{ScreenshotInfo, ScreenshotList, Visibility as ApiVisibility};
use serde::Deserialize;
use uuid::Uuid;

use super::{ApiError, ApiUser};
use crate::image::{self, ImageError, Processed};
use crate::repo::screenshots::{self, NewScreenshot, PublicLink, Screenshot, Visibility};
use crate::repo::{RepoError, users};
use crate::{AppState, crypto, short_id, shots, storage};

/// Largest accepted request body.
pub const MAX_BODY_BYTES: usize = 20 * 1024 * 1024;
/// Longest allowed link lifetime.
pub const MAX_EXPIRY_DAYS: i64 = 3650;
pub const DEFAULT_LIST_LIMIT: i64 = 20;
pub const MAX_LIST_LIMIT: i64 = 100;

#[derive(Deserialize)]
pub struct UploadParams {
    visibility: Option<String>,
    expires_in_days: Option<String>,
}

#[derive(Deserialize)]
pub struct ListParams {
    limit: Option<String>,
}

/// `POST /api/v1/screenshots`: stores a PNG sent as the raw request body.
pub async fn upload(
    State(state): State<AppState>,
    api_user: ApiUser,
    query: Result<Query<UploadParams>, QueryRejection>,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, Json<ScreenshotInfo>), ApiError> {
    let Query(params) = query.map_err(|_| ApiError::BadRequest("invalid query parameters"))?;
    let now = state.clock.now();
    let limit_key = api_user.user.id.to_string();
    if state.upload_limiter.is_limited(&limit_key, now) {
        return Err(ApiError::TooManyRequests {
            retry_after: crate::UPLOAD_WINDOW,
        });
    }
    state.upload_limiter.record(&limit_key, now);
    let visibility = parse_visibility(params.visibility.as_deref())?;
    let expiry_days = match (visibility, params.expires_in_days.as_deref()) {
        (Visibility::Private, Some(_)) => {
            return Err(ApiError::BadRequest(
                "expires_in_days is only allowed for public screenshots",
            ));
        }
        (Visibility::Private, None) => None,
        (Visibility::Public, Some(days)) => Some(parse_days(days)?),
        (Visibility::Public, None) => Some(api_user.user.default_expiry_days),
    };

    // The declared type is only a first filter; the content is decoded below
    // either way.
    if !is_png_content_type(&headers) {
        return Err(ApiError::UnsupportedMediaType);
    }
    let declared_len = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared_len.is_some_and(|len| len > MAX_BODY_BYTES as u64) {
        return Err(ApiError::PayloadTooLarge);
    }

    // Do not spend CPU on an upload that cannot fit anyway. The binding
    // check happens in the transaction that stores the row.
    let user = &api_user.user;
    if let Some(limit) = user.effective_quota(state.config.default_quota_bytes) {
        let used = screenshots::usage_for_user(&state.db, user.id).await?;
        if used >= limit {
            return Err(ApiError::QuotaExceeded { used, limit });
        }
    }

    let data = axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|_| ApiError::PayloadTooLarge)?;
    let processed = process_image(&state, data).await?;

    let id = Uuid::new_v4();
    let files_dir = state.config.files_dir();
    storage::write(&files_dir, id, &processed.png, &processed.thumb)
        .await
        .map_err(|err| ApiError::Internal(format!("writing screenshot files failed: {err}")))?;
    let shot = match store(&state, user.id, id, &processed, visibility, expiry_days).await {
        Ok(shot) => shot,
        Err(err) => {
            if let Err(remove_err) = storage::remove(&files_dir, id).await {
                tracing::warn!(screenshot_id = %id, error = %remove_err, "removing files of a failed upload failed");
            }
            return Err(err);
        }
    };
    tracing::info!(
        screenshot_id = %shot.id,
        user_id = user.id,
        bytes = shot.bytes,
        public = shot.public.is_some(),
        "screenshot uploaded"
    );
    Ok((StatusCode::CREATED, Json(info(&state, &shot))))
}

/// Decodes and re-encodes on the blocking pool, at most
/// [`crate::IMAGE_WORKERS`] at a time.
async fn process_image(state: &AppState, data: axum::body::Bytes) -> Result<Processed, ApiError> {
    let permit = state
        .image_permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::Internal("image worker pool closed".into()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        image::process(&data)
    })
    .await
    .map_err(|err| ApiError::Internal(format!("image task failed: {err}")))?
    .map_err(|err| match err {
        ImageError::Invalid => ApiError::InvalidImage,
        ImageError::TooLarge => ApiError::ImageTooLarge,
        ImageError::Encode => ApiError::Internal("encoding the image failed".into()),
    })
}

/// Checks the quota and inserts the row in one write transaction, so
/// concurrent uploads of the same user cannot exceed the quota together.
async fn store(
    state: &AppState,
    user_id: i64,
    id: Uuid,
    processed: &Processed,
    visibility: Visibility,
    expiry_days: Option<i64>,
) -> Result<Screenshot, ApiError> {
    let now = state.clock.now();
    let size = |n: usize| i64::try_from(n).map_err(|_| ApiError::ImageTooLarge);
    let (bytes, thumb_bytes) = (size(processed.png.len())?, size(processed.thumb.len())?);

    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    // Read the user again: the admin may have changed the quota meanwhile.
    let user = users::get_by_id(&mut *tx, user_id)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    let used = screenshots::usage_for_user(&mut *tx, user_id).await?;
    if let Some(limit) = user.effective_quota(state.config.default_quota_bytes)
        && used + bytes + thumb_bytes > limit
    {
        return Err(ApiError::QuotaExceeded { used, limit });
    }

    let expires_at = expiry_days.filter(|d| *d > 0).map(|d| now + d * 86_400);
    for _ in 0..shots::SHORT_ID_ATTEMPTS {
        let public = match visibility {
            Visibility::Public => Some(PublicLink {
                short_id: short_id::generate()?,
                expires_at,
            }),
            Visibility::Private => None,
        };
        let new = NewScreenshot {
            id,
            user_id,
            public,
            width: i64::from(processed.width),
            height: i64::from(processed.height),
            bytes,
            thumb_bytes,
            sha256: crypto::sha256(&processed.png),
        };
        match screenshots::insert(&mut *tx, &new, now).await {
            Ok(shot) => {
                tx.commit().await?;
                return Ok(shot);
            }
            // A failed statement does not abort the SQLite transaction, so
            // retrying with a new short ID is safe.
            Err(RepoError::Conflict) if visibility == Visibility::Public => {}
            Err(err) => return Err(err.into()),
        }
    }
    Err(ApiError::Internal("no unused short ID found".into()))
}

/// `GET /api/v1/screenshots?limit=N`: the newest screenshots of the user.
pub async fn list(
    State(state): State<AppState>,
    api_user: ApiUser,
    query: Result<Query<ListParams>, QueryRejection>,
) -> Result<Json<ScreenshotList>, ApiError> {
    let Query(params) = query.map_err(|_| ApiError::BadRequest("invalid query parameters"))?;
    let limit = match params.limit.as_deref() {
        None => DEFAULT_LIST_LIMIT,
        Some(raw) => raw
            .parse::<i64>()
            .ok()
            .filter(|n| (1..=MAX_LIST_LIMIT).contains(n))
            .ok_or(ApiError::BadRequest("limit must be between 1 and 100"))?,
    };
    let shots = screenshots::list_for_user(&state.db, api_user.user.id, None, limit, 0).await?;
    Ok(Json(ScreenshotList {
        screenshots: shots.iter().map(|shot| info(&state, shot)).collect(),
    }))
}

/// `DELETE /api/v1/screenshots/{id}`: deletes a screenshot of the user.
/// Unknown IDs and screenshots of other users give the same 404.
pub async fn delete(
    State(state): State<AppState>,
    api_user: ApiUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = Uuid::parse_str(&id).map_err(|_| ApiError::NotFound)?;
    if !shots::delete(&state.db, &state.config.files_dir(), id, api_user.user.id).await? {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

fn info(state: &AppState, shot: &Screenshot) -> ScreenshotInfo {
    ScreenshotInfo {
        id: shot.id.to_string(),
        visibility: match shot.visibility() {
            Visibility::Private => ApiVisibility::Private,
            Visibility::Public => ApiVisibility::Public,
        },
        short_url: shot
            .public
            .as_ref()
            .map(|link| format!("{}/{}", state.config.base_url, link.short_id)),
        expires_at: shot.public.as_ref().and_then(|link| link.expires_at),
        width: u32::try_from(shot.width).unwrap_or(0),
        height: u32::try_from(shot.height).unwrap_or(0),
        created_at: shot.created_at,
    }
}

fn parse_visibility(raw: Option<&str>) -> Result<Visibility, ApiError> {
    match raw {
        None | Some("private") => Ok(Visibility::Private),
        Some("public") => Ok(Visibility::Public),
        Some(_) => Err(ApiError::BadRequest("visibility must be public or private")),
    }
}

fn parse_days(raw: &str) -> Result<i64, ApiError> {
    raw.parse::<i64>()
        .ok()
        .filter(|d| (0..=MAX_EXPIRY_DAYS).contains(d))
        .ok_or(ApiError::BadRequest(
            "expires_in_days must be between 0 and 3650",
        ))
}

/// True for `image/png`, ignoring case and parameters.
fn is_png_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("image/png"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn content_type_check() {
        let check = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_str(value).unwrap());
            is_png_content_type(&headers)
        };
        assert!(check("image/png"));
        assert!(check("Image/PNG; charset=binary"));
        assert!(!check("image/jpeg"));
        assert!(!check("text/plain"));
        assert!(!check("image/pngx"));
        assert!(!is_png_content_type(&HeaderMap::new()));
    }

    #[test]
    fn query_values() {
        assert_eq!(parse_visibility(None).unwrap(), Visibility::Private);
        assert_eq!(
            parse_visibility(Some("public")).unwrap(),
            Visibility::Public
        );
        assert!(parse_visibility(Some("Public")).is_err());
        assert_eq!(parse_days("0").unwrap(), 0);
        assert_eq!(parse_days("3650").unwrap(), 3650);
        for bad in ["-1", "3651", "x", ""] {
            assert!(parse_days(bad).is_err(), "{bad}");
        }
    }
}
