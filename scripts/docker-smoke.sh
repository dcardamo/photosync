#!/usr/bin/env bash
# Docker smoke test for the read-only status server. This is not the Syncthing
# live suite; it checks the HTTP surface against a synthetic library.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v docker >/dev/null || { echo "docker (or podman-docker) is required" >&2; exit 1; }
work="$PWD/.data/docker-smoke"
rm -rf "$work"
mkdir -p "$work/photos/originals/Years/2024/2024.05.01 Demo" "$work/backup"
printf 'photo' > "$work/photos/originals/Years/2024/2024.05.01 Demo/a.jpg"
printf '(?d).DS_Store\n' > "$work/photos/.stignore"
printf '2025-01-01T00:00:00Z\n' > "$work/backup/last-backup"
token='synthetic-docker-smoke-token-0123456789'
printf '%s' "$token" > "$work/token"
chmod 644 "$work/token"
cat > "$work/status.toml" <<EOF
bind = "0.0.0.0:8787"
root = "/srv/photos"
ignore_file = ".stignore"
backup_marker = "/srv/backup/last-backup"
sizes_cache_seconds = 0
max_entries = 1000
EOF
name=photosync-status-smoke
docker rm -f "$name" >/dev/null 2>&1 || true
docker build -f deploy/Dockerfile -t photosync-status:smoke . >/dev/null
docker run -d --rm --name "$name" \
  -p 127.0.0.1:18787:8787 \
  -v "$work/status.toml:/etc/photosync/status.toml:ro" \
  -v "$work/photos:/srv/photos:ro" \
  -v "$work/backup:/srv/backup:ro" \
  -v "$work/token:/run/secrets/token:ro" \
  photosync-status:smoke >/dev/null
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT
for _ in $(seq 50); do
  if curl -fsS -H "Authorization: Bearer $token" http://127.0.0.1:18787/v1/base-ignores >/dev/null 2>&1; then break; fi
  sleep 0.2
done
curl -fsS -H "Authorization: Bearer $token" 'http://127.0.0.1:18787/v1/tree?prefix=originals/Years/2024' | grep -q 'a.jpg'
curl -fsS -H "Authorization: Bearer $token" 'http://127.0.0.1:18787/v1/sizes?prefix=originals/Years/2024' | grep -q '2024.05.01 Demo'
curl -fsS -H "Authorization: Bearer $token" http://127.0.0.1:18787/v1/base-ignores | grep -q '.DS_Store'
curl -fsS -H "Authorization: Bearer $token" http://127.0.0.1:18787/v1/backup | grep -q '2025-01-01'
if curl -fsS http://127.0.0.1:18787/v1/tree >/dev/null 2>&1; then
  echo "unauthenticated request unexpectedly succeeded" >&2
  exit 1
fi
echo "docker smoke test passed"
