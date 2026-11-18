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
    /// How much the server holds for photos and files in transit.
    pub attachments: AttachmentLimits,
}

/// Limits on the encrypted photos and files the server holds while it hands
/// them on. It is a relay, not storage: these keep a small server's disk safe.
#[derive(Clone, Debug)]
pub struct AttachmentLimits {
    /// Largest single file, in bytes (of ciphertext).
    pub max_file_bytes: u64,
    /// Most the server holds at once, for everyone together.
    pub max_total_bytes: u64,
    /// Most one person can have waiting at once.
    pub max_user_bytes: u64,
    /// A file nobody has picked up is deleted after this many seconds.
    pub keep_secs: i64,
}

const MB: u64 = 1024 * 1024;

impl Default for AttachmentLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 25 * MB,
            max_total_bytes: 1024 * MB,
            max_user_bytes: 300 * MB,
            keep_secs: 7 * 24 * 60 * 60,
        }
    }
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
            attachments: attachments_from_env()?,
        })
    }
}

fn attachments_from_env() -> anyhow::Result<AttachmentLimits> {
    let defaults = AttachmentLimits::default();
    let number = |name: &str| -> anyhow::Result<Option<u64>> {
        match env::var(name).ok().filter(|v| !v.trim().is_empty()) {
            None => Ok(None),
            Some(v) => {
                let n: u64 = v
                    .trim()
                    .parse()
                    .map_err(|_| anyhow::anyhow!("{name} must be a whole number"))?;
                anyhow::ensure!(n > 0, "{name} must be more than 0");
                Ok(Some(n))
            }
        }
    };
    let limits = AttachmentLimits {
        max_file_bytes: number("HARMONY_FILE_MAX_MB")?.map_or(defaults.max_file_bytes, |n| n * MB),
        max_total_bytes: number("HARMONY_FILES_TOTAL_MB")?.map_or(defaults.max_total_bytes, |n| n * MB),
        max_user_bytes: number("HARMONY_FILES_PER_USER_MB")?.map_or(defaults.max_user_bytes, |n| n * MB),
        keep_secs: number("HARMONY_FILES_KEEP_DAYS")?.map_or(defaults.keep_secs, |n| n as i64 * 24 * 60 * 60),
    };
    anyhow::ensure!(
        limits.max_file_bytes <= 100 * MB,
        "HARMONY_FILE_MAX_MB can be at most 100: each upload is held in memory while it is checked"
    );
    Ok(limits)
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
