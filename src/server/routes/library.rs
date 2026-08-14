use super::session;
use crate::server::{
    error::ApiError,
    jobs::{JobId, JobState},
    library::jellyfin::{JellyfinClient, JellyfinStatus},
    state::AppState,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::get,
};
use serde::Serialize;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new().route("/library/{job_id}", get(library))
}

#[derive(Debug, Serialize)]
struct LibraryResponse {
    status: &'static str,
    url: Option<String>,
}

async fn library(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<LibraryResponse>, ApiError> {
    session(&state, &headers).await?;
    let id = Uuid::parse_str(&job_id)
        .map(JobId::new)
        .map_err(|_| ApiError::not_found())?;
    let job = state
        .jobs()
        .get(id)
        .await
        .map_err(|_| ApiError::not_found())?;
    if job.state != JobState::Ready {
        return Ok(Json(LibraryResponse {
            status: job.state.as_str(),
            url: None,
        }));
    }

    let client = JellyfinClient::from_config(state.config()).map_err(|_| ApiError::internal())?;
    let status = client
        .status_for_ready_job(&job.title, job.year.as_deref())
        .await
        .unwrap_or(crate::server::library::jellyfin::JellyfinLibraryStatus {
            status: JellyfinStatus::Unavailable,
            url: None,
        });
    let url = status.url;
    let status = match status.status {
        JellyfinStatus::Ready => "ready",
        JellyfinStatus::ScanPending => "scan_pending",
        JellyfinStatus::Unavailable => "unavailable",
    };
    Ok(Json(LibraryResponse { status, url }))
}
