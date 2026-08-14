#![allow(dead_code)]

use moviebox_tui::server::security::net::{AddressResolver, DownloadClient, ResolveFuture};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::{
    collections::HashMap,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
    task::JoinHandle,
};
use url::Url;

const FIXTURE_HOST: &str = "example.com";
const REQUIRED_HEADER_NAME: &str = "x-moviebox-auth";
const REQUIRED_HEADER_VALUE: &str = "fixture-token";
const FIXTURE_ETAG: &str = "\"fixture-etag\"";
const FIXTURE_LAST_MODIFIED: &str = "Fri, 01 Jan 2021 00:00:00 GMT";
const TEST_PUBLIC_IP: Ipv4Addr = Ipv4Addr::new(1, 1, 1, 1);

#[derive(Clone, Copy)]
struct FixtureResolver;

impl AddressResolver for FixtureResolver {
    fn resolve<'a>(&'a self, _host: &'a str, port: u16) -> ResolveFuture<'a> {
        Box::pin(async move { Ok(vec![SocketAddr::new(IpAddr::V4(TEST_PUBLIC_IP), port)]) })
    }
}

#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

struct ServerState {
    content: Vec<u8>,
    subtitle: Vec<u8>,
    requests: Mutex<Vec<RecordedRequest>>,
    drop_first_ranged: AtomicBool,
}

pub struct FixtureServer {
    address: SocketAddr,
    state: Arc<ServerState>,
    shutdown: Option<oneshot::Sender<()>>,
    handle: JoinHandle<()>,
}

impl FixtureServer {
    pub async fn start(content_len: usize) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let state = Arc::new(ServerState {
            content: (0..content_len).map(|index| (index % 251) as u8).collect(),
            subtitle: b"1\n00:00:00,000 --> 00:00:01,000\nfixture subtitle\n".to_vec(),
            requests: Mutex::new(Vec::new()),
            drop_first_ranged: AtomicBool::new(false),
        });
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let server_state = Arc::clone(&state);
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else {
                            break;
                        };
                        let state = Arc::clone(&server_state);
                        tokio::spawn(async move {
                            let _ = handle_connection(stream, state, address.port()).await;
                        });
                    }
                }
            }
        });

        Ok(Self {
            address,
            state,
            shutdown: Some(shutdown_tx),
            handle,
        })
    }

    pub fn enable_drop_first_ranged_request(&self) {
        self.state.drop_first_ranged.store(true, Ordering::Relaxed);
    }

    pub fn client(&self) -> DownloadClient {
        DownloadClient::builder()
            .resolve(FIXTURE_HOST, self.address)
            .resolver(FixtureResolver)
            .test_peer_address(SocketAddr::new(
                IpAddr::V4(TEST_PUBLIC_IP),
                self.address.port(),
            ))
            .build()
            .expect("fixture client")
    }

    pub fn mismatched_client(&self) -> DownloadClient {
        DownloadClient::builder()
            .resolve(FIXTURE_HOST, self.address)
            .resolver(FixtureResolver)
            .build()
            .expect("fixture mismatch client")
    }

    pub fn url(&self, path: &str) -> Url {
        Url::parse(&format!(
            "http://{FIXTURE_HOST}:{}{}",
            self.address.port(),
            path
        ))
        .unwrap()
    }

    pub fn content_len(&self) -> usize {
        self.state.content.len()
    }

    pub fn subtitle_bytes(&self) -> &[u8] {
        &self.state.subtitle
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.state
            .requests
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    pub fn required_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static(REQUIRED_HEADER_NAME),
            HeaderValue::from_static(REQUIRED_HEADER_VALUE),
        );
        headers
    }

    pub fn etag() -> &'static str {
        FIXTURE_ETAG
    }

    pub fn last_modified() -> &'static str {
        FIXTURE_LAST_MODIFIED
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.handle.abort();
    }
}

async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    state: Arc<ServerState>,
    port: u16,
) -> io::Result<()> {
    let request = read_request(&mut stream).await?;
    state
        .requests
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .push(request.clone());

    if request.header(REQUIRED_HEADER_NAME) != Some(REQUIRED_HEADER_VALUE) {
        return write_response(
            &mut stream,
            "403 Forbidden",
            &[("content-length", "0".to_string())],
            b"",
        )
        .await;
    }

    match request.path.as_str() {
        "/redirect/download" => {
            let location = format!("http://{FIXTURE_HOST}:{port}/download");
            write_response(
                &mut stream,
                "302 Found",
                &[("location", location), ("content-length", "0".to_string())],
                b"",
            )
            .await
        }
        "/redirect/subtitle" => {
            let location = format!("http://{FIXTURE_HOST}:{port}/subtitle");
            write_response(
                &mut stream,
                "302 Found",
                &[("location", location), ("content-length", "0".to_string())],
                b"",
            )
            .await
        }
        "/redirect/private" => {
            write_response(
                &mut stream,
                "302 Found",
                &[
                    ("location", "http://127.0.0.1/private".to_string()),
                    ("content-length", "0".to_string()),
                ],
                b"",
            )
            .await
        }
        "/download" => serve_download(&mut stream, &request, &state).await,
        "/subtitle" => {
            write_response(
                &mut stream,
                "200 OK",
                &[
                    ("content-type", "text/plain".to_string()),
                    ("content-length", state.subtitle.len().to_string()),
                ],
                &state.subtitle,
            )
            .await
        }
        _ => {
            write_response(
                &mut stream,
                "404 Not Found",
                &[("content-length", "0".to_string())],
                b"",
            )
            .await
        }
    }
}

async fn serve_download(
    stream: &mut tokio::net::TcpStream,
    request: &RecordedRequest,
    state: &ServerState,
) -> io::Result<()> {
    let range = request.header("range").map(str::to_string);
    let content = &state.content;

    if let Some(range) = range {
        let (start, end) = parse_range(&range, content.len())?;
        let body = &content[start..=end];
        let headers = [
            ("accept-ranges", "bytes".to_string()),
            ("etag", FIXTURE_ETAG.to_string()),
            ("last-modified", FIXTURE_LAST_MODIFIED.to_string()),
            (
                "content-range",
                format!("bytes {start}-{end}/{}", content.len()),
            ),
            ("content-length", body.len().to_string()),
        ];
        if state
            .drop_first_ranged
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            stream
                .write_all(b"HTTP/1.1 206 Partial Content\r\n")
                .await?;
            for (name, value) in headers {
                stream
                    .write_all(format!("{name}: {value}\r\n").as_bytes())
                    .await?;
            }
            stream.write_all(b"\r\n").await?;
            let cutoff = body.len().max(2) / 2;
            stream.write_all(&body[..cutoff]).await?;
            stream.shutdown().await
        } else {
            write_response(stream, "206 Partial Content", &headers, body).await
        }
    } else {
        write_response(
            stream,
            "200 OK",
            &[
                ("accept-ranges", "bytes".to_string()),
                ("etag", FIXTURE_ETAG.to_string()),
                ("last-modified", FIXTURE_LAST_MODIFIED.to_string()),
                ("content-length", content.len().to_string()),
            ],
            content,
        )
        .await
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> io::Result<RecordedRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "fixture request closed before headers",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "fixture request headers too large",
            ));
        }
    }

    let headers_end = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing header terminator"))?;
    let request = String::from_utf8_lossy(&buffer[..headers_end]);
    let mut lines = request.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing method"))?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing path"))?
        .to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    Ok(RecordedRequest {
        method,
        path,
        headers,
    })
}

async fn write_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> io::Result<()> {
    stream
        .write_all(format!("HTTP/1.1 {status}\r\n").as_bytes())
        .await?;
    for (name, value) in headers {
        stream
            .write_all(format!("{name}: {value}\r\n").as_bytes())
            .await?;
    }
    stream.write_all(b"\r\n").await?;
    stream.write_all(body).await?;
    stream.flush().await
}

fn parse_range(value: &str, total_len: usize) -> io::Result<(usize, usize)> {
    let value = value
        .strip_prefix("bytes=")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid range prefix"))?;
    let (start, end) = value
        .split_once('-')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid range bounds"))?;
    let start = start
        .parse::<usize>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid range start"))?;
    let end = if end.is_empty() {
        total_len.saturating_sub(1)
    } else {
        end.parse::<usize>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid range end"))?
    };
    if start >= total_len || end < start || end >= total_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "range outside fixture content",
        ));
    }
    Ok((start, end))
}
