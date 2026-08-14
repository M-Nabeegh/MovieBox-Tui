use super::session;
use crate::{
    catalog::{CatalogId, EpisodeRequest, SourceId},
    server::{error::ApiError, state::AppState},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/catalog/search", get(search))
        .route("/catalog/items/{provider}/{id}", get(details))
        .route("/catalog/items/{provider}/{id}/sources", get(sources))
        .route("/catalog/items/{provider}/{id}/subtitles", get(subtitles))
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
    page: Option<u32>,
}
#[derive(Debug, Deserialize)]
struct EpisodeQuery {
    season: Option<u16>,
    episode: Option<u16>,
}
#[derive(Debug, Deserialize)]
struct SubtitleQuery {
    source_id: String,
}

async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Result<Response, ApiError> {
    session(&state, &headers).await?;
    let q = query.q.trim();
    let page = query.page.unwrap_or(1);
    if !(2..=100).contains(&q.len()) {
        return Err(ApiError::new_status(
            axum::http::StatusCode::BAD_REQUEST,
            "invalid_query",
            "The search query must be between 2 and 100 characters.",
        ));
    }
    if !(1..=50).contains(&page) {
        return Err(ApiError::new_status(
            axum::http::StatusCode::BAD_REQUEST,
            "invalid_page",
            "The page must be between 1 and 50.",
        ));
    }
    let body = state
        .catalog()
        .search(q, page)
        .await
        .map_err(map_catalog_error)?;
    json_private(body)
}

async fn details(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((_provider, id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    session(&state, &headers).await?;
    let body = state
        .catalog()
        .details(&CatalogId::new(id))
        .await
        .map_err(map_catalog_error)?;
    json_private(body)
}

async fn sources(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((_provider, id)): Path<(String, String)>,
    Query(query): Query<EpisodeQuery>,
) -> Result<Response, ApiError> {
    session(&state, &headers).await?;
    if query.season.is_some() != query.episode.is_some() {
        return Err(ApiError::invalid_request());
    }
    let body = state
        .catalog()
        .sources(EpisodeRequest {
            catalog_id: CatalogId::new(id),
            season: query.season,
            episode: query.episode,
        })
        .await
        .map_err(map_catalog_error)?;
    json_private(body)
}

async fn subtitles(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((_provider, _id)): Path<(String, String)>,
    Query(query): Query<SubtitleQuery>,
) -> Result<Response, ApiError> {
    session(&state, &headers).await?;
    let body = state
        .catalog()
        .subtitles(&SourceId::new(query.source_id))
        .await
        .map_err(map_catalog_error)?;
    json_private(body)
}

fn json_private<T: serde::Serialize>(body: T) -> Result<Response, ApiError> {
    let mut response = Json(body).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(response)
}

fn map_catalog_error(error: crate::catalog::CatalogError) -> ApiError {
    use crate::catalog::CatalogError;
    match error {
        CatalogError::QualityUnavailable {
            requested_height: Some(height),
            ..
        } if height > 1080 => ApiError::quality_exceeds_limit(),
        CatalogError::QualityUnavailable { .. } => ApiError::quality_unavailable(),
        CatalogError::NotFound(_) => ApiError::not_found(),
        CatalogError::Provider(_) => ApiError::provider_unavailable(),
        CatalogError::InvalidOpaqueId | CatalogError::OpaqueIdVerificationFailed => {
            ApiError::new_status(
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_source",
                "The selected catalog source is invalid.",
            )
        }
        _ => ApiError::invalid_request(),
    }
}
