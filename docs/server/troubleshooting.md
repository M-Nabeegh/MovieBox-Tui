# Troubleshooting

```bash
docker compose --env-file deploy/compose/.env -f deploy/compose/compose.yml ps
docker compose --env-file deploy/compose/.env -f deploy/compose/compose.yml logs --tail=120 moviebox-server jellyfin
curl -fsS http://127.0.0.1:8420/api/health
curl -fsS http://127.0.0.1:8096/health
```

For startup failures, check `.env`, both secret files, data-root directories,
UID/GID ownership, and ports 8420/8096. Validate with `docker compose ...
config` using the same `--env-file` and `-f` arguments.

For browser failures, prove loopback health first, then check
`tailscale serve status` or the reverse proxy. Loopback-only binding is
expected. `moviebox-serve.sh --check` is a no-change diagnostic.

For queued/failed jobs, check free space and keep the 10 GiB reserve. Restart
recovery requeues safe in-flight jobs and preserves valid partials. If media is
ready but Jellyfin is unavailable, check its health, `/media` mount, library
paths, and logs; media is not deleted. Without
`MOVIEBOX_JELLYFIN_API_KEY_FILE`, `scan_pending` is expected.

Do not share secrets, cookies, provider URLs, or unredacted logs. Include the
commit, redacted Compose config, container status, health results, and safe
error code instead.
