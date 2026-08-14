pub mod assets;
pub mod auth;
pub mod catalog;
pub mod events;
pub mod health;
pub mod jobs;

use axum::Router;

use crate::server::state::AppState;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .merge(health::router())
        .merge(auth::router())
        .merge(catalog::router())
        .merge(jobs::router())
        .merge(events::router());

    // Task 8 extends this API surface with catalog, jobs, events, and library routes.
    Router::new()
        .nest("/api", api)
        .merge(assets::router())
        .with_state(state)
}

pub(crate) async fn session(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<crate::server::auth::AuthenticatedSession, crate::server::error::ApiError> {
    state
        .auth()
        .authenticate(
            state.pool(),
            headers.get("cookie").and_then(|v| v.to_str().ok()),
        )
        .await
}

pub(crate) async fn mutation_session(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<crate::server::auth::AuthenticatedSession, crate::server::error::ApiError> {
    state.auth().validate_origin(
        headers.get("origin").and_then(|v| v.to_str().ok()),
        headers.get("host").and_then(|v| v.to_str().ok()),
    )?;
    let session = session(state, headers).await?;
    state.auth().validate_csrf(
        headers.get("x-csrf-token").and_then(|v| v.to_str().ok()),
        &session.csrf_token,
    )?;
    Ok(session)
}
