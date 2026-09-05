# MovieBox Authenticated DASH Downloads Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the notice-video MovieBox download path with authenticated, quality-bounded MPEG-DASH downloading while preserving the server queue and Jellyfin workflow.

**Architecture:** Port the upstream visitor-session and signed-policy parsing at the provider boundary, then represent DASH explicitly in the resolved-source/download-request contract. The server transfer layer will fetch and validate the manifest through the existing guarded network client, invoke the installed FFmpeg/FFprobe tools in stream-copy mode, handle cancellation and signed-session refresh, and validate the resulting media before the worker can finalize it.

**Tech Stack:** Rust 1.90, Tokio, reqwest/rustls, Axum server, FFmpeg/FFprobe, SQLite/sqlx, Vitest/React, Docker Compose.

**Spec:** `docs/superpowers/specs/2026-09-05-moviebox-dash-downloads.md`

## Global Constraints

- Port only the MovieBox provider and server-download behavior required by upstream commit `c5c591047dfc5a86e90479afbda394ef03e1df20`; do not merge unrelated upstream TUI changes.
- Maximum selectable/downloaded video height is 1080p.
- FFmpeg must use stream copy/remuxing; no video or audio transcoding.
- Existing HTTP-file transfer, safe paths, public-network validation, queue concurrency one, subtitles, and Jellyfin refresh remain functional.
- A DASH transfer cannot become Ready unless FFprobe confirms at least one video stream and a duration consistent with the provider metadata; the 917,554-byte, approximately 20.97-second notice must be rejected.
- JWTs, signed cookies, API keys, passwords, and secret-file contents must never be printed, logged, committed, or placed in test fixtures.
- Do not delete media, reset the repository, or overwrite the live compose file, `.env`, Tailscale configuration, or secret files.
- The implementation must follow red-green TDD and include the failing-test evidence in the task report.

---

### Task 1: Authenticated MovieBox DASH download path

**Files:**
- Create: `src/providers/moviebox/session.rs`
- Create: `src/server/jobs/dash.rs`
- Modify: `src/providers/moviebox/client.rs`
- Modify: `src/providers/moviebox/mod.rs`
- Modify: `src/catalog/models.rs`
- Modify: `src/catalog/moviebox.rs`
- Modify: `src/download.rs`
- Modify: `src/server/jobs/mod.rs`
- Modify: `src/server/jobs/worker.rs`
- Modify: `tests/catalog_moviebox.rs`
- Modify: `tests/job_worker.rs`
- Create: `tests/dash_transfer.rs`
- Modify: `README.md`

**Interfaces:**
- Consumes: upstream visitor login `POST /wefeed-mobile-bff/user-api/visitor-login`, authenticated `GET /wefeed-mobile-bff/subject-api/play-info/v2`, existing opaque MovieBox source IDs, `DownloadClient`, `TransferClient`, and the installed `ffmpeg`/`ffprobe` binaries.
- Produces: `catalog::SourceTransport::{HttpFile, Dash { maximum_height, expected_duration_seconds }}`, stored on `ResolvedSource` and copied onto `DownloadRequest`, so ordinary files and signed DASH manifests take explicit transfer paths.
- Produces: a MovieBox session manager that reuses a valid in-memory visitor token, performs one refresh after 401/403, and never logs the token.
- Produces: a DASH transfer that returns the existing `DownloadOutcome`, reports byte progress, terminates on pause/cancel, and maps authorization expiry into the worker's existing re-resolve path.

- [ ] **Step 1: Add failing provider fixtures and tests**

Add hand-authored, non-secret fixtures representing `play-info/v2` with a signed CloudFront policy, multiple resolutions, resource IDs, duration, and a legacy notice URL. Tests must prove that the selected source resolves to `index.mpd`, carries the cookie and mobile identity only in request headers, preserves season/episode/resource identity, rejects an invalid policy, and never returns the notice URL.

- [ ] **Step 2: Run focused provider tests and record RED**

Run: `cargo test --features server --test catalog_moviebox --locked`

Expected before implementation: compilation/test failure because visitor sessions, DASH transport, and play-info adaptation are absent.

- [ ] **Step 3: Add failing transfer and worker tests**

Use controlled local fixtures/fake media-tool runners rather than live network calls. Cover 1080p-or-lower stream selection, stream-copy arguments, header redaction, cancellation, non-zero FFmpeg failure, 401/403 refresh classification, output without video, notice-length/duration mismatch, successful finalization, and unchanged ordinary HTTP behavior. Name the production branch each test would catch.

- [ ] **Step 4: Run focused transfer/worker tests and record RED**

Run: `cargo test --features server --test job_worker --locked`

Run: `cargo test --features server --test dash_transfer --locked`

Expected before implementation: compilation/test failure because DASH routing and media validation are absent.

- [ ] **Step 5: Implement visitor authentication and play-info adaptation**

Port the behavioral design of upstream commit `c5c591047dfc5a86e90479afbda394ef03e1df20`: single-flight visitor login, in-memory expiry-aware token reuse, one invalidation/relogin on 401/403, `play-info/v2`, CloudFront policy decoding, and deterministic stream/resource matching. Keep legacy resource data only where still required for source metadata/subtitles; do not select its `resourceLink` for video.

- [ ] **Step 6: Implement guarded DASH transfer and media validation**

Fetch the MPD through the existing `DownloadClient` redirect/DNS/peer validation before invoking FFmpeg. Reject unsafe or non-MPD input and prevent the manifest from enabling local-file or private-network reads. Select the best video stream not exceeding the requested height, select one audio stream when available, and invoke FFmpeg with `-c copy`. Keep authorization material out of application logs and captured error messages. On cancellation terminate the child promptly; on retry restart/replace only the job's contained partial output. Use FFprobe after FFmpeg exits to require a video stream and compare duration with provider metadata before returning `Completed`.

- [ ] **Step 7: Preserve subtitles and worker invariants**

Keep external-caption resolution tied to the selected MovieBox resource ID. Keep ordinary HTTP exact-size validation unchanged. For DASH, use media validation rather than byte-for-byte equality with provider size because remuxed container size can differ. Preserve the existing Ready transition, subtitle alignment, notification, and best-effort Jellyfin refresh ordering.

- [ ] **Step 8: Verify GREEN and regressions locally**

Run:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cd web && npm run typecheck && npm run test:run && npm run build
docker compose -f deploy/compose/compose.yml --env-file deploy/compose/.env.example config --quiet
bats tests/compose.bats tests/tailscale_serve.bats
shellcheck deploy/tailscale/moviebox-serve.sh
```

Expected: every command exits zero with no Rust warnings or test failures.

- [ ] **Step 9: Document operational behavior**

Update README download troubleshooting to explain authenticated DASH, FFmpeg stream-copy (low CPU), restart-from-zero semantics if DASH cannot safely resume, the integrity validation, and that signed authorization values are never logged.

- [ ] **Step 10: Commit the implementation checkpoint**

Commit only intentional tracked files with message:

```bash
git commit -m "fix(server): download authenticated MovieBox DASH streams"
```

- [ ] **Step 11: Deploy without overwriting live scratch and verify PK**

Push `feat/moviebox-server`, update only tracked application sources on `/home/nabeegh/moviebox-server`, rebuild/recreate the MovieBox container, and leave live compose/`.env`/Tailscale/secrets untouched. Confirm `/mnt/nas-data` is `/dev/sda1`, containers are healthy, and retry exactly one PK job. Do not delete existing media. Probe the finished file with FFprobe and require feature-length video rather than the known 917,554-byte/20.97-second notice before reporting success.
