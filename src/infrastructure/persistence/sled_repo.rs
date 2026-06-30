//! Durable `JobRepository` over the embedded, pure-Rust `sled` KV store (JULES-11).
//!
//! Design:
//!   * Each job is stored under its UUID bytes; the value is `serde_json(job)` with a
//!     trailing **BLAKE3 integrity tag** (32 bytes). `load`/`list` verify the tag and
//!     reject (skip, with a warning) any corrupt/tampered record rather than handing
//!     back garbage — the same integrity discipline as the snapshot path.
//!   * `sled` is crash-safe and already durable, so `snapshot()` just flushes.
//!   * `migrate_from_snapshot` performs a one-time import of the legacy
//!     `InMemoryJobRepository` JSON+tag snapshot (idempotent: existing keys win).
//!
//! Opt-in: the daemon selects this only when `UNIFLOW_SLED_PATH` is set; the
//! in-memory repository remains the default (tests rely on it). Clean-architecture
//! is preserved — this implements the existing port with no signature changes.
//!
//! Note: `sled` is synchronous; its ops are fast and run inline inside the async
//! methods (the in-memory repo likewise holds a lock across its await points).

use crate::application::ports::JobRepository;
use crate::domain::{Job, JobId};
use crate::error::{Result, UniFlowError};
use async_trait::async_trait;
use std::path::Path;
use tracing::warn;

const TAG_LEN: usize = 32;

pub struct SledJobRepository {
    db: sled::Db,
}

impl SledJobRepository {
    /// Open (or create) the store at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = sled::open(path.as_ref())
            .map_err(|e| UniFlowError::Internal(format!("sled open failed: {e}")))?;
        Ok(Self { db })
    }

    /// Encode a job as `json || blake3(json)` for tamper-evident storage.
    fn encode(job: &Job) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(job)?;
        let tag = blake3::hash(&bytes);
        bytes.extend_from_slice(tag.as_bytes());
        Ok(bytes)
    }

    /// Verify the integrity tag and decode. Returns `None` on a corrupt record.
    fn decode(raw: &[u8]) -> Option<Job> {
        if raw.len() < TAG_LEN {
            return None;
        }
        let (payload, tag) = raw.split_at(raw.len() - TAG_LEN);
        if blake3::hash(payload).as_bytes() != tag {
            return None;
        }
        serde_json::from_slice::<Job>(payload).ok()
    }

    fn flush(&self) -> Result<()> {
        self.db
            .flush()
            .map_err(|e| UniFlowError::Internal(format!("sled flush failed: {e}")))?;
        Ok(())
    }

    /// One-time import from a legacy snapshot file (`Vec<Job>` JSON + BLAKE3 tag,
    /// the format `InMemoryJobRepository` writes). Existing keys are not overwritten.
    /// Returns the number of jobs imported.
    pub async fn migrate_from_snapshot(&self, snapshot_path: impl AsRef<Path>) -> Result<usize> {
        let path = snapshot_path.as_ref();
        if !path.exists() {
            return Ok(0);
        }
        let data = tokio::fs::read(path).await?;
        if data.len() < TAG_LEN {
            warn!("snapshot too small for integrity tag; skipping migration");
            return Ok(0);
        }
        let (payload, tag) = data.split_at(data.len() - TAG_LEN);
        if blake3::hash(payload).as_bytes() != tag {
            warn!("snapshot integrity check failed; skipping migration");
            return Ok(0);
        }
        let jobs: Vec<Job> = match serde_json::from_slice(payload) {
            Ok(j) => j,
            Err(e) => {
                warn!("snapshot decode failed ({e}); skipping migration");
                return Ok(0);
            }
        };
        let mut imported = 0;
        for job in jobs {
            if job.id.is_nil() {
                continue;
            }
            let key = job.id.as_bytes();
            if self
                .db
                .get(key)
                .map_err(|e| UniFlowError::Internal(format!("sled get failed: {e}")))?
                .is_none()
            {
                self.db
                    .insert(key, Self::encode(&job)?)
                    .map_err(|e| UniFlowError::Internal(format!("sled insert failed: {e}")))?;
                imported += 1;
            }
        }
        self.flush()?;
        Ok(imported)
    }
}

#[async_trait]
impl JobRepository for SledJobRepository {
    async fn save(&self, job: &Job) -> Result<()> {
        self.db
            .insert(job.id.as_bytes(), Self::encode(job)?)
            .map_err(|e| UniFlowError::Internal(format!("sled insert failed: {e}")))?;
        self.flush()
    }

    async fn load(&self, id: JobId) -> Result<Job> {
        let raw = self
            .db
            .get(id.as_bytes())
            .map_err(|e| UniFlowError::Internal(format!("sled get failed: {e}")))?
            .ok_or(UniFlowError::JobNotFound(id))?;
        Self::decode(&raw).ok_or_else(|| {
            UniFlowError::Internal(format!("job {id} record failed integrity verification"))
        })
    }

    async fn list(&self) -> Result<Vec<Job>> {
        let mut jobs = Vec::new();
        for item in self.db.iter() {
            let (_k, v) =
                item.map_err(|e| UniFlowError::Internal(format!("sled iter failed: {e}")))?;
            match Self::decode(&v) {
                Some(job) => jobs.push(job),
                None => warn!("skipping corrupt/tampered job record during list"),
            }
        }
        Ok(jobs)
    }

    async fn remove(&self, id: JobId) -> Result<()> {
        self.db
            .remove(id.as_bytes())
            .map_err(|e| UniFlowError::Internal(format!("sled remove failed: {e}")))?;
        self.flush()
    }

    async fn snapshot(&self) -> Result<()> {
        // sled is already durable; flush to guarantee the WAL is persisted.
        self.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Destination, Endpoint, Source, TransferMode};

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push("uniflow_sled_test");
        let _ = std::fs::create_dir_all(&p);
        // Unique per test to avoid cross-test contention.
        p.push(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn sample_job() -> Job {
        Job::new(
            Source::from(Endpoint::Local { path: "/tmp/a".into() }),
            Destination::from(Endpoint::Local { path: "/tmp/b".into() }),
            TransferMode::Copy,
        )
    }

    #[tokio::test]
    async fn crud_roundtrip() {
        let path = temp_db_path("crud");
        let repo = SledJobRepository::open(&path).unwrap();
        let job = sample_job();
        let id = job.id;

        repo.save(&job).await.unwrap();
        let loaded = repo.load(id).await.unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(repo.list().await.unwrap().len(), 1);

        repo.remove(id).await.unwrap();
        assert!(repo.load(id).await.is_err());
        assert_eq!(repo.list().await.unwrap().len(), 0);

        drop(repo);
        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn persists_across_reopen() {
        let path = temp_db_path("persist");
        let job = sample_job();
        let id = job.id;

        {
            let repo = SledJobRepository::open(&path).unwrap();
            repo.save(&job).await.unwrap();
            repo.snapshot().await.unwrap();
        } // drop closes the db

        // Reopen: the job must still be there (durability across "restart").
        let repo = SledJobRepository::open(&path).unwrap();
        let loaded = repo.load(id).await.unwrap();
        assert_eq!(loaded.id, id);

        drop(repo);
        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn tampered_record_is_rejected() {
        let path = temp_db_path("tamper");
        let repo = SledJobRepository::open(&path).unwrap();
        let job = sample_job();
        let id = job.id;
        repo.save(&job).await.unwrap();

        // Corrupt the stored value directly (flip a payload byte, keep length).
        let raw = repo.db.get(id.as_bytes()).unwrap().unwrap();
        let mut corrupt = raw.to_vec();
        corrupt[0] ^= 0xFF;
        repo.db.insert(id.as_bytes(), corrupt).unwrap();

        // load() must fail closed; list() must skip it.
        assert!(repo.load(id).await.is_err());
        assert_eq!(repo.list().await.unwrap().len(), 0);

        drop(repo);
        let _ = std::fs::remove_dir_all(&path);
    }
}
