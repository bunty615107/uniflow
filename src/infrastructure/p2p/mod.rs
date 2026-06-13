//! Adaptive P2P Network module (Module 03).
//!
//! Uses Iroh (which builds on libp2p concepts + quinn/QUIC) for mesh networking,
//! LAN discovery, NAT traversal (STUN/hole-punch + relay fallback), and multi-path.
//!
//! This is the "P2P transport" only. Mobile background policy (WorkManager/URLSession)
//! is handled separately via FFI (flutter_rust_bridge) and native platform code.

pub mod iroh_p2p_transport;

pub use iroh_p2p_transport::IrohP2PTransport;