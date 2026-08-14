# Task 4 report

Implemented Jellyfin-compatible, collision-safe media naming for the server feature without touching the SQLite/jobs/API/Jellyfin deployment tasks.

## Changes

- Added `src/server/library/` with `MediaIdentity`, `MediaPaths`, `LibraryError`, and `LibraryNamer`.
- Built deterministic movie naming as `Movies/Title (Year)/Title (Year).ext`.
- Built deterministic series naming as `Shows/Series (Year)/Season 01/Series (Year) S01E02 Episode Title.ext`.
- Added subtitle sidecar naming that reuses the same stem and appends `.language.ext`.
- Validated supported video extensions: `mp4`, `mkv`, `webm`, `avi`, `mov`, `m4v`.
- Validated supported subtitle extensions: `srt`, `vtt`, `ass`, `ssa`, `sub`.
- Sanitized path components for control characters, path separators, traversal-like dot segments, reserved Windows device names, and long titles while preserving Unicode titles instead of ASCII-folding them.
- Normalized title, language, extension, and generated naming components to Unicode NFC before sanitization, reserved-name checks, truncation, and path generation.
- Added a feature-gated `unicode-normalization` dependency and byte-aware component budgeting so final, subtitle, and partial filenames remain within 255 bytes while retaining the existing grapheme limits.
- Reserved a fixed worst-case subtitle suffix budget from the configured sanitized-language limit and longest supported subtitle extension, keeping the media stem stable when subtitles are absent or language labels vary.
- Applied Windows reserved-name protection to the basename before the first dot, including dotted inputs such as `NUL.txt`, `CON.log`, and `COM1.srt`.
- Covered Windows device-name variants `COM¹`/`COM²`/`COM³` and `LPT¹`/`LPT²`/`LPT³` alongside the existing ASCII `1`–`9` forms.
- Matched series episodes by season/episode number independently of episode-title presence; missing catalog titles now retain a valid `SxxExx` stem.
- Kept all output paths relative to the configured media root and validated them through the existing `contained_path` policy before returning them.
- Added deterministic completed-target collision handling that returns `LibraryError::AlreadyExists` instead of inventing suffixes like `_2`.
- Added job-UUID-based partial output paths under `_moviebox/jobs/<uuid>/...` so in-progress files stay outside the final `Movies/` and `Shows/` library trees.
- Exported the new server library module through `src/server/mod.rs`.
- Added generated Task 4 tests covering movie naming, episode naming, subtitle sidecars, Unicode/reserved/control/traversal sanitization, long-title truncation, unsupported extensions, collision handling, and partial job paths.

## Checks

- `cargo fmt` — passed.
- `cargo fmt --check` — passed.
- `cargo test --locked --features server --test library_naming` — passed, 15 tests, including NFC-equivalence, multibyte byte-limit, subtitle-independent stems, dotted reserved names, superscript device-name variants, and missing-episode-title regressions.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` — passed.
- `cargo build --locked` — passed.
- `cargo build --locked --all-features` — passed.
- `cargo test --all-features --locked` — passed: 5 unit tests, 12 catalog tests, 10 download/security tests, 12 library naming tests, 1 server-binary test, and doc tests.

## Residual concern

The 128-byte sanitized-language cap deliberately truncates unusually long language labels to keep stems identity-stable under the 255-byte component limit; real Jellyfin importer behavior and filesystem fixtures remain deployment-level validation for later tasks. No media is created and no external service is called by these tests.
