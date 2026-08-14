use super::session;
use crate::server::{error::ApiError, state::AppState};
use async_stream::stream;
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, HeaderValue, header::CACHE_CONTROL},
    response::IntoResponse,
    response::sse::{Event, Sse},
    routing::get,
};
use std::{convert::Infallible, time::Duration};

pub fn router() -> Router<AppState> {
    Router::new().route("/events", get(events))
}

async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    session(&state, &headers).await?;
    let after = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    let replay = sqlx::query("SELECT id, job_id, kind, state, downloaded_bytes, total_bytes, speed_bytes_per_second, error_code, error_message, warning FROM job_events WHERE id > ?1 ORDER BY id ASC LIMIT 100")
        .bind(after).fetch_all(state.pool()).await.map_err(|_| ApiError::internal())?;
    let mut receiver = state.events().subscribe();
    let stream = stream! {
        for row in replay {
            let id: i64 = sqlx::Row::get(&row, "id");
            let payload = serde_json::json!({"job_id": sqlx::Row::get::<String, _>(&row, "job_id"), "kind": sqlx::Row::get::<String, _>(&row, "kind"), "state": sqlx::Row::get::<String, _>(&row, "state"), "downloaded_bytes": sqlx::Row::get::<i64, _>(&row, "downloaded_bytes"), "total_bytes": sqlx::Row::get::<Option<i64>, _>(&row, "total_bytes")});
            yield Ok::<Event, Infallible>(Event::default().id(id.to_string()).event("job.updated").json_data(payload).unwrap());
        }
        loop {
            tokio::select! {
                item = receiver.recv() => if let Ok(item) = item { yield Ok(Event::default().event("job.updated").json_data(item).unwrap()); },
                _ = tokio::time::sleep(Duration::from_secs(15)) => yield Ok(Event::default().comment("heartbeat")),
            }
        }
    };
    let mut response = Sse::new(stream).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(response)
}
