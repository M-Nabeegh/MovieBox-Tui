<div align="center">

# MovieBox-TUI

Search, browse, play, and download movies, series, anime, and IPTV streams from a keyboard-first terminal interface using external media players.

[![Crates.io](https://img.shields.io/crates/v/moviebox-tui.svg?logo=rust)](https://crates.io/crates/moviebox-tui)
[![CI](https://github.com/mesamirh/MovieBox-Tui/actions/workflows/ci.yml/badge.svg)](https://github.com/mesamirh/MovieBox-Tui/actions/workflows/ci.yml)
[![Platforms](https://img.shields.io/badge/Platforms-macOS%20%7C%20Linux%20%7C%20Windows%20%7C%20Android-brightgreen)](#requirements)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

<video src="https://github.com/user-attachments/assets/e3dc0c11-524f-4b0e-8902-e0c66d6ca88d" alt="MovieBox-TUI demo" width="85%" autoplay loop muted></video>

</div>

## Documentation

This README is the project landing page (features, install, usage). The full
documentation set — architecture, providers, players, cache, logging, TV mode,
configuration, and debugging — lives in [`docs/`](docs/README.md). Contribution
guidance is in [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Features

### Catalogs & Browsing

- Search and browse movies, TV series, and anime from multiple content catalogs
- Inspect stream quality groupings and subtitle options before playback

> **Note on BDIX:** BDIX sources are only accessible from supported Bangladeshi ISP networks. Because of this, they are hidden by default. You can enable them manually if your network supports it.

### Playback

- Play streams directly in your local media player (mpv, VLC, or IINA)
- Play protected streams seamlessly without manual configuration

### Downloads

- Download full seasons or episodes with automatic subtitle language selection
- Resume interrupted downloads without losing progress

### IPTV

- Watch live TV by loading remote or local `.m3u` playlists organized by category

### User Interface & App

- View rich graphical posters in supported terminals (Kitty, iTerm2, Sixel) or fallback to text art
- Let the app automatically manage configuration and clean up expired caches

MovieBox-TUI resolves links from upstream services. Availability can change when those services change, and some mirrors are region- or rate-limit-dependent.

## Requirements

- 64-bit Windows, macOS, Linux, or Android (Termux)
- Terminal size of at least 85×24
- One supported player: mpv, VLC, IINA, or any native Android video player
- Internet connection

## Installation

Prebuilt binaries are available for macOS, Linux, and Windows releases. The official install scripts verify the release SHA-256 checksum automatically.
On Termux, install from TUR or build from source with `cargo install` on-device.

### macOS or Linux

#### Homebrew

```bash
brew tap mesamirh/moviebox-tui https://github.com/mesamirh/MovieBox-Tui
brew install mesamirh/moviebox-tui/moviebox-tui
```

The formula selects the correct macOS, Linux x86_64, or Linux ARM64 release.

#### Install script

```bash
curl -fsSL https://raw.githubusercontent.com/mesamirh/MovieBox-Tui/main/install.sh | bash
```

The script detects OS and CPU architecture, then installs to `/usr/local/bin`. Without write access or `sudo`, it uses `~/.local/bin`.

### Windows

Works in PowerShell or Command Prompt (cmd):

```cmd
powershell -Command "irm https://raw.githubusercontent.com/mesamirh/MovieBox-Tui/main/install.ps1 | iex"
```

The installer selects x86_64 or ARM64, installs under `%LOCALAPPDATA%\MovieBox-Tui`, and adds that directory to the user PATH. Open a new terminal after first installation.

### Android (Termux)

MovieBox-Tui runs natively in Termux and opens videos through the Android app chooser on supported devices.

Preferred install via TUR (Termux User Repository):

```bash
pkg install tur-repo
pkg install moviebox-tui
termux-setup-storage
```

TUR packages are distributed separately from crates.io. New MovieBox-Tui GitHub
releases may take some time to appear in TUR, and users still need to update
packages normally in Termux (for example via `pkg upgrade`). Termux playback
should be rechecked on real devices for each release.
If you previously installed MovieBox-Tui with `cargo install`, see
[Troubleshooting](#troubleshooting) for the Termux PATH fix.

Alternative source-based install:

```bash
pkg install rust openssl pkg-config
cargo install moviebox-tui --locked
termux-setup-storage
```

_`termux-setup-storage` is recommended if you want downloads saved to the real
Android `Download` folder._

<details>
<summary><b>Build from source</b></summary>

```bash
git clone https://github.com/mesamirh/MovieBox-Tui.git
cd MovieBox-Tui
cargo build --release --locked
```

Binary location: `target/release/moviebox-tui` (`moviebox-tui.exe` on Windows).

</details>

## Supported Players

MovieBox-TUI checks standard application locations, PATH executables, and Linux Flatpak installations.

Detected automatically:

- **macOS:** `/Applications`, `~/Applications`, Homebrew/PATH
- **Linux:** PATH, Flatpak mpv, Flatpak VLC
- **Windows:** PATH, common Program Files locations, Microsoft Store aliases

Portable or custom installations can be selected with environment variables:

| Player | Variable             |
| ------ | -------------------- |
| mpv    | `MOVIEBOX_MPV_PATH`  |
| VLC    | `MOVIEBOX_VLC_PATH`  |
| IINA   | `MOVIEBOX_IINA_PATH` |

You can also force the preferred detected player order with:

```bash
export MOVIEBOX_PLAYER=mpv
```

macOS/Linux example:

```bash
export MOVIEBOX_MPV_PATH="$HOME/Apps/mpv"
moviebox-tui
```

Windows PowerShell example:

```powershell
$env:MOVIEBOX_VLC_PATH = "D:\Apps\VLC\vlc.exe"
moviebox-tui
```

IINA is macOS-only. mpv provides the broadest source-header compatibility. Android intent playback depends on device chooser behavior and cannot attach subtitles.

## Usage

Run:

```bash
moviebox-tui
```

### Keyboard shortcuts

| Key        | Action                                                |
| ---------- | ----------------------------------------------------- |
| Arrow keys | Navigate lists, grids, seasons, episodes, and dialogs |
| Enter      | Open or confirm selection                             |
| Esc        | Close dialog or go back                               |
| `o`        | Choose another player                                 |
| `d`        | Download selected episode or season                   |
| `r`        | Refresh current content                               |
| `Ctrl+P`   | Switch content provider                               |
| `Ctrl+T`   | Toggle IPTV mode                                      |
| `?`        | Show help                                             |
| `q`        | Quit                                                  |

### Slash commands

| Command              | Action                                    |
| -------------------- | ----------------------------------------- |
| `/discover`, `/home` | Open discovery view                       |
| `/browse`            | Browse curated, rated, and most-watched views |
| `/history`           | Show watch history                        |
| `/movies`            | Browse movies                             |
| `/shows`, `/tvshows` | Browse series                             |
| `/anime`             | Browse anime                              |
| `/list`              | Show IPTV channels                        |
| `/config`            | Configure IPTV playlists                  |
| `/github`            | Open the project repository               |
| `/theme`, `/themes`  | Open the theme picker                     |
| `/update`            | Check for a newer release                 |
| `/toggle-update`     | Enable or disable automatic update checks |
| `/clear-cache`       | Remove cached application data            |
| `/enable-bdix`       | Enable BDIX FTP sources                   |
| `/disable-bdix`      | Disable BDIX FTP sources                  |

`/update` checks availability and shows the release location; it does not replace the running binary. Re-run the installer or Homebrew upgrade command to update.

Choosing a player from `Open with` updates the saved default player for later launches,
unless `MOVIEBOX_PLAYER` is set in the environment.

## Downloads

Downloads are stored under the operating system Downloads directory:

```text
MovieBox-TUI/
├── Movies/
└── Series/<title>/Season <number>/
```

- **Sequential downloads:** Entire seasons are downloaded one by one to limit disk and network pressure.
- **Smart subtitles:** Your subtitle language choice for the first episode is applied to the remaining episodes. Missing subtitles do not discard completed video files.
- **Robust resuming:** Interrupted downloads preserve `.part` and metadata files, and can be resumed without losing progress.
- On Android/Termux, the app prefers shared `storage/downloads` when it exists.

## Configuration & Cache

MovieBox-TUI uses standard OS directories:

| Platform | Configuration                                | Cache                                      |
| -------- | -------------------------------------------- | ------------------------------------------ |
| Linux    | `${XDG_CONFIG_HOME:-~/.config}/moviebox-tui` | `${XDG_CACHE_HOME:-~/.cache}/moviebox-tui` |
| macOS    | `~/Library/Application Support/moviebox-tui` | `~/Library/Caches/moviebox-tui`            |
| Windows  | `%APPDATA%\moviebox-tui`                     | `%LOCALAPPDATA%\moviebox-tui`              |

Catalog providers use separate cache namespaces. Expired or invalid cache entries are discarded automatically; files older than seven days are cleaned at startup.

The current theme, active provider, default player, BDIX visibility, and update
preferences are persisted in `config.json`. See [`docs/config.md`](docs/config.md)
for the full config and environment-variable reference.

## Updates

Automatic update checks only notify you about new releases. They do not update the application automatically.

Homebrew:

```bash
brew update
brew upgrade moviebox-tui
```

Script installation: run the install command again.

Windows PowerShell: run the install command again.

Cargo:

```bash
cargo install moviebox-tui --locked --force
```

## Uninstallation

Homebrew:

```bash
brew uninstall moviebox-tui
brew untap mesamirh/moviebox-tui
```

Script installation:

```bash
sudo rm -f /usr/local/bin/moviebox-tui
rm -f "$HOME/.local/bin/moviebox-tui"
```

Windows PowerShell:

```powershell
Remove-Item -Recurse -Force "$env:LOCALAPPDATA\MovieBox-Tui"
```

Cargo:

```bash
cargo uninstall moviebox-tui
```

Configuration and cache directories remain until removed manually.

## Troubleshooting

<details>
<summary><b>No media player found</b></summary>

MovieBox-TUI relies on external players. If it says none are found:

1. Ensure you have installed **mpv**, **VLC**, or **IINA**.
2. Verify it is in your system PATH by running `mpv --version` or `vlc --version` in your terminal.
3. If using a portable or non-standard installation, set the corresponding environment variable before running (e.g., `MOVIEBOX_MPV_PATH=/path/to/mpv`).

</details>

<details>
<summary><b>Images do not render / Only text is shown</b></summary>

MovieBox-TUI supports inline images via Kitty, Sixel, and iTerm2 protocols.

- If images don't show, ensure you are using a compatible terminal emulator (like Kitty, WezTerm, iTerm2, or Windows Terminal Preview).
- If your terminal does not support graphics, the UI gracefully falls back to text-based posters and remains fully usable.
- If you experience crashes when resizing the window (specifically with Sixel), please report it with your OS, terminal name, and version.

</details>

<details>
<summary><b>Termux: TUR install works, but `moviebox-tui` still points to `~/.cargo/bin`</b></summary>

This usually means you installed MovieBox-Tui with Cargo earlier and your shell
is still preferring the old Cargo path over the TUR package in `$PREFIX/bin`.

Check the installed TUR binary directly:

```bash
$PREFIX/bin/moviebox-tui --version
```

Then clear the shell command cache:

```bash
hash -r
```

If needed, move Termux's bin directory earlier in your shell startup file:

```bash
export PATH="$PREFIX/bin:$HOME/.cargo/bin:$PATH"
```

Then reload your shell and try `moviebox-tui` again.

</details>

<details>
<summary><b>"moviebox-tui: command not found" (Linux / macOS)</b></summary>

If you installed via the script without `sudo`, the binary was placed in `~/.local/bin`. You need to add this to your PATH. Add this line to your `~/.bashrc` or `~/.zshrc`:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Then restart your terminal.

</details>

<details>
<summary><b>Windows PowerShell script fails (Execution Policy)</b></summary>

If you receive an error about running scripts being disabled on your system when installing via PowerShell, run this command as Administrator first:

```powershell
Set-ExecutionPolicy RemoteSigned -Scope CurrentUser
```

Then try the installation command again.

</details>

## Development

Formatting and linting are enforced automatically by the pre-commit hook on every
commit (see [CONTRIBUTING.md](CONTRIBUTING.md)). Before opening a PR, run the same
local checks the repository expects plus the CI-only verification steps:

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo check --locked
cargo audit
cargo package --locked
```

This project currently has no unit-test suite by design, so the release/readme
checklist stays focused on the static gates above plus the runtime checks in
[`docs/release-checklist.md`](docs/release-checklist.md).

GitHub Actions also verifies cross-platform builds on Linux, macOS, and Windows,
plus an Android/Termux cross-build lane.

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidance and
[`docs/release-checklist.md`](docs/release-checklist.md) for the runtime checks still
required before calling a release production-ready.

## License

Licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

## Legal

MovieBox-TUI does not host media. It is not affiliated with any specific content sources, IPTV providers, player projects, or terminal vendors. Users are responsible for complying with laws and service terms applicable to them.
