//! In-memory implementation of the JobRepository port (with optional JSON snapshot).

use crate::application::ports::JobRepository;
use crate::domain::{Job, JobId};
use crate::error::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct InMemoryJobRepository {
    jobs: Arc<Mutex<HashMap<JobId, Job>>>,
    snapshot_path: Option<PathBuf>,
}

impl InMemoryJobRepository {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            snapshot_path: None,
        }
    }

    pub fn with_snapshot(mut self, path: PathBuf) -> Self {
        self.snapshot_path = Some(path);
        self
    }

    pub async fn load_snapshot(&self) -> Result<()> {
        if let Some(path) = &self.snapshot_path {
            if path.exists() {
                let data = tokio::fs::read(path).await?;
                
                // JULES-06: Integrity verification
                if data.len() < 32 {
                    tracing::warn!("Snapshot file too small to contain integrity tag, skipping load");
                    return Ok(());
                }
                
                let (payload, expected_tag) = data.split_at(data.len() - 32);
                let computed_tag = blake3::hash(payload);
                
                if computed_tag.as_bytes() != expected_tag {
                    tracing::warn!("Snapshot integrity verification failed! File is tampered or corrupt. Skipping load.");
                    return Ok(());
                }
                
                if let Ok(list) = serde_json::from_slice::<Vec<Job>>(payload) {
                    let mut map = self.jobs.lock().await;
                    for j in list {
                        // Basic post-deser validation stub
                        if j.id.is_nil() { continue; }
                        map.insert(j.id, j);
                    }
                }
            }
        }
        Ok(())
    }

    async fn write_snapshot(&self) -> Result<()> {
        if let Some(path) = &self.snapshot_path {
            let jobs = self.jobs.lock().await;
            let list: Vec<&Job> = jobs.values().collect();
            let mut data = serde_json::to_vec_pretty(&list)?;
            
            // JULES-06: Append BLAKE3 integrity tag
            let tag = blake3::hash(&data);
            data.extend_from_slice(tag.as_bytes());
            
            // Atomic write
            let tmp_path = path.with_extension("tmp");
            tokio::fs::write(&tmp_path, &data).await?;
            tokio::fs::rename(&tmp_path, path).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl JobRepository for InMemoryJobRepository {
    async fn save(&self, job: &Job) -> Result<()> {
        let mut map = self.jobs.lock().await;
        map.insert(job.id, job.clone());
        drop(map);
        self.write_snapshot().await
    }

    async fn load(&self, id: JobId) -> Result<Job> {
        let map = self.jobs.lock().await;
        map.get(&id)
            .cloned()
            .ok_or(crate::error::UniFlowError::JobNotFound(id))
    }

    async fn list(&self) -> Result<Vec<Job>> {
        let map = self.jobs.lock().await;
        Ok(map.values().cloned().collect())
    }

    async fn remove(&self, id: JobId) -> Result<()> {
        let mut map = self.jobs.lock().await;
        map.remove(&id);
        drop(map);
        self.write_snapshot().await
    }

    async fn snapshot(&self) -> Result<()> {
        self.write_snapshot().await
    }
}

impl Default for InMemoryJobRepository {
    fn default() -> Self {
        Self::new()
    }
}
