//! Aligning a subtitle to the audio it belongs to.
//!
//! A subtitle and a video routinely come from different releases of the same
//! film — a different cut, a different framerate, a different length of studio
//! logos before the first frame. The text is right and the timings are not, so
//! lines land seconds away from the speech and the file is effectively unusable.
//!
//! `ffsubsync` fixes this by listening rather than guessing: it detects speech
//! in the video's audio track and shifts and stretches the subtitle to match.

use std::{path::Path, process::Stdio, time::Duration};

use tokio::process::Command;

/// Long enough for a feature film, short enough that a hung process cannot hold
/// the download queue open indefinitely.
const SYNC_TIMEOUT: Duration = Duration::from_secs(300);

/// Realigns a subtitle file against its video.
#[async_trait::async_trait]
pub trait SubtitleSyncer: Send + Sync {
    /// Align `subtitle` to `video`, rewriting it in place.
    ///
    /// Returns whether the subtitle was changed. A failure is not an error: an
    /// unsynced subtitle is worth keeping, so the original stays as it is.
    async fn sync(&self, video: &Path, subtitle: &Path) -> bool;
}

/// Used when automatic alignment is turned off or unavailable.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopSubtitleSyncer;

#[async_trait::async_trait]
impl SubtitleSyncer for NoopSubtitleSyncer {
    async fn sync(&self, _video: &Path, _subtitle: &Path) -> bool {
        false
    }
}

/// Aligns subtitles by running `ffsubsync`.
#[derive(Debug, Clone)]
pub struct FfsubsyncSyncer {
    program: String,
}

impl Default for FfsubsyncSyncer {
    fn default() -> Self {
        Self {
            program: "ffsubsync".to_string(),
        }
    }
}

impl FfsubsyncSyncer {
    /// Use a specific executable rather than the one on `PATH`.
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

#[async_trait::async_trait]
impl SubtitleSyncer for FfsubsyncSyncer {
    async fn sync(&self, video: &Path, subtitle: &Path) -> bool {
        // Written to a sibling first: ffsubsync reads the input while writing,
        // so pointing the output at the input risks truncating the only copy of
        // a subtitle that was perfectly good, just misaligned.
        let mut aligned = subtitle.as_os_str().to_os_string();
        aligned.push(".synced");
        let aligned = Path::new(&aligned).to_path_buf();

        let spawned = Command::new(&self.program)
            .arg(video)
            .arg("-i")
            .arg(subtitle)
            .arg("-o")
            .arg(&aligned)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();

        let Ok(mut child) = spawned else {
            // Not installed, or not executable. The subtitle is still usable.
            return false;
        };

        let finished = match tokio::time::timeout(SYNC_TIMEOUT, child.wait()).await {
            Ok(Ok(status)) => status.success(),
            // A timeout or a spawn failure leaves a process to clean up; the
            // kill_on_drop guard handles the rest when `child` falls out here.
            _ => {
                let _ = child.kill().await;
                false
            }
        };

        if !finished {
            let _ = tokio::fs::remove_file(&aligned).await;
            return false;
        }

        // An empty or missing result means the run reported success without
        // producing anything usable; keep what we already had.
        match tokio::fs::metadata(&aligned).await {
            Ok(metadata)
                if metadata.len() > 0 && tokio::fs::rename(&aligned, subtitle).await.is_ok() =>
            {
                true
            }
            _ => {
                let _ = tokio::fs::remove_file(&aligned).await;
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_missing_program_is_reported_rather_than_panicking() {
        let syncer = FfsubsyncSyncer::with_program("definitely-not-installed-anywhere");
        let synced = syncer
            .sync(Path::new("/tmp/video.mkv"), Path::new("/tmp/subtitle.srt"))
            .await;
        assert!(!synced);
    }

    #[tokio::test]
    async fn a_failing_program_leaves_the_original_subtitle_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let subtitle = directory.path().join("movie.srt");
        let original = "1\n00:00:01,000 --> 00:00:02,000\nHello\n";
        tokio::fs::write(&subtitle, original).await.unwrap();

        // `false` exits non-zero without writing anything.
        let syncer = FfsubsyncSyncer::with_program("false");
        assert!(
            !syncer
                .sync(&directory.path().join("movie.mkv"), &subtitle)
                .await
        );

        // The subtitle we already had is better than none.
        assert_eq!(
            tokio::fs::read_to_string(&subtitle).await.unwrap(),
            original
        );
        assert!(!directory.path().join("movie.srt.synced").exists());
    }

    #[tokio::test]
    async fn a_successful_run_that_produces_nothing_keeps_the_original() {
        let directory = tempfile::tempdir().unwrap();
        let subtitle = directory.path().join("movie.srt");
        let original = "1\n00:00:01,000 --> 00:00:02,000\nHello\n";
        tokio::fs::write(&subtitle, original).await.unwrap();

        // `true` exits zero but writes no output file.
        let syncer = FfsubsyncSyncer::with_program("true");
        assert!(
            !syncer
                .sync(&directory.path().join("movie.mkv"), &subtitle)
                .await
        );
        assert_eq!(
            tokio::fs::read_to_string(&subtitle).await.unwrap(),
            original
        );
    }

    #[tokio::test]
    async fn the_noop_syncer_never_claims_to_have_changed_anything() {
        let synced = NoopSubtitleSyncer
            .sync(Path::new("/tmp/v.mkv"), Path::new("/tmp/s.srt"))
            .await;
        assert!(!synced);
    }
}
