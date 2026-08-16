use std::sync::Arc;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::server::{
    auth::AuthService,
    config::ServerConfig,
    error::ServerError,
    events::JobEventBus,
    jobs::{
        HttpTransferClient, JobRepository, JobSignal, JobWorker, LibraryRefresher,
        NoopLibraryRefresher, recover_interrupted_jobs, sweep_orphaned_partials,
    },
    library::{LibraryNamer, jellyfin::JellyfinClient},
    notify::{DownloadNotifier, NoopNotifier, WebhookNotifier},
};
use crate::{
    catalog::moviebox::MovieBoxCatalogProvider,
    catalog::{CatalogProvider, CatalogService, OpaqueIdCodec, QualityPolicy},
    download::DownloadClient,
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
    signal: JobSignal,
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
        let state = Self::from_provider(config, pool, auth, Arc::new(provider))?;
        state.start_worker().await?;
        Ok(state)
    }

    async fn start_worker(&self) -> Result<(), ServerError> {
        let namer = LibraryNamer::new(self.inner.config.media_root.clone())
            .map_err(|error| ServerError::Startup(format!("library setup failed: {error}")))?;
        recover_interrupted_jobs(&self.inner.jobs, namer.media_root())
            .await
            .map_err(|error| ServerError::Startup(format!("job recovery failed: {error}")))?;
        // Recovery runs first so that anything still resumable is back in the
        // queue before the sweep decides what counts as abandoned.
        match sweep_orphaned_partials(&self.inner.jobs, namer.media_root()).await {
            Ok(0) => {}
            Ok(reclaimed) => {
                eprintln!("[moviebox-server] reclaimed {reclaimed} bytes of abandoned partials");
            }
            Err(error) => {
                eprintln!("[moviebox-server] partial sweep failed: {error}");
            }
        }
        let download_client = DownloadClient::new().map_err(|error| {
            ServerError::Startup(format!("download client initialization failed: {error}"))
        })?;
        let store = Arc::new(self.inner.jobs.clone());
        let media_root = namer.media_root().to_path_buf();
        let library = self.library_refresher();
        let worker = JobWorker::new(
            Arc::clone(&store),
            Arc::clone(&self.inner.catalog),
            Arc::new(HttpTransferClient::new(download_client)),
            namer,
            self.inner.events.clone(),
        )
        .with_reserve_bytes(self.inner.config.reserve_bytes)
        .with_library_refresher(library)
        .with_notifier(self.completion_notifier())
        .with_signal(self.inner.signal.clone());

        tokio::spawn(async move {
            while let Err(error) = worker.run(CancellationToken::new()).await {
                eprintln!("[moviebox-server] job worker stopped: {error}; restarting");
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    match recover_interrupted_jobs(store.as_ref(), &media_root).await {
                        Ok(()) => break,
                        Err(error) => {
                            eprintln!(
                                "[moviebox-server] job recovery before worker restart failed: {error}"
                            );
                        }
                    }
                }
            }
        });
        Ok(())
    }

    /// Build the media-library notifier the worker calls when a job completes.
    ///
    /// A missing or unreadable API key is not fatal: downloads still finish and
    /// land in the media folders, they just wait for Jellyfin's own scan.
    fn library_refresher(&self) -> Arc<dyn LibraryRefresher> {
        match JellyfinClient::from_config(&self.inner.config) {
            Ok(client) if client.is_configured() => Arc::new(client),
            Ok(_) => Arc::new(NoopLibraryRefresher),
            Err(error) => {
                eprintln!(
                    "[moviebox-server] Jellyfin refresh disabled: {error}; downloads will rely on scheduled scans"
                );
                Arc::new(NoopLibraryRefresher)
            }
        }
    }

    /// Build the notifier that announces finished downloads.
    ///
    /// Notifications are a convenience, so a missing or malformed webhook only
    /// disables them; downloads still complete normally.
    fn completion_notifier(&self) -> Arc<dyn DownloadNotifier> {
        match WebhookNotifier::from_config(&self.inner.config) {
            Ok(Some(notifier)) => Arc::new(notifier),
            Ok(None) => Arc::new(NoopNotifier),
            Err(error) => {
                eprintln!("[moviebox-server] completion notifications disabled: {error}");
                Arc::new(NoopNotifier)
            }
        }
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
                signal: JobSignal::new(),
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
    /// Wakes the download worker so queued work starts without waiting for a poll.
    pub fn signal(&self) -> &JobSignal {
        &self.inner.signal
    }
}
