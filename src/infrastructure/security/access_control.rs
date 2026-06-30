//! RBAC + MFA hooks (Section 9).
//! Pluggable. Default is permissive for non-enterprise; enterprise supplies real impl.
//! Server always forces safe/low-privilege rbac_role defaults on calls that lack explicit trusted role.

use crate::error::{Result, UniFlowError};
use tracing::{info, warn};

#[derive(Clone, Debug, Default)]
pub struct RbacEnforcer {
    // In real: roles from config or external IdP (LDAP/AD/SSO per Section 9).
}

impl RbacEnforcer {
    pub fn new() -> Self { Self {} }

    /// Check if role can perform action on job.
    /// Updated for stricter denies (no sensitive ops for operator/auditor; auditor reads only !sensitive).
    /// Supports extended actions: cancel, get, list.
    pub fn check(&self, role: Option<&str>, action: &str, job_sensitivity: bool) -> Result<()> {
        match (role, action) {
            (Some("admin"), _) => Ok(()),
            // Stricter: operator limited to non-sensitive for most; cancel also gated by sensitivity here for deny on sensitive
            (Some("operator"), "submit" | "cancel" | "get" | "list") if !job_sensitivity => Ok(()),
            (Some("auditor"), "list" | "get") if !job_sensitivity => Ok(()),
            _ => {
                info!(role = ?role, action, job_sensitivity, "rbac_denied");
                Err(UniFlowError::NotAuthorized(format!(
                    "role {:?} cannot perform '{}' (sensitive={})",
                    role, action, job_sensitivity
                )))
            }
        }
    }
}

/// MFA hook. Enterprise provides challenge (TOTP, WebAuthn, etc.).
pub trait MfaHook: Send + Sync {
    /// Returns Ok(token) on success, Err on failure/timeout.
    fn challenge(&self, user_ref: &str, action: &str) -> Result<String>;
}

/// Demo (insecure) MFA hook replacement for NoopMfa.
/// 
/// Logs a prominent security warning on every use. Still allows the action (returns bypass token)
/// to keep demo paths working.
/// 
/// - Documented explicitly for debug/demo only; still allow in debug builds (behavior is unconditional here but callers should gate).
/// - In production: supply a real impl to JobService/Daemon (e.g. via feature flag or config).
/// - Server forces safe rbac_role on entry points; this MFA must never protect real data outside of local demos.
/// 
/// Access via full path or after re-export: crate::infrastructure::security::access_control::DemoMfa
pub struct DemoMfa;

impl MfaHook for DemoMfa {
    fn challenge(&self, _user: &str, _action: &str) -> Result<String> {
        warn!(
            "SECURITY DEMO WARNING: DemoMfa (replaced NoopMfa) bypassing real MFA for action. \
             This is INSECURE and for debug/demo only. Log this and do not deploy to prod. \
             Provide a real MfaHook impl (TOTP/WebAuthn) for enterprise use."
        );
        Ok("demo-mfa-bypass".into())
    }
}