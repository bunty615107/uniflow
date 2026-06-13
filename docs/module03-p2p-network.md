# UniFlow Module 03: Adaptive P2P Network + Mobile Background Sync Foundation

**Role**: P2P and mobile systems expert.

This module implements the Adaptive P2P Network for direct device-to-device transfers (PC ↔ Mobile, Mobile ↔ Mobile) and lays the foundation for mobile background sync, following the UniFlow Master Blueprint (Module 03, Sections 4, 10, 13).

It builds strictly on the existing clean architecture (Phase 0 core + Phase 1 local delta + Module 01 cloud connector) and the pluggable `Transport` port.

## Architecture Overview

### Integration with Clean Architecture

The design **clearly separates** "P2P transport" (Rust core logic) from "mobile UI/background policy" (platform-specific hooks + Flutter/Dart layer via flutter_rust_bridge).

- **domain/**: Minimal additions. `Endpoint` already supports `Device` variant for mobile. Add lightweight P2P value types (`PeerId`, `DiscoveryInfo`, `PathInfo`) that are serializable and connection-agnostic. No transport or mobile policy logic here.

- **application/ports.rs**:
  - Core `Transport` trait (from Phase 0) is the primary extension point. P2P will provide `IrohP2PTransport` (or equivalent) implementing it.
  - New optional ports for advanced P2P (kept separate):
    - `PeerDiscovery`
    - `NatTraversal` (for explicit control)
  - `CredentialVault` (from cloud) can be reused for any P2P auth if needed.
  - No mobile-specific traits here — mobile policy is outside the core engine.

- **application/services/**:
  - `JobService` and worker loop remain unchanged. They receive a `Transport` (P2P one selected by router).
  - `TransportRouter` (already partially present from cloud) is extended to select P2P for `Device` / mobile endpoints or when LAN/P2P is preferred.

- **infrastructure/**:
  - New `infrastructure/p2p/` module (parallel to `cloud/`, `transfer/`, `delta/`).
    - `iroh_p2p_transport.rs`: Main `impl Transport for IrohP2PTransport`.
    - Helpers for discovery, multi-path, relay.
  - The P2P transport is **pure transport** — it knows nothing about WorkManager or URLSession.
  - Mobile background hooks live in a separate `infrastructure/mobile/` or are provided as FFI-callable Rust functions (for flutter_rust_bridge) + documentation for the native side.

- **daemon.rs**:
  - Uses `TransportRouter` to pick the right transport (P2P, LocalDelta, RcloneCloud, Noop) based on `Source`/`Destination` kinds and policy.
  - P2P selection for mobile/PC endpoints when direct connectivity is viable.

- **lib.rs / main.rs**:
  - Re-exports the P2P transport.
  - Demo can show PC ↔ "Device" jobs.

This keeps the job model (`Source + Destination + Mode`) 100% transport-blind.

### Approved Crates (Strictly Section 13)

- `iroh` (or `iroh-net` + `quinn`) for the P2P mesh, QUIC, built-in discovery and NAT.
- `quinn` (if using raw libp2p; iroh brings it).
- Existing: `tokio`, `rayon` (for any parallel chunking on top of P2P streams), `async-trait`, etc.
- No other crates for the core P2P layer.

`iroh` is preferred because it provides a high-level "magic" endpoint with automatic relay, STUN, hole-punching, and LAN discovery out of the box, while still allowing low-level control.

### NAT Traversal Strategy (per Section 13)

1. **Air-gap / LAN direct (highest priority)**:
   - mDNS / UDP broadcast discovery on local network (iroh supports this via `iroh-net` or custom).
   - Direct QUIC connection (quinn) over LAN IP. No relay, no STUN.
   - Fastest, lowest latency, no internet required. Ideal for "PC ↔ Mobile on same WiFi" or "Mobile ↔ Mobile in same room".

2. **NAT traversal (hole punching)**:
   - Iroh's built-in mechanism: uses STUN to learn public endpoint, then attempts UDP hole punching.
   - QUIC (quinn) over UDP for the data plane — excellent for NATs (connection migration, 0-RTT, multiplexing).

3. **Relay / TURN fallback**:
   - If direct + hole-punch fails (symmetric NAT, strict firewalls, air-gap with no local net), fall back to iroh's relay servers (or custom TURN).
   - Still uses QUIC for the relayed path.
   - Guarantees connectivity at the cost of some latency/bandwidth.

4. **Multi-path**:
   - Iroh/libp2p supports multiple simultaneous paths (direct LAN + relayed + another interface).
   - The transport can stripe chunks or use the fastest path per chunk (or per stream).
   - `TransportReport` can be extended later to report per-path stats.

5. **Discovery**:
   - For PC ↔ Mobile: mDNS on LAN + iroh's "ticket" or node ID exchange (e.g. via QR code or cloud short-lived rendezvous for initial peer ID discovery).
   - For Mobile ↔ Mobile: same, plus proximity (BLE/nearby if needed, but stick to approved for Phase 1).

The strategy always prefers direct/air-gap, then hole-punch, then relay. The `probe()` method on `Transport` can be used by a future intelligence layer to test paths before a job.

### Mobile Background Sync Foundation (Separation of Concerns)

**Core Rule**: The Rust P2P transport (`IrohP2PTransport`) is **platform-agnostic**. It does not know about Android or iOS background restrictions. It just provides reliable send/receive over P2P streams when the process is alive.

Mobile-specific policy and hooks are handled in the UI layer (Flutter) + native wrappers.

**Rust Side (FFI-friendly, for flutter_rust_bridge)**:
- Expose a `MobileP2PBackground` trait or struct with methods:
  - `start_background_sync(job_id: JobId, source: Endpoint, dest: Endpoint)`
  - `stop_background_sync()`
  - Callbacks for progress (`on_progress(bytes: u64, total: u64)`), completion, error.
- These are thin wrappers that spawn the P2P transport in a long-lived task when allowed.
- The actual P2P connection lives in the same Rust runtime.

**Android Side (WorkManager + flutter_rust_bridge)**:
- Use `WorkManager` (or `WorkManager` + `JobScheduler` for Android 8+).
- `UniFlowP2PWorker` (Kotlin/Java) that:
  - Is scheduled as `Expedited` or long-running when user triggers "background send" or on certain events (new files in watched folder).
  - Calls into Rust via flutter_rust_bridge to start the `IrohP2PTransport` for the job.
  - Keeps a foreground service notification if needed for long transfers.
  - Handles constraints (network type = any for P2P LAN, battery not low).
- For Android 8.0+ full support as per matrix; limited background on older.

**iOS Side (URLSession + BGTaskScheduler)**:
- Use `BGTaskScheduler` for background app refresh / processing tasks (iOS 13+; full on iOS 16+ per matrix).
- `UniFlowP2PBackgroundTask` that requests time and calls Rust FFI to run the P2P transfer.
- For downloads/uploads: can use `URLSession` with background configuration for the control plane if needed, but P2P data plane uses the iroh/quinn QUIC socket directly (when the task is running).
- iOS is more restrictive: transfers must be quick or use proper background URLSession; P2P may need to fall back to relay or be user-initiated.

**flutter_rust_bridge Glue**:
- Dart side registers the Rust `MobileP2PBackground` API.
- Native Kotlin/Swift code in the Flutter plugin calls the Dart side or directly invokes the FFI when the OS wakes the background task.
- This keeps Rust pure for the mesh and lets each platform enforce its background execution policy.

**Air-gap on Mobile**: When on LAN, the P2P transport discovers peers locally and transfers directly — no cellular data used, works even if "data saver" is on.

**Relay Fallback**: If no LAN and background task has limited time, the transport can opportunistically use relay for small files or pause/resume across multiple background invocations (using Job checkpoint).

This design satisfies "clearly separate 'P2P transport' from 'mobile UI/background policy'".

## Data Flow Example (PC ↔ Mobile over P2P)

1. User on PC or Mobile creates Job: Source=Local, Dest=Device (or vice versa), Mode=Copy or OneWaySync.
2. `Daemon` / router sees `Device` endpoint → selects `IrohP2PTransport`.
3. Transport performs discovery (mDNS if same LAN, or ticket/node ID).
4. Establishes QUIC connection(s):
   - Direct if possible (air-gap or hole-punched).
   - Or via relay.
5. Multi-path: if multiple paths available, use them concurrently or for different chunks.
6. Delta (if enabled from Phase 1) or full transfer over the P2P streams.
7. Progress reported back through FFI to UI / background task.
8. On mobile: the OS background task keeps the Rust runtime alive long enough; on completion or timeout, checkpoint is saved for next wake-up.
9. Integrity via BLAKE3 (reused from Phase 1 hasher).

For Mobile ↔ Mobile: same, but both sides may be in background tasks.

## Key Rust Structs / Traits (to be implemented)

In `domain/` (lightweight):
- `PeerId` (newtype around iroh `NodeId` or public key).
- `P2PDiscoveryInfo { peer_id: PeerId, addrs: Vec<SocketAddr>, relay_url: Option<Url> }`.
- Extend `Endpoint::Device` with optional peer info.

In `application/ports.rs`:
- Keep `Transport`.
- Add (optional):
  ```rust
  pub trait PeerDiscovery: Send + Sync {
      async fn discover(&self) -> Result<Vec<P2PDiscoveryInfo>>;
  }
  ```

In `infrastructure/p2p/`:
- `IrohP2PTransport { endpoint: iroh::Endpoint, ... }`
- Implements `Transport::execute(job)`.
- Internal: `MultiPathSender`, relay manager, LAN discovery task.

In `infrastructure/`:
- `P2PBackground` trait (for mobile FFI):
  ```rust
  pub trait P2PBackground: Send + Sync {
      fn start_sync(&self, job: Job) -> Result<()>;
      fn stop_sync(&self);
      // progress callbacks via channels or flutter_rust_bridge
  }
  ```

## Implementation Plan (Step by Step)

1. **Documentation & Design** (this file + updates):
   - Create `docs/module03-p2p-network.md`.
   - Update main `architecture.md` and `README.md` with P2P section and mobile notes.

2. **Dependencies** (only approved):
   - Add to `Cargo.toml`:
     ```toml
     iroh = "0.XX"  # pick recent stable that includes quinn, discovery, relay
     # quinn is a dependency of iroh; add explicitly if raw control needed
     quinn = { version = "0.XX", features = ["rustls"] } # if not via iroh
     ```
   - Keep tokio features for UDP/QUIC.

3. **Domain Extensions** (minimal):
   - Add `PeerId`, `P2PDiscoveryInfo` to `domain/models.rs`.
   - Update `Endpoint::Device` to carry optional peer info.
   - Re-export in `domain/mod.rs` and `lib.rs`.

4. **Ports** (keep core clean):
   - Add optional P2P ports (`PeerDiscovery`) to `application/ports.rs`.
   - Export in `lib.rs`.
   - No changes to `Transport` trait.

5. **Core P2P Transport**:
   - Create `src/infrastructure/p2p/mod.rs`, `iroh_p2p_transport.rs`.
   - Implement `IrohP2PTransport`:
     - In `new()`: create `iroh::Endpoint` with default discovery + relay config.
     - `execute()`: 
       - Discover peer from `Destination` (or Source).
       - Establish connection (direct or relayed).
       - If multi-path available, use iroh's multi-path or manually stripe.
       - Stream file data (integrate with Phase 1 delta engine if both sides support it).
       - Use `job.checkpoint` for resume across connections.
       - Update progress and persist checkpoints.
     - `probe()`: return RTT/bandwidth from active paths.
   - Support air-gap: disable relays for discovery on LAN.
   - LAN discovery: rely on iroh's built-in or add mDNS/UDP broadcast helper.

6. **Router & Daemon Integration**:
   - Enhance `TransportRouter` (or create `P2PTransportRouter`) in `infrastructure/transport_router.rs`.
   - Selection logic: if any endpoint is `Device` (mobile) or both are on same LAN-capable, prefer P2P; fallback to relay or other transports.
   - Update `Daemon::new()` to construct `IrohP2PTransport` + router and wire into `JobService` (or make service accept router).
   - Add P2P to the default composition root.

7. **Mobile Background Foundation** (separation enforced):
   - Add `src/infrastructure/mobile/background.rs`:
     - `pub trait MobileP2PBackground { fn start_sync(...) ... }` (stub).
     - FFI-friendly functions (for flutter_rust_bridge).
   - In the design doc: detailed Android WorkManager and iOS URLSession/BGTask integration guide.
   - Do **not** put platform code in Rust core; only the hooks and the pure P2P transport.
   - Document OS matrix support (Android 8.0+, iOS 16+ for reliable background P2P).

8. **Demo & Polish**:
   - Update `main.rs` with example using `Endpoint::Device`.
   - Add tracing for path used (direct vs relay), multi-path stats.
   - Update `infrastructure/mod.rs` and `lib.rs` exports.
   - Add notes in README about building for mobile (flutter_rust_bridge setup).

9. **Testing / Constraints**:
   - Ensure air-gap works (no internet).
   - Relay fallback tested in design.
   - No new non-approved crates.

## Risks & Mitigations

- iroh/libp2p version stability: pin a known good version from Section 13 era.
- Mobile battery/network policy: the separation ensures the Rust layer doesn't fight the OS; the native side decides when to wake the P2P transport.
- Discovery security: use iroh's node IDs + tickets; avoid trusting mDNS alone for sensitive transfers.
- Air-gap UX: provide clear "direct only" mode in the job policy.

This module completes direct P2P for the mobile/PC use cases described in the blueprint while keeping the architecture modular and ready for full HA, managed service, and advanced P2P features in later phases.

---

**Implementation will now proceed in code** (new `infrastructure/p2p/` module, crate updates, router enhancements, mobile stubs, doc links). See the todo list for tracked steps. The design document above serves as the required output for architecture, NAT strategy, and mobile daemon design.