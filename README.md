# UniFlow — Phase 0 Core Daemon & Job Model

**Status**: Phase 0 + Phase 1 (Local & On-Prem Delta Engine) complete (runnable skeleton, clean architecture).

This is the Rust implementation of the UniFlow core daemon following a clean architecture (domain / application / infrastructure) and the connection-agnostic design from the UniFlow Master Blueprint (Sections 3, 9, and 13).

## What Phase 0 Delivers

- **Core daemon structure** (`Daemon`) using the approved stack: **Rust + tokio + rayon**.
- **Job model with explicit Source + Destination**: `Job { source: Source, destination: Destination, mode: TransferMode, ... }` (plus Policy, Schedule, full serde). Follows "Source + Destination + Mode".
- **Connection-agnostic execution engine**: Ports (`JobRepository`, `Transport`) in the application layer. Only a `NoopTransport` stub exists in P0 — real transports (LAN, P2P, Cloud, SSH, etc.) can be plugged in without changing the domain or core logic.
- **Job lifecycle management**: create → submit (queued) → run (with checkpoints) → completed / failed / cancelled. Implemented in the application `JobService` and exposed via `Daemon`.
- **Structured logging** with `tracing` + subscriber — rich fields (`job_id`, source/dest labels, mode, status) for auditability.
- **Error handling**: `thiserror` for domain errors + `anyhow` available.
- **Persistable models**: Everything important is `Serialize + Deserialize`. In-memory repository + JSON snapshot demonstrates "Persists transfer state" (ready for RocksDB later).
- **Minimal working example**: `cargo run` wires the layers via the `Daemon` and executes sample noop jobs (one completes, one is cancelled) while printing logs and a serialized `Job`.

## Quick Start (on a machine with Rust installed)

```powershell
cd D:\uniflow
cargo run
```

You should see structured logs, two jobs exercising the full path, and a pretty-printed JSON `Job`.

A snapshot file `uniflow_jobs.snapshot.json` will also be written next to the binary (P0 persist demo).

```bash
# With more logs
RUST_LOG=uniflowd=debug cargo run
```

## Project Layout (Clean Architecture)

```
src/
├── domain/           # Pure models (Source, Destination, Job, Endpoint, Status, etc.)
├── application/
│   ├── ports.rs      # Traits: JobRepository, Transport (connection-agnostic contracts)
│   └── services/     # JobService (lifecycle orchestration + worker)
├── infrastructure/   # Concrete adapters (InMemoryJobRepository, NoopTransport)
├── daemon.rs         # Thin Daemon wrapper (the "core daemon")
├── lib.rs
└── main.rs           # Minimal working example
```

See `docs/architecture.md` for the mermaid diagram and how this foundation enables later phases (real transports, RocksDB, gRPC, etc.).

## Key Types (excerpt)

```rust
pub struct Source(pub Endpoint);
pub struct Destination(pub Endpoint);

pub struct Job {
    pub id: JobId,
    pub source: Source,
    pub destination: Destination,
    pub mode: TransferMode,
    pub policy: Policy,
    ...
}

pub trait Transport: Send + Sync { ... }

pub struct JobService { ... }   // application layer

pub struct Daemon { ... }       // submit_job, cancel_job, get_job, list_jobs, shutdown
```

## Next Phases (not in scope for P0)

- Real `Transport` implementations (LAN, Iroh P2P + QUIC, rclone cloud bridge via tonic gRPC IPC, SSH, etc.)
- `rust-rocksdb` store impl + full resume from checkpoint on daemon restart
- gRPC (tonic) + WebSocket (tungstenite) API server
- Delta (librsync), BLAKE3 integrity, bandwidth controls, workflow chains, etc.
- Tauri desktop, Flutter mobile, Ratatui CLI surfaces

All of the above can be added **without changing the Job model or the core daemon orchestration** thanks to the traits and modularity chosen in Phase 0.

## License / Notes

This is an internal engineering skeleton for the UniFlow project (ICICI Bank context per the blueprint). The architecture strictly follows the P0 requirements and the Section 13 technology mandates from the master document.

For the authoritative requirements and the Source × Destination matrix, refer to the 35-page PDF.
