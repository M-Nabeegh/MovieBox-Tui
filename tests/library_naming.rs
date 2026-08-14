#![cfg(feature = "server")]

use std::path::Path;
use std::path::PathBuf;

use moviebox_tui::{
    catalog::{CatalogDetails, CatalogId, EpisodeInfo, MediaType, SeasonInfo},
    server::library::{LibraryError, LibraryNamer, MediaIdentity},
};
use tempfile::tempdir;
use uuid::Uuid;

fn movie(title: &str, year: &str) -> CatalogDetails {
    CatalogDetails {
        id: CatalogId::new(format!("movie:{title}")),
        title: title.to_string(),
        media_type: MediaType::Movie,
        year: Some(year.to_string()),
        description: None,
        tagline: None,
        imdb_rating: None,
        duration: None,
        genres: Vec::new(),
        country: None,
        seasons: Vec::new(),
        audio_options: Vec::new(),
    }
}

fn episode(series: &str, year: &str, season: u16, episode: u16, title: &str) -> CatalogDetails {
    CatalogDetails {
        id: CatalogId::new(format!("series:{series}")),
        title: series.to_string(),
        media_type: MediaType::Series,
        year: Some(year.to_string()),
        description: None,
        tagline: None,
        imdb_rating: None,
        duration: None,
        genres: Vec::new(),
        country: None,
        seasons: vec![SeasonInfo {
            number: season,
            episodes: vec![EpisodeInfo {
                number: episode,
                title: Some(title.to_string()),
            }],
        }],
        audio_options: Vec::new(),
    }
}

fn namer() -> (tempfile::TempDir, LibraryNamer) {
    let temp = tempdir().unwrap();
    let root = temp.path().join("media");
    std::fs::create_dir_all(&root).unwrap();
    let namer = LibraryNamer::new(root).unwrap();
    (temp, namer)
}

#[test]
fn movie_paths_follow_jellyfin_naming() {
    let (_temp, namer) = namer();
    let identity =
        MediaIdentity::from_details(&movie("Example Movie", "2024"), None, None).unwrap();

    let paths = namer
        .paths_for(&identity, "mkv", None, Uuid::nil())
        .unwrap();

    assert_eq!(
        paths.video_relative,
        PathBuf::from("Movies/Example Movie (2024)/Example Movie (2024).mkv")
    );
    assert_eq!(
        paths.partial_video_relative,
        PathBuf::from(
            "_moviebox/jobs/00000000-0000-0000-0000-000000000000/Example Movie (2024).mkv.part"
        )
    );
    assert_eq!(paths.subtitle_relative, None);
    assert_eq!(paths.partial_subtitle_relative, None);
}

#[test]
fn episode_paths_follow_jellyfin_naming() {
    let (_temp, namer) = namer();
    let identity = MediaIdentity::from_details(
        &episode("Example Show", "2024", 1, 2, "Second Episode"),
        Some(1),
        Some(2),
    )
    .unwrap();

    let paths = namer
        .paths_for(&identity, "mkv", Some(("en", "srt")), Uuid::nil())
        .unwrap();

    assert_eq!(
        paths.video_relative,
        PathBuf::from(
            "Shows/Example Show (2024)/Season 01/Example Show (2024) S01E02 Second Episode.mkv"
        )
    );
    assert_eq!(
        paths.subtitle_relative,
        Some(PathBuf::from(
            "Shows/Example Show (2024)/Season 01/Example Show (2024) S01E02 Second Episode.en.srt"
        ))
    );
}

#[test]
fn subtitle_sidecars_keep_the_same_stem_and_sanitize_language() {
    let (_temp, namer) = namer();
    let identity =
        MediaIdentity::from_details(&movie("Sidecar Story", "2021"), None, None).unwrap();

    let paths = namer
        .paths_for(
            &identity,
            "mp4",
            Some(("English (SDH)", "vtt")),
            Uuid::nil(),
        )
        .unwrap();

    assert_eq!(
        paths.subtitle_relative,
        Some(PathBuf::from(
            "Movies/Sidecar Story (2021)/Sidecar Story (2021).English (SDH).vtt"
        ))
    );
}

#[test]
fn titles_are_sanitized_without_flattening_unicode_text() {
    let (_temp, namer) = namer();
    let identity = MediaIdentity::from_details(
        &episode(
            "Pokémon ../CON:\u{0000} 名探偵",
            "2024",
            1,
            9,
            "Second/\u{0008}Episode..",
        ),
        Some(1),
        Some(9),
    )
    .unwrap();

    let paths = namer
        .paths_for(
            &identity,
            "mkv",
            Some(("English/\u{0000}", "srt")),
            Uuid::nil(),
        )
        .unwrap();
    let rendered = paths.video_relative.to_string_lossy();

    assert!(rendered.contains("Pokémon"));
    assert!(rendered.contains("名探偵"));
    assert!(rendered.contains("S01E09"));
    assert!(!rendered.contains(".."));
    assert!(!rendered.contains('\0'));
    assert!(!rendered.contains("CON:"));
    assert_eq!(
        paths.subtitle_relative,
        Some(PathBuf::from(
            "Shows/Pokémon CON 名探偵 (2024)/Season 01/Pokémon CON 名探偵 (2024) S01E09 Second Episode.English.srt"
        ))
    );
}

#[test]
fn long_titles_are_truncated_deterministically() {
    let (_temp, namer) = namer();
    let title = "L".repeat(180);
    let identity = MediaIdentity::from_details(&movie(&title, "2024"), None, None).unwrap();

    let paths = namer
        .paths_for(&identity, "mkv", None, Uuid::nil())
        .unwrap();
    let folder = paths.video_relative.parent().unwrap().file_name().unwrap();
    let filename = paths.video_relative.file_stem().unwrap();

    assert!(folder.to_string_lossy().ends_with(" (2024)"));
    assert!(filename.to_string_lossy().ends_with(" (2024)"));
    assert!(folder.to_string_lossy().chars().count() <= 120);
    assert!(filename.to_string_lossy().chars().count() <= 120);
}

#[test]
fn unsupported_video_extensions_are_rejected() {
    let (_temp, namer) = namer();
    let identity =
        MediaIdentity::from_details(&movie("Bad Extension", "2024"), None, None).unwrap();

    let error = namer
        .paths_for(&identity, "exe", None, Uuid::nil())
        .unwrap_err();

    assert!(matches!(
        error,
        LibraryError::UnsupportedVideoExtension(extension) if extension == "exe"
    ));
}

#[test]
fn unsupported_subtitle_extensions_are_rejected() {
    let (_temp, namer) = namer();
    let identity = MediaIdentity::from_details(&movie("Bad Subtitle", "2024"), None, None).unwrap();

    let error = namer
        .paths_for(&identity, "mkv", Some(("en", "txt")), Uuid::nil())
        .unwrap_err();

    assert!(matches!(
        error,
        LibraryError::UnsupportedSubtitleExtension(extension) if extension == "txt"
    ));
}

#[test]
fn existing_completed_targets_return_a_collision_error() {
    let (_temp, namer) = namer();
    let identity =
        MediaIdentity::from_details(&movie("Collision Course", "2024"), None, None).unwrap();
    let existing = namer
        .media_root()
        .join("Movies/Collision Course (2024)/Collision Course (2024).mkv");
    std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
    std::fs::write(&existing, b"fixture").unwrap();

    let error = namer
        .paths_for(&identity, "mkv", None, Uuid::nil())
        .unwrap_err();

    assert!(matches!(
        error,
        LibraryError::AlreadyExists { relative_path }
            if relative_path == Path::new("Movies/Collision Course (2024)/Collision Course (2024).mkv")
    ));
}

#[test]
fn partial_paths_stay_outside_the_final_library_folders() {
    let (_temp, namer) = namer();
    let job_id = Uuid::parse_str("12345678-1234-5678-1234-567812345678").unwrap();
    let identity = MediaIdentity::from_details(
        &episode("Partial Show", "2024", 3, 4, "Arrival"),
        Some(3),
        Some(4),
    )
    .unwrap();

    let paths = namer
        .paths_for(&identity, "webm", Some(("en", "ass")), job_id)
        .unwrap();
    let partial = paths.partial_video_relative.to_string_lossy();

    assert!(partial.starts_with("_moviebox/jobs/12345678-1234-5678-1234-567812345678/"));
    assert!(!partial.starts_with("Movies/"));
    assert!(!partial.starts_with("Shows/"));
    assert_eq!(
        paths.partial_subtitle_relative,
        Some(PathBuf::from(
            "_moviebox/jobs/12345678-1234-5678-1234-567812345678/Partial Show (2024) S03E04 Arrival.en.ass.part"
        ))
    );
}
