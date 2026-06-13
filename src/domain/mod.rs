//! Domain layer: pure business entities and rules.
//!
//! No dependencies on async runtimes, persistence, or concrete transports.
//! These are the core "Source + Destination + Mode" concepts from the blueprint (Sec 3).

pub mod models;

pub use models::{
    BlockSignature, DeltaChunk, DeltaInstruction, Destination, Endpoint, FileManifest, FileSignature,
    Filters, HardwareProfile, Job, JobId, JobStatus, MultiPathPlan, NetworkProbeResult, P2PDiscoveryInfo,
    PathInfo, PeerId, Policy, ProfilingResult, ResumeState, Schedule, Source, TransferMode, TuningDecision,
};
