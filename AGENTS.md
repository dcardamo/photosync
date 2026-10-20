# photosync

Android-first Tauri 2 app that chooses which photo projects a travel device
keeps in sync with a home Syncthing hub, plus a read-only hub status server.
Rust core is platform-neutral; macOS may follow.

Start here:

- `README.md` — setup, install, release, privacy.
- `docs/release-gate.md` — the non-negotiable deletion gate.
- `docs/status-server.md` — hub service and endpoints.
- `docs/development.md` — dev shell, tests, emulator.

## Layout

- `crates/core` — Syncthing REST, profile parse/render, project model,
  capacity, ignore matching, release state machine. No Android dependencies.
- `crates/status-server` — read-only hub service and the live suite.
- `crates/cli` — `photosync` developer CLI.
- `app/src` — Svelte 5 UI.
- `app/src-tauri` — Tauri commands.
- `app/src-tauri/gen/android` — committed Android project. Kotlin lives here:
  `PhotosyncPlugin.kt`, `SecretsVault.kt`, `ReleaseService.kt`.
- `nix/` — generic NixOS module and status package.
- `scripts/` — emulator, build, signing, release, live and Docker tests.

## Environment

Run everything inside `nix develop`. It provides Rust with Android targets,
cargo-tauri, Node 22, JDK 17, the Android SDK/NDK/emulator, Syncthing 2.1.3
and gitleaks. Enter from the checkout or a subdirectory; Android state is
anchored to the checkout root.

- Emulator: `scripts/emulator.sh` boots `photosync35` on `emulator-5560`.
- Debug APK: `scripts/android-debug.sh`, then `scripts/android-install.sh debug`.
- The debug app id is `app.photosync.debug`; release is `app.photosync`.
- Preserve `.android-user/debug.keystore` for in-place debug updates.
- Preserve `.data/photosync.keystore`; installed release copies reject updates
  signed by any other key.

## Checks

```sh
cargo test -p photosync-core -p photosync-status -p photosync-cli
cargo clippy -p photosync-core -p photosync-status -p photosync-cli --all-targets -- -D warnings
cd app && npm run check && npm test && npm run build
scripts/live-tests.sh          # two real Syncthing instances, ignored by default
scripts/check-secrets.sh
```

## Rules

- The release gate is not negotiable. Do not weaken a step, skip it for a
  preview, or use Syncthing's completion/remote-need views as safety evidence.
  Never call `POST /rest/db/ignores`.
- Bootstrap order matters: the profile file must exist before the `#include`
  line is written. The `.stignore` is written once by setup.
- The app writes selection only through `.stprofiles/<device>.txt` and scans
  `.stprofiles`. It never edits Syncthing's config.
- The app allows cleartext only to localhost (Syncthing-Fork). The hub status
  server is HTTPS over the tailnet.
- Never commit personal data: hostnames, tailnet names, IPs, device IDs, API
  keys, tokens, real paths or project names, or screenshots of real data. Use
  synthetic fixtures and `hub.example.ts.net` / `/srv/photos`.
- Never commit `.data/`, keystores, tokens, journals or settings. The
  pre-commit hook and CI block secrets; never use `--no-verify`.
- Signing happens only in the GitHub `release` environment; the build job gets
  no secrets. Releases come from `v*` tags.
- Commit with explicit paths; never amend. Do not push or add Co-Authored-By
  lines unless asked.
- Comments explain non-obvious constraints, not history.
