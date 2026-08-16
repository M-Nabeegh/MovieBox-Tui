//! Automatic subtitle-track selection.
//!
//! Choosing a subtitle by hand for every download is tedious, and forgetting to
//! means the file lands in the library with no subtitle at all. When the request
//! does not name a track, the server picks the best match for a preferred
//! language on the user's behalf.

use crate::catalog::SubtitleTrack;

/// Language preference used when a download request does not name a subtitle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubtitlePreference {
    /// Attach nothing unless the request names a track.
    Disabled,
    /// Attach the best track matching this language name or code.
    Language(String),
}

impl SubtitlePreference {
    /// Parse a configured preference. `off`, `none` and an empty value disable
    /// automatic selection; anything else is treated as a language name or code.
    pub fn parse(value: &str) -> Self {
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("off")
            || trimmed.eq_ignore_ascii_case("none")
        {
            Self::Disabled
        } else {
            Self::Language(trimmed.to_ascii_lowercase())
        }
    }

    /// Pick the track to attach, or `None` when nothing suitable is offered.
    pub fn select<'a>(&self, tracks: &'a [SubtitleTrack]) -> Option<&'a SubtitleTrack> {
        let Self::Language(wanted) = self else {
            return None;
        };
        tracks
            .iter()
            .filter_map(|track| score(track, wanted).map(|score| (score, track)))
            .min_by_key(|(score, _)| *score)
            .map(|(_, track)| track)
    }
}

impl Default for SubtitlePreference {
    fn default() -> Self {
        Self::Language("english".to_string())
    }
}

/// Rank a candidate track; lower is better, `None` means it does not match.
///
/// Providers label the same language many ways (`English`, `en`, `eng`,
/// `English (SDH)`), so matching is by prefix once aliases are folded together.
/// Plain tracks are preferred over hearing-impaired and forced variants, which
/// carry only partial dialogue and surprise a viewer who did not ask for them.
fn score(track: &SubtitleTrack, wanted: &str) -> Option<u8> {
    let language = track.language.trim().to_ascii_lowercase();
    let canonical = canonical_language(&language);
    let wanted = canonical_language(wanted);
    if canonical != wanted {
        return None;
    }

    let annotated = language.contains("sdh")
        || language.contains("hearing")
        || language.contains("forced")
        || language.contains("cc");
    let exact = language == wanted;

    Some(match (exact, annotated) {
        (true, false) => 0,
        (false, false) => 1,
        (true, true) => 2,
        (false, true) => 3,
    })
}

/// Reduce a language label to a comparable base name.
fn canonical_language(value: &str) -> String {
    let base = value
        .split(['(', '[', '-', ','])
        .next()
        .unwrap_or(value)
        .trim();
    match base {
        "en" | "eng" | "english" => "english".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::SubtitleId;

    fn track(language: &str) -> SubtitleTrack {
        SubtitleTrack {
            id: SubtitleId::new(format!("id-{language}")),
            language: language.to_string(),
            format: Some("srt".to_string()),
        }
    }

    #[test]
    fn english_is_selected_by_default() {
        let tracks = [track("Spanish"), track("English"), track("Arabic")];
        let selected = SubtitlePreference::default().select(&tracks).unwrap();
        assert_eq!(selected.language, "English");
    }

    #[test]
    fn language_codes_count_as_english() {
        for label in ["en", "eng", "EN", "English"] {
            let tracks = [track("French"), track(label)];
            assert_eq!(
                SubtitlePreference::default()
                    .select(&tracks)
                    .unwrap()
                    .language,
                label
            );
        }
    }

    #[test]
    fn plain_english_beats_annotated_variants() {
        let tracks = [
            track("English (SDH)"),
            track("English Forced"),
            track("English"),
        ];
        assert_eq!(
            SubtitlePreference::default()
                .select(&tracks)
                .unwrap()
                .language,
            "English"
        );
    }

    #[test]
    fn annotated_english_is_better_than_nothing() {
        let tracks = [track("German"), track("English (SDH)")];
        assert_eq!(
            SubtitlePreference::default()
                .select(&tracks)
                .unwrap()
                .language,
            "English (SDH)"
        );
    }

    #[test]
    fn no_match_selects_nothing() {
        let tracks = [track("German"), track("Spanish")];
        assert!(SubtitlePreference::default().select(&tracks).is_none());
    }

    #[test]
    fn empty_track_list_selects_nothing() {
        assert!(SubtitlePreference::default().select(&[]).is_none());
    }

    #[test]
    fn disabled_preference_never_selects() {
        let tracks = [track("English")];
        for value in ["off", "none", "", "  "] {
            assert_eq!(
                SubtitlePreference::parse(value),
                SubtitlePreference::Disabled
            );
            assert!(SubtitlePreference::parse(value).select(&tracks).is_none());
        }
    }

    #[test]
    fn other_languages_can_be_preferred() {
        let tracks = [track("English"), track("Urdu")];
        let selected = SubtitlePreference::parse("Urdu").select(&tracks).unwrap();
        assert_eq!(selected.language, "Urdu");
    }
}
