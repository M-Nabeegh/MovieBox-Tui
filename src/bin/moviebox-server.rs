#[tokio::main]
async fn main() -> Result<(), moviebox_tui::server::error::ServerError> {
    moviebox_tui::server::run().await
}
