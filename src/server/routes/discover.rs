//! Browse and discovery endpoints.
//!
//! These power the browse view: rows of artwork to scan, rather than a search
//! box that demands you already know what you want.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use time::OffsetDateTime;

use super::session;
use crate::{
    catalog::tmdb::{DiscoverRow, MediaKind, RowQuery, TmdbClient, default_rows},
    server::{error::ApiError, state::AppState},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/discover", get(rows))
        .route("/discover/search", get(search))
        .route("/discover/filter", get(filter))
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
    page: Option<u32>,
}

/// A browse request narrowed by origin, year, or kind.
#[derive(Debug, Deserialize)]
struct FilterQuery {
    /// ISO 639-1 original language, e.g. `hi`.
    language: Option<String>,
    year: Option<u16>,
    /// `movie` or `series`; defaults to films.
    kind: Option<String>,
}

fn client(state: &AppState) -> Result<TmdbClient, ApiError> {
    let token = state.config().read_tmdb_token().unwrap_or(None);
    TmdbClient::new(token).ok_or_else(unavailable)
}

fn unavailable() -> ApiError {
    ApiError::new_status(
        StatusCode::SERVICE_UNAVAILABLE,
        "discovery_unavailable",
        "Browsing is not configured on this server.",
    )
}

/// The full browse view.
///
/// Rows are fetched together rather than one after another: each is a separate
/// round trip to a remote service, so waiting for them in sequence made the
/// page's load time the sum of every row instead of the slowest one.
///
/// A row that fails is dropped rather than failing the page, so one slow or
/// missing category cannot leave the user with a blank screen.
async fn rows(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    session(&state, &headers).await?;
    let client = client(&state)?;
    let year = OffsetDateTime::now_utc().year() as u16;

    let requests = default_rows(year)
        .into_iter()
        .map(|(id, title, query)| {
            let client = &client;
            async move { client.row(&id, &title, &query).await }
        })
        .collect::<Vec<_>>();

    let collected: Vec<DiscoverRow> = futures::future::join_all(requests)
        .await
        .into_iter()
        .filter_map(Result::ok)
        .filter(|row| !row.items.is_empty())
        .collect();

    if collected.is_empty() {
        return Err(unavailable());
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
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "The search query must be between 2 and 100 characters.",
        ));
    }
    let client = client(&state)?;
    let results = client
        .search(term, query.page.unwrap_or(1))
        .await
        .map_err(|_| unavailable())?;
    private_json(results)
}

/// Titles narrowed to one origin and, optionally, one year.
///
/// This is what a dedicated section is built from: "Hindi films from 2026" is a
/// filter, not a fixed row, so the year can change without the server needing
/// to know every combination in advance.
async fn filter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FilterQuery>,
) -> Result<Response, ApiError> {
    session(&state, &headers).await?;

    let kind = match query.kind.as_deref() {
        Some("series") => MediaKind::Series,
        None | Some("movie") => MediaKind::Movie,
        Some(_) => {
            return Err(ApiError::new_status(
                StatusCode::BAD_REQUEST,
                "invalid_kind",
                "The kind must be movie or series.",
            ));
        }
    };

    // A language code is two or three letters; anything else is a typo or an
    // attempt to smuggle something into the query string.
    let language = match query
        .language
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value)
            if (2..=3).contains(&value.len()) && value.chars().all(|c| c.is_ascii_alphabetic()) =>
        {
            Some(value.to_ascii_lowercase())
        }
        Some(_) => {
            return Err(ApiError::new_status(
                StatusCode::BAD_REQUEST,
                "invalid_language",
                "The language must be a two or three letter code.",
            ));
        }
        None => None,
    };

    if let Some(year) = query.year
        && !(1900..=2100).contains(&year)
    {
        return Err(ApiError::new_status(
            StatusCode::BAD_REQUEST,
            "invalid_year",
            "The year must be between 1900 and 2100.",
        ));
    }

    let client = client(&state)?;
    let label = match (&language, query.year) {
        (_, Some(year)) => format!("{year}"),
        _ => "All years".to_string(),
    };
    let row = client
        .row(
            "filter",
            &label,
            &RowQuery::Filtered {
                kind,
                language,
                year: query.year,
                region: None,
                genre: None,
            },
        )
        .await
        .map_err(|_| unavailable())?;

    private_json(row.items)
}

fn private_json<T: serde::Serialize>(value: T) -> Result<Response, ApiError> {
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(response)
}
