//! Rooms: anyone can see and join them, each has a voice channel, and a
//! room outlives its members.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use harmony_server::{
    AppState, app, auth,
    config::{Config, LiveKit},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
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

/// Marks a device as holding the chat's keys, as a real key change would.
async fn hold_keys(state: &AppState, chat: &str, device: i64) {
    sqlx::query("INSERT OR IGNORE INTO chat_devices (chat_id, device_id) VALUES (?, ?)")
        .bind(chat)
        .bind(device)
        .execute(&state.db)
        .await
        .unwrap();
}

fn room<'a>(list: &'a Value, id: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .expect("room is listed")
}

#[tokio::test]
async fn anyone_can_see_and_join_a_room() {
    let (app, state, _dir) = server(true).await;
    let bootstrap = auth::bootstrap_invite(&state.db).await.unwrap().unwrap();
    let alice = Person::join(&app, &bootstrap, "alice").await;
    let bob = Person::join(&app, &alice.invite().await, "bob").await;
    let carol = Person::join(&app, &alice.invite().await, "carol").await;

    let (status, _) = bob
        .call("POST", "/api/rooms", Some(json!({ "name": "  " })))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let created = bob
        .ok("POST", "/api/rooms", Some(json!({ "name": "Raid night" })))
        .await;
    let id = created["chat"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["chat"]["kind"], "room");
    // The creating device starts the room's keys.
    assert_eq!(created["chat"]["joined"], true);

    // Everyone sees the room, members or not.
    let list = carol.ok("GET", "/api/rooms", None).await;
    let listed = room(&list, &id);
    assert_eq!(listed["name"], "Raid night");
    assert_eq!(listed["member"], false);
    assert_eq!(listed["member_ids"], json!([bob.user_id]));
    // Not being a member, Carol can't read or post in it.
    let events = format!("/api/chats/{id}/events");
    assert_eq!(carol.call("GET", &events, None).await.0, StatusCode::NOT_FOUND);

    // Carol joins on her own. Bob's device has to hand her device the keys,
    // so until then she is a member whose device has not joined.
    let joined = carol.ok("POST", &format!("/api/rooms/{id}/join"), None).await;
    assert_eq!(joined["start"], false);
    assert_eq!(joined["chat"]["joined"], false);
    assert_eq!(carol.call("GET", &events, None).await.0, StatusCode::CONFLICT);
    let list = carol.ok("GET", "/api/rooms", None).await;
    assert_eq!(room(&list, &id)["member"], true);
    let chats = carol.ok("GET", "/api/chats", None).await;
    assert_eq!(chats[0]["kind"], "room");
    // Joining twice changes nothing.
    let again = carol.ok("POST", &format!("/api/rooms/{id}/join"), None).await;
    assert_eq!(again["start"], false);

    // Nobody can be thrown out of a room (they could walk straight back in);
    // people leave on their own.
    let (status, _) = bob
        .call(
            "DELETE",
            &format!("/api/chats/{id}/members/{}", carol.user_id),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    carol
        .ok(
            "DELETE",
            &format!("/api/chats/{id}/members/{}", carol.user_id),
            None,
        )
        .await;
    let list = alice.ok("GET", "/api/rooms", None).await;
    assert_eq!(room(&list, &id)["member_ids"], json!([bob.user_id]));

    // The rooms list is for rooms: a group chat cannot be joined this way.
    let group = alice
        .ok(
            "POST",
            "/api/chats",
            Some(json!({ "kind": "group", "name": "private", "member_ids": [bob.user_id] })),
        )
        .await["chat"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, _) = carol
        .call("POST", &format!("/api/rooms/{group}/join"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        !carol
            .ok("GET", "/api/rooms", None)
            .await
            .to_string()
            .contains(&group)
    );
}

#[tokio::test]
async fn a_room_outlives_its_members() {
    let (app, state, _dir) = server(true).await;
    let bootstrap = auth::bootstrap_invite(&state.db).await.unwrap().unwrap();
    let alice = Person::join(&app, &bootstrap, "alice").await;
    let bob = Person::join(&app, &alice.invite().await, "bob").await;

    let id = alice
        .ok("POST", "/api/rooms", Some(json!({ "name": "Lounge" })))
        .await["chat"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    alice
        .ok(
            "POST",
            &format!("/api/chats/{id}/messages"),
            Some(json!({ "epoch": 0, "data": "c2VjcmV0", "client_id": "m1" })),
        )
        .await;
    alice
        .ok(
            "DELETE",
            &format!("/api/chats/{id}/members/{}", alice.user_id),
            None,
        )
        .await;

    // Still listed, with nobody in it.
    let list = bob.ok("GET", "/api/rooms", None).await;
    assert_eq!(room(&list, &id)["member_ids"], json!([]));

    // The next person in starts the room's keys afresh, with no history left over.
    let joined = bob.ok("POST", &format!("/api/rooms/{id}/join"), None).await;
    assert_eq!(joined["start"], true);
    assert_eq!(joined["chat"]["joined"], true);
    assert_eq!(joined["chat"]["epoch"], 0);
    assert_eq!(joined["chat"]["last_seq"], 0);
    let events = bob.ok("GET", &format!("/api/chats/{id}/events"), None).await;
    assert_eq!(events["events"], json!([]));
}

#[tokio::test]
async fn only_the_creator_or_an_admin_can_rename_or_delete() {
    let (app, state, _dir) = server(true).await;
    let bootstrap = auth::bootstrap_invite(&state.db).await.unwrap().unwrap();
    let admin = Person::join(&app, &bootstrap, "admin").await;
    let bob = Person::join(&app, &admin.invite().await, "bob").await;
    let carol = Person::join(&app, &admin.invite().await, "carol").await;

    let id = bob
        .ok("POST", "/api/rooms", Some(json!({ "name": "Games" })))
        .await["chat"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    carol.ok("POST", &format!("/api/rooms/{id}/join"), None).await;

    let rename = format!("/api/rooms/{id}/name");
    assert_eq!(
        carol
            .call("PUT", &rename, Some(json!({ "name": "Mine" })))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    bob.ok("PUT", &rename, Some(json!({ "name": "Game night" })))
        .await;
    assert_eq!(
        room(&carol.ok("GET", "/api/rooms", None).await, &id)["name"],
        "Game night"
    );

    assert_eq!(
        carol.call("DELETE", &format!("/api/rooms/{id}"), None).await.0,
        StatusCode::FORBIDDEN
    );
    admin.ok("DELETE", &format!("/api/rooms/{id}"), None).await;
    assert_eq!(carol.ok("GET", "/api/rooms", None).await, json!([]));
    assert_eq!(carol.ok("GET", "/api/chats", None).await, json!([]));
}

#[tokio::test]
async fn everyone_sees_who_is_in_a_rooms_voice_channel() {
    let (app, state, _dir) = server(true).await;
    let bootstrap = auth::bootstrap_invite(&state.db).await.unwrap().unwrap();
    let alice = Person::join(&app, &bootstrap, "alice").await;
    let bob = Person::join(&app, &alice.invite().await, "bob").await;

    let id = alice
        .ok("POST", "/api/rooms", Some(json!({ "name": "Voice" })))
        .await["chat"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    hold_keys(&state, &id, alice.device_id).await;
    alice
        .ok("POST", &format!("/api/chats/{id}/call/join"), None)
        .await;

    // Bob is not in the room, but sees who is talking in it.
    let list = bob.ok("GET", "/api/rooms", None).await;
    assert_eq!(room(&list, &id)["voice"], json!([alice.user_id]));
    // He still can't get into the call without joining the room.
    assert_eq!(
        bob.call("POST", &format!("/api/chats/{id}/call/join"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // Leaving the room takes Alice out of its voice channel too.
    alice
        .ok(
            "DELETE",
            &format!("/api/chats/{id}/members/{}", alice.user_id),
            None,
        )
        .await;
    let list = bob.ok("GET", "/api/rooms", None).await;
    assert_eq!(room(&list, &id)["voice"], json!([]));
}
