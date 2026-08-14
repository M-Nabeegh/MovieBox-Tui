//! Deterministic provider contract used by browser tests.
//!
//! The Playwright spec owns the HTTP boundary so no live MovieBox request is
//! possible. These identifiers intentionally match the browser fixture data.

pub const CATALOG_ID: &str = "fixture-movie-2024";
pub const SOURCE_ID_1080P: &str = "fixture-source-1080";
pub const SUBTITLE_ID_EN: &str = "fixture-sub-en";
pub const FIXTURE_PASSWORD: &str = "fixture-secret";
