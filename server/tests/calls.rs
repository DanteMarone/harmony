//! Voice call access: tokens, who may join, and the "call in progress" list.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64URL};
use harmony_server::{
    AppState, app, auth,
    config::{Config, LiveKit},
};
use hmac::{Hmac, KeyInit, Mac};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::Sha256;
use tower::ServiceExt;

const SECRET: &str = "test-secret-that-is-long-enough-for-livekit";

async fn server(livekit: bool) -> (Router, AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.path().join("data"),
        web_dir: dir.path().join("web"),
        cookie_secure: true,
        livekit: livekit.then(|| LiveKit {
            url: "wss://calls.example.test".into(),
            api_key: "harmony-test".into(),
            api_secret: SECRET.into(),
        }),
    };
    let state = AppState::new(config).await.unwrap();
    (app(state.clone()), state, dir)
}

struct Person {
    app: Router,
    cookie: String,
    user_id: i64,
    device_id: i64,
}

impl Person {
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

    async fn join(app: &Router, invite: &str, username: &str) -> Person {
        let request = Request::builder()
            .method("POST")
            .uri("/api/auth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "invite": invite, "username": username, "password": "correct horse battery" })
                    .to_string(),
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
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
        let mut person = Person {
            app: app.clone(),
            cookie,
            user_id: user["id"].as_i64().unwrap(),
            device_id: 0,
        };
        person.device_id = person.ok("POST", "/api/devices", None).await["id"]
            .as_i64()
            .unwrap();
        person
    }

    async fn invite(&self) -> String {
        self.ok("POST", "/api/invites", None).await["code"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

/// Checks the signature the way LiveKit does and returns the claims.
fn verify(token: &str) -> Value {
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3);
    let header: Value = serde_json::from_slice(&B64URL.decode(parts[0]).unwrap()).unwrap();
    assert_eq!(header["alg"], "HS256");
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
    mac.verify_slice(&B64URL.decode(parts[2]).unwrap())
        .expect("signature is valid");
    serde_json::from_slice(&B64URL.decode(parts[1]).unwrap()).unwrap()
}

/// Marks a device as holding the chat's keys, as a real key change would.
async fn hold_keys(state: &AppState, chat: &str, device: i64) {
    sqlx::query("INSERT OR IGNORE INTO chat_devices (chat_id, device_id) VALUES (?, ?)")
        .bind(chat)
        .bind(device)
        .execute(&state.db)
        .await
        .unwrap();
}

#[tokio::test]
async fn members_get_call_tokens_and_see_who_is_in() {
    let (app, state, _dir) = server(true).await;
    let bootstrap = auth::bootstrap_invite(&state.db).await.unwrap().unwrap();
    let alice = Person::join(&app, &bootstrap, "alice").await;
    let bob = Person::join(&app, &alice.invite().await, "bob").await;
    let eve = Person::join(&app, &alice.invite().await, "eve").await;
    let chat = alice
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "group", "name": "raid", "member_ids": [bob.user_id] })),
        )
        .await["chat"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let join = format!("/api/chats/{chat}/call/join");

    // Bob is a member, but his device has not been given the chat's keys yet.
    assert_eq!(bob.call("POST", &join, None).await.0, StatusCode::CONFLICT);
    // Eve is not a member at all.
    assert_eq!(eve.call("POST", &join, None).await.0, StatusCode::NOT_FOUND);

    hold_keys(&state, &chat, alice.device_id).await;
    hold_keys(&state, &chat, bob.device_id).await;

    let joined = alice.ok("POST", &join, None).await;
    assert_eq!(joined["url"], "wss://calls.example.test");
    let identity = format!("u{}:d{}", alice.user_id, alice.device_id);
    assert_eq!(joined["identity"], identity.as_str());
    let claims = verify(joined["token"].as_str().unwrap());
    assert_eq!(claims["iss"], "harmony-test");
    assert_eq!(claims["sub"], identity.as_str());
    assert_eq!(claims["video"]["room"], chat.as_str());
    assert_eq!(claims["video"]["roomJoin"], true);
    assert_eq!(claims["video"]["canPublishData"], false);
    let lifetime = claims["exp"].as_i64().unwrap() - claims["nbf"].as_i64().unwrap();
    assert!(lifetime > 0 && lifetime <= 15 * 60);

    // Members see the call in progress; outsiders do not.
    let calls = bob.ok("GET", "/api/calls", None).await;
    assert_eq!(calls["enabled"], true);
    assert_eq!(calls["calls"][&chat], json!([alice.user_id]));
    assert_eq!(eve.ok("GET", "/api/calls", None).await["calls"], json!({}));

    bob.ok("POST", &join, None).await;
    alice
        .ok("POST", &format!("/api/chats/{chat}/call/heartbeat"), None)
        .await;
    assert_eq!(
        bob.ok("GET", "/api/calls", None).await["calls"][&chat],
        json!([alice.user_id, bob.user_id])
    );
    // A heartbeat from outside the call is refused, so the device knows to rejoin.
    assert_eq!(
        eve.call("POST", &format!("/api/chats/{chat}/call/heartbeat"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // Leaving, and being removed from the group, both take people out.
    alice
        .ok("POST", &format!("/api/chats/{chat}/call/leave"), None)
        .await;
    assert_eq!(
        bob.ok("GET", "/api/calls", None).await["calls"][&chat],
        json!([bob.user_id])
    );
    alice
        .ok(
            "DELETE",
            &format!("/api/chats/{chat}/members/{}", bob.user_id),
            None,
        )
        .await;
    assert_eq!(alice.ok("GET", "/api/calls", None).await["calls"], json!({}));
    assert_eq!(bob.call("POST", &join, None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn calls_are_off_until_livekit_is_configured() {
    let (app, state, _dir) = server(false).await;
    let bootstrap = auth::bootstrap_invite(&state.db).await.unwrap().unwrap();
    let alice = Person::join(&app, &bootstrap, "alice").await;
    let bob = Person::join(&app, &alice.invite().await, "bob").await;
    let chat = alice
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "dm", "member_ids": [bob.user_id] })),
        )
        .await["chat"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    hold_keys(&state, &chat, alice.device_id).await;
    let (status, body) = alice
        .call("POST", &format!("/api/chats/{chat}/call/join"), None)
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "voice calls are not set up on this server");
    assert_eq!(alice.ok("GET", "/api/calls", None).await["enabled"], false);
}

#[tokio::test]
async fn the_page_may_reach_the_livekit_server() {
    let (app, _state, _dir) = server(true).await;
    let response = app
        .oneshot(Request::builder().uri("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(csp.contains("connect-src 'self' wss: wss://calls.example.test https://calls.example.test;"));
    assert!(csp.contains("script-src 'self' 'wasm-unsafe-eval';"));
}
