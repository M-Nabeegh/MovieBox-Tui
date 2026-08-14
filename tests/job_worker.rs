#![cfg(feature = "server")]

#[path = "support/mod.rs"]
mod support;

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use moviebox_tui::{
    catalog::{
        CatalogDetails, CatalogError, CatalogId, CatalogProvider, EpisodeRequest, MediaType,
        ResolvedSource, SearchPage, SourceId, SourceOption, SubtitleId, SubtitleTrack,
    },
    server::{
        events::JobEventBus,
        jobs::{
            DownloadJob, JobEvent, JobEventKind, JobId, JobProgress, JobRepositoryError, JobState,
            JobStatePatch, JobStore, JobWorker, recover_interrupted_jobs,
        },
        library::LibraryNamer,
    },
};
use support::http_server::FixtureServer;
use tempfile::TempDir;
use time::OffsetDateTime;
use tokio::{fs, sync::Mutex};
use uuid::Uuid;

const DEFAULT_RESERVE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

#[derive(Clone)]
struct MockCatalog {
    resolutions: Arc<Mutex<VecDeque<Result<ResolvedSource, CatalogError>>>>,
}

impl MockCatalog {
    fn new(values: impl IntoIterator<Item = Result<ResolvedSource, CatalogError>>) -> Self {
        Self {
            resolutions: Arc::new(Mutex::new(values.into_iter().collect())),
        }
    }
}

#[async_trait]
impl CatalogProvider for MockCatalog {
    async fn search(&self, _query: &str, _page: u32) -> Result<SearchPage, CatalogError> {
        unreachable!("search is not used by worker tests")
    }

    async fn details(&self, _id: &CatalogId) -> Result<CatalogDetails, CatalogError> {
        unreachable!("details is not used by worker tests")
    }

    async fn sources(&self, _request: EpisodeRequest) -> Result<Vec<SourceOption>, CatalogError> {
        unreachable!("sources is not used by worker tests")
    }

    async fn subtitles(&self, _source: &SourceId) -> Result<Vec<SubtitleTrack>, CatalogError> {
        unreachable!("subtitles is not used by worker tests")
    }

    async fn resolve(
        &self,
        _source: &SourceId,
        _subtitle: Option<&SubtitleId>,
    ) -> Result<ResolvedSource, CatalogError> {
        self.resolutions
            .lock()
            .await
            .pop_front()
            .expect("missing mock resolution")
    }
}

#[derive(Debug, Clone)]
struct MockDiskSpace {
    available: u64,
}

impl MockDiskSpace {
    fn new(available: u64) -> Self {
        Self { available }
    }
}

#[async_trait]
impl moviebox_tui::server::jobs::DiskSpaceChecker for MockDiskSpace {
    async fn available_bytes(&self, _path: &Path) -> Result<u64, std::io::Error> {
        Ok(self.available)
    }
}

#[derive(Debug, Clone, Default)]
struct MockStore {
    jobs: Arc<Mutex<HashMap<JobId, DownloadJob>>>,
}

impl MockStore {
    fn with_jobs(jobs: impl IntoIterator<Item = DownloadJob>) -> Self {
        let jobs = jobs.into_iter().map(|job| (job.id, job)).collect();
        Self {
            jobs: Arc::new(Mutex::new(jobs)),
        }
    }

    async fn job(&self, id: JobId) -> DownloadJob {
        self.jobs
            .lock()
            .await
            .get(&id)
            .cloned()
            .unwrap_or_else(|| panic!("missing job {id}"))
    }
}

#[async_trait]
impl JobStore for MockStore {
    async fn claim_next(&self) -> Result<Option<DownloadJob>, JobRepositoryError> {
        let mut jobs = self.jobs.lock().await;
        let next_id = jobs
            .values()
            .filter(|job| job.state == JobState::Queued)
            .min_by_key(|job| (job.created_at, job.id.to_string()))
            .map(|job| job.id);

        let Some(id) = next_id else {
            return Ok(None);
        };

        let job = jobs.get_mut(&id).unwrap();
        job.state = JobState::Resolving;
        job.attempt += 1;
        job.updated_at = OffsetDateTime::now_utc();
        job.version += 1;
        Ok(Some(job.clone()))
    }

    async fn get(&self, id: JobId) -> Result<DownloadJob, JobRepositoryError> {
        self.jobs
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or(JobRepositoryError::NotFound(id))
    }

    async fn list_all(&self) -> Result<Vec<DownloadJob>, JobRepositoryError> {
        Ok(self.jobs.lock().await.values().cloned().collect())
    }

    async fn transition(
        &self,
        id: JobId,
        expected_version: i64,
        to: JobState,
        event: Option<JobEvent>,
    ) -> Result<DownloadJob, JobRepositoryError> {
        let mut jobs = self.jobs.lock().await;
        let job = jobs.get_mut(&id).ok_or(JobRepositoryError::NotFound(id))?;
        if job.version != expected_version {
            return Err(JobRepositoryError::Conflict {
                expected_version,
                actual_version: job.version,
            });
        }
        job.state = to;
        job.updated_at = OffsetDateTime::now_utc();
        job.version += 1;
        if to == JobState::Failed {
            job.error_code = event.as_ref().and_then(|value| value.error_code.clone());
            job.error_message = event.as_ref().and_then(|value| value.error_message.clone());
        } else {
            job.error_code = None;
            job.error_message = None;
        }
        if let Some(warning) = event.and_then(|value| value.warning) {
            job.warning = Some(warning);
        }
        Ok(job.clone())
    }

    async fn update_progress(
        &self,
        id: JobId,
        progress: JobProgress,
        _event: Option<JobEvent>,
    ) -> Result<DownloadJob, JobRepositoryError> {
        let mut jobs = self.jobs.lock().await;
        let job = jobs.get_mut(&id).ok_or(JobRepositoryError::NotFound(id))?;
        if job.version != progress.expected_version {
            return Err(JobRepositoryError::Conflict {
                expected_version: progress.expected_version,
                actual_version: job.version,
            });
        }
        job.downloaded_bytes = progress.downloaded_bytes;
        job.total_bytes = progress.total_bytes;
        job.speed_bytes_per_second = progress.speed_bytes_per_second;
        job.updated_at = OffsetDateTime::now_utc();
        job.version += 1;
        Ok(job.clone())
    }

    async fn force_state(
        &self,
        id: JobId,
        expected_version: i64,
        to: JobState,
        _kind: JobEventKind,
        patch: JobStatePatch,
    ) -> Result<DownloadJob, JobRepositoryError> {
        let mut jobs = self.jobs.lock().await;
        let job = jobs.get_mut(&id).ok_or(JobRepositoryError::NotFound(id))?;
        if job.version != expected_version {
            return Err(JobRepositoryError::Conflict {
                expected_version,
                actual_version: job.version,
            });
        }
        job.state = to;
        if let Some(value) = patch.downloaded_bytes {
            job.downloaded_bytes = value;
        }
        if let Some(value) = patch.total_bytes {
            job.total_bytes = value;
        }
        if let Some(value) = patch.speed_bytes_per_second {
            job.speed_bytes_per_second = value;
        }
        if let Some(value) = patch.error_code {
            job.error_code = value;
        }
        if let Some(value) = patch.error_message {
            job.error_message = value;
        }
        if let Some(value) = patch.warning {
            job.warning = value;
        }
        job.updated_at = OffsetDateTime::now_utc();
        job.version += 1;
        Ok(job.clone())
    }
}

fn build_job(root: &Path, seed: &str, state: JobState) -> DownloadJob {
    let id = JobId::new(Uuid::new_v4());
    let base = format!("Fixture Title {seed}");
    std::fs::create_dir_all(root.join("_moviebox").join("jobs").join(id.to_string())).unwrap();

    DownloadJob {
        id,
        catalog_id: CatalogId::new(format!("catalog-{seed}")),
        source_id: SourceId::new(format!("source-{seed}")),
        subtitle_id: None,
        title: base.clone(),
        year: Some("2024".to_string()),
        media_type: MediaType::Movie,
        season_number: None,
        episode_number: None,
        episode_title: None,
        requested_height: 1080,
        state,
        final_video_path: format!("Movies/{base} (2024)/{base} (2024).mkv"),
        final_subtitle_path: None,
        partial_video_path: format!("_moviebox/jobs/{}/{base}.mkv.part", id),
        partial_subtitle_path: None,
        downloaded_bytes: 0,
        total_bytes: None,
        speed_bytes_per_second: None,
        attempt: 0,
        error_code: None,
        error_message: None,
        warning: None,
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
        version: 0,
    }
}

struct WorkerHarness {
    _temp: TempDir,
    media_root: PathBuf,
    namer: LibraryNamer,
}

impl WorkerHarness {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let media_root = temp.path().join("media");
        std::fs::create_dir_all(&media_root).unwrap();
        let namer = LibraryNamer::new(&media_root).unwrap();
        Self {
            _temp: temp,
            media_root,
            namer,
        }
    }

    fn worker(
        &self,
        store: Arc<MockStore>,
        catalog: Arc<MockCatalog>,
        transfer: moviebox_tui::server::jobs::HttpTransferClient,
        bus: JobEventBus,
        disk: Arc<MockDiskSpace>,
    ) -> JobWorker<
        MockStore,
        MockCatalog,
        moviebox_tui::server::jobs::HttpTransferClient,
        MockDiskSpace,
    > {
        JobWorker::new(store, catalog, Arc::new(transfer), self.namer.clone(), bus)
            .with_disk_space_checker(disk)
    }
}

#[tokio::test]
async fn startup_requeues_interrupted_job_and_preserves_partial_file() {
    let harness = WorkerHarness::new();
    let job = build_job(&harness.media_root, "restart", JobState::Downloading);
    let partial = harness.media_root.join(&job.partial_video_path);
    fs::write(&partial, b"fixture-prefix").await.unwrap();
    let store = MockStore::with_jobs([job.clone()]);

    recover_interrupted_jobs(&store, harness.namer.media_root())
        .await
        .unwrap();

    let repaired = store.job(job.id).await;
    assert_eq!(repaired.state, JobState::Queued);
    assert!(partial.exists());
}

#[tokio::test]
async fn worker_completes_one_job_and_finalizes_into_the_library() {
    let harness = WorkerHarness::new();
    let server = FixtureServer::start(256 * 1024).await.unwrap();
    let job = build_job(&harness.media_root, "complete", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Ok(ResolvedSource {
        url: server.url("/download"),
        headers: FixtureServer::required_headers(),
        subtitle: None,
        extension: "mkv".to_string(),
        expected_size: Some(server.content_len() as u64),
    })]));
    let worker = harness.worker(
        store.clone(),
        catalog,
        moviebox_tui::server::jobs::HttpTransferClient::new(server.client()),
        JobEventBus::new(16),
        Arc::new(MockDiskSpace::new(
            DEFAULT_RESERVE_BYTES + server.content_len() as u64 + 1,
        )),
    );

    assert!(worker.run_once().await.unwrap());

    let completed = store.job(job.id).await;
    assert_eq!(completed.state, JobState::Ready);
    assert!(
        harness
            .media_root
            .join(&completed.final_video_path)
            .exists()
    );
    assert!(
        !harness
            .media_root
            .join(&completed.partial_video_path)
            .exists()
    );
}

#[tokio::test]
async fn insufficient_space_event_is_sanitized() {
    let harness = WorkerHarness::new();
    let server = FixtureServer::start(8 * 1024).await.unwrap();
    let job = build_job(&harness.media_root, "space", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Ok(ResolvedSource {
        url: server.url("/download"),
        headers: FixtureServer::required_headers(),
        subtitle: None,
        extension: "mkv".to_string(),
        expected_size: Some(server.content_len() as u64),
    })]));
    let bus = JobEventBus::new(16);
    let mut events = bus.subscribe();
    let worker = harness.worker(
        store.clone(),
        catalog,
        moviebox_tui::server::jobs::HttpTransferClient::new(server.client()),
        bus,
        Arc::new(MockDiskSpace::new(DEFAULT_RESERVE_BYTES)),
    );

    assert!(worker.run_once().await.unwrap());

    let requeued = store.job(job.id).await;
    assert_eq!(requeued.state, JobState::Queued);
    assert_eq!(requeued.error_code.as_deref(), Some("insufficient_space"));
    assert!(server.requests().is_empty());

    let mut saw_space_error = false;
    for _ in 0..4 {
        let event = events.recv().await.unwrap();
        let serialized = serde_json::to_string(&event).unwrap();
        if serialized.contains("insufficient_space") {
            saw_space_error = true;
            assert!(!serialized.contains("http://"));
            assert!(!serialized.contains("fixture-token"));
            assert!(!serialized.contains(harness.media_root.to_string_lossy().as_ref()));
            break;
        }
    }

    assert!(saw_space_error, "expected an insufficient_space event");
}
