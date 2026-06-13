//! Environment-based credential vault (simple P0/P1 implementation).
//!
//! Credentials are read from environment variables.
//! Example for S3:
//!   UNIFLOW_S3_ACCESS_KEY_ID=...
//!   UNIFLOW_S3_SECRET_ACCESS_KEY=...
//!
//! The reference can be "s3" or a named profile. This vault is intentionally
//! basic — replace with a real vault for production.

use crate::application::ports::{CloudCredential, CredentialVault};
use crate::error::{Result, UniFlowError};
use std::collections::HashMap;
use std::env;

pub struct EnvCredentialVault;

impl EnvCredentialVault {
    pub fn new() -> Self {
        Self
    }

    fn load_from_env(&self, prefix: &str) -> HashMap<String, String> {
        let mut config = HashMap::new();
        for (key, value) in env::vars() {
            if key.starts_with(prefix) {
                // Convert UNIFLOW_S3_ACCESS_KEY_ID → access_key_id
                let rclone_key = key
                    .strip_prefix(prefix)
                    .unwrap_or(&key)
                    .to_lowercase()
                    .replace('_', "_"); // rclone often uses snake_case or specific names
                config.insert(rclone_key, value);
            }
        }
        config
    }
}

impl Default for EnvCredentialVault {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialVault for EnvCredentialVault {
    fn resolve(&self, reference: &str) -> Result<CloudCredential> {
        // reference can be "s3", "my-s3-prod", or a provider name.
        // For simplicity we treat the reference as the provider prefix.
        let provider = reference.split(':').next().unwrap_or(reference).to_lowercase();
        let env_prefix = format!("UNIFLOW_{}_", provider.to_uppercase());

        let mut config = self.load_from_env(&env_prefix);

        if config.is_empty() {
            // Fallback: try to use the reference directly as a named remote (rclone config style)
            // In a real bridge the Go side can load from rclone.conf.
            return Ok(CloudCredential {
                provider: provider.clone(),
                config: HashMap::from([("remote_name".to_string(), reference.to_string())]),
            });
        }

        Ok(CloudCredential { provider, config })
    }
}