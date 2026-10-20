#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
case "${1:-debug}" in
  debug) apk=app/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk ;;
  release) apk=$(find .data/signed -name '*arm64*release*.apk' -print -quit) ;;
  *) echo 'Usage: android-install.sh [debug|release]' >&2; exit 2 ;;
esac
[[ -n $apk && -f $apk ]] || { echo "APK missing; build first" >&2; exit 1; }
adb install -r "$apk"
