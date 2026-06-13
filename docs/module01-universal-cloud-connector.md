# UniFlow Module 01: Universal Cloud Connector

**Role**: Cloud storage integration specialist.

This module provides UniFlow with access to 70+ cloud/object storage backends by embedding Rclone via a gRPC IPC bridge, following the exact pattern approved in Section 13 of the blueprint.

It builds directly on the Phase 0 clean architecture (domain/application/infrastructure + `Transport` port) and Phase 1 delta work.

## Architecture Overview

### High-Level Integration with Clean Architecture

- **domain/**: Minimal extensions. `Endpoint::Cloud { provider, bucket, prefix }` (already present) is used for cloud locations. No new heavy types needed here — cloud specifics are handled in infrastructure. `Job` carries `Source`/`Destination` which can be Cloud.

- **application/ports.rs**: 
  - The core `Transport` trait remains the extension point (no changes required for basic integration).
  - New port: `CredentialVault` (or `CredentialProvider`) for unified credential resolution. This allows jobs to reference creds without embedding secrets in the `Job` model.
  - Optional: `CloudRouter` or selection logic (can live in application/services for now).

- **application/services/**:
  - `JobService` continues to take a `Transport` (or we evolve to a `TransportSelector` / router that picks LocalDelta vs RcloneCloud based on endpoint kinds).
  - The worker loop already handles async execution, checkpointing, cancel, and status updates — perfect for long-running cloud multipart uploads.

- **infrastructure/**:
  - New module: `infrastructure/cloud/`
    - `rclone_bridge.rs` or `rclone_client.rs`: The Rust gRPC client (tonic) that talks to the embedded Rclone process/binary.
    - `rclone_cloud_transport.rs`: `impl Transport for RcloneCloudTransport` — the adapter that translates UniFlow `Job` (Source/Dest/Mode/Policy) into Rclone operations (copy, server-side copy, upload, download, etc.).
    - Credential handling: integration with the vault to configure Rclone remotes on-the-fly or pass auth tokens.

- **daemon.rs**:
  - Composition root evolves to support multiple transports. A simple `TransportRouter` (in application or infra) inspects `Source`/`Destination` kinds and returns the appropriate `Arc<dyn Transport>`.
  - For Phase 1 cloud: if either endpoint is Cloud, use RcloneCloudTransport (with credential injection).

- **IPC Bridge (core of Section 13)**:
  - Rclone (Go) is run as a separate process or embedded via cgo (but blueprint specifies gRPC IPC for isolation).
  - Rust (tonic client) <-> Go (gRPC server wrapping Rclone library or `rclone rc` / library calls).
  - This keeps the Go runtime isolated, provides near-native performance for 70+ backends, and supports server-side copy/move (zero bandwidth when source and dest are same provider, e.g. S3 to S3).

The design prepares for Phase 4 cross-cloud delta by exposing chunk/manifest operations through the same bridge (future `ComputeDelta`, `ApplyDelta` RPCs that can work with object stores without full downloads).

### Data Flow for Key Transfer Modes (from blueprint Section 4 / 9)

1. **Local → Cloud**:
   - Source: Local (path), Dest: Cloud (provider, bucket, prefix).
   - RcloneCloudTransport resolves creds from vault → configures temp Rclone remote.
   - For large files: use multipart/chunked upload (size-based tuning: e.g. 5MiB chunks for S3, auto-tuned by file size/network).
   - Progress: stream chunks via bridge, update Job checkpoint with bytes/chunk index.
   - Optional: client-side encryption before upload if policy requires.

2. **Cloud → Local**:
   - Symmetric download.
   - Supports range requests / partial for resume (using Job checkpoint).
   - BLAKE3 verification post-download if policy.verify_integrity (can be parallelized).

3. **Cloud → Cloud (same provider)**:
   - If provider matches (e.g. both S3 or both GCS), use server-side copy/move RPC.
   - This is zero-bandwidth — Rclone calls the cloud API directly (CopyObject, etc.).
   - Falls back to download+upload only if cross-provider or not supported.

4. **Credential Flow**:
   - Job may carry `credentials_ref: Option<String>` (opaque ID into vault).
   - Before transfer, vault resolves ref → provider-specific creds (access_key, secret, token, service_account, etc.).
   - Creds are injected into the gRPC request (or used to configure remote on Go side) — never logged.
   - Support for per-job or named remotes.

5. **Error / Resume / Observability**:
   - Transport errors mapped to UniFlowError.
   - Chunk failures trigger retry (backoff from policy).
   - Checkpoints persisted via JobRepository during multipart.
   - Structured tracing with job_id + cloud-specific fields (provider, bucket, object, bytes_transferred).

### IPC Interface Design (gRPC Proto)

We define a minimal but extensible service in `proto/rclone_bridge.proto`.

```proto
syntax = "proto3";

package uniflow.rclone;

service RcloneBridge {
  // Basic object operations
  rpc Copy(CopyRequest) returns (TransferResponse);
  rpc Move(MoveRequest) returns (TransferResponse);
  
  // Server-side (zero bandwidth) when supported by provider
  rpc ServerSideCopy(ServerSideCopyRequest) returns (TransferResponse);
  rpc ServerSideMove(ServerSideMoveRequest) returns (TransferResponse);

  // Multipart / chunked for large uploads/downloads
  rpc StartMultipartUpload(MultipartRequest) returns (MultipartSession);
  rpc UploadChunk(ChunkRequest) returns (ChunkResponse);
  rpc CompleteMultipartUpload(CompleteMultipartRequest) returns (TransferResponse);

  // Download with resume support
  rpc DownloadRange(DownloadRangeRequest) returns (stream ChunkResponse);

  // Listing and metadata (for future features)
  rpc ListObjects(ListRequest) returns (ListResponse);
  rpc HeadObject(HeadRequest) returns (HeadResponse);

  // Remote config (for credential injection)
  rpc ConfigureRemote(ConfigureRemoteRequest) returns (ConfigureRemoteResponse);
}

message CopyRequest {
  string src_remote = 1;      // e.g. "s3:src-bucket"
  string src_path = 2;
  string dst_remote = 3;
  string dst_path = 4;
  bool   create_dirs = 5;
  // ... other rclone flags
}

message ServerSideCopyRequest {
  // Same as Copy but signals to use provider-native server-side copy
  // Rclone will choose CopyObject / equivalent if available
  string src_remote = 1;
  // ...
}

message MultipartRequest {
  string remote = 1;
  string path = 2;
  int64  file_size = 3;           // for size-based chunk tuning
  string content_type = 4;
}

message ChunkRequest {
  string session_id = 1;
  int32  part_number = 2;
  bytes  data = 3;
  int64  offset = 4;              // for resume
}

message TransferResponse {
  int64 bytes_transferred = 1;
  string etag_or_hash = 2;
  bool   server_side = 3;         // indicates zero-bandwidth used
}

message Credential {
  string provider = 1;            // "s3", "gcs", etc.
  map<string, string> config = 2; // access_key_id, secret, token, etc. (sensitive)
}

message ConfigureRemoteRequest {
  string name = 1;                // remote name
  Credential credential = 2;
}
```

The Go side (not implemented here, but part of the bridge) would implement this service using Rclone's Go library or `rclone fs` operations + rc (remote control) internals. It receives credentials securely over gRPC (TLS recommended in production).

This interface is designed to be extended for delta (future Phase 4): add `GetObjectSignature`, `ComputeDelta`, etc.

### Credential Vault Design

**Interface (application/ports.rs)**:

```rust
pub trait CredentialVault: Send + Sync {
    /// Resolve a reference (e.g. "my-s3-prod" or "job:123:creds") to provider config.
    /// Returns sensitive data — must be handled carefully (zeroize on drop recommended).
    async fn resolve(&self, reference: &str) -> Result<CloudCredential>;
    
    /// Store a new credential (for CLI/UI flows).
    async fn store(&self, name: &str, cred: CloudCredential) -> Result<()>;
}
```

**CloudCredential** (domain or application):

```rust
pub struct CloudCredential {
    pub provider: String,           // "s3", "azureblob", ...
    pub config: HashMap<String, String>, // rclone-style keys
    // e.g. "access_key_id", "secret_access_key", "session_token", "service_account_file"
}
```

**Infrastructure impl** (for P0/P1):
- Simple `EnvCredentialVault` (reads from env with prefix, e.g. UNIFLOW_S3_ACCESS_KEY).
- `FileCredentialVault` (encrypted JSON or age/sops encrypted file, unlocked at daemon start).
- Future: system keyring, HashiCorp Vault, AWS Secrets Manager via plugins.

Credentials are resolved once per job execution and passed to the Rclone bridge via `ConfigureRemote` (or per-call in the request). Never stored in the `Job` struct itself.

This unifies creds across all 70+ backends (Rclone handles the mapping).

### How It Prepares for Future Cross-Cloud Delta (Phase 4)

- The bridge already gives us chunk-level access (`UploadChunk`, `DownloadRange`).
- Later, we can add RPCs that accept a `FileManifest` (BLAKE3 blocks from Phase 1) and return deltas without full object materialization on the client.
- Server-side copy detection logic can be reused/extended for "same-provider or compatible" cases.
- Unified `Endpoint::Cloud` + credential vault means the job model never changes.

## Implementation Steps (Practical Order)

1. **Proto & IPC Contract**:
   - Create `proto/rclone_bridge.proto` with the service above (start minimal: Copy, ServerSideCopy, Multipart ops, ConfigureRemote).
   - Add `build.rs` using `tonic-build`.
   - Note: The Go server binary must be built separately (small Go module that imports rclone and implements the service). Place it in `rclone-bridge/` or document the expectation.

2. **Dependencies**:
   - Add to Cargo.toml:
     ```toml
     tonic = "0.12"
     prost = "0.13"
     # For build
     # In [build-dependencies]
     tonic-build = "0.12"
     ```
   - `anyhow` / `thiserror` already present.

3. **Credential Vault**:
   - Add `CloudCredential` to domain/models.rs (or a new `cloud.rs`).
   - Add `CredentialVault` trait to application/ports.rs.
   - Implement `EnvCredentialVault` and/or `FileCredentialVault` in infrastructure/credentials/.
   - Wire a default vault into Daemon.

4. **Rclone gRPC Client**:
   - In `infrastructure/cloud/rclone_client.rs`:
     - `pub struct RcloneBridgeClient { client: RcloneBridgeClient<Channel> }`
     - Methods: `copy(...)`, `server_side_copy(...)`, `start_multipart(...)`, `upload_chunk(...)`, etc.
     - Handle connection to the Rclone process (assume it's listening on localhost:port or unix socket; launch if needed).

5. **RcloneCloudTransport**:
   - `pub struct RcloneCloudTransport { client: Arc<RcloneBridgeClient>, vault: Arc<dyn CredentialVault> }`
   - `impl Transport`:
     - In `execute`:
       - Extract provider from Source/Dest Cloud endpoints.
       - Resolve creds via vault using `job.credentials_ref`.
       - Call `client.configure_remote(...)`.
       - Decide mode:
         - Same provider + server-side supported? → `server_side_copy`.
         - Else: use multipart upload/download with size tuning (e.g. chunk_size = min(100MiB, file_size / 1000) or provider defaults).
       - Stream chunks, update `job.checkpoint` periodically + `repo.save`.
       - Return `TransferReport` with `server_side: true` when applicable.
   - Support resume for downloads/uploads via range/offset in multipart.

6. **Transport Selection / Router**:
   - Add simple `TransportRouter` (in application or a new `infrastructure/router.rs`).
   - Logic:
     ```rust
     if source.is_cloud() || dest.is_cloud() {
         RcloneCloudTransport::new(...)
     } else {
         LocalDeltaTransport::new(...)
     }
     ```
   - Update `Daemon::new()` (or make it take a router) and `JobService`.

7. **Endpoint / Job Enhancements** (minimal):
   - Ensure `Endpoint::Cloud` can carry provider-specific hints if needed (currently sufficient).
   - Update `Job` creation in examples to use Cloud variants.

8. **Demo & Verification**:
   - In `main.rs`: add examples using `Source::Cloud` / `Destination::Cloud` (e.g. local to S3, S3 to S3).
   - Note: requires a running Rclone bridge process + valid credentials in the vault (env or file).
   - Add tracing for "server-side copy used", "multipart chunk N", "bytes from cloud".

9. **Documentation & Cleanup**:
   - This document.
   - Update main `architecture.md`, README.md.
   - Add notes in code about the Go bridge requirement.

10. **Future Prep**:
    - Make multipart chunk size policy-driven.
    - Expose more Rclone options via `Job.policy` or a `TransferOptions` extension.
    - Design the delta RPCs for Phase 4 (they can reuse the same client + credential flow).

## Key Risks & Mitigations

- **Go process lifecycle**: Daemon should manage starting/stopping the Rclone bridge (or document external management). Use health checks.
- **Credential security**: Use the vault abstraction; consider zeroize for secrets. Never log creds.
- **Performance**: gRPC overhead is low; for very high throughput, consider connection pooling and streaming.
- **Provider differences**: Rclone normalizes most things; the transport should query capabilities (e.g. "does this provider support server-side copy?") via the bridge if possible.
- **Testing**: Mock the gRPC client for unit tests; integration tests require real (or fake) Rclone bridge + test buckets.

This module completes the "one platform for local, server, cloud..." vision from the blueprint while keeping everything pluggable and transport-blind at the Job level.

---

**Implementation status**: Design complete. Code changes (proto, client, transport, vault, integration) to follow in the working tree. See related Phase 0/1 docs for the base on which this is built.
