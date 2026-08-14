use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use url::Url;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Movie,
    Series,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CatalogId(String);

impl CatalogId {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(String);

impl SourceId {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubtitleId(String);

impl SubtitleId {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPage {
    pub page: u32,
    pub items: Vec<CatalogItem>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogItem {
    pub id: CatalogId,
    pub title: String,
    pub media_type: MediaType,
    pub year: Option<String>,
    pub season_count: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpisodeInfo {
    pub number: u16,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeasonInfo {
    pub number: u16,
    pub episodes: Vec<EpisodeInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioOption {
    pub id: CatalogId,
    pub label: String,
    pub original: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogDetails {
    pub id: CatalogId,
    pub title: String,
    pub media_type: MediaType,
    pub year: Option<String>,
    pub description: Option<String>,
    pub tagline: Option<String>,
    pub imdb_rating: Option<String>,
    pub duration: Option<String>,
    pub genres: Vec<String>,
    pub country: Option<String>,
    pub seasons: Vec<SeasonInfo>,
    pub audio_options: Vec<AudioOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceOption {
    pub id: SourceId,
    pub height: u16,
    pub label: String,
    pub size_bytes: Option<u64>,
    pub language: Option<String>,
    pub recommended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtitleTrack {
    pub id: SubtitleId,
    pub language: String,
    pub format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeRequest {
    pub catalog_id: CatalogId,
    pub season: Option<u16>,
    pub episode: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct ResolvedSource {
    pub url: Url,
    pub headers: HeaderMap,
    pub subtitle: Option<ResolvedSubtitle>,
    pub extension: String,
    pub expected_size: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ResolvedSubtitle {
    pub url: Url,
    pub headers: HeaderMap,
    pub language: String,
    pub extension: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityPolicy {
    maximum_height: u16,
}

impl QualityPolicy {
    pub fn new(maximum_height: u16) -> Self {
        Self { maximum_height }
    }

    pub fn maximum_height(&self) -> u16 {
        self.maximum_height
    }

    pub fn validate(&self, height: u16) -> Result<(), CatalogError> {
        if height > self.maximum_height {
            return Err(CatalogError::QualityUnavailable {
                maximum_height: self.maximum_height,
                requested_height: Some(height),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpaquePayload {
    Catalog {
        provider: String,
        subject_id: String,
    },
    Source {
        provider: String,
        subject_id: String,
        resource_id: String,
        season: Option<u16>,
        episode: Option<u16>,
        height: u16,
        language: Option<String>,
    },
    Subtitle {
        provider: String,
        subject_id: String,
        resource_id: String,
        index: u16,
        language: String,
        format: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct OpaqueIdCodec {
    secret: [u8; 32],
}

impl OpaqueIdCodec {
    pub fn new(secret: [u8; 32]) -> Self {
        Self { secret }
    }

    pub fn encode(&self, payload: &OpaquePayload) -> String {
        let payload_bytes =
            serde_json::to_vec(payload).expect("opaque payload serialization must not fail");
        let mut mac =
            HmacSha256::new_from_slice(&self.secret).expect("fixed-width HMAC secret is valid");
        mac.update(&payload_bytes);
        let signature = mac.finalize().into_bytes();
        format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(payload_bytes),
            URL_SAFE_NO_PAD.encode(signature)
        )
    }

    pub fn decode(&self, value: &str) -> Result<OpaquePayload, CatalogError> {
        let (encoded_payload, encoded_signature) =
            value.split_once('.').ok_or(CatalogError::InvalidOpaqueId)?;
        let payload_bytes = URL_SAFE_NO_PAD
            .decode(encoded_payload)
            .map_err(|_| CatalogError::InvalidOpaqueId)?;
        let provided_signature = URL_SAFE_NO_PAD
            .decode(encoded_signature)
            .map_err(|_| CatalogError::InvalidOpaqueId)?;
        let mut mac =
            HmacSha256::new_from_slice(&self.secret).expect("fixed-width HMAC secret is valid");
        mac.update(&payload_bytes);
        mac.verify_slice(&provided_signature)
            .map_err(|_| CatalogError::OpaqueIdVerificationFailed)?;
        serde_json::from_slice(&payload_bytes).map_err(|_| CatalogError::InvalidOpaqueId)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CatalogError {
    #[error("catalog payload was missing required field: {0}")]
    InvalidPayload(&'static str),
    #[error("catalog provider is unsupported: {0}")]
    UnsupportedProvider(String),
    #[error("catalog provider request failed: {0}")]
    Provider(String),
    #[error("catalog item was not found: {0}")]
    NotFound(&'static str),
    #[error("catalog opaque identifier is invalid")]
    InvalidOpaqueId,
    #[error("catalog opaque identifier failed verification")]
    OpaqueIdVerificationFailed,
    #[error("no source is available at or below {maximum_height}p")]
    QualityUnavailable {
        maximum_height: u16,
        requested_height: Option<u16>,
    },
}
