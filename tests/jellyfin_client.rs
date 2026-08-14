#![cfg(feature = "server")]

use moviebox_tui::server::library::jellyfin::{JellyfinClient, JellyfinItem, JellyfinStatus};
use reqwest::StatusCode;
use serde_json::json;
use std::time::Duration;
use wiremock::{Mock, MockServer, Respond, ResponseTemplate, matchers};

struct JsonResponse(serde_json::Value);

impl Respond for JsonResponse {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(&self.0)
    }
}

fn client(server: &MockServer) -> JellyfinClient {
    JellyfinClient::with_polling(
        server.uri().parse().unwrap(),
        Some("fixture-api-key".to_string()),
        Duration::ZERO,
        3,
    )
}

#[tokio::test]
async fn refresh_attaches_key_without_leaking_it_in_errors() {
    let server = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/Library/Refresh"))
        .and(matchers::header("X-Emby-Token", "fixture-api-key"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    client(&server).refresh_library().await.unwrap();
}

#[tokio::test]
async fn find_item_polls_until_the_item_is_available() {
    let server = MockServer::start().await;
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/Items"))
        .respond_with(JsonResponse(json!({
            "Items": [{"Id": "jellyfin-item-1", "Name": "Fixture Movie", "ProductionYear": 2024, "Type": "Movie"}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let item = client(&server)
        .find_item("Fixture Movie", Some("2024"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        item,
        JellyfinItem {
            id: "jellyfin-item-1".into()
        }
    );
}

#[tokio::test]
async fn invalid_json_is_a_safe_error() {
    let server = MockServer::start().await;
    Mock::given(matchers::method("GET"))
        .and(matchers::path("/Items"))
        .respond_with(ResponseTemplate::new(StatusCode::OK).set_body_string("not-json"))
        .mount(&server)
        .await;

    let error = client(&server)
        .find_item("Fixture Movie", None)
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("fixture-api-key"));
}

#[tokio::test]
async fn missing_key_reports_scan_pending_without_a_request() {
    let server = MockServer::start().await;
    let response =
        JellyfinClient::with_polling(server.uri().parse().unwrap(), None, Duration::ZERO, 1)
            .status_for_ready_job("Fixture Movie", Some("2024"))
            .await
            .unwrap();
    assert_eq!(response.status, JellyfinStatus::ScanPending);
    assert_eq!(response.url, None);
}
