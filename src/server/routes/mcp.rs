//! Model Context Protocol endpoint.
//!
//! Lets an agent search the catalog and queue downloads on the user's behalf —
//! "download this movie" ends with the file in the media library, ready to watch.
//!
//! Transport is JSON-RPC 2.0 over a single HTTP POST, authenticated with a bearer
//! token read from `MOVIEBOX_MCP_TOKEN_FILE`. With no token configured the
//! endpoint stays off and answers as if it does not exist, so enabling agent
//! control is always a deliberate act.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::jobs::{CreateRequest, enqueue_download};
use crate::{
    catalog::{CatalogId, EpisodeRequest},
    server::{error::ApiError, state::AppState},
};

/// Protocol revision this endpoint implements.
const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "moviebox";

pub fn router() -> Router<AppState> {
    Router::new().route("/mcp", post(handle))
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

async fn handle(State(state): State<AppState>, headers: HeaderMap, body: String) -> Response {
    let Some(token) = configured_token(&state) else {
        // Disabled endpoints should be indistinguishable from absent ones.
        return ApiError::not_found().into_response();
    };
    if !presented_token_matches(&headers, &token) {
        return unauthorized();
    }

    let Ok(request) = serde_json::from_str::<RpcRequest>(&body) else {
        return rpc_error(Value::Null, -32700, "parse error");
    };

    // A JSON-RPC notification carries no id and must not be answered.
    let Some(id) = request.id.clone() else {
        return StatusCode::ACCEPTED.into_response();
    };

    match request.method.as_str() {
        "initialize" => rpc_result(id, initialize_result()),
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({ "tools": tool_definitions() })),
        "tools/call" => match call_tool(&state, &request.params).await {
            Ok(text) => rpc_result(id, tool_text(text, false)),
            // Tool failures are reported inside the result so the agent can read
            // and react to them, per the MCP tool-error convention.
            Err(message) => rpc_result(id, tool_text(message, true)),
        },
        _ => rpc_error(id, -32601, "method not found"),
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Search the catalog, then start a download. Completed downloads \
    appear in the media library automatically; use get_download to follow progress."
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "search_catalog",
            "description": "Search for movies and series by title. Returns catalog ids to pass to other tools.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Title to search for." },
                    "page": { "type": "integer", "description": "Result page, 1-50. Defaults to 1." }
                },
                "required": ["query"]
            }
        },
        {
            "name": "list_sources",
            "description": "List available qualities for a catalog item. Use only when the caller wants to choose a specific quality; start_download picks the best one on its own.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "catalog_id": { "type": "string" },
                    "season": { "type": "integer", "description": "Series only." },
                    "episode": { "type": "integer", "description": "Series only." }
                },
                "required": ["catalog_id"]
            }
        },
        {
            "name": "start_download",
            "description": "Queue a download. Picks the highest allowed quality and attaches subtitles automatically unless a source is named. Returns a job id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "catalog_id": { "type": "string" },
                    "source_id": { "type": "string", "description": "Optional. Omit to use the best available quality." },
                    "season": { "type": "integer", "description": "Series only." },
                    "episode": { "type": "integer", "description": "Series only." }
                },
                "required": ["catalog_id"]
            }
        },
        {
            "name": "list_downloads",
            "description": "List recent downloads with their state and progress.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "1-50, defaults to 20." }
                }
            }
        },
        {
            "name": "get_download",
            "description": "Get the state and progress of one download by job id.",
            "inputSchema": {
                "type": "object",
                "properties": { "job_id": { "type": "string" } },
                "required": ["job_id"]
            }
        }
    ])
}

async fn call_tool(state: &AppState, params: &Value) -> Result<String, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("missing tool name")?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    match name {
        "search_catalog" => search_catalog(state, &arguments).await,
        "list_sources" => list_sources(state, &arguments).await,
        "start_download" => start_download(state, &arguments).await,
        "list_downloads" => list_downloads(state, &arguments).await,
        "get_download" => get_download(state, &arguments).await,
        other => Err(format!("unknown tool: {other}")),
    }
}

async fn search_catalog(state: &AppState, arguments: &Value) -> Result<String, String> {
    let query = string_argument(arguments, "query")?;
    let query = query.trim();
    if !(2..=100).contains(&query.len()) {
        return Err("query must be between 2 and 100 characters".to_string());
    }
    let page = arguments
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 50) as u32;

    let results = state
        .catalog()
        .search(query, page)
        .await
        .map_err(|_| "catalog search failed".to_string())?;

    if results.items.is_empty() {
        return Ok(format!("No results for \"{query}\"."));
    }
    let items = results
        .items
        .iter()
        .map(|item| {
            json!({
                "catalog_id": item.id.as_str(),
                "title": item.title,
                "year": item.year,
                "media_type": item.media_type,
                "season_count": item.season_count,
            })
        })
        .collect::<Vec<_>>();
    Ok(to_pretty(
        &json!({ "page": results.page, "has_more": results.has_more, "results": items }),
    ))
}

async fn list_sources(state: &AppState, arguments: &Value) -> Result<String, String> {
    let catalog_id = CatalogId::new(string_argument(arguments, "catalog_id")?);
    let sources = state
        .catalog()
        .sources(EpisodeRequest {
            catalog_id,
            season: optional_u16(arguments, "season"),
            episode: optional_u16(arguments, "episode"),
        })
        .await
        .map_err(|_| "no sources are available for that item".to_string())?;

    let listed = sources
        .iter()
        .map(|source| {
            json!({
                "source_id": source.id.as_str(),
                "height": source.height,
                "label": source.label,
                "size_bytes": source.size_bytes,
                "language": source.language,
                "recommended": source.recommended,
            })
        })
        .collect::<Vec<_>>();
    Ok(to_pretty(&json!({ "sources": listed })))
}

async fn start_download(state: &AppState, arguments: &Value) -> Result<String, String> {
    let catalog_id = string_argument(arguments, "catalog_id")?;
    let season = optional_u16(arguments, "season");
    let episode = optional_u16(arguments, "episode");

    let sources = state
        .catalog()
        .sources(EpisodeRequest {
            catalog_id: CatalogId::new(catalog_id.clone()),
            season,
            episode,
        })
        .await
        .map_err(|_| "no sources are available for that item".to_string())?;

    // An agent should not have to reason about quality tiers, so pick the best
    // source within the configured ceiling when none was named.
    let ceiling = state.config().maximum_height;
    let selected = match arguments.get("source_id").and_then(Value::as_str) {
        Some(requested) => sources
            .iter()
            .find(|source| source.id.as_str() == requested)
            .ok_or("that source id is not available for this item")?,
        // Ranked on resolution *and* codec-adjusted bitrate, so an agent asking
        // for "the best" does not land on a starved encode that merely claims a
        // high resolution.
        None => crate::catalog::quality::best_within(&sources, ceiling)
            .ok_or_else(|| format!("no source is available at or below {ceiling}p"))?,
    };
    if selected.height > ceiling {
        return Err(format!("that source exceeds the {ceiling}p limit"));
    }

    let job = enqueue_download(
        state,
        CreateRequest {
            catalog_id,
            source_id: selected.id.as_str().to_string(),
            subtitle_id: None,
            requested_height: selected.height,
            season,
            episode,
        },
    )
    .await
    .map_err(|error| describe_api_error(&error))?;

    Ok(to_pretty(&json!({
        "job_id": job.id.to_string(),
        "title": job.title,
        "year": job.year,
        "quality": format!("{}p", job.requested_height),
        "state": job.state.as_str(),
        "note": "Queued. Follow it with get_download; it appears in the media library when ready.",
    })))
}

async fn list_downloads(state: &AppState, arguments: &Value) -> Result<String, String> {
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 50) as u32;
    let jobs = state
        .jobs()
        .list(limit, None)
        .await
        .map_err(|_| "could not read the download queue".to_string())?;
    let listed = jobs.iter().map(summarize_job).collect::<Vec<_>>();
    Ok(to_pretty(&json!({ "downloads": listed })))
}

async fn get_download(state: &AppState, arguments: &Value) -> Result<String, String> {
    let raw = string_argument(arguments, "job_id")?;
    let id = uuid::Uuid::parse_str(&raw).map_err(|_| "that job id is not valid".to_string())?;
    let job = state
        .jobs()
        .get(crate::server::jobs::JobId::new(id))
        .await
        .map_err(|_| "no download has that job id".to_string())?;
    Ok(to_pretty(&summarize_job(&job)))
}

fn summarize_job(job: &crate::server::jobs::DownloadJob) -> Value {
    let percent = job.total_bytes.filter(|total| *total > 0).map(|total| {
        ((job.downloaded_bytes as f64 / total as f64) * 100.0)
            .clamp(0.0, 100.0)
            .round() as u64
    });
    json!({
        "job_id": job.id.to_string(),
        "title": job.title,
        "year": job.year,
        "state": job.state.as_str(),
        "percent_complete": percent,
        "downloaded_bytes": job.downloaded_bytes,
        "total_bytes": job.total_bytes,
        "speed_bytes_per_second": job.speed_bytes_per_second,
        "attempt": job.attempt,
        "error_code": job.error_code,
        "warning": job.warning,
    })
}

/// Map an API error to a message an agent can act on, without leaking internals.
fn describe_api_error(error: &ApiError) -> String {
    match error.code() {
        "quality_exceeds_limit" | "quality_unavailable" => {
            "no source is available within the configured quality limit".to_string()
        }
        "invalid_source" => "that source id is not available for this item".to_string(),
        "invalid_subtitle" => "the requested subtitle is unavailable".to_string(),
        "not_found" => "that catalog item does not exist".to_string(),
        "invalid_request" => {
            "the request is missing a season or episode, or they are not valid".to_string()
        }
        _ => "the download could not be queued".to_string(),
    }
}

fn string_argument(arguments: &Value, key: &str) -> Result<String, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing required argument: {key}"))
}

fn optional_u16(arguments: &Value, key: &str) -> Option<u16> {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
}

fn configured_token(state: &AppState) -> Option<String> {
    state.config().read_mcp_token().ok().flatten()
}

/// Compare the presented bearer token against the configured one.
///
/// Both sides are hashed first so the comparison runs over fixed-length digests
/// and cannot reveal the secret through timing or length.
fn presented_token_matches(headers: &HeaderMap, expected: &str) -> bool {
    let Some(presented) = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
    else {
        return false;
    };
    Sha256::digest(presented.as_bytes()) == Sha256::digest(expected.as_bytes())
}

fn tool_text(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn to_pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn rpc_result(id: Value, result: Value) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn rpc_error(id: Value, code: i32, message: &str) -> Response {
    Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    }))
    .into_response()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("www-authenticate", "Bearer")],
        Json(json!({
            "jsonrpc": "2.0",
            "id": Value::Null,
            "error": { "code": -32001, "message": "unauthorized" }
        })),
    )
        .into_response()
}
