//! Core error type for UniFlow Phase 0.
//! All public APIs return `uniflow::Result<T>` (thiserror based).

use thiserror::Error;

#[derive(Error, Debug)]
pub enum UniFlowError {
    #[error("job not found: {0}")]
    JobNotFound(uuid::Uuid),

    #[error("invalid state transition: {from} -> {to} for job {job_id}")]
    InvalidStateTransition {
        job_id: uuid::Uuid,
        from: String,
        to: String,
    },

    #[error("transport error: {0}")]
    Transport(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("security policy violation: {0}")]
    Security(String),

    #[error("authentication failed: {0}")]
    Authentication(String),

    #[error("insufficient privileges: {0}")]
    NotAuthorized(String),

    #[error("rate limit exceeded")]
    RateLimit,

    #[error("internal: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, UniFlowError>;
