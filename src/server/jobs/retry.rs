//! Failure classification and backoff scheduling for download jobs.
//!
//! A download that dies partway through has usually consumed real time and real
//! bandwidth, so discarding it because of a momentary upstream problem is
//! expensive. Recoverable failures return the job to the queue with a backoff
//! delay and keep the partial data on disk; only genuinely permanent failures
//! terminate the job.

use std::time::Duration;

use crate::catalog::CatalogError;

/// Automatic attempts allowed before a recoverable failure becomes terminal.
///
/// `attempt` is incremented by `claim_next`, so this counts claims, not retries:
/// the job is claimed once normally and retried automatically up to five times.
pub const MAX_AUTOMATIC_ATTEMPTS: u32 = 6;

const BASE_BACKOFF: Duration = Duration::from_secs(15);
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Whether a failure is worth retrying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// Retrying cannot succeed; fail the job and tell the user why.
    Permanent,
    /// A later attempt has a realistic chance of succeeding.
    Transient,
}

impl FailureClass {
    pub fn is_transient(self) -> bool {
        matches!(self, Self::Transient)
    }
}

/// Classify a catalog resolution failure.
///
/// `Provider` and `InvalidPayload` cover upstream outages, rate limiting and
/// truncated or non-JSON responses — all of which routinely clear on their own.
/// Everything else describes a request that will fail identically forever.
pub fn classify_catalog_error(error: &CatalogError) -> FailureClass {
    match error {
        CatalogError::Provider(_) | CatalogError::InvalidPayload(_) => FailureClass::Transient,
        CatalogError::NotFound(_)
        | CatalogError::UnsupportedProvider(_)
        | CatalogError::InvalidOpaqueId
        | CatalogError::OpaqueIdVerificationFailed
        | CatalogError::SourceResolutionMismatch { .. }
        | CatalogError::QualityUnavailable { .. } => FailureClass::Permanent,
    }
}

/// Whether another automatic attempt is allowed after `attempt` claims.
pub fn may_retry(attempt: u32) -> bool {
    attempt < MAX_AUTOMATIC_ATTEMPTS
}

/// Delay before the next automatic attempt, doubling per attempt up to a cap.
pub fn backoff_delay(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(16);
    BASE_BACKOFF
        .saturating_mul(1_u32 << exponent)
        .min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_failures_are_transient() {
        assert_eq!(
            classify_catalog_error(&CatalogError::Provider("502".into())),
            FailureClass::Transient
        );
        assert_eq!(
            classify_catalog_error(&CatalogError::InvalidPayload("data")),
            FailureClass::Transient
        );
    }

    #[test]
    fn identity_failures_are_permanent() {
        assert_eq!(
            classify_catalog_error(&CatalogError::NotFound("source")),
            FailureClass::Permanent
        );
        assert_eq!(
            classify_catalog_error(&CatalogError::InvalidOpaqueId),
            FailureClass::Permanent
        );
        assert_eq!(
            classify_catalog_error(&CatalogError::QualityUnavailable {
                maximum_height: 1080,
                requested_height: Some(2160),
            }),
            FailureClass::Permanent
        );
    }

    #[test]
    fn backoff_grows_then_saturates() {
        assert_eq!(backoff_delay(1), Duration::from_secs(15));
        assert_eq!(backoff_delay(2), Duration::from_secs(30));
        assert_eq!(backoff_delay(3), Duration::from_secs(60));
        assert_eq!(backoff_delay(4), Duration::from_secs(120));
        assert_eq!(backoff_delay(5), Duration::from_secs(240));
        assert_eq!(backoff_delay(6), MAX_BACKOFF);
        assert_eq!(backoff_delay(99), MAX_BACKOFF);
    }

    #[test]
    fn retries_stop_at_the_attempt_ceiling() {
        assert!(may_retry(1));
        assert!(may_retry(MAX_AUTOMATIC_ATTEMPTS - 1));
        assert!(!may_retry(MAX_AUTOMATIC_ATTEMPTS));
        assert!(!may_retry(MAX_AUTOMATIC_ATTEMPTS + 1));
    }
}
