# MovieBox Server Design

**Status:** Approved for planning on 2026-08-14.

## Product decision

Build a self-hosted server edition inside `M-Nabeegh/MovieBox-Tui` while preserving the existing terminal application. The fork will add a separate `moviebox-server` binary, a browser control interface, and a Docker Compose deployment that includes Jellyfin. MovieBox Server owns catalog search, source selection, subtitles, persistent download jobs, and media-library placement. Jellyfin owns metadata enrichment, watch history, audio/subtitle controls, and playback.

The personal deployment is private: both HTTP services bind to host loopback and Tailscale Serve publishes them only to the user's tailnet. Tailscale Funnel and public router port-forwarding are out of scope.

## Approaches considered

1. **Rust server companion plus Jellyfin — selected.** Reuses the existing provider and resumable-download code, keeps playback in a mature media server, and remains practical on the current Linux hardware.
2. **Single custom Netflix-style application.** Would require a complete media scanner, metadata service, transcoder, watch-state engine, and cross-browser player. This duplicates Jellyfin and substantially increases correctness and security risk.
3. **Terminal container plus shared download folder.** Fastest to assemble but does not satisfy browser search, persistent queue management, or polished playback requirements.

## Goals

- Search movies and series from a private browser page.
- Show typed details, seasons, episodes, available qualities, sizes, languages, and subtitles.
- Enforce a hard maximum resolution of 1080p on the server, even if a client submits a forged request.
- Queue movie, episode, and season downloads on the Linux server.
- Persist queue state and safely resume partial downloads after process or host restarts.
- Store completed media using Jellyfin-compatible names and subtitle suffixes.
- Open completed titles in Jellyfin for Netflix-like browsing and playback.
- Support one-command Docker Compose installation for other self-hosters.
- Preserve upstream attribution and the existing MIT OR Apache-2.0 license.

## Non-goals for version 1

- Public internet exposure, Tailscale Funnel, or router port-forwarding.
- 4K downloads or 4K transcoding.
- Multiple application accounts, invitations, or social features.
- IPTV/M3U ingestion, BDIX providers, or arbitrary user-supplied download URLs.
- Reimplementing Jellyfin's media scanner, metadata matching, transcoder, or player.
- Automatically downloading content without an explicit user selection.

## Target server

- Ubuntu 24.04.4 LTS, x86-64.
- Intel Core i5-4590, four cores, 15 GiB RAM.
- Docker 29.7.1 and Docker Compose v5.3.1.
- Media root: `/mnt/nas-data/moviebox`; the Time Machine disk at `/mnt/mac-remote` is excluded.
- Intel i915 render device: `/dev/dri/renderD128`.
- Physical Ethernet currently negotiates at 100 Mb/s. Version 1 targets one 1080p stream and one download worker by default.

## Repository structure

The repository remains one Rust package so upstream provider changes can still be merged with limited conflict.

```text
src/
  main.rs                         existing TUI binary
  bin/moviebox-server.rs          server entrypoint
  catalog/                        typed catalog and source-selection facade
  download.rs                     reusable byte-transfer engine
  server/
    auth.rs                       bootstrap password and session cookies
    config.rs                     validated environment configuration
    db.rs                         SQLite pool and migrations
    error.rs                      stable API error envelope
    events.rs                     server-sent progress events
    jobs/                         queue model, repository, worker, recovery
    library/                      Jellyfin naming and scan integration
    routes/                       health, auth, catalog, jobs, library
    security/                     URL validation, redirects, path containment
    state.rs                      shared application services
web/
  src/                            React/TypeScript control interface
  tests/                          component and browser tests
migrations/                       SQLite schema
docker/                           app image and entrypoint
deploy/compose/                   Compose, environment example, health checks
tests/                            Rust integration tests and HTTP fixtures
```

## Components and boundaries

### Catalog facade

`CatalogService` wraps provider-specific JSON and returns stable typed models. The browser never receives direct stream or subtitle URLs. It receives opaque source and subtitle identifiers. Only the server resolves those identifiers when creating a job.

MovieBox is the only enabled provider in version 1. Existing 4KHDHub, BDIX, and M3U code stays available to the TUI but is not exposed by the server.

### Resolution policy

`QualityPolicy { maximum_height: 1080 }` filters source options and validates job creation. Sources above 1080p are never returned by the normal API, and a forged source selection above 1080p returns HTTP 422. The recommended source is the highest available source at or below 1080p. Lower qualities remain selectable.

### Persistent job queue

SQLite stores jobs, source selections, media identity, target paths, byte progress, attempts, and timestamps. Job states are:

```text
queued -> resolving -> downloading -> finalizing -> ready
                 |          |             |
                 +-------> failed <-------+
downloading <-> paused
queued/downloading/paused -> cancelled
```

Only one worker runs by default. On restart, `resolving`, `downloading`, and `finalizing` jobs become `queued`; their `.part` files and validators remain available to the download engine.

### Download engine

The existing range/resume implementation is retained but changed to consume a `DownloadRequest` containing URL and required HTTP headers. Redirects are followed manually. Every initial destination and redirect must use HTTP(S), resolve only to public addresses, and remain within a bounded redirect count. The API does not accept arbitrary URLs.

Before a job starts, the worker verifies free space, path containment beneath the configured media root, and a configurable reserve of 10 GiB. Video and subtitle content is written to partial files on the same filesystem and atomically renamed during finalization.

### Media layout

The layout follows Jellyfin's documented naming conventions:

```text
/media/Movies/Title (Year)/Title (Year).mkv
/media/Movies/Title (Year)/Title (Year).en.srt
/media/Shows/Series (Year)/Season 01/Series (Year) S01E02 Episode Title.mkv
/media/Shows/Series (Year)/Season 01/Series (Year) S01E02 Episode Title.en.srt
```

Illegal filesystem characters are sanitized, generated paths are containment-checked, and duplicate completed media is reported instead of silently overwritten.

### Jellyfin

The official `jellyfin/jellyfin` image receives read-only access to completed media and read-write access to its own config/cache. `/dev/dri/renderD128` is passed through for Intel VA-API. The i5-4590 is pre-Broadwell, so VA-API is the compatibility path; unsupported codec toggles must remain disabled. Direct play is preferred, and the Mac Jellyfin client is recommended when a browser would otherwise force transcoding.

Jellyfin's real-time monitoring discovers completed atomic renames. When an optional Jellyfin API key is configured, MovieBox Server also requests a library refresh and returns a deep link after the title becomes discoverable.

### Browser interface

The responsive React interface has five focused views:

1. Login.
2. Search and results.
3. Details with season/episode, quality, and subtitle selection.
4. Queue with live progress, speed, pause, resume, cancel, and retry.
5. Ready items with an “Open in Jellyfin” action.

The interface is Netflix-inspired but is a download-control surface, not a second media player. It supports keyboard navigation and a narrow mobile layout, while the primary playback target is a Mac at 1080p.

### Authentication and private access

MovieBox Server listens on `127.0.0.1:8420`; Jellyfin listens on `127.0.0.1:8096`. A single administrator password is bootstrapped from a Docker secret, hashed with Argon2id, and stored in SQLite. Sessions use random server-side tokens, `HttpOnly`, `Secure`, `SameSite=Strict` cookies, idle expiry, absolute expiry, CSRF checks for mutations, and login throttling.

Tailscale Serve terminates private HTTPS on two tailnet-only endpoints and forwards to the loopback services. Funnel is never enabled by project automation. The README also documents LAN-only binding as an alternative for self-hosters without Tailscale, with password authentication still required.

## API outline

```text
GET    /api/health
POST   /api/auth/login
POST   /api/auth/logout
GET    /api/auth/session
GET    /api/catalog/search?q=<query>&page=<n>
GET    /api/catalog/items/:provider/:id
GET    /api/catalog/items/:provider/:id/sources?season=<n>&episode=<n>
GET    /api/catalog/items/:provider/:id/subtitles?source_id=<opaque>
POST   /api/jobs
GET    /api/jobs
GET    /api/jobs/:id
POST   /api/jobs/:id/pause
POST   /api/jobs/:id/resume
POST   /api/jobs/:id/cancel
POST   /api/jobs/:id/retry
GET    /api/events
GET    /api/library/:job_id
```

Errors use a stable envelope with a machine code, safe message, request ID, and optional field errors. Provider URLs, credentials, tokens, filesystem internals, and raw upstream responses are excluded from client errors and sanitized in logs.

## Failure behavior

- Provider unavailable: return a retryable 503 and preserve the current queue.
- No source at or below 1080p: return `quality_unavailable`; do not fall back to 4K.
- Expired source URL: return the job to `resolving` and resolve once more before failing.
- Subtitle failure: complete the video, mark the subtitle warning, and allow subtitle retry.
- Disk reserve reached: keep the job queued with `insufficient_space` and do not create a partial video.
- Restart: recover in-flight jobs to `queued` and resume valid partial data.
- Jellyfin unavailable: media remains `ready`; deep-link/scan status becomes degraded and retries independently.

## Verification and acceptance

- Rust unit tests cover typed adapters, 1080p enforcement, URL/address validation, filename generation, state transitions, and path containment.
- A local mock HTTP server proves range resume, validator changes, redirects, required headers, idle timeout, and cancellation.
- API integration tests use temporary SQLite and media directories.
- Frontend component tests cover search, selections, job controls, and safe error messages.
- Playwright verifies login -> search -> select -> queue -> progress -> ready using a fake provider; tests never download copyrighted media.
- Compose validation and container health checks run in CI.
- A sanitized sample video verifies Jellyfin discovery, external subtitle selection, direct play, and one controlled VA-API transcode.
- On the homeserver, final acceptance requires services reachable only through Tailscale HTTPS, no public listening sockets for 8420/8096, one completed 1080p sample, restart recovery, and unchanged existing containers.

## Documentation and licensing

The README describes the repository as an independently maintained self-hosted fork, credits `mesamirh/MovieBox-Tui`, preserves MIT OR Apache-2.0 notices, and makes no claim of upstream endorsement. It includes prerequisites, quick start, storage layout, Tailscale and LAN modes, updates, backup/restore, troubleshooting, screenshots, and uninstall instructions. It states that users are responsible for complying with content rights, provider terms, and local law.

## Source-backed deployment notes

- Jellyfin publishes an official Linux container and requires the render device for hardware acceleration: <https://jellyfin.org/docs/general/installation/container/>.
- Pre-Broadwell Intel Linux systems should use VA-API rather than QSV: <https://jellyfin.org/docs/general/post-install/transcoding/hardware-acceleration/intel/>.
- Jellyfin movie/show and external-subtitle naming rules: <https://jellyfin.org/docs/general/server/media/movies/>.
- Tailscale Serve is tailnet-private, respects tailnet access controls, and should proxy a localhost-only backend: <https://tailscale.com/docs/features/tailscale-serve>.
