# Completion notifications

A download can run for hours, so the moment worth knowing about is when it
becomes watchable. When a webhook is configured, the server posts a message the
moment a job reaches `ready`:

> Hey Nabeegh, Cocktail 2 (2026) is ready to watch.

Notifications are off until a webhook is configured.

## Setting it up

The payload is a plain `{"title", "body", "url"}` JSON POST, which works with
[Hark](https://hark.ryan.ceo/) and most other webhook-to-push relays.

1. Create a service in your relay and copy its webhook URL.
2. Save it as a secret — the URL contains a token, so it does not belong in
   `.env`:

   ```bash
   umask 077
   printf '%s\n' 'https://hark.ryan.ceo/hooks/whk_your_token' \
     > deploy/compose/secrets/notify_webhook_url.txt
   ```

3. Uncomment these in `deploy/compose/.env`:

   ```
   MOVIEBOX_NOTIFY_WEBHOOK_URL_FILE=/optional-secrets/notify_webhook_url.txt
   MOVIEBOX_NOTIFY_RECIPIENT=Nabeegh
   MOVIEBOX_NOTIFY_LINK_URL=https://your-host.ts.net:8443/
   ```

4. Recreate the container:

   ```bash
   docker compose --env-file deploy/compose/.env -f deploy/compose/compose.yml up -d
   ```

## Settings

| Variable | Effect |
| --- | --- |
| `MOVIEBOX_NOTIFY_WEBHOOK_URL_FILE` | Secret file holding the webhook URL. Unset disables notifications. |
| `MOVIEBOX_NOTIFY_RECIPIENT` | Name in the greeting. Unset gives "Cocktail 2 (2026) is ready to watch." |
| `MOVIEBOX_NOTIFY_LINK_URL` | Tap destination — point it at your media library. |

## Behaviour

Only a download that finishes and passes size verification is announced; a
failed or retrying job never claims to be ready. Series episodes are announced
individually, using the episode's own title.

Delivery is best effort. The file is already in the library by the time the
notification is sent, so a relay that is down, slow, or rejecting requests is
logged and ignored rather than failing the download. Requests time out after
five seconds.

The webhook URL is treated as a credential: it is read from a secret file, kept
out of job state, events, and API responses, and never included in log messages.
Because the token travels in the URL, a remote webhook must use HTTPS — plain
HTTP is accepted only for a relay running on the same machine.
