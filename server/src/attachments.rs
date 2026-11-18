//! Photos and files: a short-term relay for end-to-end encrypted blobs.
//!
//! Harmony does not host files. The sender's device encrypts a file with a
//! fresh key, uploads the ciphertext here, and sends the key inside an
//! ordinary encrypted chat message. Each receiving device downloads the
//! ciphertext, keeps its own decrypted copy, and reports that it has it. Once
//! every device that should get the file has it, the server deletes its copy.
//! Anything not picked up is deleted after a few days (seven by default).
//!
//! The server never sees a file's name, type or contents: only which chat it
//! belongs to, who sent it, when, and how large the ciphertext is.

use std::{
    path::{Path as FsPath, PathBuf},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::{fs, io::AsyncWriteExt};

use crate::{
    AppState,
    auth::{now, random_token},
    chats::require_member,
    devices::DeviceAuth,
    error::{ApiError, ApiResult},
};

/// An upload with nobody left to deliver it to (for example in a chat with
/// only its sender) is kept this long, then deleted.
const UNCLAIMED_GRACE_SECS: i64 = 60 * 60;
const SWEEP_EVERY: Duration = Duration::from_secs(10 * 60);

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/chats/{id}/attachments",
            // The size is checked while the upload streams in, against the
            // configured limit, so axum's own (smaller) limit is lifted here.
            post(upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/attachments/{id}", get(download).delete(cancel))
        .route("/attachments/{id}/received", post(received))
}

fn dir(state: &AppState) -> PathBuf {
    state.config.data_dir.join("attachments")
}

/// Ids are 64 hex characters we made ourselves; anything else is not ours
/// and must never reach the file system.
fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn file_path(state: &AppState, id: &str) -> ApiResult<PathBuf> {
    if !valid_id(id) {
        return Err(ApiError::NotFound);
    }
    Ok(dir(state).join(id))
}

async fn upload(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<Value>)> {
    require_member(&state.db, &chat_id, auth.user.id).await?;
    let limits = &state.config.attachments;
    let too_large = ApiError::TooLarge("that file is too large to send");

    // Refuse early when the size is announced up front.
    let announced = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if announced.is_some_and(|n| n > limits.max_file_bytes) {
        return Err(too_large);
    }
    let (held, held_by_user) = held_bytes(&state, auth.user.id).await?;
    if held + announced.unwrap_or(0) > limits.max_total_bytes {
        return Err(server_full());
    }
    if held_by_user + announced.unwrap_or(0) > limits.max_user_bytes {
        return Err(you_full());
    }

    fs::create_dir_all(dir(&state)).await?;
    let id = random_token()?;
    let final_path = file_path(&state, &id)?;
    let part_path = final_path.with_extension("part");

    let size = match receive(body, &part_path, limits.max_file_bytes).await {
        Ok(size) => size,
        Err(err) => {
            let _ = fs::remove_file(&part_path).await;
            return Err(err.unwrap_or(too_large));
        }
    };
    if size == 0 {
        let _ = fs::remove_file(&part_path).await;
        return Err(ApiError::BadRequest("the file is empty"));
    }

    let created = now();
    let expires_at = created + limits.keep_secs;
    let stored = async {
        let mut tx = state.db.begin().await?;
        // Checked again now that the real size is known, inside the same
        // transaction as the insert so two uploads cannot both squeeze in.
        let (held, held_by_user): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(size), 0), COALESCE(SUM(CASE WHEN uploader_user = ? THEN size END), 0)
             FROM attachments",
        )
        .bind(auth.user.id)
        .fetch_one(&mut *tx)
        .await?;
        if held as u64 + size > limits.max_total_bytes {
            return Err(server_full());
        }
        if held_by_user as u64 + size > limits.max_user_bytes {
            return Err(you_full());
        }
        sqlx::query(
            "INSERT INTO attachments (id, chat_id, uploader_user, uploader_device, size, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&chat_id)
        .bind(auth.user.id)
        .bind(auth.device_id)
        .bind(size as i64)
        .bind(created)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
        // Every current device of every member, other than the sender's own.
        sqlx::query(
            "INSERT INTO attachment_pending (attachment_id, device_id)
             SELECT ?, d.id FROM devices d JOIN chat_members m ON m.user_id = d.user_id
             WHERE m.chat_id = ? AND d.revoked_at IS NULL AND d.id != ?",
        )
        .bind(&id)
        .bind(&chat_id)
        .bind(auth.device_id)
        .execute(&mut *tx)
        .await?;
        fs::rename(&part_path, &final_path).await?;
        tx.commit().await?;
        Ok::<_, ApiError>(())
    }
    .await;
    if let Err(err) = stored {
        let _ = fs::remove_file(&part_path).await;
        let _ = fs::remove_file(&final_path).await;
        return Err(err);
    }

    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": id, "size": size, "expires_at": expires_at })),
    ))
}

/// Writes the upload to `path`, stopping as soon as it passes `max` bytes.
/// `Err(None)` means it was too large.
async fn receive(body: Body, path: &FsPath, max: u64) -> Result<u64, Option<ApiError>> {
    let mut file = fs::File::create(path).await.map_err(|e| Some(e.into()))?;
    let mut stream = body.into_data_stream();
    let mut size: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| Some(ApiError::BadRequest("the upload was interrupted")))?;
        size += chunk.len() as u64;
        if size > max {
            return Err(None);
        }
        file.write_all(&chunk).await.map_err(|e| Some(e.into()))?;
    }
    file.flush().await.map_err(|e| Some(e.into()))?;
    file.sync_all().await.map_err(|e| Some(e.into()))?;
    Ok(size)
}

fn server_full() -> ApiError {
    ApiError::StorageFull(
        "the server is holding too many files right now; try again once friends have picked theirs up",
    )
}

fn you_full() -> ApiError {
    ApiError::StorageFull(
        "you have too many files waiting to be picked up; try again once friends have received them",
    )
}

async fn held_bytes(state: &AppState, user_id: i64) -> ApiResult<(u64, u64)> {
    let (all, mine): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(size), 0), COALESCE(SUM(CASE WHEN uploader_user = ? THEN size END), 0)
         FROM attachments",
    )
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;
    Ok((all as u64, mine as u64))
}

/// The encrypted bytes, for any device of a chat member. Only devices that
/// can read the chat's messages have the key to open them.
async fn download(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let path = file_path(&state, &id)?;
    let row: Option<(String,)> =
        sqlx::query_as("SELECT chat_id FROM attachments WHERE id = ? AND expires_at > ?")
            .bind(&id)
            .bind(now())
            .fetch_optional(&state.db)
            .await?;
    let (chat_id,) = row.ok_or(ApiError::NotFound)?;
    require_member(&state.db, &chat_id, auth.user.id).await?;
    let bytes = match fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(ApiError::NotFound),
        Err(err) => return Err(err.into()),
    };
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        Bytes::from(bytes),
    )
        .into_response())
}

/// A device has its own copy now. When the last device that was waiting for
/// the file says so, the server's copy is deleted.
async fn received(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    file_path(&state, &id)?;
    sqlx::query("DELETE FROM attachment_pending WHERE attachment_id = ? AND device_id = ?")
        .bind(&id)
        .bind(auth.device_id)
        .execute(&state.db)
        .await?;
    let gone: Vec<(String,)> = sqlx::query_as(
        "DELETE FROM attachments WHERE id = ?
           AND NOT EXISTS (SELECT 1 FROM attachment_pending WHERE attachment_id = ?)
         RETURNING id",
    )
    .bind(&id)
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    remove_files(&state, gone).await;
    Ok(StatusCode::NO_CONTENT)
}

/// The sender takes an upload back, for example when its message could not
/// be sent.
async fn cancel(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    file_path(&state, &id)?;
    let gone: Vec<(String,)> =
        sqlx::query_as("DELETE FROM attachments WHERE id = ? AND uploader_user = ? RETURNING id")
            .bind(&id)
            .bind(auth.user.id)
            .fetch_all(&state.db)
            .await?;
    if gone.is_empty() {
        return Err(ApiError::NotFound);
    }
    remove_files(&state, gone).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_files(state: &AppState, ids: Vec<(String,)>) {
    for (id,) in ids {
        if let Ok(path) = file_path(state, &id)
            && let Err(err) = fs::remove_file(&path).await
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %err, "could not delete a delivered file");
        }
    }
}

/// Deletes what no longer needs holding: files past their time, files that
/// nobody is waiting for any more (everyone who was is gone from the chat or
/// signed that device out), and stray files left by an interrupted upload.
pub async fn sweep(state: &AppState) -> anyhow::Result<()> {
    let at = now();
    let left: Vec<(String,)> = sqlx::query_as(
        "DELETE FROM attachment_pending WHERE EXISTS (
            SELECT 1 FROM attachments a JOIN devices d ON d.id = attachment_pending.device_id
            WHERE a.id = attachment_pending.attachment_id
              AND (d.revoked_at IS NOT NULL
                   OR NOT EXISTS (SELECT 1 FROM chat_members m WHERE m.chat_id = a.chat_id AND m.user_id = d.user_id))
         )
         RETURNING attachment_id",
    )
    .fetch_all(&state.db)
    .await?;
    // Files whose last waiting device just dropped out have nobody to go to.
    for (id,) in left {
        let gone: Vec<(String,)> = sqlx::query_as(
            "DELETE FROM attachments WHERE id = ?1
               AND NOT EXISTS (SELECT 1 FROM attachment_pending WHERE attachment_id = ?1)
             RETURNING id",
        )
        .bind(&id)
        .fetch_all(&state.db)
        .await?;
        remove_files(state, gone).await;
    }
    let gone: Vec<(String,)> = sqlx::query_as(
        "DELETE FROM attachments
         WHERE expires_at <= ?1
            OR (created_at <= ?2 AND NOT EXISTS (SELECT 1 FROM attachment_pending p WHERE p.attachment_id = attachments.id))
         RETURNING id",
    )
    .bind(at)
    .bind(at - UNCLAIMED_GRACE_SECS)
    .fetch_all(&state.db)
    .await?;
    remove_files(state, gone).await;

    // Files on disk with no record: a chat was deleted, or an upload was cut off.
    let mut entries = match fs::read_dir(dir(state)).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let old = entry
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() as i64 > UNCLAIMED_GRACE_SECS);
        let known = valid_id(&name)
            && sqlx::query_as::<_, (i64,)>("SELECT 1 FROM attachments WHERE id = ?")
                .bind(&name)
                .fetch_optional(&state.db)
                .await?
                .is_some();
        if !known && old {
            let _ = fs::remove_file(entry.path()).await;
        }
    }
    Ok(())
}

/// Runs [`sweep`] now and then for as long as the server does.
pub async fn sweep_forever(state: AppState) {
    let mut tick = tokio::time::interval(SWEEP_EVERY);
    loop {
        tick.tick().await;
        if let Err(err) = sweep(&state).await {
            tracing::warn!(error = ?err, "could not clean up delivered files");
        }
    }
}
