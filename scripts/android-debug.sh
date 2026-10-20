#!/usr/bin/env bash
# Builds a debug x86_64 APK for the emulator.
set -euo pipefail
cd "$(dirname "$0")/../app"
cargo tauri android build --debug --apk --target x86_64
printf 'Debug APK: %s\n' \
  "$(pwd)/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk"
