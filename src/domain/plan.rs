//! `TransferPlan` — the output of the Planner (Deliverable 1).
//!
//! A pure, serializable decision record. The parallel transfer core
//! (`infrastructure::transfer::parallel`) executes exactly what the plan says,
//! and the plan's `explanation` + cost-model fields make every choice auditable.

use serde::{Deserialize, Serialize};

/// Which symmetric cipher the per-chunk encryption stage should use.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum EncryptionCodec {
    /// AES-256-GCM — fastest when AES hardware (AES-NI / ARM FEAT_AES) is present.
    AesGcm,
    /// ChaCha20-Poly1305 — fastest cipher on CPUs WITHOUT AES hardware.
    ChaCha20,
    /// No transport encryption (only valid when the policy permits it).
    None,
}

impl EncryptionCodec {
    pub fn as_str(&self) -> &'static str {
        match self {
            EncryptionCodec::AesGcm => "aes-256-gcm",
            EncryptionCodec::ChaCha20 => "chacha20-poly1305",
            EncryptionCodec::None => "none",
        }
    }
}

/// Which compressor the per-chunk compression stage should use.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompressionCodec {
    /// zstd at the given level (1..=19). Level chosen by the cost model.
    Zstd { level: i32 },
    /// No compression (CPU-bound link, or already-compressed data).
    None,
}

impl CompressionCodec {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompressionCodec::Zstd { .. } => "zstd",
            CompressionCodec::None => "none",
        }
    }
    pub fn is_enabled(&self) -> bool {
        matches!(self, CompressionCodec::Zstd { .. })
    }
}

/// Which concrete transport the router should run this plan through.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransportHint {
    LocalParallel, // the new parallel core, path-based endpoints
    Cloud,         // rclone bridge
    P2p,           // iroh/QUIC
}

impl TransportHint {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransportHint::LocalParallel => "local-parallel",
            TransportHint::Cloud => "cloud",
            TransportHint::P2p => "p2p",
        }
    }
}

/// The near-optimal configuration the engine tunes itself to before transferring.
///
/// Every numeric field has a documented derivation in the planner's cost model;
/// `explanation` carries the human-readable trail and `cost_*` fields expose the
/// model's own estimates so decisions can be replayed and audited.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferPlan {
    /// Bytes per work unit. Derived from BDP, storage seq ceiling, and cache size.
    pub chunk_size: u64,
    /// Number of concurrent network streams to open (saturate the NIC without collapse).
    pub stream_count: u32,
    /// Max chunks allowed in flight at once (bounds memory = this * chunk_size).
    pub max_in_flight: u32,
    /// Worker pool size for the CPU pipeline (hash/compress/encrypt).
    pub worker_threads: u32,
    pub compression: CompressionCodec,
    pub encryption: EncryptionCodec,
    /// Whether to offload hashing/compression to the GPU (only set if profitable).
    pub use_gpu_offload: bool,
    pub transport: TransportHint,
    /// Hard upper bound on bytes the transfer may hold in RAM at once.
    pub memory_budget_bytes: u64,
    /// Optional bandwidth limit in bits per second (JULES-10).
    pub max_bps: Option<u64>,

    // --- cost-model transparency (estimates the planner used) ---
    /// Estimated effective throughput ceiling (MB/s) given the bottleneck resource.
    pub cost_estimated_mbps: f64,
    /// Which resource the model believes is the bottleneck ("cpu" | "disk" | "network").
    pub cost_bottleneck: String,

    /// Required, auditable rationale for every decision above.
    pub explanation: String,
}

impl TransferPlan {
    /// Memory this plan is allowed to use for in-flight chunk buffers.
    pub fn in_flight_memory_bytes(&self) -> u64 {
        self.chunk_size.saturating_mul(self.max_in_flight as u64)
    }
}
