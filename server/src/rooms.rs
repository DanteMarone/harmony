//! Rooms: named chats that everyone on the server can see and join, each
//! with its own voice channel that people drop in and out of.
//!
//! A room is a group chat with the `room` flag set, so everything about
//! messages and keys works as it does for groups and stays end-to-end
//! encrypted. What differs is who may become a member: anyone with an account
//! can join a room themselves, rather than being added by someone in it.
//! Once they are a member, the devices of the people already in the room
//! hand their devices the room's keys, exactly as when someone is added to
//! a group. So a newcomer can read the room once any member's device has
//! been online, and can never read what was said before they joined.
//!
//! A room outlives its members: when the last one leaves, its history and
//! keys are dropped and the next person to join starts them afresh.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{
    AppState,
    auth::{User, now, random_token},
    chats::{MAX_GROUP_MEMBERS, chat_info, clean_name, member_ids},
    devices::DeviceAuth,
    error::{ApiError, ApiResult},
    hub::Notice,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/rooms", get(list_rooms).post(create_room))
        .route("/rooms/{id}", delete(delete_room))
        .route("/rooms/{id}/name", put(rename_room))
        .route("/rooms/{id}/join", post(join_room))
}

pub async fn is_room(db: &SqlitePool, chat_id: &str) -> ApiResult<bool> {
    let row: Option<(bool,)> = sqlx::query_as("SELECT room = 1 FROM chats WHERE id = ?")
        .bind(chat_id)
        .fetch_optional(db)
        .await?;
    Ok(row.is_some_and(|(room,)| room))
}

async fn require_room(db: &SqlitePool, chat_id: &str) -> ApiResult<Option<i64>> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT created_by FROM chats WHERE id = ? AND room = 1")
            .bind(chat_id)
            .fetch_optional(db)
            .await?;
    row.map(|(created_by,)| created_by).ok_or(ApiError::NotFound)
}

/// Tells every account that the room list changed.
pub async fn notify_everyone(state: &AppState) -> ApiResult<()> {
    let users: Vec<(i64,)> = sqlx::query_as("SELECT id FROM users")
        .fetch_all(&state.db)
        .await?;
    state.hub.notify(
        users.into_iter().map(|(id,)| id).collect::<Vec<_>>(),
        Notice::Rooms,
    );
    Ok(())
}

/// Someone joined or left a call. If the call is a room's voice channel,
/// everyone sees who is in it, members of the room or not.
pub async fn notify_if_room(state: &AppState, chat_id: &str) -> ApiResult<()> {
    if is_room(&state.db, chat_id).await? {
        notify_everyone(state).await?;
    }
    Ok(())
}

/// The last member left: drop the history and keys nobody can read any more,
/// and reset the room so the next person in can start its keys afresh.
pub async fn empty_out(db: &SqlitePool, chat_id: &str) -> ApiResult<()> {
    let mut tx = db.begin().await?;
    for statement in [
        "DELETE FROM events WHERE chat_id = ?",
        "DELETE FROM chat_devices WHERE chat_id = ?",
        "DELETE FROM welcomes WHERE chat_id = ?",
        "DELETE FROM backup_messages WHERE chat_id = ?",
    ] {
        sqlx::query(statement).bind(chat_id).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE chats SET epoch = 0 WHERE id = ?")
        .bind(chat_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[derive(Serialize)]
struct RoomInfo {
    id: String,
    name: String,
    created_by: Option<i64>,
    created_at: i64,
    /// Everyone in the room.
    member_ids: Vec<i64>,
    /// Whether the caller is one of them.
    member: bool,
    /// Who is in the room's voice channel right now.
    voice: Vec<i64>,
}

/// Every room on the server, with who is in it and in its voice channel.
async fn list_rooms(State(state): State<AppState>, user: User) -> ApiResult<Json<Vec<RoomInfo>>> {
    let rows: Vec<(String, Option<String>, Option<i64>, i64)> = sqlx::query_as(
        "SELECT id, name, created_by, created_at FROM chats WHERE room = 1
         ORDER BY name COLLATE NOCASE, created_at",
    )
    .fetch_all(&state.db)
    .await?;
    let mut rooms = Vec::with_capacity(rows.len());
    for (id, name, created_by, created_at) in rows {
        let member_ids = member_ids(&state.db, &id).await?;
        rooms.push(RoomInfo {
            member: member_ids.contains(&user.id),
            voice: state.calls.users_in(&id),
            member_ids,
            id,
            name: name.unwrap_or_default(),
            created_by,
            created_at,
        });
    }
    Ok(Json(rooms))
}

#[derive(Deserialize)]
struct NameBody {
    name: Option<String>,
}

/// Makes a room with its creator as the first member. The creating device
/// starts the room's keys, as it would for a new group chat.
async fn create_room(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Json(body): Json<NameBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = clean_name(body.name.as_deref())?;
    let chat_id = random_token()?[..32].to_owned();
    let created = now();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO chats (id, kind, name, room, created_by, created_at) VALUES (?, 'group', ?, 1, ?, ?)",
    )
    .bind(&chat_id)
    .bind(&name)
    .bind(auth.user.id)
    .bind(created)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO chat_members (chat_id, user_id, joined_at) VALUES (?, ?, ?)")
        .bind(&chat_id)
        .bind(auth.user.id)
        .bind(created)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO chat_devices (chat_id, device_id, joined_seq) VALUES (?, ?, 0)")
        .bind(&chat_id)
        .bind(auth.device_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    state.hub.notify(vec![auth.user.id], Notice::Chats);
    notify_everyone(&state).await?;
    let chat = chat_info(&state.db, &chat_id, auth.device_id).await?;
    Ok((StatusCode::CREATED, Json(json!({ "chat": chat }))))
}

/// Joins a room. `start` is true when the room was empty: the calling device
/// then starts the room's keys. Otherwise the members' devices add this
/// person's devices to the room's keys the next time one of them is online.
async fn join_room(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
) -> ApiResult<Json<Value>> {
    require_room(&state.db, &chat_id).await?;
    let mut tx = state.db.begin().await?;
    // Writing first takes the database's write lock, so two people joining an
    // empty room at once cannot both think they are first.
    let inserted =
        sqlx::query("INSERT OR IGNORE INTO chat_members (chat_id, user_id, joined_at) VALUES (?, ?, ?)")
            .bind(&chat_id)
            .bind(auth.user.id)
            .bind(now())
            .execute(&mut *tx)
            .await?;
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM chat_members WHERE chat_id = ?")
        .bind(&chat_id)
        .fetch_one(&mut *tx)
        .await?;
    if count as usize > MAX_GROUP_MEMBERS {
        return Err(ApiError::BadRequest("this room is full"));
    }
    let joined = inserted.rows_affected() == 1;
    let start = joined && count == 1;
    if start {
        sqlx::query("INSERT OR REPLACE INTO chat_devices (chat_id, device_id, joined_seq) VALUES (?, ?, 0)")
            .bind(&chat_id)
            .bind(auth.device_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    if joined {
        let members = member_ids(&state.db, &chat_id).await?;
        state.hub.notify(members.clone(), Notice::Chats);
        state.hub.notify(
            members,
            Notice::Chat {
                chat_id: chat_id.clone(),
            },
        );
        notify_everyone(&state).await?;
    }
    let chat = chat_info(&state.db, &chat_id, auth.device_id).await?;
    Ok(Json(json!({ "chat": chat, "start": start })))
}

/// Whoever made the room, or an admin, can rename or delete it.
async fn require_owner(db: &SqlitePool, chat_id: &str, user: &User) -> ApiResult<()> {
    let created_by = require_room(db, chat_id).await?;
    if created_by != Some(user.id) && !user.is_admin {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

async fn rename_room(
    State(state): State<AppState>,
    user: User,
    Path(chat_id): Path<String>,
    Json(body): Json<NameBody>,
) -> ApiResult<StatusCode> {
    require_owner(&state.db, &chat_id, &user).await?;
    let name = clean_name(body.name.as_deref())?;
    sqlx::query("UPDATE chats SET name = ? WHERE id = ?")
        .bind(&name)
        .bind(&chat_id)
        .execute(&state.db)
        .await?;
    state
        .hub
        .notify(member_ids(&state.db, &chat_id).await?, Notice::Chats);
    notify_everyone(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Deletes the room and everything in it, for everyone.
async fn delete_room(
    State(state): State<AppState>,
    user: User,
    Path(chat_id): Path<String>,
) -> ApiResult<StatusCode> {
    require_owner(&state.db, &chat_id, &user).await?;
    let members = member_ids(&state.db, &chat_id).await?;
    sqlx::query("DELETE FROM chats WHERE id = ?")
        .bind(&chat_id)
        .execute(&state.db)
        .await?;
    state.calls.forget_chat(&chat_id);
    state.hub.notify(members, Notice::Chats);
    notify_everyone(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}
