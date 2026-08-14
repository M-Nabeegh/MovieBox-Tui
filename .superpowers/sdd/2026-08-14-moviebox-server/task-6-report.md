# Task 6 report

Implemented and hardened a small Task 6 vertical slice for the server download queue: restart recovery, one-job completion into the library, disk-reserve gating, and sanitized job events. The follow-up keeps the worker honest under low disk and rejects forged partial paths without touching arbitrary media-root files.

## Changes

- Added `src/server/jobs/recovery.rs` with restart repair for interrupted jobs.
- Requeue `resolving`, `downloading`, and `finalizing` jobs on startup while preserving valid partial files.
- Mark interrupted jobs as `failed` with `unsafe_path` when any stored final or partial path escapes the configured media root.
- Added `src/server/jobs/worker.rs` with a minimal `JobWorker`, `JobStore`, `TransferClient`, `HttpTransferClient`, `DiskSpaceChecker`, and `JobStatePatch`.
- Implemented a single-worker claim/resolve/download/finalize flow that reuses `CatalogProvider`, `DownloadRequest`, `DownloadClient`, and `LibraryNamer`.
- Download into the UUID-scoped partial area first, then atomically rename into the final library path before marking the job `ready`.
- Enforced a reserve-space check before transfer and requeued jobs with `insufficient_space` without contacting the remote media URL.
- Added an explicit `WorkerRunOutcome`; low-space requeues return `Deferred`, and `run()` stops after that one attempt instead of sleeping and reclaiming the same job. A later server wake/restart can start it again.
- Enforced the exact `_moviebox/jobs/<job-id>/<name>.part` layout for video and subtitle partial paths before recovery, transfer, and finalization. Invalid queued/recovered jobs fail with the generic `unsafe_path` code.
- Recovery only promotes a `finalizing` job to `ready` when `total_bytes` is recorded and the final file length matches it exactly; unknown-size final files are requeued conservatively.
- Added a single refresh retry for expired transfer URLs and kept failure messages generic so provider URLs, tokens, and raw paths do not leak.
- Added `src/server/events.rs` and exported it through `src/server/mod.rs`; event payloads now sanitize suspicious string fields before publishing.
- Wired the new worker/recovery exports through `src/server/jobs/mod.rs`.
- Kept the existing repository scaffold and completed its Task 6 dependency on `JobStatePatch`.
- Removed the misleading public pause/cancel/retry controls and dormant deletion path from this MVP; those controls require a separately specified lifecycle/API slice.
- Reduced `tests/job_worker.rs` to six local proofs: restart recovery, conservative unknown-size recovery, one successful completion, one-attempt low-space stop with one claim/resolve, forged in-root partial rejection, and sanitized insufficient-space events.
- Added `#![allow(dead_code)]` to `tests/support/http_server.rs` because the trimmed worker slice no longer exercises every helper in that shared fixture server.

## Checks

- `cargo fmt --check` — passed.
- `cargo check` — passed.
- `cargo check --all-features` — passed.
- `cargo test --features server --test job_worker` — passed, 6 tests.
- `cargo clippy --all-targets --all-features -- -D warnings` — passed.
- `cargo test --all-features` — passed: 5 unit tests, 12 catalog tests, 10 download/security tests, 10 job repository tests, 6 job worker tests, 15 library naming tests, 1 server-binary test, and doc tests.
- `cargo build --all-features` — passed.

## Residual concern

This slice intentionally does not expose pause/cancel/retry or deletion controls. A future lifecycle/API task must define those transitions and cleanup semantics before adding them back. Recovery remains conservative for finalizing jobs whose recorded size is missing or mismatched.
