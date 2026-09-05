use crate::catalog::CatalogProvider;
use crate::catalog::models::{
    AudioOption, CatalogDetails, CatalogError, CatalogId, CatalogItem, EpisodeInfo, EpisodeRequest,
    MediaType, OpaqueIdCodec, OpaquePayload, QualityPolicy, ResolvedSource, ResolvedSubtitle,
    SearchPage, SeasonInfo, SourceId, SourceOption, SourceTransport, SubtitleId, SubtitleTrack,
};
use crate::providers::moviebox::clean_moviebox_title;
use crate::providers::moviebox::client::{MovieBoxClient, ScraperError};
use async_trait::async_trait;
use base64::Engine;
use reqwest::header::{COOKIE, HeaderMap, HeaderValue, REFERER};
use serde_json::Value;
use std::{borrow::Cow, net::IpAddr};
use url::Url;

const MOVIEBOX_PROVIDER: &str = "moviebox";

type DecodedSource = (
    String,
    String,
    String,
    Option<u16>,
    Option<u16>,
    u16,
    Option<String>,
);

type DecodedSubtitle = (String, String, String, u16, String, Option<String>);

fn extract_subjects(payload: &Value) -> Result<&Vec<Value>, CatalogError> {
    payload
        .get("results")
        .and_then(Value::as_array)
        .and_then(|groups| groups.first())
        .and_then(|group| group.get("subjects"))
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidPayload("results[0].subjects"))
}

fn search_has_more(payload: &Value) -> bool {
    payload
        .get("pager")
        .and_then(Value::as_object)
        .and_then(|pager| pager.get("hasMore").or_else(|| pager.get("has_more")))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn media_type_from_value(subject_type: i64) -> MediaType {
    if subject_type == 2 {
        MediaType::Series
    } else {
        MediaType::Movie
    }
}

fn string_field<'a>(value: &'a Value, key: &'static str) -> Result<&'a str, CatalogError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or(CatalogError::InvalidPayload(key))
}

fn optional_string(value: &Value, key: &'static str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn optional_year(value: &Value) -> Option<String> {
    value
        .get("releaseDate")
        .or_else(|| value.get("year"))
        .and_then(|field| {
            field
                .as_str()
                .map(str::to_string)
                .or_else(|| field.as_i64().map(|number| number.to_string()))
        })
        .map(|raw| raw.chars().take(4).collect::<String>())
        .filter(|year| !year.is_empty())
}

fn parse_episode_numbers(season: &Value) -> Vec<u16> {
    if let Some(all) = season.get("allEp").and_then(Value::as_str) {
        let episodes = all
            .split(',')
            .filter_map(|entry| entry.trim().parse::<u16>().ok())
            .collect::<Vec<_>>();
        if !episodes.is_empty() {
            return episodes;
        }
    }

    let max = season
        .get("maxEp")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(1);
    (1..=max).collect()
}

fn parse_seasons(payload: &Value) -> Vec<SeasonInfo> {
    payload
        .get("seasons")
        .and_then(|wrapper| wrapper.get("seasons"))
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|seasons| seasons.iter())
        .filter_map(|season| {
            let number = season
                .get("se")
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())?;
            let episodes = parse_episode_numbers(season)
                .into_iter()
                .map(|number| EpisodeInfo {
                    number,
                    title: None,
                })
                .collect::<Vec<_>>();
            Some(SeasonInfo { number, episodes })
        })
        .collect()
}

fn parse_audio_options(payload: &Value, codec: &OpaqueIdCodec) -> Vec<AudioOption> {
    payload
        .get("dubs")
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|dubs| dubs.iter())
        .filter_map(|dub| {
            let subject_id = dub.get("subjectId").and_then(Value::as_str)?;
            let label = dub
                .get("lanName")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("Unknown")
                .to_string();
            Some(AudioOption {
                id: CatalogId::new(codec.encode(&OpaquePayload::Catalog {
                    provider: MOVIEBOX_PROVIDER.to_string(),
                    subject_id: subject_id.to_string(),
                })),
                original: label.eq_ignore_ascii_case("original"),
                label,
            })
        })
        .collect()
}

fn source_items(payload: &Value) -> Result<&Vec<Value>, CatalogError> {
    payload
        .get("list")
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidPayload("list"))
}

fn source_subject_id(payload: &Value) -> Result<&str, CatalogError> {
    payload
        .get("subjectId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(CatalogError::InvalidPayload("subjectId"))
}

fn parse_u16_field(value: &Value, key: &'static str) -> Result<u16, CatalogError> {
    value
        .get(key)
        .and_then(|field| {
            field
                .as_u64()
                .and_then(|number| u16::try_from(number).ok())
                .or_else(|| field.as_str().and_then(|text| text.parse::<u16>().ok()))
        })
        .ok_or(CatalogError::InvalidPayload(key))
}

pub fn validate_source_item_resolution(
    item: &Value,
    signed_height: u16,
    policy: QualityPolicy,
) -> Result<u16, CatalogError> {
    let actual_height = parse_u16_field(item, "resolution")?;
    policy.validate(actual_height)?;
    if actual_height != signed_height {
        return Err(CatalogError::SourceResolutionMismatch {
            signed_height,
            actual_height,
        });
    }
    Ok(actual_height)
}

fn parse_optional_u16_field(value: &Value, key: &'static str) -> Option<u16> {
    value.get(key).and_then(|field| {
        field
            .as_u64()
            .and_then(|number| u16::try_from(number).ok())
            .or_else(|| field.as_str().and_then(|text| text.parse::<u16>().ok()))
    })
}

fn parse_optional_u64_field(value: &Value, key: &'static str) -> Option<u64> {
    value.get(key).and_then(|field| {
        field
            .as_u64()
            .or_else(|| field.as_i64().and_then(|number| u64::try_from(number).ok()))
            .or_else(|| field.as_str().and_then(|text| text.parse::<u64>().ok()))
    })
}

fn stream_text<'a>(stream: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| stream.get(*key).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
}

fn stream_number(stream: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        stream.get(*key).and_then(|field| {
            field
                .as_f64()
                .or_else(|| field.as_str().and_then(|value| value.parse().ok()))
        })
    })
}

fn stream_height_values(stream: &Value) -> Vec<u16> {
    let mut values = Vec::new();
    for key in ["resolution", "height"] {
        if let Some(value) = stream.get(key) {
            if let Some(number) = value
                .as_u64()
                .and_then(|number| u16::try_from(number).ok())
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
            {
                values.push(number);
            }
        }
    }
    if let Some(value) = stream.get("resolutions") {
        match value {
            Value::String(text) => values.extend(
                text.split(',')
                    .filter_map(|part| part.trim().parse::<u16>().ok()),
            ),
            Value::Array(items) => values.extend(items.iter().filter_map(|item| {
                item.as_u64()
                    .and_then(|number| u16::try_from(number).ok())
                    .or_else(|| item.as_str().and_then(|text| text.parse().ok()))
            })),
            _ => {}
        }
    }
    values.sort_unstable_by(|left, right| right.cmp(left));
    values.dedup();
    values
}

fn candidate_height_for_stream(stream: &Value, data: &Value, requested_height: u16) -> Option<u16> {
    let mut heights = stream_height_values(stream);
    if heights.is_empty() {
        if let Some(value) = data.get("displayResolutions") {
            match value {
                Value::String(text) => heights.extend(
                    text.split(',')
                        .filter_map(|part| part.trim().parse::<u16>().ok()),
                ),
                Value::Array(items) => heights.extend(items.iter().filter_map(|item| {
                    item.as_u64()
                        .and_then(|number| u16::try_from(number).ok())
                        .or_else(|| item.as_str().and_then(|text| text.parse().ok()))
                })),
                _ => {}
            }
        }
    }
    heights
        .into_iter()
        .filter(|height| *height <= requested_height)
        .max()
        .or_else(|| stream_number(stream, &["resolution", "height"]).map(|value| value as u16))
}

/// Decode a signed CloudFront policy and derive the DASH manifest URL.
///
/// The legacy URL field is intentionally ignored. It is where MovieBox has
/// historically returned the short upgrade/notice clip.
pub fn resolve_dash_manifest_from_policy(sign_cookie: &str) -> Option<Url> {
    let encoded = sign_cookie.split(';').find_map(|part| {
        part.trim()
            .strip_prefix("CloudFront-Policy=")
            .filter(|value| !value.trim().is_empty())
    })?;
    let padding = (4 - encoded.len() % 4) % 4;
    let padded = format!("{encoded}{}", "=".repeat(padding));
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&padded))
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(&padded))
        .ok()?;
    let policy: Value = serde_json::from_slice(&bytes).ok()?;
    let resource = policy
        .get("Statement")
        .and_then(Value::as_array)
        .and_then(|statements| statements.first())
        .and_then(|statement| statement.get("Resource"))
        .and_then(Value::as_str)?;
    let resource = resource.trim_end_matches('*').trim_end_matches('/');
    let mut manifest = Url::parse(&format!("{resource}/index.mpd")).ok()?;
    if !is_public_dash_url(&manifest) {
        return None;
    }
    manifest.set_query(None);
    manifest.set_fragment(None);
    Some(manifest)
}

fn is_public_dash_url(url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.ends_with(".local")
    {
        return false;
    }
    let Some(address) = host.parse::<IpAddr>().ok() else {
        return true;
    };
    match address {
        IpAddr::V4(address) => {
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_private()
                && !address.is_link_local()
        }
        IpAddr::V6(address) => {
            let first = address.segments()[0];
            !address.is_loopback()
                && !address.is_unspecified()
                && (first & 0xfe00) != 0xfc00
                && (first & 0xffc0) != 0xfe80
        }
    }
}

/// Adapt one authenticated play-info/v2 payload into the internal DASH
/// transfer contract. Exact resource identity remains preferred. When the
/// provider's episode-scoped payload contains one nonmatching stream, the
/// caller's prior `get_resources`/source-item validation is the subject and
/// resource identity precondition; this function then requires that sole
/// stream's declared height, episode fields (when present), finite duration,
/// and signed policy to be valid. No legacy notice URL is ever selected.
pub fn resolve_play_info_source(
    payload: &Value,
    resource_id: &str,
    season: Option<u16>,
    episode: Option<u16>,
    requested_height: u16,
    user_agent: &str,
) -> Result<ResolvedSource, CatalogError> {
    if requested_height == 0 || requested_height > 1080 {
        return Err(CatalogError::QualityUnavailable {
            maximum_height: 1080,
            requested_height: Some(requested_height),
        });
    }
    let data = payload.get("data").unwrap_or(payload);
    let streams = data
        .get("streams")
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidPayload("streams"))?;
    let mut invalid_policy_for_match = false;
    let mut candidates = Vec::new();
    let mut saw_exact_identity = false;

    for stream in streams {
        let id = stream_text(
            stream,
            &["resourceId", "resource_id", "streamId", "stream_id", "id"],
        );
        if id != Some(resource_id) {
            continue;
        }
        saw_exact_identity = true;
        let stream_se = ["se", "season", "seasonNumber"]
            .iter()
            .find_map(|&key| parse_optional_u16_field(stream, key));
        let stream_ep = ["ep", "episode", "episodeNumber"]
            .iter()
            .find_map(|&key| parse_optional_u16_field(stream, key));
        if season.is_some_and(|wanted| stream_se != Some(wanted))
            || episode.is_some_and(|wanted| stream_ep != Some(wanted))
        {
            continue;
        }
        let candidate_height = candidate_height_for_stream(stream, data, requested_height);
        if candidate_height.is_none_or(|height| height > requested_height) {
            continue;
        }
        let Some(sign_cookie) = stream.get("signCookie").and_then(Value::as_str) else {
            invalid_policy_for_match = true;
            continue;
        };
        let Some(manifest_url) = resolve_dash_manifest_from_policy(sign_cookie) else {
            invalid_policy_for_match = true;
            continue;
        };
        if HeaderValue::from_str(sign_cookie).is_err() {
            invalid_policy_for_match = true;
            continue;
        }
        candidates.push((
            candidate_height.unwrap_or(requested_height),
            stream,
            sign_cookie,
            manifest_url,
        ));
    }

    if candidates.is_empty() && !saw_exact_identity && streams.len() == 1 {
        let stream = &streams[0];
        let Some(id) = stream_text(
            stream,
            &["resourceId", "resource_id", "streamId", "stream_id", "id"],
        ) else {
            return Err(CatalogError::NotFound("source"));
        };
        if id == resource_id {
            return Err(CatalogError::NotFound("source"));
        }
        let stream_se = ["se", "season", "seasonNumber"]
            .iter()
            .find_map(|&key| parse_optional_u16_field(stream, key));
        let stream_ep = ["ep", "episode", "episodeNumber"]
            .iter()
            .find_map(|&key| parse_optional_u16_field(stream, key));
        if season.is_some_and(|wanted| stream_se.is_some_and(|actual| actual != wanted))
            || episode.is_some_and(|wanted| stream_ep.is_some_and(|actual| actual != wanted))
        {
            return Err(CatalogError::NotFound("source"));
        }
        let Some(candidate_height) = candidate_height_for_stream(stream, data, requested_height)
        else {
            return Err(CatalogError::NotFound("source"));
        };
        if candidate_height > requested_height {
            return Err(CatalogError::NotFound("source"));
        }
        let Some(sign_cookie) = stream.get("signCookie").and_then(Value::as_str) else {
            return Err(CatalogError::InvalidPayload("signed CloudFront policy"));
        };
        let Some(manifest_url) = resolve_dash_manifest_from_policy(sign_cookie) else {
            return Err(CatalogError::InvalidPayload("signed CloudFront policy"));
        };
        if HeaderValue::from_str(sign_cookie).is_err() {
            return Err(CatalogError::InvalidPayload("signed cookie"));
        }
        candidates.push((candidate_height, stream, sign_cookie, manifest_url));
    }

    candidates.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.3.as_str().cmp(right.3.as_str()))
    });
    let Some((height, stream, sign_cookie, url)) = candidates.into_iter().next() else {
        if invalid_policy_for_match {
            return Err(CatalogError::InvalidPayload("signed CloudFront policy"));
        }
        return Err(CatalogError::NotFound("source"));
    };

    let expected_duration_seconds =
        stream_number(stream, &["duration", "durationSeconds", "duration_seconds"]);
    if expected_duration_seconds.is_none_or(|duration| {
        !duration.is_finite() || duration <= 0.0 || (duration - 20.97).abs() <= 0.5
    }) {
        return Err(CatalogError::InvalidPayload("duration"));
    }

    let mut headers = HeaderMap::new();
    headers.insert(REFERER, HeaderValue::from_static("https://sportslive.wine"));
    let user_agent = HeaderValue::from_str(user_agent)
        .map_err(|_| CatalogError::InvalidPayload("user-agent"))?;
    headers.insert(reqwest::header::USER_AGENT, user_agent);
    let mut cookie = HeaderValue::from_str(sign_cookie)
        .map_err(|_| CatalogError::InvalidPayload("signed cookie"))?;
    cookie.set_sensitive(true);
    headers.insert(COOKIE, cookie);

    Ok(ResolvedSource {
        url,
        headers,
        subtitle: None,
        extension: "mkv".to_string(),
        expected_size: None,
        catalog_size_bytes: parse_optional_u64_field(stream, "sizeBytes")
            .or_else(|| parse_optional_u64_field(stream, "size")),
        transport: SourceTransport::Dash {
            maximum_height: requested_height.min(height),
            expected_duration_seconds,
        },
    })
}

fn parse_extension(url: &str, fallback: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()
                .and_then(|mut segments| segments.next_back().map(ToOwned::to_owned))
        })
        .and_then(|filename| {
            filename
                .rsplit_once('.')
                .map(|(_, extension)| extension.to_string())
        })
        .filter(|extension| !extension.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn ensure_object_with_subject_id(mut payload: Value, subject_id: &str) -> Value {
    if let Value::Object(ref mut map) = payload
        && !map.contains_key("subjectId")
    {
        map.insert(
            "subjectId".to_string(),
            Value::String(subject_id.to_string()),
        );
    }
    payload
}

fn ensure_object_ids(mut payload: Value, subject_id: &str, resource_id: &str) -> Value {
    if let Value::Object(ref mut map) = payload {
        map.entry("subjectId".to_string())
            .or_insert_with(|| Value::String(subject_id.to_string()));
        map.entry("resourceId".to_string())
            .or_insert_with(|| Value::String(resource_id.to_string()));
    }
    payload
}

fn decode_catalog(codec: &OpaqueIdCodec, id: &CatalogId) -> Result<(String, String), CatalogError> {
    match codec.decode(id.as_str())? {
        OpaquePayload::Catalog {
            provider,
            subject_id,
        } => Ok((provider, subject_id)),
        _ => Err(CatalogError::InvalidOpaqueId),
    }
}

fn decode_source(codec: &OpaqueIdCodec, id: &SourceId) -> Result<DecodedSource, CatalogError> {
    match codec.decode(id.as_str())? {
        OpaquePayload::Source {
            provider,
            subject_id,
            resource_id,
            season,
            episode,
            height,
            language,
        } => Ok((
            provider,
            subject_id,
            resource_id,
            season,
            episode,
            height,
            language,
        )),
        _ => Err(CatalogError::InvalidOpaqueId),
    }
}

fn decode_subtitle(
    codec: &OpaqueIdCodec,
    id: &SubtitleId,
) -> Result<DecodedSubtitle, CatalogError> {
    match codec.decode(id.as_str())? {
        OpaquePayload::Subtitle {
            provider,
            subject_id,
            resource_id,
            index,
            language,
            format,
        } => Ok((provider, subject_id, resource_id, index, language, format)),
        _ => Err(CatalogError::InvalidOpaqueId),
    }
}

/// Adapts one MovieBox search response. Only an explicit boolean `pager.hasMore`
/// or `pager.has_more` is trusted; without it, `has_more` stays false rather
/// than being inferred from the number of returned items.
pub fn adapt_search_page(
    payload: &Value,
    codec: &OpaqueIdCodec,
    page: u32,
) -> Result<SearchPage, CatalogError> {
    let items = extract_subjects(payload)?
        .iter()
        .filter_map(|subject| {
            let subject_id = subject.get("subjectId").and_then(Value::as_str)?;
            let raw_title = subject.get("title").and_then(Value::as_str)?;
            let subject_type = subject
                .get("subjectType")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            Some(CatalogItem {
                id: CatalogId::new(codec.encode(&OpaquePayload::Catalog {
                    provider: MOVIEBOX_PROVIDER.to_string(),
                    subject_id: subject_id.to_string(),
                })),
                title: clean_moviebox_title(raw_title),
                media_type: media_type_from_value(subject_type),
                year: optional_year(subject),
                season_count: subject
                    .get("season")
                    .and_then(Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok()),
            })
        })
        .collect::<Vec<_>>();

    Ok(SearchPage {
        page,
        items,
        has_more: search_has_more(payload),
    })
}

pub fn adapt_details(
    payload: &Value,
    codec: &OpaqueIdCodec,
) -> Result<CatalogDetails, CatalogError> {
    let subject_id = string_field(payload, "subjectId")?;
    let title = clean_moviebox_title(string_field(payload, "title")?);
    let media_type = media_type_from_value(
        payload
            .get("subjectType")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
    );

    Ok(CatalogDetails {
        id: CatalogId::new(codec.encode(&OpaquePayload::Catalog {
            provider: MOVIEBOX_PROVIDER.to_string(),
            subject_id: subject_id.to_string(),
        })),
        title,
        media_type,
        year: optional_year(payload),
        description: optional_string(payload, "description"),
        tagline: optional_string(payload, "tagline"),
        imdb_rating: optional_string(payload, "imdbRatingValue"),
        duration: optional_string(payload, "duration"),
        genres: payload
            .get("genre")
            .and_then(Value::as_array)
            .map(|genres| {
                genres
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        country: optional_string(payload, "countryName"),
        seasons: parse_seasons(payload),
        audio_options: parse_audio_options(payload, codec),
    })
}

pub fn adapt_sources(
    payload: &Value,
    codec: &OpaqueIdCodec,
    policy: QualityPolicy,
) -> Result<Vec<SourceOption>, CatalogError> {
    let subject_id = source_subject_id(payload)?;
    let mut options = Vec::new();
    for item in source_items(payload)? {
        let resource_id = item
            .get("resourceId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(CatalogError::InvalidPayload("resourceId"))?;
        let height = parse_u16_field(item, "resolution")?;
        if policy.validate(height).is_err() {
            continue;
        }

        let codec_name = optional_string(item, "codecName");
        let mut label = format!("{height}p");
        if let Some(codec_name) = codec_name.as_deref() {
            label = format!("{label} {}", codec_name.to_ascii_uppercase());
        }

        options.push(SourceOption {
            id: SourceId::new(codec.encode(&OpaquePayload::Source {
                provider: MOVIEBOX_PROVIDER.to_string(),
                subject_id: subject_id.to_string(),
                resource_id: resource_id.to_string(),
                season: parse_optional_u16_field(item, "se"),
                episode: parse_optional_u16_field(item, "ep"),
                height,
                language: optional_string(item, "lanName"),
            })),
            height,
            label,
            size_bytes: parse_optional_u64_field(item, "sizeBytes")
                .or_else(|| parse_optional_u64_field(item, "size")),
            language: optional_string(item, "lanName"),
            recommended: false,
            quality: crate::catalog::QualityTier::Good,
        });
    }

    // Order by real picture quality, not resolution alone, so the recommended
    // source is the one that actually looks best.
    crate::catalog::quality::rank(&mut options);
    if options.is_empty() {
        return Err(CatalogError::QualityUnavailable {
            maximum_height: policy.maximum_height(),
            requested_height: None,
        });
    }

    Ok(options)
}

pub fn adapt_subtitles(
    payload: &Value,
    source_id: &SourceId,
    codec: &OpaqueIdCodec,
) -> Result<Vec<SubtitleTrack>, CatalogError> {
    let (provider, subject_id, resource_id, _, _, _, _) = decode_source(codec, source_id)?;
    if provider != MOVIEBOX_PROVIDER {
        return Err(CatalogError::UnsupportedProvider(provider));
    }

    let subtitles = payload
        .get("extCaptions")
        .and_then(Value::as_array)
        .map(|captions| {
            captions
                .iter()
                .enumerate()
                .filter_map(|(index, caption)| {
                    let url = caption.get("url").and_then(Value::as_str)?;
                    if url.is_empty() {
                        return None;
                    }
                    let language = caption
                        .get("lanName")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("Unknown")
                        .to_string();
                    let format = optional_string(caption, "format")
                        .or_else(|| Some(parse_extension(url, "srt")));
                    Some(SubtitleTrack {
                        id: SubtitleId::new(codec.encode(&OpaquePayload::Subtitle {
                            provider: MOVIEBOX_PROVIDER.to_string(),
                            subject_id: subject_id.clone(),
                            resource_id: resource_id.clone(),
                            index: u16::try_from(index).ok()?,
                            language: language.clone(),
                            format: format.clone(),
                        })),
                        language,
                        format,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(subtitles)
}

fn find_source_item<'a>(payload: &'a Value, resource_id: &str) -> Option<&'a Value> {
    payload
        .get("list")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find(|item| {
                item.get("resourceId")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value == resource_id)
            })
        })
}

fn resolve_subtitle_from_payload(
    payload: &Value,
    index: u16,
) -> Result<Option<ResolvedSubtitle>, CatalogError> {
    let Some(captions) = payload.get("extCaptions").and_then(Value::as_array) else {
        return Ok(None);
    };
    let Some(caption) = captions.get(usize::from(index)) else {
        return Err(CatalogError::NotFound("subtitle"));
    };
    let url = Url::parse(string_field(caption, "url")?)
        .map_err(|_| CatalogError::InvalidPayload("url"))?;
    let language = caption
        .get("lanName")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("Unknown")
        .to_string();
    let extension =
        optional_string(caption, "format").unwrap_or_else(|| parse_extension(url.as_str(), "srt"));

    Ok(Some(ResolvedSubtitle {
        url,
        headers: HeaderMap::new(),
        language,
        extension,
    }))
}

pub struct MovieBoxCatalogProvider {
    client: MovieBoxClient,
    codec: OpaqueIdCodec,
    quality_policy: QualityPolicy,
}

impl MovieBoxCatalogProvider {
    pub fn new(
        client: MovieBoxClient,
        codec: OpaqueIdCodec,
        quality_policy: QualityPolicy,
    ) -> Self {
        Self {
            client,
            codec,
            quality_policy,
        }
    }

    fn ensure_moviebox_provider<'a>(&self, provider: Cow<'a, str>) -> Result<(), CatalogError> {
        if provider.as_ref() == MOVIEBOX_PROVIDER {
            Ok(())
        } else {
            Err(CatalogError::UnsupportedProvider(provider.into_owned()))
        }
    }
}

impl From<ScraperError> for CatalogError {
    fn from(value: ScraperError) -> Self {
        CatalogError::Provider(value.to_string())
    }
}

#[async_trait]
impl CatalogProvider for MovieBoxCatalogProvider {
    async fn search(&self, query: &str, page: u32) -> Result<SearchPage, CatalogError> {
        let payload = self.client.search(query, page as usize).await?;
        adapt_search_page(&payload, &self.codec, page)
    }

    async fn details(&self, id: &CatalogId) -> Result<CatalogDetails, CatalogError> {
        let (provider, subject_id) = decode_catalog(&self.codec, id)?;
        self.ensure_moviebox_provider(Cow::Owned(provider))?;
        let payload = self.client.get_details(&subject_id).await?;
        adapt_details(&payload, &self.codec)
    }

    async fn sources(&self, request: EpisodeRequest) -> Result<Vec<SourceOption>, CatalogError> {
        let (provider, subject_id) = decode_catalog(&self.codec, &request.catalog_id)?;
        self.ensure_moviebox_provider(Cow::Owned(provider))?;
        let payload = self
            .client
            .get_resources(
                &subject_id,
                usize::from(request.season.unwrap_or_default()),
                usize::from(request.episode.unwrap_or_default()),
                1,
                None,
            )
            .await?;
        let payload = ensure_object_with_subject_id(payload, &subject_id);
        adapt_sources(&payload, &self.codec, self.quality_policy)
    }

    async fn subtitles(&self, source: &SourceId) -> Result<Vec<SubtitleTrack>, CatalogError> {
        let (provider, subject_id, resource_id, _, _, _, _) = decode_source(&self.codec, source)?;
        self.ensure_moviebox_provider(Cow::Owned(provider))?;
        let payload = self
            .client
            .get_ext_captions(&subject_id, &resource_id)
            .await?;
        let payload = ensure_object_ids(payload, &subject_id, &resource_id);
        adapt_subtitles(&payload, source, &self.codec)
    }

    async fn resolve(
        &self,
        source: &SourceId,
        subtitle: Option<&SubtitleId>,
    ) -> Result<ResolvedSource, CatalogError> {
        let (provider, subject_id, resource_id, season, episode, height, _) =
            decode_source(&self.codec, source)?;
        self.ensure_moviebox_provider(Cow::Owned(provider))?;
        self.quality_policy.validate(height)?;

        let payload = self
            .client
            .get_resources(
                &subject_id,
                usize::from(season.unwrap_or_default()),
                usize::from(episode.unwrap_or_default()),
                1,
                None,
            )
            .await?;
        let payload = ensure_object_with_subject_id(payload, &subject_id);
        let item =
            find_source_item(&payload, &resource_id).ok_or(CatalogError::NotFound("source"))?;
        validate_source_item_resolution(item, height, self.quality_policy)?;
        let play_info = self
            .client
            .get_play_info(
                &subject_id,
                usize::from(season.unwrap_or_default()),
                usize::from(episode.unwrap_or_default()),
            )
            .await?;
        let mut resolved = resolve_play_info_source(
            &play_info,
            &resource_id,
            season,
            episode,
            height,
            self.client.user_agent(),
        )?;

        let resolved_subtitle = if let Some(subtitle) = subtitle {
            let (subtitle_provider, subtitle_subject_id, subtitle_resource_id, index, _, _) =
                decode_subtitle(&self.codec, subtitle)?;
            self.ensure_moviebox_provider(Cow::Owned(subtitle_provider))?;
            if subtitle_subject_id != subject_id || subtitle_resource_id != resource_id {
                return Err(CatalogError::InvalidOpaqueId);
            }
            let captions = self
                .client
                .get_ext_captions(&subject_id, &resource_id)
                .await?;
            let captions = ensure_object_ids(captions, &subject_id, &resource_id);
            resolve_subtitle_from_payload(&captions, index)?
        } else {
            None
        };

        resolved.subtitle = resolved_subtitle;
        Ok(resolved)
    }
}
