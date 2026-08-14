use std::path::{Component, Path, PathBuf};

use super::{
    DownloadJob, JobEventKind, JobId, JobRepositoryError, JobState, JobStatePatch, JobStore,
};
use crate::server::security::path::contained_path;

pub async fn recover_interrupted_jobs<S>(
    store: &S,
    media_root: &Path,
) -> Result<(), JobRepositoryError>
where
    S: JobStore + ?Sized,
{
    for job in store.list_all().await? {
        if !matches!(
            job.state,
            JobState::Resolving | JobState::Downloading | JobState::Finalizing
        ) {
            continue;
        }

        if validate_paths(&job, media_root).is_err() {
            fail_unsafe_path(store, &job).await?;
            continue;
        }

        if job.state == JobState::Finalizing && final_video_is_complete(&job, media_root) {
            store
                .force_state(
                    job.id,
                    job.version,
                    JobState::Ready,
                    JobEventKind::StateChanged,
                    JobStatePatch::default(),
                )
                .await?;
            continue;
        }

        store
            .force_state(
                job.id,
                job.version,
                JobState::Queued,
                JobEventKind::StateChanged,
                JobStatePatch {
                    error_code: Some(None),
                    error_message: Some(None),
                    speed_bytes_per_second: Some(None),
                    ..Default::default()
                },
            )
            .await?;
    }

    Ok(())
}

fn validate_paths(job: &DownloadJob, media_root: &Path) -> Result<(), ()> {
    contained_path(media_root, Path::new(&job.final_video_path)).map_err(|_| ())?;
    validate_job_partial_path(media_root, job.id, &job.partial_video_path)?;

    if let Some(path) = &job.final_subtitle_path {
        contained_path(media_root, Path::new(path)).map_err(|_| ())?;
    }
    if let Some(path) = &job.partial_subtitle_path {
        validate_job_partial_path(media_root, job.id, path)?;
    }

    Ok(())
}

pub(crate) fn validate_job_partial_path(
    media_root: &Path,
    job_id: JobId,
    partial_path: &str,
) -> Result<PathBuf, ()> {
    let mut components = Path::new(partial_path).components();
    for expected in ["_moviebox", "jobs", job_id.to_string().as_str()] {
        match components.next() {
            Some(Component::Normal(value)) if value.to_string_lossy() == expected => {}
            _ => return Err(()),
        }
    }

    let Some(Component::Normal(filename)) = components.next() else {
        return Err(());
    };
    let filename = filename.to_string_lossy();
    if filename.len() <= ".part".len() || !filename.ends_with(".part") {
        return Err(());
    }
    if components.next().is_some() {
        return Err(());
    }

    contained_path(media_root, Path::new(partial_path)).map_err(|_| ())
}

fn final_video_is_complete(job: &DownloadJob, media_root: &Path) -> bool {
    let Ok(path) = contained_path(media_root, Path::new(&job.final_video_path)) else {
        return false;
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };

    job.total_bytes
        .is_some_and(|expected| metadata.len() == expected)
}

async fn fail_unsafe_path<S>(store: &S, job: &DownloadJob) -> Result<(), JobRepositoryError>
where
    S: JobStore + ?Sized,
{
    store
        .force_state(
            job.id,
            job.version,
            JobState::Failed,
            JobEventKind::StateChanged,
            failure_patch("unsafe_path", "job paths failed validation"),
        )
        .await?;
    Ok(())
}

pub(crate) fn failure_patch(code: &'static str, message: &'static str) -> JobStatePatch {
    JobStatePatch {
        error_code: Some(Some(code.to_string())),
        error_message: Some(Some(message.to_string())),
        speed_bytes_per_second: Some(None),
        ..Default::default()
    }
}

pub(crate) async fn fail_job<S>(
    store: &S,
    job_id: JobId,
    expected_version: i64,
    code: &'static str,
    message: &'static str,
) -> Result<(), JobRepositoryError>
where
    S: JobStore + ?Sized,
{
    store
        .force_state(
            job_id,
            expected_version,
            JobState::Failed,
            JobEventKind::StateChanged,
            failure_patch(code, message),
        )
        .await?;
    Ok(())
}
