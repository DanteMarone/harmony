//! Chats: membership, and the ordered stream of encrypted events.
//!
//! The server is a relay with access control. It decides who may read and
//! post in a chat and keeps events in order; it cannot read any of them.
//!
//! Ordering rule: every post states the epoch it was made in, and is refused
//! (409) unless that is the chat's current epoch. A device that is refused
//! fetches what it missed and tries again. This keeps every device's view of
//! the chat's keys identical.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{
    AppState,
    auth::{User, now, random_token},
    devices::{DeviceAuth, decode, encode, identity},
    error::{ApiError, ApiResult},
    hub::Notice,
};

const MAX_EVENT_BYTES: usize = 256 * 1024;
const MAX_WELCOME_BYTES: usize = 1024 * 1024;
const MAX_GROUP_MEMBERS: usize = 100;
const EVENTS_PAGE: i64 = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/chats", get(list_chats).post(create_chat))
        .route("/chats/{id}/members", post(add_members))
        .route("/chats/{id}/members/{user_id}", delete(remove_member))
        .route("/chats/{id}/pending", get(pending))
        .route("/chats/{id}/rejoin", post(request_rejoin))
        .route("/chats/{id}/claim", post(claim_key_packages))
        .route("/chats/{id}/commit", post(post_commit))
        .route("/chats/{id}/messages", post(post_message))
        .route("/chats/{id}/events", get(list_events))
        .route("/welcomes", get(list_welcomes))
        .route("/welcomes/{id}", delete(ack_welcome))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn member_ids(db: &SqlitePool, chat_id: &str) -> ApiResult<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as("SELECT user_id FROM chat_members WHERE chat_id = ?")
        .bind(chat_id)
        .fetch_all(db)
        .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Tells every member's connected devices that something in the chat changed.
pub async fn notify_chat(state: &AppState, chat_id: &str) -> ApiResult<()> {
    let members = member_ids(&state.db, chat_id).await?;
    state.hub.notify(
        members,
        Notice::Chat {
            chat_id: chat_id.to_owned(),
        },
    );
    Ok(())
}

async fn require_member(db: &SqlitePool, chat_id: &str, user_id: i64) -> ApiResult<()> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM chat_members WHERE chat_id = ? AND user_id = ?")
        .bind(chat_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    // 404, not 403: don't reveal that a chat exists to people outside it.
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// The caller must be a member, and their device must hold the chat's keys.
/// Returns the event after which the device can read.
async fn require_device_in_chat(db: &SqlitePool, chat_id: &str, auth: &DeviceAuth) -> ApiResult<i64> {
    require_member(db, chat_id, auth.user.id).await?;
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT joined_seq FROM chat_devices WHERE chat_id = ? AND device_id = ? AND needs_rejoin = 0",
    )
    .bind(chat_id)
    .bind(auth.device_id)
    .fetch_optional(db)
    .await?;
    row.map(|(seq,)| seq)
        .ok_or(ApiError::Conflict("this device has not joined the chat yet"))
}

#[derive(Serialize)]
struct MemberInfo {
    user_id: i64,
    username: String,
    display_name: String,
}

#[derive(Serialize)]
struct ChatInfo {
    id: String,
    kind: String,
    name: Option<String>,
    epoch: i64,
    created_by: Option<i64>,
    created_at: i64,
    last_seq: i64,
    members: Vec<MemberInfo>,
    /// Whether the calling device holds the chat's keys yet.
    joined: bool,
}

async fn chat_info(db: &SqlitePool, chat_id: &str, device_id: i64) -> ApiResult<ChatInfo> {
    let (id, kind, name, epoch, created_by, created_at): (
        String,
        String,
        Option<String>,
        i64,
        Option<i64>,
        i64,
    ) = sqlx::query_as("SELECT id, kind, name, epoch, created_by, created_at FROM chats WHERE id = ?")
        .bind(chat_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)?;
    let members: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT u.id, u.username, u.display_name FROM chat_members m JOIN users u ON u.id = m.user_id
         WHERE m.chat_id = ? ORDER BY u.display_name COLLATE NOCASE",
    )
    .bind(chat_id)
    .fetch_all(db)
    .await?;
    let (last_seq,): (i64,) = sqlx::query_as("SELECT COALESCE(MAX(seq), 0) FROM events WHERE chat_id = ?")
        .bind(chat_id)
        .fetch_one(db)
        .await?;
    let joined: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM chat_devices WHERE chat_id = ? AND device_id = ?")
            .bind(chat_id)
            .bind(device_id)
            .fetch_optional(db)
            .await?;
    Ok(ChatInfo {
        id,
        kind,
        name,
        epoch,
        created_by,
        created_at,
        last_seq,
        members: members
            .into_iter()
            .map(|(user_id, username, display_name)| MemberInfo {
                user_id,
                username,
                display_name,
            })
            .collect(),
        joined: joined.is_some(),
    })
}

fn clean_name(name: Option<&str>) -> ApiResult<String> {
    let name = name.map(str::trim).unwrap_or("");
    if name.is_empty() || name.chars().count() > 64 {
        return Err(ApiError::BadRequest("group name must be 1-64 characters"));
    }
    Ok(name.to_owned())
}

// ---------------------------------------------------------------------------
// Chats and membership
// ---------------------------------------------------------------------------

async fn list_chats(State(state): State<AppState>, auth: DeviceAuth) -> ApiResult<Json<Vec<ChatInfo>>> {
    let ids: Vec<(String,)> = sqlx::query_as("SELECT chat_id FROM chat_members WHERE user_id = ?")
        .bind(auth.user.id)
        .fetch_all(&state.db)
        .await?;
    let mut chats = Vec::with_capacity(ids.len());
    for (id,) in ids {
        chats.push(chat_info(&state.db, &id, auth.device_id).await?);
    }
    Ok(Json(chats))
}

#[derive(Deserialize)]
struct CreateChat {
    kind: String,
    name: Option<String>,
    /// The other people. The creator is always included.
    member_ids: Vec<i64>,
}

async fn create_chat(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Json(body): Json<CreateChat>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let mut others: Vec<i64> = body
        .member_ids
        .into_iter()
        .filter(|id| *id != auth.user.id)
        .collect();
    others.sort_unstable();
    others.dedup();

    let (name, dm_key) = match body.kind.as_str() {
        "dm" => {
            let [other] = others[..] else {
                return Err(ApiError::BadRequest(
                    "a direct message needs exactly one other person",
                ));
            };
            let (low, high) = (auth.user.id.min(other), auth.user.id.max(other));
            (None, Some(format!("{low}:{high}")))
        }
        "group" => {
            if others.len() + 1 > MAX_GROUP_MEMBERS {
                return Err(ApiError::BadRequest("too many members"));
            }
            (Some(clean_name(body.name.as_deref())?), None)
        }
        _ => return Err(ApiError::BadRequest("kind must be dm or group")),
    };

    let mut tx = state.db.begin().await?;
    for other in &others {
        let exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = ?")
            .bind(other)
            .fetch_optional(&mut *tx)
            .await?;
        if exists.is_none() {
            return Err(ApiError::BadRequest("unknown user"));
        }
    }

    let chat_id = random_token()?[..32].to_owned();
    let created = now();
    let inserted = sqlx::query(
        "INSERT INTO chats (id, kind, name, dm_key, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(dm_key) DO NOTHING",
    )
    .bind(&chat_id)
    .bind(&body.kind)
    .bind(&name)
    .bind(&dm_key)
    .bind(auth.user.id)
    .bind(created)
    .execute(&mut *tx)
    .await?;

    if inserted.rows_affected() == 0 {
        // This pair already has a DM: hand back the existing one.
        let (existing,): (String,) = sqlx::query_as("SELECT id FROM chats WHERE dm_key = ?")
            .bind(&dm_key)
            .fetch_one(&mut *tx)
            .await?;
        drop(tx);
        let info = chat_info(&state.db, &existing, auth.device_id).await?;
        return Ok((StatusCode::OK, Json(json!({ "created": false, "chat": info }))));
    }

    for user_id in std::iter::once(auth.user.id).chain(others.iter().copied()) {
        sqlx::query("INSERT INTO chat_members (chat_id, user_id, joined_at) VALUES (?, ?, ?)")
            .bind(&chat_id)
            .bind(user_id)
            .bind(created)
            .execute(&mut *tx)
            .await?;
    }
    // The creating device starts the key group, so it holds the keys from the start.
    sqlx::query("INSERT INTO chat_devices (chat_id, device_id, joined_seq) VALUES (?, ?, 0)")
        .bind(&chat_id)
        .bind(auth.device_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let mut everyone = others;
    everyone.push(auth.user.id);
    state.hub.notify(everyone, Notice::Chats);
    let info = chat_info(&state.db, &chat_id, auth.device_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "created": true, "chat": info })),
    ))
}

async fn chat_kind_and_creator(db: &SqlitePool, chat_id: &str) -> ApiResult<(String, Option<i64>)> {
    sqlx::query_as("SELECT kind, created_by FROM chats WHERE id = ?")
        .bind(chat_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

#[derive(Deserialize)]
struct AddMembers {
    user_ids: Vec<i64>,
}

/// Any member of a group chat can add people.
async fn add_members(
    State(state): State<AppState>,
    user: User,
    Path(chat_id): Path<String>,
    Json(body): Json<AddMembers>,
) -> ApiResult<StatusCode> {
    require_member(&state.db, &chat_id, user.id).await?;
    let (kind, _) = chat_kind_and_creator(&state.db, &chat_id).await?;
    if kind != "group" {
        return Err(ApiError::BadRequest("people can only be added to group chats"));
    }
    let current = member_ids(&state.db, &chat_id).await?;
    if current.len() + body.user_ids.len() > MAX_GROUP_MEMBERS {
        return Err(ApiError::BadRequest("too many members"));
    }
    let mut tx = state.db.begin().await?;
    for user_id in &body.user_ids {
        let exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
        if exists.is_none() {
            return Err(ApiError::BadRequest("unknown user"));
        }
        sqlx::query("INSERT OR IGNORE INTO chat_members (chat_id, user_id, joined_at) VALUES (?, ?, ?)")
            .bind(&chat_id)
            .bind(user_id)
            .bind(now())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let mut everyone = current;
    everyone.extend(&body.user_ids);
    state.hub.notify(everyone.clone(), Notice::Chats);
    state.hub.notify(everyone, Notice::Chat { chat_id });
    Ok(StatusCode::NO_CONTENT)
}

/// Members can leave a group; the group's creator can remove others.
async fn remove_member(
    State(state): State<AppState>,
    user: User,
    Path((chat_id, target)): Path<(String, i64)>,
) -> ApiResult<StatusCode> {
    require_member(&state.db, &chat_id, user.id).await?;
    let (kind, created_by) = chat_kind_and_creator(&state.db, &chat_id).await?;
    if kind != "group" {
        return Err(ApiError::BadRequest("direct messages cannot be left"));
    }
    if target != user.id && created_by != Some(user.id) {
        return Err(ApiError::Forbidden);
    }
    let everyone = member_ids(&state.db, &chat_id).await?;
    let removed = sqlx::query("DELETE FROM chat_members WHERE chat_id = ? AND user_id = ?")
        .bind(&chat_id)
        .bind(target)
        .execute(&state.db)
        .await?;
    if removed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    // Invitations they had not picked up yet are withdrawn.
    sqlx::query(
        "DELETE FROM welcomes WHERE chat_id = ? AND device_id IN (SELECT id FROM devices WHERE user_id = ?)",
    )
    .bind(&chat_id)
    .bind(target)
    .execute(&state.db)
    .await?;
    if everyone.len() == 1 {
        // The last person left: nothing can ever read this chat again.
        sqlx::query("DELETE FROM chats WHERE id = ?")
            .bind(&chat_id)
            .execute(&state.db)
            .await?;
    }
    state.hub.notify(everyone.clone(), Notice::Chats);
    state.hub.notify(everyone, Notice::Chat { chat_id });
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Keeping the key group in step with the member list
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct PendingDevice {
    user_id: i64,
    device_id: i64,
    identity: String,
}

/// Devices that should hold the chat's keys but do not yet (`add`), and
/// devices that hold them but should not any more (`remove`). Any device in
/// the chat can act on this by posting a commit.
async fn pending(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
) -> ApiResult<Json<Value>> {
    require_device_in_chat(&state.db, &chat_id, &auth).await?;
    let add: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT d.user_id, d.id FROM devices d
         JOIN chat_members m ON m.user_id = d.user_id AND m.chat_id = ?1
         WHERE d.revoked_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM chat_devices c WHERE c.chat_id = ?1 AND c.device_id = d.id)
           AND EXISTS (SELECT 1 FROM key_packages k WHERE k.device_id = d.id)
         ORDER BY d.id",
    )
    .bind(&chat_id)
    .fetch_all(&state.db)
    .await?;
    let remove: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT d.user_id, d.id FROM chat_devices c JOIN devices d ON d.id = c.device_id
         WHERE c.chat_id = ?1
           AND (d.revoked_at IS NOT NULL OR c.needs_rejoin = 1
                OR NOT EXISTS (SELECT 1 FROM chat_members m WHERE m.chat_id = ?1 AND m.user_id = d.user_id))
         ORDER BY d.id",
    )
    .bind(&chat_id)
    .fetch_all(&state.db)
    .await?;
    let describe = |rows: Vec<(i64, i64)>| -> Vec<PendingDevice> {
        rows.into_iter()
            .map(|(user_id, device_id)| PendingDevice {
                user_id,
                device_id,
                identity: identity(user_id, device_id),
            })
            .collect()
    };
    Ok(Json(json!({ "add": describe(add), "remove": describe(remove) })))
}

/// A device says it no longer has working keys for this chat (for example
/// its saved state was lost). It stops receiving events it could not read,
/// and another member's device will remove it and add it back fresh.
async fn request_rejoin(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
) -> ApiResult<StatusCode> {
    require_member(&state.db, &chat_id, auth.user.id).await?;
    sqlx::query("UPDATE chat_devices SET needs_rejoin = 1 WHERE chat_id = ? AND device_id = ?")
        .bind(&chat_id)
        .bind(auth.device_id)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM welcomes WHERE chat_id = ? AND device_id = ?")
        .bind(&chat_id)
        .bind(auth.device_id)
        .execute(&state.db)
        .await?;
    notify_chat(&state, &chat_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Claim {
    device_ids: Vec<i64>,
}

/// Hands out one key package for each requested device, so the caller can add
/// them to the chat. One-time packages are deleted as they are handed out.
async fn claim_key_packages(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
    Json(body): Json<Claim>,
) -> ApiResult<Json<Value>> {
    require_device_in_chat(&state.db, &chat_id, &auth).await?;
    let mut claimed = Vec::new();
    let mut tx = state.db.begin().await?;
    for device_id in body.device_ids.iter().take(MAX_GROUP_MEMBERS * 4) {
        // Only devices of current members may be claimed for this chat.
        let owner: Option<(i64,)> = sqlx::query_as(
            "SELECT d.user_id FROM devices d JOIN chat_members m ON m.user_id = d.user_id AND m.chat_id = ?
             WHERE d.id = ? AND d.revoked_at IS NULL",
        )
        .bind(&chat_id)
        .bind(device_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((user_id,)) = owner else { continue };
        let package: Option<(i64, bool, Vec<u8>)> = sqlx::query_as(
            "SELECT id, last_resort, data FROM key_packages WHERE device_id = ? ORDER BY last_resort, id LIMIT 1",
        )
        .bind(device_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((package_id, last_resort, data)) = package else {
            continue;
        };
        if !last_resort {
            sqlx::query("DELETE FROM key_packages WHERE id = ?")
                .bind(package_id)
                .execute(&mut *tx)
                .await?;
        }
        claimed.push(json!({
            "user_id": user_id,
            "device_id": device_id,
            "identity": identity(user_id, *device_id),
            "key_package": encode(&data),
        }));
    }
    tx.commit().await?;
    Ok(Json(json!({ "key_packages": claimed })))
}

#[derive(Deserialize)]
struct CommitBody {
    epoch: i64,
    commit: String,
    welcome: Option<String>,
    #[serde(default)]
    added: Vec<i64>,
    #[serde(default)]
    removed: Vec<i64>,
}

/// A key change: devices added to or removed from the chat's key group.
async fn post_commit(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
    Json(body): Json<CommitBody>,
) -> ApiResult<Json<Value>> {
    require_device_in_chat(&state.db, &chat_id, &auth).await?;
    let commit = decode("invalid commit", &body.commit, MAX_EVENT_BYTES)?;
    let welcome = body
        .welcome
        .as_deref()
        .map(|w| decode("invalid welcome", w, MAX_WELCOME_BYTES))
        .transpose()?;
    if welcome.is_some() != !body.added.is_empty() {
        return Err(ApiError::BadRequest(
            "a welcome is required exactly when devices are added",
        ));
    }
    if body.removed.contains(&auth.device_id) {
        return Err(ApiError::BadRequest("a device cannot remove itself"));
    }

    let mut tx = state.db.begin().await?;
    // First statement takes the write lock and checks the epoch in one step.
    let advanced = sqlx::query("UPDATE chats SET epoch = epoch + 1 WHERE id = ? AND epoch = ?")
        .bind(&chat_id)
        .bind(body.epoch)
        .execute(&mut *tx)
        .await?;
    if advanced.rows_affected() == 0 {
        return Err(ApiError::Conflict(
            "the chat changed; fetch new events and try again",
        ));
    }

    let created = now();
    let seq = sqlx::query(
        "INSERT INTO events (chat_id, kind, sender_user, sender_device, epoch, data, created_at)
         VALUES (?, 'commit', ?, ?, ?, ?, ?)",
    )
    .bind(&chat_id)
    .bind(auth.user.id)
    .bind(auth.device_id)
    .bind(body.epoch)
    .bind(&commit)
    .bind(created)
    .execute(&mut *tx)
    .await?
    .last_insert_rowid();

    for device_id in &body.added {
        // Only devices of current members can be given the keys.
        let allowed: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM devices d JOIN chat_members m ON m.user_id = d.user_id AND m.chat_id = ?
             WHERE d.id = ? AND d.revoked_at IS NULL",
        )
        .bind(&chat_id)
        .bind(device_id)
        .fetch_optional(&mut *tx)
        .await?;
        if allowed.is_none() {
            return Err(ApiError::BadRequest(
                "a device being added does not belong to a member",
            ));
        }
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO chat_devices (chat_id, device_id, joined_seq) VALUES (?, ?, ?)",
        )
        .bind(&chat_id)
        .bind(device_id)
        .bind(seq)
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() == 0 {
            return Err(ApiError::BadRequest(
                "a device being added is already in the chat",
            ));
        }
        sqlx::query(
            "INSERT INTO welcomes (device_id, chat_id, after_seq, data, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(device_id)
        .bind(&chat_id)
        .bind(seq)
        .bind(welcome.as_deref())
        .bind(created)
        .execute(&mut *tx)
        .await?;
    }
    for device_id in &body.removed {
        sqlx::query("DELETE FROM chat_devices WHERE chat_id = ? AND device_id = ?")
            .bind(&chat_id)
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM welcomes WHERE chat_id = ? AND device_id = ?")
            .bind(&chat_id)
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    notify_chat(&state, &chat_id).await?;
    if !body.added.is_empty() {
        let members = member_ids(&state.db, &chat_id).await?;
        state.hub.notify(members, Notice::Welcome);
    }
    Ok(Json(json!({ "seq": seq, "epoch": body.epoch + 1 })))
}

#[derive(Deserialize)]
struct MessageBody {
    epoch: i64,
    data: String,
    client_id: String,
}

async fn post_message(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
    Json(body): Json<MessageBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    require_device_in_chat(&state.db, &chat_id, &auth).await?;
    let data = decode("invalid message", &body.data, MAX_EVENT_BYTES)?;
    if body.client_id.is_empty() || body.client_id.len() > 64 {
        return Err(ApiError::BadRequest("invalid client id"));
    }
    // A retry of a message that already arrived: answer as if it were new.
    let existing: Option<(i64, i64)> = sqlx::query_as(
        "SELECT seq, created_at FROM events WHERE chat_id = ? AND sender_device = ? AND client_id = ?",
    )
    .bind(&chat_id)
    .bind(auth.device_id)
    .bind(&body.client_id)
    .fetch_optional(&state.db)
    .await?;
    if let Some((seq, created_at)) = existing {
        return Ok((
            StatusCode::OK,
            Json(json!({ "seq": seq, "created_at": created_at })),
        ));
    }
    let created = now();
    // One statement, so the epoch check and the insert cannot be separated.
    let inserted = sqlx::query(
        "INSERT INTO events (chat_id, kind, sender_user, sender_device, epoch, client_id, data, created_at)
         SELECT ?1, 'message', ?2, ?3, ?4, ?5, ?6, ?7
         WHERE (SELECT epoch FROM chats WHERE id = ?1) = ?4",
    )
    .bind(&chat_id)
    .bind(auth.user.id)
    .bind(auth.device_id)
    .bind(body.epoch)
    .bind(&body.client_id)
    .bind(&data)
    .bind(created)
    .execute(&state.db)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(ApiError::Conflict(
            "the chat changed; fetch new events and try again",
        ));
    }
    notify_chat(&state, &chat_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "seq": inserted.last_insert_rowid(), "created_at": created })),
    ))
}

#[derive(Deserialize)]
struct EventsQuery {
    #[serde(default)]
    after: i64,
}

#[derive(Serialize)]
struct EventOut {
    seq: i64,
    kind: String,
    sender_user: Option<i64>,
    sender_device: Option<i64>,
    epoch: i64,
    client_id: Option<String>,
    data: String,
    created_at: i64,
}

/// Events after `after`, oldest first, at most one page. A device is never
/// given events from before it joined: it could not decrypt them anyway.
async fn list_events(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
    Query(query): Query<EventsQuery>,
) -> ApiResult<Json<Value>> {
    let joined_seq = require_device_in_chat(&state.db, &chat_id, &auth).await?;
    type Row = (
        i64,
        String,
        Option<i64>,
        Option<i64>,
        i64,
        Option<String>,
        Vec<u8>,
        i64,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT seq, kind, sender_user, sender_device, epoch, client_id, data, created_at
         FROM events WHERE chat_id = ? AND seq > ? ORDER BY seq LIMIT ?",
    )
    .bind(&chat_id)
    .bind(query.after.max(joined_seq))
    .bind(EVENTS_PAGE)
    .fetch_all(&state.db)
    .await?;
    let more = rows.len() as i64 == EVENTS_PAGE;
    let events: Vec<EventOut> = rows
        .into_iter()
        .map(
            |(seq, kind, sender_user, sender_device, epoch, client_id, data, created_at)| EventOut {
                seq,
                kind,
                sender_user,
                sender_device,
                epoch,
                client_id,
                data: encode(&data),
                created_at,
            },
        )
        .collect();
    Ok(Json(json!({ "events": events, "more": more })))
}

// ---------------------------------------------------------------------------
// Invitations for this device
// ---------------------------------------------------------------------------

async fn list_welcomes(State(state): State<AppState>, auth: DeviceAuth) -> ApiResult<Json<Value>> {
    let rows: Vec<(i64, String, i64, Vec<u8>)> =
        sqlx::query_as("SELECT id, chat_id, after_seq, data FROM welcomes WHERE device_id = ? ORDER BY id")
            .bind(auth.device_id)
            .fetch_all(&state.db)
            .await?;
    let welcomes: Vec<Value> = rows
        .into_iter()
        .map(|(id, chat_id, after_seq, data)| {
            json!({ "id": id, "chat_id": chat_id, "after_seq": after_seq, "data": encode(&data) })
        })
        .collect();
    Ok(Json(json!({ "welcomes": welcomes })))
}

/// The device has joined the chat and saved its keys: the invitation can go.
async fn ack_welcome(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM welcomes WHERE id = ? AND device_id = ?")
        .bind(id)
        .bind(auth.device_id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
