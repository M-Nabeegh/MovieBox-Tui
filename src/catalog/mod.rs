pub mod models;
pub mod moviebox;
pub mod quality;
pub mod tmdb;

use async_trait::async_trait;
use std::sync::Arc;

pub use models::{
    AudioOption, CatalogDetails, CatalogError, CatalogId, CatalogItem, EpisodeInfo, EpisodeRequest,
    MediaType, OpaqueIdCodec, OpaquePayload, QualityPolicy, ResolvedSource, ResolvedSubtitle,
    SearchPage, SeasonInfo, SourceId, SourceOption, SubtitleId, SubtitleTrack,
};
pub use quality::QualityTier;

#[async_trait]
pub trait CatalogProvider: Send + Sync {
    async fn search(&self, query: &str, page: u32) -> Result<SearchPage, CatalogError>;
    async fn details(&self, id: &CatalogId) -> Result<CatalogDetails, CatalogError>;
    async fn sources(&self, request: EpisodeRequest) -> Result<Vec<SourceOption>, CatalogError>;
    async fn subtitles(&self, source: &SourceId) -> Result<Vec<SubtitleTrack>, CatalogError>;
    async fn resolve(
        &self,
        source: &SourceId,
        subtitle: Option<&SubtitleId>,
    ) -> Result<ResolvedSource, CatalogError>;
}

pub struct CatalogService {
    provider: Arc<dyn CatalogProvider>,
}

impl CatalogService {
    pub fn new(provider: Arc<dyn CatalogProvider>) -> Self {
        Self { provider }
    }

    pub async fn search(&self, query: &str, page: u32) -> Result<SearchPage, CatalogError> {
        self.provider.search(query, page).await
    }

    pub async fn details(&self, id: &CatalogId) -> Result<CatalogDetails, CatalogError> {
        self.provider.details(id).await
    }

    pub async fn sources(
        &self,
        request: EpisodeRequest,
    ) -> Result<Vec<SourceOption>, CatalogError> {
        self.provider.sources(request).await
    }

    pub async fn subtitles(&self, source: &SourceId) -> Result<Vec<SubtitleTrack>, CatalogError> {
        self.provider.subtitles(source).await
    }

    pub async fn resolve(
        &self,
        source: &SourceId,
        subtitle: Option<&SubtitleId>,
    ) -> Result<ResolvedSource, CatalogError> {
        self.provider.resolve(source, subtitle).await
    }
}

#[async_trait]
impl CatalogProvider for CatalogService {
    async fn search(&self, query: &str, page: u32) -> Result<SearchPage, CatalogError> {
        CatalogService::search(self, query, page).await
    }

    async fn details(&self, id: &CatalogId) -> Result<CatalogDetails, CatalogError> {
        CatalogService::details(self, id).await
    }

    async fn sources(&self, request: EpisodeRequest) -> Result<Vec<SourceOption>, CatalogError> {
        CatalogService::sources(self, request).await
    }

    async fn subtitles(&self, source: &SourceId) -> Result<Vec<SubtitleTrack>, CatalogError> {
        CatalogService::subtitles(self, source).await
    }

    async fn resolve(
        &self,
        source: &SourceId,
        subtitle: Option<&SubtitleId>,
    ) -> Result<ResolvedSource, CatalogError> {
        CatalogService::resolve(self, source, subtitle).await
    }
}
