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
- Kept all output paths relative to the configured media root and validated them through the existing `contained_path` policy before returning them.
- Added deterministic completed-target collision handling that returns `LibraryError::AlreadyExists` instead of inventing suffixes like `_2`.
- Added job-UUID-based partial output paths under `_moviebox/jobs/<uuid>/...` so in-progress files stay outside the final `Movies/` and `Shows/` library trees.
- Exported the new server library module through `src/server/mod.rs`.
- Added generated Task 4 tests covering movie naming, episode naming, subtitle sidecars, Unicode/reserved/control/traversal sanitization, long-title truncation, unsupported extensions, collision handling, and partial job paths.

## Checks

- `cargo fmt` — passed.
- `cargo fmt --check` — passed.
- `cargo test --locked --features server --test library_naming` — passed, 9 tests.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` — passed.
- `cargo build --locked` — passed.
- `cargo build --locked --all-features` — passed.
- `cargo test --all-features --locked` — passed: 5 unit tests, 12 catalog tests, 10 download/security tests, 9 library naming tests, 1 server-binary test, and doc tests.

## Residual concern

The sanitization preserves Unicode and removes path-unsafe characters, but it does not add a new external Unicode normalization dependency in this task. If later server tasks need byte-for-byte normalization compatibility with a specific filesystem or Jellyfin importer edge case, that should be validated against real deployment fixtures before widening the policy.
