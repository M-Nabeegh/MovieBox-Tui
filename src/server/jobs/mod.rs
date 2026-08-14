mod model;
mod recovery;
mod repository;
mod worker;

pub use model::{
    DownloadJob, JobEvent, JobEventKind, JobId, JobListCursor, JobProgress, JobRepositoryError,
    JobState, NewJob, can_transition,
};
pub use recovery::recover_interrupted_jobs;
pub use repository::JobRepository;
pub use worker::{
    DiskSpaceChecker, HttpTransferClient, JobStatePatch, JobStore, JobWorker, TransferClient,
    TransferError, TransferProgress,
};
