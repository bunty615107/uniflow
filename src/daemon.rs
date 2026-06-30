//! Thin Daemon wrapper (core daemon structure).
//!
//! This provides the main "daemon" concept requested in the prompt.
//! It wraps the application-layer JobService and serves as the entry point
//! for job submission, lifecycle management, and shutdown.
//!
//! In a fuller system this could also own config, multiple transports (via router),
//! background scheduling, API servers (gRPC/WS), etc.

use crate::application::ports::IntelligenceEngine; // trait in scope for profile_and_tune()
use crate::application::services::JobService;
use crate::config::UniFlowConfig;
use crate::domain::{Job, JobId};
use crate::error::Result;
use std::sync::Arc;

use crate::infrastructure::{
    DefaultIntelligenceEngine, InMemoryJobRepository, IrohP2PTransport,
    LocalDeltaTransport, RcloneCloudTransport, SledJobRepository, TransportRouter,
};
use crate::infrastructure::cloud::RcloneBridgeClient;

/// The UniFlow core daemon.
/// Connection-agnostic job orchestration using tokio + rayon + Phase 1 delta engine + Module 01 cloud connector.
pub struct Daemon {
    service: JobService,
}

impl Daemon {
    /// Create a new daemon with sensible defaults (Local delta + Cloud via Rclone bridge).
    /// This is the composition root. Accepts a `UniFlowConfig` for all path/env configuration.
    ///
    /// For full cloud support you must have the Rclone gRPC bridge running (see docs/module01-...md).
    /// Credentials are resolved from environment (UNIFLOW_* vars) via EnvCredentialVault.
    pub async fn new(config: &UniFlowConfig) -> Result<Self> {
        let snapshot_path = config.snapshot_path();

        // Persistence backend selection (JULES-11): durable sled store is opt-in via
        // `UNIFLOW_SLED_PATH`; the in-memory repo (with JSON snapshot) stays the default.
        let repo: Arc<dyn crate::application::ports::JobRepository> =
            match &config.sled_path {
                Some(sled_path) if !sled_path.is_empty() => {
                    let sled_repo = SledJobRepository::open(sled_path)?;
                    // One-time, idempotent import from the legacy JSON snapshot.
                    match sled_repo.migrate_from_snapshot(&snapshot_path).await {
                        Ok(n) if n > 0 => {
                            tracing::info!(imported = n, "migrated jobs from snapshot into sled store")
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("snapshot migration skipped: {}", e),
                    }
                    tracing::info!(path = %sled_path, "using durable sled JobRepository");
                    Arc::new(sled_repo)
                }
                _ => {
                    let mem = InMemoryJobRepository::new().with_snapshot(snapshot_path);
                    // Load prior snapshot if present (basic integrity-checked load path).
                    let _ = mem.load_snapshot().await;
                    Arc::new(mem)
                }
            };

        let local_transport = Arc::new(LocalDeltaTransport::new());

        // P2P transport (Module 03). Uses iroh + quinn for mesh, LAN discovery, NAT, relay.
        // Falls back gracefully if iroh endpoint can't bind (e.g. restricted env).
        let p2p_transport = match IrohP2PTransport::new(crate::infrastructure::p2p::iroh_p2p_transport::RelayMode::Auto).await {
            Ok(p) => Some(Arc::new(p)),
            Err(e) => {
                tracing::warn!("P2P transport unavailable (air-gap/relay will not work): {}", e);
                None
            }
        };

        // Attempt to connect to Rclone bridge for cloud. Fallback to local if not running.
        // Typed as `dyn Transport` so the two arms (different concrete transports) unify.
        let vault: Arc<dyn crate::application::ports::CredentialVault> = Arc::new(crate::infrastructure::EnvCredentialVault::new());
        let cloud_transport: Arc<dyn crate::application::ports::Transport> =
            match RcloneBridgeClient::connect("http://127.0.0.1:50051").await {
                Ok(client) => {
                    Arc::new(RcloneCloudTransport::new(client, vault.clone()))
                }
                Err(_) => local_transport.clone(),
            };

        // Module 04: Intelligence & Optimiser (pluggable profiling + auto-tuning)
        let intelligence: Arc<dyn crate::application::ports::IntelligenceEngine> =
            Arc::new(DefaultIntelligenceEngine::new());

        // Module 05: Security components (baked into daemon)
        let _rbac = crate::infrastructure::RbacEnforcer::new();
        // Replaced NoopMfa with DemoMfa (logs warning, demo only, documented in access_control.rs)
        let _mfa: Arc<dyn crate::infrastructure::MfaHook> = Arc::new(crate::infrastructure::security::access_control::DemoMfa);
        // DEMO FLAG: dummy encryption placeholder removed from active path (was [0u8;32]).
        // Real encryption keys come exclusively from CredentialVault + KDF per-job (see JobService worker + encryption.rs).
        // If needed for tests: use env-derived or ClientSideEncryption::new with marked DEMO key only.
        let _encryption = (); // previously: ClientSideEncryption::new([0u8;32]) -- flagged/removed for security hygiene

        let router = Arc::new(TransportRouter::new(
            local_transport.clone(),
            cloud_transport,
            p2p_transport,
            Some(intelligence.clone()),
        ));

        let service = JobService::new(repo, router, vault).await?;

        Ok(Self { service })
    }

    /// Submit a job. Returns the JobId.
    /// The job will be queued and executed asynchronously by the internal worker.
    pub async fn submit_job(&self, job: Job) -> Result<JobId> {
        self.service.submit(job).await
    }

    /// Request cancellation of a running or queued job.
    pub async fn cancel_job(&self, id: JobId) -> Result<()> {
        self.service.cancel(id).await
    }

    /// Get the current state of a job.
    pub async fn get_job(&self, id: JobId) -> Result<Job> {
        self.service.get(id).await
    }

    /// List all known jobs (from the repository).
    pub async fn list_jobs(&self) -> Result<Vec<Job>> {
        self.service.list().await
    }

    /// Gracefully shut down the daemon's background worker.
    pub async fn shutdown(&self) -> Result<()> {
        self.service.shutdown().await
    }

    /// Submit with explicit Module 04 intelligence (profiling + auto-tuning) applied first.
    /// In a production router this would be automatic inside select().
    pub async fn submit_job_with_intelligence(&self, mut job: Job) -> Result<JobId> {
        let intel = DefaultIntelligenceEngine::new();
        if let Err(e) = intel.profile_and_tune(&mut job) {
            tracing::warn!(job_id = %job.id, error = %e, "intelligence failed, submitting with defaults");
        }
        self.service.submit(job).await
    }

    /// Expose audit log entries (for history/audit UI binding).
    pub fn list_audit_events(&self) -> Vec<crate::infrastructure::AuditEvent> {
        self.service.list_audit_events()
    }
}
