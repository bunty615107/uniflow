//! Structured logging initialization (tracing).
//! Every significant action (job create/queue/execute/transition/cancel) logs with
//! fields: job_id, source, destination, mode, status, bytes etc. This forms the
//! foundation for the "Compliance Audit log" P0 requirement from the blueprint.

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Initialize global tracing subscriber.
/// Respects RUST_LOG env (e.g. `RUST_LOG=uniflowd=debug cargo run`).
/// Falls back to info level for the crate. Pretty format for human runs,
/// can be switched to .json() for SIEM/structured consumption.
pub fn init() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "uniflowd=info,uniflow=info".into());

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .compact();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .init();

    tracing::info!("UniFlow structured logging initialized (Phase 0 daemon)");
}
