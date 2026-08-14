use serde::Serialize;
use tokio::sync::broadcast;

use crate::server::jobs::{DownloadJob, JobEventKind};

const DEFAULT_EVENT_CAPACITY: usize = 128;

#[derive(Debug, Clone, Serialize)]
pub struct JobEvent {
    pub job_id: String,
    pub kind: String,
    pub state: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_per_second: Option<u64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub warning: Option<String>,
}

impl JobEvent {
    pub fn from_job(job: &DownloadJob, kind: JobEventKind) -> Self {
        Self {
            job_id: job.id.to_string(),
            kind: kind.as_str().to_string(),
            state: job.state.as_str().to_string(),
            downloaded_bytes: job.downloaded_bytes,
            total_bytes: job.total_bytes,
            speed_bytes_per_second: job.speed_bytes_per_second,
            error_code: sanitize_field(job.error_code.clone()),
            error_message: sanitize_field(job.error_message.clone()),
            warning: sanitize_field(job.warning.clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct JobEventBus {
    sender: broadcast::Sender<JobEvent>,
}

impl Default for JobEventBus {
    fn default() -> Self {
        Self::new(DEFAULT_EVENT_CAPACITY)
    }
}

impl JobEventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.sender.subscribe()
    }

    pub fn publish_job(&self, job: &DownloadJob, kind: JobEventKind) {
        let _ = self.sender.send(JobEvent::from_job(job, kind));
    }
}

fn sanitize_field(value: Option<String>) -> Option<String> {
    let value = value?;
    if value.contains("://")
        || value.contains("../")
        || value.contains("..\\")
        || value.starts_with('/')
        || value.contains('\\')
    {
        return None;
    }
    Some(value)
}
