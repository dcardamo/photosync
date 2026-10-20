#!/usr/bin/env bash
# Runs the two-real-Syncthing live suite plus the real status server.
# Needs `syncthing` (the flake provides 2.1.3) and a Rust toolchain.
set -euo pipefail
cd "$(dirname "$0")/.."
exec nix develop -c cargo test -p photosync-status --test live -- --ignored --test-threads=1 "$@"
