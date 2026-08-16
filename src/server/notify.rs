//! Push notification for finished downloads.
//!
//! A download can take hours, so the useful moment is the one where it becomes
//! watchable. When a webhook is configured the server announces that moment —
//! "Cocktail 2 (2026) is ready to watch" — instead of leaving you to poll the
//! queue.
//!
//! The webhook URL carries its own credential, so it is read from a secret file
//! and never logged or echoed back through the API.

use std::time::Duration;

use reqwest::Client;
use serde_json::json;
use thiserror::Error;
use url::Url;

use crate::server::config::ServerConfig;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum NotifyError {
    #[error("notification webhook URL is unavailable")]
    SecretUnavailable,
    #[error("notification webhook URL must be an absolute https URL")]
    InvalidUrl,
}

/// Announces that a download is ready to watch.
#[async_trait::async_trait]
pub trait DownloadNotifier: Send + Sync {
    async fn notify_ready(&self, title: &str, year: Option<&str>);
}

/// Used when no webhook is configured.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopNotifier;

#[async_trait::async_trait]
impl DownloadNotifier for NoopNotifier {
    async fn notify_ready(&self, _title: &str, _year: Option<&str>) {}
}

/// Posts a completion message to a webhook that turns it into a phone notification.
///
/// The payload matches the common `{title, body, url}` shape, so it works with
/// Hark and with most other webhook-to-push relays.
pub struct WebhookNotifier {
    http: Client,
    webhook: Url,
    recipient: Option<String>,
    link: Option<Url>,
}

impl WebhookNotifier {
    /// Build a notifier, or `None` when no webhook is configured.
    pub fn from_config(config: &ServerConfig) -> Result<Option<Self>, NotifyError> {
        let Some(raw) = config
            .read_notify_webhook_url()
            .map_err(|_| NotifyError::SecretUnavailable)?
        else {
            return Ok(None);
        };
        let webhook = Url::parse(raw.trim()).map_err(|_| NotifyError::InvalidUrl)?;
        if !is_acceptable_webhook(&webhook) {
            return Err(NotifyError::InvalidUrl);
        }

        Ok(Some(Self {
            http: Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("fixed notifier HTTP client options are valid"),
            webhook,
            recipient: config.notify_recipient.clone(),
            link: config.notify_link_url.clone(),
        }))
    }

    /// Compose the message body shown on the phone.
    ///
    /// Split out from sending so the wording is testable without a network call.
    pub fn compose(recipient: Option<&str>, title: &str, year: Option<&str>) -> String {
        let named = match year {
            Some(year) if !year.trim().is_empty() => format!("{title} ({year})"),
            _ => title.to_string(),
        };
        match recipient {
            Some(name) if !name.trim().is_empty() => {
                format!("Hey {}, {named} is ready to watch.", name.trim())
            }
            _ => format!("{named} is ready to watch."),
        }
    }
}

/// Whether a webhook URL is safe to send the credential-bearing payload to.
///
/// The URL embeds its own token, so it must be encrypted in transit. Plain HTTP
/// is allowed only for a relay on this machine, where nothing leaves the host.
fn is_acceptable_webhook(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => matches!(
            url.host(),
            Some(url::Host::Domain("localhost"))
                | Some(url::Host::Ipv4(std::net::Ipv4Addr::LOCALHOST))
                | Some(url::Host::Ipv6(std::net::Ipv6Addr::LOCALHOST))
        ),
        _ => false,
    }
}

#[async_trait::async_trait]
impl DownloadNotifier for WebhookNotifier {
    async fn notify_ready(&self, title: &str, year: Option<&str>) {
        let mut payload = json!({
            "title": "MovieBox",
            "body": Self::compose(self.recipient.as_deref(), title, year),
            "project": "MovieBox",
        });
        if let Some(link) = &self.link {
            payload["url"] = json!(link.as_str());
        }

        // Best effort: the file is already in the library, so a failed
        // notification must never be mistaken for a failed download. The URL is
        // a credential, so errors are reported without it.
        match self
            .http
            .post(self.webhook.clone())
            .json(&payload)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => {
                eprintln!(
                    "[moviebox-server] completion notification rejected with HTTP {}",
                    response.status()
                );
            }
            Err(_) => {
                eprintln!("[moviebox-server] completion notification could not be delivered");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_greets_the_configured_recipient() {
        assert_eq!(
            WebhookNotifier::compose(Some("Nabeegh"), "Cocktail 2", Some("2026")),
            "Hey Nabeegh, Cocktail 2 (2026) is ready to watch."
        );
    }

    #[test]
    fn message_without_a_recipient_stays_impersonal() {
        assert_eq!(
            WebhookNotifier::compose(None, "Cocktail 2", Some("2026")),
            "Cocktail 2 (2026) is ready to watch."
        );
        assert_eq!(
            WebhookNotifier::compose(Some("   "), "Cocktail 2", Some("2026")),
            "Cocktail 2 (2026) is ready to watch."
        );
    }

    #[test]
    fn message_omits_a_missing_or_blank_year() {
        assert_eq!(
            WebhookNotifier::compose(Some("Nabeegh"), "Cocktail 2", None),
            "Hey Nabeegh, Cocktail 2 is ready to watch."
        );
        assert_eq!(
            WebhookNotifier::compose(Some("Nabeegh"), "Cocktail 2", Some("")),
            "Hey Nabeegh, Cocktail 2 is ready to watch."
        );
    }

    #[test]
    fn only_encrypted_or_local_webhooks_are_accepted() {
        for accepted in [
            "https://hark.example/hooks/whk_token",
            "http://localhost:8080/hook",
            "http://127.0.0.1:8080/hook",
        ] {
            assert!(
                is_acceptable_webhook(&Url::parse(accepted).unwrap()),
                "rejected {accepted}"
            );
        }
        for rejected in [
            "http://hark.example/hooks/whk_token",
            "http://192.168.1.10/hook",
            "ftp://example.com/hook",
            "file:///etc/passwd",
        ] {
            assert!(
                !is_acceptable_webhook(&Url::parse(rejected).unwrap()),
                "accepted {rejected}"
            );
        }
    }

    #[test]
    fn episode_titles_pass_through_unchanged() {
        assert_eq!(
            WebhookNotifier::compose(Some("Nabeegh"), "Severance S02E03", None),
            "Hey Nabeegh, Severance S02E03 is ready to watch."
        );
    }
}
