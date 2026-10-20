# photosync

photosync chooses which photo projects a travel device keeps in sync with a
home Syncthing hub. It is an Android app (Tauri 2, Rust core, Svelte +
TypeScript) plus a small read-only status server for the hub. Android comes
first; the core has no Android dependencies so a macOS build can follow.

The app never edits Syncthing's configuration. It writes
`.stprofiles/<device>.txt` inside the shared folder and asks Syncthing-Fork to
scan `.stprofiles`. The device's `.stignore` is written once:

```
#include .stprofiles/<device>.txt
!/.stprofiles
*
```

The profile is the selection: hub base ignores, always-on paths, then `!/<path>`
for every active project and its `published` twin. The app escapes `[ ] { } * ? \`
in project names.

## Install

Android only. Sideload the signed arm64 APK and update it with
[Obtainium](https://github.com/ImranR98/Obtainium) from
`https://github.com/dcardamo/photosync`. Every `v*` release carries
`photosync-<version>-arm64.apk` and notes.

You need Syncthing-Fork on the device and an existing shared folder. The hub is
the only Syncthing peer; devices never connect to each other.

## Hub status server

Syncthing's own API cannot answer "what is on the hub's disk", "when did the
last restic backup succeed", or "what are the hub's base ignores", and the
device's view of the hub is unreliable. The release gate therefore uses a
read-only HTTP service on the hub:

- `GET /v1/tree?prefix=` — files under the prefix with size, `mtime_ns` and
  `ctime_ns`, read from the hub's disk and filtered by its `.stignore`.
- `GET /v1/sizes?prefix=` — recursive size/files for each immediate child
  directory, cached.
- `GET /v1/base-ignores` — the hub `.stignore` patterns, with `#include`
  expanded.
- `GET /v1/backup` — last successful backup from a marker file.

All endpoints require `Authorization: Bearer <token>`. The token comes from
`--token-file`; the config is TOML. See [docs/status-server.md](docs/status-server.md)
and [deploy/status.example.toml](deploy/status.example.toml).

The NixOS module is `nixosModules.default`. It has no host assumptions:

```nix
services.photosync-status = {
  enable = true;
  tokenFile = "/run/secrets/photosync-status-token";
  settings = {
    bind = "203.0.113.10:8787";
    root = "/srv/photos";
    ignore_file = ".stignore";
    backup_marker = "/var/lib/photosync/last-backup";
    sizes_cache_seconds = 60;
    tls_cert = "/var/lib/photosync/hub.example.ts.net.crt";
    tls_key = "/var/lib/photosync/hub.example.ts.net.key";
  };
};
```

The app allows cleartext only to localhost (Syncthing-Fork). The hub server is
expected over HTTPS, for example with a `tailscale cert` certificate bound
directly to the tailnet IP.

## Release gate

Releasing is gated on both the `originals` path and its `published` twin:

1. The local folder is idle with no error. Scan the project sub-path, wait for
   idle.
2. Every non-ignored file on the device's disk is on the hub with the same size
   and `mtime`. The app walks the filesystem directly; it does not trust
   Syncthing's completion or remote-need views.
3. The hub's last successful backup is newer than the newest hub `ctime` of
   those files.
4. The profile is rewritten without the project, `.stprofiles` is scanned.
5. `/rest/db/ignores` no longer lists the project's lines. This proves the
   ignores reloaded.
6. Local files are deleted, the folder is rescanned, and `/v1/tree` confirms
   the hub still has every file. If that fails, the app raises a loud error.

Any timeout or ambiguity aborts and leaves the project active. Once deletion
has started, a failure is never silently re-activated: the app verifies the hub
and reports the loud error. The flow runs in the foreground with the screen on;
an interrupted run is recovered from a journal on the next start. See
[docs/release-gate.md](docs/release-gate.md).

## Development

Everything runs in the Nix dev shell:

```sh
nix develop
cargo test -p photosync-core -p photosync-status -p photosync-cli
cd app && npm ci && npm run check && npm test && npm run build
```

The live suite starts two real Syncthing 2.1.3 instances on loopback (temp
homes, discovery and relays off), the real status server, and re-verifies the
selection facts and every gate fault:

```sh
scripts/live-tests.sh
```

Android:

```sh
scripts/emulator.sh
scripts/android-debug.sh
scripts/android-install.sh debug
```

The emulator is `photosync35` on `emulator-5560`. The debug app ID is
`app.photosync.debug`; the release app ID is `app.photosync`.

The optional Docker smoke test runs the status server against a synthetic
fixture:

```sh
scripts/docker-smoke.sh
```

## Releases and signing

`scripts/release.sh [X.Y.Z]` bumps the version, commits, tags `vX.Y.Z`, pushes,
and waits for `.github/workflows/release.yml`. CI builds the unsigned arm64
APK without secrets, then the `release` environment signs it and refuses an APK
whose certificate SHA-256 differs from the pinned value. Never generate a new
release key once installed copies exist: Obtainium updates would be rejected.

The release keystore, password and base64 copy live only in gitignored
`.data/`. Back up `.data/photosync.keystore` and its password: losing them means
installed copies cannot be updated. Add them to the GitHub `release`
environment as `KEYSTORE_BASE64` and `KEYSTORE_PASSWORD`. The pinned certificate is
`4591CFDABD68DE29C741F745115238DF4035AFA16A7C99CDE53E12C64466291A`.

Secret scanning runs in the pre-commit hook, in `scripts/release.sh`, and in
CI (`scripts/check-secrets.sh`, gitleaks). Do not bypass it with `--no-verify`.

## Privacy

The app talks only to Syncthing-Fork on loopback and to the hub status server
over the tailnet. The API key and status token are encrypted with Android
Keystore in `noBackupFilesDir`; nothing is sent to a third party. The public
repository contains no personal data: use synthetic fixtures and placeholders
such as `hub.example.ts.net` and `/srv/photos`.

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or
the [MIT license](LICENSE-MIT), at your option.
