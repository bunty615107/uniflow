//! Tamper-evident audit logging (Section 9 Compliance Audit Log).
//! Uses BLAKE3 (existing crate) hash chain for tamper evidence.
//! Events are append-only. Root hash can be signed/published for verification.

use blake3::Hasher;
use crate::error::{Result, UniFlowError};
use tracing::{info, warn};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct AuditEvent {
    pub job_id: String,
    pub event_type: String, // "submit", "execute_start", "checkpoint", "complete", "cancel", etc.
    pub timestamp: String,
    pub details: String,    // json or human summary, including security decisions
    pub prev_hash: String,  // chain
}

pub struct TamperEvidentAuditLogger {
    hasher: Mutex<Hasher>,
    last_hash: Mutex<String>,
    events: Mutex<Vec<AuditEvent>>,
    // In prod: persist to file/DB with the events.
}

impl TamperEvidentAuditLogger {
    pub fn new() -> Self {
        Self {
            hasher: Mutex::new(Hasher::new()),
            last_hash: Mutex::new("genesis".to_string()),
            events: Mutex::new(Vec::new()),
        }
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

        // In full impl: also write to append-only store with proof.
        // Make persistence easier: (commented append-only file example -- enable by uncommenting for file-backed durability)
        // Example (append-only, simple; real would include atomic write + fsync + hash proof):
        //   if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("uniflow_audit.log") {
        //       let _ = writeln!(f, "{}|{}|{}|{}|{}|{}", event.job_id, event.event_type, event.timestamp, event.details, event.prev_hash, new_hash);
        //       // Consider: use serde_json + line delimited for easy replay; rotate by size/date.
        //   }
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
    ///   - each event's recorded prev_hash matches the expected from prior step
    ///   - final computed root matches the logger's current_root()
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