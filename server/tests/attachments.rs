//! Photos and files: the server relays encrypted blobs and deletes them once
//! every device has its own copy, or when they expire.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use harmony_server::{
    AppState, app, attachments, auth,
    config::{AttachmentLimits, Config},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

struct Server {
    app: Router,
    state: AppState,
    _dir: tempfile::TempDir,
}

async fn server(limits: AttachmentLimits) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.path().join("data"),
        web_dir: dir.path().join("web"),
        cookie_secure: true,
        livekit: None,
        attachments: limits,
    };
    let state = AppState::new(config).await.unwrap();
    Server {
        app: app(state.clone()),
        state,
        _dir: dir,
    }
}

struct Person {
    app: Router,
    cookie: String,
    user_id: i64,
    device_id: i64,
}

impl Person {
    async fn send(&self, method: &str, path: &str, body: Body, json_body: bool) -> (StatusCode, Vec<u8>) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::COOKIE, &self.cookie)
            .header("x-harmony-device", self.device_id.to_string());
        if json_body {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        (
            status,
            response.into_body().collect().await.unwrap().to_bytes().to_vec(),
        )
    }

    async fn json(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (status, bytes) = match body {
            Some(b) => self.send(method, path, Body::from(b.to_string()), true).await,
            None => self.send(method, path, Body::empty(), false).await,
        };
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn ok(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (status, json) = self.json(method, path, body).await;
        assert!(status.is_success(), "{method} {path} -> {status}: {json}");
        json
    }

    async fn upload(&self, chat: &str, bytes: &[u8]) -> (StatusCode, Value) {
        let (status, body) = self
            .send(
                "POST",
                &format!("/api/chats/{chat}/attachments"),
                Body::from(bytes.to_vec()),
                false,
            )
            .await;
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    async fn download(&self, id: &str) -> (StatusCode, Vec<u8>) {
        self.send("GET", &format!("/api/attachments/{id}"), Body::empty(), false)
            .await
    }

    async fn new_device(&self) -> Person {
        let device = self.ok("POST", "/api/devices", None).await;
        Person {
            app: self.app.clone(),
            cookie: self.cookie.clone(),
            user_id: self.user_id,
            device_id: device["id"].as_i64().unwrap(),
        }
    }
}

async fn register(s: &Server, invite: &str, username: &str) -> Person {
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
    let account = Person {
        app: s.app.clone(),
        cookie,
        user_id: user["id"].as_i64().unwrap(),
        device_id: 0,
    };
    account.new_device().await
}

/// Ann (admin), Bob and Cat on one device each; a DM between Ann and Bob.
async fn setup(limits: AttachmentLimits) -> (Server, Person, Person, Person, String) {
    let s = server(limits).await;
    let bootstrap = auth::bootstrap_invite(&s.state.db).await.unwrap().unwrap();
    let ann = register(&s, &bootstrap, "ann").await;
    let code = ann.ok("POST", "/api/invites", None).await["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let bob = register(&s, &code, "bob").await;
    let code = ann.ok("POST", "/api/invites", None).await["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let cat = register(&s, &code, "cat").await;
    let chat = ann
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "dm", "member_ids": [bob.user_id] })),
        )
        .await["chat"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (s, ann, bob, cat, chat)
}

fn on_disk(s: &Server, id: &str) -> bool {
    s.state.config.data_dir.join("attachments").join(id).exists()
}

#[tokio::test]
async fn file_is_deleted_once_every_device_has_it() {
    let (s, ann, bob, cat, chat) = setup(AttachmentLimits::default()).await;
    let bob_laptop = bob.new_device().await;
    let ann_phone = ann.new_device().await;

    let (status, up) = ann.upload(&chat, b"ciphertext bytes").await;
    assert_eq!(status, StatusCode::CREATED, "{up}");
    let id = up["id"].as_str().unwrap().to_owned();
    assert_eq!(up["size"], 16);
    assert!(on_disk(&s, &id));

    // Someone outside the chat cannot fetch it, or learn that it exists.
    assert_eq!(cat.download(&id).await.0, StatusCode::NOT_FOUND);
    // Nor can they upload into the chat.
    assert_eq!(cat.upload(&chat, b"x").await.0, StatusCode::NOT_FOUND);

    // Bob's two devices and Ann's other device are waiting for it.
    for device in [&bob, &bob_laptop, &ann_phone] {
        let (status, bytes) = device.download(&id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bytes, b"ciphertext bytes");
    }
    // Someone else saying they received it changes nothing.
    assert_eq!(
        cat.send(
            "POST",
            &format!("/api/attachments/{id}/received"),
            Body::empty(),
            false
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    for device in [&bob, &bob_laptop] {
        device
            .ok("POST", &format!("/api/attachments/{id}/received"), None)
            .await;
        assert!(on_disk(&s, &id), "kept while a device still waits");
    }
    ann_phone
        .ok("POST", &format!("/api/attachments/{id}/received"), None)
        .await;
    assert!(!on_disk(&s, &id), "deleted once the last device has it");
    assert_eq!(bob.download(&id).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn signed_out_devices_stop_holding_files() {
    let (s, ann, bob, _cat, chat) = setup(AttachmentLimits::default()).await;
    let bob_old = bob.new_device().await;
    let (_, up) = ann.upload(&chat, b"hello").await;
    let id = up["id"].as_str().unwrap().to_owned();
    bob.ok("POST", &format!("/api/attachments/{id}/received"), None)
        .await;
    assert!(on_disk(&s, &id));

    // Bob signs his old device out instead of opening it again.
    bob.ok("DELETE", &format!("/api/devices/{}", bob_old.device_id), None)
        .await;
    attachments::sweep(&s.state).await.unwrap();
    assert!(!on_disk(&s, &id), "nobody left to deliver to");
}

#[tokio::test]
async fn unclaimed_files_expire() {
    let limits = AttachmentLimits {
        keep_secs: -1,
        ..Default::default()
    };
    let (s, ann, bob, _cat, chat) = setup(limits).await;
    let (_, up) = ann.upload(&chat, b"hello").await;
    let id = up["id"].as_str().unwrap().to_owned();
    assert_eq!(bob.download(&id).await.0, StatusCode::NOT_FOUND, "past its time");
    attachments::sweep(&s.state).await.unwrap();
    assert!(!on_disk(&s, &id));
}

#[tokio::test]
async fn sender_can_take_an_upload_back() {
    let (s, ann, bob, _cat, chat) = setup(AttachmentLimits::default()).await;
    let (_, up) = ann.upload(&chat, b"oops").await;
    let id = up["id"].as_str().unwrap().to_owned();
    assert_eq!(
        bob.send("DELETE", &format!("/api/attachments/{id}"), Body::empty(), false)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    ann.ok("DELETE", &format!("/api/attachments/{id}"), None).await;
    assert!(!on_disk(&s, &id));
}

#[tokio::test]
async fn size_limits_protect_the_disk() {
    let limits = AttachmentLimits {
        max_file_bytes: 10,
        max_total_bytes: 25,
        max_user_bytes: 15,
        ..Default::default()
    };
    let (s, ann, bob, _cat, chat) = setup(limits).await;
    assert_eq!(ann.upload(&chat, &[1; 11]).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(ann.upload(&chat, &[]).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(ann.upload(&chat, &[1; 10]).await.0, StatusCode::CREATED);
    // Ann may hold 15 bytes at once.
    assert_eq!(
        ann.upload(&chat, &[1; 6]).await.0,
        StatusCode::INSUFFICIENT_STORAGE
    );
    assert_eq!(bob.upload(&chat, &[1; 10]).await.0, StatusCode::CREATED);
    // Everyone together may hold 25.
    assert_eq!(bob.upload(&chat, &[1; 5]).await.0, StatusCode::CREATED);
    assert_eq!(
        bob.upload(&chat, &[1; 1]).await.0,
        StatusCode::INSUFFICIENT_STORAGE
    );
    // Nothing half-written is left behind.
    let files = std::fs::read_dir(s.state.config.data_dir.join("attachments"))
        .unwrap()
        .count();
    assert_eq!(files, 3);
}

#[tokio::test]
async fn ids_that_are_not_ours_never_reach_the_disk() {
    let (_s, ann, _bob, _cat, _chat) = setup(AttachmentLimits::default()).await;
    for id in ["..%2F..%2Fharmony.db", "ABC", &"A".repeat(64)] {
        assert_eq!(ann.download(id).await.0, StatusCode::NOT_FOUND);
    }
}
