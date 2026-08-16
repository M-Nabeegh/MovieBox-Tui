use std::{
    fmt,
    path::{Component, Path},
};

use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::catalog::{CatalogId, MediaType, SourceId, SubtitleId};

const MAX_REQUESTED_HEIGHT: u16 = 1080;
const MAX_ERROR_CODE_LEN: usize = 64;
const MAX_ERROR_MESSAGE_LEN: usize = 512;
const MAX_WARNING_LEN: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId(pub Uuid);

impl JobId {
    pub fn new(value: Uuid) -> Self {
        Self(value)
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Typed keyset cursor for browser-safe job listing pagination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobListCursor {
    created_at: OffsetDateTime,
    id: JobId,
}

impl JobListCursor {
    pub fn new(created_at: OffsetDateTime, id: JobId) -> Self {
        Self { created_at, id }
    }

    pub fn from_job(job: &DownloadJob) -> Self {
        Self::new(job.created_at, job.id)
    }

    pub fn created_at(self) -> OffsetDateTime {
        self.created_at
    }

    pub fn id(self) -> JobId {
        self.id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum JobState {
    Queued = 0,
    Resolving = 1,
    Downloading = 2,
    Paused = 3,
    Finalizing = 4,
    Ready = 5,
    Failed = 6,
    Cancelled = 7,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Resolving => "resolving",
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Finalizing => "finalizing",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Result<Self, JobRepositoryError> {
        match value {
            "queued" => Ok(Self::Queued),
            "resolving" => Ok(Self::Resolving),
            "downloading" => Ok(Self::Downloading),
            "paused" => Ok(Self::Paused),
            "finalizing" => Ok(Self::Finalizing),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(JobRepositoryError::InvalidData(format!(
                "unknown job state: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadJob {
    pub id: JobId,
    pub catalog_id: CatalogId,
    pub source_id: SourceId,
    pub subtitle_id: Option<SubtitleId>,
    pub title: String,
    pub year: Option<String>,
    pub media_type: MediaType,
    pub season_number: Option<u16>,
    pub episode_number: Option<u16>,
    pub episode_title: Option<String>,
    pub requested_height: u16,
    pub state: JobState,
    pub final_video_path: String,
    pub final_subtitle_path: Option<String>,
    pub partial_video_path: String,
    pub partial_subtitle_path: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_per_second: Option<u64>,
    pub attempt: u32,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub warning: Option<String>,
    /// When set, the job is queued but must not be claimed before this instant.
    /// Used to back off between automatic retries of recoverable failures.
    pub next_attempt_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    pub catalog_id: CatalogId,
    pub source_id: SourceId,
    pub subtitle_id: Option<SubtitleId>,
    pub title: String,
    pub year: Option<String>,
    pub media_type: MediaType,
    pub season_number: Option<u16>,
    pub episode_number: Option<u16>,
    pub episode_title: Option<String>,
    pub requested_height: u16,
    pub final_video_path: String,
    pub final_subtitle_path: Option<String>,
    pub partial_video_path: String,
    pub partial_subtitle_path: Option<String>,
}

impl NewJob {
    pub fn validate(&self) -> Result<(), JobRepositoryError> {
        ensure_opaque_id("catalog_id", self.catalog_id.as_str())?;
        ensure_opaque_id("source_id", self.source_id.as_str())?;
        if let Some(subtitle_id) = &self.subtitle_id {
            ensure_opaque_id("subtitle_id", subtitle_id.as_str())?;
        }

        if self.title.trim().is_empty() {
            return Err(JobRepositoryError::InvalidData(
                "title must not be empty".to_string(),
            ));
        }
        if self.requested_height == 0 || self.requested_height > MAX_REQUESTED_HEIGHT {
            return Err(JobRepositoryError::InvalidData(format!(
                "requested height must be between 1 and {MAX_REQUESTED_HEIGHT}"
            )));
        }

        match self.media_type {
            MediaType::Movie => {
                if self.season_number.is_some()
                    || self.episode_number.is_some()
                    || self.episode_title.is_some()
                {
                    return Err(JobRepositoryError::InvalidData(
                        "movie jobs must not include episode identity".to_string(),
                    ));
                }
            }
            MediaType::Series => {
                if self.season_number.is_none() || self.episode_number.is_none() {
                    return Err(JobRepositoryError::InvalidData(
                        "series jobs require season and episode numbers".to_string(),
                    ));
                }
            }
        }

        ensure_relative_path("final_video_path", &self.final_video_path)?;
        ensure_optional_relative_path("final_subtitle_path", &self.final_subtitle_path)?;
        ensure_relative_path("partial_video_path", &self.partial_video_path)?;
        ensure_optional_relative_path("partial_subtitle_path", &self.partial_subtitle_path)?;

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobProgress {
    pub expected_version: i64,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_per_second: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobEvent {
    pub kind: JobEventKind,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobEventFields {
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub warning: Option<String>,
}

impl JobEvent {
    pub fn new(kind: JobEventKind) -> Self {
        Self {
            kind,
            error_code: None,
            error_message: None,
            warning: None,
        }
    }

    pub fn with_error(
        mut self,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<Self, JobRepositoryError> {
        self.error_code = Some(
            sanitize_text(code.into(), MAX_ERROR_CODE_LEN)
                .map_err(JobRepositoryError::InvalidData)?,
        );
        self.error_message = Some(
            sanitize_text(message.into(), MAX_ERROR_MESSAGE_LEN)
                .map_err(JobRepositoryError::InvalidData)?,
        );
        Ok(self)
    }

    pub fn with_warning(mut self, warning: impl Into<String>) -> Result<Self, JobRepositoryError> {
        self.warning = Some(
            sanitize_text(warning.into(), MAX_WARNING_LEN)
                .map_err(JobRepositoryError::InvalidData)?,
        );
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobEventKind {
    Created,
    Claimed,
    StateChanged,
    ProgressUpdated,
}

impl JobEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Claimed => "claimed",
            Self::StateChanged => "state_changed",
            Self::ProgressUpdated => "progress_updated",
        }
    }
}

#[derive(Debug, Error)]
pub enum JobRepositoryError {
    #[error("job was not found: {0}")]
    NotFound(JobId),
    #[error("illegal job state transition from {from:?} to {to:?}")]
    InvalidTransition { from: JobState, to: JobState },
    #[error("job progress update is not allowed while state is {0:?}")]
    InvalidProgressState(JobState),
    #[error("job version conflict: expected {expected_version}, found {actual_version}")]
    Conflict {
        expected_version: i64,
        actual_version: i64,
    },
    #[error("invalid job data: {0}")]
    InvalidData(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

pub fn can_transition(from: JobState, to: JobState) -> bool {
    matches!(
        (from, to),
        (JobState::Queued, JobState::Resolving)
            | (JobState::Queued, JobState::Cancelled)
            | (JobState::Resolving, JobState::Downloading)
            | (JobState::Resolving, JobState::Failed)
            | (JobState::Downloading, JobState::Paused)
            | (JobState::Downloading, JobState::Finalizing)
            | (JobState::Downloading, JobState::Failed)
            | (JobState::Downloading, JobState::Cancelled)
            | (JobState::Paused, JobState::Queued)
            | (JobState::Paused, JobState::Cancelled)
            | (JobState::Finalizing, JobState::Ready)
            | (JobState::Finalizing, JobState::Failed)
            | (JobState::Failed, JobState::Queued)
    )
}

pub(crate) fn apply_event_fields(
    state: JobState,
    existing_warning: Option<&str>,
    event: Option<&JobEvent>,
) -> Result<JobEventFields, JobRepositoryError> {
    let error_code = if state == JobState::Failed {
        event
            .and_then(|item| item.error_code.as_deref())
            .map(|value| sanitize_text(value.to_string(), MAX_ERROR_CODE_LEN))
            .transpose()
            .map_err(JobRepositoryError::InvalidData)?
    } else {
        None
    };
    let error_message = if state == JobState::Failed {
        event
            .and_then(|item| item.error_message.as_deref())
            .map(|value| sanitize_text(value.to_string(), MAX_ERROR_MESSAGE_LEN))
            .transpose()
            .map_err(JobRepositoryError::InvalidData)?
    } else {
        None
    };
    if state == JobState::Failed && (error_code.is_some() != error_message.is_some()) {
        return Err(JobRepositoryError::InvalidData(
            "failed transitions require both error_code and error_message".to_string(),
        ));
    }
    let warning = match event.and_then(|item| item.warning.as_deref()) {
        Some(value) => Some(
            sanitize_text(value.to_string(), MAX_WARNING_LEN)
                .map_err(JobRepositoryError::InvalidData)?,
        ),
        None => existing_warning.map(str::to_string),
    };
    Ok(JobEventFields {
        error_code,
        error_message,
        warning,
    })
}

fn ensure_opaque_id(field: &'static str, value: &str) -> Result<(), JobRepositoryError> {
    if value.trim().is_empty() {
        return Err(JobRepositoryError::InvalidData(format!(
            "{field} must not be empty"
        )));
    }
    if value.contains("://") {
        return Err(JobRepositoryError::InvalidData(format!(
            "{field} must not contain a direct URL"
        )));
    }
    Ok(())
}

fn ensure_optional_relative_path(
    field: &'static str,
    value: &Option<String>,
) -> Result<(), JobRepositoryError> {
    if let Some(value) = value {
        ensure_relative_path(field, value)?;
    }
    Ok(())
}

fn ensure_relative_path(field: &'static str, value: &str) -> Result<(), JobRepositoryError> {
    if value.trim().is_empty() {
        return Err(JobRepositoryError::InvalidData(format!(
            "{field} must not be empty"
        )));
    }

    let path = Path::new(value);
    if path.is_absolute() {
        return Err(JobRepositoryError::InvalidData(format!(
            "{field} must be relative"
        )));
    }

    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(JobRepositoryError::InvalidData(format!(
                "{field} must contain only normal path components"
            )));
        }
    }

    Ok(())
}

fn sanitize_text(value: String, max_len: usize) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("text must not be empty".to_string());
    }
    if trimmed.len() > max_len {
        return Err(format!("text exceeds {max_len} bytes"));
    }
    if trimmed.chars().any(|ch| ch.is_control()) {
        return Err("text must not contain control characters".to_string());
    }
    Ok(trimmed.to_string())
}
