//! Runtime configuration. Everything comes from environment variables so that
//! nothing secret or machine-specific ever needs to live in the repository.

use std::{env, net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    /// Address to listen on. Keep it on localhost and put a reverse proxy in front.
    pub bind: SocketAddr,
    /// Folder holding the SQLite database and uploads.
    pub data_dir: PathBuf,
    /// Folder holding the built web app (web/dist).
    pub web_dir: PathBuf,
    /// Mark session cookies `Secure` (HTTPS only). Turn off only for local development.
    pub cookie_secure: bool,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = env::var("HARMONY_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
        Ok(Self {
            bind: bind.parse()?,
            data_dir: env::var("HARMONY_DATA_DIR")
                .unwrap_or_else(|_| "./data".into())
                .into(),
            web_dir: env::var("HARMONY_WEB_DIR")
                .unwrap_or_else(|_| "../web/dist".into())
                .into(),
            cookie_secure: env::var("HARMONY_COOKIE_SECURE")
                .map(|v| !matches!(v.as_str(), "0" | "false" | "no"))
                .unwrap_or(true),
        })
    }
}
