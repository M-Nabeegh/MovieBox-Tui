#![cfg(feature = "server")]

#[path = "support/mod.rs"]
mod support;

use moviebox_tui::{
    download::{DownloadRequest, download},
    server::security::{
        net::{
            AddressResolver, ResolveFuture, follow_checked_redirects,
            follow_checked_redirects_same_origin,
            follow_checked_redirects_same_origin_with_peer_cache, resolve_public_addresses,
            validate_public_http_url,
        },
        path::contained_path,
    },
};
use reqwest::Method;
use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use support::http_server::FixtureServer;
use tempfile::tempdir;
use tokio::fs;
use url::Url;

#[derive(Clone)]
struct RotatingResolver {
    calls: Arc<AtomicUsize>,
}

impl AddressResolver for RotatingResolver {
    fn resolve<'a>(&'a self, _host: &'a str, port: u16) -> ResolveFuture<'a> {
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        let address = if call == 0 {
            IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))
        } else {
            IpAddr::V4(Ipv4Addr::new(1, 0, 0, 1))
        };
        Box::pin(async move { Ok(vec![SocketAddr::new(address, port)]) })
    }
}

#[tokio::test]
async fn download_sends_required_headers_on_redirects_segment_retries_and_resume() {
    let server = FixtureServer::start(33 * 1024 * 1024).await.unwrap();
    server.enable_drop_first_ranged_request();
    let client = server.client();
    let destination_dir = tempdir().unwrap();
    let destination = destination_dir.path().join("movie.mkv");

    let request = DownloadRequest {
        url: server.url("/redirect/download"),
        headers: FixtureServer::required_headers(),
        maximum_redirects: 4,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
    };

    let outcome = download(
        &client,
        request,
        &destination,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .await
    .unwrap();

    assert_eq!(
        outcome,
        moviebox_tui::download::DownloadOutcome::Completed {
            bytes: server.content_len() as u64,
        }
    );
    let requests = server.requests();
    assert!(
        requests
            .iter()
            .any(|request| request.path == "/redirect/download")
    );
    assert!(requests.iter().any(|request| request.path == "/download"));
    assert!(requests.iter().all(|request| request.method == "GET"));
    assert!(
        requests
            .iter()
            .any(|request| request.header("range").is_some())
    );
    assert!(requests.iter().all(|request| {
        request
            .header("x-moviebox-auth")
            .is_some_and(|value| value == "fixture-token")
    }));
}

#[tokio::test]
async fn download_sends_required_headers_on_resume_requests() {
    let server = FixtureServer::start(1024 * 1024).await.unwrap();
    let client = server.client();
    let destination_dir = tempdir().unwrap();
    let destination = destination_dir.path().join("resume.mp4");

    let partial = sidecar_path(&destination, "part");
    let metadata = sidecar_path(&destination, "part.json");
    fs::write(&partial, vec![7_u8; 4096]).await.unwrap();
    fs::write(
        &metadata,
        serde_json::json!({
            "etag": FixtureServer::etag(),
            "last_modified": FixtureServer::last_modified(),
            "total": server.content_len(),
            "segments": serde_json::Value::Null,
        })
        .to_string(),
    )
    .await
    .unwrap();

    let request = DownloadRequest {
        url: server.url("/download"),
        headers: FixtureServer::required_headers(),
        maximum_redirects: 2,
        transport: moviebox_tui::catalog::SourceTransport::HttpFile,
    };

    let outcome = download(
        &client,
        request,
        &destination,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .await
    .unwrap();

    assert_eq!(
        outcome,
        moviebox_tui::download::DownloadOutcome::Completed {
            bytes: server.content_len() as u64,
        }
    );
    let requests = server.requests();
    assert!(requests.iter().any(|request| {
        request
            .header("range")
            .is_some_and(|range| range == "bytes=4096-")
    }));
    assert!(requests.iter().all(|request| {
        request
            .header("x-moviebox-auth")
            .is_some_and(|value| value == "fixture-token")
    }));
}

#[tokio::test]
async fn subtitle_redirect_requests_preserve_source_headers() {
    let server = FixtureServer::start(4096).await.unwrap();
    let client = server.client();

    let response = follow_checked_redirects(
        &client,
        Method::GET,
        server.url("/redirect/subtitle"),
        FixtureServer::required_headers(),
        2,
    )
    .await
    .unwrap();

    let bytes = response.bytes().await.unwrap();
    assert_eq!(bytes.as_slice(), server.subtitle_bytes());
    let requests = server.requests();
    assert!(
        requests
            .iter()
            .any(|request| request.path == "/redirect/subtitle")
    );
    assert!(requests.iter().any(|request| request.path == "/subtitle"));
    assert!(requests.iter().all(|request| {
        request
            .header("x-moviebox-auth")
            .is_some_and(|value| value == "fixture-token")
    }));
}

#[tokio::test]
async fn redirects_to_private_targets_are_rejected_before_following() {
    let server = FixtureServer::start(1024).await.unwrap();
    let client = server.client();

    let error = follow_checked_redirects(
        &client,
        Method::GET,
        server.url("/redirect/private"),
        FixtureServer::required_headers(),
        2,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("127.0.0.1"));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn scoped_redirects_reject_a_different_remote_origin_before_forwarding_auth() {
    let server = FixtureServer::start(1024).await.unwrap();
    let client = server.client();
    let origin = server.url("/download");

    let error = follow_checked_redirects_same_origin(
        &client,
        Method::GET,
        server.url("/redirect/private"),
        FixtureServer::required_headers(),
        2,
        origin,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("origin"));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn same_origin_dash_requests_keep_a_peer_validated_before_dns_rotation() {
    let server = FixtureServer::start(1024).await.unwrap();
    let client = server.client_with_resolver(RotatingResolver {
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let trusted_peers = Arc::new(Mutex::new(BTreeSet::new()));
    let origin = server.url("/download");

    let response = follow_checked_redirects_same_origin_with_peer_cache(
        &client,
        reqwest::Method::GET,
        origin.clone(),
        FixtureServer::required_headers(),
        2,
        origin.clone(),
        Arc::clone(&trusted_peers),
    )
    .await
    .unwrap();
    response.bytes().await.unwrap();

    let response = follow_checked_redirects_same_origin_with_peer_cache(
        &client,
        reqwest::Method::GET,
        origin.clone(),
        FixtureServer::required_headers(),
        2,
        origin,
        trusted_peers,
    )
    .await
    .unwrap();
    response.bytes().await.unwrap();
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn resolver_and_connected_peer_mismatch_is_rejected() {
    let server = FixtureServer::start(1024).await.unwrap();
    let client = server.mismatched_client();

    let error = follow_checked_redirects(
        &client,
        Method::GET,
        server.url("/download"),
        FixtureServer::required_headers(),
        2,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("127.0.0.1"));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn url_policy_rejects_private_and_ipv4_mapped_private_destinations() {
    let private_url = Url::parse("http://127.0.0.1/video.mp4").unwrap();
    validate_public_http_url(&private_url).unwrap();
    let private_error = resolve_public_addresses(&private_url).await.unwrap_err();
    assert!(private_error.to_string().contains("127.0.0.1"));

    let mapped_private_url = Url::parse("http://[::ffff:10.0.0.8]/video.mp4").unwrap();
    validate_public_http_url(&mapped_private_url).unwrap();
    let mapped_error = resolve_public_addresses(&mapped_private_url)
        .await
        .unwrap_err();
    assert!(mapped_error.to_string().contains("10.0.0.8"));
}

#[test]
fn url_policy_rejects_non_http_urls_and_missing_hosts() {
    let ftp = Url::parse("ftp://example.com/video.mp4").unwrap();
    let ftp_error = validate_public_http_url(&ftp).unwrap_err();
    assert!(ftp_error.to_string().contains("http"));

    let file = Url::parse("file:///tmp/video.mp4").unwrap();
    let file_error = validate_public_http_url(&file).unwrap_err();
    assert!(file_error.to_string().contains("http"));
}

#[test]
fn contained_path_keeps_paths_within_media_root() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("media");
    std::fs::create_dir_all(root.join("movies")).unwrap();

    let path = contained_path(&root, Path::new("movies/feature.mkv")).unwrap();

    assert_eq!(
        path,
        root.canonicalize().unwrap().join("movies/feature.mkv")
    );
}

#[test]
fn contained_path_rejects_absolute_paths_traversal_and_control_characters() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("media");
    std::fs::create_dir_all(&root).unwrap();

    let absolute = contained_path(&root, Path::new("/tmp/escape.mkv")).unwrap_err();
    assert!(absolute.to_string().contains("absolute"));

    let traversal = contained_path(&root, Path::new("../escape.mkv")).unwrap_err();
    assert!(traversal.to_string().contains("traversal"));

    let control = contained_path(&root, Path::new("bad\u{0000}.mkv")).unwrap_err();
    assert!(control.to_string().contains("control"));
}

#[cfg(unix)]
#[test]
fn contained_path_rejects_symlink_escapes() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap();
    let root = temp.path().join("media");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    symlink(&outside, root.join("series")).unwrap();

    let error = contained_path(&root, Path::new("series/episode.mkv")).unwrap_err();

    assert!(error.to_string().contains("outside"));
}

fn sidecar_path(destination: &Path, suffix: &str) -> PathBuf {
    let mut name = destination.as_os_str().to_os_string();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}
