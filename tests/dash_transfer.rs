#![cfg(feature = "server")]

use moviebox_tui::{
    catalog::SourceTransport,
    download::DownloadRequest,
    server::jobs::dash::{
        DashError, MediaProbe, build_ffmpeg_arguments, choose_video_height, validate_manifest,
        validate_probe,
    },
};
use reqwest::header::{HeaderMap, HeaderValue};
use std::path::PathBuf;
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
    assert!(rendered.contains("-map 0:v:0"));
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
        Err(DashError::DurationMismatch { .. })
    ));
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
