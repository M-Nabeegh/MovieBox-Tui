#![cfg(feature = "server")]

//! Completion notifications: the request that actually leaves the server.
//!
//! The wording is unit-tested next to the code; these cover the wire format and
//! the promise that a notification failure never affects the download.

use std::fs;

use moviebox_tui::server::{
    config::ServerConfig,
    notify::{DownloadNotifier, WebhookNotifier},
};
use serde_json::Value;
use std::collections::HashMap;
use tempfile::TempDir;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

/// Build a config whose webhook points at `webhook_url`.
fn config_with_webhook(root: &TempDir, webhook_url: &str, recipient: Option<&str>) -> ServerConfig {
    let media = root.path().join("media");
    let partial = root.path().join("partial");
    let config_root = root.path().join("config");
    for directory in [&media, &partial, &config_root] {
        fs::create_dir_all(directory).unwrap();
    }
    let password = root.path().join("password");
    let session_key = root.path().join("session-key");
    let hook = root.path().join("webhook-url");
    fs::write(&password, "fixture-secret\n").unwrap();
    fs::write(&session_key, "fixture-session-key-that-is-long-enough").unwrap();
    fs::write(&hook, format!("{webhook_url}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in [&password, &session_key, &hook] {
            fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
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
        (
            "MOVIEBOX_NOTIFY_WEBHOOK_URL_FILE".into(),
            hook.display().to_string(),
        ),
        (
            "MOVIEBOX_NOTIFY_LINK_URL".into(),
            "https://jellyfin.example.ts.net/".into(),
        ),
    ]);
    if let Some(recipient) = recipient {
        env.insert("MOVIEBOX_NOTIFY_RECIPIENT".into(), recipient.into());
    }

    ServerConfig::from_map(env).unwrap()
}

#[tokio::test]
async fn ready_download_posts_a_greeting_with_a_tap_through_link() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hooks/whk_fixture"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    let root = tempfile::tempdir().unwrap();
    let config = config_with_webhook(
        &root,
        &format!("{}/hooks/whk_fixture", server.uri()),
        Some("Nabeegh"),
    );
    let notifier = WebhookNotifier::from_config(&config).unwrap().unwrap();

    notifier
        .notify_ready("job-1234", "Cocktail 2", Some("2026"))
        .await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        body["body"], "Hey Nabeegh, Cocktail 2 (2026) is ready to watch.",
        "the notification should name the film and greet the recipient"
    );
    assert_eq!(body["title"], "MovieBox");
    assert_eq!(body["url"], "https://jellyfin.example.ts.net/");

    // Relays reject unknown fields, so the payload must carry nothing else.
    let fields = body
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        fields,
        ["body", "title", "url"]
            .iter()
            .map(|k| k.to_string())
            .collect::<std::collections::BTreeSet<_>>(),
        "payload contains a field the webhook schema does not allow"
    );
    assert!(body["body"].as_str().unwrap().len() <= 2000);

    // The job id lets the relay drop a duplicate of the same completion.
    assert_eq!(
        requests[0]
            .headers
            .get("idempotency-key")
            .map(|value| value.to_str().unwrap()),
        Some("job-1234")
    );
}

#[tokio::test]
async fn notification_without_a_recipient_omits_the_greeting() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let root = tempfile::tempdir().unwrap();
    let config = config_with_webhook(&root, &format!("{}/hooks/whk_fixture", server.uri()), None);
    let notifier = WebhookNotifier::from_config(&config).unwrap().unwrap();

    notifier
        .notify_ready("job-1", "Cocktail 2", Some("2026"))
        .await;

    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["body"], "Cocktail 2 (2026) is ready to watch.");
}

#[tokio::test]
async fn a_rejected_notification_does_not_panic_or_propagate() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let root = tempfile::tempdir().unwrap();
    let config = config_with_webhook(
        &root,
        &format!("{}/hooks/whk_fixture", server.uri()),
        Some("Nabeegh"),
    );
    let notifier = WebhookNotifier::from_config(&config).unwrap().unwrap();

    // The download already succeeded, so this must be silent and infallible.
    notifier
        .notify_ready("job-1", "Cocktail 2", Some("2026"))
        .await;
}

#[tokio::test]
async fn an_unreachable_webhook_does_not_panic_or_propagate() {
    let root = tempfile::tempdir().unwrap();
    // Port 1 on loopback refuses connections.
    let config = config_with_webhook(
        &root,
        "http://127.0.0.1:1/hooks/whk_fixture",
        Some("Nabeegh"),
    );
    let notifier = WebhookNotifier::from_config(&config).unwrap().unwrap();

    notifier
        .notify_ready("job-1", "Cocktail 2", Some("2026"))
        .await;
}

#[tokio::test]
async fn an_insecure_remote_webhook_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let config = config_with_webhook(&root, "http://hark.example/hooks/whk_fixture", None);
    assert!(
        WebhookNotifier::from_config(&config).is_err(),
        "a webhook token must not be sent unencrypted to a remote host"
    );
}

#[tokio::test]
async fn no_webhook_configured_means_no_notifier() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config_with_webhook(&root, "https://hark.example/hooks/whk_fixture", None);
    config.notify_webhook_url_file = None;
    assert!(WebhookNotifier::from_config(&config).unwrap().is_none());
}
