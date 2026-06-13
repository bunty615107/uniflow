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
pub mod optimizer;
pub mod probes;

pub use engine::DefaultIntelligenceEngine;
pub use hardware::{HardwareDetector, HardwareRegistry};
pub use optimizer::DefaultOptimizer;
pub use probes::CustomNetworkProbe;