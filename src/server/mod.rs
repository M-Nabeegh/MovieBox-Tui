pub mod error;
pub mod security;

pub async fn run() -> Result<(), error::ServerError> {
    Ok(())
}
