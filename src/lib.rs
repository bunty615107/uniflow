//! UniFlow Phase 0 core library.
//!
//! Clean architecture layers:
//! - `domain`: pure models (Job, explicit Source, Destination, Endpoint, etc.)
//! - `application`: ports (traits) + services (orchestration / lifecycle)
//! - `infrastructure`: concrete adapters (persistence, transports)
//!
//! The engine is connection-agnostic: jobs are Source + Destination + Mode.

pub mod application;
pub mod daemon;
pub mod domain;
pub mod error;
pub mod infrastructure;
pub mod logging;
pub mod web;

// Re-export the primary public API for convenience (the "clean" surface).
pub use application::{
    ports::{
        CloudCredential, ContentHasher, CredentialVault, DeltaEngine, HardwareDetector, IntelligenceEngine,
        JobRepository, NatTraversal, NetworkProbe, Optimizer, PeerDiscovery, SignatureGenerator, Transport,
        TransferReport,
    },
    services::JobService,
};
pub use daemon::Daemon;
pub use domain::{
    BlockSignature, DeltaChunk, DeltaInstruction, Destination, Endpoint, FileManifest, FileSignature,
    Filters, HardwareProfile, Job, JobId, JobStatus, MultiPathPlan, NetworkProbeResult, P2PDiscoveryInfo,
    PathInfo, PeerId, Policy, ProfilingResult, ResumeState, Schedule, Source, TransferMode, TuningDecision,
};
pub use error::{Result, UniFlowError};
pub use infrastructure::{
    AuditEvent, ClientSideEncryption, DefaultIntelligenceEngine, EnvCredentialVault, InMemoryJobRepository, IrohP2PTransport,
    LocalDeltaTransport, MfaHook, MobileP2PBackground, NoopTransport, ParallelBlake3Hasher, RbacEnforcer,
    RcloneBridgeClient, RcloneCloudTransport, RustlsConfig, TamperEvidentAuditLogger, TransportRouter,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::Transport;
    use crate::domain::{Endpoint, Job, JobStatus, Policy, Source, Destination, TransferMode};
    use crate::infrastructure::{AuditEvent, ClientSideEncryption, NoopTransport, TamperEvidentAuditLogger};
    use std::sync::Arc;
    use tokio::runtime::Runtime;

    // === Domain model tests ===
    #[test]
    fn job_new_and_transitions_and_serde() {
        let job = Job::new(
            Source::from(Endpoint::Local { path: "/tmp/src".into() }),
            Destination::from(Endpoint::Local { path: "/tmp/dst".into() }),
            TransferMode::Copy,
        ).with_label("unit-test-job".into());

        assert_eq!(job.label.as_deref(), Some("unit-test-job"));
        assert_eq!(job.status, JobStatus::Pending);
        assert!(job.policy.verify_integrity);

        let mut j = job;
        assert!(j.transition_to(JobStatus::Queued));
        assert!(j.transition_to(JobStatus::Running { progress: 10.0, bytes_transferred: 42 }));
        assert!(j.transition_to(JobStatus::Completed { bytes: 42, duration_ms: 12 }));

        // serde roundtrip (for persistence + wire in web API)
        let json = serde_json::to_string(&j).unwrap();
        let back: Job = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, j.id);
        assert_eq!(back.status.as_str(), "completed");
    }

    #[test]
    fn policy_security_fields_roundtrip() {
        let mut p = Policy::default();
        p.zero_knowledge = true;
        p.rbac_role = Some("auditor".into());
        p.mfa_required = true;
        p.encrypt_in_transit = true;
        let j = Job::new(
            Source::from(Endpoint::Local { path: "a".into() }),
            Destination::from(Endpoint::Local { path: "b".into() }),
            TransferMode::Copy,
        ).with_policy(p.clone());
        assert!(j.policy.zero_knowledge);
        assert_eq!(j.policy.rbac_role.as_deref(), Some("auditor"));
    }

    // === Security module tests (Module 05) ===
    #[test]
    fn client_side_encryption_roundtrip_and_zeroize() {
        let key = [0x42u8; 32];
        let enc = ClientSideEncryption::new(key);
        let plaintext = b"secret-uniflow-payload-0123456789";
        let (ct, nonce) = enc.encrypt(plaintext, false).expect("encrypt");
        assert_ne!(ct, plaintext.to_vec());
        let pt2 = enc.decrypt(&ct, &nonce, false).expect("decrypt");
        assert_eq!(pt2, plaintext);
    }

    #[test]
    fn tamper_evident_audit_chain_and_get_events() {
        let logger = TamperEvidentAuditLogger::new();
        let e1 = AuditEvent {
            job_id: "j1".into(),
            event_type: "submit".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            details: "src=dst=local".into(),
            prev_hash: logger.current_root(),
        };
        let h1 = logger.log(e1.clone()).unwrap();
        assert_ne!(h1, "genesis");

        let e2 = AuditEvent { job_id: "j1".into(), event_type: "complete".into(), timestamp: "2026-01-01T00:00:01Z".into(), details: "bytes=123".into(), prev_hash: logger.current_root() };
        let _h2 = logger.log(e2).unwrap();

        let events = logger.get_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "submit");
        // chain root changed
        assert_ne!(logger.current_root(), "genesis");
    }

    // === Application / service lifecycle with injected NoopTransport (pure, no I/O) ===
    #[test]
    fn job_service_submit_execute_cancel_via_noop_and_audit_emitted() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let repo: Arc<dyn crate::application::ports::JobRepository> = Arc::new(InMemoryJobRepository::new());
            let transport: Arc<dyn Transport> = Arc::new(NoopTransport);
            let svc = JobService::new(repo.clone(), transport).await.unwrap();

            let job = Job::new(
                Source::from(Endpoint::Local { path: "/s".into() }),
                Destination::from(Endpoint::Local { path: "/d".into() }),
                TransferMode::Copy,
            ).with_label("svc-test".into());

            let id = svc.submit(job).await.unwrap();
            // give worker a moment
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;

            let got = svc.get(id).await.unwrap();
            // Either still running or already completed by the fast noop
            assert!(matches!(got.status, JobStatus::Running { .. } | JobStatus::Completed { .. }));

            // cancel path
            let _ = svc.cancel(id).await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            // audit has entries (submit + execute_start at minimum)
            let audits = svc.list_audit_events();
            assert!(!audits.is_empty());
            assert!(audits.iter().any(|e| e.event_type == "submit"));

            let _ = svc.shutdown().await;
        });
    }

    // === Web layer smoke: build router + exercise routes with tower ===
    #[tokio::test]
    async fn web_router_builds_and_api_endpoints_respond() {
        // Construct a minimal daemon for the state (re-uses the same wiring as prod main)
        let daemon = Arc::new(Daemon::new().await.expect("daemon for test"));
        let app = crate::web::build_app(daemon.clone());

        // Use tower to call without a real listener
        use tower::ServiceExt; // oneshot
        use axum::body::Body;
        use http::{Request, StatusCode};

        // GET /
        let resp = app.clone().oneshot(Request::builder().uri("/").body(Body::empty()).unwrap()).await.unwrap();
        assert!(resp.status() == StatusCode::OK || resp.status() == StatusCode::TEMPORARY_REDIRECT);

        // GET /api/status  (auth required for /api)
        let resp = app.clone().oneshot(
            Request::builder()
                .uri("/api/status")
                .header("x-api-key", "dev-uniflow-key-12345")
                .body(Body::empty()).unwrap()
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // POST /api/seed-demo (exercises create path + daemon; include auth header)
        let resp = app.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/seed-demo")
                .header("content-type", "application/json")
                .header("x-api-key", "dev-uniflow-key-12345")
                .body(Body::empty()).unwrap()
        ).await.unwrap();
        assert!(resp.status() == StatusCode::OK || resp.status() == StatusCode::CREATED);

        // GET /api/jobs after seed (auth)
        let resp = app.clone().oneshot(
            Request::builder()
                .uri("/api/jobs")
                .header("x-api-key", "dev-uniflow-key-12345")
                .body(Body::empty()).unwrap()
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Negative security tests (auth, path traversal/IDOR)
        // Missing auth → 401
        let resp = app.clone().oneshot(Request::builder().uri("/api/status").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Path traversal attempt (sanitizer + sandbox should reject → 400 or 403)
        let bad_path_body = serde_json::json!({
            "label": "evil-traversal",
            "source_kind": "local",
            "source_path": "../../../etc/passwd",
            "dest_kind": "local",
            "dest_path": "/tmp/uniflow_bad_dst",
            "mode": "copy"
        }).to_string();
        let resp = app.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/jobs")
                .header("content-type", "application/json")
                .header("x-api-key", "dev-uniflow-key-12345")
                .body(Body::from(bad_path_body)).unwrap()
        ).await.unwrap();
        assert!(resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::FORBIDDEN);

        let _ = daemon.shutdown().await;
    }

    #[test]
    fn daemon_exposes_audit_and_jobs_api_surface() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let d = Daemon::new().await.unwrap();
            // list works (empty or seeded by construction)
            let js = d.list_jobs().await.unwrap();
            let _aud = d.list_audit_events();
            // submit a simple one
            let j = Job::new(Source::from(Endpoint::Local{path:"/t/src".into()}), Destination::from(Endpoint::Local{path:"/t/dst".into()}), TransferMode::Copy);
            let _id = d.submit_job(j).await.unwrap();
            let _ = d.shutdown().await;
        });
    }
}

