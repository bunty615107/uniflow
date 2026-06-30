//! Simple TransportRouter that selects the appropriate Transport implementation
//! based on Source/Destination endpoint kinds and the intelligence layer's plan.
//!
//! This keeps the JobService and Daemon connection-agnostic while allowing
//! the right engine (parallel local core, rclone cloud, or P2P) to be used.
//! Module 04: runs IntelligenceEngine before selection so `job.plan` (the tuned
//! TransferPlan) is available; routing then honours `plan.transport`.

use crate::application::ports::{IntelligenceEngine, Transport, TransportSelector};
use crate::domain::plan::TransportHint;
use crate::domain::Job;
use crate::infrastructure::{IrohP2PTransport, LocalDeltaTransport, ParallelTransport};
use std::sync::Arc;

pub struct TransportRouter {
    local: Arc<LocalDeltaTransport>,
    /// Deliverable 2: the self-optimizing parallel core for path-based transfers.
    parallel: Arc<ParallelTransport>,
    /// `dyn Transport` so the daemon can fall back to another transport when the
    /// rclone bridge is unavailable (the fallback may be a different concrete type).
    cloud: Arc<dyn Transport>,
    p2p: Option<Arc<IrohP2PTransport>>,
    intelligence: Option<Arc<dyn IntelligenceEngine>>,
    /// When true, local path-based jobs route through the parallel core (default),
    /// otherwise through the legacy delta transport. Lets deployments opt out.
    use_parallel_local: bool,
}

impl TransportRouter {
    pub fn new(
        local: Arc<LocalDeltaTransport>,
        cloud: Arc<dyn Transport>,
        p2p: Option<Arc<IrohP2PTransport>>,
        intelligence: Option<Arc<dyn IntelligenceEngine>>,
    ) -> Self {
        Self {
            local,
            parallel: Arc::new(ParallelTransport::new()),
            cloud,
            p2p,
            intelligence,
            use_parallel_local: true,
        }
    }

    /// Construct with an explicit parallel core (e.g. shared instance) and toggle.
    pub fn with_parallel(
        local: Arc<LocalDeltaTransport>,
        parallel: Arc<ParallelTransport>,
        cloud: Arc<dyn Transport>,
        p2p: Option<Arc<IrohP2PTransport>>,
        intelligence: Option<Arc<dyn IntelligenceEngine>>,
        use_parallel_local: bool,
    ) -> Self {
        Self { local, parallel, cloud, p2p, intelligence, use_parallel_local }
    }
}

impl TransportSelector for TransportRouter {
    /// Run intelligence (Module 04) if available, then select transport.
    /// This ensures pre-transfer profiling/tuning happens for all engines and that
    /// the resulting `job.plan` drives the routing decision.
    fn select(&self, job: &mut Job) -> Arc<dyn Transport> {
        if let Some(intel) = &self.intelligence {
            if let Err(e) = intel.profile_and_tune(job) {
                tracing::warn!(job_id = %job.id, error = %e, "intelligence profiling failed, continuing with defaults");
            }
        }

        // Prefer the plan's transport hint when the intelligence layer produced one.
        if let Some(plan) = &job.plan {
            match plan.transport {
                TransportHint::P2p => {
                    if let Some(p2p) = &self.p2p {
                        return p2p.clone() as Arc<dyn Transport>;
                    }
                }
                TransportHint::Cloud => return self.cloud.clone() as Arc<dyn Transport>,
                TransportHint::LocalParallel => {
                    if self.use_parallel_local {
                        return self.parallel.clone() as Arc<dyn Transport>;
                    }
                    return self.local.clone() as Arc<dyn Transport>;
                }
            }
        }

        // Fallback: classic kind-based routing (no plan available).
        let src = job.source.inner();
        let dst = job.destination.inner();

        let src_device = matches!(src, crate::domain::Endpoint::Device { .. });
        let dst_device = matches!(dst, crate::domain::Endpoint::Device { .. });
        if src_device || dst_device {
            if let Some(p2p) = self.p2p.as_ref() {
                return p2p.clone() as Arc<dyn Transport>;
            }
        }

        let src_cloud = matches!(src, crate::domain::Endpoint::Cloud { .. });
        let dst_cloud = matches!(dst, crate::domain::Endpoint::Cloud { .. });
        if src_cloud || dst_cloud {
            return self.cloud.clone() as Arc<dyn Transport>;
        }

        if self.use_parallel_local {
            self.parallel.clone() as Arc<dyn Transport>
        } else {
            self.local.clone() as Arc<dyn Transport>
        }
    }
}