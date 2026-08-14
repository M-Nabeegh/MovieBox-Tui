pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod events;
pub mod jobs;
pub mod library;
pub mod routes;
pub mod security;
pub mod state;

use tokio::net::TcpListener;

use crate::server::{config::ServerConfig, state::AppState};

pub async fn run() -> Result<(), error::ServerError> {
    let config = ServerConfig::from_env().map_err(|error| {
        error::ServerError::Startup(format!("configuration is invalid: {error}"))
    })?;
    let pool = db::connect(&config.database_url)
        .await
        .map_err(|_| error::ServerError::Startup("database initialization failed".to_string()))?;
    let state = AppState::bootstrap(config, pool).await?;
    let listener = TcpListener::bind(state.config().bind_addr)
        .await
        .map_err(|_| error::ServerError::Startup("server failed to bind".to_string()))?;
    axum::serve(listener, routes::router(state))
        .await
        .map_err(|_| error::ServerError::Startup("server exited unexpectedly".to_string()))
}
