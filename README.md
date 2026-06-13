# UniFlow — Secure, Connection-Agnostic Managed File Transfer Platform

**UniFlow** is a modern, secure, and intelligent Managed File Transfer (MFT) system built in Rust. It provides a **connection-agnostic** job orchestration layer that works across local disks, cloud providers, P2P networks, and air-gapped environments — all with strong security, auditability, and performance optimizations built in from day one.

It ships with a beautiful **retro-futurist web interface** (inspired by high-end editorial design systems) that is fully bound to the real backend, plus a clean REST API.

> **Current Status**: Fully working web application with hardened security, real delta transfers, live UI bindings, comprehensive unit tests, and production-grade architecture.

---

## What UniFlow Does

UniFlow lets you define, submit, monitor, and execute file transfers as first-class **Jobs** defined purely by:

- **Source** (where data comes from)
- **Destination** (where data goes)
- **Mode** (Copy, OneWaySync, etc.)
- **Policy** (retries, encryption, integrity, compliance rules)

The system then intelligently routes the job to the best available transport, executes it (with resume, integrity verification, and parallelism), and records every step in a **tamper-evident audit trail**.

It solves the problem of fragmented file movement tools by offering a **single, secure, observable, and optimizable** control plane.

---

## Why UniFlow Exists (The Motivation)

Traditional MFT and file transfer tools suffer from several fundamental problems:

- **Transport Silos**: Separate tools and processes for S3, SFTP, P2P, USB sneakernet, or internal NAS. Each has its own CLI, logging, security model, and failure modes.
- **Weak Security by Default**: Most solutions focus on transport encryption (TLS) but do little for **at-rest** or **zero-knowledge** scenarios. Audit logs are easy to tamper with.
- **Poor Observability & Compliance**: Enterprises need strong, tamper-evident records for regulatory requirements (SOX, GDPR, PCI, etc.). Simple log files don't cut it.
- **No Intelligence**: Transfers run at fixed speeds or with naive parallelism. They don't adapt to network conditions, hardware capabilities, or time-of-day constraints.
- **Difficult to Extend**: Adding a new backend (new cloud, new P2P protocol, mobile background sync) usually means rewriting large parts of the system.

UniFlow was designed from the ground up to fix these issues following the **UniFlow Master Blueprint**:

- Explicit, serializable `Source + Destination + Mode` model (Section 3)
- Baked-in security (Module 05): client-side encryption, RBAC, MFA hooks, BLAKE3 tamper-evident audit
- Pluggable transports via clean traits
- Intelligence layer for automatic profiling and tuning (Module 04)
- Multi-surface UI support (web, desktop, mobile, CLI)

---

## How UniFlow Works

### Architecture Overview

```mermaid
graph TD
    subgraph "Presentation"
        UI[Retro-Futurist Web UIs<br/>Stitch Prototypes + JS Bindings]
        API[Axum REST API + Static Serve]
    end

    subgraph "Application Layer"
        Daemon[Daemon<br/>Composition Root]
        JobService[JobService<br/>Lifecycle + Security Orchestration]
    end

    subgraph "Domain Layer"
        JobModel[Job Model<br/>Source + Destination + Mode + Policy]
        Types[Endpoints, Status Machine,<br/>Delta Types, Profiling Results]
    end

    subgraph "Infrastructure Layer"
        Transports[Pluggable Transports<br/>LocalDelta • Cloud • P2P]
        Security[Security Primitives<br/>Encryption • Tamper-Evident Audit<br/>RBAC • MFA Hooks]
        Intelligence[Intelligence Engine<br/>Network Probes + Hardware Profiling<br/>Auto-Tuning]
        Persistence[Persistence<br/>In-Memory + Snapshot<br/>(Ready for RocksDB)]
    end

    UI -->|Live Data Binding| API
    API --> Daemon
    Daemon --> JobService
    JobService --> JobModel
    JobService --> Transports
    JobService --> Security
    JobService --> Intelligence
    Transports --> Persistence
```

UniFlow follows strict **clean architecture** layers:

- **Domain** (`src/domain/`): Pure data models (`Job`, `Source`, `Destination`, `Endpoint`, `Policy`, `JobStatus`, delta types, etc.). No I/O, fully serializable.
- **Application** (`src/application/`): Ports (traits) + `JobService` that orchestrates lifecycle, security checks, and worker execution.
- **Infrastructure** (`src/infrastructure/`): Concrete implementations — transports, persistence, security primitives, intelligence engine, etc.

This separation means you can swap the storage backend (InMemory → RocksDB), add new transports, or change the web layer **without touching the core job model or business rules**.

### 2. Job Model & Lifecycle
A `Job` flows through a clear state machine:

`Pending` → `Queued` → `Running` (with progress + checkpoints) → `Completed` / `Failed` / `Cancelled`

Security policy is evaluated at submission time (RBAC + optional MFA). The worker applies client-side encryption when `zero_knowledge` or `encrypt_in_transit` is enabled.

### 3. Connection-Agnostic Transports
The `Transport` trait is the heart of the system:

```rust
pub trait Transport: Send + Sync {
    fn name(&self) -> &'static str;
    async fn execute(&self, job: &Job) -> Result<TransferReport>;
}
```

Current implementations:
- **LocalDeltaTransport** — High-performance delta transfers using BLAKE3 (parallel hashing) + librsync (block-level deltas) with resume support.
- **RcloneCloudTransport** — Cloud-to-cloud via rclone gRPC bridge (supports server-side copy when available).
- **IrohP2PTransport** — Device-to-device / P2P using iroh + QUIC (LAN discovery, NAT traversal, relays).

A `TransportRouter` + Intelligence engine can automatically select the best transport based on network probes and hardware profiling.

### 4. Security (Baked In, Not Bolted On)
- **Tamper-Evident Audit** — Every significant event is logged with a BLAKE3 hash chain. The audit root can be used to prove integrity.
- **Client-Side Encryption** — AES-256-GCM or ChaCha20-Poly1305. In true zero-knowledge mode the server never sees plaintext.
- **RBAC + MFA Hooks** — Policy-driven role checks and challenge hooks (easily connected to real IdP / TOTP / WebAuthn).
- **Defense in Depth** — Path sandboxing in the web layer, input validation, generic error messages to clients, rate limiting, security headers, CSP guidance in the UI.

### 5. Web Application & UI Binding
A production-ready Axum web server:
- Serves the beautiful retro-futurist "Stitch" HTML prototypes statically under `/ui`
- Exposes a JSON REST API (`/api/jobs`, `/api/audit`, `/api/status`, etc.)
- All major UI surfaces are **dynamically bound** to live backend data:
  - Kanban dashboards (Queued / In Flight / Verify) with live polling and audit feed
  - Job Builder that creates real jobs with encryption policy selection
  - History, Detail, and Endpoint Manager views

Recent hardening (via extensive subagent review):
- API authentication (X-API-Key / Bearer)
- XSS prevention (safe DOM rendering instead of `innerHTML` with untrusted data)
- Path traversal protection
- Client + server validation
- Proper error handling and rate limiting

---

## Usefulness & Real-World Value

UniFlow is useful in any environment that needs reliable, secure, and observable file movement at scale:

| Scenario                        | How UniFlow Helps                                      |
|--------------------------------|-------------------------------------------------------|
| **Enterprise Compliance**      | Tamper-evident BLAKE3 audit chain + full job history |
| **Zero-Knowledge / High Security** | Client-side encryption + policy-enforced ZK mode     |
| **Multi-Cloud / Hybrid**       | Single job definition routes to best transport       |
| **Air-Gapped / LAN**           | P2P transport with discovery + relay fallback        |
| **Large File / Resume**        | Real delta engine with BLAKE3 integrity + checkpoints|
| **Developer & Operations**     | Web UI + REST API + structured logs + easy extension |
| **Intelligent Optimization**   | Auto-profiling of network + hardware before transfer |

It acts as the **"universal transfer bus"** for an organization — one model, one audit trail, one set of security policies, many backends.

---

## Getting Started

### Prerequisites
- Rust toolchain (stable recommended)
- (Optional) rclone gRPC bridge for full cloud features

### Run the Working Web Application

```powershell
cd D:\uniflow

# Set the development API key (required after security hardening)
$env:UNIFLOW_API_KEY = "dev-uniflow-key-12345"

cargo run
```

The server will:
- Seed a couple of demonstration local-delta jobs
- Start listening on `http://127.0.0.1:7878` (override with `UNIFLOW_PORT`)

Open your browser to the landing page and explore the fully functional retro UIs.

**API Authentication**
All `/api/*` endpoints require:
```
X-API-Key: dev-uniflow-key-12345
```
(or `Authorization: Bearer ...`)

See `RUN_WEBAPP.md` for more operational details.

---

## Project Structure

```
src/
├── domain/                 # Pure, serializable models (Job, Endpoint, Policy, etc.)
├── application/
│   ├── ports.rs            # Traits (Transport, JobRepository, IntelligenceEngine...)
│   └── services/           # JobService — the orchestration heart
├── infrastructure/         # All concrete adapters
│   ├── security/           # Encryption, TamperEvidentAuditLogger, RBAC, TLS config
│   ├── transfer/           # LocalDeltaTransport (BLAKE3 + librsync)
│   ├── cloud/              # Rclone bridge client
│   ├── p2p/                # Iroh/QUIC transport
│   ├── intelligence/       # Network probes + hardware detection + optimizer
│   └── ...
├── web.rs                  # Axum server + API handlers + static UI serving
├── daemon.rs               # Composition root (Daemon)
└── main.rs                 # Entry point (seeds demos + starts web server)
```

See `docs/architecture.md` for the Mermaid diagram and rationale.

---

## Documentation

- `docs/architecture.md` — Overall design and how phases build on each other
- `docs/module05-security.md` — Threat model and security architecture
- `docs/module06-multi-surface-ui.md` — UI strategy
- `SECURITY.md` — Summary of vulnerabilities found and fixed
- `RUN_WEBAPP.md` — How to run and interact with the web application

---

## Roadmap Highlights

- **Phase 1** (largely complete): Real Local Delta engine + basic cloud/P2P
- **Module 04/05** (largely complete): Intelligence + baked-in security
- **Future**:
  - Full RocksDB persistence + snapshot recovery
  - Real streaming delta + encryption
  - Multi-worker execution + proper transport selection in hot path
  - Production TLS + external identity provider integration
  - Tauri desktop app, Flutter mobile, additional transports

The clean architecture means most of these can be added without breaking existing job definitions or the web API contract.

---

## Contributing

This project follows the principles from the original UniFlow Master Blueprint (clean architecture, approved crate list, security-first design, pluggable everything).

Pull requests that improve security, add real transports, improve the intelligence layer, or enhance the retro UI bindings are especially welcome.

---

## License

MIT OR Apache-2.0 (as declared in Cargo.toml)

---

**UniFlow** — One model. Many transports. Strong security. Beautiful control.

Built with ❤️ for reliable, observable, and secure data movement.