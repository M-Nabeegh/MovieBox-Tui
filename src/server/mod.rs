pub mod error;
pub mod library;
pub mod security;

pub async fn run() -> Result<(), error::ServerError> {
    Ok(())
}
