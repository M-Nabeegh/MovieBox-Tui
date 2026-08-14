use std::path::{Path, PathBuf};

use thiserror::Error;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;
use uuid::Uuid;

use crate::{
    catalog::{CatalogDetails, MediaType},
    server::security::path::{PathSecurityError, contained_path},
};

const MAX_COMPONENT_GRAPHEMES: usize = 120;
const MAX_COMPONENT_BYTES: usize = 255;
const MAX_LANGUAGE_GRAPHEMES: usize = 48;
const MAX_LANGUAGE_BYTES: usize = 128;
const MAX_SUPPORTED_SUBTITLE_EXTENSION_BYTES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaIdentity {
    Movie {
        title: String,
        year: Option<String>,
    },
    Episode {
        series_title: String,
        year: Option<String>,
        season: u16,
        episode: u16,
        episode_title: Option<String>,
    },
}

impl MediaIdentity {
    pub fn from_details(
        details: &CatalogDetails,
        season: Option<u16>,
        episode: Option<u16>,
    ) -> Result<Self, LibraryError> {
        match details.media_type {
            MediaType::Movie => {
                if season.is_some() || episode.is_some() {
                    return Err(LibraryError::UnexpectedEpisodeSelection);
                }
                Ok(Self::Movie {
                    title: details.title.clone(),
                    year: details.year.clone(),
                })
            }
            MediaType::Series => {
                let season = season.ok_or(LibraryError::MissingEpisodeSelection)?;
                let episode = episode.ok_or(LibraryError::MissingEpisodeSelection)?;
                let episode_info = details
                    .seasons
                    .iter()
                    .find(|candidate| candidate.number == season)
                    .and_then(|candidate| {
                        candidate
                            .episodes
                            .iter()
                            .find(|item| item.number == episode)
                    })
                    .ok_or(LibraryError::EpisodeNotFound { season, episode })?;
                Ok(Self::Episode {
                    series_title: details.title.clone(),
                    year: details.year.clone(),
                    season,
                    episode,
                    episode_title: episode_info.title.clone(),
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPaths {
    pub video_relative: PathBuf,
    pub subtitle_relative: Option<PathBuf>,
    pub partial_video_relative: PathBuf,
    pub partial_subtitle_relative: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("series items require both season and episode numbers")]
    MissingEpisodeSelection,
    #[error("movie items do not accept season or episode numbers")]
    UnexpectedEpisodeSelection,
    #[error("episode S{season:02}E{episode:02} was not found in catalog details")]
    EpisodeNotFound { season: u16, episode: u16 },
    #[error("unsupported video extension: {0}")]
    UnsupportedVideoExtension(String),
    #[error("unsupported subtitle extension: {0}")]
    UnsupportedSubtitleExtension(String),
    #[error("completed media target already exists: {relative_path}")]
    AlreadyExists { relative_path: PathBuf },
    #[error("path component exceeds {MAX_COMPONENT_BYTES} bytes: {relative_path}")]
    ComponentTooLong { relative_path: PathBuf },
    #[error("path validation failed: {0}")]
    Path(#[from] PathSecurityError),
}

#[derive(Debug, Clone)]
pub struct LibraryNamer {
    media_root: PathBuf,
}

impl LibraryNamer {
    pub fn new(media_root: impl Into<PathBuf>) -> Result<Self, LibraryError> {
        let media_root = media_root
            .into()
            .canonicalize()
            .map_err(PathSecurityError::from)?;
        Ok(Self { media_root })
    }

    pub fn media_root(&self) -> &Path {
        &self.media_root
    }

    pub fn paths_for(
        &self,
        identity: &MediaIdentity,
        video_extension: &str,
        subtitle: Option<(&str, &str)>,
        job_id: Uuid,
    ) -> Result<MediaPaths, LibraryError> {
        let video_extension = normalize_video_extension(video_extension)?;
        let subtitle = subtitle
            .map(|(language, extension)| {
                Ok::<_, LibraryError>((
                    sanitize_language(language),
                    normalize_subtitle_extension(extension)?,
                ))
            })
            .transpose()?;
        let max_stem_bytes = filename_stem_byte_limit(&video_extension);

        let (video_relative, subtitle_relative, stem) = match identity {
            MediaIdentity::Movie { title, year } => {
                let display = display_title(title, year.as_deref(), max_stem_bytes);
                let folder = PathBuf::from("Movies").join(&display);
                let stem = display;
                let video = folder.join(format!("{stem}.{video_extension}"));
                let subtitle = subtitle.as_ref().map(|(language, extension)| {
                    folder.join(format!("{stem}.{language}.{extension}"))
                });
                (video, subtitle, stem)
            }
            MediaIdentity::Episode {
                series_title,
                year,
                season,
                episode,
                episode_title,
            } => {
                let show = display_title(series_title, year.as_deref(), MAX_COMPONENT_BYTES);
                let folder = PathBuf::from("Shows")
                    .join(&show)
                    .join(format!("Season {season:02}"));
                let stem = episode_stem(
                    &show,
                    *season,
                    *episode,
                    episode_title.as_deref(),
                    max_stem_bytes,
                );
                let video = folder.join(format!("{stem}.{video_extension}"));
                let subtitle = subtitle.as_ref().map(|(language, extension)| {
                    folder.join(format!("{stem}.{language}.{extension}"))
                });
                (video, subtitle, stem)
            }
        };

        let partial_dir = PathBuf::from("_moviebox")
            .join("jobs")
            .join(job_id.to_string());
        let partial_video_relative = partial_dir.join(format!("{stem}.{video_extension}.part"));
        let partial_subtitle_relative = subtitle.as_ref().map(|(language, extension)| {
            partial_dir.join(format!("{stem}.{language}.{extension}.part"))
        });

        let video_absolute = self.validate_relative_path(&video_relative)?;
        if video_absolute.exists() {
            return Err(LibraryError::AlreadyExists {
                relative_path: video_relative,
            });
        }

        let subtitle_absolute = subtitle_relative
            .as_ref()
            .map(|path| self.validate_relative_path(path))
            .transpose()?;
        if let Some(existing_relative) = subtitle_relative
            .as_ref()
            .zip(subtitle_absolute.as_ref())
            .and_then(|(relative, absolute)| absolute.exists().then(|| relative.clone()))
        {
            return Err(LibraryError::AlreadyExists {
                relative_path: existing_relative,
            });
        }

        self.validate_relative_path(&partial_video_relative)?;
        if let Some(path) = &partial_subtitle_relative {
            self.validate_relative_path(path)?;
        }

        Ok(MediaPaths {
            video_relative,
            subtitle_relative,
            partial_video_relative,
            partial_subtitle_relative,
        })
    }

    fn validate_relative_path(&self, relative: &Path) -> Result<PathBuf, LibraryError> {
        if relative.components().any(|component| {
            matches!(component, std::path::Component::Normal(value)
                if value.to_string_lossy().len() > MAX_COMPONENT_BYTES)
        }) {
            return Err(LibraryError::ComponentTooLong {
                relative_path: relative.to_path_buf(),
            });
        }
        contained_path(&self.media_root, relative).map_err(Into::into)
    }
}

fn normalize_video_extension(value: &str) -> Result<String, LibraryError> {
    let normalized = normalize_extension(value);
    let supported = matches!(
        normalized.as_str(),
        "mp4" | "mkv" | "webm" | "avi" | "mov" | "m4v"
    );
    if supported {
        Ok(normalized)
    } else {
        Err(LibraryError::UnsupportedVideoExtension(normalized))
    }
}

fn normalize_subtitle_extension(value: &str) -> Result<String, LibraryError> {
    let normalized = normalize_extension(value);
    let supported = matches!(normalized.as_str(), "srt" | "vtt" | "ass" | "ssa" | "sub");
    if supported {
        Ok(normalized)
    } else {
        Err(LibraryError::UnsupportedSubtitleExtension(normalized))
    }
}

fn normalize_extension(value: &str) -> String {
    value
        .nfc()
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
}

fn display_title(title: &str, year: Option<&str>, max_bytes: usize) -> String {
    let title = sanitize_component(title);
    let year = sanitize_year(year);
    match year {
        Some(year) => truncate_with_suffix(
            &title,
            &format!(" ({year})"),
            MAX_COMPONENT_GRAPHEMES,
            max_bytes,
        ),
        None => truncate_component(&title, MAX_COMPONENT_GRAPHEMES, max_bytes),
    }
}

fn episode_stem(
    series: &str,
    season: u16,
    episode: u16,
    episode_title: Option<&str>,
    max_bytes: usize,
) -> String {
    let code = format!(" S{season:02}E{episode:02}");
    let reserved = count_graphemes(&code);
    let max_series = MAX_COMPONENT_GRAPHEMES.saturating_sub(reserved);
    let series = truncate_component(series, max_series, max_bytes.saturating_sub(code.len()));
    let prefix = format!("{series}{code}");

    match episode_title
        .map(sanitize_component)
        .filter(|value| !value.is_empty())
    {
        Some(title) => {
            let spacer = 1;
            let available = MAX_COMPONENT_GRAPHEMES
                .saturating_sub(count_graphemes(&prefix))
                .saturating_sub(spacer);
            let available_bytes = max_bytes
                .saturating_sub(prefix.len())
                .saturating_sub(spacer);
            if available == 0 || available_bytes == 0 {
                prefix
            } else {
                let title = truncate_component(&title, available, available_bytes);
                if title.is_empty() {
                    prefix
                } else {
                    format!("{prefix} {title}")
                }
            }
        }
        None => prefix,
    }
}

fn sanitize_year(year: Option<&str>) -> Option<String> {
    let mut digits = String::new();
    for character in year.unwrap_or_default().chars() {
        if character.is_ascii_digit() {
            digits.push(character);
            if digits.len() == 4 {
                return Some(digits);
            }
        } else {
            digits.clear();
        }
    }
    None
}

fn sanitize_language(language: &str) -> String {
    let language = sanitize_component(language);
    truncate_component(&language, MAX_LANGUAGE_GRAPHEMES, MAX_LANGUAGE_BYTES)
}

fn sanitize_component(value: &str) -> String {
    let value = value.nfc().collect::<String>();
    let mut collapsed = String::new();
    let mut pending_space = false;

    for character in value.chars() {
        let replacement = match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => Some(' '),
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' => None,
            _ if character.is_control() => None,
            _ if character.is_whitespace() => Some(' '),
            _ => Some(character),
        };

        match replacement {
            Some(' ') => pending_space = !collapsed.is_empty(),
            Some(character) => {
                if pending_space {
                    collapsed.push(' ');
                    pending_space = false;
                }
                collapsed.push(character);
            }
            None => {}
        }
    }

    let trimmed = collapsed.trim_matches(|character| matches!(character, '.' | ' '));
    let collapsed_dots = collapse_double_dots(trimmed);
    let normalized_words = collapsed_dots
        .split_whitespace()
        .filter(|segment| !segment.chars().all(|character| character == '.'))
        .collect::<Vec<_>>()
        .join(" ");
    let component = if normalized_words.is_empty() {
        "Untitled".to_string()
    } else {
        truncate_component(
            &normalized_words,
            MAX_COMPONENT_GRAPHEMES,
            MAX_COMPONENT_BYTES,
        )
    };

    let component = ensure_non_reserved(component);
    let component = truncate_component(&component, MAX_COMPONENT_GRAPHEMES, MAX_COMPONENT_BYTES);
    ensure_non_reserved(component)
}

fn collapse_double_dots(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut previous_dot = false;

    for character in value.chars() {
        if character == '.' {
            if !previous_dot {
                normalized.push(character);
            }
            previous_dot = true;
        } else {
            normalized.push(character);
            previous_dot = false;
        }
    }

    normalized.trim_matches('.').to_string()
}

fn ensure_non_reserved(mut component: String) -> String {
    component = component.nfc().collect::<String>();
    if component.is_empty() {
        component.push_str("Untitled");
    }

    let basename_end = component.find('.').unwrap_or(component.len());
    let basename = &component[..basename_end];
    let upper = basename.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "¹" | "²" | "³")
                    || (number.len() == 1 && number.bytes().all(|byte| matches!(byte, b'1'..=b'9')))
            });
    if reserved {
        component.insert(basename_end, '_');
    }
    component
}

fn truncate_with_suffix(
    value: &str,
    suffix: &str,
    max_graphemes: usize,
    max_bytes: usize,
) -> String {
    let value = value.nfc().collect::<String>();
    let suffix = suffix.nfc().collect::<String>();
    let suffix_len = count_graphemes(&suffix);
    if count_graphemes(&value) + suffix_len <= max_graphemes
        && value.len() + suffix.len() <= max_bytes
    {
        return format!("{value}{suffix}");
    }

    let available_graphemes = max_graphemes.saturating_sub(suffix_len);
    let available_bytes = max_bytes.saturating_sub(suffix.len());
    let prefix = truncate_component(&value, available_graphemes, available_bytes);
    if prefix.is_empty() {
        truncate_component(&suffix, max_graphemes, max_bytes)
    } else {
        format!("{prefix}{suffix}")
    }
}

fn truncate_component(value: &str, max_graphemes: usize, max_bytes: usize) -> String {
    let value = value.nfc().collect::<String>();
    let mut truncated = String::new();

    for grapheme in UnicodeSegmentation::graphemes(value.as_str(), true).take(max_graphemes) {
        let available_bytes = max_bytes.saturating_sub(truncated.len());
        if available_bytes == 0 {
            break;
        }
        if grapheme.len() <= available_bytes {
            truncated.push_str(grapheme);
        } else if truncated.is_empty() {
            truncated.push_str(truncate_to_utf8_bytes(grapheme, available_bytes));
            break;
        } else {
            break;
        }
    }

    truncated
        .trim_matches(|character| matches!(character, '.' | ' '))
        .to_string()
}

fn truncate_to_utf8_bytes(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }

    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn filename_stem_byte_limit(video_extension: &str) -> usize {
    let video_partial_suffix_len = 1 + video_extension.len() + ".part".len();
    let worst_case_subtitle_partial_suffix_len =
        1 + MAX_LANGUAGE_BYTES + 1 + MAX_SUPPORTED_SUBTITLE_EXTENSION_BYTES + ".part".len();
    MAX_COMPONENT_BYTES
        .saturating_sub(video_partial_suffix_len.max(worst_case_subtitle_partial_suffix_len))
}

fn count_graphemes(value: &str) -> usize {
    UnicodeSegmentation::graphemes(value, true).count()
}
