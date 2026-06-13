//! Default Optimizer for Module 04.
//!
//! Rule-based but fully explainable. Consumes network + hardware profiles.
//! Produces TuningDecision with detailed `explanation` (logged and attached to Job).

use crate::application::ports::{HardwareProfile, NetworkProbeResult, Optimizer, TuningDecision};
use crate::domain::Job;
use chrono::{DateTime, Utc};
use tracing::info;

pub struct DefaultOptimizer;

impl DefaultOptimizer {
    pub fn new() -> Self { Self }
}

impl Optimizer for DefaultOptimizer {
    fn optimize(&self, job: &Job, network: Option<&NetworkProbeResult>, hardware: &HardwareProfile) -> TuningDecision {
        let mut threads = hardware.cpu_cores as usize * 2; // hyper-threading bias
        let mut chunk_size = 4 * 1024 * 1024; // 4 MiB default
        let mut compression = Some(1u8);
        let mut max_bps = None;
        let mut start_at = None;

        let mut reasons = vec![];

        if let Some(net) = network {
            // High bandwidth → larger chunks, more threads
            if net.bandwidth_mbps > 500.0 {
                chunk_size = 16 * 1024 * 1024;
                threads = (threads as f64 * 1.5) as usize;
                reasons.push("high bandwidth detected → larger chunks + more concurrency".to_string());
            } else if net.bandwidth_mbps < 100.0 {
                chunk_size = 1 * 1024 * 1024;
                compression = Some(6);
                reasons.push("low bandwidth → smaller chunks + stronger compression".to_string());
            }

            // RTT for BDP-based sizing
            let bdp = (net.bandwidth_mbps * 1_000_000.0 / 8.0) * (net.rtt_ms / 1000.0);
            if bdp > 1_000_000.0 {
                chunk_size = chunk_size.max(bdp as u64);
                reasons.push(format!("BDP≈{:.1}MB → chunk size raised", bdp / 1_000_000.0));
            }
        }

        // Hardware influence
        if hardware.accelerators.iter().any(|a| a.contains("qat") || a.contains("cuda")) {
            threads = threads.max(32);
            compression = Some(0); // offload to HW
            reasons.push("hardware accelerator present → higher concurrency, light/no software compression".to_string());
        }

        if hardware.cpu_cores <= 4 {
            threads = threads.min(8);
            reasons.push("limited CPU cores → conservative thread count".to_string());
        }

        // Off-peak / scheduling
        let now = Utc::now();
        if let Some(schedule) = &job.schedule {
            // Very simplified off-peak logic
            if matches!(schedule, crate::domain::Schedule::Interval { .. }) {
                // Example: if we are outside "business hours", be more aggressive
                let hour = now.hour();
                if !(9..=17).contains(&hour) {
                    max_bps = None; // no throttle
                    start_at = None;
                    reasons.push("off-peak hours → remove throttle, aggressive settings".to_string());
                } else {
                    max_bps = Some(100 * 1024 * 1024); // 100 MB/s daytime cap example
                    reasons.push("business hours → apply bandwidth cap for fairness".to_string());
                }
            }
        }

        // Final clamps
        threads = threads.clamp(1, 128);
        chunk_size = chunk_size.clamp(256 * 1024, 128 * 1024 * 1024);

        let explanation = format!(
            "Auto-tuned for job {}: threads={}, chunk={}, compression={:?}, max_bps={:?}. Reasons: {}",
            job.id,
            threads,
            chunk_size,
            compression,
            max_bps,
            reasons.join("; ")
        );

        info!(job_id = %job.id, threads, chunk_size, explanation = %explanation, "optimizer decision");

        TuningDecision {
            threads,
            chunk_size,
            compression_level: compression,
            max_bps,
            start_at,
            explanation,
        }
    }
}