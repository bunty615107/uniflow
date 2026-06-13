//! Pluggable Hardware Abstraction Layer (HAL) for Module 04.
//!
//! Detectors are traits. New hardware (QAT, new accelerators, etc.) = new impl + register.
//! Default detectors use std + basic heuristics. Accelerators are stubbed for easy extension.

use crate::application::ports::HardwareProfile;
use std::sync::Mutex;

pub trait HardwareDetector: Send + Sync {
    fn name(&self) -> &'static str;
    fn detect(&self) -> Option<HardwareProfile>;
    fn explain(&self) -> String;
}

/// Thread-safe registry of detectors. New detectors added at runtime easily.
pub struct HardwareRegistry {
    detectors: Mutex<Vec<Box<dyn HardwareDetector>>>,
}

impl HardwareRegistry {
    pub fn new() -> Self {
        Self {
            detectors: Mutex::new(Vec::new()),
        }
    }

    pub fn register(&self, detector: Box<dyn HardwareDetector>) {
        self.detectors.lock().unwrap().push(detector);
    }

    pub fn detect_all(&self) -> HardwareProfile {
        let mut profile = HardwareProfile::default();
        let guards = self.detectors.lock().unwrap();

        for d in guards.iter() {
            if let Some(p) = d.detect() {
                profile.cpu_cores = profile.cpu_cores.max(p.cpu_cores);
                profile.cpu_features.extend(p.cpu_features);
                profile.ram_gb = profile.ram_gb.max(p.ram_gb);
                profile.accelerators.extend(p.accelerators);
            }
        }

        // Dedup features
        profile.cpu_features.sort();
        profile.cpu_features.dedup();
        profile.accelerators.sort();
        profile.accelerators.dedup();

        profile
    }
}

impl Default for HardwareRegistry {
    fn default() -> Self {
        let reg = Self::new();

        // Always-available basic detector
        reg.register(Box::new(BasicSystemDetector));
        // Stubs for accelerators (real impls would use dlopen, env, or crates)
        reg.register(Box::new(QatDetector));
        reg.register(Box::new(CudaDetector));
        reg.register(Box::new(AppleSiliconDetector));

        reg
    }
}

struct BasicSystemDetector;

impl HardwareDetector for BasicSystemDetector {
    fn name(&self) -> &'static str { "basic_system" }

    fn detect(&self) -> Option<HardwareProfile> {
        // Very basic — in production use num_cpus + sysinfo or similar (lightweight).
        let cores = std::thread::available_parallelism().map(|p| p.get() as u32).unwrap_or(4);
        Some(HardwareProfile {
            cpu_cores: cores,
            cpu_features: vec!["avx2".to_string()], // assume common
            ram_gb: 8.0, // placeholder
            disk_iops: Some(1000),
            accelerators: vec![],
        })
    }

    fn explain(&self) -> String {
        "Basic std-based detection of cores and assumed features. Extend with sysinfo for accurate RAM/disk.".to_string()
    }
}

struct QatDetector;
impl HardwareDetector for QatDetector {
    fn name(&self) -> &'static str { "intel_qat" }
    fn detect(&self) -> Option<HardwareProfile> {
        // Real: check /dev/qat* or env QAT_ENABLED or PCI IDs.
        if std::env::var("QAT_ENABLED").is_ok() {
            let mut p = HardwareProfile::default();
            p.accelerators.push("intel_qat".to_string());
            p.cpu_features.push("qat".to_string());
            return Some(p);
        }
        None
    }
    fn explain(&self) -> String { "Detects Intel QAT via env or driver presence for hardware compression/encryption offload.".to_string() }
}

struct CudaDetector;
impl HardwareDetector for CudaDetector {
    fn name(&self) -> &'static str { "nvidia_cuda" }
    fn detect(&self) -> Option<HardwareProfile> {
        if std::env::var("CUDA_VISIBLE_DEVICES").is_ok() || std::env::var("NVIDIA_VISIBLE_DEVICES").is_ok() {
            let mut p = HardwareProfile::default();
            p.accelerators.push("nvidia_cuda".to_string());
            return Some(p);
        }
        None
    }
    fn explain(&self) -> String { "Detects NVIDIA CUDA via env vars for GPU-accelerated hashing/compression (future use).".to_string() }
}

struct AppleSiliconDetector;
impl HardwareDetector for AppleSiliconDetector {
    fn name(&self) -> &'static str { "apple_silicon" }
    fn detect(&self) -> Option<HardwareProfile> {
        #[cfg(target_os = "macos")]
        {
            // Real detection via sysctl -n hw.optional.arm64 or uname.
            if std::env::consts::ARCH == "aarch64" {
                let mut p = HardwareProfile::default();
                p.accelerators.push("apple_unified".to_string());
                p.cpu_features.push("apple_silicon".to_string());
                p.cpu_cores = 8; // typical M-series
                p.ram_gb = 16.0;
                return Some(p);
            }
        }
        None
    }
    fn explain(&self) -> String { "Detects Apple Silicon (ARM + unified memory) on macOS for NEON-optimized paths.".to_string() }
}