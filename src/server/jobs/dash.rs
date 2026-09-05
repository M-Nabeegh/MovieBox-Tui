//! Guarded MPEG-DASH transfer for authenticated MovieBox sources.
//!
//! The manifest is fetched through the server's DNS/peer validation first,
//! FFmpeg only remuxes (stream copy), and FFprobe must confirm a real video
//! stream before the worker can publish the file.

use std::{
    ffi::OsString,
    net::IpAddr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use reqwest::header::HeaderMap;
use tokio::{fs, io::AsyncReadExt, process::Command, sync::mpsc, time::sleep};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    catalog::SourceTransport,
    download::{DownloadClient, DownloadError, DownloadOutcome, DownloadRequest, fetch_bytes},
};

use super::worker::TransferProgress;

const DEFAULT_FFMPEG: &str = "ffmpeg";
const DEFAULT_FFPROBE: &str = "ffprobe";
const DURATION_TOLERANCE_SECONDS: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaProbe {
    pub has_video: bool,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum DashError {
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error("DASH manifest is invalid: {0}")]
    InvalidManifest(&'static str),
    #[error("DASH manifest contains an unsafe reference")]
    UnsafeManifestReference,
    #[error("{tool} exited unsuccessfully (status {status:?})")]
    ToolFailed {
        tool: &'static str,
        status: Option<i32>,
    },
    #[error("DASH output did not contain a video stream")]
    MissingVideoStream,
    #[error(
        "DASH output duration {actual_seconds:.2}s does not match provider duration {expected_seconds:.2}s"
    )]
    DurationMismatch {
        expected_seconds: f64,
        actual_seconds: f64,
    },
    #[error("media tool output was invalid")]
    InvalidProbeOutput,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("DASH transfer cancelled")]
    Cancelled,
}

#[derive(Clone)]
pub struct DashTransfer {
    client: DownloadClient,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

impl DashTransfer {
    pub fn new(client: DownloadClient) -> Self {
        Self {
            client,
            ffmpeg: std::env::var_os("MOVIEBOX_FFMPEG")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_FFMPEG)),
            ffprobe: std::env::var_os("MOVIEBOX_FFPROBE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_FFPROBE)),
        }
    }

    pub fn with_tools(
        client: DownloadClient,
        ffmpeg: impl Into<PathBuf>,
        ffprobe: impl Into<PathBuf>,
    ) -> Self {
        Self {
            client,
            ffmpeg: ffmpeg.into(),
            ffprobe: ffprobe.into(),
        }
    }

    pub async fn transfer(
        &self,
        request: DownloadRequest,
        destination: &Path,
        cancel: CancellationToken,
        progress: mpsc::UnboundedSender<TransferProgress>,
    ) -> Result<DownloadOutcome, DashError> {
        let SourceTransport::Dash {
            maximum_height,
            expected_duration_seconds,
        } = request.transport.clone()
        else {
            return Err(DashError::InvalidManifest("non-DASH request"));
        };

        let manifest = fetch_bytes(&self.client, &request).await?;
        if manifest.len() > 16 * 1024 * 1024 {
            return Err(DashError::InvalidManifest("manifest is too large"));
        }
        validate_manifest(&manifest)?;
        let manifest_text =
            std::str::from_utf8(&manifest).map_err(|_| DashError::InvalidManifest("not UTF-8"))?;
        let representations = manifest_tag_chunks(manifest_text, "Representation");
        if !representations.is_empty()
            && choose_video_height(manifest_text, maximum_height).is_none()
        {
            return Err(DashError::InvalidManifest(
                "no video representation is within the requested height",
            ));
        }

        if cancel.is_cancelled() {
            return Ok(DownloadOutcome::Paused { bytes: 0 });
        }

        let temporary = temporary_output_path(destination);
        let _ = fs::remove_file(&temporary).await;
        let args =
            private_ffmpeg_arguments(&request.url, &request.headers, maximum_height, &temporary);
        let mut child = Command::new(&self.ffmpeg)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(DashError::Io)?;

        let monitor_cancel = cancel.clone();
        let monitor_output = temporary.clone();
        let monitor_progress = progress.clone();
        let monitor = tokio::spawn(async move {
            loop {
                if monitor_cancel.is_cancelled() {
                    break;
                }
                let bytes = fs::metadata(&monitor_output)
                    .await
                    .map(|metadata| metadata.len())
                    .unwrap_or_default();
                let _ = monitor_progress.send(TransferProgress {
                    downloaded_bytes: bytes,
                    total_bytes: None,
                    speed_bytes_per_second: None,
                });
                sleep(Duration::from_millis(250)).await;
            }
        });

        let status = tokio::select! {
            status = child.wait() => status.map_err(DashError::Io)?,
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                monitor.abort();
                let _ = fs::remove_file(&temporary).await;
                let _ = progress.send(TransferProgress {
                    downloaded_bytes: 0,
                    total_bytes: Some(0),
                    speed_bytes_per_second: None,
                });
                return Ok(DownloadOutcome::Paused { bytes: 0 });
            }
        };
        monitor.abort();
        if !status.success() {
            let _ = fs::remove_file(&temporary).await;
            return Err(DashError::ToolFailed {
                tool: "ffmpeg",
                status: status.code(),
            });
        }

        let probe = match self.probe(&temporary, cancel.clone()).await {
            Ok(probe) => probe,
            Err(error) => {
                let _ = fs::remove_file(&temporary).await;
                return Err(error);
            }
        };
        if let Err(error) = validate_probe(probe, expected_duration_seconds) {
            let _ = fs::remove_file(&temporary).await;
            return Err(error);
        }
        let bytes = fs::metadata(&temporary).await?.len();
        if cancel.is_cancelled() {
            let _ = fs::remove_file(&temporary).await;
            return Ok(DownloadOutcome::Paused { bytes: 0 });
        }
        fs::rename(&temporary, destination).await?;
        let _ = progress.send(TransferProgress {
            downloaded_bytes: bytes,
            total_bytes: Some(bytes),
            speed_bytes_per_second: None,
        });
        Ok(DownloadOutcome::Completed { bytes })
    }

    async fn probe(&self, path: &Path, cancel: CancellationToken) -> Result<MediaProbe, DashError> {
        let mut child = Command::new(&self.ffprobe)
            .args([
                OsString::from("-v"),
                OsString::from("error"),
                OsString::from("-show_entries"),
                OsString::from("stream=codec_type:format=duration"),
                OsString::from("-of"),
                OsString::from("json"),
                path.as_os_str().to_owned(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(DashError::Io)?;
        let mut output = Vec::new();
        let mut stdout = child.stdout.take().ok_or(DashError::InvalidProbeOutput)?;
        let status = tokio::select! {
            result = stdout.read_to_end(&mut output) => {
                result?;
                child.wait().await.map_err(DashError::Io)?
            }
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                return Err(DashError::Cancelled);
            }
        };
        if !status.success() {
            return Err(DashError::ToolFailed {
                tool: "ffprobe",
                status: status.code(),
            });
        }
        parse_probe_json(&output)
    }
}

fn temporary_output_path(destination: &Path) -> PathBuf {
    let mut value = destination.as_os_str().to_os_string();
    value.push(".dash.tmp");
    PathBuf::from(value)
}

/// Return the highest declared video representation not exceeding the limit.
pub fn choose_video_height(manifest: &str, maximum_height: u16) -> Option<u16> {
    let mut selected = None;
    for tag in manifest_tag_chunks(manifest, "Representation") {
        let Some(height) = attribute(tag, "height").and_then(|value| value.parse::<u16>().ok())
        else {
            continue;
        };
        if height <= maximum_height && selected.is_none_or(|current| height > current) {
            selected = Some(height);
        }
    }
    selected
}

/// Validate the fetched manifest before handing it to FFmpeg.
pub fn validate_manifest(bytes: &[u8]) -> Result<(), DashError> {
    let text = std::str::from_utf8(bytes).map_err(|_| DashError::InvalidManifest("not UTF-8"))?;
    if !text.contains("<MPD") || !text.contains("</MPD>") {
        return Err(DashError::InvalidManifest("missing MPD root"));
    }
    for token in text.split(|character: char| {
        character.is_ascii_whitespace() || matches!(character, '"' | '\'' | '<' | '>' | '=')
    }) {
        if is_unsafe_scheme(token)
            || (token.contains("://") && is_unsafe_reference(token))
            || (token.starts_with("//") && is_unsafe_reference(token))
        {
            return Err(DashError::UnsafeManifestReference);
        }
    }
    for tag in manifest_tag_chunks(text, "BaseURL") {
        let value = tag.trim();
        if is_unsafe_scheme(value)
            || (value.contains("://") && is_unsafe_reference(value))
            || (value.starts_with("//") && is_unsafe_reference(value))
        {
            return Err(DashError::UnsafeManifestReference);
        }
    }
    for key in ["media", "initialization", "sourceURL"] {
        for tag in manifest_tag_chunks(text, "") {
            if let Some(value) = attribute(tag, key)
                && (is_unsafe_scheme(value)
                    || (value.contains("://") && is_unsafe_reference(value))
                    || (value.starts_with("//") && is_unsafe_reference(value)))
            {
                return Err(DashError::UnsafeManifestReference);
            }
        }
    }
    Ok(())
}

fn is_unsafe_reference(value: &str) -> bool {
    let value = value.trim();
    let parse_value = if value.starts_with("//") {
        format!("https:{value}")
    } else {
        value.to_string()
    };
    let Ok(url) = Url::parse(&parse_value) else {
        return true;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return true;
    }
    let Some(host) = url.host_str() else {
        return true;
    };
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.ends_with(".local")
    {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|address| {
        address.is_loopback()
            || address.is_unspecified()
            || match address {
                IpAddr::V4(address) => address.is_private() || address.is_link_local(),
                IpAddr::V6(address) => {
                    let first = address.segments()[0];
                    (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
                }
            }
    })
}

fn is_unsafe_scheme(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    ["file:", "data:", "pipe:", "concat:", "subfile:"]
        .iter()
        .any(|scheme| value.starts_with(scheme))
}

pub fn validate_probe(
    probe: MediaProbe,
    expected_duration_seconds: Option<f64>,
) -> Result<(), DashError> {
    if !probe.has_video {
        return Err(DashError::MissingVideoStream);
    }
    if let Some(expected) = expected_duration_seconds {
        if !expected.is_finite() || expected <= 0.0 {
            return Err(DashError::InvalidProbeOutput);
        }
        let Some(actual) = probe.duration_seconds else {
            return Err(DashError::DurationMismatch {
                expected_seconds: expected,
                actual_seconds: 0.0,
            });
        };
        if !actual.is_finite() || actual <= 0.0 {
            return Err(DashError::InvalidProbeOutput);
        }
        if (actual - expected).abs() > DURATION_TOLERANCE_SECONDS {
            return Err(DashError::DurationMismatch {
                expected_seconds: expected,
                actual_seconds: actual,
            });
        }
    }
    Ok(())
}

/// Safe, redacted representation useful for tests and diagnostics.
pub fn build_ffmpeg_arguments(
    url: &Url,
    _headers: &HeaderMap,
    _maximum_height: u16,
    output: &Path,
) -> Vec<String> {
    private_ffmpeg_arguments(url, &HeaderMap::new(), 0, output)
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect()
}

fn private_ffmpeg_arguments(
    url: &Url,
    headers: &HeaderMap,
    _maximum_height: u16,
    output: &Path,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("-hide_banner"),
        OsString::from("-loglevel"),
        OsString::from("error"),
        OsString::from("-y"),
        OsString::from("-protocol_whitelist"),
        OsString::from("http,https,tcp,tls,crypto"),
    ];
    if let Some(value) = ffmpeg_header_argument(headers) {
        args.extend([OsString::from("-headers"), OsString::from(value)]);
    }
    args.extend([
        OsString::from("-i"),
        OsString::from(url.as_str()),
        OsString::from("-map"),
        OsString::from("0:v:0"),
        OsString::from("-map"),
        OsString::from("0:a:0?"),
        OsString::from("-c"),
        OsString::from("copy"),
        output.as_os_str().to_owned(),
    ]);
    args
}

fn ffmpeg_header_argument(headers: &HeaderMap) -> Option<String> {
    let mut rendered = String::new();
    for (name, value) in headers {
        let value = value.to_str().ok()?;
        rendered.push_str(name.as_str());
        rendered.push_str(": ");
        rendered.push_str(value);
        rendered.push_str("\r\n");
    }
    (!rendered.is_empty()).then_some(rendered)
}

fn parse_probe_json(bytes: &[u8]) -> Result<MediaProbe, DashError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| DashError::InvalidProbeOutput)?;
    let has_video = value
        .get("streams")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|streams| {
            streams.iter().any(|stream| {
                stream.get("codec_type").and_then(serde_json::Value::as_str) == Some("video")
            })
        });
    let duration_seconds = value
        .get("format")
        .and_then(|format| format.get("duration"))
        .and_then(|duration| {
            duration
                .as_f64()
                .or_else(|| duration.as_str().and_then(|value| value.parse().ok()))
        });
    Ok(MediaProbe {
        has_video,
        duration_seconds,
    })
}

fn manifest_tag_chunks<'a>(text: &'a str, name: &str) -> Vec<&'a str> {
    let mut chunks = Vec::new();
    let mut cursor = text;
    while let Some(start) = cursor.find('<') {
        cursor = &cursor[start + 1..];
        let end = cursor.find('>').unwrap_or(cursor.len());
        let chunk = &cursor[..end];
        if name.is_empty() || chunk.starts_with(name) || chunk.starts_with(&format!("{name} ")) {
            chunks.push(chunk);
        }
        if end == cursor.len() {
            break;
        }
        cursor = &cursor[end + 1..];
    }
    chunks
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("{name}=\"");
    let start = tag.find(&marker)? + marker.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}
