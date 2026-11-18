//! Identity keys, device signatures and the encrypted history backup.
//!
//! The server only stores and hands out what devices give it. It cannot
//! make a valid device signature (that needs the person's recovery code)
//! and cannot read a backup (same). Devices check signatures themselves,
//! so nothing here is trusted by them.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, put},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    AppState,
    devices::{DeviceAuth, decode, encode},
    error::{ApiError, ApiResult},
    hub::Notice,
};

const IDENTITY_KEY_BYTES: usize = 32;
const SIGNATURE_BYTES: usize = 64;
/// A backed-up message is a chat message plus a little framing.
const MAX_BACKUP_BYTES: usize = 80 * 1024;
const MAX_BACKUP_BATCH: usize = 200;
const BACKUP_PAGE: i64 = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/identity", put(set_identity))
        .route("/devices/{id}/signature", put(sign_device))
        .route("/backup/{chat_id}", get(read_backup).post(write_backup))
}

fn exact(field: &'static str, value: &str, length: usize) -> ApiResult<Vec<u8>> {
    let bytes = decode(field, value, length)?;
    if bytes.len() != length {
        return Err(ApiError::BadRequest(field));
    }
    Ok(bytes)
}

/// Everyone's devices need to look again at who is verified.
async fn notify_everyone(state: &AppState) -> ApiResult<()> {
    let users: Vec<(i64,)> = sqlx::query_as("SELECT id FROM users")
        .fetch_all(&state.db)
        .await?;
    let ids: Vec<i64> = users.into_iter().map(|(id,)| id).collect();
    state.hub.notify(ids, Notice::Users);
    Ok(())
}

#[derive(Deserialize)]
struct IdentityBody {
    identity_key: String,
    /// The new identity's signature over the calling device.
    signature: String,
}

/// Sets the caller's identity key: the first time, or to start over after a
/// lost recovery code. Starting over un-signs their other devices and drops
/// the old backup, which the new code could not read anyway. Friends'
/// devices notice the change and ask them to verify again.
async fn set_identity(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Json(body): Json<IdentityBody>,
) -> ApiResult<StatusCode> {
    let key = exact("invalid identity key", &body.identity_key, IDENTITY_KEY_BYTES)?;
    let signature = exact("invalid signature", &body.signature, SIGNATURE_BYTES)?;

    let mut tx = state.db.begin().await?;
    let (current,): (Option<Vec<u8>>,) = sqlx::query_as("SELECT identity_key FROM users WHERE id = ?")
        .bind(auth.user.id)
        .fetch_one(&mut *tx)
        .await?;
    if current.as_deref() != Some(key.as_slice()) {
        sqlx::query("UPDATE users SET identity_key = ? WHERE id = ?")
            .bind(&key)
            .bind(auth.user.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE devices SET identity_signature = NULL WHERE user_id = ?")
            .bind(auth.user.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM backup_messages WHERE user_id = ?")
            .bind(auth.user.id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE devices SET identity_signature = ? WHERE id = ?")
        .bind(&signature)
        .bind(auth.device_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    notify_everyone(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SignatureBody {
    signature: String,
}

/// Stores the owner's signature for the calling device, made after they
/// typed their recovery code on it.
async fn sign_device(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(id): Path<i64>,
    Json(body): Json<SignatureBody>,
) -> ApiResult<StatusCode> {
    if id != auth.device_id {
        return Err(ApiError::Forbidden);
    }
    let signature = exact("invalid signature", &body.signature, SIGNATURE_BYTES)?;
    let updated = sqlx::query(
        "UPDATE devices SET identity_signature = ?1 WHERE id = ?2
         AND (SELECT identity_key FROM users WHERE id = ?3) IS NOT NULL",
    )
    .bind(&signature)
    .bind(id)
    .bind(auth.user.id)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::Conflict("set up a recovery code first"));
    }
    notify_everyone(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct BackupItem {
    seq: i64,
    data: String,
}

#[derive(Deserialize)]
struct BackupBody {
    items: Vec<BackupItem>,
}

/// Saves the caller's encrypted copies of messages in a chat they are in.
/// The first copy of each message wins, so several devices can back up the
/// same chat without overwriting each other.
async fn write_backup(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
    Json(body): Json<BackupBody>,
) -> ApiResult<StatusCode> {
    crate::chats::require_member(&state.db, &chat_id, auth.user.id).await?;
    if body.items.len() > MAX_BACKUP_BATCH {
        return Err(ApiError::BadRequest("too many backup items"));
    }
    let (last_seq,): (i64,) = sqlx::query_as("SELECT COALESCE(MAX(seq), 0) FROM events WHERE chat_id = ?")
        .bind(&chat_id)
        .fetch_one(&state.db)
        .await?;
    let mut tx = state.db.begin().await?;
    for item in &body.items {
        // Only real messages of this chat can have a copy, which also
        // bounds how much one person can store.
        if item.seq < 1 || item.seq > last_seq {
            return Err(ApiError::BadRequest("no such message"));
        }
        let data = decode("invalid backup item", &item.data, MAX_BACKUP_BYTES)?;
        sqlx::query(
            "INSERT OR IGNORE INTO backup_messages (user_id, chat_id, seq, data)
             SELECT ?1, ?2, ?3, ?4
             WHERE EXISTS (SELECT 1 FROM events WHERE seq = ?3 AND chat_id = ?2 AND kind = 'message')",
        )
        .bind(auth.user.id)
        .bind(&chat_id)
        .bind(item.seq)
        .bind(data)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct BackupQuery {
    #[serde(default)]
    after: i64,
}

/// Reads back the caller's own backup of a chat, oldest first.
async fn read_backup(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
    Query(query): Query<BackupQuery>,
) -> ApiResult<Json<Value>> {
    crate::chats::require_member(&state.db, &chat_id, auth.user.id).await?;
    let mut rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT seq, data FROM backup_messages
         WHERE user_id = ? AND chat_id = ? AND seq > ? ORDER BY seq LIMIT ?",
    )
    .bind(auth.user.id)
    .bind(&chat_id)
    .bind(query.after)
    .bind(BACKUP_PAGE + 1)
    .fetch_all(&state.db)
    .await?;
    let more = rows.len() as i64 > BACKUP_PAGE;
    rows.truncate(BACKUP_PAGE as usize);
    let items: Vec<Value> = rows
        .iter()
        .map(|(seq, data)| json!({ "seq": seq, "data": encode(data) }))
        .collect();
    Ok(Json(json!({ "items": items, "more": more })))
}
