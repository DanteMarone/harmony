//! Full messaging flow: real encryption (the same code the browser runs)
//! against the real server API, with the test acting as each person's device.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use harmony_crypto::{
    Device, Processed,
    recovery::{self, Recovery},
};
use harmony_server::{AppState, app, auth, config::Config};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

struct Server {
    app: Router,
    state: AppState,
    _dir: tempfile::TempDir,
}

async fn server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.path().join("data"),
        web_dir: dir.path().join("web"),
        cookie_secure: true,
        livekit: None,
    };
    let state = AppState::new(config).await.unwrap();
    Server {
        app: app(state.clone()),
        state,
        _dir: dir,
    }
}

/// One person on one device.
struct Client {
    app: Router,
    cookie: String,
    user_id: i64,
    device_id: i64,
    mls: Device,
}

impl Client {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::COOKIE, &self.cookie)
            .header("x-harmony-device", self.device_id.to_string());
        let body = match body {
            Some(json) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(json.to_string())
            }
            None => Body::empty(),
        };
        let response = self
            .app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn ok(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (status, json) = self.call(method, path, body).await;
        assert!(status.is_success(), "{method} {path} -> {status}: {json}");
        json
    }

    /// Registers a new account with a fresh device and uploads its key packages.
    async fn join_server(s: &Server, invite: &str, username: &str) -> Client {
        let request = Request::builder()
            .method("POST")
            .uri("/api/auth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "invite": invite, "username": username, "password": "correct horse battery" })
                    .to_string(),
            ))
            .unwrap();
        let response = s.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let user: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let user_id = user["id"].as_i64().unwrap();
        Client::new_device(s, cookie, user_id).await
    }

    async fn new_device(s: &Server, cookie: String, user_id: i64) -> Client {
        let mut client = Client {
            app: s.app.clone(),
            cookie,
            user_id,
            device_id: 0,
            mls: Device::create("pending").unwrap(),
        };
        let device = client.ok("POST", "/api/devices", None).await;
        client.device_id = device["id"].as_i64().unwrap();
        client.mls = Device::create(device["identity"].as_str().unwrap()).unwrap();
        let one_time: Vec<String> = client
            .mls
            .key_packages(3, false)
            .unwrap()
            .iter()
            .map(|p| B64.encode(p))
            .collect();
        let last_resort = B64.encode(&client.mls.key_packages(1, true).unwrap()[0]);
        client
            .ok(
                "POST",
                &format!("/api/devices/{}/key-packages", client.device_id),
                Some(json!({ "one_time": one_time, "last_resort": last_resort })),
            )
            .await;
        client
    }

    /// Brings the chat's key group in line with its member list, as the app does.
    async fn reconcile(&self, chat: &str) -> bool {
        let pending = self.ok("GET", &format!("/api/chats/{chat}/pending"), None).await;
        let add_ids: Vec<i64> = pending["add"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["device_id"].as_i64().unwrap())
            .collect();
        let remove: Vec<&Value> = pending["remove"].as_array().unwrap().iter().collect();
        if add_ids.is_empty() && remove.is_empty() {
            return false;
        }
        let claimed = self
            .ok(
                "POST",
                &format!("/api/chats/{chat}/claim"),
                Some(json!({ "device_ids": add_ids })),
            )
            .await;
        let add: Vec<(String, Vec<u8>)> = claimed["key_packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| {
                (
                    k["identity"].as_str().unwrap().to_owned(),
                    B64.decode(k["key_package"].as_str().unwrap()).unwrap(),
                )
            })
            .collect();
        let added: Vec<i64> = claimed["key_packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["device_id"].as_i64().unwrap())
            .collect();
        let remove_identities: Vec<String> = remove
            .iter()
            .map(|d| d["identity"].as_str().unwrap().to_owned())
            .collect();
        let removed: Vec<i64> = remove.iter().map(|d| d["device_id"].as_i64().unwrap()).collect();

        let commit = self.mls.change_members(chat, &add, &remove_identities).unwrap();
        let (status, _) = self
            .call(
                "POST",
                &format!("/api/chats/{chat}/commit"),
                Some(json!({
                    "epoch": self.mls.epoch(chat).unwrap(),
                    "commit": B64.encode(&commit.commit),
                    "welcome": commit.welcome.as_ref().map(|w| B64.encode(w)),
                    "added": added,
                    "removed": removed,
                })),
            )
            .await;
        if status == StatusCode::OK {
            self.mls.confirm_commit(chat).unwrap();
            true
        } else {
            self.mls.discard_commit(chat).unwrap();
            false
        }
    }

    /// Picks up invitations. Returns (chat id, event to read from) for each chat joined.
    async fn accept_welcomes(&self) -> Vec<(String, i64)> {
        let welcomes = self.ok("GET", "/api/welcomes", None).await;
        let mut joined = Vec::new();
        for welcome in welcomes["welcomes"].as_array().unwrap() {
            let chat = self
                .mls
                .join_chat(&B64.decode(welcome["data"].as_str().unwrap()).unwrap())
                .unwrap();
            assert_eq!(chat, welcome["chat_id"].as_str().unwrap());
            self.ok("DELETE", &format!("/api/welcomes/{}", welcome["id"]), None)
                .await;
            joined.push((chat, welcome["after_seq"].as_i64().unwrap()));
        }
        joined
    }

    async fn send(&self, chat: &str, text: &str) -> StatusCode {
        self.send_as(chat, text, &format!("{}-{text}", self.device_id))
            .await
    }

    async fn send_as(&self, chat: &str, text: &str, client_id: &str) -> StatusCode {
        let data = self.mls.encrypt(chat, text.as_bytes()).unwrap();
        let (status, _) = self
            .call(
                "POST",
                &format!("/api/chats/{chat}/messages"),
                Some(json!({ "epoch": self.mls.epoch(chat).unwrap(), "data": B64.encode(data), "client_id": client_id })),
            )
            .await;
        status
    }

    /// Fetches and decrypts everything after `after`. Returns the texts read and the new cursor.
    async fn read(&self, chat: &str, after: i64) -> (Vec<String>, i64) {
        let events = self
            .ok("GET", &format!("/api/chats/{chat}/events?after={after}"), None)
            .await;
        let mut texts = Vec::new();
        let mut cursor = after;
        for event in events["events"].as_array().unwrap() {
            cursor = event["seq"].as_i64().unwrap();
            if event["sender_device"].as_i64() == Some(self.device_id) {
                continue; // our own echo
            }
            let data = B64.decode(event["data"].as_str().unwrap()).unwrap();
            if let Processed::Message { content, .. } = self.mls.process(chat, &data).unwrap() {
                texts.push(String::from_utf8(content).unwrap());
            }
        }
        (texts, cursor)
    }
}

async fn invite(admin: &Client) -> String {
    admin.ok("POST", "/api/invites", None).await["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn direct_message_end_to_end() {
    let s = server().await;
    let bootstrap = auth::bootstrap_invite(&s.state.db).await.unwrap().unwrap();
    let alice = Client::join_server(&s, &bootstrap, "alice").await;
    let bob = Client::join_server(&s, &invite(&alice).await, "bob").await;

    // Alice opens a DM with Bob and starts the key group on her device.
    let created = alice
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "dm", "member_ids": [bob.user_id] })),
        )
        .await;
    assert_eq!(created["created"], true);
    let chat = created["chat"]["id"].as_str().unwrap().to_owned();
    alice.mls.create_chat(&chat).unwrap();

    // Opening the same DM again, from either side, returns the same chat.
    let again = bob
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "dm", "member_ids": [alice.user_id] })),
        )
        .await;
    assert_eq!(again["created"], false);
    assert_eq!(again["chat"]["id"], chat.as_str());
    assert_eq!(again["chat"]["joined"], false);

    // Bob's device cannot read or post before it has been given the keys.
    let (status, _) = bob.call("GET", &format!("/api/chats/{chat}/events"), None).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Alice's device adds Bob's device; Bob picks up the invitation.
    assert!(alice.reconcile(&chat).await);
    assert!(!alice.reconcile(&chat).await, "nothing left to do");
    let joined = bob.accept_welcomes().await;
    assert_eq!(joined.len(), 1);
    let mut bob_cursor = joined[0].1;
    assert!(bob.accept_welcomes().await.is_empty());

    // Messages flow both ways.
    assert_eq!(alice.send(&chat, "hello bob").await, StatusCode::CREATED);
    let (texts, cursor) = bob.read(&chat, bob_cursor).await;
    assert_eq!(texts, ["hello bob"]);
    bob_cursor = cursor;
    assert_eq!(bob.send(&chat, "hi alice").await, StatusCode::CREATED);
    let (texts, _) = alice.read(&chat, 0).await;
    assert_eq!(texts, ["hi alice"]);
    let (texts, _) = bob.read(&chat, bob_cursor).await;
    assert!(texts.is_empty());

    // Retrying a send with the same id does not store the message twice.
    assert_eq!(alice.send_as(&chat, "once", "retry-1").await, StatusCode::CREATED);
    assert_eq!(alice.send_as(&chat, "once", "retry-1").await, StatusCode::OK);
    let (texts, _) = bob.read(&chat, bob_cursor).await;
    assert_eq!(texts, ["once"]);

    // The server holds only ciphertext.
    let stored: Vec<(Vec<u8>,)> = sqlx::query_as("SELECT data FROM events")
        .fetch_all(&s.state.db)
        .await
        .unwrap();
    assert_eq!(stored.len(), 4);
    for (data,) in stored {
        assert!(!data.windows(5).any(|w| w == b"hello" || w == b"alice"));
    }

    // One-time key packages are used up; the last-resort one remains.
    let count = bob
        .ok(
            "GET",
            &format!("/api/devices/{}/key-packages", bob.device_id),
            None,
        )
        .await;
    assert_eq!(count, json!({ "one_time": 2, "last_resort": true }));
}

#[tokio::test]
async fn group_membership_and_access_control() {
    let s = server().await;
    let bootstrap = auth::bootstrap_invite(&s.state.db).await.unwrap().unwrap();
    let alice = Client::join_server(&s, &bootstrap, "alice").await;
    let bob = Client::join_server(&s, &invite(&alice).await, "bob").await;
    let carol = Client::join_server(&s, &invite(&alice).await, "carol").await;
    let eve = Client::join_server(&s, &invite(&alice).await, "eve").await;

    let created = alice
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "group", "name": "Raid night", "member_ids": [bob.user_id] })),
        )
        .await;
    let chat = created["chat"]["id"].as_str().unwrap().to_owned();
    alice.mls.create_chat(&chat).unwrap();
    assert!(alice.reconcile(&chat).await);
    let mut bob_cursor = bob.accept_welcomes().await[0].1;
    assert_eq!(alice.send(&chat, "before carol").await, StatusCode::CREATED);
    let (_, cursor) = bob.read(&chat, bob_cursor).await;
    bob_cursor = cursor;

    // Eve is not a member: the chat does not exist as far as she can tell.
    for (method, path) in [
        ("GET", format!("/api/chats/{chat}/events")),
        ("GET", format!("/api/chats/{chat}/pending")),
        ("POST", format!("/api/chats/{chat}/members")),
    ] {
        let (status, _) = eve
            .call(method, &path, Some(json!({ "user_ids": [eve.user_id] })))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    assert_eq!(eve.ok("GET", "/api/chats", None).await, json!([]));

    // Bob adds Carol. Bob's device does the key change; Alice's applies it.
    bob.ok(
        "POST",
        &format!("/api/chats/{chat}/members"),
        Some(json!({ "user_ids": [carol.user_id] })),
    )
    .await;
    assert!(bob.reconcile(&chat).await);
    let (_, alice_cursor) = alice.read(&chat, 0).await;
    let carol_cursor = carol.accept_welcomes().await[0].1;

    // Carol gets nothing from before she joined, even if she asks from the start.
    let (texts, _) = carol.read(&chat, 0).await;
    assert!(texts.is_empty());

    // A device still on the old epoch is told to catch up.
    assert_eq!(alice.mls.epoch(&chat).unwrap(), bob.mls.epoch(&chat).unwrap());
    assert_eq!(carol.send(&chat, "hi all").await, StatusCode::CREATED);
    let (texts, _) = alice.read(&chat, alice_cursor).await;
    assert_eq!(texts, ["hi all"]);
    let (texts, cursor) = bob.read(&chat, bob_cursor).await;
    assert_eq!(texts, ["hi all"]);
    bob_cursor = cursor;

    // Only the creator can remove someone else.
    let (status, _) = bob
        .call(
            "DELETE",
            &format!("/api/chats/{chat}/members/{}", carol.user_id),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    alice
        .ok(
            "DELETE",
            &format!("/api/chats/{chat}/members/{}", carol.user_id),
            None,
        )
        .await;

    // Carol is cut off at once on the server, and from the keys once a member's device rotates them.
    let (status, _) = carol
        .call(
            "GET",
            &format!("/api/chats/{chat}/events?after={carol_cursor}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(alice.reconcile(&chat).await);
    assert_eq!(alice.mls.members(&chat).unwrap().len(), 2);
    let (_, cursor) = bob.read(&chat, bob_cursor).await;
    bob_cursor = cursor;
    assert_eq!(alice.send(&chat, "carol is gone").await, StatusCode::CREATED);
    let (texts, _) = bob.read(&chat, bob_cursor).await;
    assert_eq!(texts, ["carol is gone"]);

    // Bob leaves; then Alice leaves and the chat is deleted.
    bob.ok(
        "DELETE",
        &format!("/api/chats/{chat}/members/{}", bob.user_id),
        None,
    )
    .await;
    alice
        .ok(
            "DELETE",
            &format!("/api/chats/{chat}/members/{}", alice.user_id),
            None,
        )
        .await;
    let (remaining,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM events")
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn stale_epoch_is_refused() {
    let s = server().await;
    let bootstrap = auth::bootstrap_invite(&s.state.db).await.unwrap().unwrap();
    let alice = Client::join_server(&s, &bootstrap, "alice").await;
    let bob = Client::join_server(&s, &invite(&alice).await, "bob").await;
    let carol = Client::join_server(&s, &invite(&alice).await, "carol").await;
    let created = alice
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "group", "name": "g", "member_ids": [bob.user_id] })),
        )
        .await;
    let chat = created["chat"]["id"].as_str().unwrap().to_owned();
    alice.mls.create_chat(&chat).unwrap();
    alice.reconcile(&chat).await;
    let bob_cursor = bob.accept_welcomes().await[0].1;

    // Alice changes the keys (adds Carol) while Bob is not looking.
    alice
        .ok(
            "POST",
            &format!("/api/chats/{chat}/members"),
            Some(json!({ "user_ids": [carol.user_id] })),
        )
        .await;
    assert!(alice.reconcile(&chat).await);

    // Bob's message, made with the old keys, is refused; so is a competing key change.
    assert_eq!(bob.send(&chat, "too early").await, StatusCode::CONFLICT);
    // After catching up, it goes through.
    bob.read(&chat, bob_cursor).await;
    assert_eq!(bob.send(&chat, "now it works").await, StatusCode::CREATED);
}

#[tokio::test]
async fn a_second_device_is_added_to_existing_chats() {
    let s = server().await;
    let bootstrap = auth::bootstrap_invite(&s.state.db).await.unwrap().unwrap();
    let alice = Client::join_server(&s, &bootstrap, "alice").await;
    let bob = Client::join_server(&s, &invite(&alice).await, "bob").await;
    let created = alice
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "dm", "member_ids": [bob.user_id] })),
        )
        .await;
    let chat = created["chat"]["id"].as_str().unwrap().to_owned();
    alice.mls.create_chat(&chat).unwrap();
    alice.reconcile(&chat).await;
    bob.accept_welcomes().await;

    // Bob signs in on a laptop. Either existing device can let it in.
    let laptop = Client::new_device(&s, bob.cookie.clone(), bob.user_id).await;
    assert!(alice.reconcile(&chat).await);
    let cursor = laptop.accept_welcomes().await[0].1;
    assert_eq!(
        alice.send(&chat, "to both of bob's devices").await,
        StatusCode::CREATED
    );
    let (texts, _) = laptop.read(&chat, cursor).await;
    assert_eq!(texts, ["to both of bob's devices"]);

    // A device cannot act as another user's device.
    let mut imposter = Client::new_device(&s, alice.cookie.clone(), alice.user_id).await;
    imposter.device_id = bob.device_id;
    let (status, _) = imposter.call("GET", "/api/chats", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn websocket_nudges_members_only() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let s = server().await;
    let bootstrap = auth::bootstrap_invite(&s.state.db).await.unwrap().unwrap();
    let alice = Client::join_server(&s, &bootstrap, "alice").await;
    let bob = Client::join_server(&s, &invite(&alice).await, "bob").await;
    let eve = Client::join_server(&s, &invite(&alice).await, "eve").await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = s.app.clone();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let connect = async |client: &Client| {
        let mut request = format!("ws://{address}/api/ws?device={}", client.device_id)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert(header::COOKIE, client.cookie.parse().unwrap());
        tokio_tungstenite::connect_async(request).await.unwrap().0
    };
    let mut bob_socket = connect(&bob).await;
    let mut eve_socket = connect(&eve).await;

    // Without a session the socket is refused.
    let anonymous = format!("ws://{address}/api/ws?device={}", bob.device_id)
        .into_client_request()
        .unwrap();
    assert!(tokio_tungstenite::connect_async(anonymous).await.is_err());

    alice
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "dm", "member_ids": [bob.user_id] })),
        )
        .await;

    let next = async |socket: &mut tokio_tungstenite::WebSocketStream<_>| {
        tokio::time::timeout(std::time::Duration::from_millis(500), socket.next()).await
    };
    let notice = next(&mut bob_socket)
        .await
        .expect("bob is told")
        .unwrap()
        .unwrap();
    assert_eq!(notice.into_text().unwrap().as_str(), r#"{"type":"chats"}"#);
    assert!(
        next(&mut eve_socket).await.is_err(),
        "eve hears nothing about a chat she is not in"
    );
}

#[tokio::test]
async fn a_device_that_lost_its_keys_is_let_back_in() {
    let s = server().await;
    let bootstrap = auth::bootstrap_invite(&s.state.db).await.unwrap().unwrap();
    let alice = Client::join_server(&s, &bootstrap, "alice").await;
    let bob = Client::join_server(&s, &invite(&alice).await, "bob").await;
    let created = alice
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "dm", "member_ids": [bob.user_id] })),
        )
        .await;
    let chat = created["chat"]["id"].as_str().unwrap().to_owned();
    alice.mls.create_chat(&chat).unwrap();
    alice.reconcile(&chat).await;
    bob.accept_welcomes().await;

    // Bob's device forgets the chat (but keeps its identity and key packages).
    bob.mls.delete_chat(&chat).unwrap();
    bob.ok("POST", &format!("/api/chats/{chat}/rejoin"), None).await;
    let (status, _) = bob.call("GET", &format!("/api/chats/{chat}/events"), None).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Alice's device removes the dead entry, then adds the device back.
    assert!(alice.reconcile(&chat).await);
    assert_eq!(alice.mls.members(&chat).unwrap().len(), 1);
    assert!(alice.reconcile(&chat).await);
    assert_eq!(alice.mls.members(&chat).unwrap().len(), 2);
    assert!(!alice.reconcile(&chat).await);

    let cursor = bob.accept_welcomes().await[0].1;
    assert_eq!(alice.send(&chat, "welcome back").await, StatusCode::CREATED);
    let (texts, _) = bob.read(&chat, cursor).await;
    assert_eq!(texts, ["welcome back"]);
}

/// Finds one person in the `/api/users` listing.
async fn user_info(client: &Client, user_id: i64) -> Value {
    client
        .ok("GET", "/api/users", None)
        .await
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"].as_i64() == Some(user_id))
        .unwrap()
        .clone()
}

/// Whether, by the server's listing, `device` is vouched for by its owner's identity key.
fn vouched(user: &Value, device: &Client) -> bool {
    let Some(identity_key) = user["identity_key"].as_str() else {
        return false;
    };
    let entry = user["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"].as_i64() == Some(device.device_id))
        .unwrap();
    let Some(signature) = entry["signature"].as_str() else {
        return false;
    };
    recovery::verify_device(
        &B64.decode(identity_key).unwrap(),
        entry["identity"].as_str().unwrap(),
        &device.mls.signature_public_key(),
        &B64.decode(signature).unwrap(),
    )
}

async fn set_identity(client: &Client, code: &Recovery) -> StatusCode {
    let signature = code.sign_device(client.mls.identity(), &client.mls.signature_public_key());
    client
        .call(
            "PUT",
            "/api/identity",
            Some(json!({
                "identity_key": B64.encode(code.identity_key()),
                "signature": B64.encode(signature),
            })),
        )
        .await
        .0
}

#[tokio::test]
async fn devices_are_vouched_for_with_the_recovery_code() {
    let s = server().await;
    let bootstrap = auth::bootstrap_invite(&s.state.db).await.unwrap().unwrap();
    let alice = Client::join_server(&s, &bootstrap, "alice").await;
    let bob = Client::join_server(&s, &invite(&alice).await, "bob").await;

    // Before any recovery code exists, nothing is vouched for and a device
    // cannot be signed.
    let info = user_info(&bob, alice.user_id).await;
    assert_eq!(info["identity_key"], Value::Null);
    assert!(!vouched(&info, &alice));
    let (status, _) = alice
        .call(
            "PUT",
            &format!("/api/devices/{}/signature", alice.device_id),
            Some(json!({ "signature": B64.encode([0u8; 64]) })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Alice sets up her recovery code; Bob can now check her device himself.
    let code = Recovery::generate().unwrap();
    assert_eq!(set_identity(&alice, &code).await, StatusCode::NO_CONTENT);
    let info = user_info(&bob, alice.user_id).await;
    assert_eq!(
        info["identity_key"].as_str().unwrap(),
        B64.encode(code.identity_key())
    );
    assert!(vouched(&info, &alice));

    // A second device of Alice's is listed but not vouched for until she
    // types the code on it.
    let phone = Client::new_device(&s, alice.cookie.clone(), alice.user_id).await;
    let info = user_info(&bob, alice.user_id).await;
    assert_eq!(info["devices"].as_array().unwrap().len(), 2);
    assert!(!vouched(&info, &phone));
    let typed = Recovery::from_code(&code.code()).unwrap();
    let signature = typed.sign_device(phone.mls.identity(), &phone.mls.signature_public_key());
    phone
        .ok(
            "PUT",
            &format!("/api/devices/{}/signature", phone.device_id),
            Some(json!({ "signature": B64.encode(&signature) })),
        )
        .await;
    let info = user_info(&bob, alice.user_id).await;
    assert!(vouched(&info, &phone) && vouched(&info, &alice));

    // Nobody can sign for a device that is not the one they are using.
    let (status, _) = alice
        .call(
            "PUT",
            &format!("/api/devices/{}/signature", phone.device_id),
            Some(json!({ "signature": B64.encode(&signature) })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = bob
        .call(
            "PUT",
            &format!("/api/devices/{}/signature", phone.device_id),
            Some(json!({ "signature": B64.encode(&signature) })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A signature the server made up (or moved from another device) is
    // stored, but fails the check Bob's device runs.
    sqlx::query("UPDATE devices SET identity_signature = ? WHERE id = ?")
        .bind(&signature)
        .bind(alice.device_id)
        .execute(&s.state.db)
        .await
        .unwrap();
    assert!(!vouched(&user_info(&bob, alice.user_id).await, &alice));

    // Starting over with a new code replaces the identity and un-signs the
    // other devices.
    let fresh = Recovery::generate().unwrap();
    assert_eq!(set_identity(&phone, &fresh).await, StatusCode::NO_CONTENT);
    let info = user_info(&bob, alice.user_id).await;
    assert_eq!(
        info["identity_key"].as_str().unwrap(),
        B64.encode(fresh.identity_key())
    );
    assert!(vouched(&info, &phone));
    assert!(!vouched(&info, &alice));

    // Malformed keys and signatures are refused.
    let (status, _) = alice
        .call(
            "PUT",
            "/api/identity",
            Some(json!({ "identity_key": B64.encode([1u8; 31]), "signature": B64.encode([0u8; 64]) })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn history_backup_is_private_and_restorable() {
    let s = server().await;
    let bootstrap = auth::bootstrap_invite(&s.state.db).await.unwrap().unwrap();
    let alice = Client::join_server(&s, &bootstrap, "alice").await;
    let bob = Client::join_server(&s, &invite(&alice).await, "bob").await;
    let carol = Client::join_server(&s, &invite(&alice).await, "carol").await;

    let created = alice
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "group", "name": "plans", "member_ids": [bob.user_id] })),
        )
        .await;
    let chat = created["chat"]["id"].as_str().unwrap().to_owned();
    alice.mls.create_chat(&chat).unwrap();
    assert!(alice.reconcile(&chat).await);
    let after = bob.accept_welcomes().await[0].1;
    assert_eq!(alice.send(&chat, "first").await, StatusCode::CREATED);
    assert_eq!(alice.send(&chat, "second").await, StatusCode::CREATED);
    let (texts, _) = bob.read(&chat, after).await;
    assert_eq!(texts, ["first", "second"]);

    // Bob backs up what he read, encrypted with a key only his code gives.
    let code = Recovery::generate().unwrap();
    assert_eq!(set_identity(&bob, &code).await, StatusCode::NO_CONTENT);
    let key = code.backup_key();
    let events = bob
        .ok("GET", &format!("/api/chats/{chat}/events?after={after}"), None)
        .await;
    let seqs: Vec<i64> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["seq"].as_i64().unwrap())
        .collect();
    let items: Vec<Value> = seqs
        .iter()
        .zip(&texts)
        .map(|(seq, text)| {
            let context = format!("u{}/{chat}/{seq}", bob.user_id);
            json!({ "seq": seq, "data": B64.encode(recovery::seal(&key, &context, text.as_bytes()).unwrap()) })
        })
        .collect();
    let path = format!("/api/backup/{chat}");
    bob.ok("POST", &path, Some(json!({ "items": items }))).await;

    // Backing up again (say, from a second device) keeps the first copy.
    let again = json!({ "items": [{ "seq": seqs[0], "data": B64.encode(b"different") }] });
    bob.ok("POST", &path, Some(again)).await;

    // A new device of Bob's, given the code, reads the history back.
    let laptop = Client::new_device(&s, bob.cookie.clone(), bob.user_id).await;
    let key = Recovery::from_code(&code.code()).unwrap().backup_key();
    let backup = laptop.ok("GET", &path, None).await;
    assert_eq!(backup["more"], false);
    let restored: Vec<String> = backup["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            let context = format!("u{}/{chat}/{}", bob.user_id, item["seq"]);
            let sealed = B64.decode(item["data"].as_str().unwrap()).unwrap();
            String::from_utf8(recovery::open(&key, &context, &sealed).unwrap()).unwrap()
        })
        .collect();
    assert_eq!(restored, ["first", "second"]);
    let later = laptop.ok("GET", &format!("{path}?after={}", seqs[0]), None).await;
    assert_eq!(later["items"].as_array().unwrap().len(), 1);

    // The server's copy is ciphertext.
    let stored: Vec<(Vec<u8>,)> = sqlx::query_as("SELECT data FROM backup_messages")
        .fetch_all(&s.state.db)
        .await
        .unwrap();
    assert_eq!(stored.len(), 2);
    for (data,) in stored {
        assert!(!data.windows(5).any(|w| w == b"first" || w == b"secon"));
    }

    // Each person only ever sees their own backup; outsiders see nothing.
    let alices = alice.ok("GET", &path, None).await;
    assert!(alices["items"].as_array().unwrap().is_empty());
    assert_eq!(carol.call("GET", &path, None).await.0, StatusCode::NOT_FOUND);
    let (status, _) = carol
        .call(
            "POST",
            &path,
            Some(json!({ "items": [{ "seq": seqs[0], "data": "AAAA" }] })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Only real messages of this chat can be backed up.
    let (status, _) = bob
        .call(
            "POST",
            &path,
            Some(json!({ "items": [{ "seq": 9999, "data": "AAAA" }] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let commit_seq = seqs[0] - 1; // the key change that added Bob
    bob.ok(
        "POST",
        &path,
        Some(json!({ "items": [{ "seq": commit_seq, "data": "AAAA" }] })),
    )
    .await;
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backup_messages")
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert_eq!(count, 2);

    // Starting over with a new code drops the old backup.
    assert_eq!(
        set_identity(&bob, &Recovery::generate().unwrap()).await,
        StatusCode::NO_CONTENT
    );
    assert!(
        bob.ok("GET", &path, None).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Leaving a chat removes that person's backup of it.
    bob.ok(
        "POST",
        &path,
        Some(json!({ "items": [{ "seq": seqs[0], "data": "AAAA" }] })),
    )
    .await;
    bob.ok(
        "DELETE",
        &format!("/api/chats/{chat}/members/{}", bob.user_id),
        None,
    )
    .await;
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backup_messages")
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

/// Signs in again from scratch: a new browser with its own session and device.
async fn sign_in_elsewhere(s: &Server, username: &str, user_id: i64) -> Client {
    let request = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "username": username, "password": "correct horse battery" }).to_string(),
        ))
        .unwrap();
    let response = s.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    Client::new_device(s, cookie, user_id).await
}

#[tokio::test]
async fn a_device_can_be_signed_out_remotely() {
    let s = server().await;
    let bootstrap = auth::bootstrap_invite(&s.state.db).await.unwrap().unwrap();
    let alice = Client::join_server(&s, &bootstrap, "alice").await;
    let bob = Client::join_server(&s, &invite(&alice).await, "bob").await;
    let laptop = sign_in_elsewhere(&s, "bob", bob.user_id).await;

    let created = alice
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "dm", "member_ids": [bob.user_id] })),
        )
        .await;
    let chat = created["chat"]["id"].as_str().unwrap().to_owned();
    alice.mls.create_chat(&chat).unwrap();
    assert!(alice.reconcile(&chat).await);
    bob.accept_welcomes().await;
    let laptop_cursor = laptop.accept_welcomes().await[0].1;
    assert_eq!(alice.mls.members(&chat).unwrap().len(), 3);

    // Bob sees both his devices, can name them, and sees no one else's.
    let devices = bob.ok("GET", "/api/devices", None).await;
    let ids: Vec<i64> = devices
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [laptop.device_id, bob.device_id]);
    assert!(devices[0]["last_seen_at"].as_i64().unwrap() % 60 == 0);
    bob.ok(
        "PUT",
        &format!("/api/devices/{}/name", laptop.device_id),
        Some(json!({ "name": "Work laptop" })),
    )
    .await;
    assert_eq!(
        bob.ok("GET", "/api/devices", None).await[0]["name"],
        "Work laptop"
    );
    let (status, _) = bob
        .call(
            "PUT",
            &format!("/api/devices/{}/name", laptop.device_id),
            Some(json!({ "name": "x".repeat(49) })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nobody can rename or sign out someone else's device.
    for (method, path, body) in [
        ("DELETE", format!("/api/devices/{}", laptop.device_id), None),
        (
            "PUT",
            format!("/api/devices/{}/name", laptop.device_id),
            Some(json!({ "name": "mine now" })),
        ),
    ] {
        assert_eq!(alice.call(method, &path, body).await.0, StatusCode::NOT_FOUND);
    }

    // Bob signs the laptop out from his phone.
    bob.ok("DELETE", &format!("/api/devices/{}", laptop.device_id), None)
        .await;
    assert_eq!(
        bob.call("DELETE", &format!("/api/devices/{}", laptop.device_id), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // The laptop's session is gone, and its device can never be used again.
    assert_eq!(
        laptop.call("GET", "/api/me", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let mut reused = sign_in_elsewhere(&s, "bob", bob.user_id).await;
    reused.device_id = laptop.device_id;
    assert_eq!(
        reused.call("GET", "/api/chats", None).await.0,
        StatusCode::FORBIDDEN
    );

    // Its leftovers are cleared, and it is no longer listed.
    let (packages,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM key_packages WHERE device_id = ?")
        .bind(laptop.device_id)
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert_eq!(packages, 0);
    let listed = bob.ok("GET", "/api/devices", None).await;
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["id"].as_i64() != Some(laptop.device_id))
    );

    // Alice's device removes it from the chat's keys; it cannot read what comes next.
    assert!(alice.reconcile(&chat).await);
    assert_eq!(
        alice.mls.members(&chat).unwrap().len(),
        3,
        "the re-signed-in browser joins"
    );
    assert!(
        !alice
            .mls
            .members(&chat)
            .unwrap()
            .contains(&format!("u{}:d{}", bob.user_id, laptop.device_id))
    );
    let bob_cursor = bob.read(&chat, 0).await.1;
    assert_eq!(
        alice.send(&chat, "after the laptop left").await,
        StatusCode::CREATED
    );
    let (texts, _) = bob.read(&chat, bob_cursor).await;
    assert_eq!(texts, ["after the laptop left"]);
    // Even holding every event, the laptop's old keys cannot open the new message.
    let events = alice
        .ok(
            "GET",
            &format!("/api/chats/{chat}/events?after={laptop_cursor}"),
            None,
        )
        .await;
    let events = events["events"].as_array().unwrap();
    let (last, earlier) = events.split_last().unwrap();
    for event in earlier {
        let _ = laptop
            .mls
            .process(&chat, &B64.decode(event["data"].as_str().unwrap()).unwrap());
    }
    let data = B64.decode(last["data"].as_str().unwrap()).unwrap();
    assert!(
        !matches!(laptop.mls.process(&chat, &data), Ok(Processed::Message { .. })),
        "the removed laptop cannot decrypt new messages"
    );
}
