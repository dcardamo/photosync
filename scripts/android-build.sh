#!/usr/bin/env bash
# Builds the unsigned arm64 release APK. CI signs it; scripts/android-sign.sh
# signs locally with the ignored release keystore.
set -euo pipefail
cd "$(dirname "$0")/../app"
cargo tauri android build --apk --split-per-abi --target aarch64
printf 'Unsigned release APKs:\n'
find src-tauri/gen/android/app/build/outputs/apk -name '*release*.apk' -print
