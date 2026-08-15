#![cfg(feature = "server")]

use axum::{
    body::{Body, to_bytes},
    http::{
        Request, StatusCode,
        header::{CONTENT_TYPE, HOST, ORIGIN, SET_COOKIE},
    },
};
use moviebox_tui::{
    catalog::{CatalogId, MediaType, SourceId},
    server::{
        config::ServerConfig,
        db::connect,
        jobs::{JobRepository, JobState, NewJob},
        routes,
        state::AppState,
    },
};
use serde_json::Value;
use serde_json::json;
use std::{collections::HashMap, fs};
use tempfile::TempDir;
use tower::ServiceExt;

struct TestContext {
    _root: TempDir,
    app: axum::Router,
    jobs: JobRepository,
}

impl TestContext {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let media = root.path().join("media");
        let partial = root.path().join("partial");
        let config_root = root.path().join("config");
        fs::create_dir_all(&media).unwrap();
        fs::create_dir_all(&partial).unwrap();
        fs::create_dir_all(&config_root).unwrap();
        let password = root.path().join("password");
        let session_key = root.path().join("session-key");
        fs::write(&password, "fixture-secret\n").unwrap();
        fs::write(&session_key, "fixture-session-key-that-is-long-enough").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&password, fs::Permissions::from_mode(0o600)).unwrap();
            fs::set_permissions(&session_key, fs::Permissions::from_mode(0o600)).unwrap();
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
        let config = ServerConfig::from_map(env).unwrap();
        let pool = connect(&config.database_url).await.unwrap();
        let state = AppState::bootstrap(config, pool).await.unwrap();
        let jobs = state.jobs().clone();
        Self {
            _root: root,
            app: routes::router(state),
            jobs,
        }
    }

    async fn login(&self) -> (String, String) {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(HOST, "moviebox.example.ts.net")
                    .header(ORIGIN, "https://moviebox.example.ts.net")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"password":"fixture-secret"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let cookie = response.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .split_once('=')
            .unwrap()
            .1
            .to_string();
        let session = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/auth/session")
                    .header(HOST, "moviebox.example.ts.net")
                    .header("cookie", format!("moviebox_session={cookie}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(session.into_body(), usize::MAX).await.unwrap();
        let csrf = serde_json::from_slice::<Value>(&bytes).unwrap()["csrf_token"]
            .as_str()
            .unwrap()
            .to_string();
        (cookie, csrf)
    }
}

#[tokio::test]
async fn server_bootstrap_starts_worker_for_queued_jobs() {
    let context = TestContext::new().await;
    let job = context
        .jobs
        .create(NewJob {
            catalog_id: CatalogId::new("invalid-catalog".to_string()),
            source_id: SourceId::new("invalid-source".to_string()),
            subtitle_id: None,
            title: "Worker smoke test".to_string(),
            year: Some("2026".to_string()),
            media_type: MediaType::Movie,
            season_number: None,
            episode_number: None,
            episode_title: None,
            requested_height: 1080,
            final_video_path: "Movies/Worker smoke test (2026)/Worker smoke test (2026).mkv"
                .to_string(),
            final_subtitle_path: None,
            partial_video_path: "_moviebox/jobs/not-the-job/Worker smoke test.mkv.part".to_string(),
            partial_subtitle_path: None,
        })
        .await
        .unwrap();

    for _ in 0..40 {
        let current = context.jobs.get(job.id).await.unwrap();
        match current.state {
            JobState::Queued | JobState::Resolving => {}
            JobState::Failed => {
                assert_eq!(current.error_code.as_deref(), Some("unsafe_path"));
                return;
            }
            state => panic!("worker reached unexpected state: {state:?}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    panic!("queued job was not claimed by the server worker");
}

#[tokio::test]
async fn catalog_search_requires_authentication() {
    let context = TestContext::new().await;
    let response = context
        .app
        .oneshot(
            Request::builder()
                .uri("/api/catalog/search?q=ab&page=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn catalog_search_rejects_short_queries_and_out_of_range_pages() {
    let context = TestContext::new().await;
    let (cookie, _) = context.login().await;
    for uri in [
        "/api/catalog/search?q=a&page=1",
        "/api/catalog/search?q=ab&page=51",
    ] {
        let response = context
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(HOST, "moviebox.example.ts.net")
                    .header("cookie", format!("moviebox_session={cookie}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn forged_4k_job_is_rejected_before_catalog_resolution() {
    let context = TestContext::new().await;
    let (cookie, csrf) = context.login().await;
    let response = context.app.clone().oneshot(Request::builder().method("POST").uri("/api/jobs")
        .header(HOST, "moviebox.example.ts.net").header(ORIGIN, "https://moviebox.example.ts.net")
        .header("cookie", format!("moviebox_session={cookie}")).header("x-csrf-token", csrf)
        .header(CONTENT_TYPE, "application/json").body(Body::from(serde_json::to_vec(&json!({"catalog_id":"fixture","source_id":"signed-2160-source","requested_height":2160})).unwrap())).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["error"]["code"],
        "quality_exceeds_limit"
    );
}

#[allow(dead_code)]
async fn json_body(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
