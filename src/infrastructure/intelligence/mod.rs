//! Module 04: Intelligence & Optimiser
//!
//! Pluggable pre-transfer profiling, hardware detection, and auto-tuning.
//! Integrates with all previous engines (Local Delta, Cloud, P2P) via the shared
//! Transport port and Job model.
//!
//! Design goals (per constraints):
//! - Fully pluggable HAL (new detectors added via registry).
//! - All decisions produce human-readable `explanation` strings (logged + persisted).
//! - Reuses/extends existing `Transport::probe`.

pub mod engine;
pub mod hardware;
pub mod offload;
pub mod optimizer;
pub mod planner;
pub mod probes;
pub mod profiler;

pub use engine::DefaultIntelligenceEngine;
pub use hardware::{HardwareDetector, HardwareRegistry};
pub use offload::{select_offload, CpuOffload};
pub use optimizer::DefaultOptimizer;
pub use planner::CostModelPlanner;
pub use probes::CustomNetworkProbe;
pub use profiler::DefaultSystemProfiler;