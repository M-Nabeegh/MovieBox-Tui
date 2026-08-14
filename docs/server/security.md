# Security

- Compose publishes MovieBox on `127.0.0.1:8420` and Jellyfin on
  `127.0.0.1:8096`.
- Tailscale Serve is private and never uses Funnel or router port forwarding.
- Login uses the mounted admin password; mutations require origin and CSRF
  validation.
- Secrets are files at `deploy/compose/secrets/*.txt`, not environment values.
  Keep them owner-readable and out of Git and logs.
- Containers drop all capabilities, use `no-new-privileges`, run non-root, and
  give Jellyfin a read-only media mount.

Do not expose the API directly to the Internet. If using a LAN reverse proxy,
keep authentication and firewall restrictions enabled. The server accepts no
arbitrary download URL or filesystem path and does not send provider URLs or
tokens to the browser. Treat titles, paths, cookies, and logs as sensitive.
Changing the session key invalidates existing sessions.
