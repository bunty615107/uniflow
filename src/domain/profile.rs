//! Rich, infrastructure-aware profiling types (Deliverable 1).
//!
//! These are **pure, serializable** domain values produced by the `SystemProfiler`
//! port and consumed by the `Planner` port. They deliberately carry no behaviour
//! and no I/O — detection logic lives in `infrastructure::intelligence::profiler`.
//!
//! The legacy `HardwareProfile` / `NetworkProbeResult` (in `models.rs`) remain for
//! backwards compatibility with the existing `Optimizer`; these richer types are
//! what the new profile-first engine reasons over.

use serde::{Deserialize, Serialize};

/// Widest SIMD instruction family the CPU actually advertises at runtime.
/// Ordered by capability so `>=` comparisons are meaningful.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum SimdLevel {
    /// No vector acceleration detected — portable scalar fallback.
    Scalar = 0,
    /// ARM NEON (Apple Silicon, most modern ARM64).
    Neon = 1,
    /// x86 SSE4.2.
    Sse42 = 2,
    /// x86 AVX2.
    Avx2 = 3,
    /// x86 AVX-512 (foundation).
    Avx512 = 4,
}

impl SimdLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            SimdLevel::Scalar => "scalar",
            SimdLevel::Neon => "neon",
            SimdLevel::Sse42 => "sse4.2",
            SimdLevel::Avx2 => "avx2",
            SimdLevel::Avx512 => "avx512",
        }
    }
}

/// Storage medium class — drives chunk sizing and read parallelism.
/// (Random-IO ceilings differ by ~100x between NVMe and HDD.)
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum StorageClass {
    Nvme,
    Ssd,
    Hdd,
    /// Removable flash / SD / phone storage — SSD-like seq, poor sustained random.
    Flash,
    Unknown,
}

impl StorageClass {
    /// Rough sequential read ceiling (MB/s) used as a sanity bound in the planner.
    /// These are conservative medians, not vendor maxima — documented in the cost model.
    pub fn seq_read_mbps(&self) -> f64 {
        match self {
            StorageClass::Nvme => 3500.0,
            StorageClass::Ssd => 550.0,
            StorageClass::Hdd => 160.0,
            StorageClass::Flash => 90.0,
            StorageClass::Unknown => 300.0,
        }
    }
    /// Whether issuing many concurrent random reads helps (true for solid state)
    /// or hurts (false for spinning disks, where seeks dominate).
    pub fn benefits_from_random_parallelism(&self) -> bool {
        matches!(self, StorageClass::Nvme | StorageClass::Ssd | StorageClass::Flash)
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            StorageClass::Nvme => "nvme",
            StorageClass::Ssd => "ssd",
            StorageClass::Hdd => "hdd",
            StorageClass::Flash => "flash",
            StorageClass::Unknown => "unknown",
        }
    }
}

/// Best async-IO backend the OS offers. Used to decide whether deep IO queues pay off.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AsyncIoBackend {
    IoUring, // Linux 5.1+
    Iocp,    // Windows
    Kqueue,  // macOS / BSD
    Epoll,   // Linux fallback
    None,
}

impl AsyncIoBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            AsyncIoBackend::IoUring => "io_uring",
            AsyncIoBackend::Iocp => "iocp",
            AsyncIoBackend::Kqueue => "kqueue",
            AsyncIoBackend::Epoll => "epoll",
            AsyncIoBackend::None => "none",
        }
    }
}

/// Network path class between two endpoints — the single biggest input to stream count.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NetworkClass {
    /// Same host (loopback / shared memory / same disk).
    Loopback,
    /// Same LAN — low RTT, high BW, no NAT.
    Lan,
    /// Wide-area — meaningful RTT, BDP matters, possible loss.
    Wan,
    /// Relayed (NAT could not be traversed) — bandwidth-capped, latency-heavy.
    Relay,
}

impl NetworkClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            NetworkClass::Loopback => "loopback",
            NetworkClass::Lan => "lan",
            NetworkClass::Wan => "wan",
            NetworkClass::Relay => "relay",
        }
    }
}

/// CPU facts that matter for the cost model.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CpuInfo {
    pub physical_cores: u32,
    pub logical_cores: u32,
    pub simd: SimdLevel,
    /// True if the CPU exposes AES hardware (AES-NI on x86, FEAT_AES on ARM).
    /// Decides AES-GCM vs ChaCha20 selection.
    pub aes_hw: bool,
    /// L2/L3 cache hint in bytes if known (used to keep chunk working sets cache-friendly).
    pub last_level_cache_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StorageInfo {
    pub class: StorageClass,
    /// Measured or estimated sequential read ceiling (MB/s) for the endpoint's volume.
    pub seq_read_mbps: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GpuInfo {
    pub present: bool,
    pub vendor: String, // "nvidia" | "amd" | "apple" | "intel" | ""
    pub vram_bytes: Option<u64>,
}

/// OS + filesystem facts needed to make a cross-platform move correct and lossless.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OsFsInfo {
    pub platform: String,         // "windows" | "macos" | "linux" | "android" | "ios" | ...
    pub case_sensitive_fs: bool,  // ext4=true, NTFS/APFS-default=false
    pub max_path_len: u32,        // 260 (legacy Win) / 4096 (Linux) ...
    pub max_open_fds: u32,
    pub async_io: AsyncIoBackend,
    pub preserves_unix_perms: bool,   // can we set mode bits on the target?
    pub preserves_symlinks: bool,
    /// Filesystem timestamp resolution in nanoseconds (NTFS=100ns, ext4=1ns, FAT=2s).
    pub timestamp_resolution_ns: u64,
}

/// Everything we learned about ONE endpoint (its host + volume + OS/FS).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointProfile {
    pub label: String,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub storage: StorageInfo,
    pub gpu: GpuInfo,
    pub os_fs: OsFsInfo,
    /// Human-readable trail of how each fact was obtained (auditability).
    pub explanation: String,
}

/// The measured link between source and destination.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkProfile {
    pub class: NetworkClass,
    pub rtt_ms: f64,
    pub jitter_ms: f64,
    pub loss_ratio: f64,         // 0.0..=1.0
    pub throughput_mbps: f64,    // measured or estimated
    pub mtu: u32,
    pub explanation: String,
}

impl LinkProfile {
    /// Bandwidth-delay product in bytes — the amount of data "in flight" to keep
    /// the pipe full. Core input to chunk sizing and in-flight depth.
    pub fn bdp_bytes(&self) -> f64 {
        // throughput[Mbps] -> bytes/s = *1e6/8 ; RTT seconds = rtt_ms/1000
        (self.throughput_mbps * 1_000_000.0 / 8.0) * (self.rtt_ms / 1000.0)
    }
}

/// The full per-endpoint-pair profile that the Planner turns into a TransferPlan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairProfile {
    pub source: EndpointProfile,
    pub destination: EndpointProfile,
    pub link: LinkProfile,
    pub captured_at: chrono::DateTime<chrono::Utc>,
}

impl PairProfile {
    /// The slower of the two endpoints' core counts — the realistic CPU budget
    /// for a streaming pipeline that must run on both ends.
    pub fn min_logical_cores(&self) -> u32 {
        self.source.cpu.logical_cores.min(self.destination.cpu.logical_cores).max(1)
    }
    /// The tighter free-RAM budget across both ends (bounds total in-flight memory).
    pub fn min_available_ram(&self) -> u64 {
        self.source.memory.available_bytes.min(self.destination.memory.available_bytes)
    }
    /// AES hardware is only worth selecting if BOTH ends have it (the slow end dominates).
    pub fn both_have_aes_hw(&self) -> bool {
        self.source.cpu.aes_hw && self.destination.cpu.aes_hw
    }
}
