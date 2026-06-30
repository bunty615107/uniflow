//! Default IntelligenceEngine implementation for Module 04.
//!
//! Runs the full pipeline (probes + hardware + optimizer) and mutates the Job
//! with profiling + tuning. All steps produce explainable logs.

use crate::application::ports::{IntelligenceEngine, NetworkProbe, Optimizer, Planner, SystemProfiler};
use crate::domain::{Job, ProfilingResult};
use crate::error::Result;
use crate::infrastructure::intelligence::{
    CostModelPlanner, CustomNetworkProbe, DefaultOptimizer, DefaultSystemProfiler, HardwareRegistry,
};
use chrono::Utc;
use tracing::{info, warn};

pub struct DefaultIntelligenceEngine {
    network_probe: Box<dyn NetworkProbe>,
    optimizer: Box<dyn Optimizer>,
    hardware_registry: HardwareRegistry,
    /// Deliverable 1: the real profiler + cost-model planner. Populates `job.plan`.
    profiler: DefaultSystemProfiler,
    planner: CostModelPlanner,
}

impl DefaultIntelligenceEngine {
    pub fn new() -> Self {
        Self {
            network_probe: Box::new(CustomNetworkProbe::new()),
            optimizer: Box::new(DefaultOptimizer::new()),
            hardware_registry: HardwareRegistry::default(),
            profiler: DefaultSystemProfiler::new(),
            planner: CostModelPlanner::new(),
        }
    }
}

impl Default for DefaultIntelligenceEngine {
    fn default() -> Self {
        Self::new()
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

        // Deliverable 1: run the real profiler + cost-model planner and attach the
        // resulting TransferPlan to the job (auditable, consumed by the parallel core).
        match self.profiler.profile_pair(job.source.inner(), job.destination.inner()) {
            Ok(pair) => {
                let plan = self.planner.plan(job, &pair);
                info!(job_id = %job.id, plan = %plan.explanation, "transfer plan attached to job");
                job.plan = Some(plan);
            }
            Err(e) => warn!(job_id = %job.id, error = %e, "profiling failed; parallel core will self-profile"),
        }

        info!(job_id = %job.id, explanation = %decision.explanation, "intelligence tuning complete and applied to job");

        Ok(result)
    }
}