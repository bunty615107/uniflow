//! Structured logging initialization (tracing).
//! Every significant action (job create/queue/execute/transition/cancel) logs with
//! fields: job_id, source, destination, mode, status, bytes etc. This forms the
//! foundation for the "Compliance Audit log" P0 requirement from the blueprint.

use crate::config::LogFormat;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Initialize global tracing subscriber.
/// Respects RUST_LOG env (e.g. `RUST_LOG=uniflowd=debug cargo run`).
/// Falls back to info level for the crate.
///
/// `format`: controls output format —
///   - `Pretty`: compact human-readable (default for dev)
///   - `Json`: structured JSON lines for SIEM / log aggregation (production)
///
/// Uses `try_init()` to avoid panics when called multiple times (e.g. in tests).
pub fn init(format: &LogFormat) {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "uniflowd=info,uniflow=info".into());

    match format {
        LogFormat::Json => {
            let fmt_layer = tracing_subscriber::fmt::layer()
                .json()
                .with_target(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true);

            let _ = tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt_layer)
                .try_init();
        }
        LogFormat::Pretty => {
            let fmt_layer = tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_thread_ids(false)
                .with_file(false)
                .with_line_number(false)
                .compact();

            let _ = tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt_layer)
                .try_init();
        }
    }

    tracing::info!(format = ?format, "UniFlow structured logging initialized");
}
