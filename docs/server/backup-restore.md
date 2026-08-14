# Backup and restore

Stop services before backing up SQLite and service state:

```bash
DATA_ROOT=/mnt/nas-data/moviebox
docker compose --env-file deploy/compose/.env -f deploy/compose/compose.yml down
tar -C "$DATA_ROOT" -czf moviebox-state.tgz config/server config/jellyfin cache/jellyfin
tar -C "$DATA_ROOT" -czf moviebox-media.tgz media partials
```

Protect the archives. Back up the two secret files separately through the same
protected process; never put them in Git or an unencrypted issue attachment.

To restore, stop the stack, restore beneath the exact data root, verify
UID/GID and secret permissions, restore media before starting Jellyfin, then
start with the same `.env`. Preserve the original until the restored stack has
passed health checks. Partials are optional recovery data; completed media and
the database are the primary restore set.
