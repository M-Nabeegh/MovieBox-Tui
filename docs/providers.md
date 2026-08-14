# Providers

MovieBox-Tui aggregates several movie/stream providers plus user M3U playlists. Each
provider is an independent async client exposing a similar shape, normalized into the
shared typed models in `providers/models.rs` and the moviebox JSON schema used by the UI.

## Provider kinds

| Kind            | Module                     | Notes                                       |
| --------------- | -------------------------- | ------------------------------------------- |
| `MovieBox`      | `providers/moviebox`       | Primary. Requires request signing (crypto). |
| `FourKHdHub`    | `providers/fourkhdhub`     | 4K releases; hubcloud mirror resolver.      |
| `BdixCircleFtp` | `providers/bdix/circleftp` | BDIX FTP directory scrapes.                 |
| `BdixDhakaFlix` | `providers/bdix/dhakaflix` | BDIX indexer.                               |

BDIX sources are only reachable from supported Bangladeshi ISPs and are hidden by
default (`bdix_enabled` in config; `/enable-bdix`).

## Shared flow

Search, details, and episode-streams are dispatched per provider. Crate-internal seams
in `providers/mod.rs` give every client a shared async trait shape:

- `Provider::search` / `Provider::details` return the moviebox-JSON payload the UI
  consumes: MovieBox provides it natively; the other providers convert their typed
  models via `fourkhdhub`'s adapters. `app/network.rs` dispatches both through the trait.
- `ReleaseProvider::episode_streams` returns the typed `Release` list for the three
  release-based providers; MovieBox keeps its own paginated resource path.

Note: only the MovieBox search honors the `page` argument; the other providers ignore it.
Each provider keeps its own internal format behind its adapter.

Playback resolves to a `PlaybackSource { provider, url, headers, subtitle, source_label }`,
which `app/playback.rs::launch_player` feeds to the external player.

## MovieBox

- Base host pool + per-request HMAC-MD5 signature, client token, and a spoofed Android
  device identity (`crypto.rs`) to satisfy the anti-bot gateway.
- Hosts are retried; a shared runtime token is re-initialized when all hosts fail.
- Title normalization lives in `moviebox/title.rs` (`clean_moviebox_title`).

## 4KHDHub

- `client.rs::resolve_release` walks the release's mirrors; each candidate goes through
  `preflight` which validates the URL and issues an **open-range** probe
  (`Range: bytes=0-`) so probe-only trap mirrors (which answer a tiny bounded range but
  refuse real streaming) are rejected. The 4KHD referer and a browser user-agent are
  attached so referer-gated CDNs stream in the player too.
- `hubcloud.rs` resolves hubcloud/hubdrive pages into candidate playable links and
  picks the best mirror by score.
- Errors are surfaced as `FourKHdHubError`; mirror rejections and the final failure
  reason are logged with the mirror label and a sanitized URL.

## BDIX

- `circleftp` and `dhakaflix` scrape FTP-style indexes; both are used behind the
  Bangladesh-only gate.

## M3U playlists (TV mode)

`providers/m3u.rs` parses an M3U playlist from an `https://` URL or a local file path.
Each channel yields `{ id, name, logo, group, stream_url }`; TV mode groups channels by
`group-title` and dedupes by `stream_url`. See [tv-mode.md](tv-mode.md).

## Error handling

Each provider defines its own `thiserror` enum (`ScraperError`, `FourKHdHubError`,
`CircleFtpError`, `DhakaFlixError`). Errors bubble to `app/requests.rs` handlers, which
surface them in the UI status bar and log the full detail (see [logging.md](logging.md)).
