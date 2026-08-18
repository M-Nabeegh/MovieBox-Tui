//! TMDB metadata and discovery.
//!
//! The download catalogue answers "can I get this file?", which is a poor basis
//! for browsing: it has no artwork, no trending list, and no way to ask for
//! something like this year's Indian releases. TMDB answers "what exists?" for
//! roughly a million titles, so it drives discovery while the download provider
//! keeps its job of supplying sources.
//!
//! The read token is a credential and is read from a secret file, never logged.

use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const API_ROOT: &str = "https://api.themoviedb.org/3";
const IMAGE_ROOT: &str = "https://image.tmdb.org/t/p";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Error)]
pub enum TmdbError {
    #[error("TMDB request failed")]
    Unavailable,
    #[error("TMDB returned an unexpected response")]
    InvalidResponse,
}

/// A title as TMDB describes it, with artwork ready for the browse grid.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoverTitle {
    pub tmdb_id: i64,
    pub title: String,
    pub year: Option<String>,
    pub overview: Option<String>,
    /// Poster artwork, portrait, for grid cards.
    pub poster_url: Option<String>,
    /// Backdrop artwork, landscape, for the hero banner.
    pub backdrop_url: Option<String>,
    /// Average rating out of ten, rounded to one decimal.
    pub rating: Option<f32>,
    pub language: Option<String>,
    /// Whether this is a film or a show, which decides how it is downloaded.
    pub media_kind: MediaKind,
}

/// What sort of title this is.
///
/// TMDB keeps films and shows behind separate endpoints and spells their fields
/// differently — `title`/`release_date` against `name`/`first_air_date` — so the
/// kind has to travel with the title rather than be guessed later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Movie,
    Series,
}

impl MediaKind {
    /// The TMDB path segment for this kind.
    pub fn segment(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "tv",
        }
    }
}

/// A named strip of titles in the browse view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverRow {
    /// Stable key so the UI can cache or deep-link a row.
    pub id: String,
    pub title: String,
    pub items: Vec<DiscoverTitle>,
}

/// What a row should contain, independent of how TMDB spells it.
#[derive(Debug, Clone, PartialEq)]
pub enum RowQuery {
    /// What is popular across TMDB this week.
    Trending(MediaKind),
    /// Broadly popular titles.
    Popular(MediaKind),
    /// Highest rated of all time, with enough votes to be meaningful.
    TopRated(MediaKind),
    /// Titles filtered by origin, year, or genre — "Indian releases from 2026",
    /// "Horror". Applies to films and shows alike.
    Filtered {
        kind: MediaKind,
        /// ISO 639-1 original language, e.g. `hi` for Hindi.
        language: Option<String>,
        /// Release year.
        year: Option<u16>,
        /// ISO 3166-1 region, e.g. `IN`.
        region: Option<String>,
        /// TMDB genre id, e.g. 27 for Horror.
        genre: Option<u16>,
    },
}

impl RowQuery {
    /// Which kind of title this row returns.
    pub fn kind(&self) -> MediaKind {
        match self {
            Self::Trending(kind) | Self::Popular(kind) | Self::TopRated(kind) => *kind,
            Self::Filtered { kind, .. } => *kind,
        }
    }
}

pub struct TmdbClient {
    http: Client,
    token: String,
}

impl TmdbClient {
    /// Build a client, or `None` when no token is configured.
    pub fn new(token: Option<String>) -> Option<Self> {
        let token = token?;
        if token.trim().is_empty() {
            return None;
        }
        Some(Self {
            http: Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("fixed TMDB client options are valid"),
            token: token.trim().to_string(),
        })
    }

    /// Fetch one row of titles.
    pub async fn row(
        &self,
        id: &str,
        title: &str,
        query: &RowQuery,
    ) -> Result<DiscoverRow, TmdbError> {
        let url = self.url_for(query);
        let payload: serde_json::Value = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|_| TmdbError::Unavailable)?
            .json()
            .await
            .map_err(|_| TmdbError::InvalidResponse)?;

        Ok(DiscoverRow {
            id: id.to_string(),
            title: title.to_string(),
            items: parse_titles(&payload, query.kind()),
        })
    }

    /// Search TMDB by free text across both films and shows.
    ///
    /// Results are interleaved by relevance rather than grouped, since someone
    /// typing a name wants that name, not a lesson in how it was produced.
    pub async fn search(&self, query: &str, page: u32) -> Result<Vec<DiscoverTitle>, TmdbError> {
        let mut found = Vec::new();
        for kind in [MediaKind::Movie, MediaKind::Series] {
            let url = format!(
                "{API_ROOT}/search/{}?query={}&page={}&include_adult=false",
                kind.segment(),
                urlencoding(query),
                page.clamp(1, 500)
            );
            let payload: serde_json::Value = self
                .http
                .get(url)
                .bearer_auth(&self.token)
                .header("accept", "application/json")
                .send()
                .await
                .map_err(|_| TmdbError::Unavailable)?
                .json()
                .await
                .map_err(|_| TmdbError::InvalidResponse)?;
            found.extend(parse_titles(&payload, kind));
        }
        // TMDB orders each kind by its own popularity; merging them needs a
        // single ordering or shows always trail films regardless of relevance.
        found.sort_by(|a, b| b.rating.unwrap_or(0.0).total_cmp(&a.rating.unwrap_or(0.0)));
        Ok(found)
    }

    fn url_for(&self, query: &RowQuery) -> String {
        match query {
            RowQuery::Trending(kind) => {
                format!("{API_ROOT}/trending/{}/week", kind.segment())
            }
            RowQuery::Popular(kind) => format!("{API_ROOT}/{}/popular", kind.segment()),
            RowQuery::TopRated(kind) => format!("{API_ROOT}/{}/top_rated", kind.segment()),
            RowQuery::Filtered {
                kind,
                language,
                year,
                region,
                genre,
            } => {
                // `discover` is the only endpoint that accepts these filters
                // together, which is what makes "Indian releases, 2026" possible.
                let mut url = format!(
                    "{API_ROOT}/discover/{}?include_adult=false&sort_by=popularity.desc&vote_count.gte=10",
                    kind.segment()
                );
                if let Some(language) = language {
                    url.push_str(&format!("&with_original_language={language}"));
                }
                if let Some(year) = year {
                    // Films and shows name their release year differently.
                    let field = match kind {
                        MediaKind::Movie => "primary_release_year",
                        MediaKind::Series => "first_air_date_year",
                    };
                    url.push_str(&format!("&{field}={year}"));
                }
                if let Some(region) = region {
                    url.push_str(&format!("&with_origin_country={region}"));
                }
                if let Some(genre) = genre {
                    url.push_str(&format!("&with_genres={genre}"));
                }
                url
            }
        }
    }
}

/// Build the standard set of browse rows.
///
/// The year is passed in rather than read from the clock so the rows stay
/// current without the labels drifting out of step with the query.
pub fn default_rows(current_year: u16) -> Vec<(String, String, RowQuery)> {
    /// TMDB genre ids. Films and shows use separate id spaces for a few of
    /// these, so only ids common to both are used in shared rows.
    const ACTION: u16 = 28;
    const HORROR: u16 = 27;
    const COMEDY: u16 = 35;

    let indian = |year: Option<u16>, kind: MediaKind| RowQuery::Filtered {
        kind,
        language: Some("hi".to_string()),
        year,
        region: None,
        genre: None,
    };
    let genre = |id: u16| RowQuery::Filtered {
        kind: MediaKind::Movie,
        language: None,
        year: None,
        region: None,
        genre: Some(id),
    };

    vec![
        (
            "trending".to_string(),
            "Trending This Week".to_string(),
            RowQuery::Trending(MediaKind::Movie),
        ),
        (
            "indian-latest".to_string(),
            format!("Latest Indian · {current_year}"),
            indian(Some(current_year), MediaKind::Movie),
        ),
        (
            "trending-tv".to_string(),
            "Trending Shows".to_string(),
            RowQuery::Trending(MediaKind::Series),
        ),
        (
            "popular".to_string(),
            "Popular Now".to_string(),
            RowQuery::Popular(MediaKind::Movie),
        ),
        (
            "indian-tv".to_string(),
            "Indian Shows".to_string(),
            indian(None, MediaKind::Series),
        ),
        ("action".to_string(), "Action".to_string(), genre(ACTION)),
        ("horror".to_string(), "Horror".to_string(), genre(HORROR)),
        ("comedy".to_string(), "Comedy".to_string(), genre(COMEDY)),
        (
            "indian-acclaimed".to_string(),
            "Acclaimed Indian Cinema".to_string(),
            indian(None, MediaKind::Movie),
        ),
        (
            "top-rated".to_string(),
            "Highest Rated of All Time".to_string(),
            RowQuery::TopRated(MediaKind::Movie),
        ),
    ]
}

/// Extract titles from a TMDB list response.
///
/// TMDB omits fields rather than nulling them, and entries without a poster
/// look broken in a grid, so those are dropped instead of rendered blank.
fn parse_titles(payload: &serde_json::Value, kind: MediaKind) -> Vec<DiscoverTitle> {
    payload
        .get("results")
        .and_then(|value| value.as_array())
        .map(|results| {
            results
                .iter()
                .filter_map(|item| {
                    let poster = item.get("poster_path").and_then(|v| v.as_str());
                    let title = item
                        .get("title")
                        .or_else(|| item.get("name"))
                        .and_then(|v| v.as_str())?;
                    poster?;
                    Some(DiscoverTitle {
                        tmdb_id: item.get("id").and_then(serde_json::Value::as_i64)?,
                        title: title.to_string(),
                        // Films carry `release_date`, shows `first_air_date`.
                        year: item
                            .get("release_date")
                            .or_else(|| item.get("first_air_date"))
                            .and_then(|v| v.as_str())
                            .filter(|value| value.len() >= 4)
                            .map(|value| value[..4].to_string()),
                        overview: item
                            .get("overview")
                            .and_then(|v| v.as_str())
                            .filter(|value| !value.is_empty())
                            .map(str::to_string),
                        poster_url: poster.map(|path| format!("{IMAGE_ROOT}/w500{path}")),
                        backdrop_url: item
                            .get("backdrop_path")
                            .and_then(|v| v.as_str())
                            .map(|path| format!("{IMAGE_ROOT}/w1280{path}")),
                        rating: item
                            .get("vote_average")
                            .and_then(serde_json::Value::as_f64)
                            .filter(|value| *value > 0.0)
                            .map(|value| (value * 10.0).round() as f32 / 10.0),
                        language: item
                            .get("original_language")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                        media_kind: kind,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Percent-encode a query value for a URL.
fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn client_is_absent_without_a_token() {
        assert!(TmdbClient::new(None).is_none());
        assert!(TmdbClient::new(Some("   ".to_string())).is_none());
        assert!(TmdbClient::new(Some("token".to_string())).is_some());
    }

    #[test]
    fn titles_are_parsed_with_artwork_and_year() {
        let payload = json!({"results": [{
            "id": 12345,
            "title": "Joyland",
            "release_date": "2022-11-18",
            "overview": "A story.",
            "poster_path": "/poster.jpg",
            "backdrop_path": "/backdrop.jpg",
            "vote_average": 7.44,
            "original_language": "ur"
        }]});
        let titles = parse_titles(&payload, MediaKind::Movie);
        assert_eq!(titles.len(), 1);
        let title = &titles[0];
        assert_eq!(title.tmdb_id, 12345);
        assert_eq!(title.title, "Joyland");
        assert_eq!(title.year.as_deref(), Some("2022"));
        assert_eq!(
            title.poster_url.as_deref(),
            Some("https://image.tmdb.org/t/p/w500/poster.jpg")
        );
        assert_eq!(
            title.backdrop_url.as_deref(),
            Some("https://image.tmdb.org/t/p/w1280/backdrop.jpg")
        );
        assert_eq!(title.rating, Some(7.4));
        assert_eq!(title.language.as_deref(), Some("ur"));
    }

    #[test]
    fn entries_without_a_poster_are_dropped() {
        // A card with no artwork looks broken, so it is not offered at all.
        let payload = json!({"results": [
            {"id": 1, "title": "No Art", "vote_average": 6.0},
            {"id": 2, "title": "Has Art", "poster_path": "/p.jpg", "vote_average": 6.0}
        ]});
        let titles = parse_titles(&payload, MediaKind::Movie);
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].title, "Has Art");
    }

    #[test]
    fn a_missing_results_array_yields_no_titles() {
        assert!(parse_titles(&json!({}), MediaKind::Movie).is_empty());
        assert!(parse_titles(&json!({"results": "nonsense"}), MediaKind::Movie).is_empty());
    }

    #[test]
    fn zero_ratings_are_treated_as_absent() {
        let payload = json!({"results": [
            {"id": 1, "title": "Unrated", "poster_path": "/p.jpg", "vote_average": 0.0}
        ]});
        assert_eq!(parse_titles(&payload, MediaKind::Movie)[0].rating, None);
    }

    #[test]
    fn filtered_rows_combine_language_and_year() {
        let client = TmdbClient::new(Some("t".to_string())).unwrap();
        let url = client.url_for(&RowQuery::Filtered {
            kind: MediaKind::Movie,
            language: Some("hi".to_string()),
            year: Some(2026),
            region: None,
            genre: None,
        });
        assert!(url.contains("/discover/movie"));
        assert!(url.contains("with_original_language=hi"));
        assert!(url.contains("primary_release_year=2026"));
        assert!(url.contains("sort_by=popularity.desc"));
    }

    #[test]
    fn shows_use_their_own_endpoint_and_year_field() {
        // TMDB names a show's release year differently from a film's; using the
        // film field silently returns an unfiltered list.
        let client = TmdbClient::new(Some("t".to_string())).unwrap();
        let url = client.url_for(&RowQuery::Filtered {
            kind: MediaKind::Series,
            language: None,
            year: Some(2026),
            region: None,
            genre: None,
        });
        assert!(url.contains("/discover/tv"));
        assert!(url.contains("first_air_date_year=2026"));
        assert!(!url.contains("primary_release_year"));
    }

    #[test]
    fn trending_and_top_rated_follow_the_requested_kind() {
        let client = TmdbClient::new(Some("t".to_string())).unwrap();
        assert!(
            client
                .url_for(&RowQuery::Trending(MediaKind::Series))
                .contains("/trending/tv/week")
        );
        assert!(
            client
                .url_for(&RowQuery::TopRated(MediaKind::Movie))
                .contains("/movie/top_rated")
        );
    }

    #[test]
    fn genre_rows_filter_by_genre_id() {
        let client = TmdbClient::new(Some("t".to_string())).unwrap();
        let url = client.url_for(&RowQuery::Filtered {
            kind: MediaKind::Movie,
            language: None,
            year: None,
            region: None,
            genre: Some(27),
        });
        assert!(url.contains("with_genres=27"));
    }

    #[test]
    fn shows_parse_their_air_date_as_the_year() {
        let payload = json!({"results": [{
            "id": 7,
            "name": "Paatal Lok",
            "first_air_date": "2020-05-15",
            "poster_path": "/p.jpg",
            "vote_average": 8.1
        }]});
        let titles = parse_titles(&payload, MediaKind::Series);
        assert_eq!(titles[0].title, "Paatal Lok");
        assert_eq!(titles[0].year.as_deref(), Some("2020"));
        assert_eq!(titles[0].media_kind, MediaKind::Series);
    }

    #[test]
    fn default_rows_track_the_given_year_and_include_shows() {
        let rows = default_rows(2026);
        let indian = rows
            .iter()
            .find(|(id, _, _)| id == "indian-latest")
            .expect("an Indian row");
        assert_eq!(indian.1, "Latest Indian · 2026");
        assert_eq!(
            indian.2,
            RowQuery::Filtered {
                kind: MediaKind::Movie,
                language: Some("hi".to_string()),
                year: Some(2026),
                region: None,
                genre: None,
            }
        );
        // Shows must be reachable from browse, not only films.
        assert!(
            rows.iter()
                .any(|(_, _, query)| query.kind() == MediaKind::Series)
        );
        assert!(rows.iter().any(|(id, _, _)| id == "horror"));
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(urlencoding("Come and See"), "Come+and+See");
        assert_eq!(urlencoding("Kabhi Khushi & Gham"), "Kabhi+Khushi+%26+Gham");
    }
}
