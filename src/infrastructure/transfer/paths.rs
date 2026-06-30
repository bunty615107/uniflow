//! Shared path resolution + security sandbox for path-based transports.
//!
//! Factored out of `local_delta.rs` so the delta engine and the new parallel core
//! enforce the **same** sandbox (no second, divergent security check). All Local /
//! on-prem-Remote endpoints must resolve to a path inside `temp/uniflow_sandbox`,
//! with symlink/`..` escapes rejected via canonicalisation.

use crate::domain::Endpoint;
use std::path::{Path, PathBuf};

/// Resolve an endpoint to a concrete, sandbox-validated path, or `None` if the
/// endpoint isn't path-based or the path escapes the sandbox.
pub fn resolve_sandboxed(endpoint: &Endpoint) -> Option<PathBuf> {
    let raw = match endpoint {
        Endpoint::Local { path } => Some(path.clone()),
        Endpoint::Remote { uri } => {
            if let Some(p) = uri.strip_prefix("file://") {
                Some(PathBuf::from(p))
            } else if uri.starts_with('/') || uri.contains(":\\") || uri.contains(":/") {
                Some(PathBuf::from(uri))
            } else {
                None
            }
        }
        _ => None,
    }?;
    enforce_sandbox(&raw)
}

/// Sandbox validation: canonicalize (resolves symlinks/`..`) and require containment
/// in `temp/uniflow_sandbox`. Non-existent destinations are validated via their parent.
pub fn enforce_sandbox(raw: &Path) -> Option<PathBuf> {
    let sandbox_base = std::env::temp_dir().join("uniflow_sandbox");
    let _ = std::fs::create_dir_all(&sandbox_base);

    let base_canon = sandbox_base.canonicalize().unwrap_or_else(|_| sandbox_base.clone());
    let candidate = raw.to_path_buf();

    match candidate.canonicalize() {
        Ok(c) => {
            if c.starts_with(&base_canon) {
                Some(c)
            } else {
                None
            }
        }
        Err(_) => {
            // Path doesn't exist yet (typical destination): validate via parent.
            if let Some(parent) = candidate.parent() {
                if let Ok(parent_c) = parent.canonicalize() {
                    if parent_c.starts_with(&base_canon) || parent_c == base_canon {
                        return Some(candidate);
                    }
                }
            }
            let s = candidate.to_string_lossy();
            let base_s = base_canon.to_string_lossy();
            if s.starts_with(&*base_s) && !s.contains("..") {
                return Some(candidate);
            }
            None
        }
    }
}

/// The sandbox base directory (for tests / callers that need to stage files).
pub fn sandbox_base() -> PathBuf {
    std::env::temp_dir().join("uniflow_sandbox")
}
