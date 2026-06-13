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

    /// Basic load support (skeleton per subagent recommendations for integrity).
    /// In production: add atomic writes, size/schema validation, optional BLAKE3/MAC/signature, and call on construction.
    pub async fn load_snapshot(&self) -> Result<()> {
        if let Some(path) = &self.snapshot_path {
            if path.exists() {
                let data = tokio::fs::read(path).await?;
                // TODO (security): verify size, blake3 root or HMAC before deserial; reject malformed
                if let Ok(list) = serde_json::from_slice::<Vec<Job>>(&data) {
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
            let json = serde_json::to_vec_pretty(&list)?;
            tokio::fs::write(path, &json).await?;
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
            .ok_or_else(|| crate::error::UniFlowError::JobNotFound(id))
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
