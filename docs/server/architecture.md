# Server architecture

`moviebox-server` is the optional feature-gated Rust binary. The Dockerfile
builds the React web UI, embeds it in the binary, and runs it as a non-root
user. The authenticated Axum API and UI listen on port 8420.

The server resolves MovieBox sources, stores sessions/jobs/progress/events in
SQLite, and runs one resumable download worker. It writes final media beneath
the configured media root and keeps provider URLs and tokens out of browser
responses.

Jellyfin runs separately on port 8096 and mounts `/media` read-only. It scans
`Movies/` and `Shows/` paths and may provide a deep link. Without a Jellyfin API
key, completed media remains safe on disk and can report `scan_pending`.

Compose publishes both host ports on loopback. `deploy/tailscale/moviebox-serve.sh`
can map private Serve routes 443 -> `127.0.0.1:8420` and 8443 ->
`127.0.0.1:8096`; it refuses unrelated existing handlers.
