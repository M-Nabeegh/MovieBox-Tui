# Agent access (MCP)

The server can expose a [Model Context Protocol](https://modelcontextprotocol.io)
endpoint so an agent can search the catalog and queue downloads on your behalf.
Ask it to download a film and the file ends up in the media library, ready to
watch in Jellyfin.

The endpoint is **off unless a token is configured**, and while off it answers
`404` — enabling agent control is always deliberate.

## Enabling it

Create a token and point the server at it:

```bash
umask 077
openssl rand -base64 32 > deploy/compose/secrets/mcp_token.txt
```

Then uncomment this line in `deploy/compose/.env`:

```
MOVIEBOX_MCP_TOKEN_FILE=/run/secrets/mcp_token
```

and recreate the container:

```bash
docker compose --env-file deploy/compose/.env -f deploy/compose/compose.yml up -d
```

## Connecting a client

Transport is JSON-RPC 2.0 over HTTP POST to `/mcp`, authenticated with a bearer
token. Point your agent at the server's private URL:

```json
{
  "mcpServers": {
    "moviebox": {
      "url": "https://<your-tailscale-host>/mcp",
      "headers": { "Authorization": "Bearer <token from mcp_token.txt>" }
    }
  }
}
```

Keep the token out of shell history and version control; it grants the ability to
queue downloads. It never appears in logs or job state.

## Tools

| Tool | Purpose |
| --- | --- |
| `search_catalog` | Find titles by name. Returns catalog ids for the other tools. |
| `list_sources` | List available qualities for an item. Only needed to pick a specific one. |
| `start_download` | Queue a download. Picks the best allowed quality and attaches subtitles automatically. |
| `list_downloads` | Recent downloads with state and progress. |
| `get_download` | State and progress of a single job. |

A typical exchange is two calls: `search_catalog` to resolve the title to an id,
then `start_download` with that id. Quality selection and subtitles are handled
for you — `start_download` takes the highest source within
`MOVIEBOX_MAX_HEIGHT` and attaches the configured subtitle language.

For a series, pass `season` and `episode` to both tools.

## Limits

Agent requests go through exactly the same path as the browser UI, so every
existing rule still applies: the 1080p ceiling, one download at a time, the
free-space reserve, path containment, and the retry policy described in
[downloads](downloads.md). The endpoint adds no way to delete media, change
settings, or read anything outside the catalog and the download queue.

Tool failures come back as readable text rather than protocol errors, so an agent
can explain what went wrong and try something else.
