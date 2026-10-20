# Development

## Dev shell

Use `nix develop` from the repository root (or a subdirectory). It provides
Rust with the Android targets, cargo-tauri, Node 22, JDK 17, the Android SDK,
NDK and emulator, Syncthing 2.1.3, gitleaks and ImageMagick. The shell anchors
`ANDROID_USER_HOME` and `ANDROID_AVD_HOME` to the checkout root and enables the
pre-commit hook.

The private debug key is `.android-user/debug.keystore`. Preserve it for
in-place debug updates. The release key is `.data/photosync.keystore` and is
never committed.

## Checks

```sh
cargo test -p photosync-core -p photosync-status -p photosync-cli
cargo clippy -p photosync-core -p photosync-status -p photosync-cli --all-targets -- -D warnings
cd app && npm run check && npm test && npm run build
scripts/check-secrets.sh
```

The live suite is ignored by default because it starts real processes:

```sh
scripts/live-tests.sh
```

It uses `SYNCTHING_BIN` if set, otherwise `syncthing` from the shell. Each test
creates a temp directory with two Syncthing homes, two libraries, a status
server config, a token and a journal. Ports are loopback-only and chosen at run
time.

## Emulator

```sh
scripts/emulator.sh
scripts/android-debug.sh
scripts/android-install.sh debug
```

`emulator.sh` writes a Pixel 7 sized Android 15 AVD named `photosync35` and
boots it headless as `emulator-5560`. The debug app id is
`app.photosync.debug`.

For UI end-to-end tests, the emulator can reach host services with
`adb reverse`:

```sh
adb reverse tcp:8384 tcp:8384
adb reverse tcp:8787 tcp:8787
```

That keeps Syncthing and the status server on `127.0.0.1` inside the emulator,
which is the only cleartext destination the app allows.

`scripts/android-e2e.sh` does this automatically. It drives the real setup,
activate and release UI through the WebView, against a real status server and a
small device-side Syncthing REST double. The real Syncthing facts stay in
`scripts/live-tests.sh`.

## Layout

- `crates/core` — Syncthing REST client, profile rendering, project model,
  capacity, ignore matching, release state machine.
- `crates/status-server` — read-only hub service and live tests.
- `crates/cli` — `photosync` developer CLI (`list`, `activate`, `release --dry-run`).
- `app/src` — Svelte UI.
- `app/src-tauri` — Tauri commands and the Android project.
- `app/src-tauri/gen/android` — committed Android project with the Kotlin
  vault, all-files permission, keep-screen-on and release service.
- `nix/` — generic NixOS module and the status server package.
- `scripts/` — emulator, build, signing, release and live-test helpers.
