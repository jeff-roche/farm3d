# P9 History, Settings, backup and restore, and diagnostics verification

**Date:** 2026-09-30
**Platform:** Linux x86_64 (Linux 7.2.8-1-cachyos), podman for the simulators.
**Validated source:** branch `feature/p9-portability`. `just build`,
`just test`, and `just test-rust` ran on `581bb2b` ("fix(ui): keep the
restore status banner out of the backup store") in this task. The
simulator suite ran earlier the same day on `162776c` ("style: apply cargo
fmt across the Rust crate"); the commits after it changed documentation,
the AppShell restore banner, and its store (no Rust behavior).
**Covers:** Tasks 1-20 of the P9 plan
(`docs/superpowers/plans/2026-09-29-p9-history-settings-backup-diagnostics.md`,
kept uncommitted), for GitHub issue #19. The binding spec is
[`docs/superpowers/specs/2026-09-29-p9-history-settings-backup-diagnostics-design.md`](../superpowers/specs/2026-09-29-p9-history-settings-backup-diagnostics-design.md),
with ADR-0016 (whole-Farm backup restored at startup) and ADR-0017
(safe-by-construction diagnostics log).

**Overall:** every automated gate passes on Linux x86_64: `just build`,
`just test`, `just test-rust`, the P9 tracer on the fakes, and the P9
simulator leg. The installed-bundle pass ran on the deb extracted into a
scratch directory (no system install) with isolated XDG directories, and
it found and fixed one real bug (the restore outcome banner was never
rendered). **Four steps that need a native dialog are pending the owner**
(backup save, restore open plus restart plus Dismiss, diagnostics export,
and a tier (c) reset from the Settings UI); their repro steps are in the
installed-bundle section. **Windows and macOS are unverified.** The
exit-gate item for the installed-bundle restore/restart therefore stays
partly open (see the exit gate below).

`CURRENT_SCHEMA_VERSION` is **10**, applied by `0010_p9_portability.sql`.
The backup format version is 1.

## Automated evidence

Each command ran with `source "$HOME/.cargo/env"` where it needs cargo.

| Command | Exit | Result |
| --- | --- | --- |
| `just build` | 0 | `tsc` and the Vite build pass. The main chunk is `index-CJ2Y42li.js` at 582.76 kB (177.54 kB gzip); Vite's 600 kB warning is not tripped by it (P8: 603 kB). `three-renderer` is a separate 556.79 kB lazy chunk. |
| `just test` | 0 | 146 files, 1902 tests passed. |
| `just test-rust` | 0 | 87 test-result lines: 2281 passed, 0 failed, 63 ignored (simulator and real-hardware tests). No flaky test needed a rerun. Compiler output: the pre-existing ts-rs "failed to parse serde attribute" warnings. |
| `just sim-up && just test-sim; just sim-down` | 0 | Run on 2026-09-30 against `162776c`. Every suite passed, including `sim_moonraker` at 25 passed, with `p9_a_restored_printer_reconnects_to_the_simulator_once_its_credential_is_entered_again`. The manifest is [`2026-09-30-p9-sim-manifest-20260930T122525Z.json`](../superpowers/baselines/2026-09-30-p9-sim-manifest-20260930T122525Z.json). This task did not start the simulators. |

The P9 test binaries in the `just test-rust` run: `p9_backup` 18,
`p9_diagnostics` 16, `p9_fixture_compat` 13 (1 ignored, the fixture
generator), `p9_history` 17, `p9_installer` 47, `p9_integrity` 33,
`p9_log` 14, `p9_migration` 6, `p9_reference_matrix` 1, `p9_reset` 21,
`p9_restore_preview` 53, `p9_secrets_corpus` 9, `p9_storage` 11, and
`p9_tracer` 1.

## Exit gate (issue #19)

| Criterion | Evidence | Status |
| --- | --- | --- |
| Versioned fixture backups and compatibility tests | `p9_fixture_compat.rs` (Task 17): 13 tests over `farm-v1-schema10.farm3d-backup` (31,696 bytes, deterministic generator). See "Fixture compatibility". | Pass |
| Restore failure injection and interrupted-restore recovery | `p9_installer.rs` (Task 7, 47 tests) and the tier (c) roll-forward in `p9_reset.rs` (Task 8, 21 tests). See "Fault injection". Also `kill -9` on the real bundle. | Pass |
| Post-restore reference counts and integrity checks | `p9_installer.rs`, `p9_fixture_compat.rs`, and `p9_tracer.rs` compare counts to the manifest and run `integrity::check`. On the bundle, counts matched and `PRAGMA integrity_check` returned `ok`. | Pass |
| Diagnostics contain none of the seeded credential corpus | `p9_diagnostics.rs`, `p9_secrets_corpus.rs`, and the tracer's step 6 (full corpus, no exceptions). A manual `grep` of a bundle-side export found none of the seeded values. | Pass |
| Settings and history UI tests | `just test`: 146 files, 1902 tests, including the Settings workspace, storage, diagnostics, restore preview, and history screens (Tasks 13-16). | Pass |
| Final archive/delete reference matrix | `p9_reference_matrix.rs` (Task 11) drives real commands over an every-domain Farm, and `p9_integrity.rs` audits every loose `*_json` and `_id` column. See "Reference matrix". | Pass |
| The tracer completes with secrets excluded, on the fakes and the simulator, with the manifest committed | `p9_tracer.rs` on the fakes (`just test-rust`), the simulator leg in `sim_moonraker.rs` (25 passed), and the committed manifest above. | Pass |
| `just package` and the installed-bundle restore/restart check on Linux x86_64 | `just package` passed on the bundle builds. Startup install, recovery, reset install, and the banner are verified on the extracted bundle. The native open dialog, in-app restart, and Dismiss are **pending owner**. | Partly verified; pending owner |

## The tracer (Task 18)

`src-tauri/tests/p9_tracer.rs` runs one test on the fakes:

1. An every-domain Farm, with the credential corpus in credentials,
   stored URLs, the telemetry cache header, freed pages, a driven
   connection error, a driven command error, and log lines.
2. `create_backup` with all media through the real command; the archive
   holds no forbidden value, and the stored camera URL is in the database
   copy only.
3. Drift: a renamed Printer, a Spool whose number clashes, a deleted
   Project, and a newly finished Job.
4. `preview_restore`: `onlyLocal`, `changed`, and `uniqueClash`
   conflicts, the credential, linked-path, and slicer notices, the
   counts, and an unresolved upload that shows as a blocker until it is
   discarded.
5. `apply_restore`, then the real startup on an empty credential store:
   counts equal the manifest (except the two carried tables), integrity
   is clean, and `supervise_printer` reports `CredentialRequired` for
   exactly the two credentialed Printers.
6. `export_diagnostics` with all six sections: the full corpus is absent
   from the bundle and the result.
7. `reset_farm` tier (c), restart, startup: integrity clean, counts equal
   a freshly migrated database, both safety backups kept.

The simulator leg is
`p9_a_restored_printer_reconnects_to_the_simulator_once_its_credential_is_entered_again`
in `sim_moonraker.rs`: a restored Printer reports `CredentialRequired`
with no Online status, and after the key is entered again it reaches
Online against the Moonraker simulator. The archive does not hold the
simulator's key.

## Reference matrix (Task 11)

`p9_reference_matrix.rs` builds the every-domain Farm in
`tests/p9_farm/` (active and archived Printers, Spools, a Project,
managed and linked Models with revisions and thumbnails, a Preparation,
farm3d and external Slice Revisions, open and closed Queue Entries, Jobs
with a correction, Reconciliation Requirements, an Incident, snapshots,
Attention Events with a recurrence, and Host Operations). It then drives
every archive, delete, restore, and reset command and checks that no
reference dangles. `the_audit_classifies_every_json_and_id_column_without_a_foreign_key`
in `p9_integrity.rs` finds each `*_json` column and each `TEXT` `_id`
column without a foreign key and requires it to appear once in
`LOOSE_COLUMNS`, so a new unclassified column fails the test. Task 11
also fixed `attentionSource` to accept any resolved Event of a deleted
Printer (`0c0a71d`).

## Fault injection (Tasks 7 and 11)

The installer has a test-only `Fault { step, point, effect, times }`
(`FaultPoint::Start`, `Within(n)`, `End`, and `Rollback(..)`;
`FaultEffect::Crash` and `Error(kind)`). `p9_installer.rs` injects a
crash or an error at every D8 step and checks the two-phase, resumable
rollback (the durable `journal.rollback` marker and
`swapMedia.liveExisted`), the retry-once path, and roll-forward after the
`installed` phase. `p9_reset.rs` does the same for the reset steps. The
spec's "Installer fault points" table is the list. The real-bundle
`kill -9` result is in the installed-bundle section.

## Fixture compatibility (Task 17)

`p9_fixture_compat.rs` stages the committed v1 fixture and each damaged
variant through the real preview path. Each is refused with the expected
code: a flipped byte (`checksumMismatch`, or `sizeMismatch` for a deflated
entry), a truncated file (`notAZip`), an extra unlisted entry
(`entryUnlisted`), a path traversal (`unsafePath`), a declared-size lie
(`sizeMismatch`), `formatVersion` 2 (`UNSUPPORTED_BACKUP_FORMAT`),
`schemaVersion` 99 (`UNSUPPORTED_SCHEMA_VERSION`), and an altered
migration checksum in the manifest or the database (`migrationMismatch`).
The clean fixture previews, applies, and installs with counts equal to the
manifest and a clean integrity check. The generator is deterministic
(the same SHA-256 on three runs; `schema_migrations.applied_at` is pinned).
There is no ratio guard for a decompression bomb: the spec's limits are
declared-size based and the read is capped (a controller ruling).

## Installed bundle (Linux x86_64, Task 19)

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
| Install the deb on Linux x86_64 | Verified (extracted, not installed; see the substitution above) |
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

### Observations (also listed under Known follow-ups)

- The bundle logged `slicing.presetCacheSweepFailed` (warn) once, on a
  normal launch after the restore. It did not happen on the other
  launches. This is P5 code (`sweep_stale_profile_caches`), and the run
  did not investigate it.
- `farm3d.log` and `restore/journal.json` are created with mode `0644`,
  while the database files are `0600`. The directories that hold them are
  `0700`, so no other user can read them.
- The banner's state lives in its own small `restore-status-store`, so
  the shell doesn't pull the backup store into the main chunk.
  `index-*.js` is 582.76 kB with the banner, against 581.49 kB before it.

## Screenshots

Captured in `just web` (frontend only, localhost:1420) with the web
fixtures, at 1440x900 and 1024x700, in `docs/screenshots/`. Web mode has
no backend, so these show fixture data, not a live Farm.

| State | Files |
| --- | --- |
| Settings, every category (General, Appearance, Slicing, Notifications and retention, Storage and backup, Connections, Diagnostics, About) | `p9-settings-<category>-<WxH>.png`, with `<category>` one of `general`, `appearance`, `slicing`, `notifications`, `storage`, `connections`, `diagnostics`, `about` |
| Diagnostics selection (the six sections, sizes, and the reset tiers below it) | `p9-settings-diagnostics-<WxH>.png` |
| History list | `p9-history-list-<WxH>.png` |
| History filters (Failed excluded) | `p9-history-filtered-<WxH>.png` |
| Job timeline (the Lid mount Job, with its Incident timeline entry) | `p9-job-timeline-<WxH>.png` |

**Not captured, by design of web mode:** the restore preview with
conflicts, and the typed confirmation. `web-fixtures.ts` has
`webRestorePreview()`, but it is never returned by web-mode IPC (a web
restore is `unsupported`, spec D18), so neither state can be reached in
`just web`. They are covered by `RestorePreviewPanel.test.tsx` and
`StorageSettings.test.tsx`, and they will be seen in the owner's native
restore pass. They were not faked.

Looking at the PNGs turned up two things, neither fixed here:

- In the Diagnostics section list, each section's name and its description
  run together with no gap ("AboutVersion, platform, and catalog
  information."). It is a layout defect in the checkbox label.
- The About and Storage fixtures show schema 11 (`web-fixtures.ts`),
  while the real `CURRENT_SCHEMA_VERSION` is 10. It is a fixture value,
  not a product defect.

## Residual risks (plan)

- **`AppHandle::restart` in the installed bundle.** Startup install and
  recovery run correctly on the bundle. The in-app restart after the
  native restore dialog is pending owner (checklist step 3 of "Restore
  through the real open dialog"). If restart proves unreliable,
  `apply_restore` should ask the operator to quit and reopen; the journal
  makes that equally safe.
- **Uncapped backup size.** Not changed. The free-space check prevents a
  half-written file, not a slow one. This task did not check for progress events; it is
  a follow-up if the owner finds a large Farm slow.
- **The egress scan uses exact matching.** It catches known values, not a
  transformed or partial leak. The typed log and pseudonymized collectors
  are the primary control, and the corpus includes URL-encoded and
  case-changed variants. **D13 residual-risk line:** a single-word name in
  the log's `fields.*` is exempt from the name scan, because a common word
  would give false positives. A Printer or Model named with one ordinary
  word can therefore appear in a log field of an exported bundle if code
  ever logs it there; the typed log is meant to make that impossible, but
  the scan would not catch it.
- **Linked-path portability is only surfaced.** After a restore onto
  another machine, linked Models are "missing" until **Locate source**.
  The preview counts them (`linkedPathsMissing`).
- **Printer delete still drops unlinked Spool movement history**
  (decision 20). Intentional and documented.
- **Printers import stays blocked once any Job exists** (decision 3).
  Whole-Farm restore replaces that workflow.

## Unverified platforms and pending owner steps

- **Windows and macOS: not verified.** F0 supports Linux x86_64 only.
- **Pending owner** (repro steps under "Installed bundle"): backup through
  the native save dialog; restore through the native open dialog with
  restart and Dismiss; diagnostics export through the native dialog,
  including redaction of the real home path; a tier (c) reset from the
  Settings UI.

## Known follow-ups

- `connections.cacheHydrateFailed` with `MalformedSnapshot` was logged
  during the P9 simulator leg. It is **uninvestigated**: it may be a
  legitimately malformed restored connection-cache snapshot, or a bug.
- `slicing.presetCacheSweepFailed` (warn) fires on a normal bundle launch.
  It is P5 code (`sweep_stale_profile_caches`).
- `farm3d.log` and `restore/journal.json` are created with mode `0644`,
  while the database is `0600`. Their directories are `0700`, so no other
  user can read them, but the modes should match.
- A single-word name in the log's `fields.*` is exempt from the name scan
  (see the D13 residual-risk line above).
- The Diagnostics section list needs a gap between each section name and
  its description (screenshots section).
- `web-fixtures.ts` reports schema 11 against the real 10.
- Deferred review minors are in
  `.superpowers/sdd/2026-09-29-p9-history-settings-backup-diagnostics/progress.md`
  (not committed).

## Documentation updated in this task

- `README.md`: "Your data, backups, and logs" (where data, backups, and
  logs live, and how to back up, restore, and export diagnostics).
- `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`:
  the "Backup format/conflict policy" and "Diagnostics redaction" rows are
  marked resolved (P9), with links.
- `CONTEXT.md`: unchanged. Backup, Restore preview, Safety backup, and
  Diagnostics bundle were added in Task 1 and no name has changed since.
- This record moved from
  `docs/superpowers/baselines/2026-09-30-p9-verification.md`.
