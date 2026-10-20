# Release gate

The release gate is the only path that deletes local project files. It is
deliberately sequential, persisted, and conservative.

## Steps

1. **Idle and scanned.** Check that the Syncthing folder has no error and is
   idle. Trigger a scan of the project sub-path and wait for idle. Both the
   originals path and the published twin are scanned.
2. **Hub has every device file.** Walk the device's filesystem under the
   project (skipping hub base ignores). For every file, `/v1/tree` must return
   the same relative path, size and `mtime_ns`. Symlinks abort the release.
3. **Backup freshness.** Read `/v1/backup`. Its `last_success` must be strictly
   later than the newest `ctime_ns` among the verified hub files. A missing
   marker blocks.
4. **Profile rewrite.** Remove the project and its published twin from
   `.stprofiles/<device>.txt`, write it atomically, scan `.stprofiles`, and wait
   for idle. The app never calls `POST /rest/db/ignores`.
5. **Ignores reloaded.** Poll `GET /rest/db/ignores` until neither
   `!/<originals>` nor `!/<published>` appears in `ignore` or `expanded`. A
   timeout blocks.
6. **Delete and confirm.** Delete the local project and twin, scan the whole
   folder, wait for idle, then fetch `/v1/tree` again and confirm every verified
   file is still on the hub with the same size and `mtime`. A failure here is a
   loud error: the app never claims success and never re-activates the project
   automatically.

## Abort and recovery

The journal is appended after every step in the app data directory (outside the
synced folder). It persists on disk so a vendor kill is recoverable.

- Before deletion: any failure or timeout restores the profile with the project
  active, scans `.stprofiles`, and marks the attempt aborted. The local files
  are untouched.
- After deletion has started: the app does not restore the project. It verifies
  the hub still has the files. If verification fails it records a loud error
  and stops.
- On the next app start, an interrupted record is recovered: before deletion it
  restores the profile and marks the record aborted; after deletion it confirms
  the hub and marks the record completed or failed.

## Fault injection

The live suite in `crates/status-server/tests/live.rs` uses real Syncthing
instances for the device and hub folder, and injects:

- a folder error before the release starts,
- a hub missing a file,
- a size mismatch,
- an mtime mismatch,
- a stale backup,
- a failing profile scan,
- ignores that never reload,
- a hub that loses files after local deletion.

Every fault before deletion leaves the project active and the local files
present. The final case produces the loud error.

## Signals that are never used as safety evidence

- The device's `/rest/db/completion` and `/rest/db/remoteneed` for the hub.
- The hub's `/rest/db/file` availability after a device starts ignoring a file.

The live suite records these values for visibility, but the gate always uses the
device filesystem plus the hub's `/v1/tree`.
