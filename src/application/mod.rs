//! Application layer: use cases, orchestration, and port definitions.
//!
//! Depends on `domain` only. Defines the ports (traits) that infrastructure must implement.

pub mod ports;
pub mod services;

pub use ports::{JobRepository, Transport};
