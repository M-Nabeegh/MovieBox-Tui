use crate::providers::moviebox::crypto::build_signed_headers;
use crate::providers::moviebox::session::MovieBoxSession;
use reqwest::Response;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
const HOST_POOL: &[&str] = &[
    "https://api6.aoneroom.com",
    "https://api5.aoneroom.com",
    "https://api4.aoneroom.com",
    "https://api4sg.aoneroom.com",
    "https://api3.aoneroom.com",
    "https://api6sg.aoneroom.com",
    "https://api.inmoviebox.com",
];

const RETRY_STATUS_CODES: &[u16] = &[403, 406, 407, 429, 500, 502, 503, 504];

#[derive(thiserror::Error, Debug)]
pub enum ScraperError {
    #[error("Reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("API error status: {0}")]
    ApiStatus(u16),
    #[error("All hosts exhausted")]
    HostsExhausted,
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Missing expected token")]
    MissingToken,
}

#[derive(Clone)]
pub struct MovieBoxClient {
    client: reqwest::Client,
    session: Arc<RwLock<Option<MovieBoxSession>>>,
    session_lock: Arc<tokio::sync::Mutex<()>>,
    active_base_idx: Arc<AtomicUsize>,
    user_agent: String,
    client_info: String,
    spoofed_ip: String,
}

impl Default for MovieBoxClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MovieBoxClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(12))
            .connect_timeout(std::time::Duration::from_secs(3))
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .pool_max_idle_per_host(4)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let (user_agent, client_info) =
            crate::providers::moviebox::crypto::generate_client_info_and_ua();
        let spoofed_ip = crate::providers::moviebox::crypto::random_spoofed_ip();

        Self {
            client,
            session: Arc::new(RwLock::new(None)),
            session_lock: Arc::new(tokio::sync::Mutex::new(())),
            active_base_idx: Arc::new(AtomicUsize::new(0)),
            user_agent,
            client_info,
            spoofed_ip,
        }
    }

    pub fn http_client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// Headers required when using a resolved MovieBox media URL.
    ///
    /// The CDN checks the Android identity independently of the signed API
    /// request and substitutes a short notice video when it is absent.
    pub fn media_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Ok(value) = HeaderValue::from_str(&self.user_agent) {
            headers.insert(USER_AGENT, value);
        }
        headers
    }

    pub async fn init(&self) -> Result<(), ScraperError> {
        self.ensure_session().await.map(|_| ())
    }

    /// Reuse a valid visitor token and serialize login when several requests
    /// arrive at the same time.  The token remains process-local only.
    pub async fn ensure_session(&self) -> Result<String, ScraperError> {
        if let Some(session) = self
            .session
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            && session.is_valid()
        {
            return Ok(session.token().to_string());
        }

        let _guard = self.session_lock.lock().await;
        if let Some(session) = self
            .session
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            && session.is_valid()
        {
            return Ok(session.token().to_string());
        }

        let payload = self
            .request_hosts(
                "POST",
                "/wefeed-mobile-bff/user-api/visitor-login",
                Some("{}"),
                None,
            )
            .await?;
        let token = payload
            .get("token")
            .or_else(|| payload.get("data").and_then(|data| data.get("token")))
            .and_then(Value::as_str)
            .filter(|token| !token.trim().is_empty())
            .ok_or(ScraperError::MissingToken)?;
        let user_id = payload
            .get("uid")
            .or_else(|| payload.get("userId"))
            .or_else(|| payload.get("data").and_then(|data| data.get("uid")))
            .and_then(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .or_else(|| value.as_u64().map(|number| number.to_string()))
                    .or_else(|| value.as_i64().map(|number| number.to_string()))
            });
        let session = MovieBoxSession::from_token_and_payload(token.to_string(), user_id);
        let token = session.token().to_string();
        *self
            .session
            .write()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(session);
        Ok(token)
    }

    pub fn invalidate_session(&self) {
        *self
            .session
            .write()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
    }

    fn invalidate_session_if(&self, token: &str) {
        let mut session = self
            .session
            .write()
            .unwrap_or_else(|poison| poison.into_inner());
        if session
            .as_ref()
            .is_some_and(|current| current.token() == token)
        {
            *session = None;
        }
    }

    async fn absorb_x_user(&self, headers: &reqwest::header::HeaderMap) {
        let Some(x_user_val) = headers.get("x-user") else {
            return;
        };
        let Ok(x_user_str) = x_user_val.to_str() else {
            return;
        };
        let Ok(json): Result<Value, _> = serde_json::from_str(x_user_str) else {
            return;
        };
        let Some(token) = json.get("token").and_then(|t| t.as_str()) else {
            return;
        };
        if !token.is_empty() {
            let mut write_session = self
                .session
                .write()
                .unwrap_or_else(|poison| poison.into_inner());
            *write_session = Some(MovieBoxSession::from_token_and_payload(
                token.to_string(),
                json.get("uid").and_then(|value| {
                    value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .or_else(|| value.as_u64().map(|number| number.to_string()))
                }),
            ));
        }
    }

    pub async fn get(&self, path_and_query: &str) -> Result<Value, ScraperError> {
        self.request("GET", path_and_query, None).await
    }

    pub async fn post(&self, path_and_query: &str, body: &Value) -> Result<Value, ScraperError> {
        let body_str = serde_json::to_string(body)?;
        self.request("POST", path_and_query, Some(&body_str)).await
    }

    async fn request(
        &self,
        method: &str,
        path_and_query: &str,
        body: Option<&str>,
    ) -> Result<Value, ScraperError> {
        let token = self.ensure_session().await?;
        match self
            .request_hosts(method, path_and_query, body, Some(&token))
            .await
        {
            Err(ScraperError::ApiStatus(401 | 403)) => {
                self.invalidate_session_if(&token);
                let fresh_token = self.ensure_session().await?;
                self.request_hosts(method, path_and_query, body, Some(&fresh_token))
                    .await
            }
            result => result,
        }
    }

    async fn request_hosts(
        &self,
        method: &str,
        path_and_query: &str,
        body: Option<&str>,
        auth_token: Option<&str>,
    ) -> Result<Value, ScraperError> {
        let start_idx = self.active_base_idx.load(Ordering::Relaxed);
        let mut auth_status = None;

        for i in 0..HOST_POOL.len() {
            if i > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            let idx = (start_idx + i) % HOST_POOL.len();
            let base = HOST_POOL[idx];
            let url = format!("{}{}", base, path_and_query);

            let headers = build_signed_headers(
                method,
                &url,
                body,
                auth_token,
                &self.user_agent,
                &self.client_info,
                &self.spoofed_ip,
            );

            let mut builder = match method {
                "POST" => self.client.post(&url),
                _ => self.client.get(&url),
            };

            builder = builder.headers(headers);
            if let Some(b) = body {
                builder = builder.body(b.to_string());
            }

            match builder.send().await {
                Ok(resp) => {
                    self.absorb_x_user(resp.headers()).await;
                    let status = resp.status().as_u16();

                    if matches!(status, 401 | 403) {
                        auth_status = Some(status);
                        continue;
                    }
                    if RETRY_STATUS_CODES.contains(&status) {
                        log::warn!(
                            "moviebox host {idx} returned retryable status {status}: {}",
                            crate::logging::sanitize_url(&url)
                        );
                        continue;
                    }

                    self.active_base_idx.store(idx, Ordering::Relaxed);

                    match self.parse_response(resp).await {
                        Ok(val) => return Ok(val),
                        Err(error) => {
                            log::warn!(
                                "moviebox host {idx} parse failed: {error} [{}]",
                                crate::logging::sanitize_url(&url)
                            );
                            continue;
                        }
                    }
                }
                Err(error) => {
                    log::warn!(
                        "moviebox host {idx} request failed: {error} [{}]",
                        crate::logging::sanitize_url(&url)
                    );
                    continue;
                }
            }
        }

        if let Some(status) = auth_status {
            return Err(ScraperError::ApiStatus(status));
        }
        log::error!("moviebox: all hosts exhausted for [redacted]");
        Err(ScraperError::HostsExhausted)
    }

    async fn parse_response(&self, resp: Response) -> Result<Value, ScraperError> {
        let status = resp.status();
        if !status.is_success() {
            return Err(ScraperError::ApiStatus(status.as_u16()));
        }

        let raw_text = match resp.text().await {
            Ok(t) => t,
            Err(e) => return Err(ScraperError::Reqwest(e)),
        };

        let body_val: Value =
            match tokio::task::spawn_blocking(move || serde_json::from_str(&raw_text)).await {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => return Err(ScraperError::Json(e)),
                Err(_) => {
                    return Err(ScraperError::HostsExhausted);
                }
            };

        if let Some(data) = body_val.get("data") {
            Ok(data.clone())
        } else {
            Ok(body_val)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MovieBoxClient;
    use reqwest::header::USER_AGENT;

    #[test]
    fn media_requests_forward_the_same_mobile_identity_as_api_requests() {
        let client = MovieBoxClient::new();
        let headers = client.media_headers();

        assert_eq!(
            headers
                .get(USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(client.user_agent.as_str())
        );
    }
}
