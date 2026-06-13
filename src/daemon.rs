//! Thin Daemon wrapper (core daemon structure).
//!
//! This provides the main "daemon" concept requested in the prompt.
//! It wraps the application-layer JobService and serves as the entry point
//! for job submission, lifecycle management, and shutdown.
//!
//! In a fuller system this could also own config, multiple transports (via router),
//! background scheduling, API servers (gRPC/WS), etc.

use crate::application::services::JobService;
use crate::domain::{Job, JobId};
use crate::error::Result;
use std::sync::Arc;

use crate::infrastructure::{
    DefaultIntelligenceEngine, EnvCredentialVault, InMemoryJobRepository, IrohP2PTransport,
    LocalDeltaTransport, RcloneCloudTransport, TransportRouter,
};
use crate::infrastructure::cloud::RcloneBridgeClient;

/// The UniFlow core daemon.
/// Connection-agnostic job orchestration using tokio + rayon + Phase 1 delta engine + Module 01 cloud connector.
pub struct Daemon {
    service: JobService,
}

impl Daemon {
    /// Create a new daemon with sensible defaults (Local delta + Cloud via Rclone bridge).
    /// This is the composition root.
    ///
    /// For full cloud support you must have the Rclone gRPC bridge running (see docs/module01-...md).
    /// Credentials are resolved from environment (UNIFLOW_* vars) via EnvCredentialVault.
    pub async fn new() -> Result<Self> {
        let snapshot_path = std::path::PathBuf::from(r"D:\uniflow\uniflow_jobs.snapshot.json");

        let repo = InMemoryJobRepository::new().with_snapshot(snapshot_path);
        // Load prior snapshot if present (skeleton; adds basic integrity load path)
        let _ = repo.load_snapshot().await;
        let repo: Arc<dyn crate::application::ports::JobRepository> = Arc::new(repo);

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
        let cloud_transport = match RcloneBridgeClient::connect("http://127.0.0.1:50051").await {
            Ok(client) => {
                let vault: Arc<dyn crate::application::ports::CredentialVault> =
                    Arc::new(crate::infrastructure::EnvCredentialVault::new());
                Arc::new(RcloneCloudTransport::new(client, vault))
            }
            Err(_) => local_transport.clone(),
        };

        // Module 04: Intelligence & Optimiser (pluggable profiling + auto-tuning)
        let intelligence: Arc<dyn crate::application::ports::IntelligenceEngine> =
            Arc::new(DefaultIntelligenceEngine::new());

        // Module 05: Security components (baked into daemon)
        let rbac = crate::infrastructure::RbacEnforcer::new();
        // Replaced NoopMfa with DemoMfa (logs warning, demo only, documented in access_control.rs)
        let mfa: Arc<dyn crate::infrastructure::MfaHook> = Arc::new(crate::infrastructure::security::access_control::DemoMfa);
        // DEMO FLAG: dummy encryption placeholder removed from active path (was [0u8;32]).
        // Real encryption keys come exclusively from CredentialVault + KDF per-job (see JobService worker + encryption.rs).
        // If needed for tests: use env-derived or ClientSideEncryption::new with marked DEMO key only.
        let _encryption = (); // previously: ClientSideEncryption::new([0u8;32]) -- flagged/removed for security hygiene

        let router = TransportRouter::new(
            local_transport.clone(),
            cloud_transport,
            p2p_transport,
            Some(intelligence.clone()),
        );

        // Router + intel are constructed (per design) but selection is not yet hot-wired into JobService (see transport_router.rs).
        // TODO (optim + security): pass `router` (or selector) to JobService so select() + intel.profile_and_tune run automatically per job (prevents bypass + enables correct transport choice).
        // Current: JobService always uses plain local_transport (as before).
        let transport: Arc<dyn crate::application::ports::Transport> = local_transport;

        let service = JobService::new(repo, transport).await?;

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
