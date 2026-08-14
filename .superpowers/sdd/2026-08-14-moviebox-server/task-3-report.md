# Task 3 report

Implemented secure, header-aware resumable downloads without changing provider behavior.

## Changes

- Added `DownloadRequest` with parsed URL, `HeaderMap`, and redirect budget.
- Propagated source headers through probes, redirects, range/segment requests, retries, resumes, and subtitles.
- Disabled reqwest automatic redirects for the TUI download client and added manual per-hop URL and DNS/address validation.
- Rejected unsafe private, loopback, link-local, multicast, unspecified, documentation, and IPv4-mapped private IPv6 destinations.
- Added `contained_path` checks for traversal, control characters, symlink escapes, and media-root containment.
- Added local generated HTTP/range fixtures and security tests; no external sites or media are used.
- Preserved URL-only TUI callers with empty headers and preserved headers from header-bearing playback sources.

## Checks

- `cargo fmt` — passed.
- `cargo fmt --check` — passed.
- `cargo test --locked --features server --test download_request` — passed, 9 tests.
- `cargo test --all-features --locked` — passed: 4 library tests, 12 catalog tests, 9 download/security tests, 1 server-binary test, and doc tests.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` — passed.
- `cargo build --locked` — passed (default-feature build).

The default-feature `tests/server_binary.rs` is intentionally gated with a compile error when `server` is absent, so an unqualified `cargo test --locked` is not a valid default-feature check. The server-enabled test suite above passes, and the default-feature build passes.

## Residual concern

The manual DNS/address check runs before each request, while the reqwest resolver performs the subsequent connection. Future server download-client construction must continue to use automatic redirects disabled and should consider resolver pinning if DNS-rebinding resistance beyond the current per-hop policy is required.
