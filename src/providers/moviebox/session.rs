//! In-memory visitor sessions for the authenticated MovieBox API.
//!
//! A visitor token is an authorization secret.  It deliberately never leaves
//! this process: no disk cache, debug formatter, log line, or error message is
//! allowed to retain it.

use base64::Engine;
use serde_json::Value;

#[derive(Clone, PartialEq, Eq)]
pub struct MovieBoxSession {
    token: String,
    user_id: Option<String>,
    expires_at: Option<u64>,
    created_at: u64,
}

impl MovieBoxSession {
    pub fn new(token: String, user_id: Option<String>, expires_at: Option<u64>) -> Self {
        Self {
            token,
            user_id,
            expires_at,
            created_at: now_seconds(),
        }
    }

    pub fn from_token_and_payload(token: String, explicit_uid: Option<String>) -> Self {
        let (jwt_uid, jwt_exp) = parse_jwt_claims(&token);
        Self::new(token, explicit_uid.or(jwt_uid), jwt_exp)
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn user_id(&self) -> Option<&str> {
        self.user_id.as_deref()
    }

    pub fn is_valid(&self) -> bool {
        if self.token.trim().is_empty() {
            return false;
        }
        let now = now_seconds();
        if let Some(expires_at) = self.expires_at {
            // Refresh a minute before expiry so a request cannot start with a
            // token that expires while the API is processing it.
            now.saturating_add(60) < expires_at
        } else {
            // Tokens without an exp claim are bounded to one process-local
            // week instead of being treated as immortal credentials.
            now < self.created_at.saturating_add(7 * 24 * 3600)
        }
    }
}

pub fn parse_jwt_claims(token: &str) -> (Option<String>, Option<u64>) {
    let Some(payload) = token.split('.').nth(1) else {
        return (None, None);
    };
    let padding = (4 - payload.len() % 4) % 4;
    let padded = format!("{payload}{}", "=".repeat(padding));
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&padded))
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(&padded));
    let Ok(decoded) = decoded else {
        return (None, None);
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&decoded) else {
        return (None, None);
    };

    let user_id = ["userId", "uid", "sub"].into_iter().find_map(|key| {
        payload.get(key).and_then(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .or_else(|| value.as_u64().map(|number| number.to_string()))
                .or_else(|| value.as_i64().map(|number| number.to_string()))
        })
    });
    let expires_at = payload.get("exp").and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
            .or_else(|| value.as_str().and_then(|number| number.parse().ok()))
    });
    (user_id, expires_at)
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claims_without_retaining_a_renderable_token() {
        let payload = r#"{"userId":"fixture-visitor","exp":4102444800}"#;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        let token = format!("header.{encoded}.signature");
        let (user_id, expiry) = parse_jwt_claims(&token);
        assert_eq!(user_id.as_deref(), Some("fixture-visitor"));
        assert_eq!(expiry, Some(4102444800));
    }

    #[test]
    fn expiry_and_empty_tokens_are_rejected() {
        assert!(!MovieBoxSession::new("".into(), None, Some(u64::MAX)).is_valid());
        assert!(!MovieBoxSession::new("fixture".into(), None, Some(0)).is_valid());
        assert!(MovieBoxSession::new("fixture".into(), None, Some(u64::MAX)).is_valid());
    }
}
