# Storage

Compose uses one host root, `MOVIEBOX_DATA_ROOT`:

| Host path | Container path | Use |
| --- | --- | --- |
| `config/server` | `/config` | SQLite at `/config/moviebox.db` |
| `config/jellyfin` | `/config` | Jellyfin state |
| `cache/jellyfin` | `/cache` | Jellyfin cache |
| `partials` | `/partials` | Resumable partials |
| `media` | `/media` | Final library |

The personal homeserver path is `/mnt/nas-data/moviebox`; choose another root
elsewhere. MovieBox mounts media read-write and Jellyfin mounts it read-only.
Final names follow Jellyfin conventions, for example:

```text
media/Movies/Title (Year)/Title (Year).mkv
media/Shows/Series (Year)/Season 01/Series (Year) S01E02 Episode Title.mkv
```

The server rejects `/`, `/mnt/mac-remote`, unsafe paths, and weak secret files.
Use the configured UID/GID for ownership. Keep at least 10 GiB free;
`MOVIEBOX_RESERVE_GIB` cannot be below 10. Partials survive pause and safe
restart; finalization is atomic.
