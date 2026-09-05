#![cfg(feature = "server")]

#[path = "support/fixtures.rs"]
mod fixtures;

use fixtures::fixture;
use moviebox_tui::catalog::{
    CatalogError, MediaType, OpaqueIdCodec, OpaquePayload, QualityPolicy, SourceTransport,
    moviebox::{
        adapt_details, adapt_search_page, adapt_sources, adapt_subtitles, resolve_play_info_source,
        validate_source_item_resolution,
    },
};

fn fixture_codec() -> OpaqueIdCodec {
    OpaqueIdCodec::new([7_u8; 32])
}

#[test]
fn search_results_are_typed_and_hide_provider_urls() {
    let payload = fixture("moviebox-search.json");

    let page = adapt_search_page(&payload, &fixture_codec(), 1).unwrap();

    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].title, "Fixture Movie");
    assert_eq!(page.items[0].media_type, MediaType::Movie);
    assert_eq!(page.items[1].title, "Fixture Series");
    assert_eq!(page.items[1].media_type, MediaType::Series);
    let serialized = serde_json::to_string(&page).unwrap();
    assert!(!serialized.contains("https://"));
    assert!(!serialized.contains("poster"));
    assert!(!serialized.contains("subject-fixture"));
}

#[test]
fn search_page_preserves_requested_later_page_without_inventing_more_results() {
    let payload = fixture("moviebox-search.json");

    let page = adapt_search_page(&payload, &fixture_codec(), 3).unwrap();

    assert_eq!(page.page, 3);
    assert!(!page.has_more);
}

#[test]
fn search_page_uses_explicit_provider_pager_flag_when_available() {
    let mut payload = fixture("moviebox-search.json");
    payload["pager"] = serde_json::json!({ "hasMore": true });

    let page = adapt_search_page(&payload, &fixture_codec(), 2).unwrap();

    assert_eq!(page.page, 2);
    assert!(page.has_more);
}

#[test]
fn details_are_typed_and_hide_provider_urls() {
    let payload = fixture("moviebox-details.json");

    let details = adapt_details(&payload, &fixture_codec()).unwrap();

    assert_eq!(details.title, "Fixture Series");
    assert_eq!(details.media_type, MediaType::Series);
    assert_eq!(details.seasons.len(), 2);
    assert_eq!(details.seasons[0].episodes.len(), 3);
    assert_eq!(details.audio_options.len(), 2);
    assert_eq!(details.audio_options[1].label, "Hindi Dub");
    let serialized = serde_json::to_string(&details).unwrap();
    assert!(!serialized.contains("https://"));
    assert!(!serialized.contains("subject-fixture"));
}

#[test]
fn source_options_are_capped_and_hide_direct_urls() {
    let payload = fixture("moviebox-resources.json");
    let options = adapt_sources(&payload, &fixture_codec(), QualityPolicy::new(1080)).unwrap();

    assert_eq!(
        options.iter().map(|x| x.height).collect::<Vec<_>>(),
        vec![1080, 720]
    );
    assert!(options[0].recommended);
    assert!(!options[1].recommended);
    let serialized = serde_json::to_string(&options).unwrap();
    assert!(!serialized.contains("https://"));
    assert!(!serialized.contains("resourceLink"));
    assert!(!serialized.contains("resource-fixture"));
}

#[test]
fn subtitles_are_typed_and_hide_direct_urls() {
    let resources = fixture("moviebox-resources.json");
    let source = adapt_sources(&resources, &fixture_codec(), QualityPolicy::new(1080))
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let captions = fixture("moviebox-captions.json");

    let subtitles = adapt_subtitles(&captions, &source.id, &fixture_codec()).unwrap();

    assert_eq!(subtitles.len(), 2);
    assert_eq!(subtitles[0].language, "English");
    assert_eq!(subtitles[1].format.as_deref(), Some("vtt"));
    let serialized = serde_json::to_string(&subtitles).unwrap();
    assert!(!serialized.contains("https://"));
    assert!(!serialized.contains("resource-fixture"));
}

#[test]
fn quality_policy_reports_when_no_source_is_within_limit() {
    let payload = serde_json::json!({
        "subjectId": "subject-fixture-2",
        "list": [
            {
                "resourceId": "resource-fixture-2160",
                "resourceLink": "https://stream.fixtures.invalid/video-2160.mkv",
                "resolution": 2160
            }
        ]
    });

    let error = adapt_sources(&payload, &fixture_codec(), QualityPolicy::new(1080)).unwrap_err();

    assert!(matches!(error, CatalogError::QualityUnavailable { .. }));
}

#[test]
fn malformed_source_resource_id_is_invalid_payload() {
    let payload = serde_json::json!({
        "subjectId": "subject-fixture-2",
        "list": [{ "resolution": 1080 }]
    });

    let error = adapt_sources(&payload, &fixture_codec(), QualityPolicy::new(1080)).unwrap_err();

    assert_eq!(error, CatalogError::InvalidPayload("resourceId"));
}

#[test]
fn malformed_source_resolution_is_invalid_payload() {
    let payload = serde_json::json!({
        "subjectId": "subject-fixture-2",
        "list": [{ "resourceId": "resource-fixture-invalid", "resolution": "unknown" }]
    });

    let error = adapt_sources(&payload, &fixture_codec(), QualityPolicy::new(1080)).unwrap_err();

    assert_eq!(error, CatalogError::InvalidPayload("resolution"));
}

#[test]
fn resolved_source_resolution_must_match_signed_source_height() {
    let item = serde_json::json!({ "resolution": 720 });

    let error = validate_source_item_resolution(&item, 1080, QualityPolicy::new(1080)).unwrap_err();

    assert_eq!(
        error,
        CatalogError::SourceResolutionMismatch {
            signed_height: 1080,
            actual_height: 720,
        }
    );
}

#[test]
fn resolved_source_resolution_is_checked_against_quality_policy() {
    let item = serde_json::json!({ "resolution": 2160 });

    let error = validate_source_item_resolution(&item, 1080, QualityPolicy::new(1080)).unwrap_err();

    assert_eq!(
        error,
        CatalogError::QualityUnavailable {
            maximum_height: 1080,
            requested_height: Some(2160),
        }
    );
}

#[test]
fn opaque_ids_round_trip_without_exposing_fixture_identifiers() {
    let codec = fixture_codec();
    let payload = OpaquePayload::Source {
        provider: "moviebox".to_string(),
        subject_id: "subject-fixture-2".to_string(),
        resource_id: "resource-fixture-1080".to_string(),
        season: Some(1),
        episode: Some(2),
        height: 1080,
        language: Some("Original".to_string()),
    };

    let encoded = codec.encode(&payload);
    let decoded = codec.decode(&encoded).unwrap();

    assert_eq!(decoded, payload);
    assert!(!encoded.contains("https://"));
    assert!(!encoded.contains("subject-fixture-2"));
    assert!(!encoded.contains("resource-fixture-1080"));
}

#[test]
fn play_info_selects_signed_mpd_and_keeps_resource_episode_identity() {
    let payload = fixture("moviebox-play-info.json");

    let resolved = resolve_play_info_source(
        &payload,
        "resource-fixture-1080",
        Some(1),
        Some(2),
        1080,
        "FixtureAndroid/1.0",
    )
    .unwrap();

    assert_eq!(
        resolved.url.as_str(),
        "https://cdn.example.invalid/dash/resource-fixture-1080/index.mpd"
    );
    assert!(!resolved.url.as_str().contains("upgrade"));
    assert!(
        resolved
            .headers
            .get("cookie")
            .is_some_and(|value| value.to_str().unwrap().contains("CloudFront-Policy="))
    );
    assert_eq!(
        resolved.headers.get("user-agent").unwrap(),
        "FixtureAndroid/1.0"
    );
    assert_eq!(
        resolved.transport,
        SourceTransport::Dash {
            maximum_height: 1080,
            expected_duration_seconds: Some(5400.0),
        }
    );
}

#[test]
fn play_info_rejects_invalid_policy_without_falling_back_to_notice_url() {
    let payload = fixture("moviebox-play-info.json");

    let error = resolve_play_info_source(
        &payload,
        "resource-fixture-invalid",
        None,
        None,
        1080,
        "FixtureAndroid/1.0",
    )
    .unwrap_err();

    assert!(matches!(error, CatalogError::InvalidPayload(_)));
    let rendered = error.to_string();
    assert!(!rendered.contains("upgrade.example.invalid"));
}

#[test]
fn play_info_fails_closed_when_resource_episode_identity_does_not_match() {
    let mut payload = fixture("moviebox-play-info.json");
    payload["data"]["streams"][1]["se"] = serde_json::json!(9);

    let error = resolve_play_info_source(
        &payload,
        "resource-fixture-1080",
        Some(1),
        Some(2),
        1080,
        "FixtureAndroid/1.0",
    )
    .unwrap_err();

    assert!(matches!(error, CatalogError::NotFound("source")));
}
