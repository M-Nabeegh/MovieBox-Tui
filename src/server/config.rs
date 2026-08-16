use std::{
    collections::HashMap,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use percent_encoding::percent_decode_str;
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteConnectOptions;
use thiserror::Error;
use url::{Host, Url};

use crate::server::library::SubtitlePreference;

const MINIMUM_RESERVE_GIB: u16 = 10;
const COOKIE_NAME: &str = "moviebox_session";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Text,
    Json,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: SocketAddr,
    pub database_url: String,
    pub media_root: PathBuf,
    pub partial_root: PathBuf,
    pub config_root: PathBuf,
    pub admin_password_file: PathBuf,
    pub session_key_file: PathBuf,
    pub maximum_height: u16,
    pub download_concurrency: usize,
    pub reserve_bytes: u64,
    pub reserve_gib: u16,
    pub jellyfin_base_url: Url,
    pub jellyfin_api_key_file: Option<PathBuf>,
    /// Bearer token for the MCP endpoint. Unset leaves the endpoint disabled.
    pub mcp_token_file: Option<PathBuf>,
    /// Webhook posted when a download is ready. Unset disables notifications.
    pub notify_webhook_url_file: Option<PathBuf>,
    /// Name used to address the notification, e.g. "Hey Nabeegh, ...".
    pub notify_recipient: Option<String>,
    /// Where tapping the notification should take the viewer.
    pub notify_link_url: Option<Url>,
    pub log_format: LogFormat,
    /// Language attached automatically when a download names no subtitle.
    pub subtitle_preference: SubtitlePreference,
    pub session_cookie_name: String,
    pub session_pepper: [u8; 32],
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_map(std::env::vars())
    }

    pub fn from_map<I, K, V>(vars: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let env = vars
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect::<HashMap<String, String>>();

        let bind_addr = parse_bind_addr(required_var(&env, "MOVIEBOX_BIND")?)?;
        let database_url = parse_database_url(required_var(&env, "MOVIEBOX_DATABASE_URL")?)?;
        let media_root = validate_directory(
            "MOVIEBOX_MEDIA_ROOT",
            Path::new(required_var(&env, "MOVIEBOX_MEDIA_ROOT")?),
        )?;
        let partial_root = validate_directory(
            "MOVIEBOX_PARTIAL_ROOT",
            Path::new(required_var(&env, "MOVIEBOX_PARTIAL_ROOT")?),
        )?;
        let config_root = validate_directory(
            "MOVIEBOX_CONFIG_ROOT",
            Path::new(required_var(&env, "MOVIEBOX_CONFIG_ROOT")?),
        )?;
        let admin_password_file = validate_secret_file(
            "MOVIEBOX_ADMIN_PASSWORD_FILE",
            Path::new(required_var(&env, "MOVIEBOX_ADMIN_PASSWORD_FILE")?),
        )?;
        let session_key_file = validate_secret_file(
            "MOVIEBOX_SESSION_KEY_FILE",
            Path::new(required_var(&env, "MOVIEBOX_SESSION_KEY_FILE")?),
        )?;
        let maximum_height = parse_maximum_height(required_var(&env, "MOVIEBOX_MAX_HEIGHT")?)?;
        let download_concurrency =
            parse_download_concurrency(required_var(&env, "MOVIEBOX_DOWNLOAD_CONCURRENCY")?)?;
        let reserve_gib = parse_reserve_gib(required_var(&env, "MOVIEBOX_RESERVE_GIB")?)?;
        let jellyfin_base_url =
            parse_jellyfin_base_url(required_var(&env, "MOVIEBOX_JELLYFIN_BASE_URL")?)?;
        let jellyfin_api_key_file = optional_secret_file(&env, "MOVIEBOX_JELLYFIN_API_KEY_FILE")?;
        let mcp_token_file = optional_secret_file(&env, "MOVIEBOX_MCP_TOKEN_FILE")?;
        let notify_webhook_url_file =
            optional_secret_file(&env, "MOVIEBOX_NOTIFY_WEBHOOK_URL_FILE")?;
        let notify_recipient = env
            .get("MOVIEBOX_NOTIFY_RECIPIENT")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let notify_link_url = env
            .get("MOVIEBOX_NOTIFY_LINK_URL")
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                Url::parse(value).map_err(|_| ConfigError::InvalidUrl("MOVIEBOX_NOTIFY_LINK_URL"))
            })
            .transpose()?;
        let log_format = parse_log_format(required_var(&env, "MOVIEBOX_LOG_FORMAT")?)?;
        // Optional: unset means the default (English); `off` disables it.
        let subtitle_preference = env
            .get("MOVIEBOX_SUBTITLE_LANGUAGE")
            .map(|value| SubtitlePreference::parse(value))
            .unwrap_or_default();
        let session_pepper = derive_session_pepper(&session_key_file)?;

        Ok(Self {
            bind_addr,
            database_url,
            media_root,
            partial_root,
            config_root,
            admin_password_file,
            session_key_file,
            maximum_height,
            download_concurrency,
            reserve_bytes: reserve_gib as u64 * 1024 * 1024 * 1024,
            reserve_gib,
            jellyfin_base_url,
            jellyfin_api_key_file,
            mcp_token_file,
            notify_webhook_url_file,
            notify_recipient,
            notify_link_url,
            log_format,
            subtitle_preference,
            session_cookie_name: COOKIE_NAME.to_string(),
            session_pepper,
        })
    }

    /// Read the completion webhook URL, or `None` when notifications are off.
    pub fn read_notify_webhook_url(&self) -> Result<Option<String>, ConfigError> {
        self.notify_webhook_url_file
            .as_deref()
            .map(|path| {
                let url = fs::read_to_string(path).map_err(|_| {
                    ConfigError::UnreadableSecret("MOVIEBOX_NOTIFY_WEBHOOK_URL_FILE")
                })?;
                let url = url.trim();
                if url.is_empty() {
                    return Err(ConfigError::EmptySecret("MOVIEBOX_NOTIFY_WEBHOOK_URL_FILE"));
                }
                Ok(url.to_string())
            })
            .transpose()
    }

    /// Read the MCP bearer token, or `None` when the endpoint is disabled.
    pub fn read_mcp_token(&self) -> Result<Option<String>, ConfigError> {
        self.mcp_token_file
            .as_deref()
            .map(|path| {
                let token = fs::read_to_string(path)
                    .map_err(|_| ConfigError::UnreadableSecret("MOVIEBOX_MCP_TOKEN_FILE"))?;
                let token = token.trim();
                if token.is_empty() {
                    return Err(ConfigError::EmptySecret("MOVIEBOX_MCP_TOKEN_FILE"));
                }
                Ok(token.to_string())
            })
            .transpose()
    }

    pub fn read_jellyfin_api_key(&self) -> Result<Option<String>, ConfigError> {
        self.jellyfin_api_key_file
            .as_deref()
            .map(|path| {
                let key = fs::read_to_string(path)
                    .map_err(|_| ConfigError::UnreadableSecret("MOVIEBOX_JELLYFIN_API_KEY_FILE"))?;
                let key = key.trim();
                if key.is_empty() {
                    return Err(ConfigError::EmptySecret("MOVIEBOX_JELLYFIN_API_KEY_FILE"));
                }
                Ok(key.to_string())
            })
            .transpose()
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{0} is required")]
    MissingVar(&'static str),
    #[error("{0} must be a valid socket address")]
    InvalidBind(&'static str),
    #[error("{0} must be a valid sqlite URL")]
    InvalidDatabaseUrl(&'static str),
    #[error("{0} must be an absolute directory path")]
    NonAbsoluteDirectory(&'static str),
    #[error("{0} must not point at /")]
    RootDirectory(&'static str),
    #[error("{0} must not use /mnt/mac-remote")]
    ForbiddenDirectory(&'static str),
    #[error("{0} must point at an existing directory")]
    MissingDirectory(&'static str),
    #[error("{0} must point at an absolute file path")]
    NonAbsoluteFile(&'static str),
    #[error("{0} must point at an existing file")]
    MissingFile(&'static str),
    #[error("{0} contains no usable secret data")]
    EmptySecret(&'static str),
    #[error("{0} must be owner-readable only")]
    UnsafeSecretPermissions(&'static str),
    #[error("{0} must be less than or equal to 1080")]
    InvalidMaximumHeight(&'static str),
    #[error("{0} must be exactly 1 for the private MVP")]
    InvalidDownloadConcurrency(&'static str),
    #[error("{0} must be at least 10 GiB")]
    InvalidReserve(&'static str),
    #[error("{0} must be text or json")]
    InvalidLogFormat(&'static str),
    #[error("{0} must be an absolute URL")]
    InvalidUrl(&'static str),
    #[error("{0} must be http or https and target loopback or the jellyfin service")]
    InvalidJellyfinBaseUrl(&'static str),
    #[error("{0} could not be read safely")]
    UnreadableSecret(&'static str),
}

fn required_var<'a>(
    env: &'a HashMap<String, String>,
    key: &'static str,
) -> Result<&'a str, ConfigError> {
    env.get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(ConfigError::MissingVar(key))
}

fn parse_bind_addr(value: &str) -> Result<SocketAddr, ConfigError> {
    value
        .parse::<SocketAddr>()
        .map_err(|_| ConfigError::InvalidBind("MOVIEBOX_BIND"))
}

fn parse_database_url(value: &str) -> Result<String, ConfigError> {
    if !value.starts_with("sqlite://") && !value.starts_with("sqlite:") {
        return Err(ConfigError::InvalidDatabaseUrl("MOVIEBOX_DATABASE_URL"));
    }

    SqliteConnectOptions::from_str(value)
        .map_err(|_| ConfigError::InvalidDatabaseUrl("MOVIEBOX_DATABASE_URL"))?;

    let database_part = value
        .strip_prefix("sqlite://")
        .or_else(|| value.strip_prefix("sqlite:"))
        .ok_or(ConfigError::InvalidDatabaseUrl("MOVIEBOX_DATABASE_URL"))?;
    let database_part = database_part
        .split_once('?')
        .map_or(database_part, |(path, _)| path);
    let database_path = percent_decode_str(database_part)
        .decode_utf8()
        .map_err(|_| ConfigError::InvalidDatabaseUrl("MOVIEBOX_DATABASE_URL"))?;
    let database_path = Path::new(database_path.as_ref());

    if database_path.as_os_str().is_empty()
        || database_path == Path::new(":memory:")
        || !database_path.is_absolute()
        || database_path == Path::new("/")
        || database_path.starts_with("/mnt/mac-remote")
    {
        return Err(ConfigError::InvalidDatabaseUrl("MOVIEBOX_DATABASE_URL"));
    }

    let parameters = value
        .split_once('?')
        .map(|(_, parameters)| parameters)
        .unwrap_or_default();
    if url::form_urlencoded::parse(parameters.as_bytes())
        .any(|(key, value)| key == "mode" && value == "memory")
    {
        return Err(ConfigError::InvalidDatabaseUrl("MOVIEBOX_DATABASE_URL"));
    }

    let parent = database_path
        .parent()
        .ok_or(ConfigError::InvalidDatabaseUrl("MOVIEBOX_DATABASE_URL"))?;
    validate_directory("MOVIEBOX_DATABASE_URL", parent)?;
    if database_path.exists() && !database_path.is_file() {
        return Err(ConfigError::InvalidDatabaseUrl("MOVIEBOX_DATABASE_URL"));
    }

    Ok(value.to_string())
}

fn validate_directory(var: &'static str, path: &Path) -> Result<PathBuf, ConfigError> {
    if !path.is_absolute() {
        return Err(ConfigError::NonAbsoluteDirectory(var));
    }
    if path == Path::new("/") {
        return Err(ConfigError::RootDirectory(var));
    }
    if path.starts_with("/mnt/mac-remote") {
        return Err(ConfigError::ForbiddenDirectory(var));
    }
    if !path.is_dir() {
        return Err(ConfigError::MissingDirectory(var));
    }
    Ok(path.to_path_buf())
}

fn validate_secret_file(var: &'static str, path: &Path) -> Result<PathBuf, ConfigError> {
    if !path.is_absolute() {
        return Err(ConfigError::NonAbsoluteFile(var));
    }
    if !path.is_file() {
        return Err(ConfigError::MissingFile(var));
    }

    let contents = fs::read_to_string(path).map_err(|_| ConfigError::UnreadableSecret(var))?;
    if contents.trim().is_empty() {
        return Err(ConfigError::EmptySecret(var));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(path)
            .map_err(|_| ConfigError::UnreadableSecret(var))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(ConfigError::UnsafeSecretPermissions(var));
        }
    }

    Ok(path.to_path_buf())
}

fn parse_maximum_height(value: &str) -> Result<u16, ConfigError> {
    let parsed = value
        .parse::<u16>()
        .map_err(|_| ConfigError::InvalidMaximumHeight("MOVIEBOX_MAX_HEIGHT"))?;
    if parsed > 1080 {
        return Err(ConfigError::InvalidMaximumHeight("MOVIEBOX_MAX_HEIGHT"));
    }
    Ok(parsed)
}

fn parse_download_concurrency(value: &str) -> Result<usize, ConfigError> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| ConfigError::InvalidDownloadConcurrency("MOVIEBOX_DOWNLOAD_CONCURRENCY"))?;
    if parsed != 1 {
        return Err(ConfigError::InvalidDownloadConcurrency(
            "MOVIEBOX_DOWNLOAD_CONCURRENCY",
        ));
    }
    Ok(parsed)
}

fn parse_reserve_gib(value: &str) -> Result<u16, ConfigError> {
    let parsed = value
        .parse::<u16>()
        .map_err(|_| ConfigError::InvalidReserve("MOVIEBOX_RESERVE_GIB"))?;
    if parsed < MINIMUM_RESERVE_GIB {
        return Err(ConfigError::InvalidReserve("MOVIEBOX_RESERVE_GIB"));
    }
    Ok(parsed)
}

fn parse_jellyfin_base_url(value: &str) -> Result<Url, ConfigError> {
    let url = Url::parse(value)
        .map_err(|_| ConfigError::InvalidJellyfinBaseUrl("MOVIEBOX_JELLYFIN_BASE_URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ConfigError::InvalidJellyfinBaseUrl(
            "MOVIEBOX_JELLYFIN_BASE_URL",
        ));
    }

    let allowed_host = match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost") || host == "jellyfin",
        None => false,
    };

    if !allowed_host {
        return Err(ConfigError::InvalidJellyfinBaseUrl(
            "MOVIEBOX_JELLYFIN_BASE_URL",
        ));
    }

    Ok(url)
}

fn optional_secret_file(
    env: &HashMap<String, String>,
    key: &'static str,
) -> Result<Option<PathBuf>, ConfigError> {
    match env
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        Some(value) => validate_secret_file(key, Path::new(value)).map(Some),
        None => Ok(None),
    }
}

fn parse_log_format(value: &str) -> Result<LogFormat, ConfigError> {
    match value {
        "text" => Ok(LogFormat::Text),
        "json" => Ok(LogFormat::Json),
        _ => Err(ConfigError::InvalidLogFormat("MOVIEBOX_LOG_FORMAT")),
    }
}

fn derive_session_pepper(path: &Path) -> Result<[u8; 32], ConfigError> {
    let secret = fs::read_to_string(path)
        .map_err(|_| ConfigError::UnreadableSecret("MOVIEBOX_SESSION_KEY_FILE"))?;
    if secret.trim().is_empty() {
        return Err(ConfigError::EmptySecret("MOVIEBOX_SESSION_KEY_FILE"));
    }

    let digest = Sha256::digest(secret.trim().as_bytes());
    let mut pepper = [0_u8; 32];
    pepper.copy_from_slice(&digest);
    Ok(pepper)
}
