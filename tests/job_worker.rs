#![cfg(feature = "server")]

#[path = "support/mod.rs"]
mod support;

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use moviebox_tui::{
    catalog::{
        CatalogDetails, CatalogError, CatalogId, CatalogProvider, EpisodeRequest, MediaType,
        ResolvedSource, ResolvedSubtitle, SearchPage, SourceId, SourceOption, SubtitleId,
        SubtitleTrack,
    },
    download::{DownloadError, DownloadOutcome, DownloadRequest},
    server::{
        events::JobEventBus,
        jobs::{
            DownloadJob, JobEvent, JobEventKind, JobId, JobProgress, JobRepositoryError, JobState,
            JobStatePatch, JobStore, JobWorker, MAX_AUTOMATIC_ATTEMPTS, TransferClient,
            TransferError, TransferProgress, WorkerRunOutcome, recover_interrupted_jobs,
            sweep_orphaned_partials,
        },
        library::LibraryNamer,
    },
};
use support::http_server::FixtureServer;
use tempfile::TempDir;
use time::OffsetDateTime;
use tokio::{
    fs,
    sync::{Mutex, mpsc},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const DEFAULT_RESERVE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

#[derive(Clone)]
struct MockCatalog {
    resolutions: Arc<Mutex<VecDeque<Result<ResolvedSource, CatalogError>>>>,
    resolves: Arc<AtomicUsize>,
}

impl MockCatalog {
    fn new(values: impl IntoIterator<Item = Result<ResolvedSource, CatalogError>>) -> Self {
        Self {
            resolutions: Arc::new(Mutex::new(values.into_iter().collect())),
            resolves: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn resolve_count(&self) -> usize {
        self.resolves.load(Ordering::Relaxed)
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
        self.resolves.fetch_add(1, Ordering::Relaxed);
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

#[derive(Debug, Clone, Copy)]
struct FailingTransfer;

#[async_trait]
impl TransferClient for FailingTransfer {
    async fn transfer(
        &self,
        _request: DownloadRequest,
        _destination: &Path,
        _cancel: CancellationToken,
        progress: mpsc::UnboundedSender<TransferProgress>,
    ) -> Result<DownloadOutcome, TransferError> {
        progress
            .send(TransferProgress {
                downloaded_bytes: 1,
                total_bytes: Some(10),
                speed_bytes_per_second: Some(1),
            })
            .unwrap();
        Err(TransferError::Io(std::io::Error::other(
            "fixture transfer failure",
        )))
    }
}

#[derive(Clone, Default)]
struct RefreshingTransfer {
    calls: Arc<AtomicUsize>,
}

#[derive(Clone, Default)]
struct CancellableTransfer {
    observed: Arc<AtomicUsize>,
}

impl CancellableTransfer {
    fn observed(&self) -> usize {
        self.observed.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl TransferClient for CancellableTransfer {
    async fn transfer(
        &self,
        _request: DownloadRequest,
        _destination: &Path,
        cancel: CancellationToken,
        _progress: mpsc::UnboundedSender<TransferProgress>,
    ) -> Result<DownloadOutcome, TransferError> {
        cancel.cancelled().await;
        self.observed.fetch_add(1, Ordering::Relaxed);
        Ok(DownloadOutcome::Paused { bytes: 0 })
    }
}

impl RefreshingTransfer {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Default)]
struct AlwaysUnauthorizedTransfer {
    calls: Arc<AtomicUsize>,
}

impl AlwaysUnauthorizedTransfer {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl TransferClient for AlwaysUnauthorizedTransfer {
    async fn transfer(
        &self,
        _request: DownloadRequest,
        _destination: &Path,
        _cancel: CancellationToken,
        _progress: mpsc::UnboundedSender<TransferProgress>,
    ) -> Result<DownloadOutcome, TransferError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Err(TransferError::Download(DownloadError::Http(
            reqwest::StatusCode::UNAUTHORIZED,
        )))
    }
}

#[async_trait]
impl TransferClient for RefreshingTransfer {
    async fn transfer(
        &self,
        _request: DownloadRequest,
        destination: &Path,
        _cancel: CancellationToken,
        _progress: mpsc::UnboundedSender<TransferProgress>,
    ) -> Result<DownloadOutcome, TransferError> {
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        if call == 0 {
            return Err(TransferError::Download(DownloadError::Http(
                reqwest::StatusCode::UNAUTHORIZED,
            )));
        }
        fs::write(destination, b"fixture-media").await.unwrap();
        Ok(DownloadOutcome::Completed { bytes: 13 })
    }
}

/// Records how many times the worker asked the media server to rescan.
#[derive(Debug, Default)]
struct CountingRefresher {
    calls: AtomicUsize,
}

impl CountingRefresher {
    fn count(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl moviebox_tui::server::jobs::LibraryRefresher for CountingRefresher {
    async fn refresh(&self) -> Result<(), ()> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Captures what the worker announced, so tests can assert on the message.
#[derive(Debug, Default)]
struct RecordingNotifier {
    sent: Mutex<Vec<(String, Option<String>)>>,
}

impl RecordingNotifier {
    async fn sent(&self) -> Vec<(String, Option<String>)> {
        self.sent.lock().await.clone()
    }
}

#[async_trait]
impl moviebox_tui::server::notify::DownloadNotifier for RecordingNotifier {
    async fn notify_ready(&self, _job_id: &str, title: &str, year: Option<&str>) {
        self.sent
            .lock()
            .await
            .push((title.to_string(), year.map(str::to_string)));
    }
}

fn resolved_source(url: &str, expected_size: Option<u64>) -> ResolvedSource {
    ResolvedSource {
        url: url.parse().expect("fixture url is valid"),
        headers: Default::default(),
        subtitle: None,
        extension: "mkv".to_string(),
        expected_size,
        catalog_size_bytes: None,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
    }
}

#[derive(Debug, Clone, Default)]
struct MockStore {
    jobs: Arc<Mutex<HashMap<JobId, DownloadJob>>>,
    claims: Arc<AtomicUsize>,
}

impl MockStore {
    fn with_jobs(jobs: impl IntoIterator<Item = DownloadJob>) -> Self {
        let jobs = jobs.into_iter().map(|job| (job.id, job)).collect();
        Self {
            jobs: Arc::new(Mutex::new(jobs)),
            claims: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn claim_count(&self) -> usize {
        self.claims.load(Ordering::Relaxed)
    }

    async fn insert(&self, job: DownloadJob) {
        self.jobs.lock().await.insert(job.id, job);
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
        self.claims.fetch_add(1, Ordering::Relaxed);
        let mut jobs = self.jobs.lock().await;
        let now = OffsetDateTime::now_utc();
        let next_id = jobs
            .values()
            .filter(|job| job.state == JobState::Queued)
            .filter(|job| job.next_attempt_at.is_none_or(|due| due <= now))
            .min_by_key(|job| (job.created_at, job.id.to_string()))
            .map(|job| job.id);

        let Some(id) = next_id else {
            return Ok(None);
        };

        let job = jobs.get_mut(&id).unwrap();
        job.state = JobState::Resolving;
        job.attempt += 1;
        job.next_attempt_at = None;
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
        if let Some(value) = patch.next_attempt_at {
            job.next_attempt_at = value;
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
        next_attempt_at: None,
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

#[tokio::test(start_paused = true)]
async fn idle_worker_sleeps_instead_of_polling_in_a_tight_loop() {
    let harness = WorkerHarness::new();
    let store = Arc::new(MockStore::default());
    let worker = JobWorker::new(
        store.clone(),
        Arc::new(MockCatalog::new([])),
        Arc::new(FailingTransfer),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)));

    let cancel = CancellationToken::new();
    let running = tokio::spawn({
        let worker = worker.clone();
        let cancel = cancel.clone();
        async move { worker.run(cancel).await }
    });

    // An empty queue for a minute must not mean hundreds of database hits: the
    // worker should wake only about once per idle interval.
    tokio::time::sleep(Duration::from_secs(60)).await;
    cancel.cancel();
    let _ = running.await;

    let claims = store.claim_count();
    assert!(
        claims <= 20,
        "idle worker polled {claims} times in a minute; it should sleep between checks"
    );
}

#[tokio::test(start_paused = true)]
async fn queued_work_wakes_the_worker_without_waiting_for_the_poll() {
    let harness = WorkerHarness::new();
    let store = Arc::new(MockStore::default());
    let signal = moviebox_tui::server::jobs::JobSignal::new();
    let worker = JobWorker::new(
        store.clone(),
        Arc::new(MockCatalog::new([Err(CatalogError::NotFound("source"))])),
        Arc::new(FailingTransfer),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)))
    .with_signal(signal.clone());

    let cancel = CancellationToken::new();
    let running = tokio::spawn({
        let worker = worker.clone();
        let cancel = cancel.clone();
        async move { worker.run(cancel).await }
    });

    // Let the worker settle into its idle wait, then queue work and signal it.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let job = build_job(&harness.media_root, "signalled", JobState::Queued);
    store.insert(job.clone()).await;
    signal.wake();

    // Far less than the idle interval: the job must start on the signal alone.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let claimed = store.job(job.id).await;
    cancel.cancel();
    let _ = running.await;

    assert_ne!(
        claimed.state,
        JobState::Queued,
        "a signalled job should start without waiting for the idle poll"
    );
}

#[tokio::test]
async fn sweep_reclaims_partials_from_jobs_that_can_no_longer_resume() {
    let harness = WorkerHarness::new();
    let abandoned = build_job(&harness.media_root, "abandoned", JobState::Failed);
    let active = build_job(&harness.media_root, "active", JobState::Downloading);

    let abandoned_dir = harness
        .media_root
        .join("_moviebox")
        .join("jobs")
        .join(abandoned.id.to_string());
    let active_dir = harness
        .media_root
        .join("_moviebox")
        .join("jobs")
        .join(active.id.to_string());
    fs::write(abandoned_dir.join("video.mkv.part.0"), vec![0_u8; 5000])
        .await
        .unwrap();
    fs::write(active_dir.join("video.mkv.part.0"), vec![0_u8; 1000])
        .await
        .unwrap();

    let store = MockStore::with_jobs([abandoned.clone(), active.clone()]);
    let reclaimed = sweep_orphaned_partials(&store, harness.namer.media_root())
        .await
        .unwrap();

    assert_eq!(reclaimed, 5000);
    assert!(!abandoned_dir.exists());
    assert!(
        active_dir.join("video.mkv.part.0").exists(),
        "a resumable job must keep its partial data"
    );
}

#[tokio::test]
async fn sweep_ignores_directories_that_are_not_job_ids() {
    let harness = WorkerHarness::new();
    let unrelated = harness
        .media_root
        .join("_moviebox")
        .join("jobs")
        .join("notes");
    fs::create_dir_all(&unrelated).await.unwrap();
    fs::write(unrelated.join("keep.txt"), b"keep me")
        .await
        .unwrap();

    let store = MockStore::default();
    let reclaimed = sweep_orphaned_partials(&store, harness.namer.media_root())
        .await
        .unwrap();

    assert_eq!(reclaimed, 0);
    assert!(unrelated.join("keep.txt").exists());
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
        catalog_size_bytes: None,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
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

    assert_eq!(
        worker.run_once().await.unwrap(),
        WorkerRunOutcome::Progressed
    );

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
async fn worker_refreshes_once_after_an_initial_401_without_consuming_a_retry() {
    let harness = WorkerHarness::new();
    let job = build_job(&harness.media_root, "refresh-401", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([
        Ok(resolved_source("http://example.com/first.mkv", Some(13))),
        Ok(resolved_source(
            "http://example.com/refreshed.mkv",
            Some(13),
        )),
    ]));
    let transfer = RefreshingTransfer::default();
    let worker = JobWorker::new(
        store.clone(),
        catalog.clone(),
        Arc::new(transfer.clone()),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)));

    assert_eq!(
        worker.run_once().await.unwrap(),
        WorkerRunOutcome::Progressed
    );
    assert_eq!(catalog.resolve_count(), 2);
    assert_eq!(transfer.calls(), 2);
    assert_eq!(store.job(job.id).await.state, JobState::Ready);
    assert_eq!(store.job(job.id).await.attempt, 1);
}

#[tokio::test]
async fn worker_stops_after_one_auth_reresolution_when_the_refreshed_source_also_fails() {
    let harness = WorkerHarness::new();
    let job = build_job(&harness.media_root, "refresh-401-twice", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([
        Ok(resolved_source("http://example.com/first.mkv", Some(13))),
        Ok(resolved_source(
            "http://example.com/refreshed.mkv",
            Some(13),
        )),
    ]));
    let transfer = AlwaysUnauthorizedTransfer::default();
    let worker = JobWorker::new(
        store.clone(),
        catalog.clone(),
        Arc::new(transfer.clone()),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)));

    assert_eq!(
        worker.run_once().await.unwrap(),
        WorkerRunOutcome::Progressed
    );
    assert_eq!(catalog.resolve_count(), 2);
    assert_eq!(transfer.calls(), 2);
    let after = store.job(job.id).await;
    assert_eq!(after.state, JobState::Queued);
    assert_eq!(after.attempt, 1);
}

#[tokio::test]
async fn worker_shutdown_cancels_an_active_transfer_and_pauses_the_job() {
    let harness = WorkerHarness::new();
    let job = build_job(&harness.media_root, "cancel-active", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Ok(resolved_source(
        "http://example.com/active.mkv",
        None,
    ))]));
    let transfer = CancellableTransfer::default();
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(transfer.clone()),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)));
    let cancellation = CancellationToken::new();
    let running = tokio::spawn({
        let cancellation = cancellation.clone();
        async move { worker.run(cancellation).await }
    });

    tokio::time::timeout(Duration::from_secs(2), async {
        while store.job(job.id).await.state != JobState::Downloading {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(transfer.observed(), 1);
    assert_eq!(store.job(job.id).await.state, JobState::Paused);
}

#[tokio::test]
async fn worker_rejects_a_transfer_smaller_than_the_catalog_advertised() {
    let harness = WorkerHarness::new();
    let server = FixtureServer::start(256 * 1024).await.unwrap();
    let job = build_job(
        &harness.media_root,
        "short-provider-response",
        JobState::Queued,
    );
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Ok(ResolvedSource {
        url: server.url("/download"),
        headers: FixtureServer::required_headers(),
        subtitle: None,
        extension: "mkv".to_string(),
        expected_size: Some((server.content_len() * 2) as u64),
        catalog_size_bytes: None,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
    })]));
    let worker = harness.worker(
        store.clone(),
        catalog,
        moviebox_tui::server::jobs::HttpTransferClient::new(server.client()),
        JobEventBus::new(16),
        Arc::new(MockDiskSpace::new(
            DEFAULT_RESERVE_BYTES + (server.content_len() * 2) as u64 + 1,
        )),
    );

    assert_eq!(
        worker.run_once().await.unwrap(),
        WorkerRunOutcome::Progressed
    );

    let rejected = store.job(job.id).await;
    assert_eq!(rejected.state, JobState::Queued);
    assert_eq!(rejected.error_code.as_deref(), Some("incomplete_download"));
    assert_eq!(
        rejected.total_bytes,
        Some((server.content_len() * 2) as u64)
    );
    assert!(
        !harness.media_root.join(&rejected.final_video_path).exists(),
        "a response that contradicts the catalog size must not enter the library"
    );
}

#[tokio::test]
async fn transfer_failure_after_progress_does_not_stop_worker_on_version_conflict() {
    let harness = WorkerHarness::new();
    let server = FixtureServer::start(8 * 1024).await.unwrap();
    let job = build_job(&harness.media_root, "failure", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Ok(ResolvedSource {
        url: server.url("/download"),
        headers: FixtureServer::required_headers(),
        subtitle: None,
        extension: "mkv".to_string(),
        expected_size: Some(10),
        catalog_size_bytes: None,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
    })]));
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(FailingTransfer),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)));

    assert_eq!(
        worker.run_once().await.unwrap(),
        WorkerRunOutcome::Progressed
    );

    // A transfer error is recoverable while attempts remain, so the job returns
    // to the queue behind a backoff instead of ending.
    let requeued = store.job(job.id).await;
    assert_eq!(requeued.state, JobState::Queued);
    assert_eq!(requeued.downloaded_bytes, 1);
    assert_eq!(requeued.error_code.as_deref(), Some("download_failed"));
    assert!(requeued.next_attempt_at.is_some());
}

#[tokio::test]
async fn transfer_failure_keeps_partial_bytes_for_the_next_attempt() {
    let harness = WorkerHarness::new();
    let job = build_job(&harness.media_root, "keep-partial", JobState::Queued);
    let partial = harness
        .media_root
        .join(job.partial_video_path.trim_end_matches(".part"));
    fs::create_dir_all(partial.parent().unwrap()).await.unwrap();
    fs::write(&partial, vec![7_u8; 4096]).await.unwrap();

    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Ok(resolved_source(
        "http://127.0.0.1:1/download",
        Some(10),
    ))]));
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(FailingTransfer),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)));

    worker.run_once().await.unwrap();

    assert_eq!(store.job(job.id).await.state, JobState::Queued);
    assert!(
        partial.exists(),
        "a retryable failure must not discard downloaded bytes"
    );
}

#[tokio::test]
async fn exhausted_attempts_fail_the_job_and_reclaim_the_partials() {
    let harness = WorkerHarness::new();
    let mut job = build_job(&harness.media_root, "exhausted", JobState::Queued);
    // `claim_next` increments the attempt counter, so this claim spends the last
    // permitted attempt.
    job.attempt = MAX_AUTOMATIC_ATTEMPTS;
    let partial_dir = harness
        .media_root
        .join("_moviebox")
        .join("jobs")
        .join(job.id.to_string());
    fs::create_dir_all(&partial_dir).await.unwrap();
    fs::write(partial_dir.join("segment.part.0"), vec![1_u8; 2048])
        .await
        .unwrap();

    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Ok(resolved_source(
        "http://127.0.0.1:1/download",
        Some(10),
    ))]));
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(FailingTransfer),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)));

    worker.run_once().await.unwrap();

    let failed = store.job(job.id).await;
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("download_failed"));
    assert!(
        !partial_dir.exists(),
        "a terminally failed job must not leave gigabytes of segments behind"
    );
}

#[tokio::test]
async fn transient_resolve_failure_requeues_instead_of_failing() {
    let harness = WorkerHarness::new();
    let job = build_job(&harness.media_root, "resolve-flap", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Err(CatalogError::Provider(
        "upstream 502".to_string(),
    ))]));
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(FailingTransfer),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)));

    worker.run_once().await.unwrap();

    let requeued = store.job(job.id).await;
    assert_eq!(requeued.state, JobState::Queued);
    assert_eq!(
        requeued.error_code.as_deref(),
        Some("source_resolve_failed")
    );
    assert!(requeued.next_attempt_at.is_some());
}

#[tokio::test]
async fn permanent_resolve_failure_fails_immediately() {
    let harness = WorkerHarness::new();
    let job = build_job(&harness.media_root, "resolve-gone", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Err(CatalogError::NotFound("source"))]));
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(FailingTransfer),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)));

    worker.run_once().await.unwrap();

    let failed = store.job(job.id).await;
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("source_resolve_failed"));
    assert!(
        failed.next_attempt_at.is_none(),
        "a permanent failure must not be scheduled for another attempt"
    );
}

#[tokio::test]
async fn requested_subtitle_is_downloaded_next_to_the_video() {
    let harness = WorkerHarness::new();
    let server = FixtureServer::start(8 * 1024).await.unwrap();
    let mut job = build_job(&harness.media_root, "with-subs", JobState::Queued);
    job.subtitle_id = Some(SubtitleId::new("subtitle-en".to_string()));
    job.final_subtitle_path = Some(
        job.final_video_path
            .replace(".mkv", ".English.srt")
            .to_string(),
    );
    job.partial_subtitle_path = Some(
        job.partial_video_path
            .replace(".mkv.part", ".English.srt.part")
            .to_string(),
    );

    let with_subtitle = || ResolvedSource {
        url: server.url("/download"),
        headers: FixtureServer::required_headers(),
        subtitle: Some(ResolvedSubtitle {
            url: server.url("/download"),
            headers: FixtureServer::required_headers(),
            language: "English".to_string(),
            extension: "srt".to_string(),
        }),
        extension: "mkv".to_string(),
        expected_size: Some(server.content_len() as u64),
        catalog_size_bytes: None,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
    };
    // The worker resolves once to download the video and again for the subtitle.
    let catalog = Arc::new(MockCatalog::new([Ok(with_subtitle()), Ok(with_subtitle())]));
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(moviebox_tui::server::jobs::HttpTransferClient::new(
            server.client(),
        )),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)));

    worker.run_once().await.unwrap();

    let completed = store.job(job.id).await;
    assert_eq!(completed.state, JobState::Ready);
    assert_eq!(completed.warning, None);
    let subtitle = harness
        .media_root
        .join(completed.final_subtitle_path.as_ref().unwrap());
    assert!(
        subtitle.exists(),
        "a job carrying a subtitle must finalize it beside the video"
    );
}

#[tokio::test]
async fn subtitle_without_a_destination_warns_instead_of_silently_skipping() {
    let harness = WorkerHarness::new();
    let server = FixtureServer::start(8 * 1024).await.unwrap();
    let mut job = build_job(&harness.media_root, "orphan-subs", JobState::Queued);
    // A subtitle was requested but no path was recorded for it.
    job.subtitle_id = Some(SubtitleId::new("subtitle-en".to_string()));

    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Ok(ResolvedSource {
        url: server.url("/download"),
        headers: FixtureServer::required_headers(),
        subtitle: None,
        extension: "mkv".to_string(),
        expected_size: Some(server.content_len() as u64),
        catalog_size_bytes: None,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
    })]));
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(moviebox_tui::server::jobs::HttpTransferClient::new(
            server.client(),
        )),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)));

    worker.run_once().await.unwrap();

    let completed = store.job(job.id).await;
    assert_eq!(completed.state, JobState::Ready);
    assert_eq!(completed.warning.as_deref(), Some("subtitle_path_missing"));
}

#[tokio::test]
async fn completed_job_triggers_a_single_library_refresh() {
    let harness = WorkerHarness::new();
    let server = FixtureServer::start(8 * 1024).await.unwrap();
    let job = build_job(&harness.media_root, "refresh", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Ok(ResolvedSource {
        url: server.url("/download"),
        headers: FixtureServer::required_headers(),
        subtitle: None,
        extension: "mkv".to_string(),
        expected_size: Some(server.content_len() as u64),
        catalog_size_bytes: None,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
    })]));
    let library = Arc::new(CountingRefresher::default());
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(moviebox_tui::server::jobs::HttpTransferClient::new(
            server.client(),
        )),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)))
    .with_library_refresher(library.clone());

    worker.run_once().await.unwrap();

    assert_eq!(store.job(job.id).await.state, JobState::Ready);
    assert_eq!(library.count(), 1);
}

#[tokio::test]
async fn completed_job_announces_the_title_as_ready_to_watch() {
    let harness = WorkerHarness::new();
    let server = FixtureServer::start(8 * 1024).await.unwrap();
    let job = build_job(&harness.media_root, "notify", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Ok(ResolvedSource {
        url: server.url("/download"),
        headers: FixtureServer::required_headers(),
        subtitle: None,
        extension: "mkv".to_string(),
        expected_size: Some(server.content_len() as u64),
        catalog_size_bytes: None,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
    })]));
    let notifier = Arc::new(RecordingNotifier::default());
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(moviebox_tui::server::jobs::HttpTransferClient::new(
            server.client(),
        )),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)))
    .with_notifier(notifier.clone());

    worker.run_once().await.unwrap();

    assert_eq!(store.job(job.id).await.state, JobState::Ready);
    let sent = notifier.sent().await;
    assert_eq!(
        sent.len(),
        1,
        "a finished download should notify exactly once"
    );
    assert_eq!(sent[0], (job.title.clone(), Some("2024".to_string())));
}

#[tokio::test]
async fn failed_job_does_not_announce_anything() {
    let harness = WorkerHarness::new();
    let job = build_job(&harness.media_root, "notify-failed", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Err(CatalogError::NotFound("source"))]));
    let notifier = Arc::new(RecordingNotifier::default());
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(FailingTransfer),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)))
    .with_notifier(notifier.clone());

    worker.run_once().await.unwrap();

    assert_eq!(store.job(job.id).await.state, JobState::Failed);
    assert!(
        notifier.sent().await.is_empty(),
        "a failed download must not claim to be ready to watch"
    );
}

#[tokio::test]
async fn failed_job_does_not_trigger_a_library_refresh() {
    let harness = WorkerHarness::new();
    let job = build_job(&harness.media_root, "no-refresh", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Err(CatalogError::NotFound("source"))]));
    let library = Arc::new(CountingRefresher::default());
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(FailingTransfer),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)))
    .with_library_refresher(library.clone());

    worker.run_once().await.unwrap();

    assert_eq!(store.job(job.id).await.state, JobState::Failed);
    assert_eq!(library.count(), 0);
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
        catalog_size_bytes: None,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
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

    assert_eq!(worker.run_once().await.unwrap(), WorkerRunOutcome::Deferred);

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

#[tokio::test]
async fn low_disk_run_stops_after_defer() {
    let harness = WorkerHarness::new();
    let server = FixtureServer::start(8 * 1024).await.unwrap();
    let job = build_job(&harness.media_root, "backoff", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let resolved = Ok(ResolvedSource {
        url: server.url("/download"),
        headers: FixtureServer::required_headers(),
        subtitle: None,
        extension: "mkv".to_string(),
        expected_size: Some(server.content_len() as u64),
        catalog_size_bytes: None,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
    });
    let catalog = Arc::new(MockCatalog::new([resolved]));
    let bus = JobEventBus::new(16);
    let mut events = bus.subscribe();
    let worker = harness.worker(
        store.clone(),
        catalog.clone(),
        moviebox_tui::server::jobs::HttpTransferClient::new(server.client()),
        bus,
        Arc::new(MockDiskSpace::new(DEFAULT_RESERVE_BYTES)),
    );
    tokio::time::timeout(
        Duration::from_millis(100),
        worker.run(CancellationToken::new()),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(store.claim_count(), 1);
    assert_eq!(catalog.resolve_count(), 1);
    let requeued = store.job(job.id).await;
    assert_eq!(requeued.state, JobState::Queued);
    assert_eq!(requeued.error_code.as_deref(), Some("insufficient_space"));
    assert!(events.try_recv().is_ok(), "expected a claimed event");
    assert!(events.try_recv().is_ok(), "expected a safe space event");
    assert!(events.try_recv().is_err(), "expected no repeated events");
}

#[tokio::test]
async fn forged_in_root_partial_path_fails_without_touching_file() {
    let harness = WorkerHarness::new();
    let server = FixtureServer::start(8 * 1024).await.unwrap();
    let mut job = build_job(&harness.media_root, "forged", JobState::Queued);
    job.partial_video_path = "Movies/forged.mkv.part".to_string();
    let forged = harness.media_root.join(&job.partial_video_path);
    fs::create_dir_all(forged.parent().unwrap()).await.unwrap();
    fs::write(&forged, b"do-not-touch").await.unwrap();
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let worker = harness.worker(
        store.clone(),
        Arc::new(MockCatalog::new([])),
        moviebox_tui::server::jobs::HttpTransferClient::new(server.client()),
        JobEventBus::new(16),
        Arc::new(MockDiskSpace::new(u64::MAX)),
    );

    assert_eq!(
        worker.run_once().await.unwrap(),
        WorkerRunOutcome::Progressed
    );

    let failed = store.job(job.id).await;
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("unsafe_path"));
    assert_eq!(fs::read(&forged).await.unwrap(), b"do-not-touch");
}

#[tokio::test]
async fn recovery_requeues_finalizing_job_without_recorded_size() {
    let harness = WorkerHarness::new();
    let job = build_job(&harness.media_root, "unknown-size", JobState::Finalizing);
    let final_path = harness.media_root.join(&job.final_video_path);
    fs::create_dir_all(final_path.parent().unwrap())
        .await
        .unwrap();
    fs::write(&final_path, b"present-but-unverified")
        .await
        .unwrap();
    let store = MockStore::with_jobs([job.clone()]);

    recover_interrupted_jobs(&store, harness.namer.media_root())
        .await
        .unwrap();

    assert_eq!(store.job(job.id).await.state, JobState::Queued);
}

/// Records the sync requests the worker made, and whether it claimed a change.
#[derive(Debug, Default)]
struct RecordingSyncer {
    calls: Mutex<Vec<(PathBuf, PathBuf)>>,
}

impl RecordingSyncer {
    async fn calls(&self) -> Vec<(PathBuf, PathBuf)> {
        self.calls.lock().await.clone()
    }
}

#[async_trait]
impl moviebox_tui::server::library::SubtitleSyncer for RecordingSyncer {
    async fn sync(&self, video: &Path, subtitle: &Path) -> bool {
        self.calls
            .lock()
            .await
            .push((video.to_path_buf(), subtitle.to_path_buf()));
        true
    }
}

#[tokio::test]
async fn a_downloaded_subtitle_is_aligned_against_its_video() {
    let harness = WorkerHarness::new();
    let server = FixtureServer::start(8 * 1024).await.unwrap();
    let mut job = build_job(&harness.media_root, "sync", JobState::Queued);
    job.subtitle_id = Some(SubtitleId::new("subtitle-en".to_string()));
    job.final_subtitle_path = Some(job.final_video_path.replace(".mkv", ".English.srt"));
    job.partial_subtitle_path = Some(
        job.partial_video_path
            .replace(".mkv.part", ".English.srt.part"),
    );

    let with_subtitle = || ResolvedSource {
        url: server.url("/download"),
        headers: FixtureServer::required_headers(),
        subtitle: Some(ResolvedSubtitle {
            url: server.url("/download"),
            headers: FixtureServer::required_headers(),
            language: "English".to_string(),
            extension: "srt".to_string(),
        }),
        extension: "mkv".to_string(),
        expected_size: Some(server.content_len() as u64),
        catalog_size_bytes: None,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
    };
    let catalog = Arc::new(MockCatalog::new([Ok(with_subtitle()), Ok(with_subtitle())]));
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let syncer = Arc::new(RecordingSyncer::default());
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(moviebox_tui::server::jobs::HttpTransferClient::new(
            server.client(),
        )),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)))
    .with_subtitle_syncer(syncer.clone());

    worker.run_once().await.unwrap();

    let completed = store.job(job.id).await;
    assert_eq!(completed.state, JobState::Ready);

    // Alignment must run against the finished files in the library, not the
    // scratch copies, which are gone by the time the job is ready.
    let calls = syncer.calls().await;
    assert_eq!(
        calls.len(),
        1,
        "the subtitle should be aligned exactly once"
    );
    let (video, subtitle) = &calls[0];
    // Compare resolved paths: the worker canonicalizes, and on macOS the
    // temporary directory reaches the same files through a symlinked prefix.
    let expect = |relative: &str| {
        harness
            .media_root
            .join(relative)
            .canonicalize()
            .expect("the finished file exists")
    };
    assert_eq!(
        video.canonicalize().unwrap(),
        expect(&completed.final_video_path)
    );
    assert_eq!(
        subtitle.canonicalize().unwrap(),
        expect(completed.final_subtitle_path.as_ref().unwrap())
    );
    assert!(subtitle.exists());
}

#[tokio::test]
async fn a_download_without_a_subtitle_is_never_sent_for_alignment() {
    let harness = WorkerHarness::new();
    let server = FixtureServer::start(8 * 1024).await.unwrap();
    let job = build_job(&harness.media_root, "no-sync", JobState::Queued);
    let store = Arc::new(MockStore::with_jobs([job.clone()]));
    let catalog = Arc::new(MockCatalog::new([Ok(ResolvedSource {
        url: server.url("/download"),
        headers: FixtureServer::required_headers(),
        subtitle: None,
        extension: "mkv".to_string(),
        expected_size: Some(server.content_len() as u64),
        catalog_size_bytes: None,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
    })]));
    let syncer = Arc::new(RecordingSyncer::default());
    let worker = JobWorker::new(
        store.clone(),
        catalog,
        Arc::new(moviebox_tui::server::jobs::HttpTransferClient::new(
            server.client(),
        )),
        harness.namer.clone(),
        JobEventBus::new(16),
    )
    .with_disk_space_checker(Arc::new(MockDiskSpace::new(u64::MAX)))
    .with_subtitle_syncer(syncer.clone());

    worker.run_once().await.unwrap();

    assert_eq!(store.job(job.id).await.state, JobState::Ready);
    assert!(syncer.calls().await.is_empty());
}
