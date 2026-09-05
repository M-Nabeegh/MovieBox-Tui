pub mod dash;
mod model;
mod recovery;
mod repository;
mod retry;
mod worker;

pub use model::{
    DownloadJob, JobEvent, JobEventKind, JobId, JobListCursor, JobProgress, JobRepositoryError,
    JobState, NewJob, can_transition,
};
pub use recovery::{recover_interrupted_jobs, sweep_orphaned_partials};
pub use repository::JobRepository;
pub use retry::{
    FailureClass, MAX_AUTOMATIC_ATTEMPTS, backoff_delay, classify_catalog_error, may_retry,
};
pub use worker::{
    DiskSpaceChecker, HttpTransferClient, JobSignal, JobStatePatch, JobStore, JobWorker,
    LibraryRefresher, NoopLibraryRefresher, TransferClient, TransferError, TransferProgress,
    WorkerRunOutcome,
};
