# MovieBox secrets

Create the local secret files before starting Compose. Run these commands from
the repository root:

```bash
umask 077
mkdir -p deploy/compose/secrets
openssl rand -base64 32 > deploy/compose/secrets/admin_password.txt
openssl rand -base64 48 > deploy/compose/secrets/session_key.txt
```

The files are mounted read-only at `/run/secrets/admin_password` and
`/run/secrets/session_key`. Compose passes secret paths to the server instead
of putting secret values in environment variables. Do not commit these files
or share them in logs, issue reports, or backups that are not protected.

Start the private stack with:

```bash
docker compose -f deploy/compose/compose.yml \
  --env-file deploy/compose/.env.example up -d --build
```
