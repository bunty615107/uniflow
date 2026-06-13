//! Iroh-based P2P Transport implementation.
//!
//! Approved crates (Section 13): iroh (includes quinn/QUIC), tokio.
//! Features used: LAN discovery, relay for NAT, multi-path capable.
//!
//! Design goals:
//! - Air-gap / LAN direct first (mDNS + direct QUIC).
//! - NAT traversal with hole-punching (STUN).
//! - Relay fallback (TURN-like via iroh relays).
//! - Multi-path transfers (stripe or concurrent paths).
//! - Integrates with Job checkpoint for resume.
//! - Pure transport — no knowledge of Android WorkManager or iOS URLSession.

use crate::application::ports::{ProbeResult, TransferReport, Transport};
use crate::domain::{Endpoint, Job, P2PDiscoveryInfo, PeerId};
use crate::error::{Result, UniFlowError};
use async_trait::async_trait;
use iroh::endpoint::Endpoint; // from iroh crate
use std::sync::Arc;
use tracing::info;

/// The main P2P transport for Mobile ↔ PC and Mobile ↔ Mobile.
/// Implements the connection-agnostic Transport port.
pub struct IrohP2PTransport {
    /// The iroh endpoint (manages QUIC connections, discovery, relays).
    endpoint: Arc<Endpoint>,
    /// Optional custom relay config for air-gap or enterprise.
    relay_mode: RelayMode,
}

#[derive(Clone, Debug)]
pub enum RelayMode {
    /// Full auto (STUN + relays when needed).
    Auto,
    /// Air-gap only: no relays, only direct/LAN.
    AirGapOnly,
    /// Force a specific relay (for testing or controlled envs).
    Custom(String),
}

impl IrohP2PTransport {
    /// Create a new P2P transport.
    /// In real code this would configure iroh with discovery-local-network etc.
    pub async fn new(relay_mode: RelayMode) -> Result<Self> {
        // Real setup (using iroh 0.25+ style):
        // let builder = iroh::Endpoint::builder()
        //     .discovery(iroh::discovery::local_network::LocalNetworkDiscovery::new()?)
        //     .relay_mode( match &relay_mode { ... } );
        // let endpoint = builder.bind().await?;

        // For skeleton we create a placeholder. In production replace with real iroh::Endpoint.
        let endpoint = Arc::new(Endpoint::builder().bind().await.map_err(|e| {
            UniFlowError::Transport(format!("iroh endpoint bind failed: {}", e))
        })?);

        info!("IrohP2PTransport initialized (relay_mode={:?})", relay_mode);

        Ok(Self {
            endpoint,
            relay_mode,
        })
    }

    /// Discover peers (LAN first, then DHT/relay).
    /// This uses iroh's built-in discovery.
    pub async fn discover(&self) -> Result<Vec<P2PDiscoveryInfo>> {
        // In real iroh:
        // for node in self.endpoint.discovery().discover().await? { ... }
        //
        // Skeleton returns empty; real impl would yield PeerId + addrs + relay info.
        Ok(vec![])
    }

    /// Establish a QUIC connection to a peer, preferring direct then relay.
    async fn connect_to_peer(&self, peer: &PeerId) -> Result<iroh::endpoint::Connection> {
        // Real code:
        // let node_addr = ... from discovery;
        // let conn = self.endpoint.connect(node_addr, b"uniflow").await?;
        // if self.relay_mode == RelayMode::AirGapOnly && !is_direct(&conn) { reject }

        // Stub connection (in real code this would be a live QUIC conn from quinn/iroh).
        Err(UniFlowError::Transport(
            "P2P connect stub - replace with real iroh connection".into(),
        ))
    }
}

#[async_trait]
impl Transport for IrohP2PTransport {
    fn name(&self) -> &'static str {
        "p2p-iroh"
    }

    async fn execute(&self, job: &Job) -> Result<TransferReport> {
        let start = std::time::Instant::now();

        info!(
            job_id = %job.id,
            source = %job.source.label(),
            destination = %job.destination.label(),
            mode = %job.mode.as_str(),
            "p2p-iroh transfer started (adaptive mesh)"
        );

        // 1. Determine peer from Device endpoint (or derive from job).
        // For demo we expect at least one side to be Device.
        let peer_id = self.extract_peer_from_endpoints(&job.source, &job.destination)?;

        // 2. Discover + connect (air-gap first, then NAT, then relay).
        // The iroh endpoint + relay_mode handles the strategy internally.
        let _conn = self.connect_to_peer(&peer_id).await?;

        // 3. Multi-path logic (simplified).
        // In real iroh you can open multiple streams or use path-aware sending.
        // Here we simulate striping or concurrent paths.
        let paths_in_use = match &self.relay_mode {
            RelayMode::AirGapOnly => 1, // direct only
            _ => 2,                     // direct + relay for multi-path
        };

        info!(job_id = %job.id, paths = paths_in_use, "multi-path P2P transfer active");

        // 4. Actual data transfer over the QUIC streams.
        // Integrate with Phase 1 delta if desired, or raw bytes.
        // Use job.checkpoint for resume across reconnects.
        // For skeleton we simulate progress and respect checkpoint.
        let mut bytes = job.checkpoint.unwrap_or(0);
        let total = 50_000_000u64; // example size

        // Simulate parallel chunk send over multiple paths (real code would use quinn streams).
        while bytes < total {
            // Check for cancel (from JobService).
            // In real impl the worker loop in JobService already handles this via the transport.

            let chunk = 1_000_000u64.min(total - bytes);
            bytes += chunk;

            // Update checkpoint for resume.
            // The caller (JobService) will persist if we return intermediate or the transport can call back.
            // For now we just log.
            if bytes % 10_000_000 == 0 {
                info!(job_id = %job.id, checkpoint = bytes, "p2p checkpoint");
            }

            // Small yield so the async runtime isn't blocked.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let duration = start.elapsed().as_millis() as u64;

        info!(
            job_id = %job.id,
            bytes = bytes,
            duration_ms = duration,
            paths = paths_in_use,
            "p2p-iroh transfer completed (direct/relay adaptive)"
        );

        Ok(TransferReport {
            bytes_transferred: bytes,
            duration_ms: duration,
            integrity_hash: Some("p2p-b3-stub".into()),
            chunks: (bytes / 1_000_000) as u32,
        })
    }

    async fn probe(&self, _source: &Endpoint, _dest: &Endpoint) -> Option<ProbeResult> {
        // Real impl would return stats from active iroh connections (direct vs relayed).
        Some(ProbeResult {
            reachable: true,
            rtt_ms: Some(15),
            bandwidth_mbps: Some(120),
        })
    }
}

impl IrohP2PTransport {
    fn extract_peer_from_endpoints(&self, src: &Endpoint, dst: &Endpoint) -> Result<PeerId> {
        // In real code look inside Device variant for embedded PeerId or discovery info.
        // For skeleton return a dummy.
        if let Endpoint::Device { device_id, .. } = src {
            let mut id = [0u8; 32];
            id[..device_id.len().min(32)].copy_from_slice(device_id.as_bytes());
            return Ok(PeerId(id));
        }
        if let Endpoint::Device { device_id, .. } = dst {
            let mut id = [0u8; 32];
            id[..device_id.len().min(32)].copy_from_slice(device_id.as_bytes());
            return Ok(PeerId(id));
        }
        Err(UniFlowError::Config(
            "P2P transport requires at least one Device endpoint".into(),
        ))
    }
}