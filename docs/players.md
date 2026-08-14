# Players

Playback is handed to an external player. `tui/player.rs` detects available players and
builds the exact command; `tui/app/playback.rs` spawns it.

## Detection

`player::detect()` returns players in priority order:

- macOS: IINA (if present), then mpv, then VLC.
- Linux/Windows: mpv, then VLC.
- Android/Termux: the Android intent fallback is attempted last when `termux-open` or `am` is available.

Resolution runs once at startup and is cached (`OnceLock`). A preferred player can be
forced via `MOVIEBOX_PLAYER` env or `default_player` in config (e.g. `mpv`, `iina`,
`vlc`, `android`), which reorders the list. Player picker (`Open with`) lists every
detected player and saves the chosen player as the next default unless
`MOVIEBOX_PLAYER` is set.

## Command construction

| Player          | Invocation                                                                                                                                | Notes                                                                                                  |
| --------------- | ----------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| mpv             | `mpv --autofit=WxH --geometry=50%:50% --idle=no --keep-open=no [--http-header-fields=..] [--sub-file=..] <url>`                           | Window sized to the terminal. Flatpak mpv is launched via `flatpak run`.                               |
| VLC             | `vlc --width=W --height=H --play-and-exit [--http-referrer=..] [--http-user-agent=..] [--sub-file=..] <url>`                              |                                                                                                        |
| IINA            | `iina-cli --keep-running --no-stdin --mpv-autofit=.. --mpv-http-header-fields=.. --mpv-sub-files=.. <url>`                                | Uses the bundled `iina-cli`; falls back to `open -a IINA <url>` only if the CLI is absent.             |
| Android / Proot | `termux-open --chooser --content-type video/* <url>` (or absolute `/system/bin/am start` fallback, ensuring `.so` injections are dropped) | Opens an app chooser on the device. Real-device chooser behavior should be confirmed for each release. |

Window size is derived from the live terminal size times the real font cell size
(reported by the image picker), clamped to a sane range.

## Headers

Playback sources (e.g. 4KHD) may carry `Referer`/`User-Agent` headers. mpv/IINA send
them via `http-header-fields` (`--http-header-fields=...` or `--mpv-http-header-fields=...`),
while VLC maps them to `--http-referrer` / `--http-user-agent`. The `supports_headers`
gate in `app/playback.rs` warns when a player cannot satisfy a source's headers.

## Subtitles

- mpv receives the remote subtitle URL directly (`--sub-file=<url>`); mpv fetches it
  with the stream headers applied.
- VLC and IINA download the subtitle to a temp file first, preserving the URL's
  extension (srt/vtt/ass/…), and pass the local path. The download applies the source
  headers. On failure a status is shown and playback continues without subtitles.
- Temp files are cleaned up after the player exits and purged at startup if stale.

## Spawning

`launch_player` spawns the player with null stdin/stdout, piped stderr, and its own
process group (Unix) or no-console flag (Windows). A blocking task reads stderr and
reports a crash if the process exits non-zero within a few seconds with output.
Watch history is marked on launch.
