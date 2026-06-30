# UniFlow Module 04: Intelligence & Optimiser

**Role**: Performance and systems intelligence specialist.

This module implements the pre-transfer intelligence layer that profiles the environment, detects hardware, probes the network, and auto-tunes transfer parameters for optimal performance across all engines (Local Delta from Phase 1, Cloud from Module 01, P2P from Module 03).

It integrates as a pluggable pre-processing step before transport selection/execution.

## Architecture

### Placement in Clean Architecture

- **domain/models.rs**: Lightweight additions only.
  - `ProfilingResult` (or embed in Job).
  - `TuningDecision` (threads, chunk_size, compression_level, throttle_bps, schedule_hint).
  - `HardwareProfile` (detected accelerators, CPU features).
  - Keep models serializable and transport-agnostic.

- **application/ports.rs** (contracts):
  - New pluggable traits (easy to extend):
    - `NetworkProbe`: pre-transfer RTT, bandwidth estimate, jitter.
    - `HardwareDetector`: trait for CPU/RAM/Disk + accelerators (QAT, CUDA, Apple Silicon, etc.).
    - `Optimizer`: consumes profiles, produces `TuningDecision` with explanations.
  - `IntelligenceEngine`: orchestrator trait that runs probes + detectors + optimizer.
  - The existing `Transport::probe` is reused/enhanced for network data.

- **application/services/**:
  - `JobService` (or a new `IntelligenceService`) wraps job submission with intelligence:
    - Before selecting transport or calling `execute`, run `IntelligenceEngine::profile_and_tune(&mut job)`.
    - Store `job.profiling = Some(...)` and apply tuning to the transport or job.policy.
  - Decisions are logged with structured fields (job_id, decision, reason, confidence).

- **infrastructure/intelligence/**:
  - Concrete implementations:
    - `CustomNetworkProbe` (using tokio UDP/TCP for RTT + small transfers for bandwidth; follows Section 13 "custom Rust implementation").
    - `HardwareAbstractionLayer` + registry of `HardwareDetector` impls (pluggable: std-based for CPU/RAM/Disk, conditional for QAT/CUDA via feature flags or dlopen).
    - `DefaultOptimizer`: rule-based + simple heuristics for auto-tuning (threads from CPU cores, chunk_size from bandwidth*RTT, compression if CPU allows, throttle for off-peak).
  - `IntelligenceEngineImpl` wires them.
  - All outputs include human-readable `explanation: String` for logging/audit.

- **infrastructure/ (engines)**:
  - Existing transports (LocalDeltaTransport, RcloneCloudTransport, IrohP2PTransport) receive tuned params via:
    - Job extension (new fields or policy).
    - Or constructor params from the router/service.
  - They use the tuning (e.g., rayon thread pool size, chunk size in multipart/delta, compression flag).

- **daemon.rs + TransportRouter**:
  - `Daemon` or a new `IntelligenceAwareRouter` runs the intelligence layer before selecting the concrete transport.
  - This ensures every job (regardless of engine) gets optimized.

- **logging**:
  - Every probe result, detection, and decision uses `tracing::info!` with fields like:
    - `job_id`, `probe_type="network"`, `rtt_ms=12`, `bandwidth_mbps=850`, `reason="high_bandwidth_detected"`.
  - `Optimizer` always produces `decision.explanation = "Using 16 threads because 8 cores + hyperthreading + high bandwidth (>500Mbps); chunk_size=8MiB from BDP calculation..."`.

### Data Flow (Pre-Transfer Intelligence)

1. User/CLI/API submits Job (Source + Dest + Mode + optional Schedule/Policy).
2. `JobService::submit` (or Daemon) calls `intelligence.profile_and_tune(&mut job)`.
3. Inside `IntelligenceEngine`:
   a. Run `NetworkProbe::probe(source, dest)` → RTT, bandwidth, jitter (async, quick).
   b. Run registered `HardwareDetector`s (CPU/RAM/Disk + accelerators) → `HardwareProfile`.
   c. (Optional) Disk I/O bench if local endpoints.
   d. `Optimizer::optimize(job, profiles)` → `TuningDecision`.
4. Apply decision:
   - Mutate `job.policy` or add `job.tuning = Some(decision)`.
   - Log fully explainable decision.
5. Persist updated Job (checkpoint/profile for audit/resume).
6. Router selects transport based on endpoints (as before).
7. Transport uses tuning from Job (e.g., set rayon threads, chunk size for multipart/delta streams, compression level, start time if off-peak).
8. During transfer: adaptive throttling can re-probe or react to congestion.
9. Post-transfer: record actual vs predicted performance (future ML training).

### Profiling Strategy

- **Network (pre-transfer, ~1-3s)**:
  - RTT: TCP connect + small ping packets (multiple for average/jitter).
  - Bandwidth: short burst transfer (configurable size, e.g. 10-100MB) or use existing small-object probe; estimate from time.
  - Jitter: stddev of RTT samples.
  - Air-gap aware: skip if both endpoints local or same LAN (from discovery in P2P).
  - Off-peak: if current time in Schedule window, bias toward aggressive settings.

- **System Resources**:
  - CPU: core count, features (via cpuid or std), current load.
  - RAM: total/available (for buffer sizing).
  - Disk I/O: optional quick sequential read/write bench on source/dest paths (skipped for pure cloud).

- **Hardware Detection (pluggable HAL)**:
  - Base detectors (always available): CPU/RAM/Disk via `std` + optional `sysinfo` (lightweight).
  - Accelerator detectors (feature-gated or runtime dlopen):
    - Intel QAT: check for qat driver or env.
    - NVIDIA CUDA: `cuda` crate or env `CUDA_VISIBLE_DEVICES`.
    - Apple Silicon: `sysctl` or `uname` for arm64 + unified memory.
    - Others: AMD, etc. via extensible registry.
  - Registry: `HardwareRegistry` (Vec<Box<dyn HardwareDetector>>) — new detectors registered at startup easily.

- **Auto-Tuning Rules (explainable, logged)**:
  - Threads: min(cores*2, bandwidth-dependent) for rayon/parallel chunks.
  - Chunk size: BDP (bandwidth * RTT) or min(64MiB, file_size/100).
  - Compression: enable if CPU headroom high and link <1Gbps (or policy).
  - Throttling: if current load high or off-peak not active, cap bandwidth.
  - Schedule: respect Job.schedule; if off-peak window, defer or use higher concurrency.

All tunings are **reversible** and logged with "why".

### Pluggability

- Detectors implement:
  ```rust
  pub trait HardwareDetector: Send + Sync {
      fn name(&self) -> &'static str;
      fn detect(&self) -> Option<HardwareInfo>;
      fn explain(&self) -> String;
  }
  ```
- Registry allows runtime addition: `registry.register(Box::new(QatDetector))`.
- Probes/Optimizers follow the same pattern.
- New hardware (future TPUs, etc.) = new detector impl, no core changes.

### Explainability & Logging

Every decision includes:
- Raw metrics.
- Rule that fired.
- Human explanation string.
- Confidence (0.0-1.0).

Structured logs (tracing) + Job persistence of `profiling` and `tuning` for audit/UI.

## Key Structs / Traits (to implement)

(See design doc for full signatures; examples in code below.)

- `NetworkProbeResult { rtt_ms: f64, bandwidth_mbps: f64, jitter_ms: f64, explanation: String }`
- `HardwareProfile { cpu_cores: u32, has_qat: bool, has_cuda: bool, ... }`
- `TuningDecision { threads: usize, chunk_size: u64, compression_level: Option<u8>, max_bps: Option<u64>, start_at: Option<DateTime>, explanation: String }`
- Traits: `NetworkProbe`, `HardwareDetector`, `Optimizer`, `IntelligenceEngine`

## Integration with Phases 1-3 Engines

- Local Delta (Phase 1): uses tuned `chunk_size`, rayon threads for hashing/delta, compression flag.
- Cloud (Module 01): multipart chunk size, concurrency, server-side hints, throttle.
- P2P (Module 03): iroh/quinn stream concurrency, chunking, multi-path bias from probe bandwidth.
- Common: all respect `job.tuning` and `policy` for throttling/scheduling.

The intelligence runs once per job submission (or on resume if stale).

## Implementation Plan (followed in code)

1. Design doc (this file).
2. Extend ports with traits (pluggable).
3. Add `infrastructure/intelligence/`:
   - `probes.rs` (network + system).
   - `hardware.rs` (HAL + registry + sample detectors).
   - `optimizer.rs` (rules + TuningDecision).
   - `engine.rs` (orchestrator).
4. Domain model extensions (ProfilingResult, TuningDecision).
5. Wire into `TransportRouter` / `Daemon` / `JobService` (call intelligence before transport selection/execute).
6. Update main.rs demo + logging.
7. Update docs/README.
8. Ensure all decisions logged/explainable.

Dependencies: keep minimal (tokio for async probes, existing rayon). No heavy new crates unless truly needed (use std + tokio::net for probes).

This module makes UniFlow "smart" while remaining pluggable and auditable, directly fulfilling Section 6 and Section 13 intelligence requirements.

---

**Next steps in code**: ports extension, intelligence module creation, integration, demo. All decisions will be logged.
---

## Update: Real Profiler + Cost-Model Planner (self-optimizing engine)

The Module 04 skeleton (placeholder `HardwareRegistry`, fake `CustomNetworkProbe`,
rule-based `DefaultOptimizer`) is now backed by a **real, profile-first** layer.
The original traits remain for backward compatibility; the new layer adds:

- **`SystemProfiler`** (`application::ports`) → `DefaultSystemProfiler`
  (`infrastructure::intelligence::profiler`): genuine cross-platform detection of
  CPU/SIMD/AES, RAM, storage class, GPU, and OS/FS facts, plus a measured network
  link, **cached per endpoint pair**. Replaces the hard-coded `HardwareProfile`
  values (RAM was `8.0`, features assumed `avx2`).
- **`Planner`** (`application::ports`) → `CostModelPlanner`
  (`infrastructure::intelligence::planner`): an explicit, documented cost model that
  emits a `TransferPlan` (chunk size, stream count, in-flight depth, worker threads,
  compression codec+level, encryption codec, GPU on/off, memory budget) with a
  required `explanation` and the model's own `cost_estimated_mbps` / `cost_bottleneck`.
- **`ComputeOffload`** (`application::ports`) → `CpuOffload` (always) + feature-gated
  `GpuOffload` (`infrastructure::intelligence::offload`): the offload seam with a CPU
  fallback that is always compiled in.

`DefaultIntelligenceEngine::profile_and_tune` now also runs the profiler+planner and
attaches the resulting `TransferPlan` to `job.plan`, which `TransportRouter` uses to
choose the transport and which `ParallelTransport` consumes to configure the run.

See `docs/architecture.md` → *Self-Optimizing Migration Engine* for the full
profiler heuristics, planner cost model, and AIMD adaptive control loop, and
`docs/client-contract.md` for the transport boundary a mobile client drives.
