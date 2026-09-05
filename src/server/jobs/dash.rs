//! Guarded MPEG-DASH transfer for authenticated MovieBox sources.
//!
//! The manifest is fetched through the server's DNS/peer validation first,
//! FFmpeg only remuxes (stream copy), and FFprobe must confirm a real video
//! stream before the worker can publish the file.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use quick_xml::{
    Reader, Writer,
    events::{BytesStart, BytesText, Event},
};
use rand::RngExt;
use reqwest::{
    Method, StatusCode,
    header::{
        ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, HeaderMap, HeaderValue,
        LAST_MODIFIED, RANGE,
    },
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{Semaphore, mpsc, oneshot},
    task::{JoinHandle, JoinSet},
    time::{Instant, sleep, timeout_at},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    catalog::SourceTransport,
    download::{DownloadClient, DownloadError, DownloadOutcome, DownloadRequest},
    server::security::net::{NetSecurityError, follow_checked_redirects_same_origin},
};

use super::worker::TransferProgress;

const DEFAULT_FFMPEG: &str = "ffmpeg";
const DEFAULT_FFPROBE: &str = "ffprobe";
const DURATION_TOLERANCE_SECONDS: f64 = 8.0;
const NOTICE_DURATION_SECONDS: f64 = 20.97;
const NOTICE_DURATION_TOLERANCE_SECONDS: f64 = 0.5;
const NOTICE_SIZE_BYTES: u64 = 917_554;
const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
const DASH_PROXY_CONNECTIONS: usize = 1;
const DASH_PROXY_HEADER_TIMEOUT: Duration = Duration::from_millis(250);
const DASH_PROXY_MAX_HEADER_BYTES: usize = 64 * 1024;

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
    #[error("DASH proxy rejected an unsafe upstream request")]
    ProxySecurity,
    #[error("DASH proxy encountered an upstream network failure")]
    ProxyNetwork,
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
        let response = tokio::select! {
            response = follow_checked_redirects_same_origin(
                &self.client,
                Method::GET,
                request.url.clone(),
                request.headers.clone(),
                request.maximum_redirects,
                request.url.clone(),
            ) => response.map_err(DownloadError::from)?,
            _ = cancel.cancelled() => return Ok(DownloadOutcome::Paused { bytes: 0 }),
        };
        if !response.status().is_success() {
            return Err(DashError::Download(DownloadError::Http(response.status())));
        }
        let remote_url = response.url().clone();
        let manifest = match read_bounded_manifest(response, cancel.clone()).await {
            Ok(manifest) => manifest,
            Err(DashError::Cancelled) if cancel.is_cancelled() => {
                return Ok(DownloadOutcome::Paused { bytes: 0 });
            }
            Err(error) => return Err(error),
        };
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

        let mut proxy = DashProxy::start(
            self.client.clone(),
            manifest,
            remote_url,
            request.headers.clone(),
            request.maximum_redirects,
            cancel.clone(),
        )
        .await?;
        validate_proxy_tool_url(&proxy.url)?;
        let result = async {
            let selected_video_stream = match self
                .probe_video_stream(&proxy.url, maximum_height, cancel.clone())
                .await
            {
                Ok(index) => index,
                Err(DashError::Cancelled) if cancel.is_cancelled() => {
                    return Ok(DownloadOutcome::Paused { bytes: 0 });
                }
                Err(error) => return Err(proxy_error_or(error, &proxy)),
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
                return Err(proxy_error_or(
                    DashError::ToolFailed {
                        tool: "ffmpeg",
                        status: status.code(),
                    },
                    &proxy,
                ));
            }
            if let Some(error) = proxy_error(&proxy) {
                let _ = fs::remove_file(&temporary).await;
                return Err(error);
            }

            let probe = match self.probe(&temporary, cancel.clone()).await {
                Ok(probe) => probe,
                Err(DashError::Cancelled) if cancel.is_cancelled() => {
                    let _ = fs::remove_file(&temporary).await;
                    return Ok(DownloadOutcome::Paused { bytes: 0 });
                }
                Err(error) => {
                    let _ = fs::remove_file(&temporary).await;
                    return Err(proxy_error_or(error, &proxy));
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
        .await;
        proxy.shutdown().await;
        result
    }

    async fn probe_video_stream(
        &self,
        manifest_url: &Url,
        maximum_height: u16,
        cancel: CancellationToken,
    ) -> Result<u32, DashError> {
        validate_proxy_tool_url(manifest_url)?;
        let mut child = Command::new(&self.ffprobe)
            .args([
                OsString::from("-v"),
                OsString::from("error"),
                OsString::from("-show_entries"),
                OsString::from("stream=index,codec_type,height"),
                OsString::from("-of"),
                OsString::from("json"),
                OsString::from("-protocol_whitelist"),
                OsString::from("http,tcp,tls,crypto"),
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
                OsString::from("-protocol_whitelist"),
                OsString::from("file"),
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

fn validate_proxy_tool_url(url: &Url) -> Result<(), DashError> {
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.path().starts_with("/__dash/")
        || !url.path().ends_with("/manifest.mpd")
    {
        return Err(DashError::UnsafeManifestReference);
    }
    Ok(())
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

#[derive(Clone, Debug)]
struct DashCapability {
    path_template: String,
    query_template: Option<String>,
    route_path_template: String,
    route_query_template: Option<String>,
    representation_ids: Option<Vec<String>>,
    path_pattern: DashTemplate,
    query_pattern: Option<DashTemplate>,
}

#[derive(Clone, Debug)]
struct DashTemplate {
    pieces: Vec<DashTemplatePiece>,
}

#[derive(Clone, Debug)]
enum DashTemplatePiece {
    Literal(String),
    LiteralDollar,
    Variable {
        kind: DashVariable,
        width: Option<usize>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DashVariable {
    Number,
    Time,
    Bandwidth,
    RepresentationId,
}

impl DashVariable {
    fn name(self) -> &'static str {
        match self {
            Self::Number => "Number",
            Self::Time => "Time",
            Self::Bandwidth => "Bandwidth",
            Self::RepresentationId => "RepresentationID",
        }
    }

    fn numeric(self) -> bool {
        !matches!(self, Self::RepresentationId)
    }
}

#[derive(Clone, Debug)]
struct ParsedDashManifest {
    bytes: Vec<u8>,
    capabilities: Vec<DashCapability>,
}

#[derive(Clone, Copy, Debug)]
enum ProxyFailure {
    Auth(StatusCode),
    Security,
    Network,
}

#[derive(Clone)]
struct DashProxyState {
    client: DownloadClient,
    manifest: Arc<Vec<u8>>,
    manifest_path: String,
    token: String,
    origin: Url,
    capabilities: Arc<Vec<DashCapability>>,
    headers: HeaderMap,
    maximum_redirects: u8,
    failure: Arc<Mutex<Option<ProxyFailure>>>,
    cancel: CancellationToken,
    connections: Arc<Semaphore>,
}

struct DashProxy {
    url: Url,
    failure: Arc<Mutex<Option<ProxyFailure>>>,
    cancel: CancellationToken,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl DashProxy {
    async fn start(
        client: DownloadClient,
        manifest: Vec<u8>,
        remote_url: Url,
        headers: HeaderMap,
        maximum_redirects: u8,
        cancel: CancellationToken,
    ) -> Result<Self, DashError> {
        let proxy_cancel = cancel.child_token();
        let token = random_proxy_token();
        let parsed = parse_dash_manifest(&manifest, &remote_url, &token)?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let mut origin = remote_url.clone();
        origin.set_path("/");
        origin.set_query(None);
        origin.set_fragment(None);
        let manifest_path = format!("/__dash/{token}/manifest.mpd");
        let failure = Arc::new(Mutex::new(None));
        let state = DashProxyState {
            client,
            manifest: Arc::new(parsed.bytes),
            manifest_path: manifest_path.clone(),
            token: token.clone(),
            origin,
            capabilities: Arc::new(parsed.capabilities),
            headers,
            maximum_redirects,
            failure: Arc::clone(&failure),
            cancel: proxy_cancel.clone(),
            connections: Arc::new(Semaphore::new(DASH_PROXY_CONNECTIONS)),
        };
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let task_cancel = proxy_cancel.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                while connections.try_join_next().is_some() {}
                let permit = tokio::select! {
                    _ = &mut shutdown_rx => break,
                    _ = task_cancel.cancelled() => break,
                    permit = state.connections.clone().acquire_owned() => match permit {
                        Ok(permit) => permit,
                        Err(_) => break,
                    },
                };
                let accepted = tokio::select! {
                    _ = &mut shutdown_rx => {
                        drop(permit);
                        break;
                    }
                    _ = task_cancel.cancelled() => {
                        drop(permit);
                        break;
                    }
                    accepted = listener.accept() => accepted,
                };
                let (stream, _) = match accepted {
                    Ok(value) => value,
                    Err(_) => {
                        drop(permit);
                        record_proxy_failure(&state.failure, ProxyFailure::Network);
                        break;
                    }
                };
                let state = state.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    let _ = serve_dash_proxy_connection(stream, state).await;
                });
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        let url = Url::parse(&format!(
            "http://127.0.0.1:{}{}",
            address.port(),
            manifest_path
        ))
        .map_err(|_| DashError::InvalidManifest("proxy URL is invalid"))?;
        Ok(Self {
            url,
            failure,
            cancel: proxy_cancel,
            shutdown: Some(shutdown_tx),
            task: Some(task),
        })
    }

    async fn shutdown(&mut self) {
        self.cancel.cancel();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }

    fn failure(&self) -> Option<ProxyFailure> {
        self.failure.lock().ok().and_then(|failure| *failure)
    }
}

impl Drop for DashProxy {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn serve_dash_proxy_connection(
    mut stream: TcpStream,
    state: DashProxyState,
) -> Result<(), std::io::Error> {
    let request = read_dash_proxy_request(&mut stream, state.cancel.clone()).await?;
    if request.method != Method::GET {
        write_dash_proxy_headers(
            &mut stream,
            StatusCode::METHOD_NOT_ALLOWED,
            &HeaderMap::new(),
            Some(0),
        )
        .await?;
        return Ok(());
    }
    if request.target == state.manifest_path {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/dash+xml"),
        );
        write_dash_proxy_headers(
            &mut stream,
            StatusCode::OK,
            &headers,
            Some(state.manifest.len() as u64),
        )
        .await?;
        tokio::select! {
            result = stream.write_all(&state.manifest) => result?,
            _ = state.cancel.cancelled() => return Ok(()),
        }
        return Ok(());
    }

    let local_url = Url::parse(&format!("http://dash-proxy.invalid{}", request.target))
        .map_err(|_| std::io::Error::other("invalid proxy request target"))?;
    let upstream = match proxy_target_url(&state, &local_url) {
        Ok(upstream) => upstream,
        Err(_) => {
            write_dash_proxy_headers(
                &mut stream,
                StatusCode::NOT_FOUND,
                &HeaderMap::new(),
                Some(0),
            )
            .await?;
            return Ok(());
        }
    };
    let mut headers = state.headers.clone();
    if let Some(range) = request.headers.get("range") {
        if let Ok(value) = HeaderValue::from_str(range) {
            headers.insert(RANGE, value);
        }
    }
    let response = tokio::select! {
        response = follow_checked_redirects_same_origin(
            &state.client,
            Method::GET,
            upstream,
            headers,
            state.maximum_redirects,
            state.origin.clone(),
        ) => response,
        _ = state.cancel.cancelled() => return Ok(()),
    };
    match response {
        Ok(mut response) => {
            let status = response.status();
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                record_proxy_failure(&state.failure, ProxyFailure::Auth(status));
            }
            let response_headers = response.headers().clone();
            let body_length = response.content_length();
            write_dash_proxy_headers(&mut stream, status, &response_headers, body_length).await?;
            if status.is_success() || status == StatusCode::PARTIAL_CONTENT {
                loop {
                    let chunk = tokio::select! {
                        chunk = response.chunk() => match chunk {
                            Ok(chunk) => chunk,
                            Err(error) => {
                                record_proxy_failure(&state.failure, ProxyFailure::Network);
                                return Err(std::io::Error::other(error.to_string()));
                            }
                        },
                        _ = state.cancel.cancelled() => return Ok(()),
                    };
                    let Some(chunk) = chunk else { break };
                    tokio::select! {
                        result = stream.write_all(&chunk) => result?,
                        _ = state.cancel.cancelled() => return Ok(()),
                    }
                }
            }
        }
        Err(error) => {
            record_proxy_failure(&state.failure, classify_proxy_failure(&error));
            write_dash_proxy_headers(
                &mut stream,
                StatusCode::BAD_GATEWAY,
                &HeaderMap::new(),
                Some(0),
            )
            .await?;
        }
    }
    Ok(())
}

fn classify_proxy_failure(error: &NetSecurityError) -> ProxyFailure {
    match error {
        NetSecurityError::Request(_) => ProxyFailure::Network,
        _ => ProxyFailure::Security,
    }
}

fn record_proxy_failure(failure: &Arc<Mutex<Option<ProxyFailure>>>, next: ProxyFailure) {
    if let Ok(mut current) = failure.lock() {
        if current.is_none() {
            *current = Some(next);
        }
    }
}

fn proxy_error(proxy: &DashProxy) -> Option<DashError> {
    match proxy.failure()? {
        ProxyFailure::Auth(status) => Some(DashError::Download(DownloadError::Http(status))),
        ProxyFailure::Security => Some(DashError::ProxySecurity),
        ProxyFailure::Network => Some(DashError::ProxyNetwork),
    }
}

fn proxy_error_or(error: DashError, proxy: &DashProxy) -> DashError {
    proxy_error(proxy).unwrap_or(error)
}

#[derive(Debug)]
struct DashProxyRequest {
    method: Method,
    target: String,
    headers: std::collections::HashMap<String, String>,
}

async fn read_dash_proxy_request(
    stream: &mut TcpStream,
    cancel: CancellationToken,
) -> Result<DashProxyRequest, std::io::Error> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    let deadline = Instant::now() + DASH_PROXY_HEADER_TIMEOUT;
    loop {
        let read = tokio::select! {
            read = timeout_at(deadline, stream.read(&mut chunk)) => {
                read.map_err(|_| std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "proxy request headers timed out",
                ))??
            }
            _ = cancel.cancelled() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "proxy request cancelled",
                ));
            }
        };
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "proxy request ended before headers",
            ));
        }
        if bytes.len().saturating_add(read) > DASH_PROXY_MAX_HEADER_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "proxy request headers are too large",
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
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

async fn write_dash_proxy_headers(
    stream: &mut TcpStream,
    status: StatusCode,
    upstream_headers: &HeaderMap,
    body_length: Option<u64>,
) -> Result<(), std::io::Error> {
    let reason = status.canonical_reason().unwrap_or("error");
    let mut response = format!("HTTP/1.1 {} {}\r\n", status.as_u16(), reason);
    let selected = [
        CONTENT_RANGE,
        ACCEPT_RANGES,
        CONTENT_TYPE,
        ETAG,
        LAST_MODIFIED,
    ];
    for name in selected {
        if let Some(value) = upstream_headers
            .get(&name)
            .and_then(|value| value.to_str().ok())
        {
            response.push_str(name.as_str());
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
    }
    let length = if status.is_success() || status == StatusCode::PARTIAL_CONTENT {
        body_length.or_else(|| {
            upstream_headers
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok()?.parse().ok())
        })
    } else {
        Some(0)
    };
    if let Some(length) = length {
        response.push_str(&format!("Content-Length: {length}\r\n"));
    }
    response.push_str("Connection: close\r\n\r\n");
    stream.write_all(response.as_bytes()).await
}

fn proxy_target_url(state: &DashProxyState, local_url: &Url) -> Result<Url, DashError> {
    let prefix = local_url
        .path()
        .strip_prefix("/__dash/")
        .ok_or(DashError::UnsafeManifestReference)?;
    let (token, path) = prefix
        .split_once('/')
        .ok_or(DashError::UnsafeManifestReference)?;
    if token != state.token {
        return Err(DashError::UnsafeManifestReference);
    }
    let path = format!("/{path}");
    if path.split('/').any(|segment| {
        segment == "."
            || segment == ".."
            || segment.eq_ignore_ascii_case("%2e")
            || segment.eq_ignore_ascii_case("%2e%2e")
    }) {
        return Err(DashError::UnsafeManifestReference);
    }
    let query = local_url.query().map(str::to_owned);
    if !state
        .capabilities
        .iter()
        .any(|capability| capability.matches(&path, query.as_deref()))
    {
        return Err(DashError::UnsafeManifestReference);
    }
    let mut upstream = state
        .origin
        .join(path.trim_start_matches('/'))
        .map_err(|_| DashError::UnsafeManifestReference)?;
    upstream.set_query(query.as_deref());
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

async fn read_bounded_manifest(
    mut response: crate::server::security::net::DownloadResponse,
    cancel: CancellationToken,
) -> Result<Vec<u8>, DashError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MANIFEST_BYTES as u64)
    {
        return Err(DashError::InvalidManifest("manifest is too large"));
    }
    let mut body = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or_default()
            .min(MAX_MANIFEST_BYTES as u64) as usize,
    );
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(DownloadError::from)?,
            _ = cancel.cancelled() => return Err(DashError::Cancelled),
        };
        let Some(chunk) = chunk else { break };
        if body.len().saturating_add(chunk.len()) > MAX_MANIFEST_BYTES {
            return Err(DashError::InvalidManifest("manifest is too large"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
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

fn random_proxy_token() -> String {
    let mut bytes = [0_u8; 24];
    rand::rng().fill(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl DashCapability {
    fn new(
        path_template: String,
        query_template: Option<String>,
        representation_ids: Option<Vec<String>>,
    ) -> Result<Self, DashError> {
        let path_pattern = DashTemplate::parse(&path_template)?;
        let query_pattern = query_template
            .as_deref()
            .map(DashTemplate::parse)
            .transpose()?;
        if representation_ids.is_none()
            && (path_pattern.contains(DashVariable::RepresentationId)
                || query_pattern
                    .as_ref()
                    .is_some_and(|pattern| pattern.contains(DashVariable::RepresentationId)))
        {
            return Err(DashError::InvalidManifest(
                "RepresentationID requires a declared Representation",
            ));
        }
        Ok(Self {
            path_template: path_pattern.render(),
            query_template: query_pattern.as_ref().map(DashTemplate::render),
            route_path_template: path_pattern.render_route(),
            route_query_template: query_pattern.as_ref().map(DashTemplate::render_route),
            representation_ids,
            path_pattern,
            query_pattern,
        })
    }

    fn matches(&self, path: &str, query: Option<&str>) -> bool {
        self.path_pattern
            .matches(path, false, self.representation_ids.as_deref())
            && match (&self.query_template, query) {
                (None, None) => true,
                (Some(_), Some(actual)) => self.query_pattern.as_ref().is_some_and(|pattern| {
                    pattern.matches(actual, true, self.representation_ids.as_deref())
                }),
                _ => false,
            }
    }
}

#[cfg(test)]
fn template_matches(template: &str, value: &str) -> bool {
    DashTemplate::parse(template)
        .map(|pattern| pattern.matches(value, false, None))
        .unwrap_or(false)
}

impl DashTemplate {
    fn parse(template: &str) -> Result<Self, DashError> {
        let mut pieces = Vec::new();
        let mut cursor = 0;
        let mut literal_start = 0;
        let bytes = template.as_bytes();
        while cursor < bytes.len() {
            if bytes[cursor] != b'$' {
                cursor += 1;
                continue;
            }
            if cursor + 1 < bytes.len() && bytes[cursor + 1] == b'$' {
                if literal_start < cursor {
                    pieces.push(DashTemplatePiece::Literal(
                        template[literal_start..cursor].to_owned(),
                    ));
                }
                pieces.push(DashTemplatePiece::LiteralDollar);
                cursor += 2;
                literal_start = cursor;
                continue;
            }
            if literal_start < cursor {
                pieces.push(DashTemplatePiece::Literal(
                    template[literal_start..cursor].to_owned(),
                ));
            }
            let Some(end_offset) = template[cursor + 1..].find('$') else {
                return Err(DashError::InvalidManifest(
                    "unbalanced DASH template variable",
                ));
            };
            let end = cursor + 1 + end_offset;
            let token = &template[cursor + 1..end];
            let variable = parse_dash_variable(token)?;
            if matches!(pieces.last(), Some(DashTemplatePiece::Variable { .. })) {
                return Err(DashError::InvalidManifest(
                    "adjacent DASH template variables",
                ));
            }
            pieces.push(DashTemplatePiece::Variable {
                kind: variable.0,
                width: variable.1,
            });
            cursor = end + 1;
            literal_start = cursor;
        }
        if literal_start < template.len() {
            pieces.push(DashTemplatePiece::Literal(
                template[literal_start..].to_owned(),
            ));
        }
        Ok(Self { pieces })
    }

    fn render(&self) -> String {
        let mut rendered = String::new();
        for piece in &self.pieces {
            match piece {
                DashTemplatePiece::Literal(value) => rendered.push_str(value),
                DashTemplatePiece::LiteralDollar => rendered.push('$'),
                DashTemplatePiece::Variable { kind, width } => {
                    rendered.push('$');
                    rendered.push_str(kind.name());
                    if let Some(width) = width {
                        rendered.push_str(&format!("%0{width}d"));
                    }
                    rendered.push('$');
                }
            }
        }
        rendered
    }

    fn render_route(&self) -> String {
        let mut rendered = String::new();
        for piece in &self.pieces {
            match piece {
                DashTemplatePiece::Literal(value) => rendered.push_str(value),
                DashTemplatePiece::LiteralDollar => rendered.push_str("$$"),
                DashTemplatePiece::Variable { kind, width } => {
                    rendered.push('$');
                    rendered.push_str(kind.name());
                    if let Some(width) = width {
                        rendered.push_str(&format!("%0{width}d"));
                    }
                    rendered.push('$');
                }
            }
        }
        rendered
    }

    fn contains(&self, wanted: DashVariable) -> bool {
        self.pieces.iter().any(
            |piece| matches!(piece, DashTemplatePiece::Variable { kind, .. } if *kind == wanted),
        )
    }

    fn matches(&self, value: &str, query: bool, representation_ids: Option<&[String]>) -> bool {
        let mut offset = 0;
        for (index, piece) in self.pieces.iter().enumerate() {
            match piece {
                DashTemplatePiece::Literal(literal) => {
                    if !value[offset..].starts_with(literal) {
                        return false;
                    }
                    offset += literal.len();
                }
                DashTemplatePiece::LiteralDollar => {
                    if !value[offset..].starts_with('$') {
                        return false;
                    }
                    offset += '$'.len_utf8();
                }
                DashTemplatePiece::Variable { kind, width } => {
                    let next_literal = self.pieces[index + 1..].iter().find_map(|piece| {
                        if let DashTemplatePiece::Literal(literal) = piece {
                            (!literal.is_empty()).then_some(literal.as_str())
                        } else {
                            None
                        }
                    });
                    let end = next_literal
                        .and_then(|literal| {
                            value[offset..].find(literal).map(|index| offset + index)
                        })
                        .unwrap_or(value.len());
                    let candidate = &value[offset..end];
                    if end < offset
                        || !valid_dash_variable(*kind, *width, candidate, query)
                        || (*kind == DashVariable::RepresentationId
                            && representation_ids
                                .is_some_and(|ids| !ids.iter().any(|id| id == candidate)))
                    {
                        return false;
                    }
                    offset = end;
                }
            }
        }
        offset == value.len()
    }
}

fn parse_dash_variable(token: &str) -> Result<(DashVariable, Option<usize>), DashError> {
    let (name, format) = token.split_once('%').unwrap_or((token, ""));
    let kind = match name {
        "Number" => DashVariable::Number,
        "Time" => DashVariable::Time,
        "Bandwidth" => DashVariable::Bandwidth,
        "RepresentationID" => DashVariable::RepresentationId,
        _ => {
            return Err(DashError::InvalidManifest(
                "unsupported DASH template variable",
            ));
        }
    };
    if format.is_empty() {
        return Ok((kind, None));
    }
    if !kind.numeric() || !(format.ends_with('d') || format.ends_with('i')) {
        return Err(DashError::InvalidManifest("invalid DASH template format"));
    }
    let digits = &format[..format.len() - 1];
    if digits.is_empty() {
        return Ok((kind, None));
    }
    let width = digits
        .parse::<usize>()
        .ok()
        .filter(|width| *width > 0 && *width <= 64)
        .ok_or(DashError::InvalidManifest("invalid DASH template format"))?;
    Ok((kind, Some(width)))
}

fn valid_dash_variable(kind: DashVariable, width: Option<usize>, value: &str, query: bool) -> bool {
    if value.is_empty() {
        return false;
    }
    if kind.numeric() {
        value.bytes().all(|byte| byte.is_ascii_digit())
            && width.is_none_or(|width| value.len() >= width)
    } else {
        !value.contains(['/', '?', '#', '\\', '%'])
            && (!query || !value.contains('&'))
            && value != "."
            && value != ".."
    }
}

fn parse_dash_manifest(
    bytes: &[u8],
    remote_url: &Url,
    token: &str,
) -> Result<ParsedDashManifest, DashError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(DashError::InvalidManifest("manifest is too large"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| DashError::InvalidManifest("not UTF-8"))?;
    let lowered = text.to_ascii_lowercase();
    if lowered.contains("<!") {
        return Err(DashError::UnsafeManifestReference);
    }

    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    reader.config_mut().check_end_names = true;
    let mut writer = Writer::new(Vec::with_capacity(bytes.len()));
    let mut stack = Vec::<ManifestElement>::new();
    let mut capabilities = Vec::new();
    let mut root_seen = false;
    let mut root_closed = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|_| DashError::InvalidManifest("malformed XML"))?;
        match event {
            Event::Start(start) => {
                let name = xml_name(start.name().as_ref())?.to_string();
                if root_closed || (stack.is_empty() && root_seen) {
                    return Err(DashError::InvalidManifest("multiple XML roots"));
                }
                if stack.is_empty() {
                    if name != "MPD" {
                        return Err(DashError::InvalidManifest("missing MPD root"));
                    }
                    root_seen = true;
                }
                let parent_base = stack
                    .last()
                    .map(|element| {
                        if name == "BaseURL" {
                            element.inherited_base.clone()
                        } else {
                            element.base.clone()
                        }
                    })
                    .unwrap_or_else(|| remote_url.clone());
                let representation_id = representation_context(&start, &stack)?;
                let (rewritten, is_base) = rewrite_manifest_start(
                    &start,
                    &parent_base,
                    remote_url,
                    token,
                    representation_id.as_deref(),
                    &mut capabilities,
                )?;
                writer
                    .write_event(Event::Start(rewritten))
                    .map_err(|_| DashError::InvalidManifest("could not rewrite manifest"))?;
                stack.push(ManifestElement {
                    name: name.to_string(),
                    base: parent_base,
                    inherited_base: stack
                        .last()
                        .map(|element| element.base.clone())
                        .unwrap_or_else(|| remote_url.clone()),
                    has_base_url: false,
                    is_base,
                    base_text: String::new(),
                    representation_id,
                });
            }
            Event::Empty(start) => {
                let name = xml_name(start.name().as_ref())?.to_string();
                if stack.is_empty() && root_seen {
                    return Err(DashError::InvalidManifest("multiple XML roots"));
                }
                if stack.is_empty() {
                    if name != "MPD" {
                        return Err(DashError::InvalidManifest("missing MPD root"));
                    }
                    root_seen = true;
                    root_closed = true;
                }
                let parent_base = stack
                    .last()
                    .map(|element| {
                        if name == "BaseURL" {
                            element.inherited_base.clone()
                        } else {
                            element.base.clone()
                        }
                    })
                    .unwrap_or_else(|| remote_url.clone());
                let representation_id = representation_context(&start, &stack)?;
                if name == "BaseURL" {
                    return Err(DashError::InvalidManifest("empty BaseURL"));
                }
                let (rewritten, _) = rewrite_manifest_start(
                    &start,
                    &parent_base,
                    remote_url,
                    token,
                    representation_id.as_deref(),
                    &mut capabilities,
                )?;
                writer
                    .write_event(Event::Empty(rewritten))
                    .map_err(|_| DashError::InvalidManifest("could not rewrite manifest"))?;
            }
            Event::Text(text_event) => {
                let text_value = text_event.as_ref();
                if stack.is_empty() && root_closed && !text_value.trim().is_empty() {
                    return Err(DashError::InvalidManifest("text outside MPD root"));
                }
                if let Some(element) = stack.last_mut()
                    && element.is_base
                {
                    element
                        .base_text
                        .push_str(&decode_xml_entities(text_value)?);
                } else {
                    reject_external_text(&decode_xml_entities(text_value)?)?;
                    writer
                        .write_event(Event::Text(text_event))
                        .map_err(|_| DashError::InvalidManifest("could not rewrite manifest"))?;
                }
            }
            Event::GeneralRef(reference) => {
                if stack.is_empty() && root_closed {
                    return Err(DashError::InvalidManifest("reference outside MPD root"));
                }
                let value = decode_xml_entities(&format!("&{};", reference.as_ref()))?;
                if let Some(element) = stack.last_mut()
                    && element.is_base
                {
                    element.base_text.push_str(&value);
                } else {
                    return Err(DashError::UnsafeManifestReference);
                }
            }
            Event::End(end) => {
                let name = xml_name(end.name().as_ref())?.to_string();
                let Some(element) = stack.pop() else {
                    return Err(DashError::InvalidManifest("unbalanced XML"));
                };
                if element.name != name {
                    return Err(DashError::InvalidManifest("unbalanced XML"));
                }
                if element.is_base {
                    let parent_base = stack
                        .last()
                        .map(|parent| parent.base.clone())
                        .unwrap_or_else(|| remote_url.clone());
                    let value = element.base_text.trim();
                    let (route, target) = canonicalize_dash_reference(
                        value,
                        &parent_base,
                        remote_url,
                        token,
                        stack
                            .last()
                            .and_then(|element| element.representation_id.as_deref()),
                        &mut capabilities,
                    )?;
                    writer
                        .write_event(Event::Text(BytesText::new(&route)))
                        .map_err(|_| DashError::InvalidManifest("could not rewrite manifest"))?;
                    if let Some(parent) = stack.last_mut() {
                        if parent.has_base_url {
                            return Err(DashError::InvalidManifest(
                                "multiple sibling BaseURL alternatives are unsupported",
                            ));
                        }
                        parent.base = target;
                        parent.has_base_url = true;
                    }
                }
                writer
                    .write_event(Event::End(end))
                    .map_err(|_| DashError::InvalidManifest("could not rewrite manifest"))?;
                if stack.is_empty() {
                    root_closed = true;
                }
            }
            Event::Decl(decl) => {
                writer
                    .write_event(Event::Decl(decl))
                    .map_err(|_| DashError::InvalidManifest("could not rewrite manifest"))?;
            }
            Event::Comment(_) | Event::CData(_) | Event::PI(_) | Event::DocType(_) => {
                return Err(DashError::UnsafeManifestReference);
            }
            Event::Eof => {
                if !root_seen || !root_closed || !stack.is_empty() {
                    return Err(DashError::InvalidManifest("missing MPD root"));
                }
                break;
            }
        }
    }
    Ok(ParsedDashManifest {
        bytes: writer.into_inner(),
        capabilities,
    })
}

#[derive(Debug)]
struct ManifestElement {
    name: String,
    base: Url,
    inherited_base: Url,
    has_base_url: bool,
    is_base: bool,
    base_text: String,
    representation_id: Option<String>,
}

fn representation_context(
    start: &BytesStart<'_>,
    stack: &[ManifestElement],
) -> Result<Option<String>, DashError> {
    if xml_name(start.local_name().as_ref())? != "Representation" {
        return Ok(stack
            .last()
            .and_then(|element| element.representation_id.clone()));
    }
    let mut id = None;
    for attribute in start.attributes().with_checks(true) {
        let attribute =
            attribute.map_err(|_| DashError::InvalidManifest("malformed XML attribute"))?;
        if xml_name(attribute.key.as_ref())? != "id" {
            continue;
        }
        if id.is_some() {
            return Err(DashError::InvalidManifest("duplicate Representation id"));
        }
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|_| DashError::UnsafeManifestReference)?
            .into_owned();
        let value = decode_xml_entities(&value)?;
        if !valid_dash_variable(DashVariable::RepresentationId, None, &value, false) {
            return Err(DashError::UnsafeManifestReference);
        }
        id = Some(value);
    }
    id.ok_or(DashError::InvalidManifest(
        "Representation requires a declared id",
    ))
    .map(Some)
}

fn xml_name(name: &str) -> Result<&str, DashError> {
    if name.is_empty() {
        Err(DashError::InvalidManifest("invalid XML name"))
    } else {
        Ok(name)
    }
}

fn rewrite_manifest_start(
    start: &BytesStart<'_>,
    base: &Url,
    remote_url: &Url,
    token: &str,
    representation_id: Option<&str>,
    capabilities: &mut Vec<DashCapability>,
) -> Result<(BytesStart<'static>, bool), DashError> {
    let name = xml_name(start.name().as_ref())?.to_string();
    let is_base = xml_name(start.local_name().as_ref())? == "BaseURL";
    let mut output = BytesStart::new(name);
    for attribute in start.attributes().with_checks(true) {
        let attribute =
            attribute.map_err(|_| DashError::InvalidManifest("malformed XML attribute"))?;
        let key = xml_name(attribute.key.as_ref())?.to_string();
        if key == "xsi:schemaLocation" {
            continue;
        }
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|_| DashError::UnsafeManifestReference)?
            .into_owned();
        let value = decode_xml_entities(&value)?;
        let rewritten = if matches!(
            key.as_str(),
            "media" | "initialization" | "sourceURL" | "index"
        ) {
            canonicalize_dash_reference(
                value.trim(),
                base,
                remote_url,
                token,
                representation_id,
                capabilities,
            )?
            .0
        } else {
            if !key.starts_with("xmlns") {
                reject_external_text(&value)?;
            }
            value
        };
        let escaped = quick_xml::escape::escape(&rewritten).into_owned();
        output.push_attribute((key.as_str(), escaped.as_str()));
    }
    Ok((output, is_base))
}

fn canonicalize_dash_reference(
    value: &str,
    base: &Url,
    remote_url: &Url,
    token: &str,
    representation_id: Option<&str>,
    capabilities: &mut Vec<DashCapability>,
) -> Result<(String, Url), DashError> {
    let value = decode_xml_entities(value.trim())?;
    if value.is_empty() {
        return Ok((String::new(), base.clone()));
    }
    let value_pattern = DashTemplate::parse(&value)?;
    let canonical_value = value_pattern.render_route();
    if is_unsafe_scheme(&value) || value.starts_with("//") {
        return Err(DashError::UnsafeManifestReference);
    }
    let mut target = base
        .join(&canonical_value)
        .map_err(|_| DashError::UnsafeManifestReference)?;
    if !matches!(target.scheme(), "http" | "https") || target.origin() != remote_url.origin() {
        return Err(DashError::UnsafeManifestReference);
    }
    target.set_fragment(None);
    let path = target.path().to_string();
    let reference_path = value.split(['?', '#']).next().unwrap_or_default();
    if path.to_ascii_lowercase().ends_with(".mpd")
        && (!reference_path.is_empty() || path != remote_url.path())
    {
        return Err(DashError::UnsafeManifestReference);
    }
    let query_template = target.query().map(str::to_owned);
    let capability = DashCapability::new(
        path,
        query_template,
        representation_id.map(|value| vec![value.to_owned()]),
    )?;
    let route_path_template = capability.route_path_template.clone();
    let route_query_template = capability.route_query_template.clone();
    if !capabilities.iter().any(|existing| {
        existing.path_template == capability.path_template
            && existing.query_template == capability.query_template
            && existing.representation_ids == capability.representation_ids
    }) {
        capabilities.push(capability);
    }
    let mut route = format!("/__dash/{token}{route_path_template}");
    if let Some(query) = route_query_template {
        route.push('?');
        route.push_str(&query);
    }
    Ok((route, target))
}

fn decode_xml_entities(value: &str) -> Result<String, DashError> {
    let mut output = value.to_owned();
    for _ in 0..4 {
        if !output.contains(';') {
            break;
        }
        let decoded = quick_xml::escape::unescape_with(&output, |entity| {
            quick_xml::escape::resolve_predefined_entity(entity)
        })
        .map_err(|_| DashError::UnsafeManifestReference)?
        .into_owned();
        if decoded == output {
            break;
        }
        output = decoded;
    }
    if output.contains("<!") || has_unresolved_entity_reference(&output) {
        return Err(DashError::UnsafeManifestReference);
    }
    Ok(output)
}

fn has_unresolved_entity_reference(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.windows(2).enumerate().any(|(index, pair)| {
        if pair[0] != b'&' || !(pair[1].is_ascii_alphanumeric() || pair[1] == b'#') {
            return false;
        }
        value[index + 2..].find(';').is_some()
    })
}

fn reject_external_text(value: &str) -> Result<(), DashError> {
    let value = value.trim();
    if value.is_empty() || is_unsafe_scheme(value) {
        if is_unsafe_scheme(value) {
            return Err(DashError::UnsafeManifestReference);
        }
        return Ok(());
    }
    if value.starts_with("//") || value.contains("://") {
        return Err(DashError::UnsafeManifestReference);
    }
    Ok(())
}

/// Validate the fetched manifest before handing it to FFmpeg.
pub fn validate_manifest(bytes: &[u8]) -> Result<(), DashError> {
    let placeholder = Url::parse("https://manifest.invalid/index.mpd")
        .map_err(|_| DashError::InvalidManifest("invalid parser origin"))?;
    parse_dash_manifest(bytes, &placeholder, "validation").map(|_| ())
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

#[cfg(test)]
mod tests {
    use super::{DashCapability, parse_dash_manifest, template_matches};
    use std::net::Ipv4Addr;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        time::{Duration, timeout},
    };
    use tokio_util::sync::CancellationToken;
    use url::Url;

    #[test]
    fn capability_templates_preserve_dash_variables_and_query_strings() {
        assert!(template_matches(
            "/segments/$Number$.m4s",
            "/segments/17.m4s"
        ));
        assert!(template_matches("a=$Time$&b=fixture", "a=900&b=fixture"));
        assert!(!template_matches("/segments/$Number$.m4s", "/other/17.m4s"));
        let manifest = br#"<MPD><Period><BaseURL>media/</BaseURL><AdaptationSet><SegmentTemplate media="seg-$Number$.m4s?x=1&amp;y=$Time$" /></AdaptationSet></Period></MPD>"#;
        let remote = Url::parse("https://cdn.example.invalid/path/index.mpd").unwrap();
        let parsed = parse_dash_manifest(manifest, &remote, "opaque-token").unwrap();
        assert!(
            parsed
                .bytes
                .windows(b"/__dash/opaque-token/path/media/seg-".len())
                .any(|window| window == b"/__dash/opaque-token/path/media/seg-")
        );
        assert!(
            parsed
                .bytes
                .windows(b"&amp;".len())
                .any(|window| window == b"&amp;")
        );
        assert!(parsed.capabilities.iter().any(|capability| {
            capability.path_template == "/path/media/seg-$Number$.m4s"
                && capability.query_template.as_deref() == Some("x=1&y=$Time$")
        }));

        let literal_dollar =
            br#"<MPD><Period><SegmentTemplate media="segment-$$.m4s" /></Period></MPD>"#;
        let literal = parse_dash_manifest(literal_dollar, &remote, "opaque-token").unwrap();
        assert!(
            literal
                .bytes
                .windows(b"segment-$$.m4s".len())
                .any(|window| { window == b"segment-$$.m4s" })
        );
        assert!(literal.capabilities[0].matches("/path/segment-$.m4s", None));
    }

    #[test]
    fn capability_matching_requires_an_exact_opaque_token_route() {
        let capability =
            DashCapability::new("/segments/$Number$.m4s".to_string(), None, None).unwrap();
        assert!(capability.matches("/segments/3.m4s", None));
        assert!(!capability.matches("/segments/3.m4s", Some("x=1")));
    }

    #[test]
    fn capability_templates_enforce_dash_variable_grammar_and_safe_values() {
        let remote = Url::parse("https://cdn.example.invalid/index.mpd").unwrap();
        let numeric = parse_dash_manifest(
            br#"<MPD><Period><SegmentTemplate media="seg-$Number%05d$.m4s" /></Period></MPD>"#,
            &remote,
            "opaque-token",
        )
        .unwrap();
        let numeric = &numeric.capabilities[0];
        assert!(numeric.matches("/seg-00017.m4s", None));
        assert!(!numeric.matches("/seg-1/secret.m4s", None));
        assert!(!numeric.matches("/seg-%2fsecret.m4s", None));
        assert!(!numeric.matches("/seg-../secret.m4s", None));

        let literal = parse_dash_manifest(
            br#"<MPD><Period><SegmentTemplate media="seg-$$.m4s" /></Period></MPD>"#,
            &remote,
            "opaque-token",
        )
        .unwrap();
        assert_eq!(literal.capabilities[0].path_template, "/seg-$.m4s");

        for value in [
            br#"<MPD><Period><SegmentTemplate media="seg-$Unknown$.m4s" /></Period></MPD>"#
                .as_slice(),
            br#"<MPD><Period><SegmentTemplate media="seg-$Number.m4s" /></Period></MPD>"#
                .as_slice(),
            br#"<MPD><Period><SegmentTemplate media="seg-$Number%bogus$.m4s" /></Period></MPD>"#
                .as_slice(),
        ] {
            assert!(parse_dash_manifest(value, &remote, "opaque-token").is_err());
        }
    }

    #[test]
    fn baseurl_siblings_keep_their_immutable_inherited_base_and_query_only_is_allowed() {
        let remote = Url::parse("https://cdn.example.invalid/path/index.mpd").unwrap();
        let manifest = br#"<MPD><Period><AdaptationSet><BaseURL>a/</BaseURL><SegmentTemplate media="one.m4s" /></AdaptationSet><AdaptationSet><BaseURL>b/</BaseURL><SegmentTemplate media="two.m4s" /></AdaptationSet></Period></MPD>"#;
        let parsed = parse_dash_manifest(manifest, &remote, "opaque-token").unwrap();
        assert!(
            parsed
                .capabilities
                .iter()
                .any(|capability| capability.path_template == "/path/a/one.m4s")
        );
        assert!(
            parsed
                .capabilities
                .iter()
                .any(|capability| capability.path_template == "/path/b/two.m4s")
        );

        let query_only = br#"<MPD><Period><BaseURL>?signed=fixture</BaseURL><SegmentTemplate media="segment.m4s" /></Period></MPD>"#;
        assert!(parse_dash_manifest(query_only, &remote, "opaque-token").is_ok());

        let alternatives = br#"<MPD><Period><AdaptationSet><BaseURL>a/</BaseURL><BaseURL>b/</BaseURL><SegmentTemplate media="segment.m4s" /></AdaptationSet></Period></MPD>"#;
        assert!(matches!(
            parse_dash_manifest(alternatives, &remote, "opaque-token"),
            Err(super::DashError::InvalidManifest(_))
        ));
    }

    #[test]
    fn representation_id_templates_are_bound_to_declared_representation_scope() {
        let remote = Url::parse("https://cdn.example.invalid/index.mpd").unwrap();
        let declared = br#"<MPD><Period><AdaptationSet><Representation id="video-a"><SegmentTemplate media="seg-$RepresentationID$.m4s" /></Representation></AdaptationSet></Period></MPD>"#;
        let parsed = parse_dash_manifest(declared, &remote, "opaque-token").unwrap();
        assert!(parsed.capabilities[0].matches("/seg-video-a.m4s", None));
        assert!(!parsed.capabilities[0].matches("/seg-video-b.m4s", None));

        let undeclared = br#"<MPD><Period><AdaptationSet><SegmentTemplate media="seg-$RepresentationID$.m4s" /></AdaptationSet></Period></MPD>"#;
        assert!(parse_dash_manifest(undeclared, &remote, "opaque-token").is_err());

        let unsafe_id = br#"<MPD><Period><AdaptationSet><Representation id="../video"><SegmentTemplate media="seg-$RepresentationID$.m4s" /></Representation></AdaptationSet></Period></MPD>"#;
        assert!(parse_dash_manifest(unsafe_id, &remote, "opaque-token").is_err());
    }

    #[test]
    fn schema_location_metadata_is_omitted_while_media_references_stay_guarded() {
        let remote = Url::parse("https://cdn.example.invalid/index.mpd").unwrap();
        let manifest = br#"<MPD xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="urn:mpeg:dash:schema:mpd:2011 https://example.invalid/schema.xsd"><Period><SegmentTemplate media="segment-$Number$.m4s" /></Period></MPD>"#;
        let parsed = parse_dash_manifest(manifest, &remote, "opaque-token").unwrap();
        assert!(
            !parsed
                .bytes
                .windows(b"schemaLocation".len())
                .any(|window| { window == b"schemaLocation" })
        );
        assert!(
            parsed
                .bytes
                .windows(b"/__dash/opaque-token/segment-$Number$.m4s".len())
                .any(|window| window == b"/__dash/opaque-token/segment-$Number$.m4s")
        );
    }

    #[test]
    fn live_shaped_representation_templates_without_baseurl_are_rewritten() {
        let remote = Url::parse("https://cdn.example.invalid/index.mpd").unwrap();
        let manifest = br#"<MPD><Period><AdaptationSet><Representation id="v1" height="1080"><SegmentTemplate initialization="init-$RepresentationID$.m4s" media="seg-$RepresentationID$-$Number%05d$.m4s" /></Representation><Representation id="v2" height="720"><SegmentTemplate initialization="init-$RepresentationID$.m4s" media="seg-$RepresentationID$-$Number%05d$.m4s" /></Representation></AdaptationSet><AdaptationSet><Representation id="a1"><SegmentTemplate initialization="init-$RepresentationID$.m4s" media="seg-$RepresentationID$-$Number%05d$.m4s" /></Representation><Representation id="a2"><SegmentTemplate initialization="init-$RepresentationID$.m4s" media="seg-$RepresentationID$-$Number%05d$.m4s" /></Representation></AdaptationSet></Period></MPD>"#;
        let parsed = parse_dash_manifest(manifest, &remote, "opaque-token").unwrap();
        assert!(
            parsed
                .bytes
                .windows(b"/__dash/opaque-token/init-$RepresentationID$.m4s".len())
                .any(|window| { window == b"/__dash/opaque-token/init-$RepresentationID$.m4s" })
        );
        assert!(parsed.capabilities.iter().any(|capability| {
            capability.path_template == "/init-$RepresentationID$.m4s"
                && capability.matches("/init-v1.m4s", None)
                && !capability.matches("/init-v3.m4s", None)
        }));
        assert!(parsed.capabilities.iter().any(|capability| {
            capability.path_template == "/seg-$RepresentationID$-$Number%05d$.m4s"
                && capability.matches("/seg-v1-00001.m4s", None)
                && !capability.matches("/seg-v1-1.m4s", None)
        }));
    }

    #[tokio::test]
    async fn incomplete_proxy_headers_time_out_without_waiting_forever() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).await.unwrap();
        let (mut server, _) = listener.accept().await.unwrap();
        let _client = client;
        server
            .write_all(b"GET /__dash/token/manifest.mpd HTTP/1.1\r\nHost: localhost\r\n")
            .await
            .unwrap();
        let result = timeout(
            Duration::from_secs(1),
            super::read_dash_proxy_request(&mut server, CancellationToken::new()),
        )
        .await
        .expect("header reader should have its own timeout")
        .unwrap_err();
        assert_eq!(result.kind(), std::io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn omitted_error_bodies_advertise_zero_content_length() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(address).await.unwrap();
        let (mut server, _) = listener.accept().await.unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_LENGTH,
            reqwest::header::HeaderValue::from_static("17"),
        );
        super::write_dash_proxy_headers(
            &mut server,
            reqwest::StatusCode::FORBIDDEN,
            &headers,
            Some(17),
        )
        .await
        .unwrap();
        let mut response = [0_u8; 2048];
        let read = timeout(Duration::from_secs(1), client.read(&mut response))
            .await
            .unwrap()
            .unwrap();
        let response = String::from_utf8(response[..read].to_vec()).unwrap();
        assert!(response.contains("Content-Length: 0\r\n"));
        assert!(!response.contains("Content-Length: 17"));
    }
}
