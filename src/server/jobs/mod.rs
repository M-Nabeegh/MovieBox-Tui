mod model;
mod repository;

pub use model::{
    DownloadJob, JobEvent, JobEventKind, JobId, JobProgress, JobRepositoryError, JobState, NewJob,
    can_transition,
};
pub use repository::JobRepository;
