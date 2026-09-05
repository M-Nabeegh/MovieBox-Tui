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
    sync::{Arc, Mutex},
    time::Duration,
};

use reqwest::{
    Method, StatusCode,
    header::{HeaderMap, HeaderValue, RANGE},
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::sleep,
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    catalog::SourceTransport,
    download::{DownloadClient, DownloadError, DownloadOutcome, DownloadRequest},
    server::security::net::follow_checked_redirects_same_origin,
};

use super::worker::TransferProgress;

const DEFAULT_FFMPEG: &str = "ffmpeg";
const DEFAULT_FFPROBE: &str = "ffprobe";
const DURATION_TOLERANCE_SECONDS: f64 = 8.0;
const NOTICE_DURATION_SECONDS: f64 = 20.97;
const NOTICE_DURATION_TOLERANCE_SECONDS: f64 = 0.5;
const NOTICE_SIZE_BYTES: u64 = 917_554;
const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;

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
    #[error("DASH output matches the MovieBox notice clip")]
    NoticeMedia,
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

        if cancel.is_cancelled() {
            return Ok(DownloadOutcome::Paused { bytes: 0 });
        }
        let expected_duration = require_provider_duration(expected_duration_seconds)?;
        let response = follow_checked_redirects_same_origin(
            &self.client,
            Method::GET,
            request.url.clone(),
            request.headers.clone(),
            request.maximum_redirects,
            request.url.clone(),
        )
        .await
        .map_err(DownloadError::from)?;
        if !response.status().is_success() {
            return Err(DashError::Download(DownloadError::Http(response.status())));
        }
        let remote_url = response.url().clone();
        let manifest = response.bytes().await.map_err(DownloadError::from)?;
        if manifest.len() > MAX_MANIFEST_BYTES {
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

        let proxy = DashProxy::start(
            self.client.clone(),
            manifest,
            remote_url,
            request.headers.clone(),
            request.maximum_redirects,
        )
        .await?;
        let selected_video_stream = match self
            .probe_video_stream(&proxy.url, maximum_height, cancel.clone())
            .await
        {
            Ok(index) => index,
            Err(DashError::Cancelled) if cancel.is_cancelled() => {
                return Ok(DownloadOutcome::Paused { bytes: 0 });
            }
            Err(error) => {
                if let Some(status) = proxy.auth_status() {
                    return Err(DashError::Download(DownloadError::Http(status)));
                }
                return Err(error);
            }
        };
        if cancel.is_cancelled() {
            return Ok(DownloadOutcome::Paused { bytes: 0 });
        }

        let temporary = temporary_output_path(destination);
        let _ = fs::remove_file(&temporary).await;
        let args = private_ffmpeg_arguments(&proxy.url, selected_video_stream, &temporary);
        let mut command = Command::new(&self.ffmpeg);
        command
            .kill_on_drop(true)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(DashError::Io)?;

        let monitor_cancel = cancel.clone();
        let monitor_output = temporary.clone();
        let monitor_progress = progress.clone();
        let monitor = MonitorGuard::new(tokio::spawn(async move {
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
        }));

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
            if let Some(status) = proxy.auth_status() {
                return Err(DashError::Download(DownloadError::Http(status)));
            }
            return Err(DashError::ToolFailed {
                tool: "ffmpeg",
                status: status.code(),
            });
        }
        if let Some(status) = proxy.auth_status() {
            let _ = fs::remove_file(&temporary).await;
            return Err(DashError::Download(DownloadError::Http(status)));
        }

        let probe = match self.probe(&temporary, cancel.clone()).await {
            Ok(probe) => probe,
            Err(DashError::Cancelled) if cancel.is_cancelled() => {
                let _ = fs::remove_file(&temporary).await;
                return Ok(DownloadOutcome::Paused { bytes: 0 });
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary).await;
                if let Some(status) = proxy.auth_status() {
                    return Err(DashError::Download(DownloadError::Http(status)));
                }
                return Err(error);
            }
        };
        if let Err(error) = validate_probe(probe, Some(expected_duration)) {
            let _ = fs::remove_file(&temporary).await;
            return Err(error);
        }
        let bytes = fs::metadata(&temporary).await?.len();
        if bytes == NOTICE_SIZE_BYTES {
            let _ = fs::remove_file(&temporary).await;
            return Err(DashError::NoticeMedia);
        }
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

    async fn probe_video_stream(
        &self,
        manifest_url: &Url,
        maximum_height: u16,
        cancel: CancellationToken,
    ) -> Result<u32, DashError> {
        let mut child = Command::new(&self.ffprobe)
            .args([
                OsString::from("-v"),
                OsString::from("error"),
                OsString::from("-show_entries"),
                OsString::from("stream=index,codec_type,height"),
                OsString::from("-of"),
                OsString::from("json"),
                manifest_url.as_str().into(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(DashError::Io)?;
        let mut output = Vec::new();
        let mut stdout = child.stdout.take().ok_or(DashError::InvalidProbeOutput)?;
        tokio::select! {
            result = stdout.read_to_end(&mut output) => {
                result?;
            }
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                return Err(DashError::Cancelled);
            }
        }
        let status = child.wait().await.map_err(DashError::Io)?;
        if !status.success() {
            return Err(DashError::ToolFailed {
                tool: "ffprobe",
                status: status.code(),
            });
        }
        choose_video_stream(&output, maximum_height)
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
            .kill_on_drop(true)
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

struct MonitorGuard(JoinHandle<()>);

impl MonitorGuard {
    fn new(handle: JoinHandle<()>) -> Self {
        Self(handle)
    }

    fn abort(&self) {
        self.0.abort();
    }
}

impl Drop for MonitorGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone)]
struct DashProxyState {
    client: DownloadClient,
    manifest: Arc<Vec<u8>>,
    manifest_directory: Url,
    origin: Url,
    headers: HeaderMap,
    maximum_redirects: u8,
    auth_status: Arc<Mutex<Option<StatusCode>>>,
}

struct DashProxy {
    url: Url,
    auth_status: Arc<Mutex<Option<StatusCode>>>,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl DashProxy {
    async fn start(
        client: DownloadClient,
        manifest: Vec<u8>,
        remote_url: Url,
        headers: HeaderMap,
        maximum_redirects: u8,
    ) -> Result<Self, DashError> {
        let rewritten = rewrite_manifest_for_proxy(&manifest, &remote_url)?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let mut origin = remote_url.clone();
        origin.set_path("/");
        origin.set_query(None);
        origin.set_fragment(None);
        let manifest_directory = remote_url
            .join("./")
            .map_err(|_| DashError::InvalidManifest("manifest URL has no directory"))?;
        let auth_status = Arc::new(Mutex::new(None));
        let state = DashProxyState {
            client,
            manifest: Arc::new(rewritten),
            manifest_directory,
            origin,
            headers,
            maximum_redirects,
            auth_status: Arc::clone(&auth_status),
        };
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break; };
                        let state = state.clone();
                        tokio::spawn(async move {
                            let _ = serve_dash_proxy_connection(stream, state).await;
                        });
                    }
                }
            }
        });
        let url = Url::parse(&format!("http://127.0.0.1:{}/manifest.mpd", address.port()))
            .map_err(|_| DashError::InvalidManifest("proxy URL is invalid"))?;
        Ok(Self {
            url,
            auth_status,
            shutdown: Some(shutdown_tx),
            task,
        })
    }

    fn auth_status(&self) -> Option<StatusCode> {
        self.auth_status.lock().ok().and_then(|status| *status)
    }
}

impl Drop for DashProxy {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}

async fn serve_dash_proxy_connection(
    mut stream: TcpStream,
    state: DashProxyState,
) -> Result<(), std::io::Error> {
    let request = read_dash_proxy_request(&mut stream).await?;
    if request.method != Method::GET {
        write_dash_proxy_response(&mut stream, StatusCode::METHOD_NOT_ALLOWED, &[]).await?;
        return Ok(());
    }
    if request.target == "/manifest.mpd" {
        write_dash_proxy_response(&mut stream, StatusCode::OK, &state.manifest).await?;
        return Ok(());
    }

    let local_url = Url::parse(&format!("http://dash-proxy.invalid{}", request.target))
        .map_err(|_| std::io::Error::other("invalid proxy request target"))?;
    let upstream = match proxy_target_url(&state, &local_url) {
        Ok(upstream) => upstream,
        Err(_) => {
            write_dash_proxy_response(&mut stream, StatusCode::BAD_REQUEST, &[]).await?;
            return Ok(());
        }
    };
    let mut headers = state.headers.clone();
    if let Some(range) = request.headers.get("range") {
        if let Ok(value) = HeaderValue::from_str(range) {
            headers.insert(RANGE, value);
        }
    }
    let response = follow_checked_redirects_same_origin(
        &state.client,
        Method::GET,
        upstream,
        headers,
        state.maximum_redirects,
        state.origin.clone(),
    )
    .await;
    match response {
        Ok(response) => {
            let status = response.status();
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
                && let Ok(mut auth_status) = state.auth_status.lock()
            {
                *auth_status = Some(status);
            }
            let body = if status.is_success() {
                response
                    .bytes()
                    .await
                    .map_err(|error| std::io::Error::other(error.to_string()))?
            } else {
                Vec::new()
            };
            write_dash_proxy_response(&mut stream, status, &body).await?;
        }
        Err(_error) => {
            write_dash_proxy_response(&mut stream, StatusCode::BAD_GATEWAY, &[]).await?;
        }
    }
    Ok(())
}

struct DashProxyRequest {
    method: Method,
    target: String,
    headers: std::collections::HashMap<String, String>,
}

async fn read_dash_proxy_request(
    stream: &mut TcpStream,
) -> Result<DashProxyRequest, std::io::Error> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "proxy request ended before headers",
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if bytes.len() > 64 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "proxy request headers are too large",
            ));
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.split("\r\n");
    let first = lines.next().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing request line")
    })?;
    let mut parts = first.split_whitespace();
    let method = Method::from_bytes(
        parts
            .next()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing method"))?
            .as_bytes(),
    )
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid method"))?;
    let target = parts
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing target"))?
        .to_string();
    let mut headers = std::collections::HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    Ok(DashProxyRequest {
        method,
        target,
        headers,
    })
}

async fn write_dash_proxy_response(
    stream: &mut TcpStream,
    status: StatusCode,
    body: &[u8],
) -> Result<(), std::io::Error> {
    let reason = status.canonical_reason().unwrap_or("error");
    stream
        .write_all(
            format!(
                "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                status.as_u16(),
                reason,
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(body).await
}

fn proxy_target_url(state: &DashProxyState, local_url: &Url) -> Result<Url, DashError> {
    let path = local_url.path();
    let mut upstream = if let Some(path) = path.strip_prefix("/__dash_origin/") {
        state
            .origin
            .join(path)
            .map_err(|_| DashError::UnsafeManifestReference)?
    } else {
        state
            .manifest_directory
            .join(path.trim_start_matches('/'))
            .map_err(|_| DashError::UnsafeManifestReference)?
    };
    upstream.set_query(local_url.query());
    upstream.set_fragment(None);
    if upstream.origin() != state.origin.origin() {
        return Err(DashError::UnsafeManifestReference);
    }
    Ok(upstream)
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

fn rewrite_manifest_for_proxy(bytes: &[u8], remote_url: &Url) -> Result<Vec<u8>, DashError> {
    let text = std::str::from_utf8(bytes).map_err(|_| DashError::InvalidManifest("not UTF-8"))?;
    let mut rewritten = text.to_string();
    for attribute_name in ["media", "initialization", "sourceURL"] {
        rewritten = rewrite_dash_attribute(&rewritten, attribute_name, remote_url)?;
    }
    let mut cursor = 0;
    while let Some(start) = rewritten[cursor..].find("<BaseURL>") {
        let start = cursor + start + "<BaseURL>".len();
        let Some(end) = rewritten[start..].find("</BaseURL>") else {
            return Err(DashError::InvalidManifest("unterminated BaseURL"));
        };
        let end = start + end;
        let value = rewritten[start..end].trim();
        let replacement = rewrite_dash_reference(value, remote_url)?;
        rewritten.replace_range(start..end, &replacement);
        cursor = start + replacement.len();
    }
    if rewritten
        .split(|character: char| {
            character.is_ascii_whitespace() || matches!(character, '"' | '\'' | '<' | '>' | '=')
        })
        .any(|token| token.contains("://") || token.starts_with("//"))
    {
        return Err(DashError::UnsafeManifestReference);
    }
    Ok(rewritten.into_bytes())
}

fn rewrite_dash_attribute(
    text: &str,
    attribute_name: &str,
    remote_url: &Url,
) -> Result<String, DashError> {
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    while cursor < text.len() {
        let Some(found) = text[cursor..].find(attribute_name) else {
            break;
        };
        let start = cursor + found;
        let previous = start
            .checked_sub(1)
            .and_then(|index| text.as_bytes().get(index));
        if previous.is_some_and(|byte| is_xml_name_byte(*byte)) {
            cursor = start + attribute_name.len();
            continue;
        }
        let mut equals = start + attribute_name.len();
        while text
            .as_bytes()
            .get(equals)
            .is_some_and(u8::is_ascii_whitespace)
        {
            equals += 1;
        }
        if text.as_bytes().get(equals) != Some(&b'=') {
            cursor = start + attribute_name.len();
            continue;
        }
        equals += 1;
        while text
            .as_bytes()
            .get(equals)
            .is_some_and(u8::is_ascii_whitespace)
        {
            equals += 1;
        }
        let Some(&quote) = text.as_bytes().get(equals) else {
            return Err(DashError::InvalidManifest("unterminated DASH attribute"));
        };
        if !matches!(quote, b'"' | b'\'') {
            return Err(DashError::InvalidManifest(
                "DASH attribute value must be quoted",
            ));
        }
        let value_start = equals + 1;
        let Some(value_end_offset) = text[value_start..].find(char::from(quote)) else {
            return Err(DashError::InvalidManifest("unterminated DASH attribute"));
        };
        let value_end = value_start + value_end_offset;
        output.push_str(&text[cursor..value_start]);
        output.push_str(&rewrite_dash_reference(
            &text[value_start..value_end],
            remote_url,
        )?);
        cursor = value_end;
    }
    output.push_str(&text[cursor..]);
    Ok(output)
}

fn is_xml_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':')
}

fn rewrite_dash_reference(value: &str, remote_url: &Url) -> Result<String, DashError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(value.to_string());
    }
    if is_unsafe_scheme(value) {
        return Err(DashError::UnsafeManifestReference);
    }
    let Some(parsed) = parse_absolute_dash_url(value, remote_url) else {
        return Ok(value.to_string());
    };
    if parsed.origin() != remote_url.origin() {
        return Err(DashError::UnsafeManifestReference);
    }
    let mut local = format!("/__dash_origin{}", parsed.path());
    if let Some(query) = parsed.query() {
        local.push('?');
        local.push_str(query);
    }
    Ok(local)
}

fn parse_absolute_dash_url(value: &str, remote_url: &Url) -> Option<Url> {
    if value.starts_with("//") {
        Url::parse(&format!("{}:{value}", remote_url.scheme())).ok()
    } else if value.starts_with("http://") || value.starts_with("https://") {
        Url::parse(value).ok()
    } else {
        None
    }
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
    let Some(expected) = expected_duration_seconds else {
        return Err(DashError::InvalidProbeOutput);
    };
    if !expected.is_finite() || expected <= 0.0 || is_notice_duration(expected) {
        return Err(DashError::NoticeMedia);
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
    if is_notice_duration(actual) {
        return Err(DashError::NoticeMedia);
    }
    if (actual - expected).abs() > DURATION_TOLERANCE_SECONDS {
        return Err(DashError::DurationMismatch {
            expected_seconds: expected,
            actual_seconds: actual,
        });
    }
    Ok(())
}

fn require_provider_duration(duration: Option<f64>) -> Result<f64, DashError> {
    let Some(duration) = duration else {
        return Err(DashError::InvalidManifest("provider duration is required"));
    };
    if !duration.is_finite() || duration <= 0.0 || is_notice_duration(duration) {
        return Err(DashError::InvalidManifest("provider duration is invalid"));
    }
    Ok(duration)
}

fn is_notice_duration(duration: f64) -> bool {
    (duration - NOTICE_DURATION_SECONDS).abs() <= NOTICE_DURATION_TOLERANCE_SECONDS
}

pub fn choose_video_stream(bytes: &[u8], maximum_height: u16) -> Result<u32, DashError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| DashError::InvalidProbeOutput)?;
    let streams = value
        .get("streams")
        .and_then(serde_json::Value::as_array)
        .ok_or(DashError::InvalidProbeOutput)?;
    let mut selected = None;
    for stream in streams {
        if stream.get("codec_type").and_then(serde_json::Value::as_str) != Some("video") {
            continue;
        }
        let Some(index) = stream
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .and_then(|index| u32::try_from(index).ok())
        else {
            return Err(DashError::InvalidProbeOutput);
        };
        let Some(height) = stream
            .get("height")
            .and_then(serde_json::Value::as_u64)
            .and_then(|height| u16::try_from(height).ok())
        else {
            return Err(DashError::InvalidProbeOutput);
        };
        if height > maximum_height {
            continue;
        }
        if selected.is_none_or(|(current_height, current_index)| {
            height > current_height || (height == current_height && index < current_index)
        }) {
            selected = Some((height, index));
        }
    }
    selected
        .map(|(_, index)| index)
        .ok_or(DashError::InvalidManifest(
            "no video stream is within the requested height",
        ))
}

/// Safe, redacted representation useful for tests and diagnostics.
pub fn build_ffmpeg_arguments(
    url: &Url,
    _headers: &HeaderMap,
    _maximum_height: u16,
    output: &Path,
) -> Vec<String> {
    private_ffmpeg_arguments(url, 0, output)
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect()
}

fn private_ffmpeg_arguments(url: &Url, video_stream_index: u32, output: &Path) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("-hide_banner"),
        OsString::from("-loglevel"),
        OsString::from("error"),
        OsString::from("-y"),
        OsString::from("-protocol_whitelist"),
        OsString::from("http,tcp,tls,crypto"),
    ];
    args.extend([
        OsString::from("-i"),
        OsString::from(url.as_str()),
        OsString::from("-map"),
        OsString::from(format!("0:{video_stream_index}")),
        OsString::from("-map"),
        OsString::from("0:a:0?"),
        OsString::from("-c"),
        OsString::from("copy"),
        OsString::from("-f"),
        OsString::from("matroska"),
        output.as_os_str().to_owned(),
    ]);
    args
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
