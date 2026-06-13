# UniFlow — Working Web Application (UI + Backend Bound + Unit Tests)

## Run (from D:\uniflow)

```pwsh
# 1. Ensure Rust + cargo on PATH (rustup or msvc installer)
cargo --version

# 2. Build + test the whole application (domain + service + security + web handlers)
cargo test --lib

# 3. Run the working web app (starts axum on :7878, seeds 2 real local-delta jobs, serves all Stitch UIs)
cargo run
# (or UNIFLOW_PORT=3000 cargo run)

# 4. Open in browser:
#    http://127.0.0.1:7878/                 (landing + links)
#    http://127.0.0.1:7878/ui/main_dashboard/code.html
#    http://127.0.0.1:7878/ui/job_builder/code.html   (use the injected path editors + INIT button → real POST /api/jobs)
#    http://127.0.0.1:7878/ui/history_audit_log/code.html
#    etc.
```

## What is bound
- All 5+ Stitch retro pages now have live JS that talks to the axum /api (same origin).
- /api/jobs + create use the real `Job` model + `Daemon.submit_job` (RBAC/MFA hooks, encryption if ZK, full worker transitions, LocalDeltaTransport for real file delta when local paths, TamperEvident BLAKE3 audit).
- Dashboards: live Kanban (QUEUED/IN FLIGHT/VERIFY) populated + counts from actual jobs + live audit feed appended.
- Job builder: the "INITIALIZE TRANSFER" button assembles payload (incl. encryption choice → zero_knowledge/encrypt_in_transit policy) and POSTs. Server ensures sample files for demo so LocalDelta always has work.
- History: table shows both jobs + the full append-only tamper-evident events (prev_hash chain visible).
- Detail + endpoint: poll single job + daemon status.
- Seed button / POST /api/seed-demo always available to populate fresh working jobs.

## Unit Tests (cargo test --lib)
- Domain: Job construction, status machine transitions, serde roundtrip (critical for API + snapshot).
- Policy security fields.
- ClientSideEncryption: AES/ChaCha roundtrip.
- TamperEvidentAuditLogger: log produces chain, get_events() returns history, root updates.
- JobService: submit + worker execution (NoopTransport) + cancel + audit emission.
- Web: full Router constructed, GET /, /api/status, POST seed, GET /api/jobs all return expected status codes + exercise daemon path.
- Daemon surface smoke.

All business logic paths exercised without external services (P2P/ rclone optional and gracefully skipped).

## Notes
- For "real" file transfers from UI use local paths that exist (or let the seeded demo + auto-create in /api/jobs handler do the work).
- LocalDeltaTransport performs actual BLAKE3 parallel + (stub) librsync delta + resume + integrity on local src/dst.
- The server is the primary binary mode now (cargo run hosts the full retro intelligence platform).

Built per the UniFlow Master Blueprint (clean arch, pluggable Transport, baked-in security, shared daemon for UIs).
