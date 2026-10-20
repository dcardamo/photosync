#!/usr/bin/env bash
# Boots photosync's emulator headless on port 5560 (serial emulator-5560) and
# waits for boot. Run inside `nix develop`. Stop it with `adb emu kill`.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -f "$ANDROID_AVD_HOME/photosync35.ini" ] || scripts/create-avd.sh
mkdir -p .data
if ! adb devices | grep -q '^emulator-5560'; then
  setsid nohup emulator -avd photosync35 -port 5560 -no-window -no-audio -no-boot-anim \
    -gpu swiftshader_indirect -no-snapshot-save > .data/emulator.log 2>&1 < /dev/null &
fi
adb -s emulator-5560 wait-for-device
until [ "$(adb -s emulator-5560 shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; do sleep 2; done
echo "emulator-5560 booted"
