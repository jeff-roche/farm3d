# P9 verification record

The verification record for P9 (History, Settings, backup and restore,
diagnostics). Task 19 adds the installed-bundle section. Task 20 finishes
the record.

## Task 19: installed bundle (Linux x86_64)

Run on 2026-09-30 from branch `feature/p9-portability`. The first package
was built at `6449c57`. The second was built at `69f5a6b`, after the fix
described below.

### How the bundle was run

- **Substitution: extracted, not installed.** The checklist says to
  install the deb. No sudo or system-wide install was allowed, and the
  host (an Arch-based distribution) has no `dpkg-deb`. So the `.deb` was
  unpacked with `ar x` and `tar -xf data.tar.gz` into a scratch directory,
  and `usr/bin/farm3d` was run from there. Tauri resolves the bundle's
  resources from `../lib/farm3d` next to the binary, so the extracted tree
  used the packaged `resources/printer-catalog.json` and
  `farm3d-notification.png`. It did not use the build tree.
- **Isolation.** Every launch set `XDG_DATA_HOME`, `XDG_CONFIG_HOME`,
  `XDG_CACHE_HOME`, and `XDG_STATE_HOME` to fresh directories under the
  scratch root. It also set
  `DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent`, so the credential
  store fell back to its file store (`about.json` in the diagnostics
  bundle reports `credentialStore.kind: fallbackFile`). No credentials
  were entered. No isolated Farm had a Printer.
- **Where the files landed** (identifier `farm3d`):
  - metadata root, database, and restore journal:
    `$XDG_CONFIG_HOME/farm3d/`;
  - data root (content, media, backups, webview state):
    `$XDG_DATA_HOME/farm3d/`;
  - log root (`app_log_dir()`): `$XDG_DATA_HOME/farm3d/logs/`;
  - slicer cache: `$XDG_CACHE_HOME/farm3d/`.
- **The owner's Farm was not touched.** `ls -la --time-style=full-iso` of
  `~/.local/share/farm3d` and `~/.config/farm3d` was taken before and after
  the run, and the two were identical. The only difference is the parent
  directory `~/.config`, whose mtime was moved by other desktop software.
  `~/.cache/farm3d` did not exist before the run or after it.
- **Seeding.** A pending restore or reset needs a backup, a staged
  preview, and a journal. The run seeded them the same way the P9 tracer
  does: through the real IPC commands (`create_backup`, `preview_restore`,
  `apply_restore`, `reset_farm`, `export_diagnostics`). The P9 flow rig
  (`tests/common/p9_flow.rs`) ran them with fake dialogs and a recording
  restarter, pointed at the isolated Farm's directories. The harness was a
  temporary, uncommitted test file. The bundled binary then did the
  startup install, the recovery, and the reset itself.

### Checklist

| Item | Result |
| --- | --- |
| `just package` | Verified |
| Install the deb on Linux x86_64 | Verified, by extraction (see the substitution above) |
| The log file lands in `app_log_dir()` and rotates | Verified |
| A backup is created through the real save dialog | Pending owner |
| A restore through the real open dialog restarts the app and shows the outcome banner | Partly verified; the dialog and in-app restart are pending owner. The banner was missing; fixed in `69f5a6b` |
| Killing the app during install (`kill -9` while the journal says `installing`) recovers on the next launch | Verified |
| Diagnostics export writes a zip that a manual `grep` for the seeded test values doesn't match | Verified through the command, not through the bundle's native dialog; that path is pending owner |
| A tier (c) reset | The install half is verified on the bundle; the Settings UI is pending owner |
| Windows | Not verified |
| macOS | Not verified |

#### `just package`: Verified

`just package` exited 0 on both builds. It produced
`farm3d_0.1.0_amd64.deb`, `farm3d-0.1.0-1.x86_64.rpm`, and
`farm3d_0.1.0_amd64.AppImage`. The recipe's own
`scripts/assert-package-contents.sh` step passed, and a separate run of the
script also exited 0. The deb's sha256 was `a8e47fe0…` for `6449c57` and
`491516a4…` for `69f5a6b`. The release build reports one warning:
`held_back_note` in `src/attention/services.rs` is never used.

#### Log location and rotation: Verified

- **Location.** A fresh launch creates
  `$XDG_DATA_HOME/farm3d/logs/` and writes no file until the first line
  is logged, because the logger opens `farm3d.log` lazily. Once there is a
  line, it appears at `$XDG_DATA_HOME/farm3d/logs/farm3d.log`. An example
  is `{"code":"restore.installed","fields":{"attempts":2,"rolledBack":true},...}`.
- **Rotation.** The threshold is `ROTATE_BYTES = 2 MiB` and the logger
  keeps `MAX_FILES = 5`, the active file plus four rotated ones. To force a
  rotation, the run took an isolated Farm with a pending restore, padded
  `farm3d.log` with valid lines to 2,097,060 bytes (40 bytes below the
  threshold), and pre-seeded marker files `farm3d.1.log` through
  `farm3d.5.log`. The bundle was then launched. After the launch:
  - `farm3d.log` held one line (`restore.installed`, 128 bytes);
  - `farm3d.1.log` was the old 2,097,060-byte file;
  - `farm3d.2.log`, `farm3d.3.log`, and `farm3d.4.log` held the old
    markers 1, 2, and 3;
  - old marker 4 was deleted;
  - `farm3d.5.log`, which is outside the kept set and which farm3d never
    writes, was left alone.

#### Backup through the real save dialog: Pending owner

The run could not drive the native save dialog from a shell. The backup
file used in this run came from `create_backup` with a fake dialog: 21 MB,
`media: all`, six Models, and one Project. Repro:

1. Run `just package`, then install the `.deb` (or run it extracted, with
   the isolated XDG variables above).
2. Open Settings, then Storage and backup, then Create backup, and choose
   a location in the save dialog.
3. Confirm that the file exists, is named `*.farm3d-backup`, and that the
   status line reports it without showing a path.

#### Restore through the real open dialog, restart, and banner: Partly verified

- **Verified on the bundle.** The run seeded a pending restore journal.
  On launch, the bundled binary installed the restore, and the journal
  moved `pending → installing → … → done` (`attempts: 1` on a clean run).
  The database afterwards had 6 `library_models`, 1 `library_projects`
  (the seeded name), and 6 `content_blobs`, and `PRAGMA integrity_check`
  returned `ok`.
- **Found and fixed: the outcome banner was never shown.** The spec
  ("Restore status banner") has `AppShell` show the outcome of a finished
  restore or reset until the operator acknowledges it. The backup store
  already had `loadRestoreStatus` and `restoreBanner`, but nothing in
  production called or rendered them. The first bundle showed no banner
  after a finished restore (screenshot of the Monitor, journal `done`).
  Commit `69f5a6b` wires the banner into `AppShell` and adds three tests
  in `AppShell.test.tsx`. On the rebuilt bundle, the same Farm opens with
  the banner "The restore finished. You can restore from safety backup
  sfb-…" and a Dismiss button.
- **Pending owner.** Repro:
  1. Open Settings, then Storage and backup, then Restore from file, and
     pick a backup in the native open dialog.
  2. Review the preview, then type the confirmation.
  3. Confirm that farm3d restarts by itself (`AppRestarter`) and opens
     with the banner.
  4. Click Dismiss. Confirm the banner goes away and does not come back on
     the next launch.

#### `kill -9` during install: Verified

The run launched the bundle on the Farm with a pending restore. A watcher
read `restore/journal.json` in a tight loop and sent `SIGKILL` when the
journal showed `phase: installing` and `step: extractContent`, 0.138 s
after launch. The steps the watcher saw were `pending`, `recheckBlockers`,
`markInstalling`, `moveDatabaseAside`, `placeCandidate`,
`carryLocalState`, and `extractContent`. The Farm had six STL models
totalling about 180 MB, which widens the window. After the kill, the
journal was left at `installing` with `attempts: 1`, and
`restore/<id>/previous/` held the moved-aside database.

On the next normal launch, the journal reached `phase: done`,
`step: markDone`, `attempts: 2`, and the log recorded
`restore.installed {"attempts":2,"rolledBack":true}`. The database had the
backup's counts (6 / 1 / 6), `PRAGMA integrity_check` returned `ok`, and
the content store held 6 blob files.

#### Diagnostics export and `grep` for seeded values: Verified through the command

The seeded values were:

- the Project name `t19SeedProjectZqx`;
- the Model names `t19SeedModelZqx-0` through `t19SeedModelZqx-5`;
- the source file names `t19seedpathZqx-*.stl`;
- the Farm's absolute paths (the scratch root and the user name);
- the restore journal id and safety backup id.

`export_diagnostics`, with all six sections, was run twice over the
isolated Farm:

- **Before the restore:** a 2,000-byte zip. The log was still empty, so
  there was no `logs/` entry.
- **After the bundle's restore:** a 2,428-byte zip with 7 entries,
  including `logs/farm3d.log` with the bundle's `restore.installed` line.

`grep -c -a` on each zip, and `grep -r -l` on the extracted entries,
found none of these strings, with a count of 0 in each: `t19SeedProjectZqx`,
`t19SeedModelZqx`, `t19seedpathZqx`, `Zqx`, the user name, the scratch
root, `scratchpad`, the case directory name, the journal id prefix, and
the safety backup id prefix.

Limits:

- The export ran through the library command with a fake save dialog. It
  did not run through the bundled binary.
- The P9 rig gives the diagnostics service a fixed fake home directory.
  So this run did not check that the real `$HOME` is redacted.

Pending owner repro:

1. In the bundle, open Settings, then Diagnostics, select every section,
   export, and save through the native dialog.
2. Run `unzip -l` on the zip. Then run
   `grep -a -c "<a Model name>\|<a Project name>\|$USER\|$HOME"` on the
   zip and on the extracted files. Every count should be 0.

#### Tier (c) reset: Install half verified on the bundle

A Farm with 6 Models, 1 Project, and a finished restore had a
`reset_farm` request seeded (`tier: farm`, `safetyBackup: true`,
`deleteSafetyBackups: false`). That wrote a pending `rsf-…` journal and a
safety backup. On launch, the bundle ran the reset: the journal reached
`kind: reset`, `phase: done`, and the log recorded
`reset.done {"attempts":1}`. Afterwards `library_models`,
`library_projects`, and `content_blobs` were all 0, the content store had
no files, and both safety backups were kept in
`farm3d-backups/v1/safety/`. The reset also cleared the old log files, as
tier (c) is designed to do; the new `farm3d.log` held only `reset.done`.

The Settings-side flow is pending owner:

1. Open Settings, then Reset, choose tier (c) with "Write a safety backup
   first" on, and type the confirmation phrase.
2. Confirm that farm3d restarts into an empty Farm, that the banner reads
   "The reset finished. You can restore from safety backup sfb-…", and
   that the safety backup is listed under Storage and backup.

#### Windows and macOS: Not verified

This run covered Linux x86_64 only.

### Observations (not fixed here)

- The bundle logged `slicing.presetCacheSweepFailed` (warn) once, on a
  normal launch after the restore. It did not happen on the other
  launches. This is P5 code (`sweep_stale_profile_caches`), and the run
  did not investigate it.
- `farm3d.log` and `restore/journal.json` are created with mode `0644`,
  while the database files are `0600`. The directories that hold them are
  `0700`, so no other user can read them.
- The banner fix pulls the backup store into the main chunk: `index-*.js`
  grows from 581.49 kB to 588.29 kB.
