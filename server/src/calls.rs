//! Voice calls.
//!
//! Audio travels through a LiveKit server (an SFU: each person sends their
//! audio once and LiveKit forwards it to everyone else, which is what keeps
//! a 20-person call workable). Every audio frame is encrypted in the browser
//! with a key derived from the chat's MLS group, so neither LiveKit nor this
//! server can listen in.
//!
//! This module hands out short-lived LiveKit access tokens to devices in a
//! chat, and keeps a small in-memory list of who is in each call so the app
//! can show "call in progress". Nothing about calls is written to disk.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64URL};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::{
    AppState,
    auth::now,
    chats::require_member,
    config::LiveKit,
    devices::{DeviceAuth, identity},
    error::{ApiError, ApiResult},
};

/// How long a LiveKit token can be used to connect. LiveKit keeps an
/// established connection going on its own after that.
const TOKEN_TTL_SECS: i64 = 10 * 60;
/// A device in a call checks in this often; one silent for longer has left.
pub const HEARTBEAT: Duration = Duration::from_secs(20);
const STALE_AFTER: Duration = Duration::from_secs(50);

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/calls", get(list_calls))
        .route("/chats/{id}/call/join", post(join))
        .route("/chats/{id}/call/heartbeat", post(heartbeat))
        .route("/chats/{id}/call/leave", post(leave))
}

#[derive(Clone, Copy)]
struct Participant {
    user_id: i64,
    seen: Instant,
}

/// Who is in which call right now, by chat id and then device id.
#[derive(Clone, Default)]
pub struct Calls(Arc<Mutex<HashMap<String, HashMap<i64, Participant>>>>);

impl Calls {
    /// Drops devices that stopped checking in. Returns the chats that changed.
    fn sweep(&self) -> Vec<String> {
        let mut calls = self.0.lock().unwrap();
        let mut changed = Vec::new();
        calls.retain(|chat_id, devices| {
            let before = devices.len();
            devices.retain(|_, p| p.seen.elapsed() < STALE_AFTER);
            if devices.len() != before {
                changed.push(chat_id.clone());
            }
            !devices.is_empty()
        });
        changed
    }

    /// Records a device as in the call. Returns true if it was not already.
    fn enter(&self, chat_id: &str, device_id: i64, user_id: i64) -> bool {
        let mut calls = self.0.lock().unwrap();
        let devices = calls.entry(chat_id.to_owned()).or_default();
        devices
            .insert(
                device_id,
                Participant {
                    user_id,
                    seen: Instant::now(),
                },
            )
            .is_none()
    }

    fn touch(&self, chat_id: &str, device_id: i64) -> bool {
        let mut calls = self.0.lock().unwrap();
        match calls.get_mut(chat_id).and_then(|d| d.get_mut(&device_id)) {
            Some(p) => {
                p.seen = Instant::now();
                true
            }
            None => false,
        }
    }

    fn exit(&self, chat_id: &str, device_id: i64) -> bool {
        let mut calls = self.0.lock().unwrap();
        let Some(devices) = calls.get_mut(chat_id) else {
            return false;
        };
        let removed = devices.remove(&device_id).is_some();
        if devices.is_empty() {
            calls.remove(chat_id);
        }
        removed
    }

    /// Users in the call, each listed once, in the order they joined by device id.
    fn users_in(&self, chat_id: &str) -> Vec<i64> {
        let calls = self.0.lock().unwrap();
        let mut users: Vec<i64> = calls
            .get(chat_id)
            .map(|d| {
                let mut list: Vec<_> = d.iter().collect();
                list.sort_by_key(|(id, _)| **id);
                list.into_iter().map(|(_, p)| p.user_id).collect()
            })
            .unwrap_or_default();
        let mut seen = std::collections::HashSet::new();
        users.retain(|u| seen.insert(*u));
        users
    }

    fn chat_ids(&self) -> Vec<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }
}

/// Removes devices that left without saying so, and tells the members.
/// Runs in the background for as long as the server does.
pub async fn sweep_forever(state: AppState) {
    let mut tick = tokio::time::interval(HEARTBEAT);
    loop {
        tick.tick().await;
        for chat_id in state.calls.sweep() {
            let _ = crate::chats::notify_chat(&state, &chat_id).await;
        }
    }
}

fn livekit(state: &AppState) -> ApiResult<&LiveKit> {
    state
        .config
        .livekit
        .as_ref()
        .ok_or(ApiError::Unavailable("voice calls are not set up on this server"))
}

/// A LiveKit access token (a JWT signed with the shared API secret) that
/// lets one device join one room, send audio, and hear everyone else.
pub fn access_token(livekit: &LiveKit, room: &str, identity: &str, name: &str, issued: i64) -> String {
    let header = B64URL.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let claims = json!({
        "iss": livekit.api_key,
        "sub": identity,
        "name": name,
        "nbf": issued - 10,
        "exp": issued + TOKEN_TTL_SECS,
        "video": {
            "room": room,
            "roomJoin": true,
            "canPublish": true,
            "canSubscribe": true,
            "canPublishData": false,
            "canUpdateOwnMetadata": false,
        },
    });
    let payload = B64URL.encode(claims.to_string());
    let signing_input = format!("{header}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(livekit.api_secret.as_bytes())
        .expect("HMAC accepts keys of any length");
    mac.update(signing_input.as_bytes());
    let signature = B64URL.encode(mac.finalize().into_bytes());
    format!("{signing_input}.{signature}")
}

/// Joins (or rejoins) the chat's call. Only devices holding the chat's keys
/// may, since only they could decrypt anything in it anyway.
async fn join(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let livekit = livekit(&state)?.clone();
    crate::chats::require_device_in_chat(&state.db, &chat_id, &auth).await?;
    let me = identity(auth.user.id, auth.device_id);
    let token = access_token(&livekit, &chat_id, &me, &auth.user.display_name, now());
    if state.calls.enter(&chat_id, auth.device_id, auth.user.id) {
        crate::chats::notify_chat(&state, &chat_id).await?;
    }
    Ok(Json(json!({
        "url": livekit.url,
        "token": token,
        "identity": me,
        "heartbeat_secs": HEARTBEAT.as_secs(),
    })))
}

async fn heartbeat(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
) -> ApiResult<StatusCode> {
    require_member(&state.db, &chat_id, auth.user.id).await?;
    if !state.calls.touch(&chat_id, auth.device_id) {
        // Swept as stale (for example after the laptop slept): join again.
        return Err(ApiError::Conflict("not in this call"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn leave(
    State(state): State<AppState>,
    auth: DeviceAuth,
    Path(chat_id): Path<String>,
) -> ApiResult<StatusCode> {
    if state.calls.exit(&chat_id, auth.device_id) {
        crate::chats::notify_chat(&state, &chat_id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Calls in progress in the caller's chats, as `{chat id: [user ids]}`.
async fn list_calls(State(state): State<AppState>, auth: DeviceAuth) -> ApiResult<Json<Value>> {
    let mine: Vec<(String,)> = sqlx::query_as("SELECT chat_id FROM chat_members WHERE user_id = ?")
        .bind(auth.user.id)
        .fetch_all(&state.db)
        .await?;
    let mine: std::collections::HashSet<String> = mine.into_iter().map(|(id,)| id).collect();
    let mut calls = serde_json::Map::new();
    for chat_id in state.calls.chat_ids() {
        if mine.contains(&chat_id) {
            let users = state.calls.users_in(&chat_id);
            if !users.is_empty() {
                calls.insert(chat_id, json!(users));
            }
        }
    }
    Ok(Json(
        json!({ "enabled": state.config.livekit.is_some(), "calls": calls }),
    ))
}

/// Takes a removed member (or signed-out device) out of the call list.
pub fn forget_user(state: &AppState, chat_id: &str, user_id: i64) {
    let mut calls = state.calls.0.lock().unwrap();
    if let Some(devices) = calls.get_mut(chat_id) {
        devices.retain(|_, p| p.user_id != user_id);
        if devices.is_empty() {
            calls.remove(chat_id);
        }
    }
}

pub fn forget_device(state: &AppState, device_id: i64) {
    let mut calls = state.calls.0.lock().unwrap();
    calls.retain(|_, devices| {
        devices.remove(&device_id);
        !devices.is_empty()
    });
}
