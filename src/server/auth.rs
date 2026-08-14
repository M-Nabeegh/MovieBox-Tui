use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use argon2::{
    Algorithm, Argon2, PasswordHash, PasswordHasher, PasswordVerifier, Version,
    password_hash::{SaltString, rand_core::OsRng},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use cookie::{Cookie, SameSite, time::Duration as CookieDuration};
use hmac::{Hmac, Mac, digest::KeyInit};
use rand::RngExt;
use sha2::Sha256;
use sqlx::{Row, SqlitePool};
use time::{Duration, OffsetDateTime};
use url::{Host, Url};
use uuid::Uuid;

use crate::server::{
    config::ServerConfig,
    error::{ApiError, ServerError},
};

type SessionMac = Hmac<Sha256>;

const ADMIN_USERNAME: &str = "admin";
const LOGIN_FAILURE_LIMIT: usize = 5;
const LOGIN_WINDOW: Duration = Duration::minutes(10);
const IDLE_SESSION_WINDOW: Duration = Duration::minutes(30);
const ABSOLUTE_SESSION_WINDOW: Duration = Duration::days(7);

#[derive(Clone)]
pub struct AuthService {
    session_cookie_name: String,
    session_pepper: [u8; 32],
    login_failures: Arc<Mutex<HashMap<String, VecDeque<OffsetDateTime>>>>,
}

pub struct AuthenticatedSession {
    pub session_id: String,
    pub username: String,
    pub csrf_token: String,
}

impl AuthService {
    pub fn new(config: &ServerConfig) -> Self {
        Self {
            session_cookie_name: config.session_cookie_name.clone(),
            session_pepper: config.session_pepper,
            login_failures: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn bootstrap_admin(
        &self,
        pool: &SqlitePool,
        password_file: &Path,
    ) -> Result<(), ServerError> {
        let existing = sqlx::query("SELECT id FROM users LIMIT 1")
            .fetch_optional(pool)
            .await
            .map_err(|_| ServerError::Startup("database initialization failed".to_string()))?;

        if existing.is_some() {
            return Ok(());
        }

        let password = read_secret(password_file).map_err(|_| {
            ServerError::Startup("admin bootstrap secret could not be loaded".to_string())
        })?;
        let password_hash = hash_password(&password).map_err(|_| {
            ServerError::Startup("admin bootstrap secret could not be hashed".to_string())
        })?;
        let now = OffsetDateTime::now_utc().unix_timestamp();

        sqlx::query(
            "INSERT INTO users (id, username, password_hash, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(ADMIN_USERNAME)
        .bind(password_hash)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|_| ServerError::Startup("database initialization failed".to_string()))?;

        Ok(())
    }

    pub async fn login(
        &self,
        pool: &SqlitePool,
        source: &str,
        password: &str,
    ) -> Result<String, ApiError> {
        if self.is_throttled(source) {
            return Err(ApiError::too_many_attempts());
        }

        let row = sqlx::query(
            "SELECT id, username, password_hash FROM users WHERE username = ?1 LIMIT 1",
        )
        .bind(ADMIN_USERNAME)
        .fetch_optional(pool)
        .await
        .map_err(|_| ApiError::internal())?;

        let Some(row) = row else {
            return Err(ApiError::authentication_required());
        };

        let user_id = row.get::<String, _>("id");
        let password_hash = row.get::<String, _>("password_hash");

        if !verify_password(password, &password_hash) {
            self.record_failure(source);
            return Err(ApiError::invalid_credentials());
        }

        self.clear_failures(source);

        let session_id = Uuid::new_v4().to_string();
        let token = random_token(32);
        let csrf_token = random_token(24);
        let now = OffsetDateTime::now_utc();
        let expires_at = now + IDLE_SESSION_WINDOW;
        let absolute_expires_at = now + ABSOLUTE_SESSION_WINDOW;

        sqlx::query(
            "INSERT INTO sessions (id, user_id, token_hash, csrf_token, created_at, last_seen_at, expires_at, absolute_expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&session_id)
        .bind(user_id)
        .bind(self.hash_session_token(&token))
        .bind(&csrf_token)
        .bind(now.unix_timestamp())
        .bind(now.unix_timestamp())
        .bind(expires_at.unix_timestamp())
        .bind(absolute_expires_at.unix_timestamp())
        .execute(pool)
        .await
        .map_err(|_| ApiError::internal())?;

        Ok(self.build_session_cookie(&format!("{session_id}.{token}")))
    }

    pub async fn authenticate(
        &self,
        pool: &SqlitePool,
        cookie_header: Option<&str>,
    ) -> Result<AuthenticatedSession, ApiError> {
        let cookie = parse_named_cookie(cookie_header, &self.session_cookie_name)
            .ok_or_else(ApiError::authentication_required)?;
        let (session_id, token) = split_session_cookie(&cookie)?;

        let row = sqlx::query(
            "SELECT s.id, s.user_id, s.token_hash, s.csrf_token, s.expires_at, s.absolute_expires_at, u.username
             FROM sessions s
             JOIN users u ON u.id = s.user_id
             WHERE s.id = ?1
             LIMIT 1",
        )
        .bind(&session_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| ApiError::internal())?;

        let Some(row) = row else {
            return Err(ApiError::authentication_required());
        };

        let stored_hash = row.get::<Vec<u8>, _>("token_hash");
        if stored_hash != self.hash_session_token(&token) {
            return Err(ApiError::authentication_required());
        }

        let expires_at = row.get::<i64, _>("expires_at");
        let absolute_expires_at = row.get::<i64, _>("absolute_expires_at");
        let now = OffsetDateTime::now_utc().unix_timestamp();
        if now > expires_at || now > absolute_expires_at {
            sqlx::query("DELETE FROM sessions WHERE id = ?1")
                .bind(&session_id)
                .execute(pool)
                .await
                .map_err(|_| ApiError::internal())?;
            return Err(ApiError::authentication_required());
        }

        let next_expiry = (OffsetDateTime::now_utc() + IDLE_SESSION_WINDOW).unix_timestamp();
        sqlx::query("UPDATE sessions SET last_seen_at = ?1, expires_at = ?2 WHERE id = ?3")
            .bind(now)
            .bind(next_expiry)
            .bind(&session_id)
            .execute(pool)
            .await
            .map_err(|_| ApiError::internal())?;

        Ok(AuthenticatedSession {
            session_id,
            username: row.get::<String, _>("username"),
            csrf_token: row.get::<String, _>("csrf_token"),
        })
    }

    pub async fn logout(&self, pool: &SqlitePool, session_id: &str) -> Result<(), ApiError> {
        sqlx::query("DELETE FROM sessions WHERE id = ?1")
            .bind(session_id)
            .execute(pool)
            .await
            .map_err(|_| ApiError::internal())?;
        Ok(())
    }

    pub fn validate_origin(
        &self,
        origin: Option<&str>,
        host: Option<&str>,
    ) -> Result<(), ApiError> {
        let origin = origin.ok_or_else(ApiError::invalid_origin)?;
        let host = host.ok_or_else(ApiError::invalid_origin)?;
        let parsed_origin = Url::parse(origin).map_err(|_| ApiError::invalid_origin())?;
        if parsed_origin.scheme() != "https" {
            return Err(ApiError::invalid_origin());
        }

        let expected = parse_host_header(host).ok_or_else(ApiError::invalid_origin)?;
        let actual_host = parsed_origin.host().ok_or_else(ApiError::invalid_origin)?;
        let expected_host = expected.host().ok_or_else(ApiError::invalid_origin)?;
        if !hosts_match(actual_host, expected_host)
            || effective_port(&parsed_origin) != effective_port(&expected)
        {
            return Err(ApiError::invalid_origin());
        }
        Ok(())
    }

    pub fn validate_csrf(&self, presented: Option<&str>, expected: &str) -> Result<(), ApiError> {
        match presented {
            Some(token) if !token.is_empty() && token == expected => Ok(()),
            _ => Err(ApiError::csrf_mismatch()),
        }
    }

    pub fn clear_session_cookie(&self) -> String {
        Cookie::build((self.session_cookie_name.clone(), String::new()))
            .path("/")
            .http_only(true)
            .secure(true)
            .same_site(SameSite::Strict)
            .max_age(CookieDuration::seconds(0))
            .build()
            .to_string()
    }

    pub fn source_from_headers(
        &self,
        forwarded_for: Option<&str>,
        real_ip: Option<&str>,
    ) -> String {
        forwarded_for
            .and_then(|value| value.split(',').next())
            .or(real_ip)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("unknown")
            .to_string()
    }

    fn build_session_cookie(&self, value: &str) -> String {
        Cookie::build((self.session_cookie_name.clone(), value.to_string()))
            .path("/")
            .http_only(true)
            .secure(true)
            .same_site(SameSite::Strict)
            .build()
            .to_string()
    }

    fn hash_session_token(&self, token: &str) -> Vec<u8> {
        let mut mac = SessionMac::new_from_slice(&self.session_pepper).expect("valid hmac key");
        mac.update(token.as_bytes());
        mac.finalize().into_bytes().to_vec()
    }

    fn is_throttled(&self, source: &str) -> bool {
        let now = OffsetDateTime::now_utc();
        let mut failures = self
            .login_failures
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let entry = failures.entry(source.to_string()).or_default();
        prune_failures(entry, now);
        entry.len() >= LOGIN_FAILURE_LIMIT
    }

    fn record_failure(&self, source: &str) {
        let now = OffsetDateTime::now_utc();
        let mut failures = self
            .login_failures
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let entry = failures.entry(source.to_string()).or_default();
        prune_failures(entry, now);
        entry.push_back(now);
    }

    fn clear_failures(&self, source: &str) {
        self.login_failures
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(source);
    }
}

fn read_secret(path: &Path) -> Result<String, std::io::Error> {
    fs::read_to_string(path).map(|contents| contents.trim().to_string())
}

fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        argon2::Params::default(),
    );
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
}

fn verify_password(password: &str, password_hash: &str) -> bool {
    let Ok(hash) = PasswordHash::new(password_hash) else {
        return false;
    };
    Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        argon2::Params::default(),
    )
    .verify_password(password.as_bytes(), &hash)
    .is_ok()
}

fn random_token(size: usize) -> String {
    let mut bytes = vec![0_u8; size];
    let mut rng = rand::rng();
    for byte in &mut bytes {
        *byte = rng.random();
    }
    URL_SAFE_NO_PAD.encode(bytes)
}

fn parse_named_cookie(cookie_header: Option<&str>, name: &str) -> Option<String> {
    cookie_header.and_then(|header| {
        header.split(';').find_map(|part| {
            let (cookie_name, value) = part.trim().split_once('=')?;
            (cookie_name == name).then(|| value.to_string())
        })
    })
}

fn split_session_cookie(cookie: &str) -> Result<(String, String), ApiError> {
    let (session_id, token) = cookie
        .split_once('.')
        .ok_or_else(ApiError::authentication_required)?;
    if session_id.is_empty() || token.is_empty() {
        return Err(ApiError::authentication_required());
    }
    Ok((session_id.to_string(), token.to_string()))
}

fn parse_host_header(host: &str) -> Option<Url> {
    let host = host.trim();
    if host.is_empty() {
        return None;
    }

    let parsed = Url::parse(&format!("https://{host}")).ok()?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        return None;
    }
    parsed.host()?;
    Some(parsed)
}

fn hosts_match(actual: Host<&str>, expected: Host<&str>) -> bool {
    match (actual, expected) {
        (Host::Domain(actual), Host::Domain(expected)) => actual.eq_ignore_ascii_case(expected),
        (Host::Ipv4(actual), Host::Ipv4(expected)) => actual == expected,
        (Host::Ipv6(actual), Host::Ipv6(expected)) => actual == expected,
        _ => false,
    }
}

fn effective_port(url: &Url) -> Option<u16> {
    url.port().or_else(|| match url.scheme() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    })
}

fn prune_failures(entry: &mut VecDeque<OffsetDateTime>, now: OffsetDateTime) {
    while let Some(front) = entry.front().copied() {
        if now - front > LOGIN_WINDOW {
            entry.pop_front();
        } else {
            break;
        }
    }
}
