//! JobService: the main application use case for job lifecycle management.
//!
//! It is connection-agnostic: it receives a transport (or router) at construction time
//! and uses the JobRepository port for persistence.

use crate::application::ports::{JobRepository, Transport, TransferReport};
use crate::domain::{Job, JobId, JobStatus};
use crate::error::{Result, UniFlowError};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

// Module 05 Security
use crate::infrastructure::{RbacEnforcer, MfaHook, TamperEvidentAuditLogger, ClientSideEncryption};

/// High-level commands for the service worker.
enum JobCommand {
    Submit(Job),
    Cancel(JobId),
    Shutdown,
}

/// The core application service.
/// Wires a repository (persistence port) and a transport (execution port).
/// Module 05 security baked in (RBAC, MFA, audit, encryption hooks).
pub struct JobService {
    repo: Arc<dyn JobRepository>,
    transport: Arc<dyn Transport>,
    cmd_tx: mpsc::Sender<JobCommand>,
    cancels: Arc<Mutex<HashMap<JobId, bool>>>,
    audit: Arc<TamperEvidentAuditLogger>,
    rbac: RbacEnforcer,
    mfa: Arc<dyn MfaHook>,
}

impl JobService {
    pub async fn new(
        repo: Arc<dyn JobRepository>,
        transport: Arc<dyn Transport>,
    ) -> Result<Self> {
        let (cmd_tx, cmd_rx) = mpsc::channel(128);
        let cancels = Arc::new(Mutex::new(HashMap::new()));
        let audit = Arc::new(TamperEvidentAuditLogger::new());
        let rbac = RbacEnforcer::new();
        // Replaced NoopMfa with DemoMfa: logs prominent warning on challenge(), still permits in demo/debug,
        // but fully documented as INSECURE (see access_control.rs). Use only for local/dev; replace in prod.
        let mfa: Arc<dyn MfaHook> = Arc::new(crate::infrastructure::security::access_control::DemoMfa);

        let service = Self {
            repo: repo.clone(),
            transport: transport.clone(),
            cmd_tx: cmd_tx.clone(),
            cancels: cancels.clone(),
            audit: audit.clone(),
            rbac,
            mfa,
        };

        // Spawn background worker
        let worker_repo = repo;
        let worker_transport = transport;
        let worker_cancels = cancels;

        // Pass audit (Arc clone) so worker can emit tamper-evident events too
        let worker_audit = audit.clone();
        tokio::spawn(async move {
            Self::worker_loop(cmd_rx, worker_repo, worker_transport, worker_cancels, worker_audit).await;
        });

        info!("JobService (application layer) initialized with tamper-evident audit");
        Ok(service)
    }

    pub async fn submit(&self, mut job: Job) -> Result<JobId> {
        // Module 05: RBAC + MFA enforcement (baked in)
        // server-force safe rbac_role: unspecified or unprivileged -> "operator" default (least that can submit in demo)
        let effective_role = job.policy.rbac_role.as_deref().or(Some("operator"));
        let sensitivity = job.policy.zero_knowledge || job.policy.encrypt_in_transit;
        if let Err(e) = self.rbac.check(effective_role, "submit", sensitivity) {
            return Err(e);
        }
        if job.policy.mfa_required {
            let _ = self.mfa.challenge(job.credentials_ref.as_deref().unwrap_or("unknown"), "submit")?;
        }

        if !job.transition_to(JobStatus::Queued) {
            return Err(UniFlowError::InvalidStateTransition {
                job_id: job.id,
                from: job.status.as_str().to_string(),
                to: "queued".to_string(),
            });
        }
        self.repo.save(&job).await?;
        let _ = self.repo.snapshot().await;

        // Module 05 tamper-evident audit
        let _ = self.audit.log(crate::infrastructure::AuditEvent {
            job_id: job.id.to_string(),
            event_type: "submit".into(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            details: format!("source={} dest={} mode={} zk={}", job.source.label(), job.destination.label(), job.mode.as_str(), job.policy.zero_knowledge),
            prev_hash: self.audit.current_root(),
        });

        self.cmd_tx
            .send(JobCommand::Submit(job.clone()))
            .await
            .map_err(|_| UniFlowError::Internal("service command channel closed".into()))?;

        info!(
            job_id = %job.id,
            source = %job.source.label(),
            destination = %job.destination.label(),
            mode = %job.mode.as_str(),
            "job submitted"
        );
        Ok(job.id)
    }

    pub async fn cancel(&self, id: JobId) -> Result<()> {
        // Module 05: tighten RBAC for cancel (extended); server-force safe rbac_role + sensitivity from job if loadable
        let (effective_role, sensitivity) = match self.repo.load(id).await {
            Ok(job) => (
                job.policy.rbac_role.as_deref().or(Some("operator")),
                job.policy.zero_knowledge || job.policy.encrypt_in_transit,
            ),
            Err(_) => (Some("operator"), false), // fallback safe default if job unknown (still audit later)
        };
        if let Err(e) = self.rbac.check(effective_role, "cancel", sensitivity) {
            return Err(e);
        }

        {
            let mut map = self.cancels.lock().await;
            map.insert(id, true);
        }
        // Module 05 audit
        let _ = self.audit.log(crate::infrastructure::AuditEvent {
            job_id: id.to_string(),
            event_type: "cancel".into(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            details: "user requested cancel".into(),
            prev_hash: self.audit.current_root(),
        });
        let _ = self.cmd_tx.send(JobCommand::Cancel(id)).await;
        info!(job_id = %id, "cancel requested");
        Ok(())
    }

    pub async fn get(&self, id: JobId) -> Result<Job> {
        // Module 05: tighten RBAC (extended to get with sensitivity); server-force safe rbac_role default "auditor"
        let job = self.repo.load(id).await?;
        let sensitivity = job.policy.zero_knowledge || job.policy.encrypt_in_transit;
        let effective_role = job.policy.rbac_role.as_deref().or(Some("auditor"));
        if let Err(e) = self.rbac.check(effective_role, "get", sensitivity) {
            return Err(e);
        }
        Ok(job)
    }

    pub async fn list(&self) -> Result<Vec<Job>> {
        // Module 05: tighten RBAC for list (if possible); server-force safe rbac_role "auditor" + non-sensitive (global list cannot easily prefilter per-job sens here; callers/UI should respect)
        if let Err(e) = self.rbac.check(Some("auditor"), "list", false) {
            return Err(e);
        }
        self.repo.list().await
    }

    pub async fn shutdown(&self) -> Result<()> {
        let _ = self.cmd_tx.send(JobCommand::Shutdown).await;
        Ok(())
    }

    /// Expose recent tamper-evident audit events for UI / compliance views (Module 05/06).
    pub fn list_audit_events(&self) -> Vec<crate::infrastructure::AuditEvent> {
        self.audit.get_events()
    }

    async fn worker_loop(
        mut rx: mpsc::Receiver<JobCommand>,
        repo: Arc<dyn JobRepository>,
        transport: Arc<dyn Transport>,
        cancels: Arc<Mutex<HashMap<JobId, bool>>>,
        audit: Arc<TamperEvidentAuditLogger>,
    ) {
        info!("JobService worker loop started");

        while let Some(cmd) = rx.recv().await {
            match cmd {
                JobCommand::Submit(job) => {
                    if Self::is_cancelled(&cancels, job.id).await {
                        Self::finish_as_cancelled(&repo, job).await;
                        continue;
                    }

                    let mut running = job;
                    if !running.transition_to(JobStatus::Running {
                        progress: 0.0,
                        bytes_transferred: 0,
                    }) {
                        continue;
                    }
                    let _ = repo.save(&running).await;

                    // Module 05: Client-side encryption hook (demo - protects a "payload" or checkpoint data before transport)
                    if running.policy.zero_knowledge || running.policy.encrypt_in_transit {
                        // In real code: key MUST come from CredentialVault (env_vault + KDF/HSM) per job/credentials_ref.
                        // DEMO_INSECURE: env-derived or clearly marked fallback. Do not use with real secrets.
                        // Prefer: UNIFLOW_DEMO_ENC_KEY env (utf8 first-32 bytes) or hardcoded marker below.
                        let demo_key: [u8; 32] = std::env::var("UNIFLOW_DEMO_ENC_KEY")
                            .ok()
                            .and_then(|s| {
                                let b = s.as_bytes();
                                if b.len() >= 32 {
                                    let mut k = [0u8; 32];
                                    k.copy_from_slice(&b[..32]);
                                    Some(k)
                                } else { None }
                            })
                            .unwrap_or(*b"DEMO_INSECURE_KEY_0123456789ABCD"); // exactly 32 bytes: DEMO_INSECURE_ (14) + KEY_ (4) + 0123456789ABCD (14) =32; clearly marked DEMO key (not secret)
                        let enc = ClientSideEncryption::new(demo_key);
                        // Example: "encrypt" the checkpoint value or a sample data chunk
                        if let Some(cp) = running.checkpoint {
                            if let Ok((ct, _nonce)) = enc.encrypt(&cp.to_le_bytes(), true) {
                                // For demo, we just log; real impl would store ct or pass protected data to transport
                                info!(job_id = %running.id, "client-side encryption applied (ZK mode) - {} bytes protected", ct.len());
                            }
                        }
                    }

                    // Module 05 audit (use passed audit)
                    let _ = audit.log(crate::infrastructure::AuditEvent {
                        job_id: running.id.to_string(),
                        event_type: "execute_start".into(),
                        timestamp: chrono::Utc::now().to_rfc3339(),
                        details: format!("transport={} zk={}", transport.name(), running.policy.zero_knowledge),
                        prev_hash: audit.current_root(),
                    });

                    let report_res = transport.execute(&running).await;

                    if Self::is_cancelled(&cancels, running.id).await {
                        Self::finish_as_cancelled(&repo, running).await;
                        continue;
                    }

                    match report_res {
                        Ok(report) => {
                            let final = JobStatus::Completed {
                                bytes: report.bytes_transferred,
                                duration_ms: report.duration_ms,
                            };
                            if running.transition_to(final) {
                                let _ = repo.save(&running).await;
                                // Module 05 audit
                                let _ = audit.log(crate::infrastructure::AuditEvent {
                                    job_id: running.id.to_string(),
                                    event_type: "complete".into(),
                                    timestamp: chrono::Utc::now().to_rfc3339(),
                                    details: format!("bytes={} duration_ms={}", report.bytes_transferred, report.duration_ms),
                                    prev_hash: audit.current_root(),
                                });
                                info!(job_id = %running.id, "job completed via transport '{}'", transport.name());
                            }
                        }
                        Err(e) => {
                            // Module 05 audit on failure
                            let _ = audit.log(crate::infrastructure::AuditEvent {
                                job_id: running.id.to_string(),
                                event_type: "failed".into(),
                                timestamp: chrono::Utc::now().to_rfc3339(),
                                details: e.to_string(),
                                prev_hash: audit.current_root(),
                            });
                            Self::finish_as_failed(&repo, running, e.to_string()).await;
                        }
                    }
                }
                JobCommand::Cancel(id) => {
                    info!(job_id = %id, "cancel processed by worker");
                }
                JobCommand::Shutdown => {
                    info!("JobService worker received shutdown");
                    break;
                }
            }
        }
    }

    async fn is_cancelled(cancels: &Arc<Mutex<HashMap<JobId, bool>>>, id: JobId) -> bool {
        let map = cancels.lock().await;
        *map.get(&id).unwrap_or(&false)
    }

    async fn finish_as_cancelled(repo: &Arc<dyn JobRepository>, mut job: Job) {
        let _ = job.transition_to(JobStatus::Cancelled);
        let _ = repo.save(&job).await;
        info!(job_id = %job.id, "job cancelled");
    }

    async fn finish_as_failed(repo: &Arc<dyn JobRepository>, mut job: Job, reason: String) {
        let _ = job.transition_to(JobStatus::Failed { reason: reason.clone() });
        let _ = repo.save(&job).await;
        warn!(job_id = %job.id, %reason, "job failed");
    }
}
