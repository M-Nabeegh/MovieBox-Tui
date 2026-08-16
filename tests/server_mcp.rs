#![cfg(feature = "server")]

//! MCP endpoint contract: protocol shape and, above all, the auth boundary.
//! These run without network access, so they cover everything up to the point a
//! tool would reach the catalog.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::CONTENT_TYPE},
};
use moviebox_tui::server::{config::ServerConfig, db::connect, routes, state::AppState};
use serde_json::{Value, json};
use std::{collections::HashMap, fs};
use tempfile::TempDir;
use tower::ServiceExt;

const TOKEN: &str = "fixture-mcp-token-value";

struct McpContext {
    _root: TempDir,
    app: axum::Router,
}

impl McpContext {
    async fn new(with_token: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let media = root.path().join("media");
        let partial = root.path().join("partial");
        let config_root = root.path().join("config");
        fs::create_dir_all(&media).unwrap();
        fs::create_dir_all(&partial).unwrap();
        fs::create_dir_all(&config_root).unwrap();
        let password = root.path().join("password");
        let session_key = root.path().join("session-key");
        let mcp_token = root.path().join("mcp-token");
        fs::write(&password, "fixture-secret\n").unwrap();
        fs::write(&session_key, "fixture-session-key-that-is-long-enough").unwrap();
        fs::write(&mcp_token, format!("{TOKEN}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&password, &session_key, &mcp_token] {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            }
        }

        let mut env: HashMap<String, String> = HashMap::from([
            ("MOVIEBOX_BIND".into(), "127.0.0.1:8420".into()),
            (
                "MOVIEBOX_DATABASE_URL".into(),
                format!("sqlite://{}", root.path().join("server.sqlite3").display()),
            ),
            ("MOVIEBOX_MEDIA_ROOT".into(), media.display().to_string()),
            (
                "MOVIEBOX_PARTIAL_ROOT".into(),
                partial.display().to_string(),
            ),
            (
                "MOVIEBOX_CONFIG_ROOT".into(),
                config_root.display().to_string(),
            ),
            (
                "MOVIEBOX_ADMIN_PASSWORD_FILE".into(),
                password.display().to_string(),
            ),
            (
                "MOVIEBOX_SESSION_KEY_FILE".into(),
                session_key.display().to_string(),
            ),
            ("MOVIEBOX_MAX_HEIGHT".into(), "1080".into()),
            ("MOVIEBOX_DOWNLOAD_CONCURRENCY".into(), "1".into()),
            ("MOVIEBOX_RESERVE_GIB".into(), "10".into()),
            (
                "MOVIEBOX_JELLYFIN_BASE_URL".into(),
                "http://127.0.0.1:8096".into(),
            ),
            ("MOVIEBOX_LOG_FORMAT".into(), "text".into()),
        ]);
        if with_token {
            env.insert(
                "MOVIEBOX_MCP_TOKEN_FILE".into(),
                mcp_token.display().to_string(),
            );
        }

        let config = ServerConfig::from_map(env).unwrap();
        let pool = connect(&config.database_url).await.unwrap();
        let state = AppState::bootstrap(config, pool).await.unwrap();
        Self {
            _root: root,
            app: routes::router(state),
        }
    }

    async fn call(&self, token: Option<&str>, payload: Value) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let response = self
            .app
            .clone()
            .oneshot(
                builder
                    .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
        (status, body)
    }
}

#[tokio::test]
async fn initialize_reports_tool_capability() {
    let context = McpContext::new(true).await;
    let (status, body) = context
        .call(
            Some(TOKEN),
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 1);
    assert!(body["result"]["capabilities"]["tools"].is_object());
    assert_eq!(body["result"]["serverInfo"]["name"], "moviebox");
}

#[tokio::test]
async fn tools_list_advertises_the_download_workflow() {
    let context = McpContext::new(true).await;
    let (_, body) = context
        .call(
            Some(TOKEN),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .await;

    let names = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    for expected in [
        "search_catalog",
        "list_sources",
        "start_download",
        "list_downloads",
        "get_download",
    ] {
        assert!(names.contains(&expected.to_string()), "missing {expected}");
    }
    // Every tool must carry a schema or an agent cannot call it correctly.
    for tool in body["result"]["tools"].as_array().unwrap() {
        assert!(
            tool["inputSchema"]["type"] == "object",
            "{tool} lacks a schema"
        );
        assert!(tool["description"].as_str().is_some_and(|d| !d.is_empty()));
    }
}

#[tokio::test]
async fn requests_without_a_token_are_rejected() {
    let context = McpContext::new(true).await;
    let (status, _) = context
        .call(None, json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn requests_with_a_wrong_token_are_rejected() {
    let context = McpContext::new(true).await;
    for wrong in [
        "",
        "wrong",
        "fixture-mcp-token-valu",
        "fixture-mcp-token-value-extra",
    ] {
        let (status, _) = context
            .call(
                Some(wrong),
                json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}),
            )
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "accepted token {wrong:?}");
    }
}

#[tokio::test]
async fn endpoint_is_absent_when_no_token_is_configured() {
    let context = McpContext::new(false).await;
    let (status, _) = context
        .call(
            Some(TOKEN),
            json!({"jsonrpc":"2.0","id":5,"method":"tools/list"}),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a disabled endpoint must not reveal that it exists"
    );
}

#[tokio::test]
async fn notifications_receive_no_response_body() {
    let context = McpContext::new(true).await;
    let (status, _) = context
        .call(
            Some(TOKEN),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn unknown_methods_report_a_protocol_error() {
    let context = McpContext::new(true).await;
    let (_, body) = context
        .call(
            Some(TOKEN),
            json!({"jsonrpc":"2.0","id":6,"method":"resources/list"}),
        )
        .await;
    assert_eq!(body["error"]["code"], -32601);
}

#[tokio::test]
async fn malformed_payloads_report_a_parse_error() {
    let context = McpContext::new(true).await;
    let response = context
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(CONTENT_TYPE, "application/json")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::from("{not json"))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice::<Value>(&bytes).unwrap();
    assert_eq!(body["error"]["code"], -32700);
}

#[tokio::test]
async fn unknown_tools_are_reported_as_tool_errors() {
    let context = McpContext::new(true).await;
    let (status, body) = context
        .call(
            Some(TOKEN),
            json!({
                "jsonrpc":"2.0","id":7,"method":"tools/call",
                "params":{"name":"delete_everything","arguments":{}}
            }),
        )
        .await;

    // Tool failures belong in the result so the agent can read them.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["isError"], true);
    assert!(
        body["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown tool")
    );
}

#[tokio::test]
async fn tool_arguments_are_validated_before_any_catalog_call() {
    let context = McpContext::new(true).await;
    for (arguments, expected) in [
        (json!({}), "missing required argument: query"),
        (json!({"query": "a"}), "between 2 and 100 characters"),
    ] {
        let (_, body) = context
            .call(
                Some(TOKEN),
                json!({
                    "jsonrpc":"2.0","id":8,"method":"tools/call",
                    "params":{"name":"search_catalog","arguments":arguments}
                }),
            )
            .await;
        assert_eq!(body["result"]["isError"], true);
        assert!(
            body["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(expected)
        );
    }
}
