//! Rustls TLS 1.3 + PFS config (Section 13).
//! For daemon API exposure (future REST/gRPC/WS in Module 06) and any controlled paths.
//!
//! Updated for rustls 0.23: `Certificate`/`PrivateKey` became `pki_types::CertificateDer`/
//! `PrivateKeyDer`, and `ConfigBuilder::with_safe_defaults()` was removed (safe defaults
//! are now implicit). The crypto provider is the process default (ring, per Cargo features).

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, ServerConfig};
use std::sync::Arc;

pub struct RustlsConfig;

impl RustlsConfig {
    pub fn server_config(
        cert_chain: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) -> Result<Arc<ServerConfig>, rustls::Error> {
        let mut cfg = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)?;
        cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()]; // for gRPC/HTTP2
        Ok(Arc::new(cfg))
    }

    // Client config for outgoing (e.g. to rclone bridge or remote daemon).
    pub fn client_config() -> Arc<ClientConfig> {
        let cfg = ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty()) // or load system roots
            .with_no_client_auth();
        Arc::new(cfg)
    }
}
