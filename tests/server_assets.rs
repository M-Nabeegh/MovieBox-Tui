#![cfg(feature = "server")]

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use moviebox_tui::server::{config::ServerConfig, db::connect, routes, state::AppState};
use std::{collections::HashMap, fs};
use tempfile::TempDir;
use tower::ServiceExt;

async fn app() -> (TempDir, axum::Router) {
    let root = tempfile::tempdir().unwrap();
    let media = root.path().join("media");
    let partial = root.path().join("partial");
    let config = root.path().join("config");
    fs::create_dir_all(&media).unwrap();
    fs::create_dir_all(&partial).unwrap();
    fs::create_dir_all(&config).unwrap();
    let password = root.path().join("password");
    let key = root.path().join("session-key");
    fs::write(&password, "fixture-secret\n").unwrap();
    fs::write(&key, "fixture-session-key-that-is-long-enough").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&password, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let env: HashMap<String, String> = HashMap::from([
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
        ("MOVIEBOX_CONFIG_ROOT".into(), config.display().to_string()),
        (
            "MOVIEBOX_ADMIN_PASSWORD_FILE".into(),
            password.display().to_string(),
        ),
        (
            "MOVIEBOX_SESSION_KEY_FILE".into(),
            key.display().to_string(),
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
    let cfg = ServerConfig::from_map(env).unwrap();
    let pool = connect(&cfg.database_url).await.unwrap();
    let state = AppState::bootstrap(cfg, pool).await.unwrap();
    (root, routes::router(state))
}

async fn get(app: &axum::Router, path: &str) -> Response {
    app.clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn serves_spa_with_safe_headers_and_keeps_api_out_of_fallback() {
    let (_root, app) = app().await;
    let page = get(&app, "/search").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()["cache-control"], "no-store");
    assert!(
        page.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    let api = get(&app, "/api/not-a-route").await;
    assert_eq!(api.status(), StatusCode::NOT_FOUND);
}
