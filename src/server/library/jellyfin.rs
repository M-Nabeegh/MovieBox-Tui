use std::time::Duration;

use reqwest::{Client, StatusCode, Url, header::HeaderValue};
use serde::Deserialize;
use thiserror::Error;

use crate::server::config::ServerConfig;

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_MAX_ATTEMPTS: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JellyfinItem {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JellyfinStatus {
    Ready,
    ScanPending,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JellyfinLibraryStatus {
    pub status: JellyfinStatus,
    pub url: Option<String>,
}

pub struct JellyfinClient {
    http: Client,
    base_url: Url,
    /// Where a browser should be sent. Falls back to `base_url` when unset,
    /// which is only correct when the two are the same address.
    public_url: Option<Url>,
    api_key: Option<HeaderValue>,
    poll_interval: Duration,
    max_attempts: usize,
}

impl JellyfinClient {
    pub fn from_config(config: &ServerConfig) -> Result<Self, JellyfinError> {
        let api_key = config
            .read_jellyfin_api_key()
            .map_err(|_| JellyfinError::SecretUnavailable)?;
        Ok(Self::new(config.jellyfin_base_url.clone(), api_key)?
            .with_public_url(config.jellyfin_public_url.clone()))
    }

    /// Set the address used for links handed to a browser.
    pub fn with_public_url(mut self, public_url: Option<Url>) -> Self {
        self.public_url = public_url;
        self
    }

    pub fn new(base_url: Url, api_key: Option<String>) -> Result<Self, JellyfinError> {
        Ok(Self::with_polling(
            base_url,
            api_key,
            DEFAULT_POLL_INTERVAL,
            DEFAULT_MAX_ATTEMPTS,
        ))
    }

    pub fn with_polling(
        base_url: Url,
        api_key: Option<String>,
        poll_interval: Duration,
        max_attempts: usize,
    ) -> Self {
        let api_key = api_key.and_then(|key| HeaderValue::from_str(&key).ok());
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("fixed Jellyfin HTTP client options are valid"),
            base_url,
            public_url: None,
            api_key,
            poll_interval,
            max_attempts: max_attempts.max(1),
        }
    }

    pub async fn refresh_library(&self) -> Result<(), JellyfinError> {
        let Some(api_key) = self.api_key.as_ref() else {
            return Ok(());
        };
        let response = self
            .http
            .post(self.endpoint("Library/Refresh")?)
            .header("X-Emby-Token", api_key)
            .send()
            .await
            .map_err(|_| JellyfinError::Unavailable)?;
        if response.status() == StatusCode::NO_CONTENT || response.status().is_success() {
            Ok(())
        } else {
            Err(JellyfinError::Unavailable)
        }
    }

    /// Whether a credential is configured, and so whether a refresh can do anything.
    pub fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    pub async fn find_item(
        &self,
        title: &str,
        year: Option<&str>,
    ) -> Result<Option<JellyfinItem>, JellyfinError> {
        let Some(api_key) = self.api_key.as_ref() else {
            return Ok(None);
        };
        for attempt in 0..self.max_attempts {
            let mut items_url = self.endpoint("Items")?;
            items_url
                .query_pairs_mut()
                .append_pair("searchTerm", title)
                .append_pair("Recursive", "true");
            let response = self
                .http
                .get(items_url)
                .header("X-Emby-Token", api_key)
                .send()
                .await
                .map_err(|_| JellyfinError::Unavailable)?;
            if !response.status().is_success() {
                return Err(JellyfinError::Unavailable);
            }
            let payload = response
                .json::<ItemsResponse>()
                .await
                .map_err(|_| JellyfinError::InvalidResponse)?;
            if let Some(item) = payload.items.into_iter().find(|item| {
                item.name.eq_ignore_ascii_case(title)
                    && year.is_none_or(|wanted| item.production_year == wanted.parse().ok())
            }) {
                return Ok(Some(JellyfinItem { id: item.id }));
            }
            if attempt + 1 < self.max_attempts {
                tokio::time::sleep(self.poll_interval).await;
            }
        }
        Ok(None)
    }

    /// A link a browser can follow to this item.
    ///
    /// Built from the public address, never the internal one: the API talks to
    /// a container hostname that resolves only inside this network, so a link
    /// built from it is dead the moment it reaches the user.
    pub fn deep_link(&self, item_id: &str) -> Result<Url, JellyfinError> {
        let base = self.public_url.as_ref().unwrap_or(&self.base_url);
        let mut url = base
            .join("web/index.html")
            .map_err(|_| JellyfinError::InvalidBaseUrl)?;
        url.set_fragment(Some(&format!("!/details?id={item_id}")));
        Ok(url)
    }

    pub async fn status_for_ready_job(
        &self,
        title: &str,
        year: Option<&str>,
    ) -> Result<JellyfinLibraryStatus, JellyfinError> {
        if self.api_key.is_none() {
            return Ok(JellyfinLibraryStatus {
                status: JellyfinStatus::ScanPending,
                url: None,
            });
        }
        self.refresh_library().await?;
        let Some(item) = self.find_item(title, year).await? else {
            return Ok(JellyfinLibraryStatus {
                status: JellyfinStatus::Unavailable,
                url: None,
            });
        };
        Ok(JellyfinLibraryStatus {
            status: JellyfinStatus::Ready,
            url: Some(self.deep_link(&item.id)?.to_string()),
        })
    }

    fn endpoint(&self, path: &str) -> Result<Url, JellyfinError> {
        self.base_url
            .join(path)
            .map_err(|_| JellyfinError::InvalidBaseUrl)
    }
}

#[derive(Debug, Error)]
pub enum JellyfinError {
    #[error("Jellyfin is unavailable")]
    Unavailable,
    #[error("Jellyfin returned an invalid response")]
    InvalidResponse,
    #[error("Jellyfin base URL is invalid")]
    InvalidBaseUrl,
    #[error("Jellyfin API key could not be loaded")]
    SecretUnavailable,
}

/// Adapts the Jellyfin client to the worker's library-refresh port.
///
/// The worker holds only this handle, so the API key stays inside the client and
/// is never passed through job state, events, or logs.
#[async_trait::async_trait]
impl crate::server::jobs::LibraryRefresher for JellyfinClient {
    async fn refresh(&self) -> Result<(), ()> {
        self.refresh_library().await.map_err(|_| ())
    }
}

#[derive(Debug, Deserialize)]
struct ItemsResponse {
    #[serde(rename = "Items", default)]
    items: Vec<ItemResponse>,
}

#[derive(Debug, Deserialize)]
struct ItemResponse {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "ProductionYear")]
    production_year: Option<i32>,
}
