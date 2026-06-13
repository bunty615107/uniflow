//! Simple TransportRouter that selects the appropriate Transport implementation
//! based on Source/Destination endpoint kinds.
//!
//! This keeps the JobService and Daemon connection-agnostic while allowing
//! the right engine (local delta, rclone cloud, or P2P) to be used.
//! Extended for Module 03 P2P + mobile.
//! Module 04: optionally runs IntelligenceEngine before selection.

use crate::application::ports::{IntelligenceEngine, Transport};
use crate::domain::Job;
use crate::infrastructure::{DefaultIntelligenceEngine, IrohP2PTransport, LocalDeltaTransport, RcloneCloudTransport};
use std::sync::Arc;

pub struct TransportRouter {
    local: Arc<LocalDeltaTransport>,
    cloud: Arc<RcloneCloudTransport>,
    p2p: Option<Arc<IrohP2PTransport>>,
    intelligence: Option<Arc<dyn IntelligenceEngine>>,
}

impl TransportRouter {
    pub fn new(
        local: Arc<LocalDeltaTransport>,
        cloud: Arc<RcloneCloudTransport>,
        p2p: Option<Arc<IrohP2PTransport>>,
        intelligence: Option<Arc<dyn IntelligenceEngine>>,
    ) -> Self {
        Self { local, cloud, p2p, intelligence }
    }

    /// Run intelligence (Module 04) if available, then select transport.
    /// This ensures pre-transfer profiling/tuning happens for all engines.
    pub fn select(&self, job: &mut Job) -> Arc<dyn Transport> {
        if let Some(intel) = &self.intelligence {
            if let Err(e) = intel.profile_and_tune(job) {
                tracing::warn!(job_id = %job.id, error = %e, "intelligence profiling failed, continuing with defaults");
            }
        }

        let src = job.source.inner();
        let dst = job.destination.inner();

        let src_device = matches!(src, crate::domain::Endpoint::Device { .. });
        let dst_device = matches!(dst, crate::domain::Endpoint::Device { .. });

        if (src_device || dst_device) {
            if let Some(p2p) = &self.p2p {
                return p2p.clone() as Arc<dyn Transport>;
            }
        }

        let src_cloud = matches!(src, crate::domain::Endpoint::Cloud { .. });
        let dst_cloud = matches!(dst, crate::domain::Endpoint::Cloud { .. });

        if src_cloud || dst_cloud {
            return self.cloud.clone() as Arc<dyn Transport>;
        }

        self.local.clone() as Arc<dyn Transport>
    }
}