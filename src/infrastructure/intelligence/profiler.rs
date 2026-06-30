//! Real, cross-platform `SystemProfiler` (Deliverable 1).
//!
//! Replaces the hard-coded `HardwareProfile` with genuine runtime detection:
//! - **CPU**: physical/logical cores, widest SIMD family, AES hardware — via
//!   `std::arch` runtime feature detection (no deps) + `sysinfo` for core counts.
//! - **Memory**: total/available — via `sysinfo`.
//! - **Storage**: SSD/HDD/NVMe class — via `sysinfo` disk kinds + heuristics.
//! - **GPU**: best-effort presence/vendor — env + (future) driver probe.
//! - **OS/FS**: platform, case-sensitivity, max path, async-IO backend, perms,
//!   timestamp resolution — via `cfg!` + `/proc` where available.
//! - **Network**: RTT/jitter/loss/throughput/path-class — measured TCP probe with
//!   honest estimates when no address is resolvable.
//!
//! Results are **cached per endpoint pair** (5-minute TTL) so repeated jobs over
//! the same route don't re-probe. Every fact records how it was obtained.

use crate::application::ports::SystemProfiler;
use crate::domain::profile::{
    AsyncIoBackend, CpuInfo, EndpointProfile, GpuInfo, LinkProfile, MemoryInfo, NetworkClass,
    OsFsInfo, PairProfile, SimdLevel, StorageClass, StorageInfo,
};
use crate::domain::Endpoint;
use crate::error::Result;
use chrono::Utc;
use std::collections::HashMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// How long a cached `PairProfile` is considered fresh.
const CACHE_TTL_SECS: i64 = 300;

pub struct DefaultSystemProfiler {
    cache: Mutex<HashMap<String, PairProfile>>,
    /// Number of RTT samples for the network probe.
    rtt_samples: usize,
    connect_timeout_ms: u64,
}

impl DefaultSystemProfiler {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            rtt_samples: 5,
            connect_timeout_ms: 1500,
        }
    }

    fn cache_key(source: &Endpoint, dest: &Endpoint) -> String {
        format!("{}=>{}", source.label(), dest.label())
    }
}

impl Default for DefaultSystemProfiler {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------- CPU / SIMD / AES detection (std only) ----------------

#[cfg(target_arch = "x86_64")]
fn detect_simd_and_aes() -> (SimdLevel, bool, &'static str) {
    let aes = std::is_x86_feature_detected!("aes");
    let level = if std::is_x86_feature_detected!("avx512f") {
        SimdLevel::Avx512
    } else if std::is_x86_feature_detected!("avx2") {
        SimdLevel::Avx2
    } else if std::is_x86_feature_detected!("sse4.2") {
        SimdLevel::Sse42
    } else {
        SimdLevel::Scalar
    };
    (level, aes, "x86_64 runtime feature detection (cpuid)")
}

#[cfg(target_arch = "aarch64")]
fn detect_simd_and_aes() -> (SimdLevel, bool, &'static str) {
    let aes = std::arch::is_aarch64_feature_detected!("aes");
    let level = if std::arch::is_aarch64_feature_detected!("neon") {
        SimdLevel::Neon
    } else {
        SimdLevel::Scalar
    };
    (level, aes, "aarch64 runtime feature detection")
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn detect_simd_and_aes() -> (SimdLevel, bool, &'static str) {
    (SimdLevel::Scalar, false, "unknown arch — scalar fallback")
}

fn detect_cpu() -> (CpuInfo, String) {
    let logical = std::thread::available_parallelism()
        .map(|p| p.get() as u32)
        .unwrap_or(1);

    // sysinfo gives physical core count; fall back to logical/2 (or logical) if unknown.
    let physical = {
        let sys = sysinfo::System::new();
        sys.physical_core_count()
            .map(|c| c as u32)
            .unwrap_or((logical / 2).max(1))
    };

    let (simd, aes_hw, how) = detect_simd_and_aes();
    let info = CpuInfo {
        physical_cores: physical.max(1),
        logical_cores: logical,
        simd,
        aes_hw,
        last_level_cache_bytes: None, // not portably available without extra deps
    };
    let expl = format!(
        "CPU: {} physical / {} logical cores, SIMD={}, AES-HW={} ({})",
        info.physical_cores, info.logical_cores, simd.as_str(), aes_hw, how
    );
    (info, expl)
}

fn detect_memory() -> (MemoryInfo, String) {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    // sysinfo 0.30 reports bytes.
    let total = sys.total_memory();
    let available = sys.available_memory();
    let info = MemoryInfo { total_bytes: total, available_bytes: available };
    let expl = format!(
        "RAM: total={:.1} GiB, available={:.1} GiB (sysinfo)",
        total as f64 / 1.073e9,
        available as f64 / 1.073e9
    );
    (info, expl)
}

fn detect_storage() -> (StorageInfo, String) {
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();
    // Coarse, endpoint-level estimate: prefer the fastest medium present.
    // (Per-volume mount matching is a documented future refinement.)
    let mut class = StorageClass::Unknown;
    for d in &disks {
        let k = format!("{:?}", d.kind()).to_lowercase();
        let candidate = if k.contains("ssd") {
            // NVMe vs SATA SSD is not exposed portably; treat as SSD unless name hints NVMe.
            let name = format!("{:?}", d.name()).to_lowercase();
            if name.contains("nvme") { StorageClass::Nvme } else { StorageClass::Ssd }
        } else if k.contains("hdd") {
            StorageClass::Hdd
        } else {
            StorageClass::Unknown
        };
        class = pick_faster(class, candidate);
    }
    let info = StorageInfo { class, seq_read_mbps: class.seq_read_mbps() };
    let expl = format!(
        "Storage: class={} (~{:.0} MB/s seq ceiling, sysinfo disk kinds)",
        class.as_str(), info.seq_read_mbps
    );
    (info, expl)
}

fn pick_faster(a: StorageClass, b: StorageClass) -> StorageClass {
    let rank = |c: StorageClass| match c {
        StorageClass::Nvme => 4,
        StorageClass::Ssd => 3,
        StorageClass::Flash => 2,
        StorageClass::Hdd => 1,
        StorageClass::Unknown => 0,
    };
    if rank(b) > rank(a) { b } else { a }
}

fn detect_gpu() -> (GpuInfo, String) {
    // Best-effort, dependency-free: env hints (consistent with the existing detectors)
    // plus Apple unified memory on aarch64 macOS. A real CUDA/Metal probe is a documented
    // future plug-in; absence here simply means the planner won't choose GPU offload.
    if std::env::var("CUDA_VISIBLE_DEVICES").is_ok() || std::env::var("NVIDIA_VISIBLE_DEVICES").is_ok() {
        return (
            GpuInfo { present: true, vendor: "nvidia".into(), vram_bytes: None },
            "GPU: NVIDIA detected via CUDA/NVIDIA env".into(),
        );
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return (
            GpuInfo { present: true, vendor: "apple".into(), vram_bytes: None },
            "GPU: Apple Silicon unified-memory GPU".into(),
        );
    }
    #[allow(unreachable_code)]
    (GpuInfo::default(), "GPU: none detected (CPU paths only)".into())
}

fn detect_linux_async_backend() -> AsyncIoBackend {
    // io_uring requires Linux >= 5.1. Read the kernel release without libc.
    if let Ok(rel) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        let mut it = rel.trim().split(['.', '-']);
        if let (Some(maj), Some(min)) = (it.next(), it.next()) {
            if let (Ok(maj), Ok(min)) = (maj.parse::<u32>(), min.parse::<u32>()) {
                if maj > 5 || (maj == 5 && min >= 1) {
                    return AsyncIoBackend::IoUring;
                }
            }
        }
    }
    AsyncIoBackend::Epoll
}

fn detect_os_fs() -> (OsFsInfo, String) {
    let platform = std::env::consts::OS.to_string();
    // Defaults per platform (documented heuristics; real per-volume probing is future work).
    let (case_sensitive, max_path, max_fds, async_io, perms, symlinks, ts_res): (
        bool, u32, u32, AsyncIoBackend, bool, bool, u64,
    ) = if cfg!(windows) {
        // NTFS default: case-insensitive, legacy MAX_PATH 260, IOCP, 100ns timestamps.
        (false, 260, 8192, AsyncIoBackend::Iocp, false, false, 100)
    } else if cfg!(target_os = "macos") {
        (false, 1024, 10240, AsyncIoBackend::Kqueue, true, true, 1)
    } else if cfg!(target_os = "ios") {
        (true, 1024, 1024, AsyncIoBackend::Kqueue, true, true, 1)
    } else if cfg!(target_os = "android") {
        (true, 4096, 1024, AsyncIoBackend::Epoll, true, true, 1)
    } else {
        // Linux / other Unix
        (true, 4096, 1024, detect_linux_async_backend(), true, true, 1)
    };

    let info = OsFsInfo {
        platform: platform.clone(),
        case_sensitive_fs: case_sensitive,
        max_path_len: max_path,
        max_open_fds: max_fds,
        async_io,
        preserves_unix_perms: perms,
        preserves_symlinks: symlinks,
        timestamp_resolution_ns: ts_res,
    };
    let expl = format!(
        "OS/FS: {}, case_sensitive={}, max_path={}, async_io={}, unix_perms={}, ts_res={}ns",
        platform, case_sensitive, max_path, async_io.as_str(), perms, ts_res
    );
    (info, expl)
}

// ---------------- Network classification + probe ----------------

fn same_host(a: &Endpoint, b: &Endpoint) -> bool {
    matches!((a, b), (Endpoint::Local { .. }, Endpoint::Local { .. }))
        || matches!((a, b),
            (Endpoint::Device { device_id: x, .. }, Endpoint::Device { device_id: y, .. }) if x == y)
}

/// Try to extract a `host:port` socket target from an endpoint, if it has one.
fn socket_target(e: &Endpoint) -> Option<String> {
    match e {
        Endpoint::Remote { uri } => {
            // Accept forms: "tcp://h:p", "h:p", "ssh://h", "//h:p"
            let s = uri
                .trim_start_matches("tcp://")
                .trim_start_matches("ssh://")
                .trim_start_matches("//");
            let s = s.split('/').next().unwrap_or(s);
            if s.contains(':') {
                Some(s.to_string())
            } else if !s.is_empty() {
                Some(format!("{}:22", s)) // default to ssh port for RTT-only probe
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_private_lan(host: &str) -> bool {
    host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("127.")
        || host.starts_with("172.16.")
        || host.starts_with("172.17.")
        || host == "localhost"
}

impl DefaultSystemProfiler {
    fn measure_link(&self, source: &Endpoint, dest: &Endpoint) -> LinkProfile {
        if same_host(source, dest) {
            return LinkProfile {
                class: NetworkClass::Loopback,
                rtt_ms: 0.05,
                jitter_ms: 0.0,
                loss_ratio: 0.0,
                throughput_mbps: 20_000.0, // loopback / same-disk: effectively memory/IO bound
                mtu: 65535,
                explanation: "Same-host endpoints → loopback path; network is not the bottleneck."
                    .into(),
            };
        }

        // Cloud endpoints → WAN by default (no direct socket to probe here).
        if matches!(source, Endpoint::Cloud { .. }) || matches!(dest, Endpoint::Cloud { .. }) {
            return LinkProfile {
                class: NetworkClass::Wan,
                rtt_ms: 30.0,
                jitter_ms: 5.0,
                loss_ratio: 0.001,
                throughput_mbps: 500.0,
                mtu: 1500,
                explanation: "Cloud endpoint → WAN estimate (rclone bridge owns the real transfer)."
                    .into(),
            };
        }

        // Try a real TCP RTT probe if we can resolve an address.
        let target = socket_target(dest).or_else(|| socket_target(source));
        if let Some(addr) = target {
            if let Some(lp) = self.tcp_rtt_probe(&addr) {
                return lp;
            }
        }

        // No resolvable address — honest LAN estimate.
        LinkProfile {
            class: NetworkClass::Lan,
            rtt_ms: 1.0,
            jitter_ms: 0.2,
            loss_ratio: 0.0,
            throughput_mbps: 940.0, // ~1 GbE
            mtu: 1500,
            explanation: "No resolvable socket address → conservative 1 GbE LAN estimate.".into(),
        }
    }

    fn tcp_rtt_probe(&self, addr: &str) -> Option<LinkProfile> {
        let sockaddr = addr.to_socket_addrs().ok()?.next()?;
        let host = addr.split(':').next().unwrap_or(addr);
        let mut rtts = Vec::new();
        for _ in 0..self.rtt_samples {
            let start = Instant::now();
            if TcpStream::connect_timeout(&sockaddr, Duration::from_millis(self.connect_timeout_ms)).is_ok() {
                rtts.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        if rtts.is_empty() {
            return None;
        }
        let avg = rtts.iter().sum::<f64>() / rtts.len() as f64;
        let jitter = if rtts.len() > 1 {
            (rtts.iter().map(|r| (r - avg).powi(2)).sum::<f64>() / rtts.len() as f64).sqrt()
        } else {
            avg * 0.1
        };
        let loss = 1.0 - (rtts.len() as f64 / self.rtt_samples as f64);
        let class = if is_private_lan(host) { NetworkClass::Lan } else { NetworkClass::Wan };
        // Throughput estimate: LAN→1GbE, WAN→scaled by RTT (rough, labelled as estimate).
        let throughput = match class {
            NetworkClass::Lan => 940.0,
            _ => (100.0 / avg.max(1.0) * 50.0).clamp(10.0, 1000.0),
        };
        Some(LinkProfile {
            class,
            rtt_ms: avg,
            jitter_ms: jitter,
            loss_ratio: loss,
            throughput_mbps: throughput,
            mtu: 1500,
            explanation: format!(
                "TCP RTT probe to {} ({} samples): avg={:.2}ms jitter={:.2}ms loss={:.1}% class={}",
                addr, rtts.len(), avg, jitter, loss * 100.0, class.as_str()
            ),
        })
    }
}

impl SystemProfiler for DefaultSystemProfiler {
    fn profile_endpoint(&self, endpoint: &Endpoint) -> Result<EndpointProfile> {
        // NB: detection currently reflects the LOCAL host running the daemon. For a
        // remote/peer endpoint a real deployment would query the remote agent; the
        // contract (this trait) is identical, so that is a drop-in future swap.
        let (cpu, e_cpu) = detect_cpu();
        let (memory, e_mem) = detect_memory();
        let (storage, e_sto) = detect_storage();
        let (gpu, e_gpu) = detect_gpu();
        let (os_fs, e_os) = detect_os_fs();

        let explanation = format!("[{}] {}; {}; {}; {}; {}", endpoint.label(), e_cpu, e_mem, e_sto, e_gpu, e_os);
        debug!(endpoint = %endpoint.label(), "endpoint profiled");

        Ok(EndpointProfile {
            label: endpoint.label(),
            cpu,
            memory,
            storage,
            gpu,
            os_fs,
            explanation,
        })
    }

    fn profile_link(&self, source: &Endpoint, dest: &Endpoint) -> Result<LinkProfile> {
        Ok(self.measure_link(source, dest))
    }

    fn profile_pair(&self, source: &Endpoint, dest: &Endpoint) -> Result<PairProfile> {
        let key = Self::cache_key(source, dest);

        // Serve from cache if fresh.
        if let Some(cached) = self.cache.lock().unwrap().get(&key) {
            let age = Utc::now().signed_duration_since(cached.captured_at).num_seconds();
            if age < CACHE_TTL_SECS {
                debug!(key = %key, age_s = age, "profile cache hit");
                return Ok(cached.clone());
            }
        }

        let profile = PairProfile {
            source: self.profile_endpoint(source)?,
            destination: self.profile_endpoint(dest)?,
            link: self.profile_link(source, dest)?,
            captured_at: Utc::now(),
        };

        info!(
            key = %key,
            link = %profile.link.class.as_str(),
            min_cores = profile.min_logical_cores(),
            "infrastructure profiled (fresh)"
        );

        self.cache.lock().unwrap().insert(key, profile.clone());
        Ok(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sane_hardware() {
        let prof = DefaultSystemProfiler::new();
        let ep = prof.profile_endpoint(&Endpoint::Local { path: "/tmp/x".into() }).unwrap();
        assert!(ep.cpu.logical_cores >= 1);
        assert!(ep.cpu.physical_cores >= 1);
        assert!(!ep.os_fs.platform.is_empty());
        // explanation trail must be populated for auditability.
        assert!(ep.explanation.contains("CPU"));
    }

    #[test]
    fn loopback_link_is_classified() {
        let prof = DefaultSystemProfiler::new();
        let a = Endpoint::Local { path: "/tmp/a".into() };
        let b = Endpoint::Local { path: "/tmp/b".into() };
        let link = prof.profile_link(&a, &b).unwrap();
        assert_eq!(link.class, NetworkClass::Loopback);
        assert!(link.throughput_mbps > 1000.0);
    }

    #[test]
    fn pair_profile_is_cached() {
        let prof = DefaultSystemProfiler::new();
        let a = Endpoint::Local { path: "/tmp/a".into() };
        let b = Endpoint::Local { path: "/tmp/b".into() };
        let p1 = prof.profile_pair(&a, &b).unwrap();
        let p2 = prof.profile_pair(&a, &b).unwrap();
        // Same capture timestamp ⇒ served from cache, not re-probed.
        assert_eq!(p1.captured_at, p2.captured_at);
    }
}
