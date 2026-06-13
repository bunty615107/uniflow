# UniFlow Module 06: Multi-Surface UI Layer (High-Level Plan + Architecture)

**Role**: Security and enterprise compliance architect (also covering UI planning as part of the task).

All UI surfaces share the **same backend daemon** (the connection-agnostic Job orchestration core from Phase 0, enhanced by all subsequent modules). No duplication of logic.

The daemon (src/daemon.rs + lib.rs exposing JobService) is the single source of truth for job lifecycle, intelligence, security, transports (local delta, cloud, P2P), etc.

## Overall Strategy

From blueprint (Section 8 Multi-Surface UI Strategy & Personas, Section 13 frameworks):

- **One daemon, many surfaces**.
- Surfaces: Desktop (Tauri), Mobile (Flutter), CLI/TUI (Ratatui), and headless API consumers.
- Personas: Power users (CLI), everyone (GUI), automation (API), mobile workers (Flutter).
- Enterprise/Banking: Strong auth (via Module 05 security), audit, RBAC enforced at daemon.

Data flow: Surface <-> (local IPC or secure network) <-> Daemon (JobService API).

## Desktop: Tauri 2.0 Architecture

- **Why Tauri 2.0** (Section 13): Rust + WebView. Native desktop <10MB, low RAM (~50MB idle), fast start. Uses existing Rust daemon code directly.
- **Structure**:
  - Rust side (Tauri commands + sidecar or embedded daemon):
    - Embed or spawn the UniFlow daemon (from lib).
    - Expose Tauri commands that map 1:1 to Daemon methods: submit_job, cancel_job, list_jobs, get_job, etc.
    - Security: Use Module 05 (rustls for any remote, client-side encryption hooks if files selected in UI).
    - Hardware/intelligence integration: Call the IntelligenceEngine before submit if UI wants to show "recommended settings".
  - Frontend: Web (HTML/JS/TS + any framework, e.g. React/Tailwind per early blueprint notes). File pickers, job builder (Source/Dest visual per Section 8), dashboard with Kanban status lanes, progress from TransportReport.
  - Communication: Tauri IPC (tauri::command) for local; optional WebSocket for live updates (tungstenite on daemon side if exposed).
- **Packaging**: Tauri bundles daemon binary + web assets. For enterprise, support system service mode (daemon runs as Windows Service / systemd, UI connects via named pipe or localhost TLS).
- **Security**: Surface never sees raw creds (use CredentialVault via daemon). All actions go through RBAC/MFA hooks.
- **Crate decisions**: tauri (2.0), serde (already), tracing (shared with daemon). No heavy web frameworks if possible.

## Mobile: Flutter + flutter_rust_bridge Architecture

- **Why** (Section 13, OS matrices): Single codebase for iOS/Android. flutter_rust_bridge for safe FFI to the Rust core (reuses P2P, encryption, delta logic without duplication).
- **Structure**:
  - Dart/Flutter UI: Job builder (visual Source/Dest pickers, schedule, policy toggles for encryption/zero-knowledge), active jobs list, progress (from stream of events), background sync controls.
  - Rust FFI layer (via flutter_rust_bridge):
    - Expose thin wrappers around Daemon/JobService (or direct Job orchestration).
    - For background: Integrate with Module 03 mobile hooks (WorkManager on Android, URLSession/BGTask on iOS). Rust side provides `MobileP2PBackground` or general sync starter that the native code calls when OS allows.
    - P2P/Mobile modes: Direct calls to IrohP2PTransport when peer discovered (QR or contact list).
  - Communication: Local FFI for embedded daemon; or gRPC/REST if talking to remote/enterprise daemon (secured by Module 05 rustls + client-side crypto).
- **Background Sync** (Section 10, Module 03):
  - Android: WorkManager schedules Rust FFI calls for opportunistic P2P or cloud sync when on WiFi/LAN.
  - iOS: BGTaskScheduler + URLSession for constrained background; Rust handles the actual transfer logic (E2E encrypted).
  - Constraints respected: Battery, network type (prefer direct P2P air-gap to save data).
- **Crate decisions**: flutter_rust_bridge, the core crates (iroh for P2P, blake3, encryption from Module 05). No duplication of job engine.
- **Security**: Flutter UI passes opaque credential refs; all crypto and policy enforcement in Rust daemon/FFI layer. Zero-knowledge works naturally (mobile device holds keys).

## CLI / TUI: Ratatui Architecture

- **Why** (Section 13): Pure Rust TUI (ratatui + crossterm or similar). Scriptable, perfect for power users, CI/CD, headless servers. Matches "full-featured CLI — every GUI action is a command".
- **Structure**:
  - Uses the UniFlow lib directly (no separate daemon process needed, or connects to system daemon).
  - Commands: `uniflow submit --source local:/path --dest cloud:s3:bucket --mode copy --encrypt`, `uniflow list --status running`, `uniflow cancel <job-id>`, interactive TUI for browsing jobs, watching progress (live from channels or polling).
  - TUI screens: Job builder (form with completion for endpoints), dashboard (table + details), audit log viewer (tamper-evident from Module 05).
  - Output: JSON mode for scripting (`--json`), human pretty, or machine (for CI).
- **Integration**: Direct calls to Daemon::new() + submit etc., or client to remote daemon via secure gRPC/WS (using tonic/tungstenite + Module 05 auth).
- **Crate decisions**: ratatui, crossterm (or termion), clap (for CLI parsing, already common), serde_json (existing). Shares all backend crates.
- **Security**: CLI respects RBAC (prompts for MFA if required), uses same CredentialVault. Audit events emitted for every action.

## Shared API Design (REST / gRPC / WebSocket)

The daemon's JobService is the API surface. Expose it consistently for all surfaces and external automation (Section 9 API-first).

- **Core Operations** (from existing JobService + Daemon):
  - submit_job(Job) -> JobId
  - cancel_job(JobId)
  - get_job(JobId) -> Job (with status, checkpoint, profiling/tuning from Modules 04/05)
  - list_jobs(filter) -> Vec<Job>
  - (Future) watch_job(JobId) for streaming progress (TransportReport updates)

- **gRPC (tonic - already used in Module 01 cloud)**:
  - Define .proto for the above (extend the rclone bridge pattern if desired).
  - Strong typing, streaming for progress/audit feeds.
  - Auth: mTLS (rustls from Module 05) or token with RBAC.

- **REST (plan - lightweight)**:
  - Use a small server (e.g., axum or poem if adding; keep minimal or use the gRPC gateway story).
  - Endpoints: POST /jobs, GET /jobs/{id}, DELETE /jobs/{id}, GET /jobs (with query filters for status, labels).
  - JSON bodies matching Job model (serde).
  - OpenAPI spec for enterprise integration.
  - Auth: Bearer tokens or mTLS, mapped to RBAC roles.

- **WebSocket (tungstenite - Section 13)**:
  - For real-time: subscribe to job events, progress (bytes, ETA from intelligence), audit streams.
  - Bidirectional: client can send cancel while watching.
  - Per-job or global feed, filtered by RBAC.

- **Local IPC for Desktop/Mobile**:
  - Tauri commands and flutter_rust_bridge are the "local API".
  - For remote UIs or multi-user: the network APIs above (secured).

- **Security (Module 05 applied here)**:
  - All APIs require auth (RBAC + MFA hooks).
  - Payloads encrypted client-side where appropriate (zero-knowledge).
  - Audit every call.
  - TLS/mTLS for network exposure.
  - Rate limiting / intelligence throttling.

- **Crate Decisions**:
  - Core: Already have tonic (gRPC), serde.
  - WS: tungstenite (or tokio-tungstenite) for WebSocket server in daemon.
  - REST: axum (lightweight, tokio-native) or none if gRPC + WS sufficient (plan only; implement if needed).
  - Docs: utoipa for OpenAPI if REST added.
  - All surfaces reuse the exact same Job/Policy/Endpoint models (no translation layers).

## Multi-Surface Benefits & Enterprise Fit

- **Consistency**: Same job semantics, security (RBAC/MFA/encryption/audit from Module 05), intelligence (Module 04), and engines everywhere.
- **Banking/Enterprise**: Admin uses Ratatui or web dashboard for oversight; operators use Flutter for field work; automation uses gRPC/REST with full audit trail; desktop for power users.
- **Offline/Air-gap**: Mobile + P2P (Module 03) + local encryption works without central daemon connectivity.
- **Extensibility**: New surface? Implement client to the daemon API. New hardware? Handled in intelligence + security.

## Implementation Roadmap (High-Level)

1. Security (Module 05) first — bake into daemon, transports, Job model.
2. Expose/ stabilize the JobService API as the contract.
3. Desktop (Tauri): Quick win — wrap existing daemon in Tauri commands + basic web UI for job submission/monitoring.
4. CLI/TUI (Ratatui): Pure Rust, scriptable version of the same.
5. Mobile: Flutter shell + flutter_rust_bridge bindings to daemon functions + background integration (reuse Module 03 hooks).
6. Network APIs: Add tungstenite WS server in daemon for live updates; optional thin REST or rely on gRPC.
7. Polish: Theming (per Section 8), personas-specific views, audit log UI across surfaces.

All UIs must go through the secure, audited daemon. No direct file system or transport access from UI code.

This fulfills the "one control plane" goal from the blueprint while delivering native experiences on every surface.

---

See Module 05 security doc for how auth/encryption/audit are enforced uniformly across all these surfaces. The daemon is the single trusted boundary.