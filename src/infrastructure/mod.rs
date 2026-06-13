//! Infrastructure layer: concrete implementations of ports (adapters).

pub mod cloud;
pub mod credentials;
pub mod delta;
pub mod intelligence;
pub mod mobile;
pub mod p2p;
pub mod persistence;
pub mod security;
pub mod transfer;
pub mod transport;
pub mod transport_router;

pub use cloud::{RcloneBridgeClient, RcloneCloudTransport};
pub use credentials::EnvCredentialVault;
pub use delta::ParallelBlake3Hasher;
pub use mobile::MobileP2PBackground;
pub use intelligence::DefaultIntelligenceEngine;
pub use p2p::IrohP2PTransport;
pub use persistence::InMemoryJobRepository;
pub use security::{AuditEvent, ClientSideEncryption, MfaHook, RbacEnforcer, RustlsConfig, TamperEvidentAuditLogger};
pub use transfer::LocalDeltaTransport;
pub use transport::NoopTransport;
pub use transport_router::TransportRouter;
