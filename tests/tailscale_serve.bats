#!/usr/bin/env bats

setup() {
  test_root="$(mktemp -d)"
  stub_bin="$test_root/tailscale"
  calls_file="$test_root/calls"
  printf '%s\n' '{"TCP":{"443":{"HTTPS":true,"Handlers":{"/":"http://127.0.0.1:8420"}},"8443":{"HTTPS":true,"Handlers":{"/":"http://127.0.0.1:8096"}}}}' >"$test_root/status.json"

  cat >"$stub_bin" <<'EOF'
#!/usr/bin/env bash
set -eu
printf '%s\n' "$*" >>"$CALLS_FILE"
case "$*" in
  "serve status --json") cat "$STATUS_JSON" ;;
  "serve get-config --all") printf '%s\n' '{"TCP":{}}' ;;
  *) : ;;
esac
EOF
  chmod +x "$stub_bin"
  export CALLS_FILE="$calls_file"
  export STATUS_JSON="$test_root/status.json"
  export TAILSCALE_BIN="$stub_bin"
  export TAILSCALE_BACKUP_DIR="$test_root/backups"
}

teardown() {
  rm -rf "$test_root"
}

@test "check mode prints the intended private routes without invoking Tailscale" {
  run env TAILSCALE_BIN="$test_root/does-not-exist" deploy/tailscale/moviebox-serve.sh --check

  [ "$status" -eq 0 ]
  [[ "$output" == *"127.0.0.1:8420"* ]]
  [[ "$output" == *"127.0.0.1:8096"* ]]
  [ ! -s "$calls_file" ]
}

@test "apply mode is idempotent when both private routes already exist" {
  run deploy/tailscale/moviebox-serve.sh

  [ "$status" -eq 0 ]
  run cat "$calls_file"
  [ "$status" -eq 0 ]
  [ "$output" = "serve status --json" ]
  [ ! -d "$TAILSCALE_BACKUP_DIR" ]
}

@test "apply mode saves all Serve services with the current CLI syntax" {
  printf '%s\n' '{"TCP":{}}' >"$STATUS_JSON"

  run deploy/tailscale/moviebox-serve.sh

  [ "$status" -eq 0 ]
  run cat "$calls_file"
  [[ "$output" == *"serve get-config --all"* ]]
  [ -s "$TAILSCALE_BACKUP_DIR"/serve-config-*.json ]
}

@test "apply mode refuses to replace an unrelated existing Serve handler" {
  printf '%s\n' '{"TCP":{"443":{"HTTPS":true,"Handlers":{"/":"http://127.0.0.1:9999"}}}}' >"$STATUS_JSON"

  run deploy/tailscale/moviebox-serve.sh

  [ "$status" -ne 0 ]
  [[ "$output" == *"refusing"* ]]
  run cat "$calls_file"
  [ "$status" -eq 0 ]
  [ "$output" = "serve status --json" ]
}
