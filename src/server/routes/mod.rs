pub mod auth;
pub mod health;

use axum::Router;

use crate::server::state::AppState;

pub fn router(state: AppState) -> Router {
    let api = Router::new().merge(health::router()).merge(auth::router());

    // Task 8 extends this API surface with catalog, jobs, events, and library routes.
    Router::new().nest("/api", api).with_state(state)
}
