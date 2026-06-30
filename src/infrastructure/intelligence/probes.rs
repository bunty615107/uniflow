//! Network and system probes for Module 04.
//!
//! Custom implementation that delegates to the measured `DefaultSystemProfiler`
//! (satisfying JULES-04). Supports caching, TCP RTT probes, and air-gap fallbacks.

use crate::application::ports::{NetworkProbe, SystemProfiler};
use crate::domain::{Endpoint, NetworkProbeResult};
use crate::error::Result;
use crate::infrastructure::intelligence::DefaultSystemProfiler;

pub struct CustomNetworkProbe {
    profiler: DefaultSystemProfiler,
}

impl CustomNetworkProbe {
    pub fn new() -> Self {
        Self {
            profiler: DefaultSystemProfiler::new(),
        }
    }
}

impl Default for CustomNetworkProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkProbe for CustomNetworkProbe {
    fn probe(&self, source: &Endpoint, dest: &Endpoint) -> Result<NetworkProbeResult> {
        let pair = self.profiler.profile_pair(source, dest)?;
        let link = pair.link;
        Ok(NetworkProbeResult {
            rtt_ms: link.rtt_ms,
            bandwidth_mbps: link.throughput_mbps,
            jitter_ms: link.jitter_ms,
            explanation: link.explanation,
        })
    }
}