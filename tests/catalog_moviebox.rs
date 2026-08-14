#![cfg(feature = "server")]

#[path = "support/fixtures.rs"]
mod fixtures;

use fixtures::fixture;
use moviebox_tui::catalog::{
    CatalogError, MediaType, OpaqueIdCodec, OpaquePayload, QualityPolicy,
    moviebox::{adapt_details, adapt_search_page, adapt_sources, adapt_subtitles},
};

fn fixture_codec() -> OpaqueIdCodec {
    OpaqueIdCodec::new([7_u8; 32])
}

#[test]
fn search_results_are_typed_and_hide_provider_urls() {
    let payload = fixture("moviebox-search.json");

    let page = adapt_search_page(&payload, &fixture_codec()).unwrap();

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
