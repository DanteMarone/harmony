//! Real-time nudges over WebSocket.
//!
//! The socket carries no content, only small "something changed, go fetch"
//! notices. All data moves over the normal authenticated API, which keeps
//! this part simple and means a dropped socket never loses anything.

use std::sync::Arc;

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use serde::Serialize;
use tokio::sync::broadcast;

use crate::{AppState, devices::DeviceAuth};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Notice {
    /// New events or a membership change in this chat.
    Chat { chat_id: String },
    /// The list of chats changed (added to one, removed from one, renamed).
    Chats,
    /// An invitation to join a chat is waiting for this device.
    Welcome,
    /// The list of rooms changed: one was made, renamed or deleted, or
    /// someone joined or left a room or its voice channel. Sent to everyone,
    /// since every account can see every room.
    Rooms,
    /// Someone's identity key or device signatures changed.
    Users,
    /// New items in this person's own history backup of a chat, from `from_seq` on.
    /// Their other devices fetch them, so messages sent before a device
    /// joined still reach it when another device backs them up later.
    Backup { chat_id: String, from_seq: i64 },
    /// This device was signed out. Sent only to that device; its socket closes after.
    Revoked { device_id: i64 },
}

#[derive(Clone, Debug)]
struct Envelope {
    users: Arc<[i64]>,
    notice: Notice,
}

#[derive(Clone)]
pub struct Hub(broadcast::Sender<Envelope>);

impl Default for Hub {
    fn default() -> Self {
        Self(broadcast::channel(1024).0)
    }
}

impl Hub {
    /// Sends a notice to every connected device of the given users.
    pub fn notify(&self, users: impl Into<Arc<[i64]>>, notice: Notice) {
        // An error only means nobody is connected right now.
        let _ = self.0.send(Envelope {
            users: users.into(),
            notice,
        });
    }
}

pub async fn connect(State(state): State<AppState>, auth: DeviceAuth, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| serve(socket, state.hub.clone(), auth.user.id, auth.device_id))
}

async fn serve(mut socket: WebSocket, hub: Hub, user_id: i64, device_id: i64) {
    let mut notices = hub.0.subscribe();
    loop {
        tokio::select! {
            notice = notices.recv() => match notice {
                Ok(envelope) if envelope.users.contains(&user_id) => {
                    let revoked = match envelope.notice {
                        Notice::Revoked { device_id: d } if d == device_id => true,
                        Notice::Revoked { .. } => continue,
                        _ => false,
                    };
                    let Ok(text) = serde_json::to_string(&envelope.notice) else { continue };
                    if socket.send(Message::Text(text.into())).await.is_err() || revoked {
                        break;
                    }
                }
                Ok(_) => {}
                // We fell behind and missed some notices: tell the client to refetch everything.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if socket.send(Message::Text(r#"{"type":"resync"}"#.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                // Clients send nothing meaningful; pings are answered automatically.
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
}
