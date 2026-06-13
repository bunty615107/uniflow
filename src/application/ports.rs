//! Ports (interfaces) for the application layer.
//!
//! These are the contracts that the connection-agnostic engine depends on.
//! Concrete implementations live in `infrastructure`.

use crate::domain::{FileManifest, FileSignature, Job, JobId};
use crate::error::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Repository port for job persistence (P0: in-memory + snapshot; later RocksDB).
#[async_trait]
pub trait JobRepository: Send + Sync {
    async fn save(&self, job: &Job) -> Result<()>;
    async fn load(&self, id: JobId) -> Result<Job>;
    async fn list(&self) -> Result<Vec<Job>>;
    async fn remove(&self, id: JobId) -> Result<()>;
    async fn snapshot(&self) -> Result<()> {
        Ok(())
    }
}

/// The pluggable transport port.
/// This is the heart of the "connection-agnostic" design (explicit Source + Destination + Mode).
///
/// Future implementations will provide real behavior for different transports
/// without the core engine or domain model knowing the details.
#[async_trait]
pub trait Transport: Send + Sync {
    fn name(&self) -> &'static str;

    /// Execute the transfer described by the job (or simulate in P0).
    async fn execute(&self, job: &Job) -> Result<TransferReport>;

    /// Optional probe for future intelligence/routing layer.
    async fn probe(&self, _source: &crate::domain::Endpoint, _dest: &crate::domain::Endpoint) -> Option<ProbeResult> {
        None
    }
}

#[derive(Clone, Debug)]
pub struct TransferReport {
    pub bytes_transferred: u64,
    pub duration_ms: u64,
    pub integrity_hash: Option<String>,
    pub chunks: u32,
}

#[derive(Clone, Debug)]
pub struct ProbeResult {
    pub reachable: bool,
    pub rtt_ms: Option<u32>,
    pub bandwidth_mbps: Option<u32>,
}

/// === Phase 1 Delta Engine Ports (Section 13 / Module 02) ===

/// Generates block-level signatures for delta computation (librsync weak + strong).
pub trait SignatureGenerator: Send + Sync {
    fn generate_signature(&self, path: &std::path::Path) -> Result<FileSignature>;
}

/// Core delta engine (signature → delta → patch).
pub trait DeltaEngine: Send + Sync {
    fn create_delta(&self, source: &std::path::Path, sig: &FileSignature) -> Result<Vec<DeltaChunk>>;
    /// Apply delta, returning bytes written. Supports resume from byte offset.
    fn apply_delta(&self, dest: &std::path::Path, delta: &[DeltaChunk], resume_from: u64) -> Result<u64>;
}

/// Multithreaded + SIMD content hasher (BLAKE3) for integrity and dedup.
pub trait ContentHasher: Send + Sync {
    /// Parallel hash of entire file (root hash).
    fn hash_file_parallel(&self, path: &std::path::Path) -> Result<[u8; 32]>;
    /// Parallel per-block hashing (produces manifest for delta + verification).
    fn hash_blocks_parallel(&self, path: &std::path::Path, block_size: u32) -> Result<FileManifest>;
}

/// === Module 01: Universal Cloud Connector - Credential Vault ===
/// Unified credential management for all 70+ backends.
/// Jobs reference credentials by opaque string (e.g. "my-s3-prod" or env key).
/// Sensitive data is resolved at execution time and passed to the Rclone bridge.
#[derive(Clone, Debug)]
pub struct CloudCredential {
    pub provider: String,                    // "s3", "gcs", "azureblob", "dropbox", ...
    pub config: std::collections::HashMap<String, String>, // rclone-style keys (access_key_id, etc.)
}

pub trait CredentialVault: Send + Sync {
    /// Resolve a reference to a CloudCredential.
    /// For zero-knowledge (Module 05): returns client-encrypted material or challenges for MFA.
    fn resolve(&self, reference: &str) -> Result<CloudCredential>;

    /// MFA challenge hook. Enterprise impl prompts user or calls external IdP.
    fn mfa_challenge(&self, reference: &str, action: &str) -> Result<String> {
        let _ = (reference, action);
        Ok("mfa-bypass-for-demo".into())
    }

    /// Optional: store a credential (for CLI / UI flows).
    fn store(&self, name: &str, cred: CloudCredential) -> Result<()> {
        let _ = (name, cred);
        Err(crate::error::UniFlowError::Config(
            "Credential storage not implemented in this vault".into(),
        ))
    }
}

/// === Module 03: Adaptive P2P Network Ports (Section 13) ===
/// These are optional / advanced ports. The core Transport trait remains the
/// primary way to execute P2P jobs. This separation keeps the engine clean.

/// Peer discovery abstraction (LAN mDNS, DHT, etc.).
pub trait PeerDiscovery: Send + Sync {
    /// Discover nearby or known peers.
    fn discover_peers(&self) -> Result<Vec<crate::domain::P2PDiscoveryInfo>>;
}

/// NAT traversal / relay control (for explicit air-gap vs relay policy).
pub trait NatTraversal: Send + Sync {
    /// Return current connectivity status for a peer.
    fn probe_connectivity(&self, peer: &crate::domain::PeerId) -> Option<crate::application::ports::ProbeResult>;
}

/// === Module 04: Intelligence & Optimiser (Section 6 / 13) ===
/// Pluggable profiling and auto-tuning layer.
/// All detectors/probes/optimizers are traits for easy extension.
/// Every decision produces an `explanation` string that is logged and persisted.

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkProbeResult {
    pub rtt_ms: f64,
    pub bandwidth_mbps: f64,
    pub jitter_ms: f64,
    pub explanation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct HardwareProfile {
    pub cpu_cores: u32,
    pub cpu_features: Vec<String>, // e.g. "avx2", "qat", "cuda", "apple_silicon"
    pub ram_gb: f64,
    pub disk_iops: Option<u32>,
    pub accelerators: Vec<String>, // "intel_qat", "nvidia_cuda", "apple_unified"
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TuningDecision {
    pub threads: usize,
    pub chunk_size: u64,
    pub compression_level: Option<u8>,
    pub max_bps: Option<u64>,           // adaptive throttle
    pub start_at: Option<chrono::DateTime<chrono::Utc>>, // off-peak scheduling
    pub explanation: String,            // **required** for auditability
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfilingResult {
    pub network: Option<NetworkProbeResult>,
    pub hardware: HardwareProfile,
    pub decision: TuningDecision,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Pre-transfer network probe (RTT, bandwidth, jitter).
pub trait NetworkProbe: Send + Sync {
    fn probe(&self, source: &crate::domain::Endpoint, dest: &crate::domain::Endpoint) -> Result<NetworkProbeResult>;
}

/// Pluggable hardware detector (CPU/RAM/Disk + accelerators).
pub trait HardwareDetector: Send + Sync {
    fn name(&self) -> &'static str;
    fn detect(&self) -> Option<HardwareProfile>;
    fn explain(&self) -> String;
}

/// Core optimizer: turns profiles into explainable TuningDecision.
pub trait Optimizer: Send + Sync {
    fn optimize(&self, job: &Job, network: Option<&NetworkProbeResult>, hardware: &HardwareProfile) -> TuningDecision;
}

/// Orchestrator for the full intelligence pipeline.
pub trait IntelligenceEngine: Send + Sync {
    fn profile_and_tune(&self, job: &mut Job) -> Result<ProfilingResult>;
}
