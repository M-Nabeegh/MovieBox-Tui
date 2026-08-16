use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteQueryResult, SqliteRow},
};
use time::OffsetDateTime;
use uuid::Uuid;

use super::model::{
    DownloadJob, JobEvent, JobEventKind, JobId, JobListCursor, JobProgress, JobRepositoryError,
    JobState, NewJob, apply_event_fields, can_transition,
};
use super::worker::JobStatePatch;
use crate::catalog::{CatalogId, MediaType, SourceId, SubtitleId};

#[derive(Debug, Clone)]
pub struct JobRepository {
    pool: SqlitePool,
}

impl JobRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, id: JobId) -> Result<DownloadJob, JobRepositoryError> {
        fetch_job_with_executor(&self.pool, id).await
    }

    pub async fn list_all(&self) -> Result<Vec<DownloadJob>, JobRepositoryError> {
        let rows = sqlx::query("SELECT * FROM jobs ORDER BY created_at ASC, id ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(|row| job_from_row(&row)).collect()
    }

    pub async fn create(&self, input: NewJob) -> Result<DownloadJob, JobRepositoryError> {
        self.create_with_id(JobId::new(Uuid::new_v4()), input).await
    }

    pub async fn create_with_id(
        &self,
        id: JobId,
        input: NewJob,
    ) -> Result<DownloadJob, JobRepositoryError> {
        input.validate()?;

        let created_at_millis = now_millis()?;
        let version = 0_i64;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO jobs (
                id, catalog_id, source_id, subtitle_id, title, year, media_type,
                season_number, episode_number, episode_title, requested_height, state,
                final_video_path, final_subtitle_path, partial_video_path, partial_subtitle_path,
                downloaded_bytes, total_bytes, speed_bytes_per_second, attempt,
                error_code, error_message, warning, created_at, updated_at, version
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16,
                0, NULL, NULL, 0,
                NULL, NULL, NULL, ?17, ?17, ?18
            )
            "#,
        )
        .bind(id.to_string())
        .bind(input.catalog_id.as_str())
        .bind(input.source_id.as_str())
        .bind(input.subtitle_id.as_ref().map(SubtitleId::as_str))
        .bind(&input.title)
        .bind(&input.year)
        .bind(media_type_as_str(input.media_type))
        .bind(input.season_number.map(i64::from))
        .bind(input.episode_number.map(i64::from))
        .bind(&input.episode_title)
        .bind(i64::from(input.requested_height))
        .bind(JobState::Queued.as_str())
        .bind(&input.final_video_path)
        .bind(&input.final_subtitle_path)
        .bind(&input.partial_video_path)
        .bind(&input.partial_subtitle_path)
        .bind(created_at_millis)
        .bind(version)
        .execute(&mut *tx)
        .await?;

        let job = fetch_job_with_executor(&mut *tx, id).await?;
        insert_event(&mut *tx, &job, JobEventKind::Created).await?;
        tx.commit().await?;
        Ok(job)
    }

    pub async fn claim_next(&self) -> Result<Option<DownloadJob>, JobRepositoryError> {
        let mut tx = self.pool.begin().await?;
        let now = now_millis()?;
        let maybe_row = sqlx::query(
            r#"
            UPDATE jobs
            SET
                state = ?1,
                updated_at = ?2,
                version = version + 1,
                attempt = attempt + 1,
                next_attempt_at = NULL
            WHERE id = (
                SELECT id
                FROM jobs
                WHERE state = ?3
                  AND (next_attempt_at IS NULL OR next_attempt_at <= ?2)
                ORDER BY created_at ASC, id ASC
                LIMIT 1
            )
            AND state = ?3
            RETURNING *
            "#,
        )
        .bind(JobState::Resolving.as_str())
        .bind(now)
        .bind(JobState::Queued.as_str())
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = maybe_row else {
            tx.commit().await?;
            return Ok(None);
        };

        let job = job_from_row(&row)?;
        insert_event(&mut *tx, &job, JobEventKind::Claimed).await?;
        tx.commit().await?;
        Ok(Some(job))
    }

    pub async fn transition(
        &self,
        id: JobId,
        expected_version: i64,
        to: JobState,
        event: Option<JobEvent>,
    ) -> Result<DownloadJob, JobRepositoryError> {
        let mut tx = self.pool.begin().await?;
        let current = fetch_job_with_executor(&mut *tx, id).await?;
        if current.version != expected_version {
            return Err(JobRepositoryError::Conflict {
                expected_version,
                actual_version: current.version,
            });
        }
        if !can_transition(current.state, to) {
            return Err(JobRepositoryError::InvalidTransition {
                from: current.state,
                to,
            });
        }

        let now = now_millis()?;
        let event_fields = apply_event_fields(to, current.warning.as_deref(), event.as_ref())?;
        // A user-driven resume or retry should start immediately and receive a
        // fresh budget of automatic attempts, so clear any pending backoff.
        let restarting = to == JobState::Queued;
        let row = sqlx::query(
            r#"
            UPDATE jobs
            SET
                state = ?1,
                error_code = ?2,
                error_message = ?3,
                warning = ?4,
                updated_at = ?5,
                attempt = CASE WHEN ?6 THEN 0 ELSE attempt END,
                next_attempt_at = CASE WHEN ?6 THEN NULL ELSE next_attempt_at END,
                version = version + 1
            WHERE id = ?7 AND version = ?8
            RETURNING *
            "#,
        )
        .bind(to.as_str())
        .bind(event_fields.error_code)
        .bind(event_fields.error_message)
        .bind(event_fields.warning)
        .bind(now)
        .bind(restarting)
        .bind(id.to_string())
        .bind(expected_version)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = row else {
            let actual = fetch_job_with_executor(&mut *tx, id).await?;
            return Err(JobRepositoryError::Conflict {
                expected_version,
                actual_version: actual.version,
            });
        };

        let job = job_from_row(&row)?;
        insert_event(
            &mut *tx,
            &job,
            event
                .as_ref()
                .map(|item| item.kind)
                .unwrap_or(JobEventKind::StateChanged),
        )
        .await?;
        tx.commit().await?;
        Ok(job)
    }

    pub async fn update_progress(
        &self,
        id: JobId,
        progress: JobProgress,
        event: Option<JobEvent>,
    ) -> Result<DownloadJob, JobRepositoryError> {
        let mut tx = self.pool.begin().await?;
        let current = fetch_job_with_executor(&mut *tx, id).await?;
        if current.version != progress.expected_version {
            return Err(JobRepositoryError::Conflict {
                expected_version: progress.expected_version,
                actual_version: current.version,
            });
        }
        if !matches!(current.state, JobState::Downloading | JobState::Finalizing) {
            return Err(JobRepositoryError::InvalidProgressState(current.state));
        }

        let now = now_millis()?;
        let event_fields =
            apply_event_fields(current.state, current.warning.as_deref(), event.as_ref())?;
        let row = sqlx::query(
            r#"
            UPDATE jobs
            SET
                downloaded_bytes = ?1,
                total_bytes = ?2,
                speed_bytes_per_second = ?3,
                warning = ?4,
                updated_at = ?5,
                version = version + 1
            WHERE id = ?6 AND version = ?7
            RETURNING *
            "#,
        )
        .bind(to_i64(progress.downloaded_bytes)?)
        .bind(progress.total_bytes.map(to_i64).transpose()?)
        .bind(progress.speed_bytes_per_second.map(to_i64).transpose()?)
        .bind(event_fields.warning)
        .bind(now)
        .bind(id.to_string())
        .bind(progress.expected_version)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = row else {
            let actual = fetch_job_with_executor(&mut *tx, id).await?;
            return Err(JobRepositoryError::Conflict {
                expected_version: progress.expected_version,
                actual_version: actual.version,
            });
        };

        let job = job_from_row(&row)?;
        insert_event(
            &mut *tx,
            &job,
            event
                .as_ref()
                .map(|item| item.kind)
                .unwrap_or(JobEventKind::ProgressUpdated),
        )
        .await?;
        tx.commit().await?;
        Ok(job)
    }

    pub async fn force_state(
        &self,
        id: JobId,
        expected_version: i64,
        to: JobState,
        kind: JobEventKind,
        patch: JobStatePatch,
    ) -> Result<DownloadJob, JobRepositoryError> {
        let mut tx = self.pool.begin().await?;
        let current = fetch_job_with_executor(&mut *tx, id).await?;
        if current.version != expected_version {
            return Err(JobRepositoryError::Conflict {
                expected_version,
                actual_version: current.version,
            });
        }

        let downloaded_bytes = patch.downloaded_bytes.unwrap_or(current.downloaded_bytes);
        let total_bytes = patch.total_bytes.unwrap_or(current.total_bytes);
        let speed_bytes_per_second = patch
            .speed_bytes_per_second
            .unwrap_or(current.speed_bytes_per_second);
        let error_code = patch.error_code.unwrap_or(current.error_code);
        let error_message = patch.error_message.unwrap_or(current.error_message);
        let warning = patch.warning.unwrap_or(current.warning);
        let next_attempt_at = patch.next_attempt_at.unwrap_or(current.next_attempt_at);
        let row = sqlx::query(
            r#"
            UPDATE jobs
            SET
                state = ?1,
                downloaded_bytes = ?2,
                total_bytes = ?3,
                speed_bytes_per_second = ?4,
                error_code = ?5,
                error_message = ?6,
                warning = ?7,
                next_attempt_at = ?8,
                updated_at = ?9,
                version = version + 1
            WHERE id = ?10 AND version = ?11
            RETURNING *
            "#,
        )
        .bind(to.as_str())
        .bind(to_i64(downloaded_bytes)?)
        .bind(total_bytes.map(to_i64).transpose()?)
        .bind(speed_bytes_per_second.map(to_i64).transpose()?)
        .bind(error_code)
        .bind(error_message)
        .bind(warning)
        .bind(next_attempt_at.map(to_millis).transpose()?)
        .bind(now_millis()?)
        .bind(id.to_string())
        .bind(expected_version)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = row else {
            let actual = fetch_job_with_executor(&mut *tx, id).await?;
            return Err(JobRepositoryError::Conflict {
                expected_version,
                actual_version: actual.version,
            });
        };

        let job = job_from_row(&row)?;
        insert_event(&mut *tx, &job, kind).await?;
        tx.commit().await?;
        Ok(job)
    }

    pub async fn list(
        &self,
        limit: u32,
        cursor: Option<JobListCursor>,
    ) -> Result<Vec<DownloadJob>, JobRepositoryError> {
        let limit = i64::from(limit.clamp(1, 100));
        let rows = match cursor {
            Some(cursor) => {
                sqlx::query(
                    r#"
                    SELECT *
                    FROM jobs
                    WHERE created_at < ?1 OR (created_at = ?1 AND id < ?2)
                    ORDER BY created_at DESC, id DESC
                    LIMIT ?3
                    "#,
                )
                .bind(to_millis(cursor.created_at())?)
                .bind(cursor.id().to_string())
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    r#"
                    SELECT *
                    FROM jobs
                    ORDER BY created_at DESC, id DESC
                    LIMIT ?1
                    "#,
                )
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
        };

        rows.into_iter().map(|row| job_from_row(&row)).collect()
    }
}

async fn fetch_job_with_executor<'e, E>(
    executor: E,
    id: JobId,
) -> Result<DownloadJob, JobRepositoryError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row = sqlx::query("SELECT * FROM jobs WHERE id = ?1")
        .bind(id.to_string())
        .fetch_optional(executor)
        .await?;
    row.map(|row| job_from_row(&row))
        .transpose()?
        .ok_or(JobRepositoryError::NotFound(id))
}

async fn insert_event<'e, E>(
    executor: E,
    job: &DownloadJob,
    kind: JobEventKind,
) -> Result<SqliteQueryResult, JobRepositoryError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    Ok(sqlx::query(
        r#"
        INSERT INTO job_events (
            job_id, job_version, kind, state, downloaded_bytes, total_bytes,
            speed_bytes_per_second, attempt, error_code, error_message, warning, created_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6,
            ?7, ?8, ?9, ?10, ?11, ?12
        )
        "#,
    )
    .bind(job.id.to_string())
    .bind(job.version)
    .bind(kind.as_str())
    .bind(job.state.as_str())
    .bind(to_i64(job.downloaded_bytes)?)
    .bind(job.total_bytes.map(to_i64).transpose()?)
    .bind(job.speed_bytes_per_second.map(to_i64).transpose()?)
    .bind(i64::from(job.attempt))
    .bind(&job.error_code)
    .bind(&job.error_message)
    .bind(&job.warning)
    .bind(to_millis(job.updated_at)?)
    .execute(executor)
    .await?)
}

fn job_from_row(row: &SqliteRow) -> Result<DownloadJob, JobRepositoryError> {
    Ok(DownloadJob {
        id: JobId::new(parse_uuid(row.get::<String, _>("id"))?),
        catalog_id: CatalogId::new(row.get::<String, _>("catalog_id")),
        source_id: SourceId::new(row.get::<String, _>("source_id")),
        subtitle_id: row
            .get::<Option<String>, _>("subtitle_id")
            .map(SubtitleId::new),
        title: row.get::<String, _>("title"),
        year: row.get::<Option<String>, _>("year"),
        media_type: parse_media_type(row.get::<String, _>("media_type").as_str())?,
        season_number: row
            .get::<Option<i64>, _>("season_number")
            .map(to_u16)
            .transpose()?,
        episode_number: row
            .get::<Option<i64>, _>("episode_number")
            .map(to_u16)
            .transpose()?,
        episode_title: row.get::<Option<String>, _>("episode_title"),
        requested_height: to_u16(row.get::<i64, _>("requested_height"))?,
        state: JobState::parse(row.get::<String, _>("state").as_str())?,
        final_video_path: row.get::<String, _>("final_video_path"),
        final_subtitle_path: row.get::<Option<String>, _>("final_subtitle_path"),
        partial_video_path: row.get::<String, _>("partial_video_path"),
        partial_subtitle_path: row.get::<Option<String>, _>("partial_subtitle_path"),
        downloaded_bytes: to_u64(row.get::<i64, _>("downloaded_bytes"))?,
        total_bytes: row
            .get::<Option<i64>, _>("total_bytes")
            .map(to_u64)
            .transpose()?,
        speed_bytes_per_second: row
            .get::<Option<i64>, _>("speed_bytes_per_second")
            .map(to_u64)
            .transpose()?,
        attempt: to_u32(row.get::<i64, _>("attempt"))?,
        error_code: row.get::<Option<String>, _>("error_code"),
        error_message: row.get::<Option<String>, _>("error_message"),
        warning: row.get::<Option<String>, _>("warning"),
        next_attempt_at: row
            .get::<Option<i64>, _>("next_attempt_at")
            .map(from_millis)
            .transpose()?,
        created_at: from_millis(row.get::<i64, _>("created_at"))?,
        updated_at: from_millis(row.get::<i64, _>("updated_at"))?,
        version: row.get::<i64, _>("version"),
    })
}

fn media_type_as_str(media_type: MediaType) -> &'static str {
    match media_type {
        MediaType::Movie => "movie",
        MediaType::Series => "series",
    }
}

fn parse_media_type(value: &str) -> Result<MediaType, JobRepositoryError> {
    match value {
        "movie" => Ok(MediaType::Movie),
        "series" => Ok(MediaType::Series),
        _ => Err(JobRepositoryError::InvalidData(format!(
            "unknown media type: {value}"
        ))),
    }
}

fn parse_uuid(value: String) -> Result<Uuid, JobRepositoryError> {
    Uuid::parse_str(&value)
        .map_err(|error| JobRepositoryError::InvalidData(format!("invalid uuid: {error}")))
}

fn to_u16(value: i64) -> Result<u16, JobRepositoryError> {
    value.try_into().map_err(|_| {
        JobRepositoryError::InvalidData(format!("value out of range for u16: {value}"))
    })
}

fn to_u32(value: i64) -> Result<u32, JobRepositoryError> {
    value.try_into().map_err(|_| {
        JobRepositoryError::InvalidData(format!("value out of range for u32: {value}"))
    })
}

fn to_u64(value: i64) -> Result<u64, JobRepositoryError> {
    value.try_into().map_err(|_| {
        JobRepositoryError::InvalidData(format!("value out of range for u64: {value}"))
    })
}

fn to_i64(value: u64) -> Result<i64, JobRepositoryError> {
    value.try_into().map_err(|_| {
        JobRepositoryError::InvalidData(format!("value out of range for i64: {value}"))
    })
}

fn now_millis() -> Result<i64, JobRepositoryError> {
    to_millis(OffsetDateTime::now_utc())
}

fn to_millis(value: OffsetDateTime) -> Result<i64, JobRepositoryError> {
    let millis = value.unix_timestamp_nanos() / 1_000_000;
    millis
        .try_into()
        .map_err(|_| JobRepositoryError::InvalidData("timestamp out of range".to_string()))
}

fn from_millis(value: i64) -> Result<OffsetDateTime, JobRepositoryError> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(value) * 1_000_000)
        .map_err(|error| JobRepositoryError::InvalidData(format!("invalid timestamp: {error}")))
}
