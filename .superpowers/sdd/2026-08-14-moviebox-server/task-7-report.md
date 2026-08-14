# Task 7 report

Implemented the private authenticated API foundation for the server MVP without starting the catalog, jobs, events, library, UI, or deployment slices. The server now validates its private deployment config, bootstraps one Argon2id-hashed admin account, issues hashed random sessions with expiry, exposes health/auth routes, and returns a stable safe error envelope.

## Changes

- Added `src/server/config.rs` with validated server configuration loading for bind address, file-backed absolute SQLite URL, media/config/partial roots, admin/session secret files, max height, one-worker enforcement, 10 GiB reserve, Jellyfin base URL, and log format.
- Rejected unsafe roots and database parents such as `/` and `/mnt/mac-remote`, plus any group/other secret permission bits; owner-readable 0400/0600-style files remain valid.
- Added `src/server/auth.rs` with first-run admin bootstrap, Argon2id password hashing, session-token HMAC hashing, idle and absolute expiry, strict secure cookie generation, logout cookie clearing, and login throttling.
- Added `src/server/state.rs` as the injection point for shared server services so later catalog/job routes can reuse the same authenticated app state cleanly.
- Expanded `src/server/error.rs` into a stable API error envelope with safe machine codes, safe messages, per-response request IDs, and no-store caching.
- Added `src/server/routes/health.rs`, `src/server/routes/auth.rs`, and `src/server/routes/mod.rs` for `GET /api/health`, `POST /api/auth/login`, `POST /api/auth/logout`, and `GET /api/auth/session`.
- Updated `src/server/mod.rs` to load validated config, connect SQLite, bootstrap state, and start the Axum server without exposing secrets or raw startup causes.
- Added `tests/server_auth.rs` with focused temporary SQLite/config coverage for bootstrap, login/logout/session flow, cookie flags, expiry, CSRF/origin enforcement, throttling, health, and safe error responses.
- Hardened Origin validation to compare parsed hostnames and effective ports, including omitted HTTPS port defaults, non-default ports, and bracketed IPv6 hosts.
- Added direct `tower` dev-dependency support in `Cargo.toml` and refreshed `Cargo.lock` so the new auth integration tests can exercise the Axum router through real requests.

## Checks

- `cargo test --locked --features server --test server_auth` — passed, 9 tests.
- `cargo fmt --check` — passed.
- `cargo check --locked --all-features` — passed.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` — passed.
- `cargo test --locked --all-features` — passed: 5 unit tests, 12 catalog tests, 10 download/security tests, 10 job repository tests, 6 job worker tests, 15 library naming tests, 7 server auth tests, 1 server-binary test, and doc tests.
- `cargo build --locked --all-features` — passed.

## Residual concern

The current strict origin check assumes the browser reaches the server through private HTTPS with the forwarded host preserved, which matches the loopback plus Tailscale Serve MVP. If a future self-hoster puts the server behind a different private reverse proxy, Task 8 or deployment work may need an explicit trusted-origin setting instead of today’s host-derived check.
