//! Devices and their public key packages.

use axum::{
    Json, Router,
    extract::{FromRequestParts, Path, State},
    http::{StatusCode, request::Parts},
    routing::{delete, get, put},
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    AppState,
    auth::{User, now},
    error::{ApiError, ApiResult},
};

const DEVICE_HEADER: &str = "x-harmony-device";
const MAX_KEY_PACKAGE_BYTES: usize = 16 * 1024;
const MAX_ONE_TIME_PACKAGES: i64 = 200;
const MAX_NAME_CHARS: usize = 48;
/// Last-seen times are kept to the minute: enough to recognise a device,
/// no finer a record of when someone uses Harmony.
const SEEN_RESOLUTION_SECS: i64 = 60;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/devices", get(list_devices).post(create_device))
        .route("/devices/{id}", delete(revoke_device))
        .route("/devices/{id}/name", put(rename_device))
        .route(
            "/devices/{id}/key-packages",
            get(count_key_packages).post(upload_key_packages),
        )
}

pub fn decode(field: &'static str, value: &str, max: usize) -> ApiResult<Vec<u8>> {
    let bytes = B64.decode(value).map_err(|_| ApiError::BadRequest(field))?;
    if bytes.is_empty() || bytes.len() > max {
        return Err(ApiError::BadRequest(field));
    }
    Ok(bytes)
}

pub fn encode(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

/// A signed-in user acting from one of their own, non-revoked devices. The
/// device id comes from the `X-Harmony-Device` header (or `?device=` for
/// WebSockets, which cannot set headers).
pub struct DeviceAuth {
    pub user: User,
    pub device_id: i64,
}

impl FromRequestParts<AppState> for DeviceAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let user = User::from_request_parts(parts, state).await?;
        let from_header = parts
            .headers
            .get(DEVICE_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let from_query = || {
            parts
                .uri
                .query()?
                .split('&')
                .find_map(|pair| pair.strip_prefix("device="))
                .map(str::to_owned)
        };
        let device_id: i64 = from_header
            .or_else(from_query)
            .and_then(|v| v.parse().ok())
            .ok_or(ApiError::BadRequest("missing device id"))?;
        let owned: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM devices WHERE id = ? AND user_id = ? AND revoked_at IS NULL")
                .bind(device_id)
                .bind(user.id)
                .fetch_optional(&state.db)
                .await?;
        if owned.is_none() {
            return Err(ApiError::Forbidden);
        }
        // Remember which session the device is on (so signing the device out
        // ends it) and roughly when it was last used. Written at most once a
        // minute per device.
        let minute = now() / SEEN_RESOLUTION_SECS * SEEN_RESOLUTION_SECS;
        sqlx::query(
            "UPDATE devices SET last_seen_at = ?1, session_id = ?2
             WHERE id = ?3 AND (last_seen_at IS NOT ?1 OR session_id IS NOT ?2)",
        )
        .bind(minute)
        .bind(user.session_id)
        .bind(device_id)
        .execute(&state.db)
        .await?;
        Ok(Self { user, device_id })
    }
}

#[derive(Serialize)]
struct DeviceInfo {
    id: i64,
    identity: String,
    /// The owner's signature over this device's key, if they confirmed it
    /// with their recovery code.
    signature: Option<String>,
}

#[derive(Serialize)]
struct UserInfo {
    id: i64,
    username: String,
    display_name: String,
    /// Public identity key; absent until the person sets up a recovery code.
    identity_key: Option<String>,
    devices: Vec<DeviceInfo>,
}

/// Everyone on this server. Harmony is one friend group, so everyone can see everyone.
async fn list_users(State(state): State<AppState>, _user: User) -> ApiResult<Json<Vec<UserInfo>>> {
    let users: Vec<(i64, String, String, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT id, username, display_name, identity_key FROM users ORDER BY display_name COLLATE NOCASE",
    )
    .fetch_all(&state.db)
    .await?;
    let devices: Vec<(i64, i64, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT id, user_id, identity_signature FROM devices WHERE revoked_at IS NULL ORDER BY id",
    )
    .fetch_all(&state.db)
    .await?;
    let users = users
        .into_iter()
        .map(|(id, username, display_name, identity_key)| UserInfo {
            id,
            username,
            display_name,
            identity_key: identity_key.as_deref().map(encode),
            devices: devices
                .iter()
                .filter(|(_, user_id, _)| *user_id == id)
                .map(|(device_id, _, signature)| DeviceInfo {
                    id: *device_id,
                    identity: identity(id, *device_id),
                    signature: signature.as_deref().map(encode),
                })
                .collect(),
        })
        .collect();
    Ok(Json(users))
}

#[derive(Deserialize, Default)]
struct NewDevice {
    name: Option<String>,
}

fn clean_device_name(name: Option<&str>) -> ApiResult<Option<String>> {
    let name = name.map(str::trim).unwrap_or("");
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(ApiError::BadRequest("invalid device name"));
    }
    Ok(Some(name.to_owned()))
}

async fn create_device(
    State(state): State<AppState>,
    user: User,
    body: Option<Json<NewDevice>>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let name = clean_device_name(body.name.as_deref())?;
    let created = now();
    let result = sqlx::query(
        "INSERT INTO devices (user_id, created_at, name, last_seen_at, session_id) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user.id)
    .bind(created)
    .bind(name)
    .bind(created / SEEN_RESOLUTION_SECS * SEEN_RESOLUTION_SECS)
    .bind(user.session_id)
    .execute(&state.db)
    .await?;
    let id = result.last_insert_rowid();
    // The identity the device must put in its key packages. Other devices
    // check it, so a key package cannot be passed off as someone else's.
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": id, "identity": identity(user.id, id) })),
    ))
}

pub fn identity(user_id: i64, device_id: i64) -> String {
    format!("u{user_id}:d{device_id}")
}

#[derive(Deserialize)]
struct KeyPackageUpload {
    #[serde(default)]
    one_time: Vec<String>,
    last_resort: Option<String>,
}

async fn upload_key_packages(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(id): Path<i64>,
    Json(body): Json<KeyPackageUpload>,
) -> ApiResult<StatusCode> {
    if id != auth.device_id {
        return Err(ApiError::Forbidden);
    }
    let one_time: Vec<Vec<u8>> = body
        .one_time
        .iter()
        .map(|p| decode("invalid key package", p, MAX_KEY_PACKAGE_BYTES))
        .collect::<ApiResult<_>>()?;
    let last_resort = body
        .last_resort
        .as_deref()
        .map(|p| decode("invalid key package", p, MAX_KEY_PACKAGE_BYTES))
        .transpose()?;

    let mut tx = state.db.begin().await?;
    let (existing,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM key_packages WHERE device_id = ? AND last_resort = 0")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if existing + one_time.len() as i64 > MAX_ONE_TIME_PACKAGES {
        return Err(ApiError::BadRequest("too many key packages"));
    }
    for data in one_time {
        sqlx::query("INSERT INTO key_packages (device_id, last_resort, data) VALUES (?, 0, ?)")
            .bind(id)
            .bind(data)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(data) = last_resort {
        sqlx::query("DELETE FROM key_packages WHERE device_id = ? AND last_resort = 1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO key_packages (device_id, last_resort, data) VALUES (?, 1, ?)")
            .bind(id)
            .bind(data)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    // This device can now be added to chats it is missing from.
    let chats: Vec<(String,)> = sqlx::query_as("SELECT chat_id FROM chat_members WHERE user_id = ?")
        .bind(auth.user.id)
        .fetch_all(&state.db)
        .await?;
    for (chat_id,) in chats {
        crate::chats::notify_chat(&state, &chat_id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn count_key_packages(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    if id != auth.device_id {
        return Err(ApiError::Forbidden);
    }
    let (one_time,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM key_packages WHERE device_id = ? AND last_resort = 0")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    let (last_resort,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM key_packages WHERE device_id = ? AND last_resort = 1")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    Ok(Json(
        json!({ "one_time": one_time, "last_resort": last_resort > 0 }),
    ))
}

#[derive(Serialize, sqlx::FromRow)]
struct OwnDevice {
    id: i64,
    name: Option<String>,
    created_at: i64,
    last_seen_at: Option<i64>,
}

/// The caller's own devices that are still signed in, newest first.
async fn list_devices(State(state): State<AppState>, user: User) -> ApiResult<Json<Vec<OwnDevice>>> {
    let devices = sqlx::query_as(
        "SELECT id, name, created_at, last_seen_at FROM devices
         WHERE user_id = ? AND revoked_at IS NULL ORDER BY id DESC",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(devices))
}

#[derive(Deserialize)]
struct RenameBody {
    name: String,
}

async fn rename_device(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<i64>,
    Json(body): Json<RenameBody>,
) -> ApiResult<StatusCode> {
    let name = clean_device_name(Some(&body.name))?;
    let updated =
        sqlx::query("UPDATE devices SET name = ? WHERE id = ? AND user_id = ? AND revoked_at IS NULL")
            .bind(name)
            .bind(id)
            .bind(user.id)
            .execute(&state.db)
            .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Signs one of the caller's devices out for good: its login session ends,
/// it can no longer use the API, nobody can add it to a chat again, and the
/// other members' devices remove it from every chat's key group. Works from
/// any of the person's devices, including the one being removed.
async fn revoke_device(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let mut tx = state.db.begin().await?;
    let session: Option<(Option<i64>,)> = sqlx::query_as(
        "UPDATE devices SET revoked_at = ?, identity_signature = NULL
         WHERE id = ? AND user_id = ? AND revoked_at IS NULL RETURNING session_id",
    )
    .bind(now())
    .bind(id)
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((session_id,)) = session else {
        return Err(ApiError::NotFound);
    };
    if let Some(session_id) = session_id {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM key_packages WHERE device_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM welcomes WHERE device_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let chats: Vec<(String,)> = sqlx::query_as("SELECT chat_id FROM chat_devices WHERE device_id = ?")
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;

    state
        .hub
        .notify(vec![user.id], crate::hub::Notice::Revoked { device_id: id });
    crate::calls::forget_device(&state, id);
    // Other devices drop it from each chat's key group on their next sync.
    for (chat_id,) in chats {
        crate::chats::notify_chat(&state, &chat_id).await?;
    }
    let everyone: Vec<(i64,)> = sqlx::query_as("SELECT id FROM users")
        .fetch_all(&state.db)
        .await?;
    state.hub.notify(
        everyone.into_iter().map(|(id,)| id).collect::<Vec<_>>(),
        crate::hub::Notice::Users,
    );
    Ok(StatusCode::NO_CONTENT)
}
