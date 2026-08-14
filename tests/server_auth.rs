#![cfg(feature = "server")]

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use axum::{
    body::{Body, to_bytes},
    http::{
        Request, Response, StatusCode,
        header::{CONTENT_TYPE, HOST, ORIGIN, SET_COOKIE},
    },
};
use moviebox_tui::server::{config::ServerConfig, db::connect, routes, state::AppState};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tempfile::TempDir;
use time::OffsetDateTime;
use tower::ServiceExt;

#[tokio::test]
async fn successful_login_sets_strict_secure_cookie_and_supports_session_round_trip() {
    let context = TestContext::new("fixture-secret").await;

    let login = context
        .post_json(
            "/api/auth/login",
            json!({
                "password": "fixture-secret"
            }),
        )
        .await;

    assert_eq!(login.status(), StatusCode::NO_CONTENT);
    let set_cookie = header_value(&login, SET_COOKIE);
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("Secure"));
    assert!(set_cookie.contains("SameSite=Strict"));
    assert!(set_cookie.contains("Path=/"));

    let session_cookie = cookie_value(&set_cookie);
    let session = context
        .get_with_cookie("/api/auth/session", &session_cookie)
        .await;
    assert_eq!(session.status(), StatusCode::OK);

    let payload = json_body(session).await;
    assert_eq!(payload["username"], "admin");
    assert_eq!(payload["authenticated"], true);
    let csrf_token = payload["csrf_token"].as_str().unwrap();
    assert!(!csrf_token.is_empty());

    let logout = context
        .post_logout(&session_cookie, csrf_token, context.origin())
        .await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert!(header_value(&logout, SET_COOKIE).contains("Max-Age=0"));

    let after_logout = context
        .get_with_cookie("/api/auth/session", &session_cookie)
        .await;
    assert_eq!(after_logout.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bootstrap_does_not_replace_the_existing_admin_password() {
    let context = TestContext::new("first-secret").await;
    let original_config = ServerConfig::from_map(context.env_map()).unwrap();
    let original_pool = connect(&original_config.database_url).await.unwrap();
    let _state = AppState::bootstrap(original_config, original_pool)
        .await
        .unwrap();

    fs::write(context.password_file(), "second-secret\n").unwrap();

    let second_config = ServerConfig::from_map(context.env_map()).unwrap();
    let second_pool = connect(&second_config.database_url).await.unwrap();
    let second_state = AppState::bootstrap(second_config, second_pool.clone())
        .await
        .unwrap();
    let app = routes::router(second_state);

    let first_login = app
        .clone()
        .oneshot(login_request(context.origin(), "first-secret", None))
        .await
        .unwrap();
    assert_eq!(first_login.status(), StatusCode::NO_CONTENT);

    let second_login = app
        .oneshot(login_request(context.origin(), "second-secret", None))
        .await
        .unwrap();
    assert_eq!(second_login.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn repeated_failed_logins_from_the_same_source_are_throttled() {
    let context = TestContext::new("fixture-secret").await;

    for _ in 0..5 {
        let response = context
            .post_json_with_forwarded_for(
                "/api/auth/login",
                json!({
                    "password": "wrong-secret"
                }),
                "100.64.0.10",
            )
            .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let throttled = context
        .post_json_with_forwarded_for(
            "/api/auth/login",
            json!({
                "password": "wrong-secret"
            }),
            "100.64.0.10",
        )
        .await;

    assert_eq!(throttled.status(), StatusCode::TOO_MANY_REQUESTS);
    let payload = json_body(throttled).await;
    assert_eq!(payload["error"]["code"], "too_many_attempts");
}

#[tokio::test]
async fn logout_rejects_mismatched_origin_and_missing_csrf_token() {
    let context = TestContext::new("fixture-secret").await;

    let login = context
        .post_json(
            "/api/auth/login",
            json!({
                "password": "fixture-secret"
            }),
        )
        .await;
    let session_cookie = cookie_value(&header_value(&login, SET_COOKIE));

    let missing_csrf = context
        .post_logout(&session_cookie, "", context.origin())
        .await;
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(missing_csrf).await["error"]["code"],
        "csrf_mismatch"
    );

    let wrong_origin = context
        .post_logout(&session_cookie, "bogus-token", "https://evil.example")
        .await;
    assert_eq!(wrong_origin.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(wrong_origin).await["error"]["code"],
        "invalid_origin"
    );
}

#[tokio::test]
async fn expired_sessions_are_rejected_and_do_not_leak_internal_details() {
    let context = TestContext::new("fixture-secret").await;

    let login = context
        .post_json(
            "/api/auth/login",
            json!({
                "password": "fixture-secret"
            }),
        )
        .await;
    let session_cookie = cookie_value(&header_value(&login, SET_COOKIE));
    let session_id = session_id(&session_cookie);

    sqlx::query("UPDATE sessions SET expires_at = ?1, absolute_expires_at = ?2 WHERE id = ?3")
        .bind(OffsetDateTime::now_utc().unix_timestamp() - 10)
        .bind(OffsetDateTime::now_utc().unix_timestamp() - 10)
        .bind(session_id)
        .execute(context.pool())
        .await
        .unwrap();

    let response = context
        .get_with_cookie("/api/auth/session", &session_cookie)
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let payload = json_body(response).await;
    assert_eq!(payload["error"]["code"], "authentication_required");
    assert!(!payload.to_string().contains("sqlite"));
    assert!(!payload.to_string().contains("fixture-secret"));
}

#[tokio::test]
async fn configuration_validation_rejects_invalid_bind_paths_and_secret_permissions() {
    let context = TestContext::new("fixture-secret").await;

    let mut invalid_bind = context.env_map();
    invalid_bind.insert("MOVIEBOX_BIND".to_string(), "not-a-socket".to_string());
    let error = ServerConfig::from_map(invalid_bind).unwrap_err();
    assert!(error.to_string().contains("MOVIEBOX_BIND"));

    let mut invalid_root = context.env_map();
    invalid_root.insert("MOVIEBOX_MEDIA_ROOT".to_string(), "/".to_string());
    let error = ServerConfig::from_map(invalid_root).unwrap_err();
    assert!(error.to_string().contains("MOVIEBOX_MEDIA_ROOT"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(
            context.session_key_file(),
            fs::Permissions::from_mode(0o666),
        )
        .unwrap();

        let error = ServerConfig::from_map(context.env_map()).unwrap_err();
        assert!(error.to_string().contains("MOVIEBOX_SESSION_KEY_FILE"));
    }
}

#[tokio::test]
async fn health_route_is_public_and_error_responses_use_the_safe_envelope() {
    let context = TestContext::new("fixture-secret").await;

    let health = context.get("/api/health").await;
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(json_body(health).await["status"], "ok");

    let invalid_origin = context
        .post_json_with_origin(
            "/api/auth/login",
            json!({
                "password": "fixture-secret"
            }),
            "https://wrong.example",
        )
        .await;
    assert_eq!(invalid_origin.status(), StatusCode::FORBIDDEN);

    let payload = json_body(invalid_origin).await;
    assert_eq!(payload["error"]["code"], "invalid_origin");
    assert!(payload["error"]["message"].as_str().unwrap().len() > 4);
    assert!(payload["error"]["request_id"].as_str().unwrap().len() > 8);
    assert_eq!(payload["error"]["fields"], json!({}));
    assert!(!payload.to_string().contains("fixture-secret"));
    assert!(
        !payload
            .to_string()
            .contains(&context.root().display().to_string())
    );
}

struct TestContext {
    root: TempDir,
    pool: SqlitePool,
    app: axum::Router,
    env_map: HashMap<String, String>,
    origin: String,
    password_file: PathBuf,
    session_key_file: PathBuf,
}

impl TestContext {
    async fn new(password: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let media_root = root.path().join("media");
        let partial_root = root.path().join("partials");
        let config_root = root.path().join("config");
        fs::create_dir_all(&media_root).unwrap();
        fs::create_dir_all(&partial_root).unwrap();
        fs::create_dir_all(&config_root).unwrap();

        let password_file = root.path().join("admin-password.txt");
        let session_key_file = root.path().join("session-key.txt");
        fs::write(&password_file, format!("{password}\n")).unwrap();
        fs::write(
            &session_key_file,
            "this-is-a-fixture-session-key-that-is-long-enough",
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&password_file, fs::Permissions::from_mode(0o600)).unwrap();
            fs::set_permissions(&session_key_file, fs::Permissions::from_mode(0o600)).unwrap();
        }

        let database_url = format!("sqlite://{}", root.path().join("server.sqlite3").display());
        let origin = "https://moviebox.example.ts.net".to_string();
        let env_map = HashMap::from([
            ("MOVIEBOX_BIND".to_string(), "127.0.0.1:8420".to_string()),
            ("MOVIEBOX_DATABASE_URL".to_string(), database_url),
            (
                "MOVIEBOX_MEDIA_ROOT".to_string(),
                media_root.display().to_string(),
            ),
            (
                "MOVIEBOX_PARTIAL_ROOT".to_string(),
                partial_root.display().to_string(),
            ),
            (
                "MOVIEBOX_CONFIG_ROOT".to_string(),
                config_root.display().to_string(),
            ),
            (
                "MOVIEBOX_ADMIN_PASSWORD_FILE".to_string(),
                password_file.display().to_string(),
            ),
            (
                "MOVIEBOX_SESSION_KEY_FILE".to_string(),
                session_key_file.display().to_string(),
            ),
            ("MOVIEBOX_MAX_HEIGHT".to_string(), "1080".to_string()),
            ("MOVIEBOX_DOWNLOAD_CONCURRENCY".to_string(), "1".to_string()),
            ("MOVIEBOX_RESERVE_GIB".to_string(), "10".to_string()),
            (
                "MOVIEBOX_JELLYFIN_BASE_URL".to_string(),
                "http://127.0.0.1:8096".to_string(),
            ),
            ("MOVIEBOX_LOG_FORMAT".to_string(), "text".to_string()),
        ]);

        let config = ServerConfig::from_map(env_map.clone()).unwrap();
        let pool = connect(&config.database_url).await.unwrap();
        let state = AppState::bootstrap(config, pool.clone()).await.unwrap();
        let app = routes::router(state);

        Self {
            root,
            pool,
            app,
            env_map,
            origin,
            password_file,
            session_key_file,
        }
    }

    fn env_map(&self) -> HashMap<String, String> {
        self.env_map.clone()
    }

    fn origin(&self) -> &str {
        &self.origin
    }

    fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    fn password_file(&self) -> &Path {
        &self.password_file
    }

    fn session_key_file(&self) -> &Path {
        &self.session_key_file
    }

    fn root(&self) -> &Path {
        self.root.path()
    }

    async fn get(&self, uri: &str) -> Response<Body> {
        self.app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .header(HOST, "moviebox.example.ts.net")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn get_with_cookie(&self, uri: &str, cookie: &str) -> Response<Body> {
        self.app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .header(HOST, "moviebox.example.ts.net")
                    .header("cookie", format!("moviebox_session={cookie}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn post_json(&self, uri: &str, body: Value) -> Response<Body> {
        self.post_json_with_origin(uri, body, &self.origin).await
    }

    async fn post_json_with_origin(&self, uri: &str, body: Value, origin: &str) -> Response<Body> {
        self.app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(HOST, "moviebox.example.ts.net")
                    .header(ORIGIN, origin)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn post_json_with_forwarded_for(
        &self,
        uri: &str,
        body: Value,
        forwarded_for: &str,
    ) -> Response<Body> {
        self.app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(HOST, "moviebox.example.ts.net")
                    .header(ORIGIN, &self.origin)
                    .header("x-forwarded-for", forwarded_for)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn post_logout(&self, cookie: &str, csrf_token: &str, origin: &str) -> Response<Body> {
        self.app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/logout")
                    .header(HOST, "moviebox.example.ts.net")
                    .header(ORIGIN, origin)
                    .header("x-csrf-token", csrf_token)
                    .header("cookie", format!("moviebox_session={cookie}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }
}

fn login_request(origin: &str, password: &str, forwarded_for: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header(HOST, "moviebox.example.ts.net")
        .header(ORIGIN, origin)
        .header(CONTENT_TYPE, "application/json");

    if let Some(value) = forwarded_for {
        builder = builder.header("x-forwarded-for", value);
    }

    builder
        .body(Body::from(
            serde_json::to_vec(&json!({ "password": password })).unwrap(),
        ))
        .unwrap()
}

fn header_value(response: &Response<Body>, name: axum::http::header::HeaderName) -> String {
    response.headers()[name].to_str().unwrap().to_string()
}

fn cookie_value(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .and_then(|part| part.split_once('='))
        .map(|(_, value)| value.to_string())
        .unwrap()
}

fn session_id(cookie: &str) -> String {
    cookie.split('.').next().unwrap().to_string()
}

async fn json_body(response: Response<Body>) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}
