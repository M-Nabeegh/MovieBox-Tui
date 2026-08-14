use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("server startup failed: {0}")]
    Startup(String),
}
