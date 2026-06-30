//! Parallel, self-optimizing transfer core (Deliverable 2).
//!
//! Implements the `Transport` port for path-based endpoints. On `execute` it:
//!   1. uses `job.plan` (or self-profiles + plans if absent) — **profile-first**;
//!   2. fans the file out into chunks processed by a bounded worker pool sized from
//!      the plan, each running the pipeline **read → hash → [compress] → [encrypt] →
//!      send**, then on the receive side **decrypt → decompress → write** so the
//!      destination is a faithful, integrity-verified copy;
//!   3. runs an **AIMD** controller that grows/shrinks the in-flight window to live
//!      throughput and backs off on RAM pressure (memory is hard-bounded);
//!   4. verifies every chunk (BLAKE3 round-trip) and the whole file end-to-end;
//!   5. publishes **atomically** (write to temp, fsync, rename) and supports
//!      **resume** via a checkpoint sidecar — never partially overwriting the dest.
//!
//! Compression/encryption are *transport* stages: they shape the bytes on the wire
//! and are reversed before the destination is written, so the stored file is always
//! plaintext-correct (unless a future at-rest policy says otherwise). This keeps the
//! BLAKE3 integrity guarantee and the client-side crypto path intact for the new core.

use crate::application::ports::{Planner, SystemProfiler, TransferReport, Transport};
use crate::domain::plan::{CompressionCodec, EncryptionCodec, TransferPlan};
use crate::domain::{Endpoint, Job};
use crate::error::{Result, UniFlowError};
use crate::infrastructure::delta::blake3_hasher::ParallelBlake3Hasher;
use crate::application::ports::ContentHasher;
use crate::infrastructure::intelligence::{select_offload, CostModelPlanner, DefaultSystemProfiler};
use crate::infrastructure::security::ClientSideEncryption;
use crate::infrastructure::transfer::adapters::{ChunkSink, ChunkSource, LocalFileSink, LocalFileSource};
use crate::infrastructure::transfer::control::{
    AdaptiveController, ControllerConfig, TransferStats,
};
use crate::infrastructure::transfer::paths::resolve_sandboxed;
use async_trait::async_trait;
use rand::{rngs::OsRng, RngCore};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::info;

/// Worker-pool upper bound regardless of plan (avoid thread explosion).
const MAX_WORKERS: u32 = 64;
/// AIMD evaluation cadence.
const CONTROL_TICK: Duration = Duration::from_millis(100);
/// Back off the in-flight window if available RAM drops below this.
const RAM_FLOOR: u64 = 256 * 1024 * 1024;

pub struct ParallelTransport {
    profiler: DefaultSystemProfiler,
    planner: CostModelPlanner,
    hasher: ParallelBlake3Hasher,
}

impl ParallelTransport {
    pub fn new() -> Self {
        Self {
            profiler: DefaultSystemProfiler::new(),
            planner: CostModelPlanner::new(),
            hasher: ParallelBlake3Hasher::new(),
        }
    }

    /// Reuse the job's tuned plan, or self-profile + plan if none was attached.
    fn plan_for(&self, job: &Job, src: &Endpoint, dst: &Endpoint) -> Result<TransferPlan> {
        if let Some(p) = &job.plan {
            return Ok(p.clone());
        }
        let pair = self.profiler.profile_pair(src, dst)?;
        Ok(self.planner.plan(job, &pair))
    }
}

impl Default for ParallelTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// Tracks per-chunk completion to derive the highest *contiguous* byte offset for
/// crash-resume, persisting it to a sidecar file periodically.
struct CompletionTracker {
    done: Vec<bool>,
    cursor: usize,
    contiguous: u64,
    chunk_size: u64,
    total_len: u64,
    last_flushed: u64,
    flush_every: u64,
    ckpt_path: PathBuf,
}

impl CompletionTracker {
    fn new(num_chunks: usize, chunk_size: u64, total_len: u64, ckpt_path: PathBuf) -> Self {
        Self {
            done: vec![false; num_chunks],
            cursor: 0,
            contiguous: 0,
            chunk_size,
            total_len,
            last_flushed: 0,
            flush_every: chunk_size * 4, // bound sidecar IO
            ckpt_path,
        }
    }

    fn chunk_len(&self, i: usize) -> u64 {
        let offset = i as u64 * self.chunk_size;
        (self.total_len - offset).min(self.chunk_size)
    }

    fn mark(&mut self, idx: usize) {
        if idx < self.done.len() {
            self.done[idx] = true;
        }
        while self.cursor < self.done.len() && self.done[self.cursor] {
            self.contiguous += self.chunk_len(self.cursor);
            self.cursor += 1;
        }
        if self.contiguous - self.last_flushed >= self.flush_every {
            let _ = std::fs::write(&self.ckpt_path, self.contiguous.to_string());
            self.last_flushed = self.contiguous;
        }
    }
}

fn read_checkpoint(ckpt_path: &Path) -> u64 {
    std::fs::read_to_string(ckpt_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

#[async_trait]
impl Transport for ParallelTransport {
    fn name(&self) -> &'static str {
        "parallel-core"
    }

    async fn execute(&self, job: &Job) -> Result<TransferReport> {
        let start = Instant::now();

        let src_path = resolve_sandboxed(job.source.inner()).ok_or_else(|| {
            UniFlowError::Config(
                "Source must be Local/on-prem Remote inside the sandbox (rejected for security)".into(),
            )
        })?;
        let dst_path = resolve_sandboxed(job.destination.inner()).ok_or_else(|| {
            UniFlowError::Config(
                "Destination must be Local/on-prem Remote inside the sandbox (rejected)".into(),
            )
        })?;

        let plan = self.plan_for(job, job.source.inner(), job.destination.inner())?;
        info!(job_id = %job.id, plan = %plan.explanation, "parallel-core executing with plan");

        // ---- open source + size the work ----
        let source = LocalFileSource::open(&src_path)?;
        let total_len = source.len();
        let chunk_size = plan.chunk_size.max(1);
        let num_chunks = total_len.div_ceil(chunk_size) as usize;

        // ---- destination temp file (atomic publish) + resume checkpoint ----
        let temp_path = dst_path.with_extension("uniflow-tmp");
        let ckpt_path = dst_path.with_extension("uniflow-ckpt");
        let resume_from = read_checkpoint(&ckpt_path).max(job.checkpoint.unwrap_or(0));
        let sink = if resume_from > 0 && temp_path.exists() {
            info!(job_id = %job.id, resume_from, "resuming from checkpoint");
            LocalFileSink::open_existing(&temp_path, total_len)?
        } else {
            LocalFileSink::create(&temp_path, total_len)?
        };

        // ---- pipeline stage selection from the plan ----
        let comp_level: Option<i32> = match plan.compression {
            CompressionCodec::Zstd { level } => Some(level),
            CompressionCodec::None => None,
        };
        let enc_mode: Option<bool> = match plan.encryption {
            EncryptionCodec::AesGcm => Some(false),  // use_chacha = false
            EncryptionCodec::ChaCha20 => Some(true),
            EncryptionCodec::None => None,
        };
        // Per-transfer key. Production: derived from CredentialVault / zero-knowledge.
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        let enc = ClientSideEncryption::new(key);
        let offload = select_offload(plan.use_gpu_offload);

        // ---- shared coordination state ----
        let stats = std::sync::Arc::new(TransferStats::default());
        let worker_count = plan.max_in_flight.clamp(1, MAX_WORKERS);
        let controller = AdaptiveController::start(
            stats.clone(),
            ControllerConfig {
                min_window: 2.min(plan.max_in_flight).max(1),
                max_window: plan.max_in_flight.max(1),
                chunk_size,
                memory_budget: plan.memory_budget_bytes.max(chunk_size * 2),
                tick: CONTROL_TICK,
                ram_floor_bytes: RAM_FLOOR,
            },
        );
        let sem = controller.sem.clone();
        let limiter = plan.max_bps.map(crate::infrastructure::transfer::control::TokenBucket::new);

        let next_chunk = AtomicU64::new(0);
        let abort = AtomicBool::new(false);
        let err: Mutex<Option<UniFlowError>> = Mutex::new(None);
        let tracker = Mutex::new(CompletionTracker::new(
            num_chunks,
            chunk_size,
            total_len,
            ckpt_path.clone(),
        ));

        let source_ref: &dyn ChunkSource = &source;
        let sink_ref: &dyn ChunkSink = &sink;
        let offload_ref = &*offload;
        let enc_ref = &enc;
        let sem_ref = &*sem;
        let stats_ref = &*stats;
        let limiter_ref = limiter.as_ref();

        // ---- bounded worker pool (scoped threads borrow stack state, no 'static needed) ----
        std::thread::scope(|scope| {
            for _ in 0..worker_count {
                scope.spawn(|| {
                    let mut buf = vec![0u8; chunk_size as usize];
                    loop {
                        if abort.load(Ordering::Relaxed) {
                            break;
                        }
                        let idx = next_chunk.fetch_add(1, Ordering::SeqCst) as usize;
                        if idx >= num_chunks {
                            break;
                        }
                        let offset = idx as u64 * chunk_size;
                        let size = (total_len - offset).min(chunk_size) as usize;

                        // Resume: skip already-completed contiguous chunks.
                        if offset + size as u64 <= resume_from {
                            tracker.lock().unwrap().mark(idx);
                            continue;
                        }

                        if let Some(lim) = limiter_ref {
                            lim.consume(size as u64);
                        }
                        sem_ref.acquire();
                        let res = process_chunk(
                            source_ref, sink_ref, offload_ref, enc_ref, &mut buf, offset, size,
                            comp_level, enc_mode,
                        );
                        sem_ref.release();

                        match res {
                            Ok((plen, wlen)) => {
                                stats_ref.record_chunk(plen, wlen);
                                tracker.lock().unwrap().mark(idx);
                            }
                            Err(e) => {
                                *err.lock().unwrap() = Some(e);
                                abort.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                    }
                });
            }
        });

        let window_history = controller.stop();
        if let Some(e) = err.lock().unwrap().take() {
            return Err(e);
        }

        // ---- durably flush, then verify, then publish atomically ----
        sink.sync()?;

        let mut integrity_hash = None;
        if job.policy.verify_integrity {
            let src_root = self.hasher.hash_file_parallel(&src_path)?;
            let dst_root = self.hasher.hash_file_parallel(&temp_path)?;
            if src_root != dst_root {
                let _ = std::fs::remove_file(&temp_path);
                return Err(UniFlowError::Internal(
                    "end-to-end BLAKE3 verification failed; destination not published".into(),
                ));
            }
            integrity_hash = Some(to_hex(&dst_root));
            info!(job_id = %job.id, "end-to-end BLAKE3 verification passed");
        }

        atomic_publish(&temp_path, &dst_path)?;
        let _ = std::fs::remove_file(&ckpt_path); // success → drop the resume sidecar

        let duration_ms = start.elapsed().as_millis() as u64;
        let bytes = stats.bytes_done.load(Ordering::Relaxed);
        let wire = stats.wire_bytes.load(Ordering::Relaxed);
        info!(
            job_id = %job.id,
            bytes,
            wire_bytes = wire,
            chunks = num_chunks,
            duration_ms,
            peak_window = window_history.iter().copied().max().unwrap_or(0),
            "parallel-core transfer complete (atomic publish)"
        );

        Ok(TransferReport {
            bytes_transferred: bytes,
            duration_ms,
            integrity_hash,
            chunks: num_chunks as u32,
        })
    }
}

/// One unit of the hot pipeline: read → hash → [compress] → [encrypt] → (wire) →
/// [decrypt] → [decompress] → verify → write. Returns (plaintext_len, wire_len).
#[allow(clippy::too_many_arguments)]
fn process_chunk(
    source: &dyn ChunkSource,
    sink: &dyn ChunkSink,
    offload: &dyn crate::application::ports::ComputeOffload,
    enc: &ClientSideEncryption,
    buf: &mut [u8],
    offset: u64,
    size: usize,
    comp_level: Option<i32>,
    enc_mode: Option<bool>,
) -> Result<(u64, u64)> {
    let n = source.read_at(offset, &mut buf[..size])?;
    let plain = &buf[..n];
    let plain_hash = offload.hash(plain);

    // --- wire encode ---
    let compressed = match comp_level {
        Some(level) => zstd::bulk::compress(plain, level).map_err(UniFlowError::Io)?,
        None => plain.to_vec(),
    };
    let (wire, nonce) = match enc_mode {
        Some(use_chacha) => enc.encrypt(&compressed, use_chacha)?,
        None => (compressed, [0u8; 12]),
    };
    let wire_len = wire.len() as u64;

    // --- wire decode (receiver side) ---
    let decrypted = match enc_mode {
        Some(use_chacha) => enc.decrypt(&wire, &nonce, use_chacha)?,
        None => wire,
    };
    let recovered = match comp_level {
        Some(_) => zstd::bulk::decompress(&decrypted, n).map_err(UniFlowError::Io)?,
        None => decrypted,
    };

    // --- per-chunk integrity across the full round trip ---
    if offload.hash(&recovered) != plain_hash {
        return Err(UniFlowError::Internal(format!(
            "per-chunk BLAKE3 mismatch at offset {offset}"
        )));
    }

    sink.write_at(offset, &recovered)?;
    Ok((n as u64, wire_len))
}

/// Atomic-on-completion publish: replace the destination in one rename.
fn atomic_publish(temp: &Path, dst: &Path) -> Result<()> {
    // On Windows, rename fails if the target exists; remove first (small window,
    // acceptable for this engine — a production build would use ReplaceFileW).
    if dst.exists() {
        let _ = std::fs::remove_file(dst);
    }
    std::fs::rename(temp, dst).map_err(UniFlowError::Io)
}

fn to_hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}
