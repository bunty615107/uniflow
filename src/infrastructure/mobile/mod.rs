//! Mobile background sync foundation (Module 03).
//!
//! IMPORTANT: This module deliberately contains **only** the Rust-side hooks
//! and FFI-friendly API. It is completely separate from the P2P transport logic
//! (which lives in `p2p/`) and from any platform policy (Android WorkManager,
//! iOS BGTaskScheduler / URLSessionBackground).
//!
//! The actual mobile daemon scheduling, constraints, and notifications are
//! implemented in the native (Kotlin/Swift) + Flutter layer using
//! flutter_rust_bridge to call into these functions.

use crate::domain::Job;
use crate::error::Result;
use tracing::info;

/// Rust-side API that can be exposed via flutter_rust_bridge to mobile code.
/// The P2P transport itself is injected or created internally when a background
/// task starts the sync.
pub trait MobileP2PBackground: Send + Sync {
    /// Start (or resume) a P2P transfer for the given job.
    /// Called by the platform background task (WorkManager / URLSession).
    fn start_sync(&self, job: Job) -> Result<()>;

    /// Stop the current background sync (best-effort).
    fn stop_sync(&self);
}

/// Default no-op implementation (useful when P2P is disabled or not built for mobile).
pub struct NoopMobileBackground;

impl MobileP2PBackground for NoopMobileBackground {
    fn start_sync(&self, job: Job) -> Result<()> {
        info!(job_id = %job.id, "NoopMobileBackground: start_sync called (P2P not active in this build)");
        Ok(())
    }

    fn stop_sync(&self) {
        info!("NoopMobileBackground: stop_sync called");
    }
}

// In a real mobile build you would provide an implementation that holds
// an Arc<IrohP2PTransport> (or the router) and spawns the transfer when
// the OS background task allows it.
//
// Example FFI exposure (for flutter_rust_bridge):
// #[flutter_rust_bridge::frb]
// pub fn start_p2p_background(job_json: String) -> Result<()> { ... }
