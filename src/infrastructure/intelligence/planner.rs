//! `CostModelPlanner` — turns a `PairProfile` into a `TransferPlan` (Deliverable 1).
//!
//! # Cost model (documented heuristics — no magic constant without rationale)
//!
//! The planner reasons about three throughput ceilings and tunes the pipeline to
//! the bottleneck one:
//!
//! * `T_net`  — network throughput (from the link probe).
//! * `T_disk` — storage sequential ceiling (from the medium class).
//! * `T_cpu`  — CPU pipeline ceiling (hash + optional compress/encrypt), scaled
//!   by core count and SIMD level.
//!
//! Decisions and *why*:
//!
//! 1. **chunk_size** — large enough to amortise per-chunk fixed costs (syscalls,
//!    hash init, framing) yet small enough that `max_in_flight * chunk` fits the
//!    memory budget and resume granularity stays reasonable. On WAN we size from
//!    the **bandwidth-delay product** so a handful of chunks keep the pipe full;
//!    on local/LAN we size for the storage medium (big sequential reads on
//!    NVMe/SSD, smaller on HDD to bound the cost of any seek).
//! 2. **stream_count** — one TCP flow is limited by its congestion window on
//!    high-BDP paths, so we open several to fill the pipe; but too many cause
//!    self-congestion, so we cap (LAN low, WAN higher, relay lowest).
//! 3. **worker_threads** — the CPU pipeline pool, sized to the *slower* endpoint's
//!    logical cores (the transfer runs on both ends), reserving one core for IO
//!    orchestration when there are spare cores.
//! 4. **max_in_flight** — derived from a hard memory budget (a fraction of the
//!    tighter free RAM, capped), so peak buffer memory = `max_in_flight * chunk`.
//! 5. **compression** — only when the link is the bottleneck AND the CPU can
//!    compress faster than the link can drain (else compression *is* the
//!    bottleneck). Level scales inversely with link speed: a slow link makes CPU
//!    cycles cheap relative to bytes saved.
//! 6. **encryption** — AES-GCM when BOTH ends have AES hardware (multi-GB/s), else
//!    ChaCha20-Poly1305 (fastest software AEAD without AES-NI). Honoured only if
//!    the policy asks for in-transit encryption.
//! 7. **GPU offload** — only if a GPU is present, the `gpu` feature is compiled,
//!    and the CPU is the predicted bottleneck (offload can't help an IO-bound job).
//!    Always has a CPU fallback.

use crate::application::ports::Planner;
use crate::domain::plan::{CompressionCodec, EncryptionCodec, TransferPlan, TransportHint};
use crate::domain::profile::{NetworkClass, PairProfile, SimdLevel, StorageClass};
use crate::domain::{Endpoint, Job};
use tracing::info;

// --- Cost-model constants (each with a rationale) ---

/// Per-core zstd(level 3) compression throughput, MB/s. Conservative published
/// median for zstd negative-to-low levels on modern x86; used to compare against
/// the link to decide if compression helps.
const ZSTD_MBPS_PER_CORE: f64 = 450.0;

/// Per-core BLAKE3 hashing throughput (MB/s) with SIMD. BLAKE3 is ~1–3 GB/s/core;
/// 1500 is a safe midpoint that we further scale by SIMD level below.
const BLAKE3_MBPS_PER_CORE_BASE: f64 = 1500.0;

/// Fraction of the tighter endpoint's *available* RAM we are willing to dedicate
/// to in-flight chunk buffers. 1/4 leaves ample headroom for the OS + app.
const RAM_BUDGET_FRACTION: f64 = 0.25;

/// Absolute cap on in-flight buffer memory regardless of RAM (1 GiB). Prevents a
/// 256-core/1-TB-RAM box from buffering tens of GB and hurting latency/fairness.
const MAX_INFLIGHT_MEMORY: u64 = 1 << 30;

/// Chunk-size clamps. Floor amortises fixed per-chunk costs; ceiling bounds memory
/// and keeps resume granularity sane.
const CHUNK_MIN: u64 = 256 * 1024; // 256 KiB
const CHUNK_MAX: u64 = 64 * 1024 * 1024; // 64 MiB

pub struct CostModelPlanner;

impl CostModelPlanner {
    pub fn new() -> Self {
        Self
    }

    fn simd_scale(level: SimdLevel) -> f64 {
        // Hashing/compression scale roughly with vector width.
        match level {
            SimdLevel::Avx512 => 1.6,
            SimdLevel::Avx2 => 1.3,
            SimdLevel::Sse42 => 1.0,
            SimdLevel::Neon => 1.1,
            SimdLevel::Scalar => 0.6,
        }
    }
}

impl Default for CostModelPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Planner for CostModelPlanner {
    fn plan(&self, job: &Job, p: &PairProfile) -> TransferPlan {
        let mut reasons: Vec<String> = Vec::new();

        let cores = p.min_logical_cores();
        let link = &p.link;
        // Storage ceiling is the slower of the two volumes (both must read/write).
        let disk_mbps = p
            .source
            .storage
            .seq_read_mbps
            .min(p.destination.storage.seq_read_mbps);
        let simd = p.source.cpu.simd.min(p.destination.cpu.simd);

        // -------- (3) worker_threads --------
        // Reserve one core for IO/orchestration when we have spares.
        let worker_threads = if cores > 2 { cores - 1 } else { cores };
        reasons.push(format!(
            "workers={} (min logical cores={} across endpoints, reserve 1 for IO)",
            worker_threads, cores
        ));

        // -------- (1) chunk_size --------
        let chunk_size = match link.class {
            NetworkClass::Wan | NetworkClass::Relay => {
                // Size from BDP so ~4 chunks keep the pipe full; clamp.
                let bdp = link.bdp_bytes();
                let target = (bdp / 4.0).max(CHUNK_MIN as f64);
                reasons.push(format!(
                    "chunk from BDP≈{:.2} MiB ({} path) → ~4 chunks fill the pipe",
                    bdp / 1.049e6,
                    link.class.as_str()
                ));
                target as u64
            }
            NetworkClass::Lan => {
                reasons.push("chunk=4 MiB (LAN: balance syscall amortisation vs latency)".into());
                4 * 1024 * 1024
            }
            NetworkClass::Loopback => {
                // Local copy: size for the storage medium.
                let by_disk = match p.source.storage.class {
                    StorageClass::Nvme => 16 * 1024 * 1024,
                    StorageClass::Ssd | StorageClass::Flash => 8 * 1024 * 1024,
                    StorageClass::Hdd => 1024 * 1024, // small to bound seek cost
                    StorageClass::Unknown => 4 * 1024 * 1024,
                };
                reasons.push(format!(
                    "chunk sized for {} storage (loopback path, network not bottleneck)",
                    p.source.storage.class.as_str()
                ));
                by_disk
            }
        }
        .clamp(CHUNK_MIN, CHUNK_MAX);

        // -------- (2) stream_count --------
        let stream_count: u32 = match link.class {
            NetworkClass::Loopback => 1, // local copy: parallelism is in the worker pool, not streams
            NetworkClass::Lan => 4,      // overcome per-flow limits on a fat LAN
            NetworkClass::Wan => {
                // Fill BDP with multiple flows; scale with BDP/chunk, cap to avoid self-congestion.
                let need = (link.bdp_bytes() / chunk_size as f64).ceil() as u32;
                need.clamp(4, 32)
            }
            NetworkClass::Relay => 2, // relay is bandwidth-capped; extra flows just add overhead
        };
        reasons.push(format!("streams={} for {} path", stream_count, link.class.as_str()));

        // -------- (6) encryption --------
        let encryption = if job.policy.encrypt_in_transit || job.policy.zero_knowledge {
            if p.both_have_aes_hw() {
                reasons.push("encryption=AES-256-GCM (AES hardware on both ends)".into());
                EncryptionCodec::AesGcm
            } else {
                reasons.push("encryption=ChaCha20-Poly1305 (no AES-HW on ≥1 end → faster in SW)".into());
                EncryptionCodec::ChaCha20
            }
        } else {
            reasons.push("encryption=none (policy.encrypt_in_transit=false)".into());
            EncryptionCodec::None
        };

        // -------- CPU pipeline ceiling (for compression decision + bottleneck) --------
        let scale = Self::simd_scale(simd);
        let hash_mbps = BLAKE3_MBPS_PER_CORE_BASE * scale * worker_threads as f64;
        // Encryption also consumes the pipeline; AEAD ~ a few GB/s, treat as not the limiter
        // unless software AES. We fold a rough factor into the CPU ceiling.
        let enc_factor = match encryption {
            EncryptionCodec::AesGcm => 0.9,
            EncryptionCodec::ChaCha20 => 0.8,
            EncryptionCodec::None => 1.0,
        };
        let cpu_mbps = hash_mbps * enc_factor;

        // -------- (5) compression --------
        let link_mbps = link.throughput_mbps / 8.0; // Mbps → MB/s
        let compress_mbps = ZSTD_MBPS_PER_CORE * scale * worker_threads as f64;
        let compression = if link.class == NetworkClass::Loopback {
            // Local copy: compression rarely helps (disk is fast, CPU becomes the limit).
            reasons.push("compression=off (loopback: CPU would become the bottleneck)".into());
            CompressionCodec::None
        } else if link_mbps < compress_mbps && link_mbps < disk_mbps {
            // Link is the bottleneck and CPU can compress faster than the link drains.
            // Slower link → higher level (CPU cycles are cheap relative to bytes saved).
            let level = if link.throughput_mbps < 100.0 {
                12
            } else if link.throughput_mbps < 500.0 {
                6
            } else {
                3
            };
            reasons.push(format!(
                "compression=zstd:{} (link {:.0} Mbps < CPU compress {:.0} MB/s → worth it)",
                level, link.throughput_mbps, compress_mbps
            ));
            CompressionCodec::Zstd { level }
        } else {
            reasons.push(format!(
                "compression=off (link {:.0} MB/s not the bottleneck vs CPU {:.0} MB/s)",
                link_mbps, compress_mbps
            ));
            CompressionCodec::None
        };

        // -------- (4) max_in_flight + memory budget --------
        let ram_budget = ((p.min_available_ram() as f64 * RAM_BUDGET_FRACTION) as u64)
            .min(MAX_INFLIGHT_MEMORY)
            .max(chunk_size * 2); // always allow at least a double-buffer
        let max_in_flight =
            ((ram_budget / chunk_size) as u32).clamp(2, (stream_count * 4).max(4));
        let memory_budget_bytes = chunk_size * max_in_flight as u64;
        reasons.push(format!(
            "in_flight={} chunks (budget {:.0} MiB = {:.0} MiB RAM-cap / {} chunk)",
            max_in_flight,
            memory_budget_bytes as f64 / 1.049e6,
            ram_budget as f64 / 1.049e6,
            chunk_size
        ));

        // -------- (7) GPU offload --------
        let gpu_present = p.source.gpu.present || p.destination.gpu.present;
        let cpu_is_bottleneck = cpu_mbps < link_mbps.min(disk_mbps);
        let use_gpu_offload = cfg!(feature = "gpu") && gpu_present && cpu_is_bottleneck;
        reasons.push(format!(
            "gpu_offload={} (present={}, cpu_bottleneck={}, feature={})",
            use_gpu_offload,
            gpu_present,
            cpu_is_bottleneck,
            cfg!(feature = "gpu")
        ));

        // -------- transport hint --------
        let transport = transport_for(job.source.inner(), job.destination.inner());

        // -------- bottleneck + estimate --------
        let (bottleneck, est_mbps) = {
            let candidates = [("cpu", cpu_mbps), ("disk", disk_mbps), ("network", link_mbps)];
            let (name, mbps) = candidates
                .iter()
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .copied()
                .unwrap();
            (name.to_string(), mbps)
        };
        reasons.push(format!(
            "estimate≈{:.0} MB/s, bottleneck={} (cpu={:.0}, disk={:.0}, net={:.0} MB/s)",
            est_mbps, bottleneck, cpu_mbps, disk_mbps, link_mbps
        ));

        let explanation = format!("Plan for job {}: {}", job.id, reasons.join("; "));
        info!(job_id = %job.id, %explanation, "transfer plan computed");

        TransferPlan {
            chunk_size,
            stream_count,
            max_in_flight,
            worker_threads,
            compression,
            encryption,
            use_gpu_offload,
            transport,
            memory_budget_bytes,
            max_bps: job.policy.max_bps,
            cost_estimated_mbps: est_mbps,
            cost_bottleneck: bottleneck,
            explanation,
        }
    }
}

fn transport_for(src: &Endpoint, dst: &Endpoint) -> TransportHint {
    let is_device = |e: &Endpoint| matches!(e, Endpoint::Device { .. });
    let is_cloud = |e: &Endpoint| matches!(e, Endpoint::Cloud { .. });
    if is_device(src) || is_device(dst) {
        TransportHint::P2p
    } else if is_cloud(src) || is_cloud(dst) {
        TransportHint::Cloud
    } else {
        TransportHint::LocalParallel
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::profile::{
        AsyncIoBackend, CpuInfo, EndpointProfile, GpuInfo, LinkProfile, MemoryInfo, OsFsInfo,
        StorageInfo,
    };
    use crate::domain::{Destination, Endpoint, Source, TransferMode};

    fn endpoint_profile(cores: u32, aes: bool, simd: SimdLevel, storage: StorageClass, ram_gb: f64) -> EndpointProfile {
        EndpointProfile {
            label: "test".into(),
            cpu: CpuInfo { physical_cores: cores, logical_cores: cores, simd, aes_hw: aes, last_level_cache_bytes: None },
            memory: MemoryInfo {
                total_bytes: (ram_gb * 1.073e9) as u64,
                available_bytes: (ram_gb * 1.073e9 * 0.7) as u64,
            },
            storage: StorageInfo { class: storage, seq_read_mbps: storage.seq_read_mbps() },
            gpu: GpuInfo::default(),
            os_fs: OsFsInfo {
                platform: "linux".into(),
                case_sensitive_fs: true,
                max_path_len: 4096,
                max_open_fds: 1024,
                async_io: AsyncIoBackend::IoUring,
                preserves_unix_perms: true,
                preserves_symlinks: true,
                timestamp_resolution_ns: 1,
            },
            explanation: String::new(),
        }
    }

    fn pair(link: LinkProfile, cores: u32, aes: bool, simd: SimdLevel, storage: StorageClass) -> PairProfile {
        PairProfile {
            source: endpoint_profile(cores, aes, simd, storage, 16.0),
            destination: endpoint_profile(cores, aes, simd, storage, 16.0),
            link,
            captured_at: chrono::Utc::now(),
        }
    }

    fn job() -> Job {
        Job::new(
            Source::from(Endpoint::Local { path: "/sandbox/a".into() }),
            Destination::from(Endpoint::Local { path: "/sandbox/b".into() }),
            TransferMode::Copy,
        )
    }

    fn wan(throughput_mbps: f64, rtt: f64) -> LinkProfile {
        LinkProfile {
            class: NetworkClass::Wan,
            rtt_ms: rtt,
            jitter_ms: 1.0,
            loss_ratio: 0.0,
            throughput_mbps,
            mtu: 1500,
            explanation: String::new(),
        }
    }

    #[test]
    fn aes_hardware_selects_aesgcm_else_chacha() {
        let p_hw = pair(wan(1000.0, 20.0), 8, true, SimdLevel::Avx2, StorageClass::Ssd);
        let plan = CostModelPlanner::new().plan(&job(), &p_hw);
        assert_eq!(plan.encryption, EncryptionCodec::AesGcm);

        let p_nohw = pair(wan(1000.0, 20.0), 8, false, SimdLevel::Avx2, StorageClass::Ssd);
        let plan = CostModelPlanner::new().plan(&job(), &p_nohw);
        assert_eq!(plan.encryption, EncryptionCodec::ChaCha20);
    }

    #[test]
    fn slow_link_enables_compression_with_higher_level() {
        // 50 Mbps WAN → link is the bottleneck, CPU can compress faster → compress on.
        let p = pair(wan(50.0, 40.0), 8, true, SimdLevel::Avx2, StorageClass::Ssd);
        let plan = CostModelPlanner::new().plan(&job(), &p);
        match plan.compression {
            CompressionCodec::Zstd { level } => assert!(level >= 6, "slow link should pick a higher level"),
            CompressionCodec::None => panic!("expected compression on a slow link"),
        }
    }

    #[test]
    fn loopback_disables_compression() {
        let link = LinkProfile {
            class: NetworkClass::Loopback,
            rtt_ms: 0.05,
            jitter_ms: 0.0,
            loss_ratio: 0.0,
            throughput_mbps: 20000.0,
            mtu: 65535,
            explanation: String::new(),
        };
        let p = pair(link, 8, true, SimdLevel::Avx2, StorageClass::Nvme);
        let plan = CostModelPlanner::new().plan(&job(), &p);
        assert_eq!(plan.compression, CompressionCodec::None);
        assert_eq!(plan.stream_count, 1, "loopback uses a single stream (pool parallelism instead)");
    }

    #[test]
    fn high_bdp_wan_opens_multiple_streams() {
        // Fat, high-latency pipe → many streams to fill the BDP.
        let p = pair(wan(1000.0, 100.0), 16, true, SimdLevel::Avx2, StorageClass::Nvme);
        let plan = CostModelPlanner::new().plan(&job(), &p);
        assert!(plan.stream_count >= 4, "high-BDP WAN should open >=4 streams");
    }

    #[test]
    fn memory_budget_bounds_in_flight() {
        let p = pair(wan(1000.0, 20.0), 8, true, SimdLevel::Avx2, StorageClass::Ssd);
        let plan = CostModelPlanner::new().plan(&job(), &p);
        // peak buffer memory must equal chunk * max_in_flight and stay under 1 GiB cap.
        assert_eq!(plan.memory_budget_bytes, plan.chunk_size * plan.max_in_flight as u64);
        assert!(plan.memory_budget_bytes <= (1u64 << 30) + plan.chunk_size);
        assert!(plan.max_in_flight >= 2);
    }

    #[test]
    fn explanation_is_populated() {
        let p = pair(wan(500.0, 20.0), 8, true, SimdLevel::Avx2, StorageClass::Ssd);
        let plan = CostModelPlanner::new().plan(&job(), &p);
        assert!(plan.explanation.contains("Plan for job"));
        assert!(!plan.cost_bottleneck.is_empty());
    }
}
