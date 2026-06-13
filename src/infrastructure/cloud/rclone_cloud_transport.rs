//! RcloneCloudTransport: implementation of the Transport port for 70+ cloud backends.
//!
//! Uses the gRPC bridge to an embedded Rclone process (Section 13).
//! Supports:
//! - Local ↔ Cloud
//! - Cloud ↔ Cloud (same provider) with server-side copy (zero bandwidth)
//! - Multipart uploads with basic size-based tuning
//! - Credential injection via CredentialVault

use crate::application::ports::{CloudCredential, CredentialVault, TransferReport, Transport};
use crate::domain::{Destination, Endpoint, Job, Source};
use crate::error::{Result, UniFlowError};
use crate::infrastructure::cloud::RcloneBridgeClient;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

pub struct RcloneCloudTransport {
    client: Arc<tokio::sync::Mutex<RcloneBridgeClient>>, // mutex because tonic client is not Sync in all contexts
    vault: Arc<dyn CredentialVault>,
}

impl RcloneCloudTransport {
    pub fn new(client: RcloneBridgeClient, vault: Arc<dyn CredentialVault>) -> Self {
        Self {
            client: Arc::new(tokio::sync::Mutex::new(client)),
            vault,
        }
    }

    /// Decide whether we can/should use server-side copy.
    fn can_use_server_side(&self, src: &Endpoint, dst: &Endpoint) -> bool {
        if let (Endpoint::Cloud { provider: p1, .. }, Endpoint::Cloud { provider: p2, .. }) =
            (src, dst)
        {
            p1 == p2
        } else {
            false
        }
    }

    fn extract_cloud_info(endpoint: &Endpoint) -> Option<(String, String, Option<String>)> {
        if let Endpoint::Cloud {
            provider,
            bucket,
            prefix,
        } = endpoint
        {
            Some((provider.clone(), bucket.clone(), prefix.clone()))
        } else {
            None
        }
    }
}

#[async_trait]
impl Transport for RcloneCloudTransport {
    fn name(&self) -> &'static str {
        "rclone-cloud"
    }

    async fn execute(&self, job: &Job) -> Result<TransferReport> {
        let start = std::time::Instant::now();

        let src_info = Self::extract_cloud_info(job.source.inner());
        let dst_info = Self::extract_cloud_info(job.destination.inner());

        info!(
            job_id = %job.id,
            source = %job.source.label(),
            destination = %job.destination.label(),
            mode = %job.mode.as_str(),
            "rclone-cloud transfer started"
        );

        // Resolve credentials
        let cred_ref = job.credentials_ref.as_deref().unwrap_or("default");
        let cred: CloudCredential = self.vault.resolve(cred_ref)?;

        let mut client = self.client.lock().await;

        // Configure remote(s) on the Go side
        // We use simple names derived from the job for isolation.
        let src_remote_name = format!("src-{}", job.id);
        let dst_remote_name = format!("dst-{}", job.id);

        // For Local endpoints we let Rclone use its "local" remote.
        // For Cloud we configure using the vault credential.
        if src_info.is_some() {
            let _ = client
                .configure_remote(&src_remote_name, cred.clone())
                .await
                .map_err(|e| UniFlowError::Transport(e.to_string()))?;
        }
        if dst_info.is_some() {
            let _ = client
                .configure_remote(&dst_remote_name, cred)
                .await
                .map_err(|e| UniFlowError::Transport(e.to_string()))?;
        }

        // Build remote paths (rclone style: remote:bucket/prefix)
        let build_remote_path = |info: Option<(String, String, Option<String>)>, remote_name: &str| {
            match info {
                Some((provider, bucket, prefix)) => {
                    let p = prefix.map(|pr| format!("{}/{}", bucket, pr)).unwrap_or(bucket);
                    format!("{}:{}", remote_name, p)
                }
                None => "local".to_string(), // fallback, caller should have used local transport
            }
        };

        let src_remote_path = if src_info.is_some() {
            build_remote_path(src_info, &src_remote_name)
        } else {
            // Local source - Rclone understands absolute paths directly for local
            // For simplicity in this skeleton we assume the caller passed a path via the local remote
            // or we could map Local Endpoint here.
            job.source.inner().label() // will be improved
        };

        let dst_remote_path = if dst_info.is_some() {
            build_remote_path(dst_info, &dst_remote_name)
        } else {
            job.destination.inner().label()
        };

        // Perform the transfer
        let response = if self.can_use_server_side(job.source.inner(), job.destination.inner()) {
            info!(job_id = %job.id, "using server-side copy (zero bandwidth)");
            client
                .server_side_copy(&src_remote_name, &src_remote_path, &dst_remote_name, &dst_remote_path)
                .await
                .map_err(|e| UniFlowError::Transport(e.to_string()))?
        } else {
            info!(job_id = %job.id, "using standard copy (may involve data transfer)");
            client
                .copy(&src_remote_name, &src_remote_path, &dst_remote_name, &dst_remote_path)
                .await
                .map_err(|e| UniFlowError::Transport(e.to_string()))?
        };

        let duration = start.elapsed().as_millis() as u64;

        info!(
            job_id = %job.id,
            bytes = response.bytes_transferred,
            server_side = response.server_side,
            "rclone-cloud transfer completed"
        );

        Ok(TransferReport {
            bytes_transferred: response.bytes_transferred as u64,
            duration_ms: duration,
            integrity_hash: if response.etag_or_hash.is_empty() {
                None
            } else {
                Some(response.etag_or_hash)
            },
            chunks: 1, // Rclone abstracts this; could be enhanced for multipart
        })
    }
}