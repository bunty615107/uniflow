//! NoopTransport: a stub implementation of the Transport port for Phase 0.
//!
//! Demonstrates the contract (tracing, checkpoints, rayon usage) without
//! performing real I/O. This is the "connection-agnostic" placeholder.

use crate::application::ports::{ProbeResult, TransferReport, Transport, TransportSelector};
use crate::domain::Job;
use crate::error::Result;
use async_trait::async_trait;
use rayon::prelude::*;
use std::time::Duration;
use tokio::time::sleep;
use tracing::info;

pub struct NoopTransport;

#[async_trait]
impl Transport for NoopTransport {
    fn name(&self) -> &'static str {
        "noop"
    }

    async fn execute(&self, job: &Job) -> Result<TransferReport> {
        let start = std::time::Instant::now();

        let mode_str = job.mode.as_str();
        info!(
            job_id = %job.id,
            source = %job.source.label(),
            destination = %job.destination.label(),
            mode = %mode_str,
            "noop transfer started (simulating {} work)",
            mode_str
        );

        // Simulate transfer latency
        let simulated_ms = 300 + (job.id.as_u128() % 500) as u64;
        sleep(Duration::from_millis(simulated_ms)).await;

        // Demonstrate rayon (CPU-bound parallel work) inside the async execution path.
        // Real transports will use this for hashing, delta computation, chunking, etc.
        let parallel_sum: u64 = rayon::join(
            || (0u64..5_000).into_par_iter().map(|x| x * 3 + 1).sum::<u64>(),
            || (10_000u64..15_000).into_par_iter().map(|x| x / 2).sum::<u64>(),
        )
        .0 + (15_000u64..20_000).into_par_iter().map(|x| x % 7).sum::<u64>();

        // Simulate checkpoints (P0 resume support)
        info!(job_id = %job.id, checkpoint = 42, "checkpoint");
        info!(job_id = %job.id, checkpoint = 100, parallel_work = parallel_sum, "checkpoint");

        if job.policy.verify_integrity {
            info!(job_id = %job.id, "integrity verification (stub) passed");
        }

        let duration = start.elapsed().as_millis() as u64;

        info!(
            job_id = %job.id,
            bytes = 12_345_678,
            duration_ms = duration,
            "noop transfer completed"
        );

        Ok(TransferReport {
            bytes_transferred: 12_345_678,
            duration_ms: duration,
            integrity_hash: Some(format!("noop-{:x}", parallel_sum % 1_000_000)),
            chunks: 128,
        })
    }

    async fn probe(&self, _source: &crate::domain::Endpoint, _dest: &crate::domain::Endpoint) -> Option<ProbeResult> {
        Some(ProbeResult {
            reachable: true,
            rtt_ms: Some(2),
            bandwidth_mbps: Some(1000),
        })
    }
}

pub struct NoopSelector;

impl TransportSelector for NoopSelector {
    fn select(&self, _job: &mut Job) -> std::sync::Arc<dyn Transport> {
        std::sync::Arc::new(NoopTransport)
    }
}
