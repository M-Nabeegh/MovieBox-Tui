use std::{
    collections::HashMap,
    ffi::CString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use reqwest::StatusCode;
use thiserror::Error;
use tokio::{
    fs,
    sync::{Mutex, mpsc},
};
use tokio_util::sync::CancellationToken;

use super::{
    DownloadJob, JobEvent, JobEventKind, JobId, JobProgress, JobRepository, JobRepositoryError,
    JobState,
    recovery::{clear_job_errors, fail_job},
};
use crate::{
    catalog::{CatalogError, CatalogProvider, QualityPolicy, ResolvedSource, SubtitleId},
    download::{DownloadClient, DownloadError, DownloadOutcome, DownloadRequest, download},
    server::{events::JobEventBus, library::LibraryNamer, security::path::contained_path},
};

const DEFAULT_RESERVE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
const IDLE_WAIT: Duration = Duration::from_millis(250);
const WARNING_THROTTLE: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Default)]
pub struct JobStatePatch {
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<Option<u64>>,
    pub speed_bytes_per_second: Option<Option<u64>>,
    pub error_code: Option<Option<String>>,
    pub error_message: Option<Option<String>>,
    pub warning: Option<Option<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_per_second: Option<u64>,
}

#[derive(Debug, Error)]
pub enum TransferError {
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[async_trait]
pub trait TransferClient: Send + Sync {
    async fn transfer(
        &self,
        request: DownloadRequest,
        destination: &Path,
        cancel: CancellationToken,
        progress: mpsc::UnboundedSender<TransferProgress>,
    ) -> Result<DownloadOutcome, TransferError>;
}

#[derive(Clone)]
pub struct HttpTransferClient {
    client: DownloadClient,
}

impl HttpTransferClient {
    pub fn new(client: DownloadClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl TransferClient for HttpTransferClient {
    async fn transfer(
        &self,
        request: DownloadRequest,
        destination: &Path,
        cancel: CancellationToken,
        progress: mpsc::UnboundedSender<TransferProgress>,
    ) -> Result<DownloadOutcome, TransferError> {
        let flag = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::clone(&flag);
        let watcher = tokio::spawn(async move {
            cancel.cancelled().await;
            cancel_flag.store(true, Ordering::Relaxed);
        });

        let result = download(&self.client, request, destination, flag, |update| {
            let _ = progress.send(TransferProgress {
                downloaded_bytes: update.downloaded,
                total_bytes: update.total,
                speed_bytes_per_second: Some(update.bytes_per_second.max(0.0) as u64),
            });
        })
        .await
        .map_err(TransferError::from);

        watcher.abort();
        result
    }
}

#[async_trait]
pub trait DiskSpaceChecker: Send + Sync {
    async fn available_bytes(&self, path: &Path) -> Result<u64, std::io::Error>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDiskSpaceChecker;

#[async_trait]
impl DiskSpaceChecker for SystemDiskSpaceChecker {
    async fn available_bytes(&self, path: &Path) -> Result<u64, std::io::Error> {
        let candidate = if path.exists() {
            path.to_path_buf()
        } else {
            path.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| path.to_path_buf())
        };

        tokio::task::spawn_blocking(move || available_bytes_blocking(&candidate))
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?
    }
}

#[cfg(unix)]
fn available_bytes_blocking(path: &Path) -> Result<u64, std::io::Error> {
    use std::os::unix::ffi::OsStrExt;

    let bytes = path.as_os_str().as_bytes();
    let c_path = CString::new(bytes)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path"))?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let result = unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stats = unsafe { stats.assume_init() };
    Ok((stats.f_bavail as u64).saturating_mul(stats.f_frsize))
}

#[cfg(not(unix))]
fn available_bytes_blocking(_path: &Path) -> Result<u64, std::io::Error> {
    Ok(u64::MAX)
}

#[async_trait]
pub trait JobStore: Send + Sync {
    async fn claim_next(&self) -> Result<Option<DownloadJob>, JobRepositoryError>;
    async fn get(&self, id: JobId) -> Result<DownloadJob, JobRepositoryError>;
    async fn list_all(&self) -> Result<Vec<DownloadJob>, JobRepositoryError>;
    async fn transition(
        &self,
        id: JobId,
        expected_version: i64,
        to: JobState,
        event: Option<JobEvent>,
    ) -> Result<DownloadJob, JobRepositoryError>;
    async fn update_progress(
        &self,
        id: JobId,
        progress: JobProgress,
        event: Option<JobEvent>,
    ) -> Result<DownloadJob, JobRepositoryError>;
    async fn force_state(
        &self,
        id: JobId,
        expected_version: i64,
        to: JobState,
        kind: JobEventKind,
        patch: JobStatePatch,
    ) -> Result<DownloadJob, JobRepositoryError>;
}

#[async_trait]
impl JobStore for JobRepository {
    async fn claim_next(&self) -> Result<Option<DownloadJob>, JobRepositoryError> {
        JobRepository::claim_next(self).await
    }

    async fn get(&self, id: JobId) -> Result<DownloadJob, JobRepositoryError> {
        JobRepository::get(self, id).await
    }

    async fn list_all(&self) -> Result<Vec<DownloadJob>, JobRepositoryError> {
        JobRepository::list_all(self).await
    }

    async fn transition(
        &self,
        id: JobId,
        expected_version: i64,
        to: JobState,
        event: Option<JobEvent>,
    ) -> Result<DownloadJob, JobRepositoryError> {
        JobRepository::transition(self, id, expected_version, to, event).await
    }

    async fn update_progress(
        &self,
        id: JobId,
        progress: JobProgress,
        event: Option<JobEvent>,
    ) -> Result<DownloadJob, JobRepositoryError> {
        JobRepository::update_progress(self, id, progress, event).await
    }

    async fn force_state(
        &self,
        id: JobId,
        expected_version: i64,
        to: JobState,
        kind: JobEventKind,
        patch: JobStatePatch,
    ) -> Result<DownloadJob, JobRepositoryError> {
        JobRepository::force_state(self, id, expected_version, to, kind, patch).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveCommand {
    Pause,
    Cancel,
}

#[derive(Debug, Clone)]
struct ActiveJob {
    token: CancellationToken,
    command: ActiveCommand,
}

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error(transparent)]
    Repository(#[from] JobRepositoryError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct JobWorker<S, C, T, D = SystemDiskSpaceChecker> {
    store: Arc<S>,
    catalog: Arc<C>,
    transfer: Arc<T>,
    namer: LibraryNamer,
    bus: JobEventBus,
    disk: Arc<D>,
    reserve_bytes: u64,
    active: Arc<Mutex<HashMap<JobId, ActiveJob>>>,
    throttled_warnings: Arc<Mutex<HashMap<JobId, Instant>>>,
}

impl<S, C, T> JobWorker<S, C, T, SystemDiskSpaceChecker>
where
    S: JobStore + 'static,
    C: CatalogProvider + 'static,
    T: TransferClient + 'static,
{
    pub fn new(
        store: Arc<S>,
        catalog: Arc<C>,
        transfer: Arc<T>,
        namer: LibraryNamer,
        bus: JobEventBus,
    ) -> Self {
        Self {
            store,
            catalog,
            transfer,
            namer,
            bus,
            disk: Arc::new(SystemDiskSpaceChecker),
            reserve_bytes: DEFAULT_RESERVE_BYTES,
            active: Arc::new(Mutex::new(HashMap::new())),
            throttled_warnings: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl<S, C, T, D> JobWorker<S, C, T, D>
where
    S: JobStore + 'static,
    C: CatalogProvider + 'static,
    T: TransferClient + 'static,
    D: DiskSpaceChecker + 'static,
{
    pub fn with_disk_space_checker<D2>(self, disk: Arc<D2>) -> JobWorker<S, C, T, D2>
    where
        D2: DiskSpaceChecker + 'static,
    {
        JobWorker {
            store: self.store,
            catalog: self.catalog,
            transfer: self.transfer,
            namer: self.namer,
            bus: self.bus,
            disk,
            reserve_bytes: self.reserve_bytes,
            active: self.active,
            throttled_warnings: self.throttled_warnings,
        }
    }

    pub async fn run(&self, cancel: CancellationToken) -> Result<(), WorkerError> {
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }

            if self.run_once().await? {
                continue;
            }

            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                _ = tokio::time::sleep(IDLE_WAIT) => {}
            }
        }
    }

    pub async fn run_once(&self) -> Result<bool, WorkerError> {
        let Some(job) = self.store.claim_next().await? else {
            return Ok(false);
        };

        self.publish(&job, JobEventKind::Claimed);

        self.process_claimed_job(job).await?;

        Ok(true)
    }

    pub async fn pause(&self, id: JobId) -> Result<(), JobRepositoryError> {
        let mut active = self.active.lock().await;
        if let Some(job) = active.get_mut(&id) {
            job.command = ActiveCommand::Pause;
            job.token.cancel();
        }
        Ok(())
    }

    pub async fn cancel(&self, id: JobId) -> Result<(), JobRepositoryError> {
        let mut active = self.active.lock().await;
        if let Some(job) = active.get_mut(&id) {
            job.command = ActiveCommand::Cancel;
            job.token.cancel();
        } else {
            let current = self.store.get(id).await?;
            match current.state {
                JobState::Queued | JobState::Paused => {
                    let next = self
                        .store
                        .transition(
                            id,
                            current.version,
                            JobState::Cancelled,
                            Some(JobEvent::new(JobEventKind::StateChanged)),
                        )
                        .await?;
                    self.remove_job_files(&next)
                        .await
                        .map_err(|error| JobRepositoryError::InvalidData(error.to_string()))?;
                    self.publish(&next, JobEventKind::StateChanged);
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub async fn retry(&self, id: JobId) -> Result<DownloadJob, JobRepositoryError> {
        let current = self.store.get(id).await?;
        let retried = self
            .store
            .force_state(
                id,
                current.version,
                JobState::Queued,
                JobEventKind::StateChanged,
                clear_job_errors(),
            )
            .await?;
        self.publish(&retried, JobEventKind::StateChanged);
        Ok(retried)
    }

    async fn process_claimed_job(&self, job: DownloadJob) -> Result<(), WorkerError> {
        if !self.paths_are_safe(&job) {
            fail_job(
                self.store.as_ref(),
                job.id,
                job.version,
                "unsafe_path",
                "job paths failed validation",
            )
            .await?;
            let failed = self.store.get(job.id).await?;
            self.publish(&failed, JobEventKind::StateChanged);
            return Ok(());
        }

        let resolve = self
            .catalog
            .resolve(&job.source_id, job.subtitle_id.as_ref())
            .await;
        let resolved = match resolve {
            Ok(resolved) => resolved,
            Err(CatalogError::QualityUnavailable { .. }) => {
                fail_job(
                    self.store.as_ref(),
                    job.id,
                    job.version,
                    "quality_unavailable",
                    "no source is available at or below 1080p",
                )
                .await?;
                self.publish(&self.store.get(job.id).await?, JobEventKind::StateChanged);
                return Ok(());
            }
            Err(_) => {
                fail_job(
                    self.store.as_ref(),
                    job.id,
                    job.version,
                    "source_resolve_failed",
                    "source resolution failed",
                )
                .await?;
                self.publish(&self.store.get(job.id).await?, JobEventKind::StateChanged);
                return Ok(());
            }
        };

        QualityPolicy::new(1080)
            .validate(job.requested_height)
            .map_err(|_| {
                JobRepositoryError::InvalidData("requested height exceeds 1080p".into())
            })?;

        if self.final_target_exists(&job).await? {
            fail_job(
                self.store.as_ref(),
                job.id,
                job.version,
                "duplicate_target",
                "final media target already exists",
            )
            .await?;
            self.publish(&self.store.get(job.id).await?, JobEventKind::StateChanged);
            return Ok(());
        }

        let available = self.disk.available_bytes(self.namer.media_root()).await?;
        let expected_size = resolved.expected_size.unwrap_or(0);
        if available <= self.reserve_bytes.saturating_add(expected_size) {
            let requeued = self
                .store
                .force_state(
                    job.id,
                    job.version,
                    JobState::Queued,
                    JobEventKind::StateChanged,
                    JobStatePatch {
                        error_code: Some(Some("insufficient_space".to_string())),
                        error_message: Some(Some("insufficient free space".to_string())),
                        ..Default::default()
                    },
                )
                .await?;
            if self.should_emit_warning(job.id).await {
                self.publish(&requeued, JobEventKind::StateChanged);
            }
            return Ok(());
        }

        let downloading = self
            .store
            .transition(
                job.id,
                job.version,
                JobState::Downloading,
                Some(JobEvent::new(JobEventKind::StateChanged)),
            )
            .await?;
        self.publish(&downloading, JobEventKind::StateChanged);

        let download = self.download_with_refresh(downloading, resolved).await?;
        let completed = match download {
            Some(job) => job,
            None => return Ok(()),
        };

        let mut finalized = self
            .store
            .force_state(
                completed.id,
                completed.version,
                JobState::Finalizing,
                JobEventKind::StateChanged,
                JobStatePatch {
                    downloaded_bytes: Some(
                        completed.total_bytes.unwrap_or(completed.downloaded_bytes),
                    ),
                    total_bytes: Some(completed.total_bytes),
                    speed_bytes_per_second: Some(None),
                    warning: Some(completed.warning.clone()),
                    ..Default::default()
                },
            )
            .await?;
        self.publish(&finalized, JobEventKind::StateChanged);

        self.finalize_video(&finalized).await?;
        let subtitle_warning = self
            .finalize_subtitle_if_needed(&finalized, job.subtitle_id.as_ref())
            .await?;
        if let Some(warning) = subtitle_warning {
            finalized = self
                .store
                .force_state(
                    finalized.id,
                    finalized.version,
                    JobState::Finalizing,
                    JobEventKind::StateChanged,
                    JobStatePatch {
                        warning: Some(Some(warning)),
                        ..Default::default()
                    },
                )
                .await?;
            self.publish(&finalized, JobEventKind::StateChanged);
        }

        let ready = self
            .store
            .transition(
                finalized.id,
                finalized.version,
                JobState::Ready,
                Some(JobEvent::new(JobEventKind::StateChanged)),
            )
            .await?;
        self.publish(&ready, JobEventKind::StateChanged);
        Ok(())
    }

    async fn download_with_refresh(
        &self,
        downloading: DownloadJob,
        mut resolved: ResolvedSource,
    ) -> Result<Option<DownloadJob>, WorkerError> {
        let video_destination =
            active_transfer_path(&downloading.partial_video_path).ok_or_else(|| {
                JobRepositoryError::InvalidData("partial video path must end with .part".into())
            })?;
        let absolute_destination = self.contained_path(&video_destination)?;
        if let Some(parent) = absolute_destination.parent() {
            fs::create_dir_all(parent).await?;
        }

        let mut attempts = 0_u8;
        loop {
            attempts += 1;
            let token = CancellationToken::new();
            self.active.lock().await.insert(
                downloading.id,
                ActiveJob {
                    token: token.clone(),
                    command: ActiveCommand::Pause,
                },
            );

            let request = build_request(&resolved);
            let outcome = self
                .run_transfer(downloading.clone(), request, &absolute_destination, token)
                .await;
            self.active.lock().await.remove(&downloading.id);

            match outcome {
                Ok(TransferOutcome::Completed(job)) => return Ok(Some(job)),
                Ok(TransferOutcome::Paused(job, ActiveCommand::Pause)) => {
                    let paused = self
                        .store
                        .transition(
                            job.id,
                            job.version,
                            JobState::Paused,
                            Some(JobEvent::new(JobEventKind::StateChanged)),
                        )
                        .await?;
                    self.publish(&paused, JobEventKind::StateChanged);
                    return Ok(None);
                }
                Ok(TransferOutcome::Paused(job, ActiveCommand::Cancel)) => {
                    self.remove_job_files(&job).await?;
                    let cancelled = self
                        .store
                        .transition(
                            job.id,
                            job.version,
                            JobState::Cancelled,
                            Some(JobEvent::new(JobEventKind::StateChanged)),
                        )
                        .await?;
                    self.publish(&cancelled, JobEventKind::StateChanged);
                    return Ok(None);
                }
                Err(TransferError::Download(DownloadError::Http(status)))
                    if attempts == 1 && is_expired_status(status) =>
                {
                    resolved = match self
                        .catalog
                        .resolve(&downloading.source_id, downloading.subtitle_id.as_ref())
                        .await
                    {
                        Ok(value) => value,
                        Err(_) => {
                            fail_job(
                                self.store.as_ref(),
                                downloading.id,
                                downloading.version,
                                "download_failed",
                                "download request failed",
                            )
                            .await?;
                            self.publish(
                                &self.store.get(downloading.id).await?,
                                JobEventKind::StateChanged,
                            );
                            return Ok(None);
                        }
                    };
                }
                Err(error) => {
                    let _ = error;
                    fail_job(
                        self.store.as_ref(),
                        downloading.id,
                        downloading.version,
                        "download_failed",
                        "download request failed",
                    )
                    .await?;
                    self.publish(
                        &self.store.get(downloading.id).await?,
                        JobEventKind::StateChanged,
                    );
                    return Ok(None);
                }
            }
        }
    }

    async fn run_transfer(
        &self,
        starting_job: DownloadJob,
        request: DownloadRequest,
        destination: &Path,
        token: CancellationToken,
    ) -> Result<TransferOutcome, TransferError> {
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let mut current_job = starting_job;
        let transfer = self
            .transfer
            .transfer(request, destination, token.clone(), progress_tx);
        tokio::pin!(transfer);

        loop {
            tokio::select! {
                maybe_progress = progress_rx.recv() => {
                    let Some(progress) = maybe_progress else {
                        continue;
                    };
                    current_job = self
                        .store
                        .update_progress(
                            current_job.id,
                            JobProgress {
                                expected_version: current_job.version,
                                downloaded_bytes: progress.downloaded_bytes,
                                total_bytes: progress.total_bytes,
                                speed_bytes_per_second: progress.speed_bytes_per_second,
                            },
                            None,
                        )
                        .await
                        .map_err(map_repo_as_io)?;
                    self.publish(&current_job, JobEventKind::ProgressUpdated);
                }
                outcome = &mut transfer => {
                    while let Ok(progress) = progress_rx.try_recv() {
                        current_job = self
                            .store
                            .update_progress(
                                current_job.id,
                                JobProgress {
                                    expected_version: current_job.version,
                                    downloaded_bytes: progress.downloaded_bytes,
                                    total_bytes: progress.total_bytes,
                                    speed_bytes_per_second: progress.speed_bytes_per_second,
                                },
                                None,
                            )
                            .await
                            .map_err(map_repo_as_io)?;
                        self.publish(&current_job, JobEventKind::ProgressUpdated);
                    }
                    return match outcome? {
                        DownloadOutcome::Completed { bytes } => {
                            let completed = self
                                .store
                                .force_state(
                                    current_job.id,
                                    current_job.version,
                                    JobState::Downloading,
                                    JobEventKind::ProgressUpdated,
                                    JobStatePatch {
                                        downloaded_bytes: Some(bytes),
                                        total_bytes: Some(Some(bytes)),
                                        speed_bytes_per_second: Some(None),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(map_repo_as_io)?;
                            Ok(TransferOutcome::Completed(completed))
                        }
                        DownloadOutcome::Paused { bytes } => {
                            let paused = self
                                .store
                                .force_state(
                                    current_job.id,
                                    current_job.version,
                                    JobState::Downloading,
                                    JobEventKind::ProgressUpdated,
                                    JobStatePatch {
                                        downloaded_bytes: Some(bytes),
                                        speed_bytes_per_second: Some(None),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(map_repo_as_io)?;
                            let command = self
                                .active
                                .lock()
                                .await
                                .get(&paused.id)
                                .map(|job| job.command)
                                .unwrap_or(ActiveCommand::Pause);
                            Ok(TransferOutcome::Paused(paused, command))
                        }
                    };
                }
            }
        }
    }

    async fn finalize_video(&self, job: &DownloadJob) -> Result<(), WorkerError> {
        let source =
            self.contained_path(&active_transfer_path(&job.partial_video_path).ok_or_else(
                || JobRepositoryError::InvalidData("partial video path must end with .part".into()),
            )?)?;
        let target = self.contained_path(Path::new(&job.final_video_path))?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::rename(source, target).await?;
        Ok(())
    }

    async fn finalize_subtitle_if_needed(
        &self,
        job: &DownloadJob,
        subtitle_id: Option<&SubtitleId>,
    ) -> Result<Option<String>, WorkerError> {
        let Some(subtitle_id) = subtitle_id else {
            return Ok(None);
        };
        let Some(partial) = &job.partial_subtitle_path else {
            return Ok(None);
        };
        let Some(final_subtitle) = &job.final_subtitle_path else {
            return Ok(None);
        };

        let resolved = match self
            .catalog
            .resolve(&job.source_id, Some(subtitle_id))
            .await
        {
            Ok(resolved) => resolved,
            Err(_) => return Ok(Some("subtitle_download_failed".to_string())),
        };
        let Some(subtitle) = resolved.subtitle else {
            return Ok(Some("subtitle_download_failed".to_string()));
        };

        let destination = active_transfer_path(partial).ok_or_else(|| {
            JobRepositoryError::InvalidData("partial subtitle path must end with .part".into())
        })?;
        let absolute_destination = self.contained_path(&destination)?;
        if let Some(parent) = absolute_destination.parent() {
            fs::create_dir_all(parent).await?;
        }

        let request = DownloadRequest {
            url: subtitle.url,
            headers: subtitle.headers,
            maximum_redirects: 5,
        };
        let token = CancellationToken::new();
        let (progress_tx, _progress_rx) = mpsc::unbounded_channel();
        if self
            .transfer
            .transfer(request, &absolute_destination, token, progress_tx)
            .await
            .is_err()
        {
            return Ok(Some("subtitle_download_failed".to_string()));
        }

        let final_path = self.contained_path(Path::new(final_subtitle))?;
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::rename(absolute_destination, final_path).await?;
        Ok(None)
    }

    async fn final_target_exists(&self, job: &DownloadJob) -> Result<bool, WorkerError> {
        let video = self.contained_path(Path::new(&job.final_video_path))?;
        if video.exists() {
            return Ok(true);
        }
        if let Some(path) = &job.final_subtitle_path {
            let subtitle = self.contained_path(Path::new(path))?;
            if subtitle.exists() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn paths_are_safe(&self, job: &DownloadJob) -> bool {
        self.contained_path(Path::new(&job.final_video_path))
            .is_ok()
            && self
                .contained_path(Path::new(&job.partial_video_path))
                .is_ok()
            && job
                .final_subtitle_path
                .as_ref()
                .map(|path| self.contained_path(Path::new(path)).is_ok())
                .unwrap_or(true)
            && job
                .partial_subtitle_path
                .as_ref()
                .map(|path| self.contained_path(Path::new(path)).is_ok())
                .unwrap_or(true)
    }

    fn contained_path(&self, relative: &Path) -> Result<PathBuf, WorkerError> {
        contained_path(self.namer.media_root(), relative)
            .map_err(|error| WorkerError::Io(std::io::Error::other(error.to_string())))
    }

    async fn remove_job_files(&self, job: &DownloadJob) -> Result<(), WorkerError> {
        self.remove_transfer_artifacts(&job.partial_video_path)
            .await?;
        if let Some(path) = &job.partial_subtitle_path {
            self.remove_transfer_artifacts(path).await?;
        }
        Ok(())
    }

    async fn remove_transfer_artifacts(&self, partial_relative: &str) -> Result<(), WorkerError> {
        let partial = self.contained_path(Path::new(partial_relative))?;
        remove_if_exists(&partial).await?;
        remove_if_exists(Path::new(&format!("{}.json", partial.display()))).await?;

        if let Some(active) = active_transfer_path(partial_relative) {
            let active = self.contained_path(&active)?;
            remove_if_exists(&active).await?;
        }
        Ok(())
    }

    async fn should_emit_warning(&self, job_id: JobId) -> bool {
        let mut throttled = self.throttled_warnings.lock().await;
        let now = Instant::now();
        match throttled.get(&job_id) {
            Some(last) if now.duration_since(*last) < WARNING_THROTTLE => false,
            _ => {
                throttled.insert(job_id, now);
                true
            }
        }
    }

    fn publish(&self, job: &DownloadJob, kind: JobEventKind) {
        self.bus.publish_job(job, kind);
    }
}

enum TransferOutcome {
    Completed(DownloadJob),
    Paused(DownloadJob, ActiveCommand),
}

fn build_request(resolved: &ResolvedSource) -> DownloadRequest {
    DownloadRequest {
        url: resolved.url.clone(),
        headers: resolved.headers.clone(),
        maximum_redirects: 5,
    }
}

fn active_transfer_path(partial_relative: &str) -> Option<PathBuf> {
    partial_relative.strip_suffix(".part").map(PathBuf::from)
}

fn is_expired_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN | StatusCode::GONE
    )
}

fn map_repo_as_io(error: JobRepositoryError) -> TransferError {
    TransferError::Io(std::io::Error::other(error.to_string()))
}

async fn remove_if_exists(path: &Path) -> Result<(), WorkerError> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(WorkerError::Io(error)),
    }
}
