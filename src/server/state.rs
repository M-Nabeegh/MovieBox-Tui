use std::sync::Arc;

use sqlx::SqlitePool;

use crate::server::{auth::AuthService, config::ServerConfig, error::ServerError};

#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

struct AppStateInner {
    config: ServerConfig,
    pool: SqlitePool,
    auth: AuthService,
}

impl AppState {
    pub async fn bootstrap(config: ServerConfig, pool: SqlitePool) -> Result<Self, ServerError> {
        let auth = AuthService::new(&config);
        auth.bootstrap_admin(&pool, &config.admin_password_file)
            .await?;

        Ok(Self {
            inner: Arc::new(AppStateInner { config, pool, auth }),
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
}
