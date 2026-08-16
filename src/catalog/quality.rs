//! Ranking the sources offered for one title.
//!
//! Resolution alone is a poor guide to how a file actually looks. A 1080p
//! encode squeezed into 800 MB looks worse than a 720p one at 1.5 GB, and a
//! smaller HEVC file routinely beats a larger H.264 file at the same
//! resolution. Picking by height alone therefore recommends the wrong source
//! often enough to matter.
//!
//! Every source for a title is the same film, so its running time is the same
//! for all of them. That makes file size a direct proxy for bitrate without
//! needing the duration: comparing sizes *is* comparing bitrates, once each is
//! adjusted for how efficient its codec is.

use serde::{Deserialize, Serialize};

use super::SourceOption;

/// How this source compares with the others offered for the same title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityTier {
    /// The best available balance of resolution and bitrate.
    Best,
    /// Perfectly watchable, but bettered by another source.
    Good,
    /// Noticeably compressed for its resolution.
    Lower,
}

/// Relative quality weight of a codec at equal file size.
///
/// HEVC and AV1 retain more detail per byte than H.264, so an equal-size file
/// in those codecs carries more picture. The multipliers are deliberately
/// conservative: the aim is to stop a bulky H.264 file beating a leaner, better
/// HEVC one, not to model encoders precisely.
fn codec_efficiency(codec: Option<&str>) -> f64 {
    let codec = codec.unwrap_or_default().to_ascii_lowercase();
    if codec.contains("av1") {
        2.0
    } else if codec.contains("265") || codec.contains("hevc") {
        1.6
    } else if codec.contains("vp9") {
        1.4
    } else {
        1.0
    }
}

/// Codec-adjusted size, used to compare two sources of the same title.
fn effective_size(option: &SourceOption) -> f64 {
    let codec = option.label.split_whitespace().nth(1);
    option.size_bytes.unwrap_or(0) as f64 * codec_efficiency(codec)
}

/// Bytes per vertical pixel — a resolution-independent density measure.
///
/// Used to spot a source whose resolution promises more than its bitrate can
/// deliver, which is the usual signature of an upscale or a poor rip.
fn density(option: &SourceOption) -> f64 {
    if option.height == 0 {
        return 0.0;
    }
    effective_size(option) / f64::from(option.height)
}

/// Order sources best-first and label each one's relative quality.
///
/// Height leads, because a viewer asking for 1080p wants 1080p. Within a
/// resolution, the codec-adjusted size decides, so the sharper encode wins even
/// when it is the smaller download.
pub fn rank(options: &mut [SourceOption]) {
    options.sort_by(|a, b| {
        b.height
            .cmp(&a.height)
            .then_with(|| effective_size(b).total_cmp(&effective_size(a)))
    });

    // A source is only "starved" relative to what else is on offer, so compare
    // against the richest source rather than a fixed bitrate threshold.
    let best_density = options.iter().map(density).fold(0.0_f64, f64::max);

    for (index, option) in options.iter_mut().enumerate() {
        option.recommended = index == 0;
    }

    let tiers: Vec<QualityTier> = options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            let starved = best_density > 0.0
                && option.size_bytes.is_some()
                && density(option) < best_density * 0.45;
            if starved {
                QualityTier::Lower
            } else if index == 0 {
                QualityTier::Best
            } else {
                QualityTier::Good
            }
        })
        .collect();

    for (option, tier) in options.iter_mut().zip(tiers) {
        option.quality = tier;
    }
}

/// The source to download when the caller expressed no preference.
///
/// Returns the highest-ranked source at or below `maximum_height`.
pub fn best_within(options: &[SourceOption], maximum_height: u16) -> Option<&SourceOption> {
    options
        .iter()
        .filter(|option| option.height <= maximum_height)
        .max_by(|a, b| {
            a.height
                .cmp(&b.height)
                .then_with(|| effective_size(a).total_cmp(&effective_size(b)))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::SourceId;

    fn option(height: u16, label: &str, size_gb: f64) -> SourceOption {
        SourceOption {
            id: SourceId::new(format!("{height}-{label}")),
            height,
            label: label.to_string(),
            size_bytes: Some((size_gb * 1024.0 * 1024.0 * 1024.0) as u64),
            language: None,
            recommended: false,
            quality: QualityTier::Good,
        }
    }

    #[test]
    fn higher_resolution_wins_first() {
        let mut options = vec![
            option(720, "720p H264", 1.5),
            option(1080, "1080p H264", 2.0),
        ];
        rank(&mut options);
        assert_eq!(options[0].height, 1080);
        assert!(options[0].recommended);
        assert!(!options[1].recommended);
    }

    #[test]
    fn bigger_file_wins_at_equal_resolution() {
        // The user's case: two 1080p sources, 2.5 GB versus 1.8 GB, same codec.
        let mut options = vec![
            option(1080, "1080p H264", 1.8),
            option(1080, "1080p H264", 2.5),
        ];
        rank(&mut options);
        assert_eq!(options[0].size_bytes, Some(2684354560));
        assert!(options[0].recommended);
    }

    #[test]
    fn efficient_codec_beats_a_larger_inefficient_one() {
        // 1.8 GB of HEVC carries more picture than 2.5 GB of H.264.
        let mut options = vec![
            option(1080, "1080p H264", 2.5),
            option(1080, "1080p H265", 1.8),
        ];
        rank(&mut options);
        assert_eq!(options[0].label, "1080p H265");
        assert!(options[0].recommended);
    }

    #[test]
    fn a_starved_encode_is_flagged_even_at_high_resolution() {
        // A 1080p file this small is the signature of a bad rip.
        let mut options = vec![
            option(1080, "1080p H264", 0.5),
            option(720, "720p H264", 2.0),
        ];
        rank(&mut options);
        let starved = options
            .iter()
            .find(|o| o.height == 1080)
            .expect("1080p source");
        assert_eq!(starved.quality, QualityTier::Lower);
    }

    #[test]
    fn best_within_respects_the_ceiling() {
        let options = vec![
            option(2160, "2160p H265", 8.0),
            option(1080, "1080p H264", 2.5),
            option(720, "720p H264", 1.2),
        ];
        let picked = best_within(&options, 1080).expect("a source within the limit");
        assert_eq!(picked.height, 1080);
    }

    #[test]
    fn best_within_prefers_the_richer_encode_at_the_same_height() {
        let options = vec![
            option(1080, "1080p H264", 1.2),
            option(1080, "1080p H264", 3.0),
        ];
        let picked = best_within(&options, 1080).expect("a source");
        assert_eq!(picked.size_bytes, Some(3221225472));
    }

    #[test]
    fn missing_sizes_do_not_panic_or_flag_everything() {
        let mut options = vec![
            SourceOption {
                id: SourceId::new("no-size".to_string()),
                height: 1080,
                label: "1080p".to_string(),
                size_bytes: None,
                language: None,
                recommended: false,
                quality: QualityTier::Good,
            },
            option(720, "720p H264", 1.0),
        ];
        rank(&mut options);
        assert_eq!(options[0].height, 1080);
        assert!(options[0].recommended);
    }

    #[test]
    fn best_within_returns_nothing_when_all_exceed_the_ceiling() {
        let options = vec![option(2160, "2160p H265", 8.0)];
        assert!(best_within(&options, 1080).is_none());
    }
}
