# Configuration

## `config.json`

Written atomically under the config directory (`dirs::config_dir()/moviebox-tui/`;
macOS `~/Library/Application Support/moviebox-tui`, Linux `~/.config/moviebox-tui`).

| Field               | Type           | Meaning                                                                                                     |
| ------------------- | -------------- | ----------------------------------------------------------------------------------------------------------- |
| `auto_update`       | bool           | Check for updates on startup (max once/hour).                                                               |
| `last_update_check` | u64            | Epoch seconds of the last update check.                                                                     |
| `active_provider`   | string         | Last provider (`moviebox`, `fourkhdhub`, …).                                                                |
| `active_theme`      | string         | Theme name.                                                                                                 |
| `bdix_enabled`      | bool           | Show BDIX providers (Bangladesh-only).                                                                      |
| `default_player`    | string or null | Preferred player: `mpv`, `iina`, `vlc`, `android`; absent/null until you choose one from the in-app picker. |

## Other persisted files

- `tv_config.json` — list of M3U playlist sources in the config directory (see [tv-mode.md](tv-mode.md)).
- `history.json` — watch history in the system data directory (`dirs::data_dir()/moviebox-tui/`).
- `iptv_cache/` — legacy TV image cache directory that `ClearCache` still removes.

## Environment variables

| Variable                  | Purpose                                                                           |
| ------------------------- | --------------------------------------------------------------------------------- |
| `MOVIEBOX_LOG`            | Log level: `off`, `warn`, `info`, `debug`, `trace`. See [logging.md](logging.md). |
| `MOVIEBOX_PLAYER`         | Preferred player (overrides `default_player`).                                    |
| `MOVIEBOX_MPV_PATH`       | Custom mpv executable.                                                            |
| `MOVIEBOX_VLC_PATH`       | Custom VLC executable.                                                            |
| `MOVIEBOX_IINA_PATH`      | Custom IINA/iina-cli executable.                                                  |
| `MOVIEBOX_FOURKHDHUB_URL` | Override the 4KHDHub base URL.                                                    |
| `MOVIEBOX_THEME`          | Force a theme (e.g. `Mocha`, `Latte`).                                            |

## CLI

- `moviebox-tui --version`, `moviebox-tui -v`, and `moviebox-tui -V` print the version and exit.
- No other flags.
