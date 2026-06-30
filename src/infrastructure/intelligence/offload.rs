//! `ComputeOffload` backends (Deliverable 2 — GPU offload, feature-gated).
//!
//! The hot per-chunk work (hashing, and optionally compression) can run on the CPU
//! or, when profitable and available, on a GPU. The contract guarantees:
//!   * a CPU backend is **always** compiled in (graceful degradation), and
//!   * a GPU backend is only used when the `gpu` feature is enabled, a device is
//!     present, and the planner deemed it profitable; any failure falls back to CPU.
//!
//! The GPU backend here is a clearly-marked placeholder that *defers to the CPU*
//! (so behaviour and hashes are identical) — the real CUDA/Metal kernels are a
//! drop-in behind this same trait. This keeps the architecture honest: the seam
//! exists and is exercised, without shipping an unverifiable native kernel.

use crate::application::ports::ComputeOffload;

/// Always-available CPU backend (BLAKE3, SIMD-accelerated via the `blake3` crate).
pub struct CpuOffload;

impl ComputeOffload for CpuOffload {
    fn name(&self) -> &'static str {
        "cpu"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn hash(&self, data: &[u8]) -> [u8; 32] {
        *blake3::hash(data).as_bytes()
    }
}

/// GPU backend — only compiled when the `gpu` feature is on.
///
/// This reference build computes the SAME BLAKE3 on the CPU (so results are
/// byte-identical and tests pass everywhere); a production build swaps `hash` for a
/// device kernel and reports real availability from the driver. Because it produces
/// identical output and falls back transparently, enabling the feature can never
/// corrupt a transfer.
#[cfg(feature = "gpu")]
pub struct GpuOffload {
    available: bool,
}

#[cfg(feature = "gpu")]
impl GpuOffload {
    pub fn detect() -> Self {
        let mut available = false;
        
        // Genuine hardware detection using wgpu (cross-platform GPU API)
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // Synchronously enumerate all attached GPUs (CUDA/Metal/Vulkan/DX12)
        for adapter in instance.enumerate_adapters(wgpu::Backends::all()) {
            let info = adapter.get_info();
            tracing::info!("GPU Offload detected compatible hardware: {} ({:?})", info.name, info.backend);
            available = true;
        }

        if !available {
            tracing::info!("GPU Offload requested but no compatible hardware found. Falling back to CPU.");
        }

        Self { available }
    }
}

#[cfg(feature = "gpu")]
impl ComputeOffload for GpuOffload {
    fn name(&self) -> &'static str {
        "gpu"
    }
    fn is_available(&self) -> bool {
        self.available
    }
    fn hash(&self, data: &[u8]) -> [u8; 32] {
        // Placeholder: identical to CPU. Production replaces with a device kernel.
        *blake3::hash(data).as_bytes()
    }
}

/// Pick the best available backend for the plan. Always returns something usable.
pub fn select_offload(use_gpu: bool) -> Box<dyn ComputeOffload> {
    #[cfg(feature = "gpu")]
    {
        if use_gpu {
            let gpu = GpuOffload::detect();
            if gpu.is_available() {
                return Box::new(gpu);
            }
        }
    }
    let _ = use_gpu; // unused without the feature
    Box::new(CpuOffload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::ComputeOffload;

    #[test]
    fn cpu_offload_always_available_and_matches_blake3() {
        let cpu = CpuOffload;
        assert!(cpu.is_available());
        let data = b"uniflow-offload-test-payload";
        assert_eq!(cpu.hash(data), *blake3::hash(data).as_bytes());
    }

    #[test]
    fn select_offload_falls_back_to_cpu() {
        // Without the gpu feature (or no device) we must still get a usable backend.
        let backend = select_offload(true);
        assert!(backend.is_available());
        let data = b"x";
        assert_eq!(backend.hash(data), *blake3::hash(data).as_bytes());
    }
}
