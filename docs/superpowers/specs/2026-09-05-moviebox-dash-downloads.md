# MovieBox Authenticated DASH Downloads Specification

## Problem

MovieBox's legacy resource endpoint can return a small upgrade/advertisement video with HTTP 200 instead of the selected title. The server's size-integrity guard correctly refuses that response, but the requested movie is not downloaded. Upstream MovieBox-Tui `v0.1.15` replaced legacy playback with visitor authentication and a signed CloudFront MPEG-DASH manifest.

## Required outcome

- Use the current visitor-login and `play-info/v2` flow from upstream commit `c5c591047dfc5a86e90479afbda394ef03e1df20` to resolve MovieBox video.
- Download the selected MovieBox title from its signed DASH manifest into the existing Movies or Shows library path.
- Respect the selected source and the configured maximum height of 1080p.
- Use stream copy/remuxing only; never transcode video or audio during download.
- Keep legacy HTTP-file downloads working unchanged for non-DASH providers.
- Preserve pause, cancel, retry, queue recovery, subtitle sidecars, and the automatic Jellyfin refresh.
- Never mark a manifest, notice clip, truncated output, or media without a video stream as Ready.
- Do not expose JWTs, CloudFront cookies, Jellyfin keys, or other secrets in logs, process output, tests, commits, or documentation.
- Keep the existing safe-path and public-network validation. DASH fetching must not enable arbitrary local-file or private-network reads.
- Keep download concurrency at one and avoid persistent server load while idle.

## Verification

- Add deterministic unit/integration fixtures for visitor session parsing, signed-policy manifest resolution, DASH source selection, 1080p enforcement, cancellation, failure cleanup, and media validation.
- Demonstrate the regression test failing before production changes and passing afterward.
- Run formatting, clippy, all Rust tests, web typecheck/tests/build, and deployment policy checks.
- Deploy without overwriting `/home/nabeegh/moviebox-server/deploy/compose/compose.yml`, `.env`, `deploy/tailscale/config/`, or secret files.
- Retry PK (2014) once. Confirm it is not the 917,554-byte, approximately 20.97-second notice and only report success after media probing shows a real video of the expected feature-length duration.
- Confirm `/mnt/nas-data` is still the backing mount and both MovieBox and Jellyfin containers are healthy after deployment.
