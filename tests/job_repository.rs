#![cfg(feature = "server")]

use std::{collections::HashSet, path::PathBuf, time::Duration};

use moviebox_tui::{
    catalog::{CatalogId, MediaType, SourceId, SubtitleId},
    server::{
        db::connect,
        jobs::{
            JobEvent, JobEventKind, JobId, JobListCursor, JobProgress, JobRepository,
            JobRepositoryError, JobState, NewJob,
        },
    },
};
use sqlx::{Row, SqlitePool};
use tempfile::TempDir;
use tokio::{task::JoinSet, time::sleep};
use uuid::Uuid;

async fn test_repository() -> (TempDir, SqlitePool, JobRepository) {
    let temp_dir = tempfile::tempdir().unwrap();
    let database_path = temp_dir.path().join("jobs.sqlite3");
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = connect(&database_url).await.unwrap();
    let repository = JobRepository::new(pool.clone());
    (temp_dir, pool, repository)
}

fn new_job(seed: usize) -> NewJob {
    NewJob {
        catalog_id: CatalogId::new(format!("catalog-{seed}")),
        source_id: SourceId::new(format!("source-{seed}")),
        subtitle_id: Some(SubtitleId::new(format!("subtitle-{seed}"))),
        title: format!("Fixture Title {seed}"),
        year: Some("2024".to_string()),
        media_type: MediaType::Series,
        season_number: Some(1),
        episode_number: Some(seed as u16 + 1),
        episode_title: Some(format!("Episode {seed}")),
        requested_height: 1080,
        final_video_path: format!("Shows/Fixture Title {seed} (2024)/Season 01/Episode {seed}.mkv"),
        final_subtitle_path: Some(format!(
            "Shows/Fixture Title {seed} (2024)/Season 01/Episode {seed}.en.srt"
        )),
        partial_video_path: format!("_moviebox/jobs/job-{seed}/Episode {seed}.mkv.part"),
        partial_subtitle_path: Some(format!(
            "_moviebox/jobs/job-{seed}/Episode {seed}.en.srt.part"
        )),
    }
}

async fn create_job_in_state(
    repository: &JobRepository,
    state: JobState,
) -> moviebox_tui::server::jobs::DownloadJob {
    let mut job = repository
        .create(new_job(200 + state as usize))
        .await
        .unwrap();

    if state == JobState::Queued {
        return job;
    }

    job = repository
        .transition(
            job.id,
            job.version,
            JobState::Resolving,
            Some(JobEvent::new(JobEventKind::StateChanged)),
        )
        .await
        .unwrap();

    if state == JobState::Resolving {
        return job;
    }

    match state {
        JobState::Downloading
        | JobState::Paused
        | JobState::Finalizing
        | JobState::Ready
        | JobState::Cancelled => {
            job = repository
                .transition(
                    job.id,
                    job.version,
                    JobState::Downloading,
                    Some(JobEvent::new(JobEventKind::StateChanged)),
                )
                .await
                .unwrap();
        }
        JobState::Failed => {
            return repository
                .transition(
                    job.id,
                    job.version,
                    JobState::Failed,
                    Some(JobEvent::new(JobEventKind::StateChanged)),
                )
                .await
                .unwrap();
        }
        JobState::Queued | JobState::Resolving => {}
    }

    match state {
        JobState::Downloading => job,
        JobState::Paused => repository
            .transition(
                job.id,
                job.version,
                JobState::Paused,
                Some(JobEvent::new(JobEventKind::StateChanged)),
            )
            .await
            .unwrap(),
        JobState::Finalizing | JobState::Ready => {
            job = repository
                .transition(
                    job.id,
                    job.version,
                    JobState::Finalizing,
                    Some(JobEvent::new(JobEventKind::StateChanged)),
                )
                .await
                .unwrap();

            if state == JobState::Finalizing {
                job
            } else {
                repository
                    .transition(
                        job.id,
                        job.version,
                        JobState::Ready,
                        Some(JobEvent::new(JobEventKind::StateChanged)),
                    )
                    .await
                    .unwrap()
            }
        }
        JobState::Cancelled => repository
            .transition(
                job.id,
                job.version,
                JobState::Cancelled,
                Some(JobEvent::new(JobEventKind::StateChanged)),
            )
            .await
            .unwrap(),
        JobState::Queued | JobState::Resolving | JobState::Failed => unreachable!(),
    }
}

fn all_states() -> [JobState; 8] {
    [
        JobState::Queued,
        JobState::Resolving,
        JobState::Downloading,
        JobState::Paused,
        JobState::Finalizing,
        JobState::Ready,
        JobState::Failed,
        JobState::Cancelled,
    ]
}

#[tokio::test]
async fn migration_creates_tables_and_indexes() {
    let (_temp_dir, pool, _repository) = test_repository().await;

    let names = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type IN ('table', 'index') ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| row.get::<String, _>("name"))
    .collect::<Vec<_>>();

    assert!(names.contains(&"jobs".to_string()));
    assert!(names.contains(&"job_events".to_string()));
    assert!(names.contains(&"sessions".to_string()));
    assert!(names.contains(&"users".to_string()));
    assert!(names.contains(&"idx_jobs_state_next_attempt_at".to_string()));
    assert!(names.contains(&"idx_sessions_expires_at".to_string()));
}

#[test]
fn malformed_warning_is_rejected_without_panicking() {
    let result = std::panic::catch_unwind(|| {
        JobEvent::new(JobEventKind::StateChanged).with_warning("malformed\u{0}warning")
    });

    assert!(result.is_ok(), "malformed warning input must not panic");
    let error = result.unwrap().unwrap_err();
    assert!(matches!(
        error,
        JobRepositoryError::InvalidData(message)
            if message == "text must not contain control characters"
    ));
}

#[tokio::test]
async fn create_persists_job_with_relative_paths_only() {
    let (_temp_dir, pool, repository) = test_repository().await;

    let job = repository.create(new_job(1)).await.unwrap();

    assert_eq!(job.state, JobState::Queued);
    assert_eq!(job.version, 0);
    assert!(!PathBuf::from(job.final_video_path.clone()).is_absolute());
    let stored = sqlx::query(
        "SELECT final_video_path, partial_video_path, catalog_id, source_id, subtitle_id FROM jobs WHERE id = ?1",
    )
    .bind(job.id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        stored.get::<String, _>("final_video_path"),
        "Shows/Fixture Title 1 (2024)/Season 01/Episode 1.mkv"
    );
    assert_eq!(
        stored.get::<String, _>("partial_video_path"),
        "_moviebox/jobs/job-1/Episode 1.mkv.part"
    );
    assert_eq!(stored.get::<String, _>("catalog_id"), "catalog-1");
    assert_eq!(stored.get::<String, _>("source_id"), "source-1");
    assert_eq!(
        stored.get::<Option<String>, _>("subtitle_id"),
        Some("subtitle-1".to_string())
    );
}

#[tokio::test]
async fn create_with_id_keeps_job_id_in_partial_paths() {
    let (_temp_dir, _pool, repository) = test_repository().await;
    let uuid = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
    let job_id = JobId::new(uuid);
    let mut input = new_job(2);
    input.partial_video_path = format!("_moviebox/jobs/{uuid}/Episode.mkv.part");
    input.partial_subtitle_path = Some(format!("_moviebox/jobs/{uuid}/Episode.en.srt.part"));

    let job = repository.create_with_id(job_id, input).await.unwrap();

    assert_eq!(job.id, job_id);
    assert_eq!(
        job.partial_video_path,
        format!("_moviebox/jobs/{uuid}/Episode.mkv.part")
    );
    assert_eq!(
        job.partial_subtitle_path.as_deref(),
        Some(format!("_moviebox/jobs/{uuid}/Episode.en.srt.part").as_str())
    );
}

#[tokio::test]
async fn sqlite_busy_writes_wait_for_short_locks() {
    let (_temp_dir, pool, repository) = test_repository().await;
    let locked_job = repository.create(new_job(90)).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE jobs SET warning = ?1 WHERE id = ?2")
        .bind("short lock")
        .bind(locked_job.id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();

    let waiter = tokio::spawn({
        let repository = repository.clone();
        async move { repository.create(new_job(91)).await }
    });
    sleep(Duration::from_secs(6)).await;
    tx.commit().await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(8), waiter)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_ok(), "short SQLite locks should be retried");
}

#[tokio::test]
async fn transition_allows_every_legal_state_change() {
    let (_temp_dir, _pool, repository) = test_repository().await;
    let legal = [
        (JobState::Queued, JobState::Resolving),
        (JobState::Queued, JobState::Cancelled),
        (JobState::Resolving, JobState::Downloading),
        (JobState::Resolving, JobState::Failed),
        (JobState::Downloading, JobState::Paused),
        (JobState::Downloading, JobState::Finalizing),
        (JobState::Downloading, JobState::Failed),
        (JobState::Downloading, JobState::Cancelled),
        (JobState::Paused, JobState::Queued),
        (JobState::Paused, JobState::Cancelled),
        (JobState::Finalizing, JobState::Ready),
        (JobState::Finalizing, JobState::Failed),
        (JobState::Failed, JobState::Queued),
    ];

    for (index, (from, to)) in legal.into_iter().enumerate() {
        let job = create_job_in_state(&repository, from).await;
        let event = JobEvent::new(JobEventKind::StateChanged)
            .with_warning(format!("legal-{index}"))
            .unwrap();
        let updated = repository
            .transition(job.id, job.version, to, Some(event))
            .await
            .unwrap();

        assert_eq!(updated.state, to, "{from:?} -> {to:?}");
        assert_eq!(updated.version, job.version + 1, "{from:?} -> {to:?}");
    }
}

#[tokio::test]
async fn transition_rejects_every_illegal_state_change() {
    let (_temp_dir, _pool, repository) = test_repository().await;
    let legal = [
        (JobState::Queued, JobState::Resolving),
        (JobState::Queued, JobState::Cancelled),
        (JobState::Resolving, JobState::Downloading),
        (JobState::Resolving, JobState::Failed),
        (JobState::Downloading, JobState::Paused),
        (JobState::Downloading, JobState::Finalizing),
        (JobState::Downloading, JobState::Failed),
        (JobState::Downloading, JobState::Cancelled),
        (JobState::Paused, JobState::Queued),
        (JobState::Paused, JobState::Cancelled),
        (JobState::Finalizing, JobState::Ready),
        (JobState::Finalizing, JobState::Failed),
        (JobState::Failed, JobState::Queued),
    ];

    for from in all_states() {
        let job = create_job_in_state(&repository, from).await;
        for to in all_states() {
            if from == to || legal.contains(&(from, to)) {
                continue;
            }

            let error = repository
                .transition(
                    job.id,
                    job.version,
                    to,
                    Some(JobEvent::new(JobEventKind::StateChanged)),
                )
                .await
                .unwrap_err();

            assert!(
                matches!(
                    error,
                    JobRepositoryError::InvalidTransition {
                        from: actual_from,
                        to: actual_to,
                    } if actual_from == from && actual_to == to
                ),
                "{from:?} -> {to:?} returned {error:?}"
            );
        }
    }
}

#[tokio::test]
async fn transition_reports_stale_version_conflicts() {
    let (_temp_dir, _pool, repository) = test_repository().await;
    let job = repository.create(new_job(2)).await.unwrap();

    let updated = repository
        .transition(
            job.id,
            job.version,
            JobState::Resolving,
            Some(JobEvent::new(JobEventKind::StateChanged)),
        )
        .await
        .unwrap();

    let error = repository
        .transition(
            job.id,
            job.version,
            JobState::Cancelled,
            Some(JobEvent::new(JobEventKind::StateChanged)),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        JobRepositoryError::Conflict {
            expected_version,
            actual_version,
        } if expected_version == job.version && actual_version == updated.version
    ));
}

#[tokio::test]
async fn progress_updates_persist_and_append_events() {
    let (_temp_dir, pool, repository) = test_repository().await;
    let job = repository.create(new_job(3)).await.unwrap();
    let resolving = repository
        .transition(
            job.id,
            job.version,
            JobState::Resolving,
            Some(JobEvent::new(JobEventKind::Claimed)),
        )
        .await
        .unwrap();
    let downloading = repository
        .transition(
            resolving.id,
            resolving.version,
            JobState::Downloading,
            Some(JobEvent::new(JobEventKind::StateChanged)),
        )
        .await
        .unwrap();

    repository
        .update_progress(
            downloading.id,
            JobProgress {
                expected_version: downloading.version,
                downloaded_bytes: 1_048_576,
                total_bytes: Some(5_242_880),
                speed_bytes_per_second: Some(256_000),
            },
            Some(JobEvent::new(JobEventKind::ProgressUpdated)),
        )
        .await
        .unwrap();

    let row = sqlx::query(
        "SELECT state, downloaded_bytes, total_bytes, speed_bytes_per_second, version FROM jobs WHERE id = ?1",
    )
    .bind(downloading.id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("state"), "downloading");
    assert_eq!(row.get::<i64, _>("downloaded_bytes"), 1_048_576);
    assert_eq!(row.get::<Option<i64>, _>("total_bytes"), Some(5_242_880));
    assert_eq!(
        row.get::<Option<i64>, _>("speed_bytes_per_second"),
        Some(256_000)
    );
    assert_eq!(row.get::<i64, _>("version"), downloading.version + 1);

    let event_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM job_events WHERE job_id = ?1")
            .bind(downloading.id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(event_count, 4);
}

#[tokio::test]
async fn list_paginates_by_created_at() {
    let (_temp_dir, _pool, repository) = test_repository().await;
    let first = repository.create(new_job(10)).await.unwrap();
    sleep(Duration::from_millis(5)).await;
    let second = repository.create(new_job(11)).await.unwrap();
    sleep(Duration::from_millis(5)).await;
    let third = repository.create(new_job(12)).await.unwrap();

    let all = repository.list(10, None).await.unwrap();
    assert_eq!(
        all.iter().map(|job| job.id).collect::<Vec<_>>(),
        vec![third.id, second.id, first.id]
    );

    let older = repository
        .list(10, Some(JobListCursor::from_job(&second)))
        .await
        .unwrap();
    assert_eq!(older.len(), 1);
    assert_eq!(older[0].id, first.id);
}

#[tokio::test]
async fn list_paginates_through_jobs_with_identical_timestamps() {
    let (_temp_dir, pool, repository) = test_repository().await;
    let mut jobs = Vec::new();
    for seed in 60..65 {
        jobs.push(repository.create(new_job(seed)).await.unwrap());
    }
    let expected_ids = jobs.iter().map(|job| job.id).collect::<HashSet<_>>();

    let timestamp = sqlx::query_scalar::<_, i64>("SELECT created_at FROM jobs LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET created_at = ?1")
        .bind(timestamp)
        .execute(&pool)
        .await
        .unwrap();

    let mut cursor = None;
    let mut seen_ids = Vec::new();
    loop {
        let page = repository.list(2, cursor).await.unwrap();
        if page.is_empty() {
            break;
        }
        cursor = Some(JobListCursor::from_job(page.last().unwrap()));
        seen_ids.extend(page.into_iter().map(|job| job.id));
    }

    assert_eq!(seen_ids.len(), jobs.len());
    assert_eq!(seen_ids.into_iter().collect::<HashSet<_>>(), expected_ids);
}

#[tokio::test]
async fn claim_next_is_atomic_under_concurrent_callers() {
    let (_temp_dir, pool, repository) = test_repository().await;
    let job = repository.create(new_job(50)).await.unwrap();
    let mut tasks = JoinSet::new();

    for _ in 0..4 {
        let repository = repository.clone();
        tasks.spawn(async move { repository.claim_next().await });
    }

    let mut claimed = Vec::new();
    while let Some(result) = tasks.join_next().await {
        let job = result.unwrap().unwrap();
        if let Some(job) = job {
            claimed.push(job);
        }
    }

    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, job.id);
    assert_eq!(claimed[0].state, JobState::Resolving);

    let stored = sqlx::query("SELECT state, version FROM jobs WHERE id = ?1")
        .bind(job.id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored.get::<String, _>("state"), "resolving");
    assert_eq!(stored.get::<i64, _>("version"), 1);
}
