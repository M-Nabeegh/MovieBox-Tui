//! Browse and discovery endpoints.
//!
//! These power the browse view: rows of artwork to scan, rather than a search
//! box that demands you already know what you want.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use time::OffsetDateTime;

use super::session;
use crate::{
    catalog::tmdb::{DiscoverRow, RowQuery, TmdbClient, default_rows},
    server::{error::ApiError, state::AppState},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/discover", get(rows))
        .route("/discover/search", get(search))
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
    page: Option<u32>,
}

fn client(state: &AppState) -> Result<TmdbClient, ApiError> {
    let token = state.config().read_tmdb_token().unwrap_or(None);
    TmdbClient::new(token).ok_or_else(|| {
        ApiError::new_status(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "discovery_unavailable",
            "Browsing is not configured on this server.",
        )
    })
}

/// The full browse view: several rows fetched together.
///
/// A row that fails is dropped rather than failing the page, so one slow or
/// missing category cannot leave the user with a blank screen.
async fn rows(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    session(&state, &headers).await?;
    let client = client(&state)?;
    let year = OffsetDateTime::now_utc().year() as u16;

    let mut collected: Vec<DiscoverRow> = Vec::new();
    for (id, title, query) in default_rows(year) {
        if let Ok(row) = client.row(&id, &title, &query).await
            && !row.items.is_empty()
        {
            collected.push(row);
        }
    }

    if collected.is_empty() {
        return Err(ApiError::new_status(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "discovery_unavailable",
            "Browsing is temporarily unavailable.",
        ));
    }

    private_json(collected)
}

/// Free-text search across the metadata catalogue.
async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Result<Response, ApiError> {
    session(&state, &headers).await?;
    let term = query.q.trim();
    if !(2..=100).contains(&term.len()) {
        return Err(ApiError::new_status(
            axum::http::StatusCode::BAD_REQUEST,
            "invalid_query",
            "The search query must be between 2 and 100 characters.",
        ));
    }
    let client = client(&state)?;
    let results = client
        .search(term, query.page.unwrap_or(1))
        .await
        .map_err(|_| {
            ApiError::new_status(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "discovery_unavailable",
                "Search is temporarily unavailable.",
            )
        })?;
    private_json(results)
}

fn private_json<T: serde::Serialize>(value: T) -> Result<Response, ApiError> {
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(response)
}

/// Keep the unused import warning honest about what a row is.
#[allow(dead_code)]
fn _row_query_is_used(_: &RowQuery) {}
