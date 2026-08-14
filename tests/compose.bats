#!/usr/bin/env bats

setup() {
  rendered_config="$(mktemp)"
  run docker compose \
    -f deploy/compose/compose.yml \
    --env-file deploy/compose/.env.example \
    config --format json
  [ "$status" -eq 0 ]
  printf '%s\n' "$output" >"$rendered_config"
}

teardown() {
  rm -f "$rendered_config"
}

@test "both services publish only loopback ports and restart automatically" {
  run jq -e '
    (.services["moviebox-server"].ports | all(.[]; .host_ip == "127.0.0.1")) and
    (.services.jellyfin.ports | all(.[]; .host_ip == "127.0.0.1")) and
    .services["moviebox-server"].restart == "unless-stopped" and
    .services.jellyfin.restart == "unless-stopped"
  ' "$rendered_config"
  [ "$status" -eq 0 ]
}

@test "services use health checks and the server drops all capabilities" {
  run jq -e '
    .services["moviebox-server"].healthcheck != null and
    .services.jellyfin.healthcheck != null and
    (.services["moviebox-server"].cap_drop | index("ALL")) != null and
    (.services["moviebox-server"].privileged // false) == false
  ' "$rendered_config"
  [ "$status" -eq 0 ]
}

@test "jellyfin receives read-only media and exactly one render GPU" {
  run jq -e '
    (.services.jellyfin.volumes | any(.[]; .target == "/media" and .read_only == true)) and
    (.services.jellyfin.devices | length == 1) and
    .services.jellyfin.devices[0].source == "/dev/dri/renderD128" and
    .services.jellyfin.devices[0].target == "/dev/dri/renderD128"
  ' "$rendered_config"
  [ "$status" -eq 0 ]
}

@test "persistent mounts do not use the forbidden remote path" {
  run jq -e '
    ([.services[].volumes[]?.source, .services[].volumes[]?.target] | any(. == "/mnt/mac-remote" or contains("/mnt/mac-remote"))) | not
  ' "$rendered_config"
  [ "$status" -eq 0 ]
}
