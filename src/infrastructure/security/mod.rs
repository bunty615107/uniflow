//! Module 05: Security Layer (baked in, not bolted on).
//!
//! Pluggable components for TLS (rustls), client-side encryption (AES-256-GCM + ChaCha20),
//! tamper-evident audit (blake3 chain), and RBAC/MFA hooks.
//!
//! Integrates with CredentialVault (enhanced for zero-knowledge), all transports,
//! JobService, and the daemon. Zero-knowledge mode ensures the daemon sees only ciphertext.

pub mod encryption;
pub mod audit;
pub mod access_control;
pub mod tls;

pub use encryption::ClientSideEncryption;
pub use audit::{TamperEvidentAuditLogger, AuditEvent};
pub use access_control::{RbacEnforcer, MfaHook};
pub use tls::RustlsConfig;