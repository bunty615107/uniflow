# UniFlow Phase 1: Core Local & On-Prem Transfer Engine (Delta + BLAKE3 + Parallel + Resume)

**Role**: High-performance file transfer engine (Rsync-grade delta + parallelism).

**Goal**: Replace the NoopTransport for Local ↔ Local and Server ↔ Server (on-prem path-based) modes with a real, production-grade delta engine on top of the Phase 0 clean architecture foundation.

## Architecture Overview

We extend the existing clean architecture without breaking the connection-agnostic contract.

### Layer Responsibilities (Phase 1 additions)

- **domain/** (pure, serializable, no I/O):
  - Add delta-specific value types: `BlockSignature`, `FileSignature`, `DeltaChunk`, `TransferChunk`, `ResumeState`, `FileManifest`.
  - `Job` remains the central entity; `checkpoint: Option<u64>` (or richer `ResumeState`) is used for byte-level resume.
  - `Endpoint` (via Source/Destination) already supports Local and Remote for on-prem paths.

- **application/ports.rs** (contracts):
  - Keep `Transport` trait unchanged (execute(job) -> TransferReport). This preserves Phase 0 compatibility.
  - Optionally introduce finer-grained ports for testability:
    - `SignatureGenerator`
    - `DeltaEngine`
    - `ContentHasher` (BLAKE3)
  - These ports live in `application/ports/` to allow swapping implementations (e.g. librsync vs future pure-Rust or cloud-optimized delta).

- **application/services/**:
  - `JobService` remains largely unchanged. It receives a `Transport` at construction (now we will pass a real `LocalDeltaTransport`).
  - The worker loop already supports checkpoint updates via `repo.save()` during execution and cancel tokens.
  - Minor enhancement: during long transfers, the transport can call back to update `job.checkpoint` and persist via the repo (or we pass a progress callback).

- **infrastructure/** (adapters + heavy lifting):
  - New module: `infrastructure/delta/` or `infrastructure/transfer/local/`
    - `librsync.rs`: Safe wrappers around `librsync-sys` (signature, delta, patch streaming).
    - `blake3_hasher.rs`: Multithreaded + SIMD BLAKE3 using `blake3` crate + rayon.
    - `local_delta_transport.rs`: The main `impl Transport for LocalDeltaTransport`.
      - Inspects Source/Destination (both Local or both "Remote" for on-prem server paths).
      - For Local ↔ Local: uses direct filesystem access + delta optimization.
      - For Server ↔ Server (Phase 1 path-based): assumes paths are accessible (NFS, mounted volumes, or later SSH/gRPC will provide the byte streams).
  - Keep `noop.rs` for tests / non-local modes.
  - Update `infrastructure/mod.rs` to export the new transport.

- **daemon.rs**:
  - Update `Daemon::new()` to construct and use `LocalDeltaTransport` (or a router that selects based on endpoint kinds).
  - For now (Phase 1), default to delta transport for local/on-prem jobs; fallback to noop for cloud/device.

- **Integration with existing Job model**:
  - `TransferMode::Copy` and `OneWaySync` trigger the delta engine.
  - `policy.verify_integrity` forces post-transfer BLAKE3 check.
  - `checkpoint` stores the last successfully transferred byte offset (or chunk index) for resume.
  - Deduplication: we maintain a simple in-transfer fingerprint map (BLAKE3 of blocks); future persistent dedup DB will be added.

## Data Flow (Local ↔ Local or Server ↔ Server)

1. **Job Submission** (via Daemon/JobService):
   - User defines `Source(Local { path })` + `Destination(Local { path })` + `mode: Copy`.
   - Job persisted with status `Queued`, `checkpoint: None`.

2. **Execution Start** (in LocalDeltaTransport::execute):
   - Determine if source and dest are local paths or on-prem remote paths.
   - Open source file(s) for reading, dest for writing (create dirs as needed).

3. **Signature Generation** (block-level, librsync):
   - If destination exists and has previous signature (or we generate weak/strong rolling checksums via librsync-sys).
   - Generate `FileSignature` (block hashes) for the *old* version at destination (or use cached sig if resume).
   - Use librsync-sys `rs_sig_begin` / streaming to produce signatures for blocks (typically 2KB-8KB blocks).

4. **Parallel BLAKE3 Fingerprinting + Dedup** (rayon + blake3 SIMD):
   - Walk the source file in chunks (parallel with rayon).
   - Compute BLAKE3 hash per chunk (multithreaded, hardware-accelerated when possible via SIMD).
   - Build `FileManifest` containing ordered list of `BlockSignature { offset, size, blake3_hash, weak_sum? }`.
   - Dedup: if a block's BLAKE3 matches a block in the destination signature, mark as "copy from dest" (zero-copy where possible).

5. **Delta Creation** (librsync):
   - Feed the source file + destination signature into librsync `rs_delta_begin`.
   - Produce a stream of `DeltaChunk` (literal data or "copy from old offset" commands).
   - This is the Rsync-grade block-level delta.

6. **Parallel Transfer with Byte-level Resume / Checkpointing**:
   - The delta is processed in parallel chunks where safe (rayon work-stealing pool for I/O-bound + CPU work).
   - For each chunk:
     - If "copy from old", seek in destination and copy range (or use reflink/clone if filesystem supports).
     - If literal, write the new data.
   - After every N bytes or successful chunk:
     - Update `job.checkpoint = current_byte_offset`.
     - Persist via `repo.save(&job)` (this enables resume after crash/interrupt).
     - Respect cancellation token (from Phase 0).
   - Stream the writes to destination file.

7. **Post-Transfer Integrity Verification** (BLAKE3):
   - If `policy.verify_integrity`:
     - Re-hash the final destination file (or critical blocks) using the same parallel BLAKE3 hasher.
     - Compare root hash or per-block against the source manifest.
     - On mismatch: fail the job (or retry specific blocks).
   - Store the final BLAKE3 root hash in `TransferReport.integrity_hash`.

8. **Completion / Resume Handling**:
   - On resume (job loaded with existing `checkpoint`):
     - Skip already-transferred prefix.
     - Re-generate signature only from the checkpoint point if possible (librsync supports streaming).
     - Continue delta from the resume point.
   - Update status to `Completed` or `Failed`.
   - Return `TransferReport` with actual bytes moved (delta size is often << full file), duration, hash, chunk count.

9. **Error & Cancellation**:
   - Any I/O, librsync, or hash mismatch -> `UniFlowError::Transport(...)`.
   - Cancellation: stop writing, update status to `Cancelled`, preserve checkpoint for later resume.

## Key Rust Structs / Traits (Phase 1)

### Domain (src/domain/models.rs additions)

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockSignature {
    pub offset: u64,
    pub size: u32,
    pub blake3: [u8; 32],           // strong hash
    pub weak: u32,                  // rolling checksum from librsync (for delta)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileSignature {
    pub block_size: u32,
    pub blocks: Vec<BlockSignature>,
    pub total_size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DeltaInstruction {
    Copy { old_offset: u64, size: u32 },
    Literal { data: Vec<u8> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeltaChunk {
    pub instructions: Vec<DeltaInstruction>,
    pub source_offset: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ResumeState {
    pub bytes_transferred: u64,
    pub last_verified_block: u64,
    pub source_manifest: Option<FileSignature>,
}

 // Job already has `checkpoint: Option<u64>` – we can evolve it to hold ResumeState (serialized).
```

### Application Ports (extensions in application/ports.rs)

```rust
// Existing Transport trait stays the same.

pub trait DeltaEngine: Send + Sync {
    fn generate_signature(&self, path: &std::path::Path) -> Result<FileSignature>;
    fn create_delta(&self, source: &std::path::Path, sig: &FileSignature) -> Result<Vec<DeltaChunk>>;
    fn apply_delta(&self, dest: &std::path::Path, delta: &[DeltaChunk], resume_from: u64) -> Result<u64>;
}

pub trait ContentHasher: Send + Sync {
    fn hash_file_parallel(&self, path: &std::path::Path) -> Result<[u8; 32]>; // root BLAKE3
    fn hash_blocks_parallel(&self, path: &std::path::Path, block_size: u32) -> Result<Vec<BlockSignature>>;
}
```

### Infrastructure Implementation

```rust
// src/infrastructure/transfer/local_delta.rs
pub struct LocalDeltaTransport {
    hasher: Arc<dyn ContentHasher>,
    engine: Arc<dyn DeltaEngine>,
    // rayon pool handle if we want a dedicated one
}

#[async_trait]
impl Transport for LocalDeltaTransport {
    async fn execute(&self, job: &Job) -> Result<TransferReport> { ... }
}
```

Internal helpers:
- `librsync_signature(path) -> FileSignature` (wraps rs_sig_begin / rs_sig_load / streaming)
- `blake3_parallel_hasher` using `blake3::Hasher::new().update_rayon(...)` or manual rayon + blake3::Hasher per chunk + tree hash.
- Resume logic: load `job.checkpoint`, seek source/dest, continue signature/delta from offset.

## Implementation Plan (Step-by-Step)

1. **Cargo & Crates** (update Cargo.toml):
   - Add `blake3 = { version = "1", features = ["rayon", "std"] }`
   - Add `librsync-sys = "0.2"` (or latest compatible; note: requires librsync dev lib on build machine)
   - Optional: `memmap2` for faster large-file access (not strictly required by Sec 13, but helpful).

2. **Domain Extensions**:
   - Add the structs above to `domain/models.rs`.
   - Update `Job.checkpoint` to be `Option<ResumeState>` (or keep u64 + add a separate field; serialize the richer state into the existing Option for backward compat in Phase 0 jobs).

3. **Ports**:
   - Add `DeltaEngine` and `ContentHasher` traits to `application/ports.rs` (or a new `delta.rs`).
   - Re-export in lib.rs.

4. **Delta Engine Core** (infrastructure):
   - Create `src/infrastructure/delta/mod.rs`
   - `librsync.rs`: `pub fn rsync_signature(...)`, `pub fn rsync_delta(...)`, `pub fn rsync_patch(...)` using the C bindings safely (use `std::io::Read/Write` streams).
   - `blake3.rs`: `pub struct ParallelBlake3Hasher; impl ContentHasher for ...` – split file, rayon::scope or par_iter, blake3 per chunk, combine.
   - Handle errors with our `UniFlowError`.

5. **LocalDeltaTransport**:
   - Implement `Transport`.
   - In `execute`:
     - Resolve paths from Source/Destination (support both Local and Remote variants for on-prem).
     - If dest doesn't exist or no resume: full transfer with delta (still benefits from dedup if partial overlap).
     - Generate sig of dest (if exists).
     - Parallel hash source manifest.
     - Create delta.
     - Apply with resume logic, updating checkpoint frequently (every 1MB or per chunk).
     - Final BLAKE3 verify if policy requires.
     - Return detailed `TransferReport`.

6. **Integration**:
   - In `daemon.rs`: construct `LocalDeltaTransport` (wrapping the hasher/engine) and pass to `JobService`.
   - In `job_service.rs` (minor): during worker loop, allow the transport to mutate/update the job's checkpoint before saving.
   - Update `infrastructure/mod.rs` and `lib.rs` exports.
   - Make `JobService` and `Daemon` generic over transport or keep the current injection pattern.

7. **Demo & Polish**:
   - Update `main.rs` to include a Local ↔ Local example (create temp files with similar content, run Copy job, demonstrate small delta size, simulate interrupt + resume).
   - Add tracing at key steps (signature size, delta size, bytes actually written, hash time).
   - Handle edge cases: empty files, identical files (zero delta), permission errors.

8. **Testing / Verification** (future but recommended):
   - Unit tests for hasher and delta roundtrips.
   - Integration test in main or `#[test]` that exercises resume.

9. **Documentation**:
   - This file + update main architecture.md and README.

## Constraints & Future-Proofing

- **Approved crates only** for the core (librsync-sys, blake3, rayon, tokio). Other helpers (e.g. for directory walking) can use std or small crates if needed.
- Every real transfer path must go through signature → delta → patch + BLAKE3.
- Resume must be byte-granular (not just file-level).
- Deduplication fingerprinting is performed via the BLAKE3 block hashes in the manifest.
- Design allows swapping the delta engine later for cross-cloud (e.g. object-store delta without full download).
- For Server ↔ Server in this phase: treat "Remote" endpoints as local paths on the server (the actual remote access will come in later transport layers like SSH/gRPC that can stream the same delta chunks).

This Phase 1 engine gives UniFlow Rsync-grade performance for on-prem use cases while keeping the overall system transport-agnostic and ready for cloud/P2P extensions.

---

**Next**: Proceed to code changes per the todo list (update Cargo, extend domain, implement engine, etc.).
