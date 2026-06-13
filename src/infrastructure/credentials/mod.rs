//! Credential vault implementations for the Universal Cloud Connector (Module 01).
//!
//! These implement the CredentialVault port from application/ports.
//! For production use a more secure backend (system keyring, HashiCorp Vault, etc.).

pub mod env_vault;

pub use env_vault::EnvCredentialVault;