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
    /// The LiveKit media server used for calls. Calls are off when unset.
    pub livekit: Option<LiveKit>,
}

/// How to reach the LiveKit server that relays call audio and video. It
/// only ever forwards media that is already end-to-end encrypted.
#[derive(Clone)]
pub struct LiveKit {
    /// Address browsers connect to, for example `wss://chat.example.com`.
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
}

impl std::fmt::Debug for LiveKit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the secret, even in debug logs.
        f.debug_struct("LiveKit")
            .field("url", &self.url)
            .field("api_key", &self.api_key)
            .finish_non_exhaustive()
    }
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
            livekit: livekit_from_env()?,
        })
    }
}

fn livekit_from_env() -> anyhow::Result<Option<LiveKit>> {
    let get = |name: &str| env::var(name).ok().filter(|v| !v.trim().is_empty());
    match (
        get("HARMONY_LIVEKIT_URL"),
        get("HARMONY_LIVEKIT_API_KEY"),
        get("HARMONY_LIVEKIT_API_SECRET"),
    ) {
        (None, None, None) => Ok(None),
        (Some(url), Some(api_key), Some(api_secret)) => {
            anyhow::ensure!(
                url.starts_with("wss://") || url.starts_with("ws://"),
                "HARMONY_LIVEKIT_URL must start with wss:// (or ws:// for local testing)"
            );
            anyhow::ensure!(
                !url.contains(|c: char| c.is_whitespace() || c.is_control() || ";,'\"".contains(c)),
                "HARMONY_LIVEKIT_URL contains characters a URL cannot"
            );
            anyhow::ensure!(
                api_secret.len() >= 32,
                "HARMONY_LIVEKIT_API_SECRET must be at least 32 characters"
            );
            Ok(Some(LiveKit {
                url: url.trim_end_matches('/').to_owned(),
                api_key,
                api_secret,
            }))
        }
        _ => anyhow::bail!(
            "set all of HARMONY_LIVEKIT_URL, HARMONY_LIVEKIT_API_KEY and HARMONY_LIVEKIT_API_SECRET, or none"
        ),
    }
}
