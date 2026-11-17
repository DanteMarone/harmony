//! Harmony server: a small, private, self-hosted chat backend.

pub mod auth;
pub mod config;
pub mod db;
pub mod error;

use std::sync::Arc;

use axum::{
    Json, Router,
    http::{HeaderValue, header},
    routing::get,
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};

use crate::{auth::LoginLimiter, config::Config};

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: Arc<Config>,
    pub login_limiter: Arc<LoginLimiter>,
    pub dummy_password_hash: String,
}

impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        Ok(Self {
            db: db::open(&config.data_dir).await?,
            config: Arc::new(config),
            login_limiter: Arc::new(LoginLimiter::default()),
            dummy_password_hash: auth::make_dummy_hash().await?,
        })
    }
}

/// Everything the server answers: the JSON API under `/api`, and the built web
/// app for every other path.
pub fn app(state: AppState) -> Router {
    let api = Router::new().route("/health", get(health)).merge(auth::routes());

    let web_dir = state.config.web_dir.clone();
    let web = ServeDir::new(&web_dir).fallback(ServeFile::new(web_dir.join("index.html")));

    Router::new()
        .nest("/api", api)
        .fallback_service(web)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        // Only our own scripts may run: this is what keeps the encryption code
        // from being tampered with by injected content.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
                 img-src 'self' blob: data:; media-src 'self' blob:; connect-src 'self' wss:; \
                 frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            ),
        ))
        // Request logging records method, path and status only: never bodies or cookies.
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}
