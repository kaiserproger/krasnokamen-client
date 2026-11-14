#!/usr/bin/env bash
# Stage the singleplayer server and plugin pack beside the client binaries.
#   stage-singleplayer.sh DEST_DIR [EXE_SUFFIX]
# Expects the pinned checkouts `krasnokamen-server/` (built in release mode) and
# `krasnokamen-default-plugins/` in the current directory (see release.yml).
# The client drops `krasnokamen-auth` from the pack at runtime: singleplayer is
# offline and the local player is the operator.
set -euo pipefail

dest="${1:?usage: stage-singleplayer.sh DEST_DIR [EXE_SUFFIX]}"
suffix="${2:-}"
packages="basic-economy krasnokamen-audit krasnokamen-auth krasnokamen-economy krasnokamen-essentials krasnokamen-permissions krasnokamen-towns"

install -m 0755 "krasnokamen-server/target/release/krasnokamen${suffix}" "$dest/krasnokamen${suffix}"
for pkg in $packages; do
  src="krasnokamen-default-plugins/$pkg"
  mkdir -p "$dest/plugins/$pkg"
  install -m 0644 "$src/plugin.toml" "$src/plugin.wasm" "$dest/plugins/$pkg/"
  if [ -f "$src/config.toml" ]; then
    install -m 0644 "$src/config.toml" "$dest/plugins/$pkg/"
  fi
done
