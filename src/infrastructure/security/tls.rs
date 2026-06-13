//! Rustls TLS 1.3 + PFS config (Section 13).
//! For daemon API exposure (future REST/gRPC/WS in Module 06) and any controlled paths.

use rustls::{ServerConfig, ClientConfig};
use std::sync::Arc;

pub struct RustlsConfig;

impl RustlsConfig {
    pub fn server_config(cert_chain: Vec<rustls::Certificate>, key: rustls::PrivateKey) -> Result<Arc<ServerConfig>, rustls::Error> {
        let mut cfg = ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)?;
        cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()]; // for gRPC/HTTP2
        Ok(Arc::new(cfg))
    }

    // Client config for outgoing (e.g. to rclone bridge or remote daemon).
    pub fn client_config() -> Arc<ClientConfig> {
        let mut cfg = ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(rustls::RootCertStore::empty()) // or load system roots
            .with_no_client_auth();
        Arc::new(cfg)
    }
}