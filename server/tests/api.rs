//! End-to-end tests of the HTTP API against a throwaway database.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use harmony_server::{AppState, app, auth, config::Config};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

struct TestServer {
    app: Router,
    state: AppState,
    _dir: tempfile::TempDir,
}

async fn server() -> TestServer {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.path().join("data"),
        web_dir: dir.path().join("web"),
        cookie_secure: true,
        livekit: None,
    };
    let state = AppState::new(config).await.unwrap();
    TestServer {
        app: app(state.clone()),
        state,
        _dir: dir,
    }
}

/// Sends a request and returns (status, session cookie if one was set, JSON body).
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Option<String>, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    let request = match body {
        Some(json) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_string()))
            .unwrap(),
        None => request.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let set_cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, set_cookie, json)
}

fn cookie_pair(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap().to_owned()
}

async fn register(app: &Router, invite: &str, username: &str) -> (StatusCode, Option<String>, Value) {
    call(
        app,
        "POST",
        "/api/auth/register",
        None,
        Some(json!({ "invite": invite, "username": username, "password": "correct horse battery" })),
    )
    .await
}

#[tokio::test]
async fn health_is_public() {
    let s = server().await;
    let (status, _, body) = call(&s.app, "GET", "/api/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn full_invite_flow() {
    let s = server().await;

    // Nobody is signed in.
    let (status, _, _) = call(&s.app, "GET", "/api/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A fresh server hands out one admin invite.
    let bootstrap = auth::bootstrap_invite(&s.state.db)
        .await
        .unwrap()
        .expect("bootstrap invite");

    // Sign-up without a valid invite is refused.
    let (status, _, _) = register(&s.app, "not-a-real-invite", "mallory").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The admin signs up with the bootstrap invite.
    let (status, set_cookie, body) = register(&s.app, &bootstrap, "dante").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["is_admin"], true);
    let set_cookie = set_cookie.unwrap();
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("Secure"));
    assert!(set_cookie.contains("SameSite=Strict"));
    let admin = cookie_pair(&set_cookie);

    // The invite cannot be used twice, and no new bootstrap invite appears.
    let (status, _, _) = register(&s.app, &bootstrap, "mallory").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(auth::bootstrap_invite(&s.state.db).await.unwrap().is_none());

    // The admin invites a friend.
    let (status, _, invite) = call(&s.app, "POST", "/api/invites", Some(&admin), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let code = invite["code"].as_str().unwrap().to_owned();

    let (status, set_cookie, body) = register(&s.app, &code, "friend").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["is_admin"], false);
    let friend = cookie_pair(&set_cookie.unwrap());

    // Usernames are unique, ignoring case.
    let (_, _, invite2) = call(&s.app, "POST", "/api/invites", Some(&admin), None).await;
    let (status, _, _) = register(&s.app, invite2["code"].as_str().unwrap(), "FRIEND").await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Friends cannot manage invites.
    let (status, _, _) = call(&s.app, "POST", "/api/invites", Some(&friend), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(&s.app, "GET", "/api/invites", Some(&friend), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The admin sees who used which invite, and can revoke an unused one.
    let (status, _, list) = call(&s.app, "GET", "/api/invites", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 3);
    assert!(
        list.to_string().find(&code).is_none(),
        "invite codes must never be listed"
    );
    let unused = invite2["id"].as_i64().unwrap();
    let (status, _, _) = call(
        &s.app,
        "DELETE",
        &format!("/api/invites/{unused}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = register(&s.app, invite2["code"].as_str().unwrap(), "late").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Log out, then log back in.
    let (status, _, _) = call(&s.app, "POST", "/api/auth/logout", Some(&friend), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = call(&s.app, "GET", "/api/me", Some(&friend), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _, _) = call(
        &s.app,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "username": "friend", "password": "wrong password!" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, set_cookie, _) = call(
        &s.app,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "username": "Friend", "password": "correct horse battery" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let friend = cookie_pair(&set_cookie.unwrap());
    let (status, _, body) = call(&s.app, "GET", "/api/me", Some(&friend), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["username"], "friend");
    assert!(body.get("password_hash").is_none());
}

#[tokio::test]
async fn weak_input_is_rejected() {
    let s = server().await;
    let invite = auth::bootstrap_invite(&s.state.db).await.unwrap().unwrap();
    for (username, password) in [
        ("ab", "long enough password"),
        ("has space", "long enough password"),
        ("fine", "short"),
    ] {
        let (status, _, _) = call(
            &s.app,
            "POST",
            "/api/auth/register",
            None,
            Some(json!({ "invite": invite, "username": username, "password": password })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{username} / {password}");
    }
}

#[tokio::test]
async fn repeated_failed_logins_are_throttled() {
    let s = server().await;
    let mut last = StatusCode::OK;
    for _ in 0..10 {
        let (status, _, _) = call(
            &s.app,
            "POST",
            "/api/auth/login",
            None,
            Some(json!({ "username": "nobody", "password": "guess guess guess" })),
        )
        .await;
        last = status;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn secrets_are_stored_hashed() {
    let s = server().await;
    let invite = auth::bootstrap_invite(&s.state.db).await.unwrap().unwrap();
    let (_, set_cookie, _) = register(&s.app, &invite, "dante").await;
    let token = cookie_pair(&set_cookie.unwrap())
        .split('=')
        .nth(1)
        .unwrap()
        .to_owned();

    let (hash,): (String,) = sqlx::query_as("SELECT password_hash FROM users")
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert!(hash.starts_with("$argon2id$"));
    let (stored,): (String,) = sqlx::query_as("SELECT token_hash FROM sessions")
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert_ne!(stored, token);
    let (stored,): (String,) = sqlx::query_as("SELECT code_hash FROM invites")
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert_ne!(stored, invite);
}
