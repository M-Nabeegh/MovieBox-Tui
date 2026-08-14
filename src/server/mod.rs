pub mod db;
pub mod error;
pub mod events;
pub mod jobs;
pub mod library;
pub mod security;

pub async fn run() -> Result<(), error::ServerError> {
    Ok(())
}
