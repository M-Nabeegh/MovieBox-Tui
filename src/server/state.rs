use std::sync::Arc;

use sqlx::SqlitePool;

use crate::server::{
    auth::AuthService, config::ServerConfig, error::ServerError, events::JobEventBus,
    jobs::JobRepository,
};
use crate::{
    catalog::moviebox::MovieBoxCatalogProvider,
    catalog::{CatalogProvider, CatalogService, OpaqueIdCodec, QualityPolicy},
    providers::moviebox::client::MovieBoxClient,
};

#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

struct AppStateInner {
    config: ServerConfig,
    pool: SqlitePool,
    auth: AuthService,
    catalog: Arc<CatalogService>,
    jobs: JobRepository,
    events: JobEventBus,
}

impl AppState {
    pub async fn bootstrap(config: ServerConfig, pool: SqlitePool) -> Result<Self, ServerError> {
        let auth = AuthService::new(&config);
        auth.bootstrap_admin(&pool, &config.admin_password_file)
            .await?;

        let provider = MovieBoxCatalogProvider::new(
            MovieBoxClient::new(),
            OpaqueIdCodec::new(config.session_pepper),
            QualityPolicy::new(config.maximum_height),
        );
        Self::from_provider(config, pool, auth, Arc::new(provider))
    }

    fn from_provider(
        config: ServerConfig,
        pool: SqlitePool,
        auth: AuthService,
        provider: Arc<dyn CatalogProvider>,
    ) -> Result<Self, ServerError> {
        Ok(Self {
            inner: Arc::new(AppStateInner {
                config,
                jobs: JobRepository::new(pool.clone()),
                pool,
                auth,
                catalog: Arc::new(CatalogService::new(provider)),
                events: JobEventBus::default(),
            }),
        })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.inner.pool
    }

    pub fn auth(&self) -> &AuthService {
        &self.inner.auth
    }

    pub fn config(&self) -> &ServerConfig {
        &self.inner.config
    }

    pub fn catalog(&self) -> &CatalogService {
        &self.inner.catalog
    }
    pub fn jobs(&self) -> &JobRepository {
        &self.inner.jobs
    }
    pub fn events(&self) -> &JobEventBus {
        &self.inner.events
    }
}
