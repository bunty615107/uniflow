//! Centralized configuration for UniFlow.
//!
//! All environment-variable-driven settings are collected here and validated
//! on startup. This is the single source of truth — no more scattered
//! `std::env::var(...)` calls in random modules.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use tracing::{info, warn};

/// Top-level configuration for the UniFlow daemon.
#[derive(Clone, Debug)]
pub struct UniFlowConfig {
    /// TCP bind address for the HTTP(S) server.
    pub bind_addr: SocketAddr,
    /// API key for `/api/*` authentication.
    /// When `demo_mode` is false and this is `None`, the server refuses to start.
    pub api_key: Option<String>,
    /// Path to the directory containing the Stitch retro UI files.
    pub ui_dir: PathBuf,
    /// Base directory for data files (snapshots, sled DB).
    pub data_dir: PathBuf,
    /// Path to the sandbox directory for user-controlled FS operations.
    pub sandbox_dir: PathBuf,
    /// Optional sled database path for durable persistence.
    /// When `None`, the in-memory repository (with JSON snapshot) is used.
    pub sled_path: Option<String>,
    /// TLS certificate and key paths (both required to enable HTTPS).
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    /// Optional hex-encoded 32-byte pre-shared key for P2P end-to-end AEAD.
    pub p2p_psk: Option<String>,
    /// Path for append-only audit log file. `None` = in-memory only.
    pub audit_file: Option<PathBuf>,
    /// Log format: "pretty" (default, human-readable) or "json" (structured/SIEM).
    pub log_format: LogFormat,
    /// When true, enables demo-only behaviour (auto-file creation, seed endpoint,
    /// fallback dev API key). Must be explicitly disabled for production.
    pub demo_mode: bool,
}

/// Log output format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogFormat {
    Pretty,
    Json,
}

impl UniFlowConfig {
    /// Load configuration from environment variables with sensible defaults.
    /// Call this once at startup before constructing the Daemon.
    pub fn from_env() -> Result<Self, String> {
        let port: u16 = env_parse("UNIFLOW_PORT", 7878);
        let bind_ip: IpAddr = env_parse("UNIFLOW_BIND_ADDR", IpAddr::V4(Ipv4Addr::LOCALHOST));
        let bind_addr = SocketAddr::new(bind_ip, port);

        let demo_mode = env_bool("UNIFLOW_DEMO_MODE", false);

        let api_key = std::env::var("UNIFLOW_API_KEY").ok().filter(|s| !s.is_empty());
        if api_key.is_none() && !demo_mode {
            return Err(
                "UNIFLOW_API_KEY must be set in production (or set UNIFLOW_DEMO_MODE=true for dev)"
                    .into(),
            );
        }
        if api_key.is_none() && demo_mode {
            warn!(
                "SECURITY: No UNIFLOW_API_KEY set — using hardcoded dev key. \
                 DO NOT deploy this configuration to production."
            );
        }

        let ui_dir = env_path("UNIFLOW_UI_DIR", PathBuf::from("./ui"));
        let data_dir = env_path("UNIFLOW_DATA_DIR", PathBuf::from("./data"));
        let sandbox_dir = env_path(
            "UNIFLOW_SANDBOX_DIR",
            std::env::temp_dir().join("uniflow_sandbox"),
        );

        let sled_path = std::env::var("UNIFLOW_SLED_PATH")
            .ok()
            .filter(|s| !s.is_empty());

        let tls_cert = std::env::var("UNIFLOW_TLS_CERT").ok().map(PathBuf::from);
        let tls_key = std::env::var("UNIFLOW_TLS_KEY").ok().map(PathBuf::from);
        if tls_cert.is_some() != tls_key.is_some() {
            return Err(
                "Both UNIFLOW_TLS_CERT and UNIFLOW_TLS_KEY must be set together (or neither)"
                    .into(),
            );
        }

        let p2p_psk = std::env::var("UNIFLOW_P2P_PSK")
            .ok()
            .filter(|s| !s.is_empty());

        let audit_file = std::env::var("UNIFLOW_AUDIT_FILE")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        let log_format = match std::env::var("UNIFLOW_LOG_FORMAT")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "json" => LogFormat::Json,
            _ => LogFormat::Pretty,
        };

        // Ensure critical directories exist.
        let _ = std::fs::create_dir_all(&data_dir);
        let _ = std::fs::create_dir_all(&sandbox_dir);

        let config = Self {
            bind_addr,
            api_key,
            ui_dir,
            data_dir,
            sandbox_dir,
            sled_path,
            tls_cert,
            tls_key,
            p2p_psk,
            audit_file,
            log_format,
            demo_mode,
        };

        info!(?config, "UniFlow configuration loaded");
        Ok(config)
    }

    /// The effective API key (configured or dev fallback in demo mode).
    pub fn effective_api_key(&self) -> &str {
        const DEV_KEY: &str = "dev-uniflow-key-12345";
        self.api_key.as_deref().unwrap_or(DEV_KEY)
    }

    /// Path to the job snapshot file (in data_dir).
    pub fn snapshot_path(&self) -> PathBuf {
        self.data_dir.join("uniflow_jobs.snapshot.json")
    }

    /// Whether TLS is configured and the cert/key files are readable.
    pub fn tls_ready(&self) -> bool {
        match (&self.tls_cert, &self.tls_key) {
            (Some(c), Some(k)) => c.exists() && k.exists(),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn env_path(key: &str, default: PathBuf) -> PathBuf {
    std::env::var(key)
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|s| matches!(s.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(default)
}
