//! Default IntelligenceEngine implementation for Module 04.
//!
//! Runs the full pipeline (probes + hardware + optimizer) and mutates the Job
//! with profiling + tuning. All steps produce explainable logs.

use crate::application::ports::{IntelligenceEngine, NetworkProbe, Optimizer};
use crate::domain::{Job, ProfilingResult};
use crate::error::Result;
use crate::infrastructure::intelligence::{CustomNetworkProbe, DefaultOptimizer, HardwareRegistry};
use chrono::Utc;
use tracing::info;

pub struct DefaultIntelligenceEngine {
    network_probe: Box<dyn NetworkProbe>,
    optimizer: Box<dyn Optimizer>,
    hardware_registry: HardwareRegistry,
}

impl DefaultIntelligenceEngine {
    pub fn new() -> Self {
        Self {
            network_probe: Box::new(CustomNetworkProbe::new()),
            optimizer: Box::new(DefaultOptimizer::new()),
            hardware_registry: HardwareRegistry::default(),
        }
    }
}

impl IntelligenceEngine for DefaultIntelligenceEngine {
    fn profile_and_tune(&self, job: &mut Job) -> Result<ProfilingResult> {
        info!(job_id = %job.id, "starting intelligence profiling and tuning");

        // 1. Network probe (reuses/enhances the spirit of Transport::probe)
        let network = self.network_probe.probe(job.source.inner(), job.destination.inner()).ok();

        // 2. Hardware profile (pluggable detectors)
        let hardware = self.hardware_registry.detect_all();

        // 3. Optimizer produces explainable decision
        let decision = self.optimizer.optimize(job, network.as_ref(), &hardware);

        let result = ProfilingResult {
            network,
            hardware,
            decision: decision.clone(),
            timestamp: Utc::now(),
        };

        // 4. Apply to job (for engines to consume) + persistable
        // We attach via a simple extension mechanism (in real code could use a new Job field
        // or serialize decision into policy/extensions).
        // For this skeleton we log heavily and mutate job.label with summary for visibility.
        let summary = format!("[INTEL] {}", decision.explanation);
        if let Some(ref mut label) = job.label {
            if !label.contains("[INTEL]") {
                label.push_str(" | ");
                label.push_str(&summary);
            }
        } else {
            job.label = Some(summary);
        }

        // In a fuller system we would also set job.tuning = Some(decision) after extending the model.
        info!(job_id = %job.id, explanation = %decision.explanation, "intelligence tuning complete and applied to job");

        Ok(result)
    }
}