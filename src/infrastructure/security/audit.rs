//! Tamper-evident audit logging (Section 9 Compliance Audit Log).
//! Uses BLAKE3 (existing crate) hash chain for tamper evidence.
//! Events are append-only. Root hash can be signed/published for verification.

use blake3::Hasher;
use crate::error::{Result, UniFlowError};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use std::sync::Mutex;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEvent {
    pub job_id: String,
    pub event_type: String, // "submit", "execute_start", "checkpoint", "complete", "cancel", etc.
    pub timestamp: String,
    pub details: String,    // json or human summary, including security decisions
    pub prev_hash: String,  // chain
}

pub struct TamperEvidentAuditLogger {
    last_hash: Mutex<String>,
    events: Mutex<Vec<AuditEvent>>,
    /// When set, events are appended to this file as JSON lines for durability.
    persist_path: Option<std::path::PathBuf>,
}

impl Default for TamperEvidentAuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl TamperEvidentAuditLogger {
    pub fn new() -> Self {
        Self {
            last_hash: Mutex::new("genesis".to_string()),
            events: Mutex::new(Vec::new()),
            persist_path: None,
        }
    }

    /// Create a logger that persists events to an append-only file.
    /// On construction, loads existing events from the file and verifies the chain.
    pub fn with_file(path: std::path::PathBuf) -> Self {
        let logger = Self {
            last_hash: Mutex::new("genesis".to_string()),
            events: Mutex::new(Vec::new()),
            persist_path: Some(path.clone()),
        };

        // Load existing events from file if present.
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                let mut loaded = Vec::new();
                for line in content.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<AuditEvent>(line) {
                        Ok(event) => loaded.push(event),
                        Err(e) => {
                            warn!("skipping malformed audit log line: {e}");
                        }
                    }
                }
                if !loaded.is_empty() {
                    // Replay the chain to recover the last hash.
                    let mut prev = "genesis".to_string();
                    for event in &loaded {
                        let mut hasher = Hasher::new();
                        hasher.update(prev.as_bytes());
                        hasher.update(event.job_id.as_bytes());
                        hasher.update(event.event_type.as_bytes());
                        hasher.update(event.timestamp.as_bytes());
                        hasher.update(event.details.as_bytes());
                        prev = hasher.finalize().to_hex().to_string();
                    }
                    info!(events = loaded.len(), root = %prev, "loaded audit log from file");
                    *logger.last_hash.lock().unwrap() = prev;
                    *logger.events.lock().unwrap() = loaded;
                }
            }
        }

        // Verify the loaded chain (warns but continues on failure — defence in depth).
        if let Err(e) = logger.verify_chain() {
            warn!("audit chain verification failed on startup: {e} — continuing with caution");
        }

        logger
    }

    pub fn log(&self, event: AuditEvent) -> Result<String> {
        let mut last = self.last_hash.lock().unwrap();

        // Chain: hash(prev || event) -- use *fresh* hasher per link for correct tamper chain.
        // (Previous impl mutated shared hasher across events leading to non-standard accumulation;
        // now each new_hash = blake3(prev || job || type || ts || details) which supports replay verify from genesis.)
        let mut hasher = Hasher::new();
        hasher.update(last.as_bytes());
        hasher.update(event.job_id.as_bytes());
        hasher.update(event.event_type.as_bytes());
        hasher.update(event.timestamp.as_bytes());
        hasher.update(event.details.as_bytes());

        let new_hash = hasher.finalize().to_hex().to_string();
        *last = new_hash.clone();

        // Store for UI / query (tamper evident via chain)
        {
            let mut evs = self.events.lock().unwrap();
            evs.push(event.clone());
        }

        // Emit via tracing for the daemon logs + structured audit
        info!(
            job_id = %event.job_id,
            event_type = %event.event_type,
            hash = %new_hash,
            prev = %event.prev_hash,
            "tamper_evident_audit"
        );

        // Persist to append-only file if configured.
        if let Some(path) = &self.persist_path {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                if let Ok(json) = serde_json::to_string(&event) {
                    let _ = writeln!(f, "{json}");
                    let _ = f.flush();
                }
            }
        }
        Ok(new_hash)
    }

    pub fn current_root(&self) -> String {
        self.last_hash.lock().unwrap().clone()
    }

    /// Return copy of all logged events (for history/audit UI page). Newest last.
    pub fn get_events(&self) -> Vec<AuditEvent> {
        self.events.lock().unwrap().clone()
    }

    /// Basic verify_chain: replay from genesis verifying the tamper-evident hash links.
    /// Recomputes each successive root using fresh BLAKE3(prev || event_fields) and checks:
    ///
    /// - each event's recorded prev_hash matches the expected from prior step
    /// - final computed root matches the logger's current_root()
    ///
    /// Returns Ok(()) on clean chain (from initial "genesis"), or Err on first mismatch (indicates tamper or bug).
    /// This can be called periodically or on shutdown/UI for compliance. Does not mutate state.
    /// Pairs well with the commented append-only file persistence above (external tools can replay the log file too).
    pub fn verify_chain(&self) -> Result<()> {
        let events = self.get_events();
        let mut expected_prev = "genesis".to_string();

        for (idx, event) in events.iter().enumerate() {
            if event.prev_hash != expected_prev {
                warn!(
                    idx,
                    job_id = %event.job_id,
                    expected = %expected_prev,
                    got = %event.prev_hash,
                    "audit chain verification failed: prev_hash mismatch (possible tamper)"
                );
                return Err(UniFlowError::Internal(format!(
                    "audit chain broken at event {} (job {}): prev mismatch",
                    idx, event.job_id
                )));
            }

            // Replay exact link hash (must match what log() now computes)
            let mut hasher = Hasher::new();
            hasher.update(expected_prev.as_bytes());
            hasher.update(event.job_id.as_bytes());
            hasher.update(event.event_type.as_bytes());
            hasher.update(event.timestamp.as_bytes());
            hasher.update(event.details.as_bytes());
            let computed = hasher.finalize().to_hex().to_string();

            expected_prev = computed;
        }

        // Final root check against live state
        let current = self.current_root();
        if expected_prev != current {
            warn!(expected = %expected_prev, current = %current, "audit root mismatch after replay");
            return Err(UniFlowError::Internal("audit root does not match replayed chain".into()));
        }

        info!(num_events = events.len(), root = %current, "audit chain verified successfully from genesis");
        Ok(())
    }
}