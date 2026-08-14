#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
tailscale_bin="${TAILSCALE_BIN:-tailscale}"
backup_dir="${TAILSCALE_BACKUP_DIR:-${script_dir}/config}"

if [[ "${1:-}" == "--check" ]]; then
  cat <<'EOF'
Tailscale Serve would configure these private HTTPS routes:
  :443   -> http://127.0.0.1:8420
  :8443  -> http://127.0.0.1:8096
No Tailscale command was invoked.
EOF
  exit 0
fi

if [[ "${1:-}" != "" ]]; then
  printf 'usage: %s [--check]\n' "$0" >&2
  exit 2
fi

if ! command -v jq >/dev/null 2>&1; then
  printf 'error: jq is required to inspect Tailscale Serve configuration\n' >&2
  exit 1
fi

status_json="$($tailscale_bin serve status --json)"
handler_values="$(jq -r '[.. | objects | .Handlers? // empty | to_entries[] | .value | select(type == "string")] | unique[]?' <<<"$status_json")"

conflict=''
while IFS= read -r handler; do
  [[ -z "$handler" ]] && continue
  case "$handler" in
    http://127.0.0.1:8420|http://127.0.0.1:8096) ;;
    *) conflict="$handler"; break ;;
  esac
done <<<"$handler_values"

if [[ -n "$conflict" ]]; then
  printf 'error: refusing to replace unrelated Tailscale Serve handler: %s\n' "$conflict" >&2
  exit 1
fi

moviebox_route="$(jq -r '.TCP["443"].Handlers["/"] // empty' <<<"$status_json")"
jellyfin_route="$(jq -r '.TCP["8443"].Handlers["/"] // empty' <<<"$status_json")"
needs_change=0
[[ "$moviebox_route" == "http://127.0.0.1:8420" ]] || needs_change=1
[[ "$jellyfin_route" == "http://127.0.0.1:8096" ]] || needs_change=1

if [[ "$needs_change" -eq 0 ]]; then
  printf 'Tailscale Serve is already configured for MovieBox and Jellyfin.\n'
  exit 0
fi

mkdir -p "$backup_dir"
backup_file="$backup_dir/serve-config-$(date -u +%Y%m%dT%H%M%SZ).json"
if ! "$tailscale_bin" serve get-config "$backup_file" --all; then
  rm -f "$backup_file"
  printf 'error: could not capture the existing Tailscale Serve configuration\n' >&2
  exit 1
fi

printf 'Saved the previous Serve configuration to %s\n' "$backup_file"
if [[ "$moviebox_route" != "http://127.0.0.1:8420" ]]; then
  "$tailscale_bin" serve --bg --https=443 http://127.0.0.1:8420
fi
if [[ "$jellyfin_route" != "http://127.0.0.1:8096" ]]; then
  "$tailscale_bin" serve --bg --https=8443 http://127.0.0.1:8096
fi
