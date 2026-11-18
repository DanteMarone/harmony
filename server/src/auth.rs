//! Accounts: invite-only sign-up, login, sessions and invite management.
//!
//! Privacy rules followed here:
//! - A username and password are all we ask for. No email, no phone, no ID.
//! - Passwords are hashed with Argon2id. Invite codes and session tokens are
//!   random, and only their SHA-256 hashes are stored, so a leaked database
//!   cannot be used to sign in or to redeem invites.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::{
    Json, Router,
    extract::{FromRequestParts, Path, State},
    http::{StatusCode, request::Parts},
    routing::{delete, get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};

const SESSION_COOKIE: &str = "harmony_session";
const SESSION_TTL_SECS: i64 = 30 * 24 * 3600;
const INVITE_TTL_SECS: i64 = 7 * 24 * 3600;
const MAX_FAILED_LOGINS: u32 = 8;
const FAILED_LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
        .route("/invites", get(list_invites).post(create_invite))
        .route("/invites/{id}", delete(revoke_invite))
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 32 random bytes from the operating system, hex encoded.
pub fn random_token() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no system randomness: {e}"))?;
    Ok(hex::encode(bytes))
}

pub fn sha256_hex(input: &str) -> String {
    hex::encode(Sha256::digest(input.as_bytes()))
}

async fn hash_password(password: String) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
            .map_err(|e| anyhow::anyhow!("password hashing failed: {e}"))
    })
    .await?
}

async fn verify_password(password: String, stored_hash: String) -> anyhow::Result<bool> {
    tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&stored_hash)
            .map_err(|e| anyhow::anyhow!("stored password hash is invalid: {e}"))?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    })
    .await?
}

fn valid_username(name: &str) -> bool {
    (3..=24).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// Counts failed logins per username to slow down password guessing.
#[derive(Default)]
pub struct LoginLimiter(Mutex<HashMap<String, (u32, Instant)>>);

impl LoginLimiter {
    fn blocked(&self, key: &str) -> bool {
        let mut map = self.0.lock().unwrap();
        match map.get(key) {
            Some((_, since)) if since.elapsed() > FAILED_LOGIN_WINDOW => {
                map.remove(key);
                false
            }
            Some((count, _)) => *count >= MAX_FAILED_LOGINS,
            None => false,
        }
    }

    fn record_failure(&self, key: &str) {
        let mut map = self.0.lock().unwrap();
        // Keep the map from growing without bound if someone sprays usernames.
        if map.len() > 10_000 {
            map.retain(|_, (_, since)| since.elapsed() <= FAILED_LOGIN_WINDOW);
        }
        let entry = map.entry(key.to_owned()).or_insert((0, Instant::now()));
        entry.0 += 1;
    }

    fn clear(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

// ---------------------------------------------------------------------------
// The signed-in user, available to any handler as an extractor
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
    /// The login session this request came in on (0 right after logging in).
    #[serde(skip)]
    pub session_id: i64,
}

impl FromRequestParts<AppState> for User {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar
            .get(SESSION_COOKIE)
            .ok_or(ApiError::Unauthorized)?
            .value()
            .to_owned();
        sqlx::query_as::<_, User>(
            "SELECT u.id, u.username, u.display_name, u.is_admin, s.id AS session_id
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ? AND s.expires_at > ?",
        )
        .bind(sha256_hex(&token))
        .bind(now())
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::Unauthorized)
    }
}

async fn start_session(state: &AppState, jar: CookieJar, user_id: i64) -> ApiResult<CookieJar> {
    let token = random_token()?;
    let created = now();
    sqlx::query("INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)")
        .bind(sha256_hex(&token))
        .bind(user_id)
        .bind(created)
        .bind(created + SESSION_TTL_SECS)
        .execute(&state.db)
        .await?;
    // Opportunistic cleanup of expired sessions.
    sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(created)
        .execute(&state.db)
        .await?;

    let cookie = Cookie::build((SESSION_COOKIE, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(state.config.cookie_secure)
        .max_age(time_duration(SESSION_TTL_SECS))
        .build();
    Ok(jar.add(cookie))
}

fn time_duration(secs: i64) -> time::Duration {
    time::Duration::seconds(secs)
}

async fn fetch_user(db: &SqlitePool, id: i64) -> ApiResult<User> {
    Ok(sqlx::query_as::<_, User>(
        "SELECT id, username, display_name, is_admin, 0 AS session_id FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_one(db)
    .await?)
}

// ---------------------------------------------------------------------------
// Sign-up, login, logout
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RegisterBody {
    invite: String,
    username: String,
    password: String,
    display_name: Option<String>,
}

async fn register(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<RegisterBody>,
) -> ApiResult<(StatusCode, CookieJar, Json<User>)> {
    let username = body.username.trim().to_owned();
    if !valid_username(&username) {
        return Err(ApiError::BadRequest(
            "username must be 3-24 characters: letters, digits, dot, dash or underscore",
        ));
    }
    if body.password.chars().count() < 10 || body.password.len() > 512 {
        return Err(ApiError::BadRequest("password must be at least 10 characters"));
    }
    let display_name = body
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&username);
    if display_name.chars().count() > 48 {
        return Err(ApiError::BadRequest("display name must be at most 48 characters"));
    }
    let display_name = display_name.to_owned();

    // Hash before opening the transaction: it is slow on purpose.
    let password_hash = hash_password(body.password).await?;
    let created = now();

    let mut tx = state.db.begin().await?;
    let invite: Option<(i64, bool)> = sqlx::query_as(
        "SELECT id, grants_admin FROM invites WHERE code_hash = ? AND used_at IS NULL AND expires_at > ?",
    )
    .bind(sha256_hex(body.invite.trim()))
    .bind(created)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((invite_id, grants_admin)) = invite else {
        return Err(ApiError::BadRequest("invite is invalid, used or expired"));
    };

    let inserted = sqlx::query(
        "INSERT INTO users (username, display_name, password_hash, is_admin, created_at)
         VALUES (?, ?, ?, ?, ?) ON CONFLICT(username) DO NOTHING",
    )
    .bind(&username)
    .bind(&display_name)
    .bind(&password_hash)
    .bind(grants_admin)
    .bind(created)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(ApiError::Conflict("that username is taken"));
    }
    let user_id = inserted.last_insert_rowid();

    // `used_at IS NULL` again so two people racing on one invite cannot both win.
    let claimed = sqlx::query("UPDATE invites SET used_by = ?, used_at = ? WHERE id = ? AND used_at IS NULL")
        .bind(user_id)
        .bind(created)
        .bind(invite_id)
        .execute(&mut *tx)
        .await?;
    if claimed.rows_affected() == 0 {
        return Err(ApiError::BadRequest("invite is invalid, used or expired"));
    }
    tx.commit().await?;

    let jar = start_session(&state, jar, user_id).await?;
    Ok((
        StatusCode::CREATED,
        jar,
        Json(fetch_user(&state.db, user_id).await?),
    ))
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<LoginBody>,
) -> ApiResult<(CookieJar, Json<User>)> {
    let key = body.username.trim().to_lowercase();
    if key.len() > 64 || body.password.len() > 512 {
        return Err(ApiError::Unauthorized);
    }
    if state.login_limiter.blocked(&key) {
        return Err(ApiError::TooManyRequests);
    }

    let row: Option<(i64, String)> = sqlx::query_as("SELECT id, password_hash FROM users WHERE username = ?")
        .bind(&key)
        .fetch_optional(&state.db)
        .await?;

    // Always do the expensive check, so response time does not reveal
    // whether a username exists.
    let (user_id, stored_hash) = match row {
        Some((id, hash)) => (Some(id), hash),
        None => (None, state.dummy_password_hash.clone()),
    };
    let password_ok = verify_password(body.password, stored_hash).await?;

    match user_id {
        Some(id) if password_ok => {
            state.login_limiter.clear(&key);
            let jar = start_session(&state, jar, id).await?;
            Ok((jar, Json(fetch_user(&state.db, id).await?)))
        }
        _ => {
            state.login_limiter.record_failure(&key);
            Err(ApiError::BadRequest("wrong username or password"))
        }
    }
}

async fn logout(State(state): State<AppState>, jar: CookieJar) -> ApiResult<(CookieJar, StatusCode)> {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(sha256_hex(cookie.value()))
            .execute(&state.db)
            .await?;
    }
    Ok((
        jar.remove(Cookie::build(SESSION_COOKIE).path("/").build()),
        StatusCode::NO_CONTENT,
    ))
}

async fn me(user: User) -> Json<User> {
    Json(user)
}

// ---------------------------------------------------------------------------
// Invites (admin only)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct NewInvite {
    id: i64,
    /// Shown once. Only its hash is stored, so it cannot be recovered later.
    code: String,
    expires_at: i64,
}

#[derive(Serialize, sqlx::FromRow)]
struct InviteInfo {
    id: i64,
    created_at: i64,
    expires_at: i64,
    used_at: Option<i64>,
    used_by: Option<String>,
}

pub async fn insert_invite(
    db: &SqlitePool,
    created_by: Option<i64>,
    grants_admin: bool,
) -> anyhow::Result<(i64, String, i64)> {
    let code = random_token()?;
    let created = now();
    let expires = created + INVITE_TTL_SECS;
    let result = sqlx::query(
        "INSERT INTO invites (code_hash, created_by, grants_admin, created_at, expires_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(sha256_hex(&code))
    .bind(created_by)
    .bind(grants_admin)
    .bind(created)
    .bind(expires)
    .execute(db)
    .await?;
    Ok((result.last_insert_rowid(), code, expires))
}

async fn create_invite(
    State(state): State<AppState>,
    user: User,
) -> ApiResult<(StatusCode, Json<NewInvite>)> {
    if !user.is_admin {
        return Err(ApiError::Forbidden);
    }
    let (id, code, expires_at) = insert_invite(&state.db, Some(user.id), false).await?;
    Ok((StatusCode::CREATED, Json(NewInvite { id, code, expires_at })))
}

async fn list_invites(State(state): State<AppState>, user: User) -> ApiResult<Json<Vec<InviteInfo>>> {
    if !user.is_admin {
        return Err(ApiError::Forbidden);
    }
    let invites = sqlx::query_as::<_, InviteInfo>(
        "SELECT i.id, i.created_at, i.expires_at, i.used_at, u.username AS used_by
         FROM invites i LEFT JOIN users u ON u.id = i.used_by
         ORDER BY i.created_at DESC, i.id DESC",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(invites))
}

async fn revoke_invite(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    if !user.is_admin {
        return Err(ApiError::Forbidden);
    }
    let result = sqlx::query("DELETE FROM invites WHERE id = ? AND used_at IS NULL")
        .bind(id)
        .execute(&state.db)
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// On a brand-new server nobody can sign up, because there is no admin to
/// invite them. So while there are no users, make one admin invite at startup.
/// It is returned to the caller to print on the console and never stored in clear.
pub async fn bootstrap_invite(db: &SqlitePool) -> anyhow::Result<Option<String>> {
    let (users,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users").fetch_one(db).await?;
    if users > 0 {
        return Ok(None);
    }
    // Replace any earlier unused bootstrap invite, so only the latest one works.
    sqlx::query("DELETE FROM invites WHERE created_by IS NULL AND used_at IS NULL")
        .execute(db)
        .await?;
    let (_, code, _) = insert_invite(db, None, true).await?;
    Ok(Some(code))
}

/// A real Argon2 hash of a random password, checked when someone tries to log
/// in with a username that does not exist.
pub async fn make_dummy_hash() -> anyhow::Result<String> {
    hash_password(random_token()?).await
}
