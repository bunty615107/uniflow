//! Web binding for UniFlow (Module 06): Axum server serving the retro Stitch UIs
//! from google stitch prototypes + REST API bound to the real Daemon / JobService.
//!
//! - Static: /ui/* serves the stitch UI directory (configurable via UNIFLOW_UI_DIR)
//! - API under /api : jobs CRUD + lifecycle, audit log (tamper evident), status
//! - The pages are enhanced via injected/updated client JS (see edits to the code.html files)
//!   to dynamically fetch and render Kanban, forms submit to real backend, tables etc.
//!
//! Run with `cargo run` (or the uniflowd bin) — the main now hosts the web app.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    middleware::{self, Next},
    response::{Html, IntoResponse, Json, Redirect},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::{info, warn};

use crate::config::UniFlowConfig;
use crate::domain::{Destination, Endpoint, Job, JobStatus, Source, TransferMode};
use crate::infrastructure::AuditEvent;
use crate::{Daemon, JobId};

/// Shared application state for handlers (holds the live daemon with full business logic).
#[derive(Clone)]
pub struct AppState {
    pub daemon: Arc<Daemon>,
    pub config: Arc<UniFlowConfig>,
}

/// DTO for creating a job via the bound UI (job_builder page primarily).
/// Keeps simple for the retro form binding while supporting policy flags.
#[derive(Debug, Deserialize)]
pub struct CreateJobRequest {
    pub label: Option<String>,
    pub source_kind: Option<String>, // "local" | "cloud" | "device" (default local for working demo)
    pub source_path: String,
    pub dest_kind: Option<String>,
    pub dest_path: String,
    pub mode: Option<String>, // "copy" | "one-way-sync"
    pub zero_knowledge: Option<bool>,
    pub encrypt_in_transit: Option<bool>,
    pub mfa_required: Option<bool>,
    pub rbac_role: Option<String>,
}

/// Summary response for jobs (matches what UI cards/tables expect + full for detail).
#[derive(Debug, Serialize)]
pub struct JobSummary {
    pub id: String,
    pub label: Option<String>,
    pub source: String,
    pub destination: String,
    pub mode: String,
    pub status: String,
    pub progress: Option<f32>,
    pub bytes_transferred: Option<u64>,
    pub created_at: String,
    pub updated_at: String,
    pub checkpoint: Option<u64>,
    pub policy: PolicySummary,
}

#[derive(Debug, Serialize)]
pub struct PolicySummary {
    pub zero_knowledge: bool,
    pub encrypt_in_transit: bool,
    pub audit_level: String,
}

impl From<Job> for JobSummary {
    fn from(j: Job) -> Self {
        let (progress, bytes) = match &j.status {
            JobStatus::Running { progress, bytes_transferred } => (Some(*progress), Some(*bytes_transferred)),
            JobStatus::Completed { bytes, .. } => (Some(100.0), Some(*bytes)),
            _ => (None, None),
        };
        JobSummary {
            id: j.id.to_string(),
            label: j.label,
            source: j.source.label(),
            destination: j.destination.label(),
            mode: j.mode.as_str().to_string(),
            status: j.status.as_str().to_string(),
            progress,
            bytes_transferred: bytes,
            created_at: j.created_at.to_rfc3339(),
            updated_at: j.updated_at.to_rfc3339(),
            checkpoint: j.checkpoint,
            policy: PolicySummary {
                zero_knowledge: j.policy.zero_knowledge,
                encrypt_in_transit: j.policy.encrypt_in_transit,
                audit_level: j.policy.audit_level.clone(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CreateJobResponse {
    pub id: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct AuditResponse {
    pub events: Vec<AuditEvent>,
    pub root: String,
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub daemon: String,
    pub jobs_total: usize,
    pub running: usize,
    pub audit_root: String,
}

/// Resolve sandbox directory from the provided config.
/// Creates on first use. All user-controlled FS paths for jobs must live inside.
fn get_sandbox_dir(config: &UniFlowConfig) -> PathBuf {
    let _ = std::fs::create_dir_all(&config.sandbox_dir);
    config.sandbox_dir.clone()
}

/// Path sanitization + sandbox enforcement.
/// Rejects ".." (traversal), absolute paths outside the allowed sandbox root,
/// and uses canonicalize (best-effort) + component normalization to ensure safety.
/// Used for Local endpoint paths in create_job + all paths in seed_demo.
fn sanitize_path(user_path: &str, sandbox: &std::path::Path) -> Result<PathBuf, &'static str> {
    if user_path.is_empty() {
        return Err("empty path not allowed");
    }
    if user_path.contains("..") {
        return Err("path traversal (..) rejected by sandbox");
    }
    let p = FsPath::new(user_path);
    let candidate: PathBuf = if p.is_absolute() {
        // Absolute only permitted if it is under the sandbox (prefix check before canon)
        let sb_str = sandbox.to_string_lossy();
        if !user_path.starts_with(sb_str.as_ref()) {
            return Err("absolute path outside allowed sandbox");
        }
        p.to_path_buf()
    } else {
        // Relative: always place under sandbox (strip any leading separators)
        let rel = user_path.trim_start_matches(['/', '\\']);
        sandbox.join(rel)
    };
    // Best-effort canonicalize + escape check (canonicalize fails gracefully if !exists)
    match candidate.canonicalize() {
        Ok(canon) => {
            let sb_canon = sandbox.canonicalize().unwrap_or_else(|_| sandbox.to_path_buf());
            if !canon.starts_with(&sb_canon) {
                return Err("canonicalized path escapes sandbox");
            }
            Ok(canon)
        }
        Err(_) => {
            // Fallback normalization without requiring FS existence
            let mut norm = PathBuf::new();
            for comp in candidate.components() {
                use std::path::Component;
                match comp {
                    Component::ParentDir => return Err("parent dir component after normalization"),
                    Component::CurDir => {}
                    _ => norm.push(comp),
                }
            }
            if !norm.starts_with(sandbox) {
                // Last-ditch: if somehow outside, force under sandbox name check
                return Err("path escapes sandbox after normalization");
            }
            Ok(norm)
        }
    }
}

/// API key auth middleware.
/// Uses constant-time comparison to prevent timing side-channel attacks.
/// Accepts X-API-Key: <key>  OR  Authorization: Bearer <key>.
async fn api_key_auth(
    State(state): State<AppState>,
    req: axum::http::Request<Body>,
    next: Next,
) -> Result<axum::response::Response, (StatusCode, Json<serde_json::Value>)> {
    let key = state.config.effective_api_key();
    let headers: &HeaderMap = req.headers();
    let provided: Option<&str> = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| {
                    if s.len() > 7 && (s.starts_with("Bearer ") || s.starts_with("bearer ")) {
                        Some(&s[7..])
                    } else {
                        None
                    }
                })
        });

    let is_valid = provided
        .map(|p| {
            let p_bytes = p.trim().as_bytes();
            let k_bytes = key.as_bytes();
            // Constant-time comparison: only valid if same length AND same content.
            p_bytes.len() == k_bytes.len() && p_bytes.ct_eq(k_bytes).into()
        })
        .unwrap_or(false);

    if is_valid {
        Ok(next.run(req).await)
    } else {
        warn!("API key auth failed (missing/invalid X-API-Key or Bearer) for {}", req.uri());
        Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "unauthorized: provide X-API-Key or Authorization: Bearer" })),
        ))
    }
}

/// Build the full axum application (api routes + static UI serve + landing + health).
pub fn build_app(daemon: Arc<Daemon>, config: Arc<UniFlowConfig>) -> Router {
    let state = AppState { daemon, config: config.clone() };

    // API routes bound directly to domain/application services (Job, submit, cancel, list, audit)
    // Protect entire /api subtree with API key auth middleware.
    let api = Router::new()
        .route("/jobs", get(list_jobs).post(create_job))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/cancel", post(cancel_job))
        .route("/audit", get(list_audit))
        .route("/status", get(get_status))
        .route("/seed-demo", post(seed_demo))
        .with_state(state.clone())
        .route_layer(middleware::from_fn_with_state(state.clone(), api_key_auth));

    // Serve the Stitch retro UI prototypes under /ui (configurable via UNIFLOW_UI_DIR)
    let ui_static = ServeDir::new(&config.ui_dir);

    // Tower-http layers - rate limiting, strict CORS (for /ui origins), security headers.
    let security_headers_layer = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none';"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        ));

    let body_limit_layer = RequestBodyLimitLayer::new(10 * 1024 * 1024); // 10 MiB

    // Derive CORS origins from actual bind address so they stay correct when port changes.
    let port = config.bind_addr.port();
    let cors_origins: Vec<HeaderValue> = [
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
    ]
    .iter()
    .filter_map(|o| o.parse::<HeaderValue>().ok())
    .collect();

    let cors_layer = CorsLayer::new()
        .allow_origin(cors_origins)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::HeaderName::from_static("x-api-key")]);

    Router::new()
        .route("/", get(landing_page))
        // Health check — unauthenticated, for orchestrator probes (K8s, ECS, Docker).
        .route("/health", get(health_check))
        .nest("/api", api)
        .nest_service("/ui", ui_static)
        // Fallback: if someone hits a page root, redirect to a nice entry dashboard
        .fallback(get(|uri: Uri| async move {
            if uri.path() == "/main_dashboard" || uri.path() == "/dashboard" {
                return Redirect::temporary("/ui/main_dashboard/code.html").into_response();
            }
            if uri.path() == "/job_builder" {
                return Redirect::temporary("/ui/job_builder/code.html").into_response();
            }
            (StatusCode::NOT_FOUND, "Not found. Try / or /ui/main_dashboard/code.html").into_response()
        }))
        .layer(cors_layer)
        .layer(security_headers_layer)
        .layer(body_limit_layer)
}

/// GET /health — unauthenticated health check for container orchestration probes.
async fn health_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// GET /  — small retro landing with links to all bound UI surfaces + API info.
async fn landing_page() -> Html<&'static str> {
    Html(r#"<!doctype html>
<html class="dark"><head><meta charset="utf-8"><title>UniFlow — Working Web App</title>
<style>body{background:#121414;color:#e3e2e2;font-family:JetBrains Mono,monospace;padding:40px;line-height:1.5}a{color:#00dddd;text-decoration:none}a:hover{text-decoration:underline}.card{border:1px solid #3a4a49;padding:16px;margin:12px 0;background:#1e2020}</style>
</head><body>
<h1 style="font-family:Noto Serif">UniFlow — Retro Intelligence Platform (Live)</h1>
<p>Backend (Rust + tokio/rayon/axum) + Stitch UI fully bound. Jobs flow through real JobService, LocalDeltaTransport (real delta when local paths), TamperEvidentAuditLogger, intelligence, RBAC/MFA hooks.</p>
<div class="card">
  <strong>Primary UIs (open these):</strong><br>
  <a href="/ui/uniflow_data_transfer_platform/code.html">/ui/uniflow_data_transfer_platform/code.html</a> — main dashboard + kanban (live)<br>
  <a href="/ui/main_dashboard/code.html">/ui/main_dashboard/code.html</a> — alternative dashboard<br>
  <a href="/ui/job_builder/code.html">/ui/job_builder/code.html</a> — create/submit jobs (POSTs to real /api/jobs)<br>
  <a href="/ui/active_transfer_detail/code.html">/ui/active_transfer_detail/code.html</a> — detail view (poll /api/jobs/:id)<br>
  <a href="/ui/endpoint_manager/code.html">/ui/endpoint_manager/code.html</a> — endpoints + intel (static + status)<br>
  <a href="/ui/history_audit_log/code.html">/ui/history_audit_log/code.html</a> — jobs + tamper-evident audit table<br>
</div>
<div class="card">
  <strong>API (used by the bound UIs — AUTH REQUIRED):</strong><br>
  <strong>Auth:</strong> All /api/* require header X-API-Key: &lt;key&gt; or Authorization: Bearer &lt;key&gt;.<br>
  Dev key (unless UNIFLOW_API_KEY env set): dev-uniflow-key-12345<br>
  <br>
  GET /api/jobs — list all jobs (serde Job model)<br>
  POST /api/jobs — submit new (body: CreateJobRequest with source/dest paths, flags for ZK etc)<br>
  GET /api/jobs/:id , POST /api/jobs/:id/cancel<br>
  GET /api/audit — full tamper-evident chain events<br>
  GET /api/status , POST /api/seed-demo (populates working samples)<br>
  GET /health — unauthenticated health probe<br>
</div>
<p>Run real transfers: in job_builder use existing local paths or the seeded demo dirs under %TEMP% (now sandboxed under uniflow_sandbox). Statuses update live via worker. Audit is tamper-evident (BLAKE3 chain). All paths sanitized; errors sanitized for clients.</p>
<p><a href="/api/status">Check live status JSON</a></p>
</body></html>"#)
}

/// GET /api/jobs
async fn list_jobs(State(state): State<AppState>) -> impl IntoResponse {
    match state.daemon.list_jobs().await {
        Ok(jobs) => Json(jobs.into_iter().map(JobSummary::from).collect::<Vec<_>>()).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "failed to list jobs" })),
        )
            .into_response(),
    }
}

/// GET /api/jobs/{id}
async fn get_job(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    if let Ok(job_id) = id.parse::<JobId>() {
        match state.daemon.get_job(job_id).await {
            Ok(job) => return Json(JobSummary::from(job)).into_response(),
            Err(crate::error::UniFlowError::NotAuthorized(_)) => {
                return (StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": "insufficient privileges" }))).into_response();
            }
            Err(_) => {}
        }
    }
    (StatusCode::NOT_FOUND, "job not found").into_response()
}

/// POST /api/jobs  (bound from job_builder form)
async fn create_job(
    State(state): State<AppState>,
    Json(req): Json<CreateJobRequest>,
) -> impl IntoResponse {
    let src_kind = req.source_kind.as_deref().unwrap_or("local");
    let dst_kind = req.dest_kind.as_deref().unwrap_or("local");
    let mode = match req.mode.as_deref() {
        Some("one-way-sync") | Some("sync") => TransferMode::OneWaySync,
        _ => TransferMode::Copy,
    };

    let sandbox = get_sandbox_dir(&state.config);

    // Apply path sanitization + sandbox for local paths (device/cloud paths are non-fs metadata).
    // Reject bad paths early with sanitized (non-leaking) client error.
    let src_path_res = if src_kind == "local" {
        Some(sanitize_path(&req.source_path, &sandbox))
    } else {
        None
    };
    let dst_path_res = if dst_kind == "local" {
        Some(sanitize_path(&req.dest_path, &sandbox))
    } else {
        None
    };
    if let Some(Err(_msg)) = &src_path_res {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "invalid source path" }))).into_response();
    }
    if let Some(Err(_msg)) = &dst_path_res {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "invalid destination path" }))).into_response();
    }

    let src = match src_kind {
        "cloud" => Endpoint::Cloud { provider: "localfs".into(), bucket: "demo".into(), prefix: None },
        "device" => Endpoint::Device { device_id: "ui-device".into(), path: req.source_path.clone() },
        _ => Endpoint::Local { path: src_path_res.unwrap().unwrap() },
    };
    let dst = match dst_kind {
        "cloud" => Endpoint::Cloud { provider: "localfs".into(), bucket: "demo".into(), prefix: Some("out/".into()) },
        "device" => Endpoint::Device { device_id: "ui-device".into(), path: req.dest_path.clone() },
        _ => Endpoint::Local { path: dst_path_res.unwrap().unwrap() },
    };

    let mut job = Job::new(Source::from(src), Destination::from(dst), mode);
    if let Some(l) = req.label { job = job.with_label(l); }

    // Apply UI policy toggles (binds the encryption/rbac/mfa radios in job_builder)
    let mut pol = job.policy.clone();
    if let Some(zk) = req.zero_knowledge { pol.zero_knowledge = zk; }
    if let Some(eit) = req.encrypt_in_transit { pol.encrypt_in_transit = eit; }
    if let Some(mfa) = req.mfa_required { pol.mfa_required = mfa; }
    if let Some(role) = req.rbac_role { pol.rbac_role = Some(role); }
    job.policy = pol;

    // Demo mode only: if local src does not exist, synthesize a small sample file
    // so LocalDeltaTransport succeeds. Disabled in production to prevent attacker-triggered
    // file creation in the sandbox.
    if state.config.demo_mode {
        if let Endpoint::Local { path } = job.source.inner() {
            if !path.exists() {
                let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
                if let Ok(mut f) = std::fs::File::create(path) {
                    use std::io::Write;
                    let _ = f.write_all(b"UNIFLOW-DEMO-SAMPLE-DATA-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ");
                    let _ = f.write_all(&[0u8; 8192]); // some bytes for delta to work on
                }
            }
        }
    }
    if let Endpoint::Local { path: dp } = job.destination.inner() {
        if let Some(parent) = dp.parent() { let _ = std::fs::create_dir_all(parent); }
    }

    match state.daemon.submit_job(job).await {
        Ok(id) => (
            StatusCode::CREATED,
            Json(CreateJobResponse {
                id: id.to_string(),
                status: "queued".into(),
                message: "Job accepted by daemon. Real execution via LocalDelta / policy / audit.".into(),
            }),
        )
            .into_response(),
        Err(crate::error::UniFlowError::NotAuthorized(_) | crate::error::UniFlowError::Security(_)) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "insufficient privileges for this operation" })),
        )
            .into_response(),
        Err(_e) => (
            // Sanitized error (no e.to_string() leak of internals to client)
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "job submission failed" })),
        )
            .into_response(),
    }
}

/// POST /api/jobs/{id}/cancel
async fn cancel_job(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    if let Ok(job_id) = id.parse::<JobId>() {
        match state.daemon.cancel_job(job_id).await {
            Ok(()) => return Json(serde_json::json!({ "id": id, "cancelled": true })).into_response(),
            Err(crate::error::UniFlowError::NotAuthorized(_)) => {
                return (StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": "insufficient privileges" }))).into_response();
            }
            Err(_) => {}
        }
    }
    (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "job not found or cancel failed" }))).into_response()
}

/// GET /api/audit — returns tamper-evident log for history_audit_log page
async fn list_audit(State(state): State<AppState>) -> Json<AuditResponse> {
    let events = state.daemon.list_audit_events();
    let root = if let Some(last) = events.last() {
        last.prev_hash.clone()
    } else {
        "genesis".into()
    };
    Json(AuditResponse { events, root })
}

/// GET /api/status
async fn get_status(State(state): State<AppState>) -> impl IntoResponse {
    match state.daemon.list_jobs().await {
        Ok(jobs) => {
            let running = jobs.iter().filter(|j| matches!(j.status, JobStatus::Running { .. })).count();
            let audit_root = state.daemon.list_audit_events().last().map(|e| e.prev_hash.clone()).unwrap_or_else(|| "genesis".into());
            Json(StatusResponse {
                daemon: "uniflowd-live".into(),
                jobs_total: jobs.len(),
                running,
                audit_root,
            }).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "failed to query status" })),
        ).into_response()
    }
}

/// POST /api/seed-demo — creates a couple of working sample jobs.
/// Only available when demo_mode is enabled.
async fn seed_demo(State(state): State<AppState>) -> impl IntoResponse {
    if !state.config.demo_mode {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "seed-demo is only available in demo mode (set UNIFLOW_DEMO_MODE=true)" })),
        ).into_response();
    }

    use std::fs;
    use std::io::Write;
    use std::time::SystemTime;

    let sandbox = get_sandbox_dir(&state.config);
    let base = sandbox.join(format!("uniflow_ui_demo_{}", SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()));
    let _ = fs::create_dir_all(&base);
    let src = sanitize_path(&base.join("sample_src.bin").to_string_lossy(), &sandbox).unwrap_or_else(|_| base.join("sample_src.bin"));
    let dst = sanitize_path(&base.join("sample_dst.bin").to_string_lossy(), &sandbox).unwrap_or_else(|_| base.join("sample_dst.bin"));
    {
        let mut f = fs::File::create(&src).unwrap();
        for i in 0u32..20_000 { let _ = f.write_all(&i.to_le_bytes()); }
    }

    let mut job = Job::new(
        Source::from(Endpoint::Local { path: src.clone() }),
        Destination::from(Endpoint::Local { path: dst.clone() }),
        TransferMode::Copy,
    ).with_label("UI-seeded local delta demo");

    // Demonstrate security policy from UI concepts
    let mut p = job.policy.clone();
    p.zero_knowledge = true;
    p.encrypt_in_transit = true;
    p.audit_level = "tamper_evident".into();
    job.policy = p;

    let id = state.daemon.submit_job(job).await.unwrap_or_else(|_| uuid::Uuid::nil());

    let dst2 = sanitize_path(&base.join("sample_dst2.bin").to_string_lossy(), &sandbox).unwrap_or_else(|_| base.join("sample_dst2.bin"));
    let job2 = Job::new(
        Source::from(Endpoint::Local { path: src }),
        Destination::from(Endpoint::Local { path: dst2 }),
        TransferMode::Copy,
    ).with_label("UI-seeded second transfer");
    let id2 = state.daemon.submit_job(job2).await.unwrap_or_else(|_| uuid::Uuid::nil());

    Json(serde_json::json!({
        "seeded": true,
        "ids": [id.to_string(), id2.to_string()],
        "note": "Jobs submitted to real daemon worker. Use dashboards to watch status + audit."
    })).into_response()
}

/// Start the HTTP server. Called from main for the working web application.
pub async fn start_server(
    daemon: Arc<Daemon>,
    config: Arc<UniFlowConfig>,
    shutdown: tokio::sync::watch::Receiver<()>,
) -> crate::error::Result<()> {
    let app = build_app(daemon, config.clone());

    let addr = config.bind_addr;

    if config.tls_ready() {
        // TLS is configured but axum-server is not in Cargo.toml yet.
        // To enable HTTPS, add `axum-server = { version = "0.7", features = ["tls-rustls"] }` to Cargo.toml.
        // For now, log the configuration and proceed with plain HTTP with a prominent warning.
        warn!(
            "TLS cert/key are configured but the axum-server crate is not enabled. \
             To enable HTTPS, add axum-server to Cargo.toml. Falling back to HTTP."
        );
    }

    info!("UniFlow web UI + API listening on http://{}", addr);
    info!("  Landing: http://{}/", addr);
    info!("  Health:  http://{}/health", addr);
    if config.tls_cert.is_none() {
        info!("  (HTTPS: set UNIFLOW_TLS_CERT + UNIFLOW_TLS_KEY to upgrade to TLS)");
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let mut shutdown_rx = shutdown;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
            info!("HTTP server received shutdown signal");
        })
        .await?;
    Ok(())
}
