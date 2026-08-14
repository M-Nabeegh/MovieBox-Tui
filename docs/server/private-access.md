# Private access with Tailscale Serve

MovieBox and Jellyfin are published only to the tailnet. Compose keeps both
services on loopback: MovieBox at `http://127.0.0.1:8420` and Jellyfin at
`http://127.0.0.1:8096`.

Run on the deployment host:

```sh
deploy/tailscale/moviebox-serve.sh --check
deploy/tailscale/moviebox-serve.sh
```

`--check` is a dry run and does not invoke Tailscale. Apply mode refuses to
replace unrelated Serve handlers, saves the previous configuration under
`deploy/tailscale/config/`, and is idempotent once both routes exist.

The private endpoints are `https://<tailnet-hostname>/` for MovieBox on 443 and
`https://<tailnet-hostname>:8443/` for Jellyfin. Verify the result with:

```sh
tailscale serve status
ss -ltn | grep -E ':(8420|8096) '
curl -fsS http://127.0.0.1:8420/api/health
```

The sockets should be loopback-only and Serve should show tailnet HTTPS routes.
This project never enables `tailscale funnel` or router port forwarding. Keep
password authentication enabled when using a trusted LAN binding instead.
