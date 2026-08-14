# Task 5 report

Implemented SQLite-backed download job persistence for the server feature without starting worker, API, UI, or deployment tasks.

## Changes

- Added `migrations/0001_server.sql` with `users`, `sessions`, `jobs`, and `job_events`, plus the required job-state and session-expiry indexes.
- Added `src/server/db.rs` to open SQLite pools with foreign keys enabled and run the server migrations automatically.
- Added `src/server/jobs/model.rs` with typed job IDs, a browser-safe compound `JobListCursor`, states, progress/event metadata, relative-path validation, and the legal state-transition matrix.
- Added `src/server/jobs/repository.rs` with transactional `create`, atomic `claim_next`, compare-and-set `transition`, guarded `update_progress`, keyset-paginated `list` ordered by `created_at DESC, id DESC`, and durable event writes.
- Hardened review findings by making `JobEvent::with_warning` return the same fallible validation error as `with_error`, and by covering malformed warnings plus equal-timestamp pagination through all rows.
- Added `src/server/jobs/mod.rs` and minimal `src/server/mod.rs` exports so the new persistence layer is available behind the existing `server` feature gate.
- Added `tests/job_repository.rs` with temp-SQLite coverage for migration shape, create/read, legal and illegal transitions, stale-version conflicts, progress persistence, compound-cursor pagination including equal timestamps, concurrent claiming, and malformed warning rejection.
- Kept persisted job identifiers opaque and stored only relative media paths; no provider URLs, tokens, or absolute filesystem paths are written into browser-facing job rows.

## Checks

- `cargo test --locked --features server --test job_repository` — passed, 10 tests.
- `cargo fmt --check` — passed.
- `cargo check --locked --all-features` — passed.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` — passed.
- `cargo build --locked --all-features` — passed.
- `cargo test --locked --all-features` — passed: 5 library tests, 12 catalog tests, 10 download/security tests, 10 job repository tests, 15 library-naming tests, 1 server-binary test, and doc tests.
- `cargo sqlx migrate info --source migrations` — could not run because `cargo-sqlx` is not installed in this environment.

## Residual concern

Task 5 stores session rows and expiry indexes, but the authentication workflow that will actually create, hash, rotate, and expire those sessions still belongs to later tasks. The migration and repository layer are ready for that follow-on work.
