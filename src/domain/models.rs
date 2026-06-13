//! Core domain models for UniFlow jobs.
//!
//! A job is defined purely as Source + Destination + Mode (plus Policy, Schedule, etc.).
//! Source and Destination are first-class types (wrapping Endpoint).
//! The model is deliberately transport-agnostic and fully serializable.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub type JobId = Uuid;

/// A location that can act as either a source or destination.
/// This is the internal representation. See `Source` and `Destination` below.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Endpoint {
    Local { path: PathBuf },
    Cloud {
        provider: String,
        bucket: String,
        prefix: Option<String>,
    },
    Remote { uri: String },
    Device { device_id: String, path: String },
}

impl Endpoint {
    pub fn label(&self) -> String {
        match self {
            Endpoint::Local { path } => format!("local:{}", path.display()),
            Endpoint::Cloud { provider, bucket, prefix } => {
                let p = prefix.as_deref().unwrap_or("");
                format!("cloud:{}:{}{}", provider, bucket, p)
            }
            Endpoint::Remote { uri } => format!("remote:{}", uri),
            Endpoint::Device { device_id, path } => format!("device:{}:{}", device_id, path),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Endpoint::Local { .. } => "local",
            Endpoint::Cloud { .. } => "cloud",
            Endpoint::Remote { .. } => "remote",
            Endpoint::Device { .. } => "device",
        }
    }
}

/// Explicit Source type (connection-agnostic).
/// A job is defined as Source + Destination + Mode (per UniFlow blueprint Section 3).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Source(pub Endpoint);

impl Source {
    pub fn new(endpoint: Endpoint) -> Self {
        Self(endpoint)
    }

    pub fn label(&self) -> String {
        self.0.label()
    }

    pub fn kind(&self) -> &'static str {
        self.0.kind()
    }

    pub fn inner(&self) -> &Endpoint {
        &self.0
    }
}

impl From<Endpoint> for Source {
    fn from(e: Endpoint) -> Self {
        Self(e)
    }
}

/// Explicit Destination type (connection-agnostic).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Destination(pub Endpoint);

impl Destination {
    pub fn new(endpoint: Endpoint) -> Self {
        Self(endpoint)
    }

    pub fn label(&self) -> String {
        self.0.label()
    }

    pub fn kind(&self) -> &'static str {
        self.0.kind()
    }

    pub fn inner(&self) -> &Endpoint {
        &self.0
    }
}

/// === Phase 1 Delta Transfer Types (Module 02 / Section 13) ===

/// Rolling weak checksum + strong BLAKE3 for a block (librsync + blake3).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct BlockSignature {
    pub offset: u64,
    pub size: u32,
    /// Strong cryptographic hash (BLAKE3)
    pub blake3: [u8; 32],
    /// Weak rolling checksum from librsync (for fast delta matching)
    pub weak: u32,
}

/// Signature of an entire file for delta computation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileSignature {
    pub block_size: u32,
    pub blocks: Vec<BlockSignature>,
    pub total_size: u64,
}

/// One instruction in a delta (copy from old location or literal new data).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DeltaInstruction {
    /// Copy bytes from the old version at this offset.
    Copy { old_offset: u64, size: u32 },
    /// Literal bytes that are new/changed.
    Literal { data: Vec<u8> },
}

/// A chunk of delta instructions (for streaming / parallel application).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeltaChunk {
    pub instructions: Vec<DeltaInstruction>,
    pub source_offset: u64, // original source position for resume tracking
}

/// Manifest of the source file (used for verification and dedup).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileManifest {
    pub signature: FileSignature,
    pub root_blake3: [u8; 32], // overall content hash for quick verification
}

/// Resume / checkpoint state for byte-level resume.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ResumeState {
    pub bytes_transferred: u64,
    pub last_block_index: u64,
    pub source_manifest: Option<FileManifest>,
}

/// === Module 04: Intelligence & Optimiser (Section 6/13) ===

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
    pub cpu_features: Vec<String>,
    pub ram_gb: f64,
    pub disk_iops: Option<u32>,
    pub accelerators: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TuningDecision {
    pub threads: usize,
    pub chunk_size: u64,
    pub compression_level: Option<u8>,
    pub max_bps: Option<u64>,
    pub start_at: Option<DateTime<Utc>>,
    pub explanation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfilingResult {
    pub network: Option<NetworkProbeResult>,
    pub hardware: HardwareProfile,
    pub decision: TuningDecision,
    pub timestamp: DateTime<Utc>,
}

/// === Module 03: Adaptive P2P Network (Section 13 / Mobile modes) ===

/// Unique identifier for a P2P peer (from iroh NodeId or libp2p PeerId).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PeerId(pub [u8; 32]);  // Simplified; real iroh uses more compact

/// Information needed to discover and connect to a peer over P2P.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct P2PDiscoveryInfo {
    pub peer_id: PeerId,
    pub direct_addrs: Vec<String>,   // IP:port for LAN/direct
    pub relay_url: Option<String>,   // relay for NAT traversal fallback
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
}

/// Plan for multi-path transfer (used by P2P transport for striping or failover).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct MultiPathPlan {
    pub paths: Vec<PathInfo>,
    pub strategy: String, // "stripe", "primary+backup", "fastest-first"
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PathInfo {
    pub path_id: String,
    pub rtt_ms: Option<u32>,
    pub bandwidth_mbps: Option<u32>,
    pub is_direct: bool,   // air-gap / LAN vs relayed
}

impl From<Endpoint> for Destination {
    fn from(e: Endpoint) -> Self {
        Self(e)
    }
}

/// Transfer mode (P0 focuses on Copy and OneWaySync).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TransferMode {
    Copy,
    OneWaySync,
}

impl TransferMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransferMode::Copy => "copy",
            TransferMode::OneWaySync => "one-way-sync",
        }
    }
}

/// Policy attached to a job.
/// Security fields (Module 05) are baked in from the start.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Policy {
    pub max_retries: u32,
    pub retry_backoff_ms: u64,
    pub verify_integrity: bool,
    pub encrypt_in_transit: bool,
    pub encrypt_at_rest: bool,
    // Module 05 additions
    pub zero_knowledge: bool,           // daemon/server never sees plaintext
    pub rbac_role: Option<String>,      // e.g. "admin", "operator", "auditor"
    pub mfa_required: bool,
    pub audit_level: String,            // "none" | "standard" | "tamper_evident"
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_backoff_ms: 500,
            verify_integrity: true,
            encrypt_in_transit: true,
            encrypt_at_rest: false,
            zero_knowledge: false,
            rbac_role: None,
            mfa_required: false,
            audit_level: "standard".to_string(),
        }
    }
}

/// Scheduling / trigger information.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Schedule {
    Immediate,
    Cron(String),
    Interval { seconds: u64 },
}

/// File filters.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Filters {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

/// Job status machine.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Queued,
    Running { progress: f32, bytes_transferred: u64 },
    Paused,
    Completed { bytes: u64, duration_ms: u64 },
    Failed { reason: String },
    Cancelled,
}

impl JobStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, JobStatus::Completed { .. } | JobStatus::Failed { .. } | JobStatus::Cancelled)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Pending => "pending",
            JobStatus::Queued => "queued",
            JobStatus::Running { .. } => "running",
            JobStatus::Paused => "paused",
            JobStatus::Completed { .. } => "completed",
            JobStatus::Failed { .. } => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }
}

/// The central domain entity: a connection-agnostic job.
/// Defined as Source + Destination + Mode (blueprint Section 3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub source: Source,
    pub destination: Destination,
    pub mode: TransferMode,
    pub policy: Policy,
    pub schedule: Option<Schedule>,
    pub credentials_ref: Option<String>,
    pub filters: Filters,
    pub status: JobStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub checkpoint: Option<u64>,
    pub label: Option<String>,
}

impl Job {
    pub fn new(source: impl Into<Source>, destination: impl Into<Destination>, mode: TransferMode) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            source: source.into(),
            destination: destination.into(),
            mode,
            policy: Policy::default(),
            schedule: Some(Schedule::Immediate),
            credentials_ref: None,
            filters: Filters::default(),
            status: JobStatus::Pending,
            created_at: now,
            updated_at: now,
            checkpoint: None,
            label: None,
        }
    }

    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self.updated_at = Utc::now();
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self.updated_at = Utc::now();
        self
    }

    pub fn record_checkpoint(&mut self, offset: u64) {
        self.checkpoint = Some(offset);
        self.updated_at = Utc::now();
    }

    /// Basic transition logic (domain rule).
    pub fn transition_to(&mut self, new_status: JobStatus) -> bool {
        if Self::can_transition(&self.status, &new_status) {
            self.status = new_status;
            self.updated_at = Utc::now();
            true
        } else {
            false
        }
    }

    fn can_transition(from: &JobStatus, to: &JobStatus) -> bool {
        use JobStatus::*;
        match (from, to) {
            (Pending, Queued) => true,
            (Queued, Running { .. }) => true,
            (Running { .. }, Running { .. }) => true,
            (Running { .. }, Completed { .. }) => true,
            (Running { .. }, Failed { .. }) => true,
            (Running { .. }, Cancelled) => true,
            (Running { .. }, Paused) => true,
            (Paused, Running { .. }) => true,
            (Paused, Cancelled) => true,
            (_, Cancelled) => true,
            _ => false,
        }
    }
}
