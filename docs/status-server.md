# Status server

`photosync-status` is a read-only HTTP service for the hub. It reads the
Syncthing folder from disk, applies the hub's `.stignore`, and never writes to
the library.

## Configuration

```toml
bind = "127.0.0.1:8787"
root = "/srv/photos"
ignore_file = ".stignore"
backup_marker = "/var/lib/photosync/last-backup"
sizes_cache_seconds = 60
max_entries = 100000
# Optional; both must be set together.
# tls_cert = "/var/lib/photosync/hub.example.ts.net.crt"
# tls_key = "/var/lib/photosync/hub.example.ts.net.key"
```

Run it with a token file:

```sh
photosync-status --config /etc/photosync/status.toml --token-file /run/secrets/photosync-status-token
```

The token file must contain at least 16 bytes. The server compares it in
constant time and returns `401` without a valid `Authorization: Bearer ...`
header. Bind it to loopback or a tailnet IP; if TLS is not configured it logs a
warning.

## Endpoints

All endpoints are under `/v1` and are read-only (`GET` only).

### `GET /v1/tree?prefix=`

Returns files under `prefix`, recursively:

```json
{
  "entries": [
    {
      "path": "originals/Years/2024/2024.05.01 Coast/a.jpg",
      "size": 123456,
      "mtime_ns": 1714567890123456789,
      "ctime_ns": 1714567890123456789
    }
  ]
}
```

Paths are relative to `root`, use `/`, and are filtered by the expanded hub
`.stignore`. Symlinks are skipped. A missing prefix returns an empty list. The
server rejects `..`, absolute prefixes and more than `max_entries` files.

### `GET /v1/sizes?prefix=`

Returns recursive totals for each immediate child directory of `prefix`:

```json
{
  "projects": [
    {
      "path": "originals/Years/2024/2024.05.01 Coast",
      "size": 123456,
      "files": 12,
      "mtime_ns": 1714567890123456789
    }
  ]
}
```

Results are cached for `sizes_cache_seconds`. The app combines the originals
and published trees to report a project's total.

### `GET /v1/base-ignores`

```json
{ "lines": ["(?d).DS_Store", "(?d)**/.DS_Store"] }
```

`#include` lines are expanded recursively (maximum depth 16, loop detection),
so the device profile inherits real patterns rather than a hub-specific include
line.

### `GET /v1/backup`

```json
{
  "last_success": "2024-05-01T12:00:00Z",
  "marker_mtime_ns": 1714567890123456789
}
```

The marker file may contain RFC3339 or Unix seconds. A missing or empty marker
returns `last_success: null`, which blocks the release gate. An invalid marker
returns `500`, also blocking the gate.

## Backup job

Write the marker only after a successful restic run, for example:

```sh
restic backup /srv/photos && date -u +%Y-%m-%dT%H:%M:%SZ > /var/lib/photosync/last-backup
```

The release gate compares this time with the newest `ctime` of the files it
verified on the hub.

## NixOS

The flake exports `nixosModules.default`; see `deploy/photosync-status.nix`.
The module creates a system user, writes the TOML config, uses systemd
hardening, and keeps the service read-only. It does not open the firewall
unless `openFirewall = true`.
