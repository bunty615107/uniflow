//! Cloud connector module for the Universal Cloud Connector (Module 01).
//!
//! Contains the gRPC client for the Rclone bridge and the RcloneCloudTransport.

pub mod rclone_client;
pub mod rclone_cloud_transport;

pub use rclone_client::RcloneBridgeClient;
pub use rclone_cloud_transport::RcloneCloudTransport;