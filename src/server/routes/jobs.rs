use super::{mutation_session, session};
use crate::{
    catalog::{CatalogId, EpisodeRequest, MediaType, SourceId, SubtitleId},
    server::{
        error::ApiError,
        jobs::{JobEvent as RepoEvent, JobEventKind, JobId, JobState, NewJob},
        library::{LibraryNamer, MediaIdentity},
        state::AppState,
    },
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list).post(create))
        .route("/jobs/{id}", get(get_one))
        .route("/jobs/{id}/pause", post(pause))
        .route("/jobs/{id}/resume", post(resume))
        .route("/jobs/{id}/cancel", post(cancel))
        .route("/jobs/{id}/retry", post(retry))
}

#[derive(Debug, Deserialize)]
struct CreateRequest {
    catalog_id: String,
    source_id: String,
    subtitle_id: Option<String>,
    requested_height: u16,
    season: Option<u16>,
    episode: Option<u16>,
}
#[derive(Debug, Deserialize)]
struct ListQuery {
    limit: Option<u32>,
}
#[derive(Debug, Deserialize)]
struct TransitionRequest {
    version: i64,
}

#[derive(Debug, Serialize)]
struct JobDto {
    id: String,
    catalog_id: String,
    source_id: String,
    subtitle_id: Option<String>,
    title: String,
    year: Option<String>,
    media_type: MediaType,
    season: Option<u16>,
    episode: Option<u16>,
    episode_title: Option<String>,
    requested_height: u16,
    state: String,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    speed_bytes_per_second: Option<u64>,
    attempt: u32,
    error_code: Option<String>,
    error_message: Option<String>,
    warning: Option<String>,
    version: i64,
}

impl From<crate::server::jobs::DownloadJob> for JobDto {
    fn from(job: crate::server::jobs::DownloadJob) -> Self {
        Self {
            id: job.id.to_string(),
            catalog_id: job.catalog_id.as_str().into(),
            source_id: job.source_id.as_str().into(),
            subtitle_id: job.subtitle_id.map(|v| v.as_str().into()),
            title: job.title,
            year: job.year,
            media_type: job.media_type,
            season: job.season_number,
            episode: job.episode_number,
            episode_title: job.episode_title,
            requested_height: job.requested_height,
            state: job.state.as_str().into(),
            downloaded_bytes: job.downloaded_bytes,
            total_bytes: job.total_bytes,
            speed_bytes_per_second: job.speed_bytes_per_second,
            attempt: job.attempt,
            error_code: job.error_code,
            error_message: job.error_message,
            warning: job.warning,
            version: job.version,
        }
    }
}

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    session(&state, &headers).await?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=50).contains(&limit) {
        return Err(ApiError::invalid_request());
    }
    let jobs = state
        .jobs()
        .list(limit, None)
        .await
        .map_err(|_| ApiError::internal())?;
    private_json(jobs.into_iter().map(JobDto::from).collect::<Vec<_>>())
}

async fn get_one(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    session(&state, &headers).await?;
    let id = parse_id(&id)?;
    let job = state.jobs().get(id).await.map_err(map_job_error)?;
    private_json(JobDto::from(job))
}

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateRequest>,
) -> Result<Response, ApiError> {
    mutation_session(&state, &headers).await?;
    if request.requested_height == 0 {
        return Err(ApiError::invalid_request());
    }
    if request.requested_height > state.config().maximum_height {
        return Err(ApiError::quality_exceeds_limit());
    }
    let catalog_id = CatalogId::new(request.catalog_id);
    let details = state
        .catalog()
        .details(&catalog_id)
        .await
        .map_err(map_catalog_error)?;
    let identity = MediaIdentity::from_details(&details, request.season, request.episode)
        .map_err(|_| ApiError::invalid_request())?;
    let source_id = SourceId::new(request.source_id);
    let sources = state
        .catalog()
        .sources(EpisodeRequest {
            catalog_id: catalog_id.clone(),
            season: request.season,
            episode: request.episode,
        })
        .await
        .map_err(map_catalog_error)?;
    let selected_source = sources
        .iter()
        .find(|source| source.id.as_str() == source_id.as_str())
        .ok_or_else(|| {
            ApiError::new_status(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_source",
                "The selected source is invalid.",
            )
        })?;
    if selected_source.height != request.requested_height {
        return Err(ApiError::quality_unavailable());
    }
    let subtitle_for_resolution = request
        .subtitle_id
        .as_ref()
        .map(|value| SubtitleId::new(value.clone()));
    state
        .catalog()
        .resolve(&source_id, subtitle_for_resolution.as_ref())
        .await
        .map_err(map_catalog_error)?;
    let id = Uuid::new_v4();
    let namer = LibraryNamer::new(&state.config().media_root).map_err(|_| ApiError::internal())?;
    let paths = namer
        .paths_for(&identity, "mkv", None, id)
        .map_err(|_| ApiError::internal())?;
    let job = state
        .jobs()
        .create_with_id(
            JobId::new(id),
            NewJob {
                catalog_id,
                source_id,
                subtitle_id: request.subtitle_id.map(SubtitleId::new),
                title: details.title,
                year: details.year,
                media_type: details.media_type,
                season_number: request.season,
                episode_number: request.episode,
                episode_title: match &identity {
                    MediaIdentity::Episode { episode_title, .. } => episode_title.clone(),
                    _ => None,
                },
                requested_height: request.requested_height,
                final_video_path: paths.video_relative.to_string_lossy().into(),
                final_subtitle_path: paths.subtitle_relative.map(|p| p.to_string_lossy().into()),
                partial_video_path: paths.partial_video_relative.to_string_lossy().into(),
                partial_subtitle_path: paths
                    .partial_subtitle_relative
                    .map(|p| p.to_string_lossy().into()),
            },
        )
        .await
        .map_err(|_| ApiError::internal())?;
    state.events().publish_job(&job, JobEventKind::Created);
    let mut response = (StatusCode::ACCEPTED, Json(JobDto::from(job))).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(response)
}

async fn pause(
    state: State<AppState>,
    headers: HeaderMap,
    path: Path<String>,
    body: Json<TransitionRequest>,
) -> Result<Response, ApiError> {
    transition(state, headers, path, body, JobState::Paused).await
}
async fn resume(
    state: State<AppState>,
    headers: HeaderMap,
    path: Path<String>,
    body: Json<TransitionRequest>,
) -> Result<Response, ApiError> {
    transition(state, headers, path, body, JobState::Queued).await
}
async fn cancel(
    state: State<AppState>,
    headers: HeaderMap,
    path: Path<String>,
    body: Json<TransitionRequest>,
) -> Result<Response, ApiError> {
    transition(state, headers, path, body, JobState::Cancelled).await
}
async fn retry(
    state: State<AppState>,
    headers: HeaderMap,
    path: Path<String>,
    body: Json<TransitionRequest>,
) -> Result<Response, ApiError> {
    transition(state, headers, path, body, JobState::Queued).await
}

async fn transition(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<TransitionRequest>,
    to: JobState,
) -> Result<Response, ApiError> {
    mutation_session(&state, &headers).await?;
    let job = state
        .jobs()
        .transition(
            parse_id(&id)?,
            body.version,
            to,
            Some(RepoEvent::new(JobEventKind::StateChanged)),
        )
        .await
        .map_err(map_job_error)?;
    state.events().publish_job(&job, JobEventKind::StateChanged);
    private_json(JobDto::from(job))
}

fn parse_id(value: &str) -> Result<JobId, ApiError> {
    Uuid::parse_str(value)
        .map(JobId::new)
        .map_err(|_| ApiError::not_found())
}
fn private_json<T: Serialize>(value: T) -> Result<Response, ApiError> {
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(response)
}
fn map_job_error(error: crate::server::jobs::JobRepositoryError) -> ApiError {
    match error {
        crate::server::jobs::JobRepositoryError::NotFound(_) => ApiError::not_found(),
        crate::server::jobs::JobRepositoryError::InvalidTransition { .. }
        | crate::server::jobs::JobRepositoryError::Conflict { .. } => ApiError::conflict(),
        _ => ApiError::internal(),
    }
}
fn map_catalog_error(error: crate::catalog::CatalogError) -> ApiError {
    use crate::catalog::CatalogError;
    match error {
        CatalogError::QualityUnavailable {
            requested_height: Some(h),
            ..
        } if h > 1080 => ApiError::quality_exceeds_limit(),
        CatalogError::NotFound(_) => ApiError::not_found(),
        CatalogError::Provider(_) => ApiError::provider_unavailable(),
        CatalogError::InvalidOpaqueId | CatalogError::OpaqueIdVerificationFailed => {
            ApiError::new_status(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_source",
                "The selected source is invalid.",
            )
        }
        _ => ApiError::quality_unavailable(),
    }
}
