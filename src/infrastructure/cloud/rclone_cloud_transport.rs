//! RcloneCloudTransport: implementation of the Transport port for 70+ cloud backends.
//!
//! Uses the gRPC bridge to an embedded Rclone process (Section 13).
//! Supports:
//! - Local ↔ Cloud
//! - Cloud ↔ Cloud (same provider) with server-side copy (zero bandwidth)
//! - Multipart uploads with basic size-based tuning
//! - Credential injection via CredentialVault

use crate::application::ports::{CloudCredential, CredentialVault, TransferReport, Transport};
use crate::domain::{Endpoint, Job};
use crate::error::{Result, UniFlowError};
use crate::infrastructure::cloud::RcloneBridgeClient;
use crate::infrastructure::transfer::paths::resolve_sandboxed;
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

    /// Map a Local/Remote endpoint to the bare OS path rclone's implicit local backend
    /// understands. The path goes through the same sandbox check as the local engines,
    /// so a cloud job cannot be used to read/write arbitrary filesystem locations.
    fn local_path_string(endpoint: &Endpoint) -> Result<String> {
        resolve_sandboxed(endpoint)
            .map(|p| p.to_string_lossy().into_owned())
            .ok_or_else(|| {
                UniFlowError::Config(
                    "local side of a cloud transfer must be a sandboxed path (rejected)".into(),
                )
            })
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

        // This transport only applies when at least one side is cloud; otherwise the
        // router should have selected the local/parallel engine. Fail loudly rather
        // than silently doing nothing.
        if src_info.is_none() && dst_info.is_none() {
            return Err(UniFlowError::Config(
                "rclone-cloud transport requires at least one Cloud endpoint".into(),
            ));
        }

        let mut client = self.client.lock().await;

        // Per-job remote names keep concurrent jobs isolated on the bridge side.
        let src_remote_name = format!("src-{}", job.id);
        let dst_remote_name = format!("dst-{}", job.id);

        // Credentials are resolved only when a cloud side actually needs them, and only
        // the cloud side(s) get a configured remote. Local sides use rclone's implicit
        // local backend (a bare OS path), so they need no credential.
        let cred: Option<CloudCredential> = if src_info.is_some() || dst_info.is_some() {
            let cred_ref = job.credentials_ref.as_deref().unwrap_or("default");
            Some(self.vault.resolve(cred_ref)?)
        } else {
            None
        };

        if src_info.is_some() {
            let c = cred.clone().expect("cred resolved when a cloud side exists");
            client
                .configure_remote(&src_remote_name, c)
                .await
                .map_err(|e| UniFlowError::Transport(e.to_string()))?;
        }
        if dst_info.is_some() {
            let c = cred.clone().expect("cred resolved when a cloud side exists");
            client
                .configure_remote(&dst_remote_name, c)
                .await
                .map_err(|e| UniFlowError::Transport(e.to_string()))?;
        }

        // Build rclone-style paths: cloud → "remote:bucket/prefix"; local → the bare
        // sandbox-resolved OS path (rclone's implicit local backend).
        let build_cloud_path = |info: &(String, String, Option<String>), remote_name: &str| {
            let (_provider, bucket, prefix) = info;
            let p = match prefix {
                Some(pr) => format!("{}/{}", bucket, pr),
                None => bucket.clone(),
            };
            format!("{}:{}", remote_name, p)
        };

        let src_remote_path = match &src_info {
            Some(info) => build_cloud_path(info, &src_remote_name),
            None => Self::local_path_string(job.source.inner())?,
        };
        let dst_remote_path = match &dst_info {
            Some(info) => build_cloud_path(info, &dst_remote_name),
            None => Self::local_path_string(job.destination.inner())?,
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
