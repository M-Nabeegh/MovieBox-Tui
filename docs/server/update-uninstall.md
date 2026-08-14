# Update and uninstall

Back up state first, then rebuild the local image:

```bash
docker compose --env-file deploy/compose/.env -f deploy/compose/compose.yml pull jellyfin
docker compose --env-file deploy/compose/.env -f deploy/compose/compose.yml up -d --build
docker compose --env-file deploy/compose/.env -f deploy/compose/compose.yml ps
```

Keep `.env` and secrets during normal updates. Check both health endpoints and
the queue after recreation.

To stop and remove only the Compose containers and network:

```bash
docker compose --env-file deploy/compose/.env -f deploy/compose/compose.yml down
```

This does not delete the data root, media, database, Jellyfin state, `.env`, or
secrets. Remove Tailscale routes separately after checking for other handlers.
Deleting data is a separate irreversible action: verify the exact dedicated
`MOVIEBOX_DATA_ROOT`, keep a backup if needed, and never target
`/mnt/mac-remote`.
