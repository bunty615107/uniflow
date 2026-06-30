//! Ports (interfaces) for the application layer.
//!
//! These are the contracts that the connection-agnostic engine depends on.
//! Concrete implementations live in `infrastructure`.

use crate::domain::{DeltaChunk, FileManifest, FileSignature, Job, JobId};
use crate::error::Result;
use async_trait::async_trait;

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

/// A port for dynamic transport selection and routing.
pub trait TransportSelector: Send + Sync {
    /// Routes the job, potentially profiling and tuning it (which is why it needs `&mut Job`),
    /// and returns the concrete `Transport` that should execute it.
    fn select(&self, job: &mut Job) -> std::sync::Arc<dyn Transport>;
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

// === Phase 1 Delta Engine Ports (Section 13 / Module 02) ===
///
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

// === Module 03: Adaptive P2P Network Ports (Section 13) ===
// These are optional / advanced ports. The core Transport trait remains the
// primary way to execute P2P jobs. This separation keeps the engine clean.
///
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
///
/// These value types live in `domain` (pure/serializable). They were previously
/// duplicated here, which made the port types incompatible with the domain types
/// the engines actually produce. Re-export the single source of truth instead.
pub use crate::domain::{HardwareProfile, NetworkProbeResult, ProfilingResult, TuningDecision};

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

// === Deliverable 1: Profiler & Planner (profile-first engine) ===
use crate::domain::{Endpoint, EndpointProfile, LinkProfile, PairProfile, TransferPlan};

/// Measures and caches the infrastructure profile for an endpoint pair.
///
/// Implementations detect real hardware / network / OS-FS facts and cache them
/// per endpoint pair so repeated jobs over the same route don't re-probe.
pub trait SystemProfiler: Send + Sync {
    /// Profile a single endpoint's host (CPU/RAM/storage/GPU/OS-FS).
    fn profile_endpoint(&self, endpoint: &Endpoint) -> Result<EndpointProfile>;

    /// Measure the link between two endpoints (RTT/jitter/loss/throughput/path class).
    fn profile_link(&self, source: &Endpoint, dest: &Endpoint) -> Result<LinkProfile>;

    /// Full per-pair profile, served from cache when fresh.
    fn profile_pair(&self, source: &Endpoint, dest: &Endpoint) -> Result<PairProfile>;
}

/// Turns a `PairProfile` into a concrete, explainable `TransferPlan` via a
/// documented cost model (no magic constants without rationale).
pub trait Planner: Send + Sync {
    fn plan(&self, job: &Job, profile: &PairProfile) -> TransferPlan;
}

/// Pluggable compute backend for the hot per-chunk work (hashing / compression).
///
/// There is ALWAYS a CPU implementation; GPU implementations are feature-gated and
/// optional. The engine only routes work here when the planner deems it profitable,
/// and any failure falls back to CPU — satisfying graceful degradation.
pub trait ComputeOffload: Send + Sync {
    fn name(&self) -> &'static str;
    /// True if this backend can actually run right now (device present, driver ok).
    fn is_available(&self) -> bool;
    /// BLAKE3 hash of a buffer (must be byte-identical to the CPU path).
    fn hash(&self, data: &[u8]) -> [u8; 32];
}
