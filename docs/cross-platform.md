# Cross-platform

Supported: macOS, Linux, Windows, and Android (Termux). The codebase uses
`crossterm` (all platforms), `ratatui`, and a couple of small per-OS branches.

## OS notes

| Platform         | Notes                                                                                                                                                                                                                                                                                                |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| macOS            | IINA preferred player (via bundled `iina-cli`); VLC/mpv `.app` paths detected. `process_group(0)` on spawn.                                                                                                                                                                                          |
| Windows          | mpv/VLC detected via Program Files + `%LOCALAPPDATA%`; players spawned with `CREATE_NO_WINDOW`; path-safe file stems.                                                                                                                                                                                |
| Linux            | Flatpak mpv/VLC supported (`flatpak run …`); xdg data/cache dirs.                                                                                                                                                                                                                                    |
| Android (Termux) | Dynamic filesystem checks for `/system/bin/am` or `termux-open`; playback strips `LD_LIBRARY_PATH` and `LD_PRELOAD` before launching the chooser. Real-device chooser behavior should be confirmed separately from the desktop release pipeline; Proot/Termux X11 behavior is environment-dependent. |

## Terminal capabilities

The app probes the terminal at startup via `ratatui_image`:

- **Poster rendering**: Sixel / Kitty / iTerm2 / HalfBlocks per terminal support; the
  image protocol is re-queried on resize and scroll, and focus changes use a soft
  refresh so launching a player does not flash the screen.
- **Terminal classification**: `TERM=dumb`/`linux` fall back to a basic UI.
- Focus events (focus loss/gain) are used to re-render in place without clearing.

## Things to verify per release

- Player launch on each OS (mpv/VLC/IINA/Android intent) — window sizing, subtitles,
  headers.
- Poster rendering across Sixel (Windows Terminal, foot), Kitty, and half-block
  terminals.
- TV mode with a sample M3U playlist (URL and local file).
- Termux: on-device check that Play opens the Android chooser and the stream plays.
