//! UniFlow daemon entry point.
//!
//! Demonstrates the clean architecture layers + pluggable transports.
//! Uses tokio + rayon. Connection-agnostic by design (Source + Destination + Mode).

use std::sync::Arc;
use tracing::info;

use uniflow::config::UniFlowConfig;
use uniflow::domain::{Destination, Endpoint, Job, Source, TransferMode};
use uniflow::Daemon;

#[tokio::main]
async fn main() -> uniflow::Result<()> {
    // Load and validate configuration from environment before anything else.
    let config = match UniFlowConfig::from_env() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("FATAL: configuration error: {e}");
            std::process::exit(1);
        }
    };

    uniflow::logging::init(&config.log_format);

    info!("=== UniFlow — Production Daemon ===");
    info!("Phase 0/1 core + Module 04 intelligence + Module 05 security + Module 06 web binding");

    // The real Daemon (contains JobService + worker + pluggable transports + audit + rbac + intel)
    let daemon = Arc::new(Daemon::new(&config).await?);

    // Optional: seed demo jobs on startup (only in demo mode).
    if config.demo_mode {
        seed_demo_jobs(&daemon).await;
    }

    // Graceful shutdown channel: the web server and signal handler coordinate through this.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());

    // Spawn signal handler for graceful shutdown (Ctrl+C / SIGTERM).
    let daemon_for_shutdown = daemon.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("Shutdown signal received — stopping daemon and web server...");
        let _ = daemon_for_shutdown.shutdown().await;
        let _ = shutdown_tx.send(());
    });

    // Launch the working web application.
    info!("Starting web server. Press Ctrl+C to stop.");
    uniflow::web::start_server(daemon.clone(), config.clone(), shutdown_rx).await?;

    info!("UniFlow daemon shut down cleanly.");
    Ok(())
}

/// Wait for Ctrl+C or SIGTERM (on Unix).
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

/// Seed two working local-delta jobs so the Kanban / history / detail pages have data.
/// Only called when UNIFLOW_DEMO_MODE=true.
async fn seed_demo_jobs(daemon: &Daemon) {
    use std::fs;
    use std::io::Write;

    let base = std::env::temp_dir().join("uniflow_webapp_seed");
    let _ = fs::create_dir_all(&base);
    let src = base.join("seed_src.bin");
    let dst = base.join("seed_dst.bin");
    {
        let mut f = fs::File::create(&src).unwrap_or_else(|_| fs::File::create(&src).unwrap());
        for i in 0u32..8_000 { let _ = f.write_all(&i.to_le_bytes()); }
    }
    let mut j = Job::new(
        Source::from(Endpoint::Local { path: src.clone() }),
        Destination::from(Endpoint::Local { path: dst.clone() }),
        TransferMode::Copy,
    ).with_label("web-seed: initial local delta");
    let mut p = j.policy.clone();
    p.zero_knowledge = true;
    p.encrypt_in_transit = true;
    p.audit_level = "tamper_evident".into();
    j.policy = p;
    let _ = daemon.submit_job(j).await;

    // second job
    let j2 = Job::new(
        Source::from(Endpoint::Local { path: src }),
        Destination::from(Endpoint::Local { path: base.join("seed_dst2.bin") }),
        TransferMode::Copy,
    ).with_label("web-seed: follow-up (delta)");
    let _ = daemon.submit_job(j2).await;

    info!("Seeded two working demo jobs (local paths under TEMP). Watch them progress in the bound dashboards.");
}
