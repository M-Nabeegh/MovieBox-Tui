#![cfg(feature = "server")]

#[path = "support/mod.rs"]
mod support;

use moviebox_tui::{
    catalog::{ResolvedSource, SourceTransport},
    download::{DownloadError, DownloadOutcome, DownloadRequest},
    server::jobs::dash::{
        DashError, DashTransfer, MediaProbe, build_ffmpeg_arguments, choose_video_height,
        choose_video_stream, validate_manifest, validate_probe,
    },
};
use reqwest::header::{HeaderMap, HeaderValue};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use support::http_server::FixtureServer;
use tempfile::{TempDir, tempdir};
use tokio::{
    sync::mpsc,
    time::{Duration, sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use url::Url;

#[test]
fn dash_selects_the_best_video_height_at_or_below_the_requested_limit() {
    let manifest = r#"
      <MPD><Period>
        <AdaptationSet contentType="video">
          <Representation id="low" height="720" />
          <Representation id="high" height="1080" />
          <Representation id="too-high" height="2160" />
        </AdaptationSet>
      </Period></MPD>
    "#;

    assert_eq!(choose_video_height(manifest, 1080), Some(1080));
    assert_eq!(choose_video_height(manifest, 800), Some(720));
    assert_eq!(choose_video_height(manifest, 480), None);
}

#[test]
fn dash_manifest_rejects_non_mpd_and_local_protocol_references() {
    assert!(matches!(
        validate_manifest(b"not a manifest"),
        Err(DashError::InvalidManifest(_))
    ));
    assert!(matches!(
        validate_manifest(br#"<MPD><Period><BaseURL>file:///etc/passwd</BaseURL></Period></MPD>"#),
        Err(DashError::UnsafeManifestReference)
    ));
}

#[test]
fn dash_ffmpeg_arguments_use_stream_copy_and_do_not_render_secret_headers() {
    let mut headers = HeaderMap::new();
    headers.insert("cookie", HeaderValue::from_static("CloudFront-Policy=fake"));
    headers.insert("user-agent", HeaderValue::from_static("FixtureAndroid/1.0"));
    let args = build_ffmpeg_arguments(
        &Url::parse("https://cdn.example.invalid/index.mpd").unwrap(),
        &headers,
        1080,
        &PathBuf::from("/tmp/fixture.mkv"),
    );
    let rendered = args.join(" ");

    assert!(rendered.contains("-c copy"));
    assert!(rendered.contains("-map 0:0"));
    assert!(!rendered.contains("CloudFront-Policy=fake"));
}

#[test]
fn dash_probe_requires_video_and_rejects_notice_duration() {
    let expected = Some(5400.0);
    let no_video = MediaProbe {
        has_video: false,
        duration_seconds: Some(5400.0),
    };
    assert!(matches!(
        validate_probe(no_video, expected),
        Err(DashError::MissingVideoStream)
    ));

    let notice = MediaProbe {
        has_video: true,
        duration_seconds: Some(20.97),
    };
    assert!(matches!(
        validate_probe(notice, expected),
        Err(DashError::NoticeMedia)
    ));

    let notice_as_provider_duration = MediaProbe {
        has_video: true,
        duration_seconds: Some(20.97),
    };
    assert!(validate_probe(notice_as_provider_duration, Some(20.97)).is_err());
    assert!(validate_probe(notice_as_provider_duration, None).is_err());
}

#[test]
fn dash_arguments_force_matroska_without_rendering_auth_headers() {
    let headers = HeaderMap::from_iter([
        (
            "cookie".parse().unwrap(),
            HeaderValue::from_static("CloudFront-Policy=fake-secret"),
        ),
        (
            "user-agent".parse().unwrap(),
            HeaderValue::from_static("FixtureAndroid/1.0"),
        ),
    ]);
    let args = build_ffmpeg_arguments(
        &Url::parse("https://cdn.example.invalid/index.mpd").unwrap(),
        &headers,
        720,
        &PathBuf::from("/tmp/fixture.mkv.dash.tmp"),
    );
    let rendered = args.join(" ");

    assert!(rendered.contains("-f matroska"));
    assert!(!rendered.contains("-headers"));
    assert!(!rendered.contains("fake-secret"));
}

#[test]
fn source_and_request_debug_output_redacts_signed_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        HeaderValue::from_static("CloudFront-Policy=fake-debug-secret"),
    );
    let source = ResolvedSource {
        url: Url::parse("https://cdn.example.invalid/index.mpd").unwrap(),
        headers: headers.clone(),
        subtitle: None,
        extension: "mkv".to_string(),
        expected_size: None,
        catalog_size_bytes: None,
        transport: SourceTransport::Dash {
            maximum_height: 720,
            expected_duration_seconds: Some(5400.0),
        },
    };
    let request = DownloadRequest {
        url: source.url.clone(),
        headers,
        maximum_redirects: 5,
        transport: source.transport.clone(),
    };

    assert!(!format!("{source:?}").contains("fake-debug-secret"));
    assert!(!format!("{request:?}").contains("fake-debug-secret"));
}

#[test]
fn dash_request_carries_explicit_transport_without_changing_http_defaults() {
    let http = DownloadRequest::new(Url::parse("https://cdn.example.invalid/movie.mkv").unwrap());
    assert_eq!(http.transport, SourceTransport::HttpFile);
    let dash = DownloadRequest {
        url: Url::parse("https://cdn.example.invalid/index.mpd").unwrap(),
        headers: HeaderMap::new(),
        maximum_redirects: 5,
        transport: SourceTransport::Dash {
            maximum_height: 1080,
            expected_duration_seconds: Some(5400.0),
        },
    };
    assert!(matches!(dash.transport, SourceTransport::Dash { .. }));
}

#[test]
fn dash_errors_never_include_authorization_material() {
    let error = DashError::ToolFailed {
        tool: "ffmpeg",
        status: Some(1),
    };
    let rendered = error.to_string();
    assert!(!rendered.contains("CloudFront"));
    assert!(!rendered.contains("fake"));
}

#[test]
fn dash_selects_an_actual_ffprobe_video_stream_at_or_below_the_limit() {
    let streams = br#"{"streams":[
      {"index":0,"codec_type":"video","height":1080},
      {"index":1,"codec_type":"video","height":720},
      {"index":2,"codec_type":"audio"}
    ]}"#;

    assert_eq!(choose_video_stream(streams, 720).unwrap(), 1);
    assert!(
        choose_video_stream(br#"{"streams":[{"index":0,"codec_type":"video"}]}"#, 720).is_err()
    );
}

#[tokio::test]
async fn dash_transfer_fetches_manifest_and_segments_through_the_guarded_proxy() {
    let server = FixtureServer::start_dash(dash_manifest(), b"fixture-segment".to_vec())
        .await
        .unwrap();
    let tools = fake_media_tools(tempdir().unwrap(), FakeMediaOutput::FeatureLength);
    let transfer = DashTransfer::with_tools(server.client(), &tools.ffmpeg, &tools.ffprobe);
    let workspace = tempdir().unwrap();
    let destination = workspace.path().join("feature.mkv");
    let request = dash_request(server.url("/manifest.mpd"));

    let result = transfer
        .transfer(
            request,
            &destination,
            CancellationToken::new(),
            mpsc::unbounded_channel().0,
        )
        .await
        .unwrap();

    assert_eq!(result, DownloadOutcome::Completed { bytes: 13 });
    assert_eq!(fs::read(&destination).unwrap(), b"fixture-media");
    let requests = server.requests();
    assert!(
        requests
            .iter()
            .any(|request| request.path == "/manifest.mpd")
    );
    assert!(
        requests
            .iter()
            .any(|request| request.path == "/segment.m4s")
    );
    assert!(
        requests
            .iter()
            .all(|request| request.header("x-moviebox-auth") == Some("fixture-token"))
    );
}

#[tokio::test]
async fn dash_transfer_maps_guarded_segment_403_to_an_auth_error_and_cleans_up() {
    let server = FixtureServer::start_dash(dash_manifest(), b"fixture-segment".to_vec())
        .await
        .unwrap();
    server.enable_segment_forbidden();
    let tools = fake_media_tools(tempdir().unwrap(), FakeMediaOutput::FeatureLength);
    let transfer = DashTransfer::with_tools(server.client(), &tools.ffmpeg, &tools.ffprobe);
    let workspace = tempdir().unwrap();
    let destination = workspace.path().join("forbidden.mkv");
    let temporary = PathBuf::from(format!("{}.dash.tmp", destination.display()));

    let error = transfer
        .transfer(
            dash_request(server.url("/manifest.mpd")),
            &destination,
            CancellationToken::new(),
            mpsc::unbounded_channel().0,
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        DashError::Download(DownloadError::Http(reqwest::StatusCode::FORBIDDEN))
    ));
    assert!(!destination.exists());
    assert!(!temporary.exists());
}

#[tokio::test]
async fn dash_transfer_maps_an_initial_manifest_401_to_an_auth_error() {
    let server = FixtureServer::start_dash(dash_manifest(), b"fixture-segment".to_vec())
        .await
        .unwrap();
    server.enable_manifest_unauthorized();
    let tools = fake_media_tools(tempdir().unwrap(), FakeMediaOutput::FeatureLength);
    let transfer = DashTransfer::with_tools(server.client(), &tools.ffmpeg, &tools.ffprobe);
    let workspace = tempdir().unwrap();

    let error = transfer
        .transfer(
            dash_request(server.url("/manifest.mpd")),
            &workspace.path().join("manifest-401.mkv"),
            CancellationToken::new(),
            mpsc::unbounded_channel().0,
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        DashError::Download(DownloadError::Http(reqwest::StatusCode::UNAUTHORIZED))
    ));
}

#[tokio::test]
async fn dash_transfer_cancellation_kills_ffmpeg_and_removes_scratch_output() {
    let server = FixtureServer::start_dash(dash_manifest(), b"fixture-segment".to_vec())
        .await
        .unwrap();
    let tools = fake_media_tools(tempdir().unwrap(), FakeMediaOutput::Sleep);
    let transfer = DashTransfer::with_tools(server.client(), &tools.ffmpeg, &tools.ffprobe);
    let workspace = tempdir().unwrap();
    let destination = workspace.path().join("cancelled.mkv");
    let temporary = PathBuf::from(format!("{}.dash.tmp", destination.display()));
    let cancellation = CancellationToken::new();
    let transfer_destination = destination.clone();
    let marker = tools.marker.clone();
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        let url = server.url("/manifest.mpd");
        async move {
            transfer
                .transfer(
                    dash_request(url),
                    &transfer_destination,
                    cancellation,
                    mpsc::unbounded_channel().0,
                )
                .await
        }
    });
    timeout(Duration::from_secs(10), async {
        while !marker.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();

    let result = timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap(), DownloadOutcome::Paused { bytes: 0 });
    assert!(!destination.exists());
    assert!(!temporary.exists());
}

#[tokio::test]
async fn dash_transfer_rejects_missing_video_output_and_ffmpeg_failures_without_publishing() {
    let server = FixtureServer::start_dash(dash_manifest(), b"fixture-segment".to_vec())
        .await
        .unwrap();
    let workspace = tempdir().unwrap();
    let destination = workspace.path().join("invalid.mkv");
    let tools = fake_media_tools(tempdir().unwrap(), FakeMediaOutput::NoVideo);
    let transfer = DashTransfer::with_tools(server.client(), &tools.ffmpeg, &tools.ffprobe);
    let error = transfer
        .transfer(
            dash_request(server.url("/manifest.mpd")),
            &destination,
            CancellationToken::new(),
            mpsc::unbounded_channel().0,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, DashError::MissingVideoStream));
    assert!(!destination.exists());

    let destination = workspace.path().join("tool-failed.mkv");
    let tools = fake_media_tools(tempdir().unwrap(), FakeMediaOutput::Fail);
    let transfer = DashTransfer::with_tools(server.client(), &tools.ffmpeg, &tools.ffprobe);
    let error = transfer
        .transfer(
            dash_request(server.url("/manifest.mpd")),
            &destination,
            CancellationToken::new(),
            mpsc::unbounded_channel().0,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        DashError::ToolFailed { tool: "ffmpeg", .. }
    ));
    assert!(!destination.exists());
}

#[tokio::test]
async fn dash_transfer_rejects_the_known_notice_size_and_duration_signature() {
    let server = FixtureServer::start_dash(dash_manifest(), b"fixture-segment".to_vec())
        .await
        .unwrap();
    let tools = fake_media_tools(tempdir().unwrap(), FakeMediaOutput::Notice);
    let transfer = DashTransfer::with_tools(server.client(), &tools.ffmpeg, &tools.ffprobe);
    let workspace = tempdir().unwrap();
    let destination = workspace.path().join("notice.mkv");

    let error = transfer
        .transfer(
            dash_request(server.url("/manifest.mpd")),
            &destination,
            CancellationToken::new(),
            mpsc::unbounded_channel().0,
        )
        .await
        .unwrap_err();

    assert!(matches!(error, DashError::NoticeMedia));
    assert!(!destination.exists());
}

fn dash_manifest() -> Vec<u8> {
    br#"<MPD><Period><AdaptationSet contentType="video">
      <Representation id="1080" height="1080" />
      <Representation id="720" height="720" />
      <SegmentTemplate media="segment.m4s" />
    </AdaptationSet></Period></MPD>"#
        .to_vec()
}

fn dash_request(url: Url) -> DownloadRequest {
    DownloadRequest {
        url,
        headers: FixtureServer::required_headers(),
        maximum_redirects: 5,
        transport: SourceTransport::Dash {
            maximum_height: 720,
            expected_duration_seconds: Some(5400.0),
        },
    }
}

#[derive(Clone, Copy)]
enum FakeMediaOutput {
    FeatureLength,
    NoVideo,
    Notice,
    Fail,
    Sleep,
}

struct FakeMediaTools {
    _temp: TempDir,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    marker: PathBuf,
}

fn fake_media_tools(temp: TempDir, output: FakeMediaOutput) -> FakeMediaTools {
    let mode = match output {
        FakeMediaOutput::FeatureLength => "feature",
        FakeMediaOutput::NoVideo => "no-video",
        FakeMediaOutput::Notice => "notice",
        FakeMediaOutput::Fail => "fail",
        FakeMediaOutput::Sleep => "sleep",
    };
    let ffmpeg = temp.path().join("ffmpeg");
    let ffprobe = temp.path().join("ffprobe");
    let marker = temp.path().join("ffmpeg-started");
    write_executable(
        &ffmpeg,
        &format!(
            r#"#!/bin/sh
set -eu
printf started > "{}"
input=""
previous=""
selected=""
output=""
for argument in "$@"; do
  if [ "$previous" = "-i" ]; then input="$argument"; fi
  if [ "$previous" = "-map" ] && [ "$argument" = "0:1" ]; then selected="yes"; fi
  previous="$argument"
  output="$argument"
done
case "$input" in
  http://127.0.0.1:*/manifest.mpd) ;;
  *) exit 90 ;;
esac
case "$selected" in
  yes) ;;
  *) exit 91 ;;
esac
curl --fail --silent "$input" >/dev/null
curl --fail --silent "${{input%/*}}/segment.m4s" >/dev/null
case "{mode}" in
  feature) printf 'fixture-media' > "$output" ;;
  notice) dd if=/dev/zero of="$output" bs=917554 count=1 2>/dev/null ;;
  fail) exit 7 ;;
  sleep) sleep 30 ;;
  no-video) printf 'fixture-media' > "$output" ;;
esac
"#,
            marker.display()
        ),
    );
    write_executable(
        &ffprobe,
        &format!(
            r#"#!/bin/sh
set -eu
last=""
for argument in "$@"; do last="$argument"; done
case "$last" in
  http://127.0.0.1:*/manifest.mpd)
    printf '%s\n' '{{"streams":[{{"index":0,"codec_type":"video","height":1080}},{{"index":1,"codec_type":"video","height":720}},{{"index":2,"codec_type":"audio"}}]}}'
    ;;
  *)
    case "{mode}" in
      no-video) printf '%s\n' '{{"streams":[{{"index":2,"codec_type":"audio"}}],"format":{{"duration":"5400.0"}}}}' ;;
      notice) printf '%s\n' '{{"streams":[{{"index":1,"codec_type":"video","height":720}}],"format":{{"duration":"20.97"}}}}' ;;
      *) printf '%s\n' '{{"streams":[{{"index":1,"codec_type":"video","height":720}}],"format":{{"duration":"5400.0"}}}}' ;;
    esac
    ;;
esac
"#
        ),
    );
    FakeMediaTools {
        _temp: temp,
        ffmpeg,
        ffprobe,
        marker,
    }
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}
