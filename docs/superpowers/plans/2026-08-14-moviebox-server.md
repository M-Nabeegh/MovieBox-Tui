# MovieBox Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a private, forkable, self-hosted MovieBox Server that searches the existing MovieBox catalog, queues resumable server-side downloads capped at 1080p, writes Jellyfin-compatible media and subtitles, and opens completed titles in Jellyfin.

**Architecture:** Preserve the existing TUI and add a feature-gated `moviebox-server` Rust binary in the same package. The server exposes an authenticated Axum API and embedded React interface, stores jobs in SQLite, resolves all provider URLs server-side, and writes completed files beneath a configured media root. The official Jellyfin container scans the library and provides playback; both services bind to loopback and are published privately with Tailscale Serve.

**Tech Stack:** Rust 1.90/edition 2024, Tokio, Axum, SQLx/SQLite, Reqwest/Rustls, Argon2id, Tower HTTP, React 19, TypeScript, Vite, Vitest, Playwright, Docker Compose, Jellyfin, Tailscale Serve.

## Global Constraints

- Keep the existing `moviebox-tui` binary and its macOS/Linux/Windows behavior working.
- The server binary supports Linux x86-64 first; the browser remains cross-platform.
- MovieBox is the only server provider in version 1; TUI provider behavior remains unchanged.
- Enforce `maximum_height = 1080` in server code; never rely on browser filtering.
- Run one download worker by default and reserve at least 10 GiB on the media filesystem.
- Never expose direct video/subtitle URLs, provider tokens, or raw provider responses to the browser.
- Never accept an arbitrary download URL, filesystem path, M3U URL, or redirect target from a client.
- Bind application and Jellyfin host ports to `127.0.0.1` in the private deployment.
- Do not use Tailscale Funnel or router port-forwarding.
- Store the personal deployment only under `/mnt/nas-data/moviebox`; never use `/mnt/mac-remote`. Public self-hosters choose their own dedicated media root.
- Run containers as non-root where supported, drop unnecessary capabilities, and mount Jellyfin media read-only.
- Preserve the repository's MIT OR Apache-2.0 licensing and credit the upstream project.
- Tests and demonstrations use generated fixtures or freely licensed sample media, never copyrighted downloads.

---

## Planned file map

```text
Cargo.toml                                  feature gates, server dependencies and binary
src/lib.rs                                 export catalog/server modules behind features
src/catalog/mod.rs                         stable CatalogService interface
src/catalog/models.rs                      typed browser-safe catalog DTOs
src/catalog/moviebox.rs                    MovieBox JSON adapters and opaque IDs
src/download.rs                            header-aware resumable transfer request
src/bin/moviebox-server.rs                 server process entrypoint
src/server/mod.rs                          server module exports and startup
src/server/config.rs                       validated environment configuration
src/server/error.rs                        API error codes and response envelope
src/server/state.rs                        shared services and application state
src/server/auth.rs                         Argon2id bootstrap and sessions
src/server/db.rs                           SQLite pool and migrations
src/server/events.rs                       server-sent event broadcaster
src/server/routes/*.rs                     health, auth, catalog, jobs and library routes
src/server/jobs/model.rs                   job state and request types
src/server/jobs/repository.rs              transactional job persistence
src/server/jobs/worker.rs                  resolver/download/finalization worker
src/server/jobs/recovery.rs                restart state repair
src/server/library/naming.rs               Jellyfin-safe media paths
src/server/library/jellyfin.rs             scan and deep-link integration
src/server/security/net.rs                 public-address and redirect validation
src/server/security/path.rs                media-root path containment
migrations/0001_server.sql                 users, sessions, jobs and job_events
web/package.json                           frontend scripts and pinned dependencies
web/src/api/*.ts                           typed API client and SSE connection
web/src/features/*                         login, search, details, queue and ready views
web/src/styles/*                           responsive Netflix-inspired visual system
web/tests/*                                Vitest component tests
web/e2e/*                                  Playwright browser flows
docker/server.Dockerfile                   frontend plus Rust multi-stage image
deploy/compose/compose.yml                 server and official Jellyfin services
deploy/compose/.env.example                ports, paths, IDs and 1080p defaults
deploy/compose/secrets/README.md            local secret-generation instructions
deploy/tailscale/moviebox-serve.sh          idempotent private Serve configuration
tests/fixtures/*                            provider JSON and generated media metadata
tests/support/http_server.rs                deterministic range/redirect fixture server
.github/workflows/ci.yml                    Rust, web, Compose and security checks
README.md                                   fork identity and self-host quick start
docs/server/*.md                            architecture, security, backup and operations
```

## Phase 1 — Preserve the fork and establish server boundaries

### Task 1: Synchronize the fork and create the server build boundary

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Create: `src/bin/moviebox-server.rs`
- Create: `src/server/mod.rs`
- Create: `src/server/error.rs`
- Test: `tests/server_binary.rs`

**Interfaces:**
- Produces: Cargo feature `server` and binary target `moviebox-server`.
- Produces: `pub async fn server::run() -> Result<(), server::error::ServerError>`.
- Preserves: default `cargo build --locked` still builds the existing TUI.

- [ ] **Step 1: Add an `upstream` remote and compare before changing code**

```bash
git remote get-url upstream >/dev/null 2>&1 || git remote add upstream https://github.com/mesamirh/MovieBox-Tui.git
git fetch upstream main
git log --oneline --left-right --cherry-pick HEAD...upstream/main
```

Expected: list the four upstream-only commits found during design. Review them, then fast-forward or merge upstream `main` before creating `feat/moviebox-server`; do not overwrite fork-only commits.

- [ ] **Step 2: Create the implementation branch and prove the baseline**

```bash
git switch -c feat/moviebox-server
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Expected: all baseline commands pass before server dependencies are introduced.

- [ ] **Step 3: Write a failing binary smoke test**

```rust
#[test]
fn server_binary_is_built_with_server_feature() {
    let path = assert_cmd::cargo::cargo_bin!("moviebox-server");
    assert!(path.exists());
}
```

Run:

```bash
cargo test --features server --test server_binary
```

Expected: FAIL because the server binary target does not exist.

- [ ] **Step 4: Add feature-gated dependencies and binary metadata**

Add `server = [...]` and optional dependencies for `axum`, `sqlx` with SQLite and migrations, `tower-http`, `argon2`, `cookie`, `uuid`, `time`, `tokio-stream`, `async-stream`, `async-trait`, `rust-embed`, `mime_guess`, and `tracing`. Add dev dependencies `assert_cmd`, `tempfile`, and `wiremock`. Keep server dependencies out of the default TUI build.

```toml
[[bin]]
name = "moviebox-server"
path = "src/bin/moviebox-server.rs"
required-features = ["server"]
```

- [ ] **Step 5: Add the smallest compiling server entrypoint**

```rust
#[tokio::main]
async fn main() -> Result<(), moviebox_tui::server::error::ServerError> {
    moviebox_tui::server::run().await
}
```

Create `src/server/error.rs` with a non-sensitive `ServerError::Startup(String)` variant. `server::run` initially returns `Ok(())`; Task 7 replaces it with validated startup logic.

- [ ] **Step 6: Run the focused and baseline checks**

```bash
cargo test --features server --test server_binary
cargo build --locked
cargo build --locked --features server --bin moviebox-server
cargo clippy --all-targets --all-features --locked -- -D warnings
```

Expected: the focused test and both binaries pass.

- [ ] **Step 7: Commit the build boundary**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/bin/moviebox-server.rs src/server/mod.rs src/server/error.rs tests/server_binary.rs
git commit -m "feat(server): add feature-gated server binary"
```

### Task 2: Introduce typed, browser-safe catalog models

**Files:**
- Create: `src/catalog/mod.rs`
- Create: `src/catalog/models.rs`
- Create: `src/catalog/moviebox.rs`
- Modify: `src/lib.rs`
- Test: `tests/catalog_moviebox.rs`
- Create: `tests/fixtures/moviebox-search.json`
- Create: `tests/fixtures/moviebox-details.json`
- Create: `tests/fixtures/moviebox-resources.json`
- Create: `tests/fixtures/moviebox-captions.json`
- Create: `tests/support/fixtures.rs`

**Interfaces:**
- Produces: `CatalogService::search`, `details`, `sources`, and `subtitles`.
- Produces: `CatalogItem`, `CatalogDetails`, `SourceOption`, `SubtitleTrack`, and opaque ID newtypes.
- Guarantees: serialized DTOs contain no `url`, `link`, `token`, or raw provider JSON fields.

- [ ] **Step 1: Capture sanitized provider fixtures**

Remove stream URLs, tokens, and personal data from captured JSON. Replace identifiers with deterministic fixture values such as `subject-fixture-1` and `resource-fixture-1080`.

Add `tests/support/fixtures.rs` with `pub fn fixture(name: &str) -> serde_json::Value` that reads only the four allowlisted fixture names from `tests/fixtures` and panics with the fixture name when test data is invalid.

- [ ] **Step 2: Write failing adapter and serialization tests**

```rust
#[test]
fn source_options_are_capped_and_hide_direct_urls() {
    let payload: serde_json::Value = fixture("moviebox-resources.json");
    let options = adapt_sources(&payload, QualityPolicy::new(1080)).unwrap();
    assert_eq!(options.iter().map(|x| x.height).collect::<Vec<_>>(), vec![1080, 720]);
    let serialized = serde_json::to_string(&options).unwrap();
    assert!(!serialized.contains("https://"));
    assert!(!serialized.contains("resourceLink"));
}
```

Run:

```bash
cargo test --features server --test catalog_moviebox
```

Expected: FAIL because typed adapters do not exist.

- [ ] **Step 3: Define exact domain types**

```rust
pub struct SourceOption {
    pub id: SourceId,
    pub height: u16,
    pub label: String,
    pub size_bytes: Option<u64>,
    pub language: Option<String>,
    pub recommended: bool,
}

pub struct EpisodeRequest {
    pub catalog_id: CatalogId,
    pub season: Option<u16>,
    pub episode: Option<u16>,
}

pub struct ResolvedSource {
    pub url: url::Url,
    pub headers: reqwest::header::HeaderMap,
    pub subtitle: Option<ResolvedSubtitle>,
    pub extension: String,
    pub expected_size: Option<u64>,
}

pub struct ResolvedSubtitle {
    pub url: url::Url,
    pub headers: reqwest::header::HeaderMap,
    pub language: String,
    pub extension: String,
}

pub struct QualityPolicy {
    maximum_height: u16,
}

impl QualityPolicy {
    pub fn new(maximum_height: u16) -> Self;
    pub fn validate(&self, height: u16) -> Result<(), CatalogError>;
}
```

Opaque IDs must encode provider/resource identity with URL-safe base64 plus an HMAC generated from the server secret. They must not contain a direct media URL.

Define `OpaqueIdCodec::new([u8; 32])`, `encode(&OpaquePayload) -> String`, and `decode(&str) -> Result<OpaquePayload, CatalogError>`. Adapter tests use `[7_u8; 32]`; server startup derives the production codec key from the session-key secret.

- [ ] **Step 4: Implement MovieBox adapters**

Map search/details/resource/caption payloads into the typed models. Sort sources by height descending, filter above 1080, and mark only the first remaining option recommended. Return `CatalogError::QualityUnavailable` when nothing remains.

- [ ] **Step 5: Implement the catalog service facade**

```rust
#[async_trait]
pub trait CatalogProvider: Send + Sync {
    async fn search(&self, query: &str, page: u32) -> Result<SearchPage, CatalogError>;
    async fn details(&self, id: &CatalogId) -> Result<CatalogDetails, CatalogError>;
    async fn sources(&self, request: EpisodeRequest) -> Result<Vec<SourceOption>, CatalogError>;
    async fn subtitles(&self, source: &SourceId) -> Result<Vec<SubtitleTrack>, CatalogError>;
    async fn resolve(&self, source: &SourceId, subtitle: Option<&SubtitleId>) -> Result<ResolvedSource, CatalogError>;
}
```

- [ ] **Step 6: Run adapter, formatting and lint checks**

```bash
cargo test --features server --test catalog_moviebox
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
```

Expected: all adapter tests pass and DTO snapshots contain no direct URLs.

- [ ] **Step 7: Commit the catalog boundary**

```bash
git add src/catalog src/lib.rs tests/catalog_moviebox.rs tests/fixtures
git commit -m "feat(server): add typed moviebox catalog facade"
```

## Phase 2 — Make downloads daemon-safe

### Task 3: Add required headers, safe redirects and deterministic targets to downloads

**Files:**
- Modify: `src/download.rs`
- Create: `src/server/security/net.rs`
- Create: `src/server/security/path.rs`
- Create: `src/server/security/mod.rs`
- Test: `tests/download_request.rs`
- Create: `tests/support/http_server.rs`
- Create: `tests/support/mod.rs`

**Interfaces:**
- Replaces: `download(client, url, destination, cancel, report)`.
- Produces: `download(client, DownloadRequest, destination, cancel, report)`.
- Produces: `validate_public_http_url`, `resolve_public_addresses`, `follow_checked_redirects`, and `contained_path`.

- [ ] **Step 1: Write failing required-header and redirect tests**

```rust
#[tokio::test]
async fn download_sends_required_headers_on_every_request() {
    let fixture = RangeFixture::requiring("Referer", "https://provider.example/").await;
    let outcome = download(&client(), fixture.request(), &target, cancel(), |_| {}).await.unwrap();
    assert!(matches!(outcome, DownloadOutcome::Completed { .. }));
    fixture.assert_all_requests_had_required_header().await;
}

#[tokio::test]
async fn redirect_to_private_address_is_rejected() {
    let error = follow_checked_redirects(&client(), public_redirect_to("http://127.0.0.1/x"))
        .await
        .unwrap_err();
    assert!(matches!(error, NetworkPolicyError::PrivateAddress(_)));
}
```

Run:

```bash
cargo test --features server --test download_request
```

Expected: FAIL because requests cannot carry headers and redirects are unchecked.

- [ ] **Step 2: Define the transfer request**

```rust
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub url: url::Url,
    pub headers: reqwest::header::HeaderMap,
    pub maximum_redirects: u8,
}
```

Use a Reqwest client with automatic redirects disabled. Validate scheme, DNS results, and every redirect. Reject loopback, private, link-local, multicast, unspecified, documentation, and IPv4-mapped private IPv6 addresses.

- [ ] **Step 3: Apply headers to probes, ranges, segments and subtitle requests**

Every request path in `download.rs`, including segmented workers, must start from the same validated `DownloadRequest`. Tests must assert headers survive retries and range resumes.

- [ ] **Step 4: Add path containment**

```rust
pub fn contained_path(root: &Path, relative: &Path) -> Result<PathBuf, PathPolicyError>;
```

Reject absolute components, `..`, symlink escapes in existing parents, control characters, and paths that canonicalize outside the configured media root.

- [ ] **Step 5: Preserve TUI compatibility**

Update TUI callers to construct `DownloadRequest` from existing links. For header-bearing sources, pass `PlaybackSource.headers`; for MovieBox links, use an empty header map. Keep destination behavior unchanged for the TUI.

- [ ] **Step 6: Run focused and full tests**

```bash
cargo test --features server --test download_request
cargo test --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
```

Expected: headers, resume, redirect rejection, cancellation, and existing TUI tests pass.

- [ ] **Step 7: Commit secure transfer support**

```bash
git add src/download.rs src/server/security src/tui/app/download.rs tests/download_request.rs tests/support
git commit -m "feat(download): secure header-aware resumable transfers"
```

### Task 4: Generate Jellyfin-compatible, collision-safe media paths

**Files:**
- Create: `src/server/library/mod.rs`
- Create: `src/server/library/naming.rs`
- Test: `tests/library_naming.rs`

**Interfaces:**
- Produces: `MediaIdentity`, `MediaPaths`, and `LibraryNamer::paths_for`.
- Consumes: configured media root and typed catalog details.

- [ ] **Step 1: Write failing movie, episode, subtitle and traversal tests**

```rust
#[test]
fn episode_paths_follow_jellyfin_naming() {
    let paths = namer().paths_for(&episode("Example Show", 2024, 1, 2, "Second Episode"), "mkv", Some("en"), "srt").unwrap();
    assert_eq!(paths.video_relative, PathBuf::from("Shows/Example Show (2024)/Season 01/Example Show (2024) S01E02 Second Episode.mkv"));
    assert_eq!(paths.subtitle_relative.unwrap(), PathBuf::from("Shows/Example Show (2024)/Season 01/Example Show (2024) S01E02 Second Episode.en.srt"));
}
```

Run:

```bash
cargo test --features server --test library_naming
```

Expected: FAIL because the library namer does not exist.

- [ ] **Step 2: Implement sanitization and naming**

Use Jellyfin's `Title (Year)` movie folders and `Series (Year) SxxExx Episode Title` files. Preserve supported extensions: video `mp4`, `mkv`, `webm`, `avi`, `mov`, `m4v`; subtitles `srt`, `vtt`, `ass`, `ssa`, `sub`.

- [ ] **Step 3: Add deterministic collision behavior**

If the completed target already exists, return `LibraryError::AlreadyExists` with the existing relative path. Never append `_2` to a completed library item because that creates ambiguous Jellyfin versions. Partial files use the job UUID and remain outside the final library folder until finalization.

- [ ] **Step 4: Run naming tests**

```bash
cargo test --features server --test library_naming
```

Expected: movie, episode, language suffix, Unicode normalization, reserved-name, long-title and traversal cases pass.

- [ ] **Step 5: Commit library naming**

```bash
git add src/server/library tests/library_naming.rs
git commit -m "feat(server): add jellyfin-compatible media naming"
```

## Phase 3 — Persist and execute jobs

### Task 5: Create SQLite schema and transactional job repository

**Files:**
- Create: `migrations/0001_server.sql`
- Create: `src/server/db.rs`
- Create: `src/server/jobs/mod.rs`
- Create: `src/server/jobs/model.rs`
- Create: `src/server/jobs/repository.rs`
- Test: `tests/job_repository.rs`

**Interfaces:**
- Produces: `JobId(Uuid)`, `JobState`, `DownloadJob`, `NewJob`, `JobRepository`.
- Produces: compare-and-set state transitions and durable progress updates.

- [ ] **Step 1: Write failing migration and transition tests**

```rust
#[tokio::test]
async fn transition_rejects_invalid_state_change() {
    let repo = test_repository().await;
    let job = repo.create(new_job()).await.unwrap();
    let error = repo.transition(job.id, JobState::Ready, None).await.unwrap_err();
    assert!(matches!(error, JobRepositoryError::InvalidTransition { from: JobState::Queued, to: JobState::Ready }));
}
```

Run:

```bash
cargo test --features server --test job_repository
```

Expected: FAIL because the schema and repository do not exist.

- [ ] **Step 2: Add the schema**

Create `users`, `sessions`, `jobs`, and `job_events`. `jobs` stores media identity, season/episode, selected source/subtitle IDs, requested height, state, relative final/partial paths, downloaded/total bytes, speed, attempt, safe error code/message, warning, created/updated timestamps, and version for optimistic concurrency. Add indexes on `(state, created_at)` and session expiry.

- [ ] **Step 3: Define legal transitions**

```rust
pub fn can_transition(from: JobState, to: JobState) -> bool {
    matches!((from, to),
        (JobState::Queued, JobState::Resolving)
        | (JobState::Queued, JobState::Cancelled)
        | (JobState::Resolving, JobState::Downloading)
        | (JobState::Resolving, JobState::Failed)
        | (JobState::Downloading, JobState::Paused)
        | (JobState::Downloading, JobState::Finalizing)
        | (JobState::Downloading, JobState::Failed)
        | (JobState::Downloading, JobState::Cancelled)
        | (JobState::Paused, JobState::Queued)
        | (JobState::Paused, JobState::Cancelled)
        | (JobState::Finalizing, JobState::Ready)
        | (JobState::Finalizing, JobState::Failed)
        | (JobState::Failed, JobState::Queued)
    )
}
```

- [ ] **Step 4: Implement transactional repository methods**

```rust
pub async fn create(&self, input: NewJob) -> Result<DownloadJob, JobRepositoryError>;
pub async fn claim_next(&self) -> Result<Option<DownloadJob>, JobRepositoryError>;
pub async fn transition(&self, id: JobId, to: JobState, event: Option<JobEvent>) -> Result<DownloadJob, JobRepositoryError>;
pub async fn update_progress(&self, id: JobId, progress: JobProgress) -> Result<(), JobRepositoryError>;
pub async fn list(&self, limit: u32, before: Option<time::OffsetDateTime>) -> Result<Vec<DownloadJob>, JobRepositoryError>;
```

- [ ] **Step 5: Run repository tests and inspect migration**

```bash
cargo test --features server --test job_repository
cargo sqlx migrate info --source migrations
```

Expected: migrations apply to a temporary database; concurrent claim test gives a job to only one worker.

- [ ] **Step 6: Commit persistence**

```bash
git add migrations src/server/db.rs src/server/jobs tests/job_repository.rs
git commit -m "feat(server): persist download jobs in sqlite"
```

### Task 6: Implement restart recovery and the single-worker lifecycle

**Files:**
- Create: `src/server/jobs/recovery.rs`
- Create: `src/server/jobs/worker.rs`
- Create: `src/server/events.rs`
- Modify: `src/server/jobs/mod.rs`
- Test: `tests/job_worker.rs`

**Interfaces:**
- Consumes: `CatalogProvider`, `JobRepository`, `LibraryNamer`, `DownloadRequest`.
- Produces: `JobWorker::run(CancellationToken)` and `recover_interrupted_jobs`.
- Produces: `JobEventBus::subscribe() -> broadcast::Receiver<JobEvent>`.

- [ ] **Step 1: Write failing restart and successful-job tests**

```rust
#[tokio::test]
async fn startup_requeues_interrupted_job_and_preserves_partial_file() {
    let harness = WorkerHarness::with_state(JobState::Downloading).await;
    harness.write_partial(b"fixture-prefix").await;
    recover_interrupted_jobs(&harness.repo, &harness.media_root).await.unwrap();
    assert_eq!(harness.job().await.state, JobState::Queued);
    assert!(harness.partial_path().exists());
}
```

Run:

```bash
cargo test --features server --test job_worker
```

Expected: FAIL because recovery and worker do not exist.

- [ ] **Step 2: Implement startup recovery**

Requeue `resolving`, `downloading`, and `finalizing`. Preserve valid partials beneath the partial root. If a final file exists and matches recorded size, mark ready; otherwise requeue. Mark records with invalid/outside paths as failed with `unsafe_path`.

- [ ] **Step 3: Implement disk reserve check**

Before resolving a source, read free bytes for the media filesystem. If `free_bytes <= reserve_bytes + expected_size`, keep the job queued and record `insufficient_space`; emit an event no more than once every five minutes.

- [ ] **Step 4: Implement worker state flow**

Claim one queued job, resolve its opaque source ID, revalidate requested height, create the secure transfer request, download video and selected subtitle, atomically rename into the library, persist ready state, and emit sanitized events. On an expired URL, resolve once more and retry; do not loop indefinitely.

- [ ] **Step 5: Implement pause, cancel and retry signals**

Maintain one `CancellationToken` per active job. Pause preserves partials; cancel removes only the job's UUID-scoped partial files after containment validation; retry changes failed to queued and clears safe error fields.

- [ ] **Step 6: Run worker tests**

```bash
cargo test --features server --test job_worker
```

Expected: complete, resume, pause, cancel, URL-refresh, subtitle-warning, insufficient-space, restart and duplicate-target cases pass.

- [ ] **Step 7: Commit the worker**

```bash
git add src/server/jobs src/server/events.rs tests/job_worker.rs
git commit -m "feat(server): run restart-safe download queue"
```

## Phase 4 — Authenticated API

### Task 7: Add validated configuration, authentication and safe API errors

**Files:**
- Create: `src/server/config.rs`
- Create: `src/server/auth.rs`
- Create: `src/server/error.rs`
- Create: `src/server/state.rs`
- Create: `src/server/routes/mod.rs`
- Create: `src/server/routes/health.rs`
- Create: `src/server/routes/auth.rs`
- Modify: `src/server/mod.rs`
- Test: `tests/server_auth.rs`

**Interfaces:**
- Produces: `ServerConfig::from_env`, `AppState`, `ApiError`, session middleware.
- Produces: `/api/health`, `/api/auth/login`, `/api/auth/logout`, `/api/auth/session`.

- [ ] **Step 1: Write failing bootstrap/login/cookie tests**

```rust
#[tokio::test]
async fn successful_login_sets_strict_secure_cookie() {
    let app = test_app().await;
    let response = post_json(&app, "/api/auth/login", json!({"password":"fixture-secret"})).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = response.headers()[SET_COOKIE].to_str().unwrap();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("Secure"));
    assert!(cookie.contains("SameSite=Strict"));
}
```

Run:

```bash
cargo test --features server --test server_auth
```

Expected: FAIL because routes and sessions do not exist.

- [ ] **Step 2: Validate exact configuration**

Require a valid bind address, database URL, media/partial/config paths, admin password file, session key file, 1080 maximum, 10 GiB reserve, concurrency 1, Jellyfin base URL, optional Jellyfin API key file, and log format. The server may bind `0.0.0.0:8420` inside its container; private exposure is enforced by Compose publishing the host port only as `127.0.0.1:8420`. Reject `/mnt/mac-remote`, root `/`, world-writable secret files, and maximum height above 1080.

- [ ] **Step 3: Implement first-run admin bootstrap**

Read the password from the mounted secret file, hash with Argon2id using a random salt, insert one admin user if none exists, and never log or return the password. Subsequent starts ignore the password file when the user already exists.

- [ ] **Step 4: Implement server-side sessions and CSRF**

Store a hash of each random 256-bit session token in SQLite. Use 30-minute idle and seven-day absolute expiry. Require `Origin` match and `X-CSRF-Token` for state-changing API calls. Apply an in-memory token bucket of five login failures per source per ten minutes.

- [ ] **Step 5: Implement the stable error envelope**

```json
{
  "error": {
    "code": "quality_unavailable",
    "message": "No source is available at or below 1080p.",
    "request_id": "018f...",
    "fields": {}
  }
}
```

Log detailed causes with sanitized URLs while returning only safe codes/messages.

- [ ] **Step 6: Run auth and configuration tests**

```bash
cargo test --features server --test server_auth
```

Expected: login, logout, expiry, CSRF, throttling, bad secret permissions and invalid bind tests pass.

- [ ] **Step 7: Commit server foundations**

```bash
git add src/server/config.rs src/server/auth.rs src/server/error.rs src/server/state.rs src/server/routes src/server/mod.rs tests/server_auth.rs
git commit -m "feat(server): add private authenticated api foundation"
```

### Task 8: Expose catalog, job and event routes

**Files:**
- Create: `src/server/routes/catalog.rs`
- Create: `src/server/routes/jobs.rs`
- Create: `src/server/routes/events.rs`
- Modify: `src/server/routes/mod.rs`
- Modify: `src/server/state.rs`
- Test: `tests/server_api.rs`

**Interfaces:**
- Consumes: typed catalog facade, job repository and event bus.
- Produces: the API paths in the approved design.

- [ ] **Step 1: Write failing API contract tests**

Test unauthenticated rejection, trimmed query length 2–100, page bounds 1–50, details lookup, source filtering, forged 2160p job rejection, season expansion, job actions, pagination, and SSE replay from `Last-Event-ID`.

```rust
#[tokio::test]
async fn forged_4k_job_is_rejected_by_server() {
    let response = authenticated_post("/api/jobs", json!({
        "catalog_id":"fixture",
        "source_id":"signed-2160-source",
        "requested_height":2160
    })).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(response).await, "quality_exceeds_limit");
}
```

- [ ] **Step 2: Implement catalog routes**

Return typed DTOs and cache non-sensitive search/details responses by provider/query for five minutes. Never cache resolved URLs. Include `Cache-Control: private, no-store` on authenticated responses.

- [ ] **Step 3: Implement job creation**

Verify the opaque source signature, catalog identity, season/episode, selected subtitle, and height. For a season request, create one parent group and ordered child jobs in one transaction. Return HTTP 202 with job IDs.

- [ ] **Step 4: Implement job action routes**

Pause, resume, cancel and retry call repository compare-and-set transitions. Return HTTP 409 for illegal transitions and HTTP 404 for unknown IDs.

- [ ] **Step 5: Implement SSE**

Send named `job.updated` events with monotonically increasing database event IDs, heartbeat comments every 15 seconds, and a bounded replay query after reconnect. Do not include provider URLs or local absolute paths.

- [ ] **Step 6: Run API contract tests**

```bash
cargo test --features server --test server_api
```

Expected: all route, auth, forgery, transition and SSE tests pass.

- [ ] **Step 7: Commit the API**

```bash
git add src/server/routes src/server/state.rs tests/server_api.rs
git commit -m "feat(server): expose catalog and download queue api"
```

## Phase 5 — Browser control interface

### Task 9: Build the React shell, authentication and typed client

**Files:**
- Create: `web/package.json`
- Create: `web/package-lock.json`
- Create: `web/tsconfig.json`
- Create: `web/vite.config.ts`
- Create: `web/src/main.tsx`
- Create: `web/src/App.tsx`
- Create: `web/src/api/client.ts`
- Create: `web/src/api/types.ts`
- Create: `web/src/features/auth/LoginPage.tsx`
- Create: `web/src/styles/tokens.css`
- Create: `web/src/styles/global.css`
- Create: `web/tests/LoginPage.test.tsx`
- Create: `src/server/routes/assets.rs`
- Modify: `src/server/routes/mod.rs`
- Test: `tests/server_assets.rs`

**Interfaces:**
- Produces: `api.request<T>()`, authenticated app shell and CSRF handling.
- Consumes: auth/session routes from Task 7.

- [ ] **Step 1: Initialize pinned frontend tooling**

Use React, TypeScript and Vite with scripts `dev`, `build`, `typecheck`, `test`, `test:run`, and `e2e`. Commit the lockfile; do not use floating CDN scripts.

- [ ] **Step 2: Write the failing login component test**

```tsx
it("submits the password and enters the authenticated shell", async () => {
  server.use(loginSucceeds(), sessionIsAuthenticated());
  render(<App />);
  await userEvent.type(screen.getByLabelText(/password/i), "fixture-secret");
  await userEvent.click(screen.getByRole("button", { name: /sign in/i }));
  expect(await screen.findByRole("search")).toBeVisible();
});
```

Run:

```bash
npm --prefix web test -- --run LoginPage
```

Expected: FAIL because the app shell does not exist.

- [ ] **Step 3: Implement the client and session flow**

Use `credentials: "same-origin"`, read CSRF from session response, attach it only to mutations, parse the stable error envelope, and redirect to login on 401. Never persist the password or session token in localStorage.

- [ ] **Step 4: Implement visual tokens and responsive shell**

Use near-black surfaces, warm off-white text, one red accent, 8/12/16/24/32 spacing, 12px card radius, visible keyboard focus, reduced-motion support, 44px minimum controls, and layouts tested at 375px and 1440px widths.

- [ ] **Step 5: Run frontend tests**

Before running the checks, embed `web/dist` with `rust-embed` and add a route fallback that serves `index.html` for non-API browser routes. Hashed assets receive `Cache-Control: public, max-age=31536000, immutable`; `index.html` receives `Cache-Control: no-store`. Add `Content-Security-Policy`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, and `frame-ancestors 'none'`. `tests/server_assets.rs` must prove `/api/*` never falls through to the SPA.

```bash
npm --prefix web run typecheck
npm --prefix web run test:run
npm --prefix web run build
cargo test --features server --test server_assets
```

Expected: typecheck, login tests and production build pass.

- [ ] **Step 6: Commit the frontend foundation**

```bash
git add web src/server/routes/assets.rs src/server/routes/mod.rs tests/server_assets.rs
git commit -m "feat(web): add authenticated moviebox server shell"
```

### Task 10: Build search, details and download selection

**Files:**
- Create: `web/src/features/search/SearchPage.tsx`
- Create: `web/src/features/search/ResultCard.tsx`
- Create: `web/src/features/details/DetailsDrawer.tsx`
- Create: `web/src/features/details/SourcePicker.tsx`
- Create: `web/src/features/details/SubtitlePicker.tsx`
- Create: `web/tests/SearchDownload.test.tsx`

**Interfaces:**
- Consumes: catalog and job-creation routes.
- Produces: explicit movie, episode and season download requests.

- [ ] **Step 1: Write failing interaction tests**

Cover debounced search, empty/no-result/error states, poster fallback, season/episode selection, 1080p recommendation, lower-quality selection, subtitle “None”, and confirmation before a season queue is created.

- [ ] **Step 2: Implement search**

Submit after 300ms debounce only when the normalized query has at least two characters. Cancel stale requests with `AbortController`. Show title, year, type and poster; do not render provider HTML.

- [ ] **Step 3: Implement details and source selection**

Load details only after selection. Default to the recommended source at or below 1080p and the first preferred subtitle language when available. Display exact quality and reported size; never offer 2160p.

- [ ] **Step 4: Implement job creation confirmation**

The confirmation states movie/episode/season, quality, subtitle and estimated size. Disable duplicate submission while the POST is in flight and route successful requests to the queue.

- [ ] **Step 5: Run frontend checks**

```bash
npm --prefix web run typecheck
npm --prefix web run test:run
npm --prefix web run build
```

- [ ] **Step 6: Commit search and selection**

```bash
git add web/src/features web/tests/SearchDownload.test.tsx
git commit -m "feat(web): search and queue 1080p downloads"
```

### Task 11: Build live queue controls and Jellyfin-ready state

**Files:**
- Create: `web/src/api/events.ts`
- Create: `web/src/features/queue/QueuePage.tsx`
- Create: `web/src/features/queue/JobCard.tsx`
- Create: `web/src/features/library/ReadyPage.tsx`
- Create: `web/tests/QueuePage.test.tsx`
- Create: `web/tests/ReadyPage.test.tsx`

**Interfaces:**
- Consumes: job list/action routes, SSE and library deep-link route.
- Produces: reconnecting live progress UI and explicit job controls.

- [ ] **Step 1: Write failing queue-state tests**

Test queued, resolving, downloading, paused, finalizing, ready and failed cards; byte/speed/ETA formatting; pause/resume/cancel/retry confirmation; SSE reconnect; subtitle warning; and “Open in Jellyfin”.

- [ ] **Step 2: Implement SSE reconciliation**

Treat SSE as an update hint, merge events by job version, and refetch the list after reconnect. Exponential reconnect delays are 1, 2, 4, 8 and 15 seconds, capped at 15 seconds.

- [ ] **Step 3: Implement queue controls**

Show only legal actions for each state. Require confirmation before cancel. Preserve failed-job diagnostics as safe human text and a copyable request ID.

- [ ] **Step 4: Implement ready view**

Show title, year, episode identity, subtitle warning and completion time. Fetch the deep link when clicked; if Jellyfin is degraded, show that media is safe on disk and offer retry scan.

- [ ] **Step 5: Run frontend checks**

```bash
npm --prefix web run typecheck
npm --prefix web run test:run
npm --prefix web run build
```

- [ ] **Step 6: Commit queue and ready views**

```bash
git add web/src/api/events.ts web/src/features/queue web/src/features/library web/tests
git commit -m "feat(web): manage downloads and open ready media"
```

## Phase 6 — Jellyfin and deployable containers

### Task 12: Add Jellyfin scan/deep-link integration

**Files:**
- Create: `src/server/library/jellyfin.rs`
- Create: `src/server/routes/library.rs`
- Modify: `src/server/routes/mod.rs`
- Modify: `src/server/config.rs`
- Test: `tests/jellyfin_client.rs`

**Interfaces:**
- Produces: `JellyfinClient::refresh_library`, `find_item`, and `deep_link`.
- Produces: `/api/library/:job_id` response `{ status, url }` without exposing the API key.

- [ ] **Step 1: Write failing mock Jellyfin tests**

Test authenticated refresh, item polling with bounded attempts, unavailable server, invalid JSON, and API key redaction from errors/log snapshots.

- [ ] **Step 2: Implement the client**

Use a dedicated Reqwest client with a five-second timeout. The base URL is fixed by configuration and must resolve to loopback or the Compose service name. Read the API key from a mounted secret file and attach it only to Jellyfin requests.

- [ ] **Step 3: Implement scan and deep-link behavior**

After ready state, request a library refresh when a key exists, then poll item lookup for up to 60 seconds at five-second intervals. If no key exists, report `scan_pending` and rely on Jellyfin's real-time watcher. Never fail or remove completed media because Jellyfin is unavailable.

- [ ] **Step 4: Run Jellyfin tests**

```bash
cargo test --features server --test jellyfin_client
```

- [ ] **Step 5: Commit Jellyfin integration**

```bash
git add src/server/library/jellyfin.rs src/server/routes/library.rs src/server/routes/mod.rs src/server/config.rs tests/jellyfin_client.rs
git commit -m "feat(server): refresh and deep-link jellyfin library"
```

### Task 13: Build hardened images and Compose deployment

**Files:**
- Create: `docker/server.Dockerfile`
- Create: `docker/entrypoint.sh`
- Create: `deploy/compose/compose.yml`
- Create: `deploy/compose/.env.example`
- Create: `deploy/compose/secrets/README.md`
- Create: `.dockerignore`
- Test: `tests/compose.bats`

**Interfaces:**
- Produces: localhost ports `8420` and `8096`.
- Produces: bind mounts beneath `${MOVIEBOX_DATA_ROOT}` and the official Jellyfin container.

- [ ] **Step 1: Write failing Compose policy tests**

The test must parse rendered Compose config and assert:

```bash
docker compose -f deploy/compose/compose.yml --env-file deploy/compose/.env.example config
```

- Both published ports begin with `127.0.0.1:`.
- Server is not privileged and has `cap_drop: [ALL]`.
- Jellyfin media mount is read-only.
- `/dev/dri/renderD128` is the only GPU device passed.
- No mount contains `/mnt/mac-remote`.
- Both services have health checks and `restart: unless-stopped`.

- [ ] **Step 2: Build a multi-stage server image**

Stage 1 builds `web/dist` with the committed npm lockfile. Stage 2 builds `moviebox-server --release --locked --features server`. Runtime contains the binary, CA certificates and a non-root UID/GID selected through build args; it does not contain Cargo, Node or source code.

- [ ] **Step 3: Define persistent paths**

```text
${MOVIEBOX_DATA_ROOT}/config/server     -> /config
${MOVIEBOX_DATA_ROOT}/partials          -> /partials
${MOVIEBOX_DATA_ROOT}/media             -> /media
${MOVIEBOX_DATA_ROOT}/config/jellyfin   -> /config inside Jellyfin
${MOVIEBOX_DATA_ROOT}/cache/jellyfin    -> /cache inside Jellyfin
```

MovieBox Server mounts `/media` read-write; Jellyfin mounts it read-only.

- [ ] **Step 4: Define exact environment defaults**

```dotenv
MOVIEBOX_DATA_ROOT=/srv/moviebox
MOVIEBOX_BIND=0.0.0.0:8420
MOVIEBOX_MAX_HEIGHT=1080
MOVIEBOX_DOWNLOAD_CONCURRENCY=1
MOVIEBOX_RESERVE_GIB=10
MOVIEBOX_UID=1000
MOVIEBOX_GID=1000
JELLYFIN_PUBLISHED_PORT=8096
```

The container listens on its bridge interface; host publishing remains loopback-only.

- [ ] **Step 5: Add secrets workflow**

Document exact commands:

```bash
umask 077
mkdir -p deploy/compose/secrets
openssl rand -base64 32 > deploy/compose/secrets/admin_password.txt
openssl rand -base64 48 > deploy/compose/secrets/session_key.txt
```

Secrets are gitignored, mounted read-only, and never supplied as environment variables.

- [ ] **Step 6: Validate and smoke-test images**

```bash
docker compose -f deploy/compose/compose.yml --env-file deploy/compose/.env.example config --quiet
docker build -f docker/server.Dockerfile -t moviebox-server:test .
bats tests/compose.bats
```

Expected: policy tests and image health check pass.

- [ ] **Step 7: Commit deployment assets**

```bash
git add docker deploy/compose .dockerignore tests/compose.bats
git commit -m "feat(deploy): add private moviebox and jellyfin compose stack"
```

### Task 14: Configure private Tailscale Serve without touching existing public routes

**Files:**
- Create: `deploy/tailscale/moviebox-serve.sh`
- Create: `docs/server/private-access.md`
- Test: `tests/tailscale_serve.bats`

**Interfaces:**
- Produces: private HTTPS controller on 443 and Jellyfin on 8443 for the homeserver tailnet name.
- Consumes: loopback ports 8420 and 8096.

- [ ] **Step 1: Write a dry-run/idempotency test**

The script supports `--check` and prints intended changes without invoking Tailscale. The test stubs `tailscale serve status --json` and proves it refuses to replace unrelated existing Serve handlers.

- [ ] **Step 2: Implement guarded configuration**

Use current CLI syntax:

```bash
tailscale serve --bg --https=443 http://127.0.0.1:8420
tailscale serve --bg --https=8443 http://127.0.0.1:8096
```

Before applying, capture `tailscale serve get-config`, verify no conflicting handler exists, and save the prior JSON under the deployment config directory. Never invoke `tailscale funnel`.

- [ ] **Step 3: Document privacy verification**

```bash
tailscale serve status
ss -ltn | grep -E ':(8420|8096) '
curl -fsS http://127.0.0.1:8420/api/health
```

Expected: app ports listen only on `127.0.0.1`; Serve reports tailnet-only HTTPS endpoints.

- [ ] **Step 4: Run script tests**

```bash
bats tests/tailscale_serve.bats
shellcheck deploy/tailscale/moviebox-serve.sh
```

- [ ] **Step 5: Commit private access tooling**

```bash
git add deploy/tailscale docs/server/private-access.md tests/tailscale_serve.bats
git commit -m "feat(deploy): publish services privately with tailscale serve"
```

## Phase 7 — Full verification, documentation and homeserver rollout

### Task 15: Add end-to-end tests and CI gates

**Files:**
- Create: `web/playwright.config.ts`
- Create: `web/e2e/private-download.spec.ts`
- Create: `tests/fixtures/sample-video.mp4`
- Create: `tests/fixtures/sample.en.srt`
- Create: `tests/e2e/fake_provider.rs`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: deterministic login-to-ready browser test using local fixtures.
- Preserves: existing three-platform TUI build/test matrix.

- [ ] **Step 1: Generate tiny legal fixtures**

Generate a two-second color-bar MP4 and subtitle with FFmpeg in a documented script or commit a CC0 fixture whose source and license are recorded. Keep total fixture size below 1 MiB.

- [ ] **Step 2: Write the failing Playwright flow**

```ts
test("login, queue a 1080p fixture, observe completion, and open Jellyfin", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("Password").fill("fixture-secret");
  await page.getByRole("button", { name: "Sign in" }).click();
  await page.getByRole("searchbox").fill("Fixture Movie");
  await page.getByText("Fixture Movie (2024)").click();
  await page.getByLabel("1080p").check();
  await page.getByRole("button", { name: "Download" }).click();
  await expect(page.getByText("Ready")).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole("link", { name: "Open in Jellyfin" })).toBeVisible();
});
```

- [ ] **Step 3: Add CI jobs**

Keep current Rust 1.90 platform checks. Add Linux all-features tests, frontend typecheck/test/build, Playwright with fake provider, Compose config policy, `cargo audit`, `npm audit --audit-level=high`, ShellCheck, and image build. No CI job calls live MovieBox providers.

- [ ] **Step 4: Run CI parity locally**

```bash
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
npm --prefix web ci
npm --prefix web run typecheck
npm --prefix web run test:run
npm --prefix web run build
npm --prefix web run e2e
docker compose -f deploy/compose/compose.yml --env-file deploy/compose/.env.example config --quiet
bats tests/compose.bats tests/tailscale_serve.bats
```

Expected: every command passes without live provider access.

- [ ] **Step 5: Commit verification**

```bash
git add web/playwright.config.ts web/e2e tests/fixtures tests/e2e .github/workflows/ci.yml
git commit -m "test: verify self-hosted download and playback flow"
```

### Task 16: Write the fork README and operations documentation

**Files:**
- Modify: `README.md`
- Modify: `docs/README.md`
- Create: `docs/server/architecture.md`
- Create: `docs/server/install.md`
- Create: `docs/server/storage.md`
- Create: `docs/server/security.md`
- Create: `docs/server/backup-restore.md`
- Create: `docs/server/troubleshooting.md`
- Create: `docs/server/update-uninstall.md`
- Create: `docs/server/attribution.md`

**Interfaces:**
- Produces: complete self-host instructions and honest fork attribution.

- [ ] **Step 1: Write README contract tests**

Add a test that requires README links/commands for Docker prerequisites, secret creation, Compose startup, Jellyfin first-run setup, Tailscale private access, LAN alternative, 1080p limit, storage location, update, backup, uninstall, troubleshooting, upstream attribution, licensing and content-rights disclaimer.

- [ ] **Step 2: Rewrite the README opening**

Identify this as an independently maintained self-hosted fork of `mesamirh/MovieBox-Tui`, not an official upstream edition. Keep the original TUI install path and add a “Self-host MovieBox Server” path. Include architecture diagram and screenshots captured from synthetic data.

- [ ] **Step 3: Document exact quick start**

```bash
git clone https://github.com/M-Nabeegh/MovieBox-Tui.git
cd MovieBox-Tui/deploy/compose
cp .env.example .env
umask 077
mkdir -p secrets
openssl rand -base64 32 > secrets/admin_password.txt
openssl rand -base64 48 > secrets/session_key.txt
docker compose pull
docker compose up -d
docker compose ps
```

State that users must edit `MOVIEBOX_DATA_ROOT`, ownership and Tailscale routes for their own host. The personal homeserver deployment overrides it with `/mnt/nas-data/moviebox`.

- [ ] **Step 4: Document backup and removal precisely**

Back up SQLite, server config and Jellyfin config while services are stopped; media files can be backed up independently. Uninstall stops containers and removes only deployment-created containers/networks. Data deletion is a separate explicit command and is never part of normal uninstall.

- [ ] **Step 5: Run docs checks**

```bash
cargo test --features server --test readme_contract
npx --yes markdownlint-cli2 README.md 'docs/**/*.md'
```

- [ ] **Step 6: Commit documentation**

```bash
git add README.md docs tests/readme_contract.rs
git commit -m "docs: publish self-hosted moviebox server guide"
```

### Task 17: Deploy safely to the verified Linux homeserver

**Files on server:**
- Create after approval: `/opt/moviebox-server` checkout or deployment copy
- Create after local sudo: `/mnt/nas-data/moviebox/{config,cache,partials,media}`
- Preserve: `/mnt/mac-remote`, existing n8n, Home Assistant, OpenClaw, homepage and finance containers

**Interfaces:**
- Produces: private control and Jellyfin URLs on the homeserver tailnet.
- Requires: all Task 15 checks green and explicit user approval for server writes.

- [ ] **Step 1: Reverify the exact host and resources read-only**

```bash
ssh -o BatchMode=yes homeserver 'hostname; id -un; date -Is; docker --version; docker compose version; df -hT / /mnt/nas-data /mnt/mac-remote; ethtool eno1 | grep -E "Speed:|Duplex:|Link detected:"'
```

Expected: `homeserver`, user `nabeegh`, `/mnt/nas-data` mounted with sufficient free space, and `/mnt/mac-remote` remains a separate mount.

- [ ] **Step 2: Capture the existing container and Serve baseline**

```bash
ssh homeserver 'docker ps --format "{{.Names}} {{.Status}} {{.Ports}}"; tailscale serve status; ss -ltn'
```

Save output in the deployment report. Abort on port 8420, 8096, 443 or 8443 conflicts that are not accounted for by existing Serve configuration.

- [ ] **Step 3: Create storage with a guarded user-local sudo command**

The user enters sudo locally; no password is sent in chat.

```bash
ssh -tt homeserver 'sudo install -d -o 1000 -g 1000 -m 0750 /mnt/nas-data/moviebox /mnt/nas-data/moviebox/config/server /mnt/nas-data/moviebox/config/jellyfin /mnt/nas-data/moviebox/cache/jellyfin /mnt/nas-data/moviebox/partials /mnt/nas-data/moviebox/media'
```

Immediately verify ownership and mount identity after creation.

- [ ] **Step 4: Install the reviewed release and secrets**

Use a tagged release or commit SHA that passed CI. Generate secrets directly on the server with `umask 077`; never copy them into Git or the task transcript.

- [ ] **Step 5: Start Compose and inspect health**

```bash
docker compose pull
docker compose up -d
docker compose ps
docker compose logs --tail=120 moviebox-server jellyfin
curl -fsS http://127.0.0.1:8420/api/health
curl -fsS http://127.0.0.1:8096/health
```

Expected: both services healthy; all pre-existing containers remain in their previous state.

- [ ] **Step 6: Configure Tailscale Serve with the guarded script**

Run `deploy/tailscale/moviebox-serve.sh --check`, review its exact diff, then apply. Verify `tailscale serve status` says the routes are tailnet-only and no Funnel route exists.

- [ ] **Step 7: Complete Jellyfin first-run setup**

Create the Jellyfin administrator, add `/media/Movies` as Movies and `/media/Shows` as Shows, disable remote internet exposure, enable real-time monitoring, configure preferred subtitle language, and enable VA-API on `/dev/dri/renderD128` only for codecs reported by Jellyfin's bundled `vainfo`.

- [ ] **Step 8: Prove hardware and playback with synthetic media**

```bash
docker exec jellyfin /usr/lib/jellyfin-ffmpeg/vainfo
docker logs --since 10m jellyfin | grep -Ei 'vaapi|transcod|error'
```

Verify direct play in the Mac Jellyfin client, external subtitle selection, and one forced 1080p H.264 VA-API transcode. Do not claim hardware acceleration until the Jellyfin playback dashboard and logs both confirm it.

- [ ] **Step 9: Prove queue recovery**

Start a generated fixture download, restart only `moviebox-server`, confirm the job returns to queued/downloading and resumes its `.part` data, then reaches ready and appears in Jellyfin.

- [ ] **Step 10: Verify network exposure and existing workloads**

```bash
ssh homeserver 'ss -ltn | grep -E ":(8420|8096) "; docker ps --format "{{.Names}} {{.Status}}"; systemctl --failed --no-pager'
```

Expected: 8420 and 8096 bind only to loopback; existing 11 containers remain running/healthy as applicable; no failed units.

- [ ] **Step 11: Record rollout and rollback commands**

Rollback stops only the two new containers, restores the prior Tailscale Serve JSON, and leaves `/mnt/nas-data/moviebox` intact. Data removal is excluded unless separately approved.

- [ ] **Step 12: Commit only repository-side deployment notes**

```bash
git add docs/server
git commit -m "docs: record verified homeserver deployment"
```

## Final release gate

- [ ] Existing TUI builds and tests pass on Ubuntu, macOS and Windows.
- [ ] All server, frontend, security, Compose and browser tests pass without live provider access.
- [ ] Live smoke test searches and queues only an explicitly authorized title or synthetic fixture.
- [ ] Browser never receives direct stream URLs, subtitle URLs, API keys or absolute paths.
- [ ] Forged 2160p and arbitrary-URL requests are rejected server-side.
- [ ] Restart resumes partial downloads and preserves valid completed media.
- [ ] Jellyfin recognizes movie/show names and external subtitles.
- [ ] VA-API is described as verified only if playback logs confirm it.
- [ ] Host ports 8420 and 8096 are loopback-only; Tailscale Serve is private; Funnel is absent.
- [ ] `/mnt/mac-remote` is unchanged and no media path resolves onto it.
- [ ] Existing homeserver containers and systemd services remain healthy.
- [ ] README attribution, dual license and content-rights disclaimer are present.
- [ ] A tagged image/release records exact Git commit, Cargo lockfile and npm lockfile.

## Recommended execution order

Execute Tasks 1–8 first and review the API using fixture data. Execute Tasks 9–11 next and review responsive UI screenshots at 375px and 1440px. Execute Tasks 12–16 only after the backend/UI contract is stable. Task 17 is a separate, approval-gated deployment session because it writes to the Linux server and changes Tailscale Serve state.
