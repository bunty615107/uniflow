# UniFlow Module 05: Security Layer

**Role**: Security and enterprise compliance architect.

Security is baked into the core from day one, not bolted on. It protects the connection-agnostic job engine (Source + Destination + Mode from Phase 0), all transfer engines (delta, cloud, P2P), intelligence, and shared daemon.

All surfaces (Module 06) share this secure backend daemon.

## Security Architecture

### Layered Security Model (Clean Architecture Integration)

- **domain/**: Pure data models with security metadata.
  - Extend `Policy` with security controls (already has some encrypt_* from early work).
  - Add fields for zero-knowledge, RBAC role, MFA required, audit level, encryption keys refs (opaque).
  - `Job` carries these; models remain serializable.

- **application/ports.rs**: Contracts and hooks (baked-in).
  - `CredentialVault` (existing from cloud module) extended for zero-knowledge (client-only decryption) and MFA challenge/response.
  - New `SecurityPolicyEnforcer` trait or hooks.
  - `TamperEvidentAudit` port.
  - RBAC/MFA as traits with default no-op for simple deployments, pluggable for enterprise.
  - `Transport` executions are wrapped by security (encryption, auth checks).

- **application/services/job_service.rs**:
  - Before/after every lifecycle transition (submit, execute, checkpoint, complete), emit tamper-evident audit events.
  - Enforce policy (e.g., if zero_knowledge, ensure no plaintext in logs or server-side).
  - RBAC checks on submit/cancel (via hook).

- **infrastructure/security/** (new dedicated module, pluggable):
  - **EncryptionEngine**: Client-side (end-to-end) encryption for data at rest/transit using AES-256-GCM and/or ChaCha20-Poly1305 (per Section 13). Supports streaming for large files/deltas.
  - **TlsProvider**: rustls-based TLS 1.3 with PFS for any controlled network paths (gRPC control plane, future API servers). P2P (iroh) and cloud (rclone bridge) already have strong crypto; we add rustls where we own the stack and E2E client-side on top for zero-knowledge.
  - **AuditLogger**: Tamper-evident implementation using chained BLAKE3 hashes (already have blake3 crate) + optional signatures. Events are append-only, verifiable.
  - **AccessControl**: RBAC implementation + MFA hook trait (e.g., TOTP or WebAuthn challenge). Integrates with CredentialVault.
  - **ZeroKnowledgeMode**: When enabled, daemon never sees plaintext data or long-term keys; only encrypted blobs and public metadata.

- **infrastructure/ (transports & others)**:
  - Existing transports (LocalDelta, RcloneCloud, IrohP2P) are wrapped or composed with security components.
  - Data is encrypted client-side before handing to transport (zero-knowledge even if transport is "trusted").
  - For local: encrypt before writing to disk (at-rest).
  - For network: E2E + transport TLS.
  - Credential vault used for key material (zero-knowledge: keys derived/stored only on client side where possible).

- **daemon.rs**:
  - Secure initialization: loads rustls certs, initializes audit logger, enforces global security policy.
  - Exposes secure API surface (the JobService methods become the basis for authenticated/authorized calls from UIs).

- **Cross-cutting**:
  - Structured logging with tracing already emits job_id, source/dest labels, etc. Security layer adds event_type, hash_chain, user/role, explanation.
  - All sensitive operations (key handling, decryption) use zeroize where possible (add zeroize crate if needed).

### Threat Model (High-Level, per Enterprise/Banking Use Cases)

**Assets**:
- File data (confidentiality, integrity).
- Job metadata and audit logs (tamper-evidence, non-repudiation).
- Credentials and encryption keys (zero-knowledge goal: server learns as little as possible).
- Control plane (daemon API).

**Threat Actors**:
- External network attacker (MITM, replay, DoS).
- Malicious/compromised insider or multi-tenant user (RBAC bypass, log tampering, data exfil).
- Compromised daemon/server (zero-knowledge mitigates: can't read data without client keys).
- Supply chain / malicious transport (E2E encryption + verification).
- Mobile device theft/loss (client-side encryption + secure enclave where possible).

**Key Mitigations (Baked In)**:
- **Confidentiality**: TLS 1.3 (PFS via rustls) for control + E2E client-side encryption (AES-256-GCM/ChaCha20) for payload. Zero-knowledge mode: daemon stores only ciphertext + metadata.
- **Integrity**: BLAKE3 (existing) for hashes in delta/manifest + tamper-evident audit chain (hash-linked logs). Post-transfer verification (from Phase 1 intelligence/policy).
- **Authentication/Authorization**: CredentialVault + RBAC (roles like admin/operator/auditor). MFA hooks (challenge before privileged actions like submit high-sensitivity job).
- **Audit/Non-repudiation**: Tamper-evident logger (every submit/execute/cancel/checkpoint event is hashed and chained; verifiable proof). Signed events where possible.
- **Availability**: Existing retry, checkpoint/resume (Phases 1+). Rate limiting/throttling via intelligence (Module 04).
- **Air-gap/P2P specific**: Direct LAN paths bypass some network threats; relay still protected by QUIC + E2E.
- **Mobile**: Background tasks inherit security (encrypted transfers, credential isolation via platform keychain).

**Residual Risks & Assumptions**:
- Client device compromise (user responsibility; recommend hardware-backed keys).
- Side-channel on encryption (use constant-time impls from ring/RustCrypto).
- Audit log exfil (access control on logs).
- For banking: compliance (e.g., data residency via endpoint choices + policy).

This aligns with Section 9 Security, Identity & Compliance table (enterprise identity, MFA, RBAC, in-transit/at-rest encryption, key mgmt, compliance audit log).

### Crate Decisions (Section 13 Approved + Minimal)

- **TLS**: `rustls` (memory-safe, no OpenSSL baggage) + `rustls-pemfile` or `rustls-native-certs` for cert loading. Features for TLS 1.3 + ring for crypto backend.
- **Client-side Encryption**: `aes-gcm` (for AES-256-GCM) and/or `chacha20poly1305`. Or `ring` for unified (matches "ring or RustCrypto"). Streaming support for large files.
- **Tamper-evident Audit**: Leverage existing `blake3` (hash chain on log entries: prev_hash || event || timestamp). Optional `ed25519` or ring for signatures on audit roots.
- **Zeroize**: `zeroize` crate for sensitive buffers/keys (recommended for security crates).
- **Existing synergies**: blake3 (already), tracing (for audit events), serde (for signed events if needed).
- Avoid: openssl, unmaintained crypto.

All crypto is "baked in" at the infrastructure layer and composed into transports.

### Integration with Existing Codebase

- **Policy/Job**: Extend to carry `zero_knowledge: bool`, `rbac_role: Option<String>`, `mfa_token_ref: Option<String>`, `audit_level: String` ("none" | "standard" | "tamper_evident").
- **CredentialVault**: Enhance `resolve` to support MFA challenges and return zero-knowledge wrapped creds (e.g., client-encrypted keys).
- **Transports**: In execute(), before/after data movement:
  - If policy.encrypt_in_transit or client-side: wrap payload with EncryptionEngine.
  - Log to AuditLogger on start/complete/fail.
  - For network transports: ensure TLS/rustls is used/configured.
- **JobService worker**: Wrap state transitions with audit events. Enforce RBAC before submit/cancel.
- **Daemon**: Init security components (rustls config, audit logger, access control). Support secure local socket or TLS for remote UIs.
- **Intelligence (Module 04)**: Can influence security (e.g., stronger encryption on weak networks?).
- **P2P/Cloud/Local**: E2E encryption is transport-agnostic layer on top.

Zero-knowledge: when enabled, the daemon (and any server-side rclone/relay) only ever handles ciphertext. Key material stays with client (or derived via user password + KDF).

### Implementation Notes

- Start with client-side encryption + audit in infrastructure/security.
- Wire rustls into any new API surface (Module 06) and control the gRPC bridge if possible.
- MFA as async hook (e.g., `mfa_challenge(role) -> Result<token>`).
- Tamper-evident: simple hash chain + periodic root hash publication/verification.
- Testing: property-based for crypto roundtrips, audit chain verification tests.

This module ensures UniFlow meets enterprise/banking compliance (Section 9) while remaining usable for individuals (disable via policy).

---

See also the overall architecture doc and previous modules for how security composes with the connection-agnostic core.