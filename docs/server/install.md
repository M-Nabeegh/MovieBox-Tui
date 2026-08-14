# Install

Requirements: Linux, Docker Engine with the Compose plugin, and OpenSSL for
secret generation. Tailscale and `jq` are optional.

From the repository root:

```bash
cp deploy/compose/.env.example deploy/compose/.env
umask 077
mkdir -p deploy/compose/secrets
openssl rand -base64 32 > deploy/compose/secrets/admin_password.txt
openssl rand -base64 48 > deploy/compose/secrets/session_key.txt
docker compose --env-file deploy/compose/.env -f deploy/compose/compose.yml up -d --build
docker compose --env-file deploy/compose/.env -f deploy/compose/compose.yml ps
```

The Compose secret names are fixed: `admin_password.txt` and
`session_key.txt`. Edit `MOVIEBOX_DATA_ROOT`, `MOVIEBOX_UID`, and
`MOVIEBOX_GID`. The personal homeserver convention is `/mnt/nas-data/moviebox`;
other hosts must choose their own dedicated root.

Open Jellyfin through private access, create its administrator, and add
`/media/Movies` and `/media/Shows` as libraries. Hardware acceleration is
optional and is not verified here.

Check Tailscale without changing it:

```bash
deploy/tailscale/moviebox-serve.sh --check
```

After review, run the script with no argument. It configures private HTTPS on
443 for MovieBox and 8443 for Jellyfin. For a LAN alternative, keep the
loopback bindings and use an authenticated reverse proxy; changing Compose to
bind publicly is an operator security decision.
