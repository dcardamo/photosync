#!/usr/bin/env bash
# Emulator E2E: setup, activate and release against a real status server and a
# loopback device Syncthing double. The canonical Syncthing facts are covered
# by scripts/live-tests.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
serial="${ANDROID_SERIAL:-emulator-5560}"
app=app.photosync.debug
work="$PWD/.data/e2e"
root="$work/hub"
device_root=/sdcard/Download/photosync-e2e
token=e2e-status-token-0123456789

scripts/emulator.sh
if [[ ! -f app/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk ]]; then
  scripts/android-debug.sh
fi
adb -s "$serial" install -r app/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk >/dev/null
adb -s "$serial" shell pm clear "$app" >/dev/null 2>&1 || true
adb -s "$serial" shell appops set "$app" MANAGE_EXTERNAL_STORAGE allow >/dev/null 2>&1 || true
adb -s "$serial" reverse tcp:8384 tcp:18384 >/dev/null
adb -s "$serial" reverse tcp:8787 tcp:18787 >/dev/null

rm -rf "$work"
mkdir -p "$root/originals/Projects/Demo" "$root/published/Projects/Demo" "$work"
printf 'demo-photo' > "$root/originals/Projects/Demo/a.jpg"
printf 'demo-published' > "$root/published/Projects/Demo/a.jpg"
printf '(?d).DS_Store\n' > "$root/.stignore"
printf '2030-01-01T00:00:00Z\n' > "$work/last-backup"
printf '%s' "$token" > "$work/status.token"
chmod 600 "$work/status.token"
touch -d @1600000000 "$root/originals/Projects/Demo/a.jpg" "$root/published/Projects/Demo/a.jpg"
cat > "$work/status.toml" <<EOF
bind = "127.0.0.1:18787"
root = "$root"
ignore_file = ".stignore"
backup_marker = "$work/last-backup"
sizes_cache_seconds = 0
max_entries = 1000
EOF

adb -s "$serial" shell rm -rf "$device_root"
adb -s "$serial" shell mkdir -p "$device_root/originals/Projects/Demo" "$device_root/published/Projects/Demo"
adb -s "$serial" push "$root/originals/Projects/Demo/a.jpg" "$device_root/originals/Projects/Demo/a.jpg" >/dev/null
adb -s "$serial" push "$root/published/Projects/Demo/a.jpg" "$device_root/published/Projects/Demo/a.jpg" >/dev/null
adb -s "$serial" shell touch -d @1600000000 "$device_root/originals/Projects/Demo/a.jpg" "$device_root/published/Projects/Demo/a.jpg" >/dev/null

cargo build -p photosync-status >/dev/null
PORT=18384 node scripts/e2e/fake-syncthing.mjs > "$work/fake-syncthing.log" 2>&1 &
fake=$!
target/debug/photosync-status --config "$work/status.toml" --token-file "$work/status.token" > "$work/status.log" 2>&1 &
status=$!
trap 'kill "$fake" "$status" 2>/dev/null || true; adb -s "$serial" reverse --remove tcp:8384 >/dev/null 2>&1 || true; adb -s "$serial" reverse --remove tcp:8787 >/dev/null 2>&1 || true' EXIT
for _ in $(seq 100); do
  if curl -fsS -H "Authorization: Bearer $token" http://127.0.0.1:18787/v1/base-ignores >/dev/null 2>&1 &&
     curl -fsS http://127.0.0.1:18384/test/health >/dev/null 2>&1; then break; fi
  sleep 0.1
done
adb -s "$serial" shell am start -n "$app/app.photosync.MainActivity" >/dev/null
sleep 2
ANDROID_SERIAL="$serial" APP_ID="$app" API_KEY=e2e-api-key STATUS_TOKEN="$token" \
  node scripts/e2e/android-e2e.mjs
