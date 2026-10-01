# P9 History, Settings, Backup/Restore, and Diagnostics Design

## Status

Approved by the controller, 2026-09-29, after two review rounds (issue #19 decision gate satisfied).

This is the focused design for GitHub issue #19 (P9). It is Task 1 of
`docs/superpowers/plans/2026-09-29-p9-history-settings-backup-diagnostics.md`.
It turns the plan's Design reference into final decisions and binds Tasks
2–20. Where this spec and the plan differ, this spec wins. Task 1 also
edited the plan wherever this spec changed a name or signature a later
task uses ("Decisions made in this spec" lists every change).

Two ADRs record the hard-to-reverse choices:

- ADR-0016 (`docs/adr/0016-whole-farm-backup-restored-at-startup.md`): a
  backup is one versioned, checksummed archive of the whole Farm; restore
  replaces the whole Farm and never merges; the replacement is installed
  at the next startup, before `Storage::open`, driven by an on-disk
  journal that makes every crash either finish or roll back.
- ADR-0017 (`docs/adr/0017-safe-by-construction-diagnostics-log.md`): the
  diagnostics log accepts only typed `LogSafe` fields, so free text can't
  compile into a log line; a diagnostics bundle is pseudonymized by its
  collectors, then egress-scanned against the live secret corpus before
  anything is written.

The owner's decisions 1–4 in the plan ("Owner decisions (fixed)") are
fixed, and the planner defaults 5–21 were confirmed as written on
2026-09-29. This spec applies them and does not reopen them:

- **Decision 1:** restore replaces the whole Farm. The preview lists
  `onlyLocal`, `changed`, and `uniqueClash` conflicts. Restore is refused
  while local work is in flight.
- **Decision 2:** a backup is the whole Farm plus optional camera media
  (`none`, `pinned`, `all`). Machine-specific paths are never restored.
  The Settings and Printers JSON exports stay unchanged.
- **Decision 3:** Job history is never pruned in v1.
- **Decision 4:** resets are tiered (settings, camera media, entire
  Farm), each behind typed confirmation. The entire-Farm tier runs
  through the restore journal and only rolls forward.
- **Decisions 5–21:** the archive format, checksums and archive safety,
  the compatibility window, consistency, install at the next startup,
  the refusal policy, machine-specific data, credentials on restore,
  safety backups, camera URLs in backups, logging, the diagnostics bundle
  and its threat model, reset mechanics, the history query, the Settings
  workspace, the P3 movement cascade, and Linux-only verification.

The umbrella spec's "Backup and restore" asks for a backup with
"selectable settings, Printers, Projects, managed Models, …". Owner
decision 2 narrows that on purpose: the Farm is always backed up whole,
and only camera media is selectable. The Settings and Printers JSON
exports remain the partial-portability path.

### The portability principle

> A backup is a point-in-time copy of the whole Farm, taken through
> SQLite's online backup API while a lease stops every file deletion. A
> restore never edits the live Farm. It stages and verifies the backup
> beside it, shows what would change, writes a safety backup, and asks
> farm3d to restart. The swap happens at the next start, before any
> connection, supervisor, or service exists, and an on-disk journal makes
> every crash either finish the swap or put the old Farm back. Nothing
> that leaves the machine (a diagnostics bundle) is built from free text:
> the log holds only typed fields, collectors emit pseudonyms, and a
> final scan refuses to write a bundle that contains any known secret,
> host, URL, path, or name.

## Goal

An operator can:

- Search and page through every finished Job, including Jobs on archived
  Printers, and open an immutable timeline that joins the Job's events,
  host work, reservation, material ledger, requirements, Incident,
  Attention Events, and surviving camera evidence.
- Manage farm3d from a **Settings workspace** on the activity rail, with
  eight categories, which replaces the gear menu and keeps theme preview.
- Back up the whole Farm to one `.farm3d-backup` file, with credentials,
  machine paths, and caches excluded.
- Preview a restore (counts, conflicts, notices, and blockers), then
  restore after an automatic safety backup, and trust that a crash
  mid-restore either finishes or rolls back and never leaves a mixed
  Farm.
- See disk use by class and reclaim what is safe to reclaim.
- Export a selectable diagnostics bundle that holds no credential, host,
  URL, path, or user-authored name.
- Reset settings, camera media, or the whole Farm deliberately, behind
  typed confirmation.

After P9, a final cross-domain matrix proves that no archive, delete,
prune, restore, or reset path leaves a dangling reference.

## Scope

### In scope

- Migration 0010: the history indexes, the `camera_snapshots` and
  `incident_events` rebuild that widens `prune_reason`, the
  `pending_credential_cleanup` rebuild that adds `reset`, and the
  `operations` rebuild.
- `persistence::integrity`, the single reference-integrity checker.
- The `history` read model and its two commands.
- The `backup` module: the archive format, manifest, writer, lease,
  inventory, safety backups, restore staging, preview, the restore
  journal, and the startup installer.
- The `diagnostics` module: the typed log with rotation, pseudonyms, the
  bundle collectors, the egress scan, storage usage and cleanup, tiered
  reset, and About.
- 18 new commands, 12 new error codes, one new recovery code, and one new
  navigation selection kind.
- Frontend: the Settings workspace and its eight categories, the history
  view and Job timeline, the restore preview, storage, diagnostics, and
  reset panels, vertical `Tabs`, and `TypedConfirmDialog`.
- Versioned fixture backups, the reference matrix, the tracer on the
  in-process fakes (CI) and the simulator, and the installed-bundle pass.

### Non-goals

- Merge restore, per-domain backups, or selecting domains to back up
  (decisions 1 and 2).
- Job history pruning (decision 3).
- Signing or encrypting backups; cloud, scheduled, or incremental
  backups.
- Backup progress events and cancellation (a residual risk).
- Moving storage roots to another disk.
- Log upload, crash reporting, or telemetry.
- Any new printer capability or product-domain behavior (issue #19).
- Verification on Windows and macOS (decision 21).

## Product vocabulary

`CONTEXT.md` gains these entries (Task 1 wrote them):

- **Backup** (new): one `.farm3d-backup` file holding a point-in-time
  copy of the whole Farm and, optionally, its camera media. Credentials,
  machine paths, and caches are never in it. _Avoid_: Snapshot (a camera
  frame), export (the Settings and Printers JSON files).
- **Safety backup** (new): a Backup farm3d writes by itself, with all
  camera media, before a restore or an entire-Farm reset. The newest
  three are kept.
- **Restore preview** (new): what a restore would do, computed from a
  staged, verified copy of a Backup: per-table counts, conflicts, notices,
  and blockers. It changes nothing in the live Farm.
- **Diagnostics bundle** (new): a zip of selectable, pseudonymized
  sections for a support thread. It never holds a credential, host, URL,
  path, or user-authored name.
- **Reset** (new): a deliberate, typed-confirmed return of settings,
  camera media, or the whole Farm to an empty state.

## Decision gate

The umbrella P9 decision gate names ten items. Each has a named decision
here.

| Gate item | Decision | Section |
|---|---|---|
| Archive format | One zip, `farm3d-<UTC>.farm3d-backup`: `manifest.json` first, then the sanitized database, content blobs, and selected media; `formatVersion` 1 | D2 |
| Checksums | SHA-256 of every entry's uncompressed bytes in the manifest; verified while streaming at preview, and again as each staged file is installed; content entries must hash to their own name | D3 |
| Compatibility window | `formatVersion` 1; `schemaVersion` 10 through the current version; older schemas migrated on the staged copy; newer refused | D4 |
| Linked-path portability | `linked_path` and `source_path` restored verbatim; a path missing here shows through P4's source state and **Locate source**; the preview counts them | D10 |
| Conflict identity | Same id across local and backup (compared by every column of the root row), plus four natural keys: active host identity, Spool number, Project name, tare name | D6, "Conflict fixtures" |
| Merge/replace policy | Replace only: the database is swapped whole, so conflicts are informational and nothing merges | D6 (ADR-0016) |
| Safety-backup cleanup | Same format with `media: all`, in `farm3d-backups/v1/safety/`; newest 3 kept; older ones deleted only after a new one verifies | D9 |
| Log rotation | `farm3d.log` rotates at 2 MiB; the active file plus four rotated files are kept | D12 (ADR-0017) |
| Redaction threat model | A third-party reader; collectors emit only typed facts and pseudonyms; an egress scan over the live corpus refuses the whole bundle on any hit | D13 (ADR-0017) |
| Partial-reset recovery | Tier (a) is one transaction after a pre-reset snapshot; tier (b) follows P8's prune order; tier (c) is journaled and every step is idempotent, so a crash rolls it forward | D15, "Installer fault points" |

## Decisions

### D1. Vocabulary and ownership

**Decision: three new Rust modules and one persistence helper own P9.
Rust owns every conflict, blocker, redaction, reset scope, and cleanup
decision. TypeScript presents them.**

| Owner | Owns |
|---|---|
| `persistence::integrity` | The reference-integrity catalogue and `check` (D17) |
| `history` | The Job history query and the Job timeline (D11). Reads only |
| `backup` | The archive format and manifest, the writer, `BackupLease`, the inventory, safety backups, restore staging and preview, the restore journal, the startup installer, the process-local operation ledger |
| `diagnostics` | The typed log and rotation, pseudonyms, the bundle collectors, the egress scan, storage usage and cleanup, the three reset tiers, About |
| `library::content`, `cameras` (P4, P8) | Unchanged logic, except that both honor `BackupLease` (D5) and camera pruning accepts the reasons `reset` and `notInBackup` |
| `settings` (F1) | Unchanged logic. Tier (a) reuses its snapshot-then-transaction path |

- **Staging** ids are `stg-<uuid v4>`. **Journal** ids are `rst-<uuid
  v4>` for a restore and `rsf-<uuid v4>` for an entire-Farm reset.
  **Safety backup** ids are `sfb-<uuid v4>`.
- No P9 table is added. Backups, staging, and journals live on disk,
  because they must survive the database being swapped.

### D2. Archive format (ADR-0016)

**Decision: one zip file per backup, `formatVersion` 1, with a closed
JSON manifest as its first entry.**

- **File name.** An operator backup is suggested as
  `farm3d-<yyyymmddThhmmssZ>.farm3d-backup` (UTC) in the native save
  dialog. A safety backup is `<backupId>.farm3d-backup` under
  `<backup_root>/safety/`.
- **Entries, in this order:**

  | Entry | Contents | Compression |
  |---|---|---|
  | `manifest.json` | The manifest (below), UTF-8 JSON | deflate |
  | `database/farm3d.sqlite3` | The sanitized, `VACUUM`ed online-backup copy (D5) | deflate |
  | `content/sha256/<hh>/<hex>` | One entry per `content_blobs` row, sorted by `<hex>` | stored when the blob is a 3MF (`model_source_revisions.format = '3mf'` or `slice_revision_blobs.role = 'plate3mf'`), else deflate |
  | `media/<rel_path>` | One entry per selected camera snapshot, sorted by path; `rel_path` is `snapshots/<yyyy>/<mm>/<snp-id>.<jpg\|png>` | stored |

- Every entry is a regular file. The writer never writes a directory
  entry, a symlink, an encrypted entry, or an extra field beyond what the
  `zip` crate writes for a stored or deflated file. Every entry's
  modification time is the manifest's `createdAt` (UTC, DOS precision),
  so a generated fixture is byte-stable.
- **The manifest** is a closed object: an unknown field is
  `BACKUP_INVALID` (`manifestInvalid`). Any addition bumps
  `formatVersion`.

  ```json
  {
    "format": "farm3d-backup",
    "formatVersion": 1,
    "createdAt": "2026-09-29T12:00:00.000Z",
    "appVersion": "0.1.0",
    "schemaVersion": 10,
    "migrations": [
      { "version": 1, "name": "0001_foundation", "checksum": "<64 hex>" }
    ],
    "platform": { "os": "linux", "arch": "x86_64" },
    "origin": "operator",
    "contents": { "media": "all" },
    "counts": { "attention_events": 4, "camera_snapshots": 3 },
    "excluded": ["credentials", "slicerRuntimePaths", "printerStatusCache",
                 "pendingCredentialCleanup", "pendingBlobCleanup", "logs"],
    "credentialRefCount": 2,
    "media": { "included": 2, "notInBackup": 1, "missingFile": 0 },
    "entries": [
      { "path": "database/farm3d.sqlite3", "bytes": 245760, "sha256": "<64 hex>" }
    ]
  }
  ```

  - `migrations` lists every row of the copy's `schema_migrations`,
    ascending by version.
  - `platform.os` and `platform.arch` are Rust's `std::env::consts::OS`
    and `ARCH`. Nothing else about the machine is recorded.
  - `origin` is `operator`, `beforeRestore`, or `beforeReset`.
  - `counts` has one key per user table in the copy (every
    `sqlite_schema` table not named `sqlite_%`), sorted by name, with the
    copy's exact `count(*)`.
  - `excluded` is exactly the six classes above, in that order.
  - `credentialRefCount` is the number of distinct
    `$.credentialRef` values in `printers.connection_json`.
  - `media` counts the camera snapshot rows the writer handled: included
    as entries, marked `notInBackup`, or marked `missingFile` (D5).
  - `entries` lists every entry except `manifest.json`, in archive order.
    `bytes` is the uncompressed length; `sha256` is the lowercase hex
    SHA-256 of the uncompressed bytes.

### D3. Checksums and archive safety

**Decision: the threat model is corruption, truncation, and a hostile or
malformed file. Tampering by someone who can also rewrite the manifest is
out of scope: the archive is not signed.**

A reader (preview, and the safety-backup verifier) applies these rules in
order. Each failure is `BACKUP_INVALID` with `details.reason` and a safe
`details.fieldPath`, and never echoes an entry's bytes or an unsafe path.

1. The file opens as a zip and its central directory reads; otherwise
   `notAZip` (fieldPath `archive`). A truncated file fails here.
2. The central directory has at most 1,000,001 entries (the manifest plus
   1,000,000); otherwise `tooManyEntries`.
3. Every entry name passes the path rules: UTF-8, not empty, at most 512
   bytes, relative (no leading `/`, no drive letter), no `\`, no `..`
   or `.` segment, no empty segment, no control character (U+0000–U+001F,
   U+007F), and a first segment of `manifest.json`, `database`,
   `content`, or `media`. A failure is `unsafePath` with fieldPath
   `entries[<central-directory index>]`; the name itself is never
   reported.
4. Every entry is a regular file, unencrypted, stored or deflated;
   otherwise `unsupportedEntry`. A duplicate name is `duplicatePath`.
5. `manifest.json` exists (`manifestMissing`), is at most 64 MiB
   uncompressed (`manifestTooLarge`), and parses into the closed schema
   (`manifestInvalid`, fieldPath the JSON path, for example
   `manifest.entries[3].sha256`). `format` must equal `farm3d-backup`.
6. The compatibility window (D4) is checked next, so a newer backup is
   reported as newer rather than as malformed.
7. The manifest's `entries` paths are unique and each also passes rule 3;
   `database/farm3d.sqlite3` appears exactly once; every `content/` path
   is `content/sha256/<hh>/<hex>` with `<hex>` 64 lowercase hex digits
   and `<hh>` its first two; every `media/` path is `media/` plus a valid
   `rel_path`.
8. The archive's entries (other than `manifest.json`) and the manifest's
   `entries` are the same set: an archive entry missing from the manifest
   is `entryUnlisted`, a manifest entry missing from the archive is
   `entryMissing`.
9. The sum of `bytes` over every entry that will be staged, plus 10 %,
   fits the free space of the root it will be staged under (D8
   "Staging"); otherwise `INSUFFICIENT_SPACE` (not `BACKUP_INVALID`).
10. Each entry is streamed once. Its decompressed stream is cut off at its
    declared `bytes` plus one byte, so a zip bomb or a lying header stops
    there: more bytes than declared, or fewer, is `sizeMismatch`. The
    streaming SHA-256 must equal the manifest's (`checksumMismatch`). A
    content entry's hash must also equal the `<hex>` in its name
    (`contentNameMismatch`). The zip crate's CRC-32 check also applies.
11. The staged database is validated as D4 describes (`migrationMismatch`,
    `countMismatch`, `databaseInvalid`).

`BackupInvalidReason` is exactly: `notAZip`, `tooManyEntries`,
`unsafePath`, `unsupportedEntry`, `duplicatePath`, `manifestMissing`,
`manifestTooLarge`, `manifestInvalid`, `entryUnlisted`, `entryMissing`,
`sizeMismatch`, `checksumMismatch`, `contentNameMismatch`,
`migrationMismatch`, `countMismatch`, `databaseInvalid`.

Verification happens twice. Preview streams and verifies every entry as
it stages it (rule 10). The installer hashes every staged file again as
it installs it (D8), so a staged file changed between preview and restart
is caught before the swap completes.

### D4. Compatibility window

**Decision: accept `formatVersion` 1 and any `schemaVersion` from 10
through `CURRENT_SCHEMA_VERSION`.**

- 10 is the first schema a backup can carry (P9's own).
- `formatVersion` greater than 1 is `UNSUPPORTED_BACKUP_FORMAT`; less
  than 1 or missing is `BACKUP_INVALID` (`manifestInvalid`).
- `schemaVersion` greater than the binary's is
  `UNSUPPORTED_SCHEMA_VERSION` (`details.receivedVersion`,
  `details.supportedVersion` = `CURRENT_SCHEMA_VERSION`); less than 10 is
  `BACKUP_INVALID` (`manifestInvalid`, fieldPath
  `manifest.schemaVersion`).
- Every manifest `migrations` row must equal the binary's embedded
  migration of that version (name and checksum), and the staged copy's
  `schema_migrations` rows must equal the manifest's list exactly;
  otherwise `migrationMismatch`. The staged copy's `user_version` must
  equal `schemaVersion`.
- Before migrating, the staged copy's per-table counts must equal
  `manifest.counts`, and its table set must equal the manifest's keys
  (`countMismatch`, fieldPath `manifest.counts.<table>`).
- An older schema is migrated forward **on the staged copy** with
  `migrations::apply`, the same code the live database uses. The live
  database is untouched until install.
- Then the staged copy must pass `PRAGMA integrity_check` (exactly `ok`),
  `PRAGMA foreign_key_check` (no rows), and `integrity::check` with no
  violation (roots: none, since its files are staged, not installed);
  otherwise `databaseInvalid`.
- Every committed fixture backup keeps restoring for as long as its
  `formatVersion` is accepted (Task 17). A fixture is never regenerated;
  a new one is added.

### D5. Consistency, the lease, and sanitization

**Decision: a backup is the Farm at the instant of one online-backup
copy. A process-wide `BackupLease` stops every blob and media deletion
while any P9 file operation runs, so every file the copy lists still
exists when it is read.**

**The lease.** `BackupLease` is one process-wide, exclusive, in-memory
lease (`backup::lease`). It is held by `create_backup`, `preview_restore`,
`apply_restore`, `reset_farm` (every tier), `clear_storage`, and
`delete_backup` for their whole run; `apply_restore` and tier (c) keep it
until the process exits for the restart. A second holder gets
`BACKUP_IN_PROGRESS` with `details.activity` naming the current holder
(`backup` for `create_backup`, `restorePreview`, `restoreApply`, `reset`,
`storageCleanup` for `clear_storage`, `backupDelete` for
`delete_backup`). While it is held:

- `ContentStore::release_unreferenced` returns without unlinking. Its
  `pending_blob_cleanup` rows stay and are retried.
- The `MediaJanitor`'s prune pass is skipped.
- A capture still runs. When it must prune for the disk cap, it marks the
  rows pruned in its transaction exactly as P8 does, but queues their
  file unlinks in memory instead of unlinking after commit.
- Dropping the lease runs `release_unreferenced` once, unlinks the queued
  media files, and pokes the `MediaJanitor`. A crash loses the queue
  harmlessly: P8's startup sweep deletes image files with no unpruned
  row, and P4's deletes blob files with no row.

Both deferrals are safe because P4 and P8 already treat cleanup as
deferred and retried.

**The writer** (`backup::writer`, used by `create_backup` and by every
safety backup):

1. Take the lease (a safety backup runs under its caller's lease:
   `apply_restore` or `reset_farm` already holds it). Estimate the size (the live database's
   `page_count × page_size`, the sum of `content_blobs.size_bytes`, and
   the selected snapshots' `byte_len`). The estimate plus 10 % must fit
   the destination's free space; otherwise `INSUFFICIENT_SPACE`.
2. Copy the live database with the online backup API
   (`Storage::create_snapshot_to(path)`, F1's machinery generalized) into
   a private working directory, `<backup_root>/tmp/<uuid>/farm3d.sqlite3`.
3. From the copy, read the `content_blobs` rows and the selected
   snapshot rows: `none` selects nothing; `pinned` selects unpruned rows
   with `pinned_at` set; `all` selects every unpruned row.
4. **Media pre-pass.** Read each selected file from `media_root`; a file
   that is missing, has another length than `byte_len`, or another
   SHA-256 than `sha256` is **missing**. Media files are immutable and the
   lease stops unlinks, so what this pass saw is what step 7 reads.
5. **Sanitize the copy** in one transaction, then `VACUUM` it, so no
   freed page keeps an old value:
   - `UPDATE slicer_runtime_config SET engine_path = NULL,
     preset_source_path = NULL`;
   - `DELETE FROM printer_status_snapshots` (a telemetry cache);
   - `DELETE FROM pending_credential_cleanup` (refs into this machine's
     store);
   - `DELETE FROM pending_blob_cleanup` (files on this machine);
   - every unpruned `camera_snapshots` row that is not selected gets
     `pruned_at = createdAt`, `prune_reason = 'notInBackup'`, and
     `revision = revision + 1`;
   - every selected row found missing in step 4 gets the same with
     `prune_reason = 'missingFile'`.

   No `incident_events` row is appended: the backup's Incident timelines
   stay exactly as they were, and the snapshot row carries its pruned
   state (the history immutability rule).
6. Validate the copy: `integrity_check`, `foreign_key_check`, and
   `integrity::check` with the live roots (every listed blob must exist
   and every still-unpruned row must have its file). A violation means
   the local Farm is damaged: the backup fails with
   `BACKUP_SOURCE_DAMAGED` (`details.entry` is the archive path of the
   first missing or damaged file, or `database`). Then hash the copy and
   read its counts and migrations.
7. Build the manifest (all hashes are now known: the copy's, each blob's
   own name, each included media row's `sha256`), then stream the zip
   into a random same-directory temporary file next to the destination:
   `manifest.json`, the database, each blob (hashing as it copies), each
   included media file. A blob whose bytes don't hash to its name, or
   whose file is gone, aborts the backup with `BACKUP_SOURCE_DAMAGED`.
8. `fsync` the file, atomically replace the destination
   (`document_io::atomic_replace`), `fsync` the directory, delete the
   working directory, and drop the lease. Any failure deletes the
   temporary file and the working directory; the destination is never
   left half-written.

A backup taken while Jobs are active is allowed. Their rows are copied as
they are; D7 explains what a restore does with them.

### D6. Conflict identity and the replace-only policy (ADR-0016)

**Decision: restore swaps the database whole, so there is nothing to
merge and referential integrity holds by construction. Conflicts are
computed only to tell the operator what the swap will change.**

Classification compares the live database (one read transaction) with
the staged, migrated candidate (a read-only connection). It covers these
**domains**, each a root table with its own id:

| Domain (wire) | Table | Id | Natural key |
|---|---|---|---|
| `settings` | `settings` | the singleton: `localId` and `backupId` are both the string `"settings"` | — |
| `printer` | `printers` | `id` | `host_identity`, only where `archived_at IS NULL AND host_identity IS NOT NULL` (the `printers_active_host_identity` index) |
| `spool` | `spools` | `id` | `spool_number` |
| `tare` | `spool_tares` | `id` | `lower(name)`, SQLite's ASCII-only `lower()` (the `spool_tares_name` index) |
| `project` | `library_projects` | `id` | `lower(name)`, ASCII-only (the `library_projects_name` index) |
| `model` | `library_models` | `id` | — |
| `sliceRevision` | `slice_revisions` | `id` | — |
| `queueEntry` | `queue_entries` | `id` | — |
| `job` | `jobs` | `id` | — |
| `incident` | `incidents` | `id` | — |
| `attentionEvent` | `attention_events` | `id` | — |
| `snapshot` | `camera_snapshots` | `id` | — |

**Rules** (the same for every domain):

1. A row whose id is on both sides is **unchanged** when every column of
   the root row is equal (same SQLite type and value, in column order),
   and **`changed`** otherwise. A revision or `updated_at` difference
   alone is a change. Child rows (slots, cameras, blobs, events) are not
   compared; their differences show in the per-table counts.
2. A local row whose id is not in the backup is **`uniqueClash`** when
   the domain has a natural key, the row has one, and a backup row with a
   different id has the same key. The item names both ids. Otherwise the
   row is **`onlyLocal`**: it will be removed, and the safety backup
   keeps it.
3. A backup row whose id is not local is never a conflict. It is counted
   as added in the per-table counts. A backup row that clashes with a
   local row is reported once, from the local side (rule 2).
4. Tables that aren't domains (every child table, the ledgers,
   `operations`, `schema_migrations`, `migration_warnings`,
   `legacy_imports`) are only counted. `printer_status_snapshots`,
   `pending_credential_cleanup`, and `pending_blob_cleanup` are excluded
   from the backup (D5), so the candidate always has zero of each;
   `slicer_runtime_config` is kept local (D10) and gets a notice instead.

**Grouping.** Conflicts are grouped by class (`onlyLocal`, `changed`,
`uniqueClash`, in that order), then by domain in the table's order. Each
group carries its `total` and at most 200 items, ordered by local id
(bytewise), then backup id. Each item has a `label`: the Printer name,
`#<spool number>`, the tare, Project, or Model name, `Settings`, or the
row id for the other domains. The label comes from the local row when
there is one, else from the backup row.

The fixture table in "Conflict fixtures" is the contract.

### D7. Refusal policy

**Decision: restore is refused while the local Farm has work in flight;
a backup is never refused for it.**

- **Blockers** are, in the live database:
  - `activeJob`: a Job whose `state` is not `completed`, `failed`, or
    `cancelled` (so `outcomeUnknown` blocks);
  - `hostOperation`: a Host Operation in `dispatching`, `uncertain`, or
    `reconciling`;
  - `sliceOperation`: a slice operation in `queued` or `running`.

  Replacing any of these would orphan live host or slicer work.
- The preview lists the blockers (at most 50, ordered by kind in that
  order, then id, with `blockerTotal`). `apply_restore` re-checks them
  inside its own read transaction and refuses with `RESTORE_BLOCKED`
  (`details.blockers`, `details.blockerTotal`, `recovery: [OPEN_QUEUE]`).
- The installer checks them once more before it moves anything (D8
  `recheckBlockers`). If the check fails there, the journal becomes
  `failed` with code `RESTORE_BLOCKED` and nothing is moved.
- A backup taken while Jobs were active restores those Jobs as they
  were. The preview says so (the `activeJobsAtBackup` notice), and
  farm3d reconciles them with their Printers at startup through P7's
  restart recovery (`jobs::recover_after_restart`), exactly as after a
  normal restart. Queued or running slice operations in the backup are
  interrupted by P5's startup recovery, unresolved Host Operations
  become `uncertain` through P6's, and P8's startup backfill re-projects
  the restored Farm's Attention Events (ADR-0014: the backfill is the
  same code path, so no Event is duplicated).
- Tier (c) reset is **not** refused for work in flight; its preview warns
  instead (D15).

### D8. The restore journal and the startup installer (ADR-0016)

**Decision: `apply_restore` never touches the live database. It writes a
safety backup and a journal, then restarts. At the next start, after the
ownership lease and before `Storage::open`, `backup::installer::run`
swaps the Farm, validates it, and either commits the swap or rolls it
back. Any crash is recovered by the same installer on the next start.**

#### Storage paths

`StoragePaths` gains two roots (`persistence/database.rs`):

- `log_root`: `StoragePaths::new(metadata_root, app_data_root)` keeps its
  signature and sets `log_root = <app_data_root>/logs`. Production then
  calls `.with_log_root(app.path().app_log_dir())`, which replaces it and
  re-runs the collision checks. On Linux both are the same directory.
- `backup_root = <app_data_root>/farm3d-backups/v1`, with `safety/` and
  `tmp/` below it.

Both join the tree collision checks: `log_root`, `backup_root`,
`legacy_root`, `snapshot_root`, `content_root`, and `media_root` must be
pairwise neither equal nor nested, and none may contain `database`.

The restore layout:

| Path | Holds |
|---|---|
| `<metadata_root>/restore/journal.json` | The one journal (below) |
| `<metadata_root>/restore/<journalId>/previous/` | The live database set moved aside during an install |
| `<snapshot_root>/.restore-staging/<stagingId>/` | `candidate.sqlite3` (the staged, migrated database) and `manifest.json` (a copy of the backup's manifest) |
| `<content_root>/staging/restore-<stagingId>/<hh>/<hex>` | Staged content blobs (same filesystem as `content_root`, so install is a rename) |
| `<media_root>/restore-<stagingId>/snapshots/…` | Staged media |
| `<media_root>/restore-<stagingId>/previous-snapshots/` | The live `snapshots/` directory moved aside during an install |

#### Staging

`preview_restore` takes the lease, removes any earlier staging (at most
one exists per process), creates `stg-<uuid>`, and streams the archive
once (D3 rule 10): the database entry to `candidate.sqlite3`, content to
the content staging directory, media to the media staging directory, each
through a same-directory `.part` file that is fsynced and renamed when its
hash matches. It then validates and migrates the candidate (D4), writes
`manifest.json` beside it, computes the preview (D6, D7, D10), records
`createdAt`, and drops the lease.

- A staging expires 24 hours after `createdAt`. `apply_restore` on an
  expired or unknown staging id is `RESTORE_STAGING_EXPIRED`.
- `discard_restore_preview` removes all three staging directories.
- At startup, the installer removes every staging directory that no
  journal in phase `pending` or `installing` names (all three kinds), and
  F1's `cleanup_restore_staging` is changed to do the same instead of
  keeping every valid candidate. P4's content sweep already empties
  `staging/`.

#### `apply_restore`

1. `confirmation` must be exactly `restore` (no trim, no case folding);
   otherwise `CONFIRMATION_MISMATCH`.
2. Look up `operationId` in the process-local ledger (D18).
3. The staging must exist and not be expired (`RESTORE_STAGING_EXPIRED`).
4. Take the lease (`BACKUP_IN_PROGRESS`). Step 6's `RESTART_PENDING`
   refusal runs before this step, so a second apply while the first
   still holds the lease is refused `RESTART_PENDING` (fault row f25).
5. Re-check the blockers (`RESTORE_BLOCKED`).
6. If a journal in phase `pending`, `installing`, or `installed` exists
   (another restore or reset is waiting for the restart), refuse with
   `RESTART_PENDING`. If a finished journal (`done` or `failed`) is
   present, acknowledge it (delete it and its directory).
7. Write a safety backup with `origin: beforeRestore` (D9). Any failure
   stops here; nothing else has changed.
8. Read, in one live read transaction, what the installer must carry
   into the new database: the `slicer_runtime_config` row, every
   `pending_credential_cleanup` row, and every local credential ref
   (from `printers.connection_json` and `pending_credential_cleanup`).
   The **orphan refs** are the local refs the candidate's Printers don't
   reference. A local `pending_credential_cleanup` row whose ref the
   candidate's Printers **do** reference is **dropped** from the carry:
   the restored Farm uses that credential again, and carrying an
   automatic reason (`printer_deleted`, `cleared`, …) would let F1's
   startup retry delete it. Only rows for refs the candidate doesn't
   reference are carried, verbatim.
9. Compute `expectedCounts`: the candidate's per-table counts.
10. Write the journal with phase `pending` (atomically: temp file,
    `fsync`, rename, `fsync` the directory).
11. Record the result in the ledger, keep the lease (so no other P9
    operation starts before the restart), return
    `{ status: "restarting", safetyBackupId }`, and request the restart
    through the injected `Restarter` 500 ms later, so the response reaches
    the frontend first. Production's `Restarter` calls
    `AppHandle::restart()`.

#### The journal

`<metadata_root>/restore/journal.json`, written only with the atomic rule
above. It holds refs and local paths, never a credential value, and never
leaves the machine.

```ts
type RestoreJournal = {
  journalVersion: 1;
  id: string;                          // rst-… (restore) or rsf-… (reset)
  kind: "restore" | "reset";
  createdAt: string;
  phase: "pending" | "installing" | "installed" | "done" | "failed";
  step: InstallerStep | null;          // the step being run; written before it starts
  attempts: number;                    // restore: 0 until the first markInstalling; at most 2
  stagingId: string | null;            // restore only
  safetyBackupId: string | null;       // null only for a reset without a safety backup
  expectedCounts: Record<string, number> | null;          // restore only
  carry: {                             // restore only
    slicerRuntime: { enginePath: string | null; presetSourcePath: string | null } | null;
    pendingCredentialCleanup: {
      credentialRef: string; printerId: string | null; reason: string;
      attemptCount: number; lastErrorCode: string | null;
      createdAt: string; lastAttemptAt: string | null;
    }[];
  } | null;
  orphanCredentialRefs: string[];      // restore: queued as import_orphan; reset: deleted from the store
  swapMedia: { liveExisted: boolean } | null;   // restore: written at the start of swapMedia, before any rename
  rollback: {                          // restore: written before a rollback touches anything
    from: InstallerStep;               // the step that crashed or failed
    mediaRestored: boolean;            // set once rollback phase (a) is durable
    candidateCleared: boolean;         // set once rollback phase (b) is durable
  } | null;
  reset: { deleteSafetyBackups: boolean } | null;         // reset only
  outcome: { finishedAt: string } | null;                 // set with phase "done"
  failure: { code: ErrorCode; step: InstallerStep | null; finishedAt: string } | null;
};
type InstallerStep =
  | "recheckBlockers" | "markInstalling" | "moveDatabaseAside" | "placeCandidate"
  | "carryLocalState" | "extractContent" | "swapMedia" | "validate" | "markInstalled"
  | "moveRootsAside" | "createFreshDatabase" | "deleteCredentials" | "deleteSafetyBackups"
  | "removePrevious" | "markDone";
```

- Legal transitions: `pending → installing` (restore), `pending →
  failed` (blocked, staging gone), `installing → pending` (rolled back,
  one retry left), `installing → failed`, `installing → installed`,
  `installed → done`. A reset goes `pending → installing → done`. `done`
  and `failed` are final; `acknowledge_restore_status` deletes the file.
- `failure.code` is one of `RESTORE_BLOCKED`, `RESTORE_STAGING_EXPIRED`,
  `BACKUP_INVALID`, `INSUFFICIENT_SPACE`, or `RESTORE_FAILED`.
- An unreadable journal, or one whose `journalVersion` isn't 1, stops
  startup with `RESTORE_FAILED` (`details.reason` `journalUnreadable` or
  `journalVersion`) and touches nothing.

#### Startup order

F1's startup order gains the installer and the log:

1. Resolve and validate `StoragePaths`, including `log_root` and
   `backup_root`.
2. Acquire the metadata-root lease. **When `restore/journal.json`
   exists**, a busy lock is retried every 100 ms for up to 10 s before
   it counts as contention, because the process that requested the
   restart may still be exiting. Without a journal, F1's immediate
   failure is unchanged.
3. `backup::installer::run(&paths, &lease, open_credentials)`, where
   `open_credentials: impl FnOnce() -> CredentialStore` (production:
   `|| CredentialStore::detect(metadata_root)`; tests: a fake). The
   installer calls it only for a reset's `deleteCredentials` step, so the
   store is opened after the lease, as F1 requires, and never for a
   restore. It returns an `InstallReport`, or a
   `RESTORE_FAILED` error that becomes the bootstrap `Failed` state; the
   recovery UI's retry re-runs the installer, then continues.
4. Initialize the log (`diagnostics::log::init(paths.log_root())`), then
   log the `InstallReport`. The log starts after the installer because a
   reset moves `log_root`.
5. `Storage::open`, and F1's remaining steps unchanged, including P4's
   content sweep, P5, P6, and P7 recovery, and P8's backfill and media
   sweep.

#### Installer steps (restore)

Each step records itself in `journal.step` before it starts, so recovery
knows what the files on disk are.

1. `recheckBlockers` (phase `pending`). Open the live database read-write
   with `SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE` and `query_only = ON`, run the
   D7 blocker query in one read transaction, and close. The main file
   isn't written and no checkpoint runs; opening can still create an
   empty `-wal` and a new `-shm` beside a cleanly closed database, which
   the byte-exact rule below allows for. Blocked → phase `failed`
   (`RESTORE_BLOCKED`). The staging is gone → `failed`
   (`RESTORE_STAGING_EXPIRED`). Either way nothing is moved.
2. `markInstalling`: phase `installing`, `attempts += 1`.
3. `moveDatabaseAside`: rename `farm3d.sqlite3`, `farm3d.sqlite3-wal`,
   and `farm3d.sqlite3-shm` (each only if present) into
   `restore/<id>/previous/`, in that order, then `fsync` both
   directories. The three always move as a set: recovery puts back a
   partial move before anything opens the database.
4. `placeCandidate`: copy `candidate.sqlite3` to
   `<metadata_root>/.farm3d-restore-candidate.partial`, `fsync`, rename it
   to `farm3d.sqlite3`, `fsync` the directory. The staged candidate stays,
   so a retry can place it again. The candidate has no `-wal` or `-shm`;
   if one exists beside it, the step fails.
5. `carryLocalState`: one transaction on the placed database:
   - if `carry.slicerRuntime` is not null, write its two paths into the
     `slicer_runtime_config` singleton (inserting it if absent), with
     `revision = revision + 1` and `updated_at = now`;
   - insert every `carry.pendingCredentialCleanup` row, and an
     `import_orphan` row (`printer_id` NULL) for every
     `orphanCredentialRefs` entry, through F1's precedence upsert.
6. `extractContent`: for each manifest `content/` entry, if
   `blobs/sha256/<hh>/<hex>` already exists with the manifest's length,
   keep it; else re-hash the staged file (D3's second verification) and
   rename it into place, then `fsync` the prefix directory. This step
   only adds files.
7. `swapMedia`: re-hash every staged media file. Then record
   `swapMedia.liveExisted` in the journal (atomically): whether
   `<media_root>/snapshots` exists. P8 creates it lazily at the first
   capture (`cameras/media.rs`) and its sweep tolerates its absence, so a
   Farm that never captured has none. If it exists, rename it to
   `restore-<stagingId>/previous-snapshots`. Then rename
   `restore-<stagingId>/snapshots` to `<media_root>/snapshots`, and `fsync`
   the media root. Staging always creates `restore-<stagingId>/snapshots`,
   empty for `media: none`.
8. `validate`: open the placed database as `Storage::open` would (WAL,
   `synchronous = FULL`, foreign keys on), run `migrations::apply`,
   `integrity_check` (exactly `ok`), `foreign_key_check` (no rows), and
   `integrity::check` with the live roots (no violation). Every table's
   count must equal `expectedCounts`, except that
   `pending_credential_cleanup` must hold every carried and orphan ref and
   `slicer_runtime_config` must hold the carried paths. Close normally
   (a checkpoint, so no `-wal` is left).
9. `markInstalled`: phase `installed`.
10. `removePrevious`: delete `restore/<id>/previous/`, the
    `restore-<stagingId>` media directory, the content staging directory,
    and the database staging directory.
11. `markDone`: phase `done`, `outcome.finishedAt`.

#### Rollback and retry (restore)

- **Rollback** is two-phase and idempotent. Each phase's completion is
  made durable in `journal.rollback` before the next starts, so a crash
  or an I/O error inside a rollback resumes it without redoing a
  destructive step:
  0. If `journal.rollback` is null, write `rollback: { from: <the failed
     step>, mediaRestored: false, candidateCleared: false }` first.
     `journal.step` is never changed by a rollback.
  1. **(a) Media**, unless `mediaRestored`. If `journal.swapMedia` is null,
     `swapMedia` never renamed anything: skip. If `liveExisted` is true:
     when `restore-<stagingId>/previous-snapshots` exists, move any
     `<media_root>/snapshots` present back to `restore-<stagingId>/snapshots`
     (with `previous-snapshots` still present, it can only be the staged
     tree), then rename `previous-snapshots` to `<media_root>/snapshots`;
     when it doesn't exist, the first rename never happened: nothing to
     do. If `liveExisted` is false: move any `<media_root>/snapshots`
     present (it can only be the staged tree) back to
     `restore-<stagingId>/snapshots`, leaving no `snapshots` directory, as
     before. Then write `mediaRestored: true` and `swapMedia: null`.
  2. **(b) Clear the candidate**, unless `candidateCleared`. If `from` is
     `placeCandidate` or later, every original database file is already in
     `previous/` (the step after `moveDatabaseAside` is recorded only once
     it completed), so delete `farm3d.sqlite3`, `-wal`, `-shm`, and
     `.farm3d-restore-candidate.partial` from `metadata_root`: they are
     the candidate. If `from` is `moveDatabaseAside` or earlier, delete
     nothing: whatever is in `metadata_root` is original. Then write
     `candidateCleared: true`. After this, the rollback never deletes a
     database file again.
  3. **(c) Move back.** For `farm3d.sqlite3`, `-wal`, and `-shm`, in that
     order: if `restore/<id>/previous/<name>` exists, rename it into
     `metadata_root`. A rename is atomic, so each file is in exactly one
     of the two places; a file already moved back is simply not in
     `previous/` any more. Then `fsync` both directories.
  4. **Content**: nothing. Blobs added by a rolled-back install have no
     row and are removed by P4's startup sweep.
  5. One journal write then clears `rollback` and sets the phase:
     `pending` for a retry, or `failed`.
- **When to roll back.** A crash (the next start finds phase
  `installing`) and an I/O error at any step both roll back. Then, if
  `attempts < 2`, the phase returns to `pending` and the install runs
  again at once; otherwise the phase becomes `failed` with
  `INSUFFICIENT_SPACE` (the last error was out of space) or
  `RESTORE_FAILED`.
- A verification failure rolls back and fails at once with
  `BACKUP_INVALID`, because retrying the same staged files can't succeed:
  a staged file whose hash no longer matches the manifest
  (`extractContent`, `swapMedia`: `checksumMismatch`), or a `validate`
  failure (`databaseInvalid` or `countMismatch`).
- A crash after `installed` rolls **forward**: the next start runs
  `removePrevious` and `markDone`.
- If a rollback itself fails, startup stops with `RESTORE_FAILED`
  (`details.reason: "rollbackFailed"`) with `journal.rollback` recording
  how far it got, so the retry resumes the rollback at the first phase
  not yet marked done.
- **Byte-exact** is measured against the files as they were before the
  installer ran (before `recheckBlockers`), and means:
  - `farm3d.sqlite3` is byte-identical;
  - `farm3d.sqlite3-wal`: if it existed and was non-empty, it is
    byte-identical; otherwise it is absent or empty (`recheckBlockers` may
    leave an empty one);
  - `farm3d.sqlite3-shm` is ignored (presence and bytes): it is a
    rebuildable index, and `recheckBlockers` may create one;
  - every pre-existing blob file is unchanged (extra blob files are
    allowed until the sweep);
  - `<media_root>/snapshots` is byte-identical, tree and files, or still
    absent if it was absent.

The "Installer fault points" table below is the contract.

### D9. Safety backups and their cleanup

**Decision: a safety backup is an ordinary backup with `media: all`,
written by farm3d before a restore or a tier (c) reset.**

- Written by D5's writer to `<backup_root>/safety/<sfb-id>.farm3d-backup`
  with `origin` `beforeRestore` or `beforeReset`. Its free-space check
  uses the estimate for `media: all`.
- After writing, it is **verified** by reading it back with D3's rules 1–8
  and 10 (every entry streamed and hashed; nothing extracted). A safety
  backup that fails verification is deleted and the operation fails with
  `BACKUP_SOURCE_DAMAGED` (`details.entry` names the first bad entry).
- **Retention.** Only after the new one verifies, safety backups beyond
  the newest three (by `createdAt` in their manifests) are deleted. A
  deletion failure is ignored and retried after the next safety backup.
  Files in `safety/` that don't parse are listed as `valid: false` and
  never counted or deleted by retention.
- They are listed by `list_backups`, restorable through
  `preview_restore({ kind: "safetyBackup", backupId })`, and deletable by
  `delete_backup`.
- They survive every reset tier. Tier (c) deletes them only when the
  operator ticks "also delete safety backups", and even then keeps the
  safety backup that reset itself just wrote (the operator turns that one
  off with its own option).
- `<backup_root>/tmp/` is emptied at startup, after the installer.

### D10. Machine-specific data, linked paths, and credentials

**Decision: the backup carries the Farm, never the machine. Paths that
point at this machine's files stay verbatim and are surfaced; paths that
configure this machine are replaced by the local ones; credential values
never enter or leave.**

- **Slicer runtime.** The backup nulls both paths (D5). At install, the
  local row is carried into the new database (D8 `carryLocalState`). The
  preview always shows the `slicerRuntimeKeptLocal` notice.
- **Linked paths.** `library_models.linked_path` and
  `model_source_revisions.source_path` are restored verbatim. A path that
  doesn't exist here surfaces through P4's existing source state
  (`missing`) and **Locate source**. The preview's `linkedPathsMissing`
  notice counts linked Models whose `linked_path` doesn't exist on this
  machine (`Path::try_exists` returning `Ok(false)` or an error).
- **Camera URLs** stay in the database copy (decision 14): they are
  Printer configuration the operator owns. Diagnostics never carry them.
- **Credential values** are never read into or out of a backup. The
  preview's `credentialsToReenter` notice counts the candidate's Printers
  whose `credentialRef` the local store lacks, using
  `CredentialStore::contains(ref)`, a new method that reads the value
  into a `Zeroizing` buffer and drops it at once. After install, those
  Printers report `CREDENTIAL_REQUIRED` (existing behavior).
- **Orphaned local refs.** Local refs the new Farm doesn't reference are
  queued in the new database's `pending_credential_cleanup` with reason
  `import_orphan`, which F1 never deletes automatically; local pending
  rows are carried as they are, except a row whose ref the restored
  Printers use, which is dropped (D8 `apply_restore` step 8). A rollback, or restoring the safety
  backup, therefore still finds every credential. The preview's
  `credentialsOrphaned` notice counts them.

### D11. History read model

**Decision: history is a read model over the P7 and P8 tables, keyset
paged on a history time that exists for every Job it can show.**

- **History time.** `historyAt = COALESCE(jobs.ended_at,
  jobs.created_at)`. A terminal Job has `ended_at`, which never changes; an
  `outcomeUnknown` Job has none (0008's CHECK), so it sorts by
  `created_at`, which never changes either. `updated_at` is not usable:
  every `jobs_repository::write_row` bumps it, including
  `update_columns` from `dispatch::apply_host_outcome`
  (`jobs/dispatch.rs:209`) when a pause or cancel Host Operation resolves
  after the Job went `outcomeUnknown`. The key changes exactly once, when
  the operator declares the outcome and `ended_at` is set.
- **`list_job_history(query)`**, one read transaction:
  - `states`: 1–4 distinct `JobHistoryState` values; default
    `["completed", "failed", "cancelled"]`, so `outcomeUnknown` is shown
    only when asked for.
  - `printerLifecycle`: `any` (default), `active` (`printers.archived_at
    IS NULL`), or `archived`.
  - `printerId`, `spoolId`, `modelId`: exact ids.
  - `endedAfter` (inclusive) and `endedBefore` (exclusive), RFC 3339,
    compared with `historyAt`.
  - `text`: trimmed; blank means absent; at most 200 characters (more is
    `VALIDATION` on `query.text`). It matches, with `LIKE '%' || ?
    || '%' ESCAPE '\'` after escaping `\`, `%`, and `_`, any of: the
    Job's Printer snapshot name (`json_extract(printer_snapshot_json,
    '$.name')`), the Model's current name, the Slice Revision's
    `plate_name`; and it matches the Spool number when the text, less
    one leading `#`, is all digits and equals it; and the Job id by
    prefix (`jobs.id LIKE ? || '%' ESCAPE '\'`). SQLite's `LIKE` folds
    ASCII case only, so non-ASCII text matches case-sensitively.
  - `after`: an opaque cursor encoding `(historyAt, id)`; malformed is
    `VALIDATION` on `query.after`.
  - `limit`: 1–200, default 50.
- **Order and paging.** `historyAt DESC, id DESC`. The query always
  carries the index's exact term `j.state IN
  ('completed','failed','cancelled','outcomeUnknown')` next to the
  narrower `states` filter, so the partial indexes apply. The keyset
  predicate is written in the seekable form:

  ```sql
  COALESCE(j.ended_at, j.created_at) <= :at
    AND (COALESCE(j.ended_at, j.created_at) < :at OR j.id < :id)
  ```

  It reads `limit + 1` rows; `nextCursor` is the last returned row's key
  when the extra row exists. A Job that finishes between pages sorts
  above the first page, so paging never repeats or skips a row that
  didn't change; an `outcomeUnknown` Job declared between pages moves
  above the first page too (so a paging session can miss it once, never
  repeat it).
- **Index choice is forced.** farm3d never runs `ANALYZE`, so without
  `sqlite_stat1` the planner may pick `jobs_state` and a temporary B-tree
  for the first page. The query names its index: `FROM jobs j INDEXED BY
  jobs_history_printer` when `printerId` is given, else `INDEXED BY
  jobs_history`. `INDEXED BY` fails the statement if the index can't be
  used, so a regression is an error, not a slow scan.
- **Indexes** (migration 0010): `jobs_history` for the default query and
  `jobs_history_printer` for the Printer filter. Task 4 asserts both with
  `EXPLAIN QUERY PLAN` and may add indexes to 0010 in place (it is
  unreleased).
- **`get_job_timeline(jobId)`**, one read transaction, returns an
  immutable view: nothing in it reads current mutable state other than a
  snapshot's pruned fields. It joins:
  - `job_events` (P7's `JobEvent`, with its typed detail);
  - the Job's Host Operations (`host_operations.job_id`);
  - its reservation (`spool_reservations` where `holder_kind = 'job'` and
    `holder_id` is the Job);
  - its material ledger: `spool_amount_events` whose `reservation_id` is
    the Job's reservation, plus the correction event
    (`jobs.correction_event_id`);
  - its Reconciliation Requirements;
  - its Incident (P8's `Incident`) and every `incident_events` entry;
  - its Attention Events (`attention_events.job_id`);
  - its camera snapshots (`camera_snapshots.job_id`), pruned or not.

  It also carries the Job, its Queue Entry and lineage, the Printer
  snapshot, and the Slice Revision's immutable facts (target, runtime,
  plate, estimates). It never carries the Printer's current name or
  archived flag, the Model's current name, or the Spool's lifecycle: the
  history row carries those current links.
- **Order.** Items sort by `at` ascending, then by source in the order
  `job`, `hostOperation`, `reservation`, `amountEvent`, `requirement`,
  `attention`, `incident`, `snapshot`, then by the source's own sequence
  (`job_events.sequence`, `spool_amount_events.sequence`,
  `incident_events.sequence`), else by id bytewise.
- **Immutability.** The timeline of a finished Job is byte-identical
  before and after archiving its Printer, archiving its Spool, and pruning
  media, except each pruned snapshot's `revision`, `prunedAt`, and
  `pruneReason`.
- Spool movements have no Job column, so the timeline doesn't list them;
  they stay on the Spool's own history. FTS5 is not used.

### D12. The typed log and rotation (ADR-0017)

**Decision: one JSON-lines log, written by a background thread, that
accepts only values of a sealed `LogSafe` trait.**

- **Location.** `<log_root>/farm3d.log`. At 2 MiB it rotates:
  `farm3d.3.log` becomes `farm3d.4.log` (replacing it), and so on down to
  `farm3d.log` becoming `farm3d.1.log`; then a new `farm3d.log` opens. At
  most five files exist.
- **Line format.** One JSON object per line, at most 4 KiB:

  ```json
  {"ts":"2026-09-29T12:00:00.123Z","level":"warn","code":"cameras.captureFailed",
   "ids":{"printerId":"prn-…"},"fields":{"errorKind":"timeout","attempt":2}}
  ```

  `level` is `error`, `warn`, or `info`. `code` is a `&'static str` of the
  form `<domain>.<camelCase>`. Values whose `LogSafe` kind is an id go in
  `ids`; everything else goes in `fields`. At most 16 entries in each.
- **The macro.** `f3d_log!(warn, "cameras.captureFailed", printer_id =
  LogId::printer(&id), error_kind = kind)` turns each key into camelCase
  and requires every value to implement `LogSafe`. The trait is sealed
  (a private supertrait), and its implementors are exactly:
  - `LogId`, built only by typed constructors (`LogId::printer`, `job`,
    `spool`, `model`, `project`, `slice_revision`, `queue_entry`,
    `host_operation`, `incident`, `attention`, `snapshot`,
    `slice_operation`, `operation`, `staging`, `journal`, `backup`);
  - `ErrorCode`, and fieldless domain enums that opt in with a one-line
    impl in `diagnostics/log.rs` (they log their serde name);
  - `StorageError` and `RepositoryError`, which log their variant name
    only, never `Display` or `Debug`;
  - `u8`–`u64`, `i8`–`i64`, `usize`, `bool`, `std::time::Duration`
    (milliseconds), and `DateTime<Utc>` (RFC 3339);
  - `Pseudonym` (below).

  `String`, `&str`, `&'static str`, `PathBuf`, `Url`, and every error's
  `Display` are not `LogSafe`, so free text can't compile into a line.
  Task 3 proves this with a compile-fail test or, if `trybuild` isn't
  worth adding, a unit test over the sealed implementor list, and records
  the choice.
- **`Pseudonym`** is built only by `Pseudonym::of(kind, value)`:
  `<kind>-` plus the first 8 hex digits of SHA-256 over a random 16-byte
  process salt, `0x00`, the kind, `0x00`, and the value. The salt is never
  persisted, so a pseudonym correlates lines within one run and can't be
  reversed.
- **Writer.** A bounded channel of 1024 lines feeds one writer thread.
  Logging never blocks and never panics: a full channel or a write error
  drops the line and counts it, and the next successful write first
  records `log.dropped` with the count. Debug builds also mirror each line
  to stderr.
- **`eprintln!` retires.** Every `eprintln!` and `println!` in
  `src-tauri/src` outside `bin/` and outside `#[cfg(test)]` code moves to
  `f3d_log!`, and a test walks the tree and fails on a new one. The
  notification service's in-memory line buffer keeps its fixed messages
  and also logs them.

### D13. The diagnostics bundle and the redaction threat model (ADR-0017)

**Decision: the reader is a third party (a support thread or a public
issue). The bundle is built only from typed facts and pseudonyms, and a
final egress scan against the live corpus refuses to write it on any
hit.**

**Protected** (never in a bundle): credential values and refs; hosts,
IPs, and ports; every URL and its query values; absolute paths and
usernames; user-authored names and notes (Printer, Spool, tare, Model,
Project, Location, storage labels, notes, Incident notes); host-supplied
strings (job and file names); raw ids.

- **File.** `farm3d-diagnostics-<yyyymmddThhmmssZ>.zip` through the
  native save dialog, built in memory (at most 32 MiB; larger is
  `INTERNAL`) and written with `document_io::atomic_write`.
- **Entries.** `bundle.json` (`{ format: "farm3d-diagnostics",
  formatVersion: 1, createdAt, appVersion, sections }`), then one entry
  per selected section:

  | Section | Entry | Contents |
  |---|---|---|
  | `about` | `about.json` | `AboutInfo` (D18 wire types) |
  | `health` | `health.json` | per Printer: pseudonym, adapter kind, archived, Setup incomplete, connection state, error code, seconds since last observation, each Capability's state and reason, and the host software version if it is a token of 1–64 characters from `[0-9A-Za-z.+-]` (else null) |
  | `storage` | `storage.json` | `StorageUsage` classes, `integrity::check` findings as `{rule, outcome, count}`, `migration_warnings` codes with counts, `pending_credential_cleanup` counts by reason, the `pending_blob_cleanup` count |
  | `configuration` | `configuration.json` | the `settings` values (theme mode only if it names a built-in theme, else `custom`), per-table row counts, camera source counts by kind, Printer counts by adapter kind, archived, and Profile-only, whether a Slicer runtime is configured |
  | `logs` | `logs/farm3d.log`, `logs/farm3d.1.log`, … | the log files, with every `ids` value rewritten to its bundle pseudonym |
  | `recentProblems` | `recent-problems.json` | the 50 newest Attention Events by `firstObservedAt`: condition, severity, source kind and pseudonym, `firstObservedAt`, `resolvedAt`, resolution |

- **Bundle pseudonyms.** Each export draws a fresh random salt. For each
  kind, the ids that appear in the bundle are sorted by SHA-256(salt ‖
  id) and numbered from 1: `printer-3`, `job-12`. Pseudonyms are stable
  within one bundle and differ between bundles. An id in a log line that
  no longer exists still gets a number.
- **Egress scan.** After every entry is serialized and before any file is
  opened, `diagnostics::egress::scan` checks each entry's bytes against
  a corpus read from the live Farm in one read transaction:
  - every credential value behind every ref in the database (read with
    `CredentialStore::get` into `Zeroizing`, compared, dropped);
  - every Connection host (`printers.connection_json` `$.host`) and
    Host Operation endpoint host (`host_operations.endpoint_json`);
  - every camera `snapshot_url`, and each of its query values;
  - every credential ref (both forms), from `printers.connection_json`
    and `pending_credential_cleanup`;
  - the home directory path, and every absolute path farm3d knows:
    `library_models.linked_path`, `model_source_revisions.source_path`,
    the Slicer runtime's two paths, and the storage roots
    (`metadata_root`, the app data root, `log_root`, `backup_root`, the
    cache directory);
  - every Printer, Spool (manufacturer, product, color name, storage
    label), tare, Model, and Project name, Printer Location and notes,
    and Incident note text, longer than 3 characters.

  Secret, host, URL, and path terms are matched against all bytes of
  every entry, in their exact, lowercase, uppercase, and percent-encoded
  forms. Name terms are matched against the string values of each JSON
  entry, except values at enum-typed paths (the section schema lists
  them; in logs, `level`, `code`, and enum `fields`), in their exact and
  percent-encoded forms, so a Printer named after a state word doesn't
  block the export. A term shorter than 4 bytes is matched only as a
  whole string value. A hit aborts with `DIAGNOSTICS_REDACTION_FAILED`
  (`details.section`), nothing is written, and the matched term is never
  reported or logged.
- **Query-value demotion.** A camera URL query value that is only ASCII
  letters and at most 12 long (`action=snapshot`) is a name term, not an
  all-bytes term, unless its key contains `token`, `key`, `pass`, `pwd`,
  `auth`, `sig`, `secret`, `cred`, `session`, or `user` (any case); then
  it stays an all-bytes term. Otherwise the common MJPEG-streamer URL
  would block every export.
- **Limits.** The scan catches known values, not a transformed leak it
  has no variant for. The typed log and the typed collectors are the
  primary control; the scan is defense in depth (a residual risk).

### D14. Storage usage and cleanup

**Decision: usage is reported by class from the same bases P4 and P8
already use, and cleanup never touches referenced data.**

- **Classes** (`StorageClass`):
  - `database`: the lengths of `farm3d.sqlite3`, `-wal`, and `-shm`;
  - `contentModelSources`, `contentThumbnails`, `contentGcode`,
    `contentSliceArtifacts`, `contentSlicerLogs`: `content_blobs.size_bytes`
    (P4's `library_content_info` basis), each blob counted once, in the
    first class that references it, in that order (a `slice_revision_blobs`
    row with role `log` and `slice_operations.log_sha256` are slicer logs;
    the other roles are slice artifacts);
  - `contentUnreferenced`: `content_blobs` rows no class references plus
    `pending_blob_cleanup` rows;
  - `cameraMediaPinned`, `cameraMediaUnpinned`: `byte_len` over unpruned
    rows (P8's `media_usage` basis);
  - `safetyBackups`, `preImportSnapshots`, `logs`, `slicerProfileCache`:
    the summed file lengths of `<backup_root>/safety/`, the accepted
    `.farm3d-pre-import-*.sqlite3` files, `<log_root>/farm3d*.log`, and
    `<app_cache_dir>/orca-profiles/`.

  Every class reports apparent file length (or the recorded size, which
  equals it by invariant), never allocated blocks, so the tolerance
  against the bytes on disk is zero for a quiescent Farm.
- **`clear_storage(target)`** holds the lease and returns what it freed
  and the new usage:
  - `unreferencedContent`: `release_unreferenced`, then the startup
    sweep's orphan removal, now;
  - `preImportSnapshots`: delete every accepted pre-import snapshot but
    the newest one;
  - `rotatedLogs`: delete `farm3d.1.log`–`farm3d.4.log`, never
    `farm3d.log`;
  - `orcaCache`: delete `orca-profiles/`, refused with `STORAGE_IN_USE`
    (`details.reason: "sliceRunning"`) while a slice operation is
    `queued` or `running`.

  Each leaves `integrity::check` with no violation.

### D15. Tiered reset and partial-reset recovery

**Decision: three tiers, each with its own confirmation phrase, each
naming every affected data class in `reset_preview` first.**

| Tier (wire) | Phrase | Affects | Keeps |
|---|---|---|---|
| `settings` | `reset settings` | the `settings` row | Slicer runtime, per-Printer alert defaults, everything else |
| `cameraMedia` | `reset media` | unpinned snapshot images (`scope: "unpinned"`), or all of them (`scope: "all"`) | every snapshot row and Incident timeline entry |
| `farm` | `reset farm` | the database (every domain), content, camera media, logs, farm3d's own credentials, pre-import snapshots, restore staging, F0 legacy archives | safety backups (unless ticked), the OrcaSlicer profile cache |

- **Tier (a), settings.** F1's pre-import snapshot
  (`SnapshotKind::Settings`), then one transaction that checks
  `expectedRevision` (`CONFLICT` on mismatch), claims the `operationId`
  (`resetSettings`), and writes the defaults `ensure_default` and the
  column defaults define (`theme_mode 'system'`, `monitor_section
  'printerModel'`, `monitor_density 'comfortable'`, notify classes
  `1,1,1,0,0,0`, 30 days, 2048 MiB), with `revision + 1`. It returns the
  new `SettingsRecord`. Pokes the `MediaJanitor` when retention changed.
- **Tier (b), camera media.** Under the `MediaJanitor` lock, P8's prune
  order: one transaction marks every selected unpruned row
  `pruned_at = now`, `prune_reason = 'reset'`, `revision + 1`, appends
  `evidencePruned { reason: "reset" }` for each Incident-linked row, and
  claims the `operationId` (`resetCameraMedia`); after commit the files
  are queued, because `reset_farm` holds the lease, and unlinked when it
  drops the lease at the end of the command. It publishes what a P8
  prune pass publishes. `scope: "all"` includes pinned rows, which keep
  `pinned_at` (0010's widened CHECK allows it). A crash between commit and
  unlink leaves orphan files that P8's startup sweep deletes.
- **Tier (c), entire Farm.** `reset_farm` refuses with `RESTART_PENDING`
  when an unfinished journal exists and acknowledges a finished one, as
  `apply_restore` does (D8 step 6). Then it takes the lease, optionally
  writes a safety backup (`safetyBackup: true` by default, `origin:
  beforeReset`), reads every credential ref (from
  `printers.connection_json` and `pending_credential_cleanup`), writes a
  `reset` journal (phase `pending`), and restarts through the
  `Restarter`. The installer then runs, **roll forward only**, resuming at
  the recorded step after any crash:
  1. `markInstalling`.
  2. `moveDatabaseAside` (as D8; each file moved only if still present).
  3. `moveRootsAside`: rename `content_root`, `media_root`, and
     `log_root` each to a sibling `<parent>/.aside-<journalId>-<name>`,
     `<name>` being the root's own directory name (a sibling stays on the
     same filesystem, and the name keeps two roots under one parent from
     sharing an aside), and move the contents of
     `snapshot_root` and `legacy_root` into `restore/<id>/previous/`.
     A root whose aside already exists was moved; a root that exists
     again then is the empty directory `StoragePaths` recreated, and is
     left alone. Missing roots are recreated with user-only permissions.
  4. `createFreshDatabase`: delete any database set in `metadata_root`
     (it can only be a partial fresh one), then open, migrate to the
     current schema, and close.
  5. `deleteCredentials`: delete each `orphanCredentialRefs` entry from
     the credential store; an absent ref counts as deleted. Each failure
     (or an unavailable store) is upserted into the fresh database's
     `pending_credential_cleanup` with reason `reset`, which F1's startup
     retry treats as automatically eligible. The step itself never fails
     for a credential.
  6. `deleteSafetyBackups`, only when `reset.deleteSafetyBackups`: delete
     every file in `<backup_root>/safety/` except the journal's
     `safetyBackupId`.
  7. `removePrevious`: delete `restore/<id>/previous/` and every
     `.aside-<journalId>-<name>` directory.
  8. `markDone`.

  An I/O error at any step stops startup with `RESTORE_FAILED`
  (`installFailed`); the retry resumes at the same step. `reset` joins
  F1's cleanup precedence below every automatic reason and above
  `import_orphan`: `printer_deleted > cleared > replaced > provisional >
  reset > import_orphan`.
- **Preview.** `reset_preview(tier)` lists every affected and every kept
  class with counts and bytes (`ResetDataClass`), plus warnings: for tier
  (c), `activeWork` (the D7 blockers' counts; a reset is not refused for
  them, and their prints continue on the Printers, unknown to the fresh
  Farm) and `credentialStoreUnavailable`.

### D16. The Settings workspace

**Decision: Settings becomes a lazy-loaded workspace on the existing
`settings` destination, with eight categories in vertical tabs.**

- The rail's gear becomes an ordinary activity-bar destination.
  `App.tsx` adds `settings` to `availableDestinations`.
- `NavigationSelectionKind` gains `settingsCategory`. Its ids are the
  category slugs: `general`, `appearance`, `slicing`, `notifications`,
  `storage`, `connections`, `diagnostics`, `about`. A deep link is
  `#nav=v1/settings/settingsCategory/storage`. An unknown slug falls back
  to `general` with the standard availability notice.
- Categories, in umbrella order, with their contents:

  | Category | Slug | Contents |
  |---|---|---|
  | General | `general` | Monitor section and density, Settings export and import |
  | Appearance | `appearance` | Theme preview, Apply, Revert |
  | Slicing | `slicing` | `SlicerSettingsForm` (engine and preset source) |
  | Notifications and retention | `notifications` | `NotificationSettingsForm` (classes, retention, disk cap, notifier status, test) |
  | Storage and backup | `storage` | Storage usage and cleanup, Create backup (media choice with sizes), Restore from file, safety backups (restore, delete) |
  | Connections | `connections` | Printers export and import, the credential store tier |
  | Diagnostics | `diagnostics` | Diagnostics bundle (sections), Reset (three tiers) |
  | About | `about` | `AboutInfo` |

- **Appearance keeps preview semantics.** Selecting a theme previews it;
  Apply commits it. Leaving the category or the workspace, or pressing
  Revert, calls `cancelPreview()`.
- `SettingsMenu.tsx` and `ThemePopover.tsx` are deleted after their tests
  are ported. The Slicer and Notifications dialog bodies become forms the
  workspace hosts; the Slicer dialog stays for its Preparation openers,
  and the Notifications dialog is retired.
- Settings import now re-applies the imported theme (a gap on `main`).
- About replaces `AppShell`'s hard-coded `VERSION`.
- The workspace and its heavy categories are lazy chunks, so the main
  chunk doesn't grow.

### D17. The integrity catalogue

**Decision: one checker, `persistence::integrity::check(conn,
roots: Option<&IntegrityRoots>) -> Result<IntegrityReport,
StorageError>`, used by the backup writer, restore staging and install,
the diagnostics storage section, `clear_storage`, and the reference
matrix. Its reads run in one deferred read transaction.**

| Rule (wire) | What it checks | Outcome |
|---|---|---|
| `fk` | `PRAGMA foreign_key_check` | violation |
| `jobCorrectionEvent` | a non-NULL `jobs.correction_event_id` names a `spool_amount_events` row of the Job's Spool | violation |
| `amountEventReservation` | a non-NULL `spool_amount_events.reservation_id` names a `spool_reservations` row | violation |
| `reservationHolder` | `spool_reservations.holder_kind` is `job` and `holder_id` names a Job | violation |
| `attentionSource` | `attention_events(source_kind, source_id)` names a row of `printers`, `jobs`, `reconciliation_requirements`, or `spools`; a Printer source may be missing only when the Event is resolved (Printer delete resolves open Events `sourceRemoved`; one resolved earlier keeps its resolution; Task 11) | violation |
| `completionEvidence` | a `job.completed` Event whose `evidence_json` has `status: "captured"` names a `camera_snapshots` row | violation |
| `hostOperationGcode` | an unresolved (`dispatching`, `uncertain`, `reconciling`) `upload` or `start` Host Operation's `gcode_sha256` has a `content_blobs` row | violation |
| `embeddedReference` | (Task 11 audit) an id embedded in JSON names its row: `job_events.detail_json` `$.correctionEventId`/`$.hostOperationId`, `jobs.last_failure_json` `$.hostOperationId`, `attention_events.detail_json` `$.spoolId`, `incident_events.detail_json` `$.eventId`/`$.snapshotId` | violation |
| `blobFile` | every `content_blobs` row has `blobs/sha256/<hh>/<hex>` with its `size_bytes` length (roots only) | violation |
| `mediaFile` | every unpruned `camera_snapshots` row has its `rel_path` file (roots only) | violation |
| `sliceTargetPrinter` | `slice_revisions.target_json` `$.target.printerId` names a Printer | tolerated (label fallback by design) |
| `preparationTargetPrinter` | `slice_preparations.document_json` `$.target.printerId` names a Printer | tolerated |
| `orphanBlobFile` | a blob file with neither a `content_blobs` nor a `pending_blob_cleanup` row (roots only) | tolerated (P4's startup sweep) |
| `orphanMediaFile` | an image file under `snapshots/` with no unpruned row (roots only) | tolerated (P8's startup sweep) |

- `IntegrityReport { counts: BTreeMap<String, i64>, findings:
  Vec<IntegrityFinding> }`, where each finding is `{ rule, outcome:
  Violation | Tolerated, count, sample }` and `sample` holds at most 20
  row ids or content hashes (never a path). `counts` has one entry per
  user table, the exact `count(*)`.
- `IntegrityReport::violations()` is empty for a clean Farm.
- The Task 11 audit greps every `*_json` column and every `TEXT` column
  ending `_id` for references this catalogue misses. Each one it finds
  gets a rule or a named tolerance here and in `integrity.rs`, in the same
  commit.
- (Task 11) Named tolerances, with no rule: `pending_credential_cleanup.printer_id`
  (provenance; the Printer is usually gone), `migration_warnings.details_json`
  (an advisory migration ledger), `printers.connection_json`'s
  `credentialRef` (the credential store, D10), and a terminal Host
  Operation's `gcode_sha256`. `p9_integrity.rs` classifies every loose
  column, so a new one fails until it is classified.

### D18. Commands, idempotency, and dialogs

**Decision: 18 commands. Commands whose whole effect is one database
transaction claim their `operationId` in the `operations` ledger; the
rest, whose effect is a file or a swapped database, use a process-local
ledger.**

- **Database-claimed** (`spools::operations::claim`, in the command's own
  transaction): `reset_farm` for tiers `settings` (`resetSettings`) and
  `cameraMedia` (`resetCameraMedia`). A replay returns the current
  settings record, or zero counts, with no side effect.
- **Process-local** (`backup::process_ops::ProcessOperations`, an
  in-memory map from `operationId` to kind, digest, and cached result):
  `create_backup` (`createBackup`, digest `{ media }`), `delete_backup`
  (`deleteBackup`, `{ backupId }`), `apply_restore` (`applyRestore`,
  `{ stagingId }`), `clear_storage` (`clearStorage`, `{ target }`),
  `export_diagnostics` (`exportDiagnostics`, `{ sections }` sorted), and
  `reset_farm` tier `farm` (`resetFarm`, `{ request }`). A replay in the
  same process returns the cached result; a reuse with another kind or
  digest is `VALIDATION` on `operationId`. The ledger is lost at restart,
  so a replay after a restart runs again (a new file). Neither kind of
  claim can live in the database: a file isn't a transaction, and
  `apply_restore` and tier (c) replace the database.
- **Dialogs** follow F1's "Dialog ownership": the frontend never passes a
  path; a result never returns a full path (a basename at most); closing
  the dialog returns `{ status: "cancelled" }` with no side effect.
  `backup::dialogs::PortabilityDialogs` is injected (open
  `.farm3d-backup`, save `.farm3d-backup`, save `.zip`).
- **Web mode.** Under `just web`, the frontend wrappers of the three
  dialog-owning commands (`create_backup`, `export_diagnostics`,
  `preview_restore` with a file source) return `{ status: "unsupported",
  reason: "desktopRequired" }` without invoking IPC; the other writes
  throw `needsDesktopError`; reads return web fixtures.
- **Confirmation** phrases are checked in Rust exactly (no trim, no case
  folding, the `DeletePrinterDialog` rule). The frontend check only
  enables the button.

## Conflict fixtures

Each fixture starts from a baseline where local and backup are equal (one
row per domain, same ids, same content) and changes only what the row
says. "L" is a local row, "B" a backup row. Expected items are exactly
those listed; every other row is unchanged. Task 6 copies this table
verbatim into `p9_restore_preview.rs`.

| # | Domain | Local | Backup | Content equal | Natural key situation | Expected |
|---|---|---|---|---|---|---|
| c1 | settings | singleton | singleton | yes | — | none |
| c2 | settings | theme `farm3d-dark` | theme `system` | no | — | `changed` settings |
| c3 | printer | L1 | L1 | yes | active, same identity | none |
| c4 | printer | L1 named "Left" | L1 named "Left Carbon" | no | — | `changed` L1 |
| c5 | printer | L1 revision 3 | L1 revision 2, otherwise equal | no | — | `changed` L1 |
| c6 | printer | L2 (not in backup), active, identity `192.0.2.20:7125` | B2 (not local), active, same identity | — | clash | `uniqueClash` L2/B2 |
| c7 | printer | L2 active, identity H | B2 archived, identity H | — | backup side archived | `onlyLocal` L2 |
| c8 | printer | L2 archived, identity H | B2 active, identity H | — | local side archived | `onlyLocal` L2 |
| c9 | printer | L2 with no Connection | B2 with no Connection | — | no identity | `onlyLocal` L2 |
| c10 | printer | — | B3 (not local) | — | no local match | none (added) |
| c11 | printer | L2 identity H; L1 identity K | B has L1 with identity H | no (L1) | clash through a shared id | `changed` L1, `uniqueClash` L2/L1 |
| c12 | printer | L1 active identity H | L1 archived identity H | no | same id | `changed` L1 only |
| c13 | spool | S2 #12 (not in backup) | T2 #12 (not local) | — | clash | `uniqueClash` S2/T2 |
| c14 | spool | S2 #12 | — | — | no backup #12 | `onlyLocal` S2 |
| c15 | spool | S1 `current_mg` 800000 | S1 `current_mg` 750000 | no | — | `changed` S1 |
| c16 | spool | S1 #12; S2 #14 (not in backup) | S1 #14 | no (S1) | clash through a shared id | `changed` S1, `uniqueClash` S2/S1 |
| c17 | tare | R2 "Cardboard 1kg" | R3 "cardboard 1KG" | — | ASCII case fold | `uniqueClash` R2/R3 |
| c18 | tare | R2 "Ölspule" | R3 "ölspule" | — | differs outside ASCII | `onlyLocal` R2 |
| c19 | project | P2 "Brackets" | P3 "BRACKETS" | — | clash | `uniqueClash` P2/P3 |
| c20 | project | P1 | P1 | yes | — | none |
| c21 | project | — | P3 (not local) | — | — | none (added) |
| c22 | model | M2 (not in backup) | — | — | no natural key | `onlyLocal` M2 |
| c23 | model | M1 named "Hook v2" | M1 named "Hook" | no | — | `changed` M1 |
| c24 | sliceRevision | SR2 (not in backup) | — | — | — | `onlyLocal` SR2 |
| c25 | queueEntry | Q1 `closed` | Q1 `queued` | no | — | `changed` Q1 |
| c26 | job | J2 (finished after the backup) | — | — | — | `onlyLocal` J2 |
| c27 | job | J1 | J1 | yes | — | none |
| c28 | incident | I1 closed | I1 open | no | — | `changed` I1 |
| c29 | attentionEvent | A1 with `read_at` set | A1 unread | no | — | `changed` A1 |
| c30 | snapshot | N2 (not in backup) | — | — | — | `onlyLocal` N2 |
| c31 | snapshot | N1 pruned `age` | N1 unpruned | no | — | `changed` N1 |
| c32 | (child) | Printer L1's slot renamed | original slot name | root equal | — | none; `material_slots` counts equal |
| c33 | (excluded) | a `printer_status_snapshots` row | none (sanitized) | — | — | none; local 1, backup 0 in counts |
| c34 | (kept local) | Slicer runtime paths set | paths NULL (sanitized) | — | — | none; `slicerRuntimeKeptLocal` notice |
| c35 | job | 250 local-only finished Jobs | — | — | — | one `onlyLocal`/`job` group, `total` 250, 200 items, ordered by id |
| c36 | printer | L2 identity H, L3 identity K (neither in backup) | B2 identity H, B3 identity K | — | two clashes | two `uniqueClash` items, ordered L2, L3 |

## Installer fault points

Each row injects a fault at the named point (`installer::Fault { step,
point }`, test-only, where a point can also be a rollback phase or a
single move-back), drops every handle as a crash would, then runs the
installer again with no fault. "Byte-exact" in an outcome is D8's
definition (baseline before `recheckBlockers`; `-shm` ignored; an empty
`-wal` allowed where none or an empty one existed). Rollback markers are
written `rollback { from, mediaRestored, candidateCleared }`. "Installed" means: counts equal
`expectedCounts` (with D8's two carried exceptions), `integrity::check`
has no violation, orphan refs are queued, and the journal is `done`.
"Rolled back" means byte-exact as D8 defines it, and the journal is
`failed`. No row may end mixed. Task 7 copies this table verbatim into
`p9_installer.rs`; Task 8 copies the reset rows into `p9_reset.rs`.

**Restore**

| # | Fault point | Journal at the fault | Disk at the fault | Next start | Outcome |
|---|---|---|---|---|---|
| f1 | after `apply_restore` writes the journal, before restart | `pending`, attempts 0 | nothing moved | runs from `recheckBlockers` | installed |
| f2 | `recheckBlockers` finds an active Job (seeded after the journal was written) | `pending` | nothing moved | — | `failed` `RESTORE_BLOCKED`; nothing moved; staging removed |
| f3 | the staging directory is deleted before start | `pending` | nothing moved | — | `failed` `RESTORE_STAGING_EXPIRED`; nothing moved |
| f4 | after `markInstalling`, before any move | `installing`/`moveDatabaseAside`, 1 | nothing moved | rollback (no-op), retry | installed, attempts 2 |
| f5 | `moveDatabaseAside` after moving the main file only, with a live `-wal` holding uncheckpointed frames | `installing`/`moveDatabaseAside`, 1 | main in `previous/`, `-wal` and `-shm` in root | rollback moves main back, retry | installed; and the pre-restore database, read before the retry, still shows the WAL's rows |
| f6 | after `moveDatabaseAside` completes | `installing`/`moveDatabaseAside`, 1 | no database in root | rollback, retry | installed |
| f7 | `placeCandidate` with the `.partial` copy half-written | `installing`/`placeCandidate`, 1 | `.partial` in root | rollback deletes it, moves the set back, retry | installed |
| f8 | after `placeCandidate` completes | `installing`/`placeCandidate`, 1 | candidate in root | rollback, retry | installed |
| f9 | `carryLocalState` before its commit | `installing`/`carryLocalState`, 1 | candidate plus `-wal` in root | rollback deletes the candidate set, retry | installed |
| f10 | `extractContent` after the first blob is renamed | `installing`/`extractContent`, 1 | one blob added | rollback, retry (keeps the added blob) | installed |
| f11 | `extractContent` fails with no space (twice) | — (in-run errors) | some blobs added | rollback, retry, same error | `failed` `INSUFFICIENT_SPACE`; rolled back; the added blobs are removed by P4's sweep at that same start |
| f12 | `swapMedia` between its two renames | `installing`/`swapMedia`, 1, `liveExisted` true | no `<media_root>/snapshots` | rollback renames `previous-snapshots` back, retry | installed |
| f12a | `swapMedia` on a Farm with no `<media_root>/snapshots` (never captured), after the staged tree is renamed in | `installing`/`swapMedia`, 1, `liveExisted` false | staged tree at `<media_root>/snapshots` | rollback moves it back to staging, leaving no `snapshots`, retry | installed |
| f12b | as f12a, but both attempts crash there | `installing`/`swapMedia`, 2, `liveExisted` false | staged tree in place | rollback, no retry | `failed` `RESTORE_FAILED`; rolled back, `<media_root>/snapshots` still absent |
| f12c | after `swapMedia` records `liveExisted`, before its first rename | `installing`/`swapMedia`, 1 | untouched | rollback (media: nothing to undo), retry | installed |
| f13 | after `swapMedia` completes | `installing`/`swapMedia`, 1 | staged media in place | rollback swaps both back, retry | installed |
| f14 | a staged blob altered after preview | — (in-run, at `extractContent`) | — | rollback, no retry | `failed` `BACKUP_INVALID` (`checksumMismatch`); rolled back |
| f15 | `validate` fails (candidate corrupted after staging) | — (in-run) | candidate in place | rollback, no retry | `failed` `BACKUP_INVALID` (`databaseInvalid`); rolled back |
| f16 | crash during `validate` | `installing`/`validate`, 1 | candidate in place | rollback, retry | installed |
| f17 | two consecutive crashes (f8, then f8 again on the retry) | `installing`/`placeCandidate`, 2 | candidate in root | rollback, no retry | `failed` `RESTORE_FAILED`; rolled back |
| f18 | after `markInstalled` | `installed` | `previous/` present | `removePrevious`, `markDone` | installed |
| f19 | `removePrevious` half done | `installed`/`removePrevious` | `previous/` partly deleted | `removePrevious`, `markDone` | installed |
| f20 | after `markDone` | `done` | clean | nothing | installed; `restore_status` is `done` until acknowledged |
| f21 | rollback fails once (an injected I/O error during f8's rollback, phase b) | `installing`/`placeCandidate`, 1, `rollback` written | candidate in root | startup `RESTORE_FAILED` (`rollbackFailed`); the retry resumes the rollback, then retries the install | installed |
| f21a | crash in f13's rollback after `rollback` is written, before phase (a) | `rollback { from: swapMedia, false, false }` | staged media in place, candidate in root | rollback resumes at phase (a) | installed |
| f21b | crash in f13's rollback after `mediaRestored` is written | `rollback { …, true, false }` | original media back, candidate in root | rollback resumes at phase (b) | installed |
| f21c | crash in f8's rollback after `candidateCleared` is written, before any move-back | `rollback { from: placeCandidate, true, true }` | no database in root; the original set in `previous/` | phase (c) moves all three back | installed |
| f21d | crash in f8's rollback after `farm3d.sqlite3` is moved back, before `-wal` (with a live `-wal` holding uncheckpointed frames) | `rollback { …, true, true }` | original main in root, original `-wal` and `-shm` in `previous/` | phase (c) moves `-wal` and `-shm` back; it never deletes the main file | installed; and, read before the retry, the pre-restore database shows the WAL's rows |
| f21e | crash in f8's rollback after `-wal` is moved back, before `-shm` | `rollback { …, true, true }` | main and `-wal` in root, `-shm` in `previous/` | phase (c) moves `-shm` back | installed |
| f21f | f21d, with `attempts` 2 | `rollback { …, true, true }`, attempts 2 | as f21d | phase (c), then no retry | `failed` `RESTORE_FAILED`; rolled back byte-exact, the WAL's rows readable |
| f21g | crash in f5's rollback (`from: moveDatabaseAside`) after `candidateCleared` is written | `rollback { from: moveDatabaseAside, true, true }` | main in `previous/`, `-wal` and `-shm` in root | phase (c) moves main back; nothing is deleted | installed |
| f22 | the journal file is corrupt | — | untouched | startup `RESTORE_FAILED` (`journalUnreadable`) | nothing touched |
| f23 | `journalVersion` is 2 | — | untouched | startup `RESTORE_FAILED` (`journalVersion`) | nothing touched |
| f24 | the restarting process still holds the lease for 2 s | `pending` | — | lease retried for up to 10 s | installed |
| f25 | a second `apply_restore` (or any `reset_farm`) while the journal is `pending`, before the restart | `pending` | — | — | refused `RESTART_PENDING`; the journal is unchanged; the next start installs the first restore |

**Reset (tier c, roll forward only)**

| # | Fault point | Journal at the fault | Next start | Outcome |
|---|---|---|---|---|
| r1 | after `reset_farm` writes the journal, before restart | `pending` | runs from `markInstalling` | reset done |
| r2 | `moveDatabaseAside` after the main file only | `installing`/`moveDatabaseAside` | moves the rest | reset done |
| r3 | `moveRootsAside` after `content_root` only | `installing`/`moveRootsAside` | skips the moved root, moves the rest | reset done |
| r4 | `createFreshDatabase` before its first commit | `installing`/`createFreshDatabase` | deletes the partial set, recreates | reset done |
| r5 | `deleteCredentials` after half the refs | `installing`/`deleteCredentials` | deletes the rest (absent counts as deleted) | reset done; every ref gone from the fake store |
| r6 | the credential store is unavailable | — (in-run) | — | reset done; every ref queued with reason `reset` |
| r7 | `deleteSafetyBackups` half done (option on) | `installing`/`deleteSafetyBackups` | deletes the rest | reset done; `safety/` holds only this reset's safety backup |
| r8 | `removePrevious` half done | `installing`/`removePrevious` | deletes the rest | reset done |
| r9 | after `markDone` | `done` | nothing | reset done; `restore_status` is `done` until acknowledged |
| r10 | an I/O error at `moveRootsAside`, twice | — | startup `RESTORE_FAILED` (`installFailed`) each time; the third start succeeds | reset done |

"Reset done" means: the database is fresh at the current schema with no
domain rows, `content_root`, `media_root`, and `log_root` exist and are
empty, `snapshot_root` and `legacy_root` are empty, no `previous/` or
`.aside-*` directory remains, every journaled ref is deleted or queued
with reason `reset`, and `safety/` holds this reset's safety backup (if
one was written) and, unless the option was ticked, the older ones.

## Backend model

### Module layout

| File | Content |
|---|---|
| `persistence/integrity.rs` | `check`, `IntegrityRoots`, `IntegrityReport`, `IntegrityFinding`, `IntegrityRule`, `IntegrityOutcome` |
| `history/mod.rs` | wire types: `JobHistoryState`, `PrinterLifecycleFilter`, `JobHistoryQuery`, `JobHistoryRow`, `JobHistoryPage`, `JobTimeline`, `JobTimelineItem`, `JobTimelineSliceRevision` |
| `history/repository.rs` | the query builder, the cursor codec, the timeline joins |
| `history/commands.rs` | `list_job_history`, `get_job_timeline` |
| `backup/mod.rs` | wire types: `BackupMediaChoice`, `BackupOrigin`, `BackupInventory`, `BackupSummary`, `CreateBackupOutcome`, `RestoreSource`, `PreviewRestoreOutcome`, `RestorePreview`, `RestoreCount`, `RestoreConflictClass`, `RestoreDomain`, `RestoreConflict`, `RestoreConflictGroup`, `RestoreNotice`, `RestoreBlocker`, `RestoreBlockerKind`, `ApplyRestoreResult`, `RestoreStatus`, `BackupInvalidReason`, `InstallerStep` |
| `backup/manifest.rs` | `Manifest` (closed serde schema), validation, the compatibility window |
| `backup/archive.rs` | the zip reader and writer, the path rules, limits, streaming SHA-256 |
| `backup/inventory.rs` | what goes in: blobs from the copy, media by choice, the media pre-pass, sanitization |
| `backup/lease.rs` | `BackupLease`, `LeaseActivity`, the deferred unlink queue |
| `backup/writer.rs` | D5's writer |
| `backup/safety.rs` | safety-backup location, verification, retention, listing |
| `backup/staging.rs` | extends F1's `stage_restore`; staging layout, expiry, migration of the staged copy |
| `backup/preview.rs` | counts, conflicts (D6), blockers (D7), notices (D10) |
| `backup/journal.rs` | `RestoreJournal`, transitions, the atomic write |
| `backup/installer.rs` | `run`, `InstallReport`, the restore and reset steps, rollback, `Fault` (test-only) |
| `backup/process_ops.rs` | `ProcessOperations` (D18) |
| `backup/dialogs.rs` | `PortabilityDialogs`, the native implementation |
| `backup/restart.rs` | `Restarter`, `AppRestarter` |
| `backup/commands.rs` | the nine backup and restore commands |
| `diagnostics/log.rs` | `f3d_log!`, `LogSafe` (sealed), `LogId`, `init`, rotation, the writer thread |
| `diagnostics/pseudonym.rs` | `Pseudonym`, the bundle pseudonymizer |
| `diagnostics/collect.rs` | the six section collectors |
| `diagnostics/egress.rs` | the corpus and `scan` |
| `diagnostics/bundle.rs` | the zip layout and the export |
| `diagnostics/storage.rs` | `StorageUsage`, `clear_storage`, free-space helper (`rustix::fs::statvfs` on Unix, `GetDiskFreeSpaceExW` on Windows) |
| `diagnostics/reset.rs` | tiers (a) and (b), the tier (c) request, `ResetPreview` |
| `diagnostics/about.rs` | `AboutInfo` |
| `diagnostics/commands.rs` | the seven diagnostics, storage, reset, and About commands |

`RuntimeServices` gains `backup: Arc<BackupServices<R>>` (lease, process
ledger, current staging, dialogs, restarter) and `diagnostics:
Arc<DiagnosticsServices>`.

### Schema (migration `0010_p9_portability.sql`)

```sql
-- P9 D11: Job history paging, keyset on the history time, then id.
CREATE INDEX jobs_history ON jobs(COALESCE(ended_at, created_at) DESC, id DESC)
  WHERE state IN ('completed','failed','cancelled','outcomeUnknown');
CREATE INDEX jobs_history_printer ON jobs(printer_id, COALESCE(ended_at, created_at) DESC, id DESC)
  WHERE state IN ('completed','failed','cancelled','outcomeUnknown');

-- P9 D5/D15: camera_snapshots gains the prune reasons 'reset' and
-- 'notInBackup', and a pinned row may be pruned for either. SQLite can't
-- alter a CHECK, and incident_events holds an ON DELETE RESTRICT foreign
-- key to camera_snapshots, so dropping camera_snapshots alone fails once
-- any Incident has evidence. Both tables are rebuilt: the new
-- incident_events references the new camera_snapshots by its temporary
-- name, and the rename rewrites that reference.
CREATE TABLE camera_snapshots_p9 (
  id TEXT PRIMARY KEY CHECK (id GLOB 'snp-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  printer_id TEXT NOT NULL REFERENCES printers(id) ON DELETE RESTRICT,
  incident_id TEXT REFERENCES incidents(id) ON DELETE RESTRICT,
  job_id TEXT REFERENCES jobs(id) ON DELETE RESTRICT,
  trigger TEXT NOT NULL CHECK (trigger IN ('incident','completion','manual')),
  operation_id TEXT UNIQUE,
  captured_at TEXT NOT NULL,
  content_type TEXT NOT NULL CHECK (content_type IN ('image/jpeg','image/png')),
  byte_len INTEGER NOT NULL CHECK (byte_len BETWEEN 1 AND 10485760),
  sha256 TEXT NOT NULL CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
  rel_path TEXT NOT NULL UNIQUE CHECK (rel_path GLOB 'snapshots/*' AND rel_path NOT GLOB '*..*'),
  pinned_at TEXT,
  pruned_at TEXT,
  prune_reason TEXT CHECK (prune_reason IN ('age','diskCap','missingFile','reset','notInBackup')),
  CHECK ((pruned_at IS NULL) = (prune_reason IS NULL)),
  CHECK (pinned_at IS NULL OR pruned_at IS NULL
         OR prune_reason IN ('missingFile','reset','notInBackup')),
  CHECK (trigger <> 'incident' OR incident_id IS NOT NULL),
  CHECK (trigger <> 'completion' OR (job_id IS NOT NULL AND incident_id IS NULL)),
  CHECK ((trigger = 'manual') = (operation_id IS NOT NULL))
) STRICT;
INSERT INTO camera_snapshots_p9(id, revision, printer_id, incident_id, job_id, trigger, operation_id,
    captured_at, content_type, byte_len, sha256, rel_path, pinned_at, pruned_at, prune_reason)
  SELECT id, revision, printer_id, incident_id, job_id, trigger, operation_id,
    captured_at, content_type, byte_len, sha256, rel_path, pinned_at, pruned_at, prune_reason
  FROM camera_snapshots;

CREATE TABLE incident_events_p9 (
  id TEXT PRIMARY KEY CHECK (id GLOB 'iev-*' AND length(id) BETWEEN 5 AND 64),
  incident_id TEXT NOT NULL REFERENCES incidents(id) ON DELETE RESTRICT,
  sequence INTEGER NOT NULL CHECK (sequence >= 1),
  kind TEXT NOT NULL CHECK (kind IN ('opened','eventLinked','reopened','eventAcknowledged',
    'eventResolved','evidenceCaptured','evidenceSkipped','evidencePruned','evidencePinned',
    'evidenceUnpinned','noteAdded','closed')),
  attention_event_id TEXT REFERENCES attention_events(id) ON DELETE RESTRICT,
  snapshot_id TEXT REFERENCES camera_snapshots_p9(id) ON DELETE RESTRICT,
  detail_json TEXT NOT NULL CHECK (json_valid(detail_json)),
  operation_id TEXT,
  at TEXT NOT NULL,
  UNIQUE (incident_id, sequence),
  CHECK (kind NOT IN ('opened','eventLinked','reopened','eventAcknowledged','eventResolved')
         OR attention_event_id IS NOT NULL),
  CHECK (kind NOT IN ('evidenceCaptured','evidencePruned','evidencePinned','evidenceUnpinned')
         OR snapshot_id IS NOT NULL)
) STRICT;
INSERT INTO incident_events_p9(id, incident_id, sequence, kind, attention_event_id, snapshot_id,
    detail_json, operation_id, at)
  SELECT id, incident_id, sequence, kind, attention_event_id, snapshot_id,
    detail_json, operation_id, at
  FROM incident_events;

-- DROP TABLE's implicit delete fires no trigger, so the append-only
-- triggers don't block it; they are recreated below.
DROP TABLE incident_events;
DROP TABLE camera_snapshots;
ALTER TABLE camera_snapshots_p9 RENAME TO camera_snapshots;
ALTER TABLE incident_events_p9 RENAME TO incident_events;

CREATE INDEX camera_snapshots_printer ON camera_snapshots(printer_id, captured_at);
CREATE INDEX camera_snapshots_incident ON camera_snapshots(incident_id) WHERE incident_id IS NOT NULL;
CREATE INDEX camera_snapshots_job ON camera_snapshots(job_id) WHERE job_id IS NOT NULL;
CREATE INDEX camera_snapshots_prunable ON camera_snapshots(captured_at, id)
  WHERE pruned_at IS NULL AND pinned_at IS NULL;
CREATE TRIGGER incident_events_append_only_u BEFORE UPDATE ON incident_events
  BEGIN SELECT RAISE(ABORT, 'incident_events is append-only'); END;
CREATE TRIGGER incident_events_append_only_d BEFORE DELETE ON incident_events
  BEGIN SELECT RAISE(ABORT, 'incident_events is append-only'); END;

-- P9 D15: pending_credential_cleanup gains the reason 'reset'. Nothing
-- holds a foreign key to it.
CREATE TABLE pending_credential_cleanup_p9 (
    credential_ref TEXT PRIMARY KEY CHECK (
        length(CAST(credential_ref AS BLOB)) BETWEEN 1 AND 512
    ),
    printer_id TEXT CHECK (
        printer_id IS NULL
        OR length(CAST(printer_id AS BLOB)) BETWEEN 1 AND 512
    ),
    reason TEXT NOT NULL CHECK (
        reason IN ('provisional', 'replaced', 'cleared', 'printer_deleted', 'import_orphan', 'reset')
    ),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    last_error_code TEXT,
    created_at TEXT NOT NULL,
    last_attempt_at TEXT
) STRICT;
INSERT INTO pending_credential_cleanup_p9(credential_ref, printer_id, reason, attempt_count,
    last_error_code, created_at, last_attempt_at)
  SELECT credential_ref, printer_id, reason, attempt_count,
    last_error_code, created_at, last_attempt_at
  FROM pending_credential_cleanup;
DROP TABLE pending_credential_cleanup;
ALTER TABLE pending_credential_cleanup_p9 RENAME TO pending_credential_cleanup;

-- P9 D18: the operations ledger gains the two database-claimed reset
-- tiers (as 0009 rebuilt 0008's table).
CREATE TABLE operations_p9 (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('moveSpool','archivePrinter','spoolLifecycle','startSlice',
    'createExternalSliceRevision','stageSliceRevision','startStagedArtifact','pauseHostPrint',
    'resumeHostPrint','cancelHostPrint','abandonHostOperation','addToQueue','updateQueueEntry',
    'moveQueueEntry','removeQueueEntry','assignQueueEntry','stageJob','startJob','pauseJob',
    'resumeJob','cancelJob','releaseJob','retryJob','declareJobOutcome','settleJobMaterial',
    'correctJobMaterial',
    'markAttentionRead','acknowledgeAttention','resolveAttention','addIncidentNote',
    'setPrinterCamera','clearPrinterCamera','captureSnapshot','setSnapshotPinned',
    'setPrinterAlertDefaults',
    'resetSettings','resetCameraMedia')),
  request_digest TEXT NOT NULL,
  created_at TEXT NOT NULL
) STRICT;
INSERT INTO operations_p9(id, kind, request_digest, created_at)
  SELECT id, kind, request_digest, created_at FROM operations;
DROP TABLE operations;
ALTER TABLE operations_p9 RENAME TO operations;
```

Notes:

- The migration runs inside `migrations::apply`'s exclusive transaction
  with foreign keys on. The rebuild order was checked against a v9
  database with an `evidenceCaptured` row: dropping `camera_snapshots`
  first fails (`FOREIGN KEY constraint failed`); the order above passes
  `foreign_key_check`.
- After the renames, `sqlite_schema.sql` reads `CREATE TABLE
  "camera_snapshots"(…` and `REFERENCES "camera_snapshots"(id)`, as the
  earlier `operations` rebuilds already read `"operations"`. Task 2's
  schema comparison ignores the quoting and the CHECK text.
- Every column, default, FK, index, and trigger of the three rebuilt
  tables is kept. `migrations.rs` gets `CURRENT_SCHEMA_VERSION = 10`.
- `printers/repository.rs`'s `cleanup_precedence` gains `reset`, between
  `import_orphan` and `provisional` (D15).
- No new column name contains `credential`, `secret`, `token`, `password`,
  or `key`.

### Operation kinds and digests

`OperationKind` gains `ResetSettings` (`resetSettings`, digest
`{ tier, expectedRevision }`) and `ResetCameraMedia`
(`resetCameraMedia`, digest `{ tier, scope }`). The process-local kinds
and digests are in D18. A reused id with a different request is
`VALIDATION` on `operationId` in both ledgers.

### Wire types (ts-rs, camelCase)

```ts
// persistence::integrity (diagnostics and the matrix)
type IntegrityRule = "fk" | "jobCorrectionEvent" | "amountEventReservation" | "reservationHolder"
  | "attentionSource" | "completionEvidence" | "hostOperationGcode" | "embeddedReference"
  | "blobFile" | "mediaFile"
  | "sliceTargetPrinter" | "preparationTargetPrinter" | "orphanBlobFile" | "orphanMediaFile";
type IntegrityOutcome = "violation" | "tolerated";

// history
type JobHistoryState = "completed" | "failed" | "cancelled" | "outcomeUnknown";
type PrinterLifecycleFilter = "any" | "active" | "archived";
type JobHistoryCursor = string;                // opaque; encodes (historyAt, id)
type JobHistoryQuery = {
  states?: JobHistoryState[];                  // 1..=4 distinct; default completed, failed, cancelled
  printerLifecycle?: PrinterLifecycleFilter;   // default "any"
  printerId?: string; spoolId?: string; modelId?: string;
  endedAfter?: string; endedBefore?: string;   // RFC 3339; after inclusive, before exclusive
  text?: string;                               // trimmed, ≤ 200 characters
  after?: JobHistoryCursor;
  limit?: number;                              // 1..=200, default 50
};
type JobHistoryRow = {
  jobId: string; state: JobHistoryState; cancelReason: CancelReason | null;
  historyAt: string; startedAt: string | null; endedAt: string | null;
  printerId: string; printerSnapshotName: string;   // the Job's Printer snapshot
  printerArchived: boolean;                         // current
  spoolId: string; spoolNumber: number;
  modelId: string; modelName: string;               // current
  sliceRevisionId: string; plateName: string | null;
  settlement: Settlement;
  incidentId: string | null; snapshotCount: number; // snapshots with this job_id, pruned included
};
type JobHistoryPage = { rows: JobHistoryRow[]; nextCursor: JobHistoryCursor | null };
type JobTimelineSliceRevision = {
  id: string; kind: SliceRevisionKind; modelId: string; sourceRevisionId: string;
  plate: SlicePlateRef | null; target: SliceRevisionTarget | null;
  runtime: SliceRuntimeInfo | null; estimates: SliceEstimates | null; facts: SliceFacts;
  createdAt: string;
};
type JobTimelineItem =
  | { source: "job"; at: string; event: JobEvent }
  | { source: "hostOperation"; at: string; hostOperation: HostOperation }   // at = createdAt
  | { source: "reservation"; at: string; reservation: Reservation }         // at = createdAt
  | { source: "amountEvent"; at: string; amountEvent: AmountEvent; isCorrection: boolean }
  | { source: "requirement"; at: string; requirement: ReconciliationRequirement } // at = openedAt
  | { source: "attention"; at: string; event: AttentionEvent }              // at = firstObservedAt
  | { source: "incident"; at: string; entry: IncidentEntry }
  | { source: "snapshot"; at: string; snapshot: CameraSnapshot };          // at = capturedAt
type JobTimeline = {
  job: Job; entry: QueueEntry; lineage: QueueEntry[];
  printerSnapshot: PrinterSnapshot; sliceRevision: JobTimelineSliceRevision;
  spoolId: string; spoolNumber: number;
  incident: Incident | null;
  items: JobTimelineItem[];
};

// backup
type BackupMediaChoice = "none" | "pinned" | "all";
type BackupOrigin = "operator" | "beforeRestore" | "beforeReset";
type BackupExcludedClass = "credentials" | "slicerRuntimePaths" | "printerStatusCache"
  | "pendingCredentialCleanup" | "pendingBlobCleanup" | "logs";
type TableCount = { table: string; rows: number };
type BackupInventory = {
  databaseBytes: number;                       // live page_count × page_size
  content: { count: number; bytes: number };
  media: { choice: BackupMediaChoice; count: number; bytes: number }[];  // one per choice, in order
  counts: TableCount[];                        // sorted by table
  credentialRefCount: number;
  activeJobCount: number;
  excluded: BackupExcludedClass[];
};
type CreateBackupOutcome =
  | { status: "cancelled" }
  | { status: "exported"; exportedAt: string; fileName: string; bytes: number;
      media: BackupMediaChoice; mediaNotInBackup: number; mediaMissingFile: number }
  | { status: "unsupported"; reason: "desktopRequired" };
type BackupSummary = {
  backupId: string; origin: BackupOrigin; createdAt: string | null; bytes: number;
  appVersion: string | null; schemaVersion: number | null; media: BackupMediaChoice | null;
  valid: boolean;                              // false: the file didn't parse; only delete is offered
};
type RestoreSource = { kind: "file" } | { kind: "safetyBackup"; backupId: string };
type RestoreCount = { table: string; local: number; backup: number };
type RestoreConflictClass = "onlyLocal" | "changed" | "uniqueClash";
type RestoreDomain = "settings" | "printer" | "spool" | "tare" | "project" | "model"
  | "sliceRevision" | "queueEntry" | "job" | "incident" | "attentionEvent" | "snapshot";
type RestoreConflict = { localId: string | null; backupId: string | null; label: string };
type RestoreConflictGroup = {
  class: RestoreConflictClass; domain: RestoreDomain; total: number;
  items: RestoreConflict[];                    // ≤ 200
};
type RestoreNotice =
  | { kind: "credentialsToReenter"; printerCount: number }
  | { kind: "credentialsOrphaned"; refCount: number }
  | { kind: "linkedPathsMissing"; modelCount: number }
  | { kind: "activeJobsAtBackup"; jobCount: number }
  | { kind: "mediaNotInBackup"; snapshotCount: number; missingFileCount: number }
  | { kind: "slicerRuntimeKeptLocal" }
  | { kind: "migrated"; fromSchemaVersion: number };
type RestoreBlockerKind = "activeJob" | "hostOperation" | "sliceOperation";
type RestoreBlocker = { kind: RestoreBlockerKind; id: string };
type RestorePreview = {
  stagingId: string; createdAt: string; expiresAt: string;
  source: { kind: "file"; fileName: string } | { kind: "safetyBackup"; backupId: string };
  backup: {
    createdAt: string; appVersion: string; schemaVersion: number; formatVersion: 1;
    origin: BackupOrigin; media: BackupMediaChoice; platform: { os: string; arch: string };
  };
  counts: RestoreCount[];                      // every table either side has, sorted
  conflicts: RestoreConflictGroup[];           // only non-empty groups, D6 order
  notices: RestoreNotice[];                    // in the order of the type
  blockers: RestoreBlocker[];                  // ≤ 50
  blockerTotal: number;
};
type PreviewRestoreOutcome =
  | { status: "cancelled" }
  | { status: "previewed"; preview: RestorePreview }
  | { status: "unsupported"; reason: "desktopRequired" };
type ApplyRestoreResult = { status: "restarting"; safetyBackupId: string };
type RestoreStatus =
  | { state: "none" }
  | { state: "done"; journalId: string; kind: "restore" | "reset"; finishedAt: string;
      safetyBackupId: string | null }
  | { state: "failed"; journalId: string; kind: "restore" | "reset"; finishedAt: string;
      code: ErrorCode; failedStep: InstallerStep | null; safetyBackupId: string | null };

// diagnostics
type StorageClass = "database" | "contentModelSources" | "contentThumbnails" | "contentGcode"
  | "contentSliceArtifacts" | "contentSlicerLogs" | "contentUnreferenced"
  | "cameraMediaPinned" | "cameraMediaUnpinned" | "safetyBackups" | "preImportSnapshots"
  | "logs" | "slicerProfileCache";
type StorageUsage = {
  classes: { class: StorageClass; bytes: number; count: number }[];   // every class, in order
  totalBytes: number; measuredAt: string;
};
type StorageCleanupTarget = "unreferencedContent" | "preImportSnapshots" | "rotatedLogs" | "orcaCache";
type ClearStorageResult = {
  target: StorageCleanupTarget; removedCount: number; freedBytes: number; usage: StorageUsage;
};
type DiagnosticsSection = "about" | "health" | "storage" | "configuration" | "logs" | "recentProblems";
type DiagnosticsPreview = { sections: { section: DiagnosticsSection; estimatedBytes: number }[] };
type ExportDiagnosticsOutcome =
  | { status: "cancelled" }
  | { status: "exported"; exportedAt: string; fileName: string; bytes: number;
      sections: DiagnosticsSection[] }
  | { status: "unsupported"; reason: "desktopRequired" };
type ResetTier = "settings" | "cameraMedia" | "farm";
type ResetRequest =
  | { tier: "settings"; expectedRevision: number }
  | { tier: "cameraMedia"; scope: "unpinned" | "all" }
  | { tier: "farm"; safetyBackup: boolean; deleteSafetyBackups: boolean };
type ResetClass = "settings" | "slicerRuntime" | "printerAlertDefaults" | "unpinnedSnapshots"
  | "pinnedSnapshots" | "snapshotRecords" | "printers" | "spools" | "library" | "sliceRevisions"
  | "queueAndJobs" | "incidentsAndAttention" | "content" | "cameraMedia" | "logs"
  | "credentials" | "preImportSnapshots" | "restoreStaging" | "legacyArchives"
  | "safetyBackups" | "slicerProfileCache";
type ResetDataClass = {
  class: ResetClass; effect: "reset" | "pruned" | "deleted" | "kept";
  count: number | null; bytes: number | null;
};
type ResetPreview = {
  tier: ResetTier; phrase: string;             // the exact confirmation phrase
  classes: ResetDataClass[];
  warnings: ({ kind: "activeWork"; activeJobs: number; hostOperations: number; sliceOperations: number }
    | { kind: "credentialStoreUnavailable" })[];
};
type ResetResult =
  | { tier: "settings"; settings: SettingsRecord }
  | { tier: "cameraMedia"; prunedCount: number; freedBytes: number }
  | { tier: "farm"; status: "restarting"; safetyBackupId: string | null };
type AboutInfo = {
  appVersion: string; schemaVersion: number; backupFormatVersion: 1;
  platform: { os: string; arch: string };
  catalog: { sourceTag: string; generatedAt: string };
  slicer: { configured: boolean; version: string | null };   // never a path
  credentialStore: { kind: CredentialStoreKind; available: boolean };
};
```

- `PruneReason` gains `reset` and `notInBackup`, which also reach
  `IncidentEntryDetail`'s `evidencePruned`.
- `NavigationSelectionKind` gains `settingsCategory`.
- `RecoveryCode` gains `OPEN_QUEUE` (open Queue on its Jobs view).

### Commands

18 commands, each registered per Global Constraint 10. Count assertions
become `58 + 21 + 2 + 8 + 11 + 4 + 1 + 2 + 7 + 6 + 5 + 4 + 18`, each task
adding its own term (Task 4: 2, Task 5: 4, Task 6: 2, Task 7: 3, Task 8:
2, Task 9: 3, Task 10: 2). Arguments are top-level camelCase fields.

| Command | Arguments | Result |
|---|---|---|
| `list_job_history` | `{ query: JobHistoryQuery }` | `JobHistoryPage` |
| `get_job_timeline` | `{ jobId }` | `JobTimeline`; `NOT_FOUND` for an unknown Job |
| `backup_inventory` | `{}` | `BackupInventory` |
| `create_backup` | `{ operationId, media: BackupMediaChoice }` | `CreateBackupOutcome` |
| `list_backups` | `{}` | `BackupSummary[]`, safety backups only, newest first |
| `delete_backup` | `{ operationId, backupId }` | `{ backupId: string; deleted: true }`; `NOT_FOUND` for an unknown id |
| `preview_restore` | `{ source: RestoreSource }` | `PreviewRestoreOutcome` |
| `discard_restore_preview` | `{ stagingId }` | `{ discarded: boolean }` (false when nothing was staged under that id) |
| `apply_restore` | `{ operationId, stagingId, confirmation }` | `ApplyRestoreResult` |
| `restore_status` | `{}` | `RestoreStatus` |
| `acknowledge_restore_status` | `{ journalId }` | `RestoreStatus` (`none` after it deletes the finished journal; a `journalId` that isn't the current finished one is `NOT_FOUND`) |
| `storage_usage` | `{}` | `StorageUsage` |
| `clear_storage` | `{ operationId, target: StorageCleanupTarget }` | `ClearStorageResult` |
| `diagnostics_preview` | `{}` | `DiagnosticsPreview` |
| `export_diagnostics` | `{ operationId, sections: DiagnosticsSection[] }` (1..=6, distinct) | `ExportDiagnosticsOutcome` |
| `reset_preview` | `{ tier: ResetTier }` | `ResetPreview` |
| `reset_farm` | `{ operationId, request: ResetRequest, confirmation }` | `ResetResult` |
| `about_farm3d` | `{}` | `AboutInfo` (version from `app.package_info()`) |

- Every command consults the bootstrap state first, as F1 requires, so a
  `RESTORE_FAILED` startup returns that error from each.
- `restore_status` and `acknowledge_restore_status` read and delete the
  journal only when it is `done` or `failed`.
- No P9 command emits an event, except tier (b), which publishes what a
  P8 prune pass publishes on the `attention` stream.

### Error codes

`ErrorCode` gains the codes below. `details` values are camelCase JSON
and never carry a path (except archive-internal entry names that passed
D3's rules), host, URL, credential, name, or matched term.

| Code | Raised by | `details` | `recovery` | Message |
|---|---|---|---|---|
| `BACKUP_INVALID` | `preview_restore`, the installer (as a journal failure code) | `{ reason: BackupInvalidReason, fieldPath: string }` | `[]` | "This file is damaged or isn't a farm3d backup." |
| `UNSUPPORTED_BACKUP_FORMAT` | `preview_restore` | `{ supportedFormatVersion: 1, receivedFormatVersion: number }` | `[UPGRADE_FARM3D]` | "This backup was made by a newer version of farm3d." |
| `RESTORE_BLOCKED` | `apply_restore`, the installer | `{ blockers: RestoreBlocker[] (≤ 50), blockerTotal: number }` | `[OPEN_QUEUE]` | "Finish or resolve the Farm's active work before restoring." |
| `RESTORE_STAGING_EXPIRED` | `apply_restore`, the installer | `{ stagingId: string }` | `[]` | "This restore preview expired. Preview the backup again." |
| `RESTORE_FAILED` | startup (bootstrap `Failed`), the installer (as a journal failure code, after the second attempt) | `{ reason: "journalUnreadable" \| "journalVersion" \| "installFailed" \| "rollbackFailed", step: InstallerStep \| null }` | `[RETRY]` for `installFailed` and `rollbackFailed` (retryable); `[]` otherwise | "farm3d couldn't finish restoring or resetting the Farm." |
| `INSUFFICIENT_SPACE` | `create_backup`, `preview_restore`, `apply_restore`, `reset_farm`, the installer | `{ requiredBytes: number, availableBytes: number, target: "backupDestination" \| "safetyBackup" \| "restoreStaging" \| "install" }` | `[RETRY]` | "There isn't enough free disk space." |
| `BACKUP_IN_PROGRESS` | every lease holder (D5) | `{ activity: "backup" \| "restorePreview" \| "restoreApply" \| "reset" \| "storageCleanup" \| "backupDelete" }` | `[RETRY]` (retryable) | "Another backup, restore, reset, or cleanup is running." |
| `BACKUP_SOURCE_DAMAGED` | `create_backup`, safety backups | `{ entry: string }` (`database` or an archive entry name) | `[]` | "A file this Farm uses is missing or damaged, so it can't be backed up." |
| `DIAGNOSTICS_REDACTION_FAILED` | `export_diagnostics` | `{ section: DiagnosticsSection }` | `[]` | "farm3d stopped the export: the <section> section wasn't fully redacted. Nothing was written." |
| `CONFIRMATION_MISMATCH` | `apply_restore`, `reset_farm` | `{ expected: string }` (the phrase) | `[EDIT_FIELDS]` | "Type the confirmation phrase exactly." |
| `RESTART_PENDING` | `apply_restore`, `reset_farm` (every tier) while a journal is `pending`, `installing`, or `installed` | `{ journalId: string, kind: "restore" \| "reset" }` | `[RESTART_APPLICATION]` | "farm3d is about to restart to finish a restore or reset." |
| `STORAGE_IN_USE` | `clear_storage` (`orcaCache`) | `{ target: StorageCleanupTarget, reason: "sliceRunning" }` | `[RETRY]` | "The slicer is using this data right now." |
| `UNSUPPORTED_SCHEMA_VERSION` | `preview_restore` (existing code) | `{ supportedVersion: CURRENT_SCHEMA_VERSION, receivedVersion: number }` | `[UPGRADE_FARM3D]` | "This backup was made by a newer version of farm3d." |

- The twelve new codes are `BACKUP_INVALID`, `UNSUPPORTED_BACKUP_FORMAT`,
  `RESTORE_BLOCKED`, `RESTORE_STAGING_EXPIRED`, `RESTORE_FAILED`,
  `INSUFFICIENT_SPACE`, `BACKUP_IN_PROGRESS`, `BACKUP_SOURCE_DAMAGED`,
  `DIAGNOSTICS_REDACTION_FAILED`, `CONFIRMATION_MISMATCH`,
  `RESTART_PENDING`, and `STORAGE_IN_USE`. `UNSUPPORTED_SCHEMA_VERSION` gains a backup
  constructor with the details above.
- As a journal failure code, `RESTORE_FAILED` carries only the failed
  step (`RestoreStatus.failedStep`); the `reason` details belong to the
  startup error.
- Reused, unchanged: `VALIDATION` (field paths as named above, and a
  reused `operationId`), `NOT_FOUND`, `CONFLICT` (tier a),
  `PERSISTENCE_UNAVAILABLE`, `INTERNAL` (a diagnostics bundle over
  32 MiB).
- `RepositoryError` gains `BackupInProgress`, `ConfirmationMismatch`, and
  `StorageInUse`, each mapped in `CommandError::from_repository`; the
  backup and restore errors are built by `backup::commands` directly.

## Frontend architecture

Rust is the only source of conflicts, blockers, redactions, reset scopes,
and cleanup eligibility. TypeScript presents them.

- **`src/history/history-store.ts`**: one paged query. A filter or text
  change is debounced 250 ms, resets the cursor, and replaces the rows;
  "Load more" appends the next page. Each request carries a sequence
  number, and a response older than the latest is dropped.
  `presentation.ts` labels every `JobHistoryState`, `JobTimelineItem`
  source, and `PruneReason` (including "Removed by a reset" and "Not in
  the backup").
- **`src/backup/backup-store.ts`**: `inventory`, `createBackup`,
  `listBackups`, `deleteBackup`, and the restore flow as a state machine:
  `idle → choosing → previewing → confirming → restarting`, with
  `failed` and `cancelled` branches that return to `idle`. It holds the
  current `RestorePreview`, discards it on leaving the panel, and reads
  `restore_status` once at startup. `presentation.ts` groups conflicts
  and words the notices.
- **`src/diagnostics/diagnostics-store.ts`**: `diagnosticsPreview`,
  `exportDiagnostics`, `storageUsage`, `clearStorage`, `resetPreview`,
  `resetFarm`, and `about`. No result ever holds a path; the store shows
  `fileName` at most.
- Every write uses a fresh `crypto.randomUUID()` `operationId`, reused on
  one transport retry. **Web mode**: D18.
- **Design system.** `Tabs` gains `orientation` (`horizontal` default,
  `vertical`), passed to Kobalte's `Tabs.Root`, so arrow keys follow it
  and `aria-orientation` is set. `TypedConfirmDialog` (generalized from
  `DeletePrinterDialog`): props `title`, `consequences: string[]`,
  `phrase`, `confirmLabel`, `pending`, `onConfirm`, `onOpenChange`; exact
  matching; the input resets on open; the consequence list is the
  dialog's description. Both go into `components/index.ts` and
  `Showcase.tsx`. `DeletePrinterDialog` uses it.
- **`SettingsWorkspace`** (lazy, `src/screens/`): a `Panel` with vertical
  `Tabs` of the eight categories (D16), each category a lazy component
  under `src/screens/settings/`. The selected category follows the
  navigation selection.
- **Storage and backup**: a `DataTable` of `StorageUsage` classes with
  each cleanup action beside its class (pending and result states); the
  backup section with a media `RadioGroup` whose options show
  `BackupInventory` counts and sizes, and Create backup; Restore from
  file; the safety-backup list with Restore and Delete.
- **`RestorePreviewPanel`**: the backup's facts, counts side by side
  (local, backup), conflict groups by class then domain with "and N more"
  past the 200-item cap, the notices, and the blockers (with Open Queue).
  Blockers disable Restore. Restore opens `TypedConfirmDialog` (phrase
  `restore`) whose consequences state that a safety backup is written
  first and that farm3d will restart. On success the panel shows
  "Restarting…".
- **Diagnostics**: section checkboxes with descriptions and estimated
  sizes, a fixed statement of what is never included (credentials,
  hosts, URLs, paths, names), and Export. `DIAGNOSTICS_REDACTION_FAILED`
  shows its section and says nothing was written.
- **`ResetPanel`**: three tiers, each listing its `reset_preview` classes
  (affected and kept), its warnings, and its own `TypedConfirmDialog`
  phrase. Tier (b) has the scope choice; tier (c) has "Write a safety
  backup first" (on) and "Also delete safety backups" (off).
- **Restore status banner**: when `restore_status` is `done` or `failed`
  at startup, `AppShell` shows one banner (the outcome, the safety backup
  it can restore from) until the operator acknowledges it.
- **History**: `JobHistoryView` replaces the Queue History tab: the
  search field, state, Printer (archived Printers included and
  labelled), and date filters, a `DataTable` with a "Load more" row, and
  empty, filtered-empty, loading, and error states. Selecting a row
  navigates to `queue/job/<id>`. `JobTimelinePanel` (in `JobPanel` and
  `QueueEntryDetail`) renders every item kind with a text label, time,
  and detail, a material section (reservation, deduction, correction),
  and links to `monitor/incident/<id>` and the snapshot viewer; pruned
  evidence shows as text.
- **Shell.** `ActivityBar` gets the Settings button (the gear icon) as a
  destination, marked current on Settings. `AppShell` reads the version
  from `about_farm3d`.

## Accessibility and adaptation

- Vertical tabs expose `aria-orientation="vertical"` and move with
  ArrowUp and ArrowDown.
- `TypedConfirmDialog` labels its input with the exact phrase ("Type
  reset farm to confirm"), states each consequence as text, and keeps the
  confirm button disabled, not hidden, until the text matches. Destructive
  confirm buttons use the danger role and never rely on an icon alone.
- The restore preview's conflict groups are headed lists with their
  totals in text. Blockers are a list with links.
- One polite live region announces "Restarting farm3d to finish the
  restore" and export results; errors use the existing alert pattern.
- The history table is a `DataTable` with a caption, and "Load more" is a
  real button. The timeline is an ordered list with visible times.
- Every dialog is a Kobalte `Dialog` that traps and returns focus.
- The layout works at 1440 × 900 and 1024 × 700. Tokens, CSS Modules,
  and Kobalte only, in the dense editor aesthetic: no elevation, no
  ripple.

## Acceptance criteria

1. **Migration.** 0010 applies to a populated v9 database and keeps every
   row; the three rebuilt tables keep every column, default, FK, index,
   and trigger (schema compared ignoring CHECK text and name quoting); the
   new CHECK values are accepted, including a pinned row pruned `reset` or
   `notInBackup`; a v11 database is refused; 0010 is atomic
   (`apply_through_failing_before_commit`).
2. **Integrity.** Every D17 rule has a positive and a negative test; a
   clean every-domain Farm has no violation; counts equal `count(*)`.
3. **Log.** Free text can't reach the log (the Task 3 proof); rotation
   keeps five files; a write failure never blocks or panics; no
   `eprintln!` remains; the corpus scan over the log files passes.
4. **History.** Every D11 filter alone and combined; archived Printers'
   Jobs by snapshot name; escaped and non-ASCII text; stable keyset
   paging; the limit bounds; `outcomeUnknown` excluded by default; both
   indexes used; every timeline item kind; the immutability proof.
5. **Backup.** For each media choice the archive holds exactly the
   expected entries; the copy is sanitized and `VACUUM`ed; counts equal
   the source's; the lease holds back blob and media deletion; a second
   lease holder gets `BACKUP_IN_PROGRESS`; the backup subset of the
   corpus (`secrets::BACKUP_FORBIDDEN`: every credential value, the
   header value, the userinfo URL form, its `user:password@` part, and
   its password) is absent from the archive, every decompressed entry,
   and the copy's free pages. The rest of the corpus (hosts, Printer
   names, the home path, the stored camera URL with its query token) is
   Farm data a whole-Farm backup legitimately holds (D10, decision 14),
   so it is not asserted absent.
6. **Restore preview.** Every "Conflict fixtures" row; every D3 and D4
   rejection; notices and blockers; the live database's bytes are
   unchanged by a preview.
7. **Installer.** Every "Installer fault points" row; restored active
   Jobs go through P7 startup recovery exactly as after a restart.
8. **Reset.** Each tier touches exactly its classes; every reset row of
   the fault table.
9. **Diagnostics.** Each section holds its expected facts; no corpus item
   reaches any section; pseudonyms are stable within a bundle and differ
   between bundles; a deliberately leaking collector is caught before any
   file is written.
10. **Storage.** Usage equals the bytes on disk for a quiescent Farm;
    each cleanup target frees only what D14 says and leaves
    `integrity::check` clean.
11. **Reference matrix.** Every archive, delete, prune, cleanup, reset,
    and restore row of Task 11 passes, with decision 20's movement
    cascade as a named allowed loss.
12. **Fixture compatibility.** The committed `formatVersion` 1 fixture
    restores; each tampered variant is refused with its code.
13. **Secrets.** The full shared corpus (Task 2) never appears in a
    diagnostics bundle or a log line. `secrets::BACKUP_FORBIDDEN` (credential
    values, the header value, the userinfo URL and its parts) never
    appears in a backup, a journal, an error, an event, or frontend state.
14. **Frontend.** The Task 12–16 tests pass; theme preview behaves as
    before; the main chunk doesn't grow; screenshots exist at both
    viewports.
15. **Tracer.** The plan's Task 18 tracer passes on the fakes (CI) and
    the simulator, with the manifest committed.
16. **Installed bundle.** Task 19's checklist on the installed deb (Linux
    x86_64), with Windows and macOS recorded as unverified.

## Delivery

| Task | Delivers from this spec |
|---|---|
| 2 | Migration 0010, `cleanup_precedence`, D17, the shared secret corpus |
| 3 | D12, `log_root`, the startup log init |
| 4 | D11 and its two commands |
| 5 | D2, D3's writer side, D5, D9, `backup_root`, `ProcessOperations`, `PortabilityDialogs`, the first cut of the every-domain Farm |
| 6 | D3's reader side, D4, D6 with "Conflict fixtures", D7's preview, D10's notices, staging |
| 7 | D8 with "Installer fault points" (restore rows), `Restarter`, the startup order |
| 8 | D15 with the reset fault rows |
| 9 | D13, About |
| 10 | D14 |
| 11 | The extended every-domain Farm, the D17 audit, the matrix |
| 12–16 | Frontend, D16 |
| 17 | D4's fixture rule |
| 18–20 | Tracer, installed bundle, verification record |

## Decisions made in this spec

Each departs from, or sharpens, the plan's Design reference or its
evidence. Where the code disagreed with the plan, the code won.

1. **18 commands, not 17.** The plan's table lists `restore_status` and
   `acknowledge_restore_status` in one row; they are two commands. The
   P9 count term is 18.
2. **Migration 0010 rebuilds `incident_events` too.** The plan said to
   rebuild `camera_snapshots` "following the `operations` rebuild
   pattern". That pattern only works for a table nothing references:
   `incident_events.snapshot_id` is `REFERENCES camera_snapshots(id) ON
   DELETE RESTRICT`, so dropping `camera_snapshots` fails as soon as any
   Incident has evidence (checked against a v9 database). Both tables are
   rebuilt, in the order the migration shows.
3. **The pinned-and-pruned CHECK is widened as well.** 0009's `CHECK
   (pinned_at IS NULL OR pruned_at IS NULL OR prune_reason =
   'missingFile')` would reject both a pinned snapshot left out of a
   `media: none` backup (`notInBackup`) and tier (b)'s `all` scope
   (`reset`). The plan only widened the reason list.
4. **History pages on `COALESCE(ended_at, created_at)`.** The plan's
   index was on `ended_at` alone, but an `outcomeUnknown` Job has no
   `ended_at` (0008's CHECK), and decision 18 allows it in the results.
   `created_at` is the fallback because it never changes, and
   `updated_at` does (fix round 1: `write_row` always bumps it, including
   from `dispatch::apply_host_outcome` on an `outcomeUnknown` Job). There
   are two partial indexes, `jobs_history` and `jobs_history_printer`,
   the query names one with `INDEXED BY` (no `sqlite_stat1` exists), and
   the keyset predicate is written so SQLite seeks (checked with
   `EXPLAIN QUERY PLAN`).
5. **`operationLedger` is not an integrity rule.** The plan listed "the
   `operation_id` columns → `operations`" as unenforced references. They
   are idempotency keys, not references: 0004's comment and P3's code
   record Printer create's initial loads with a server-generated id that
   is never in `operations`, so the rule would fire on a clean Farm. The
   catalogue instead adds `completionEvidence`, `hostOperationGcode`, and
   `preparationTargetPrinter`, which the code does rely on.
6. **Most P9 writes use a process-local operation ledger.** Global
   Constraint 11 said every mutating command claims through
   `spools::operations::claim`. That claim is a row in the same
   transaction as the write; a file write has no transaction, and
   `apply_restore` and tier (c) replace the database the row would be in.
   Only tiers (a) and (b) claim in the database; `operations` gains just
   `resetSettings` and `resetCameraMedia`.
7. **Content and media are staged at preview, beside their roots.** The
   plan staged only the database under `snapshots/.restore-staging/`.
   Staging blobs under `content_root/staging/` and media under
   `media_root/restore-<id>/` keeps install to same-filesystem renames and
   means the operator's backup file can move or vanish after the preview.
8. **Tier (c) moves roots to siblings, not into `restore/<id>/previous/`.**
   Decision 17 moved `content_root`, `media_root`, and `log_root` into
   the journal's `previous/` under `metadata_root`, but `metadata_root`
   (`app_config_dir`) and the data roots (`app_data_dir`) may be on
   different filesystems, where a rename fails. Each root moves to a
   sibling `.aside-<journalId>-<name>`. Tier (c) also clears pre-import snapshots,
   restore staging, and F0 legacy archives, which hold Farm data too.
9. **The manifest is closed and gains `origin`, `media`, and
   `pendingBlobCleanup`.** `origin` lets safety backups list their reason;
   `media` records how many snapshots were left out or found missing;
   `pending_blob_cleanup` rows name this machine's files, so they are
   sanitized like `pending_credential_cleanup`.
10. **Missing media is found in a pre-pass**, so the manifest can be the
    first entry and still be exact: a missing or altered media file is
    marked `missingFile` in the copy before the copy is hashed. A content
    blob that fails its own hash aborts with the new
    `BACKUP_SOURCE_DAMAGED`, since it means the local Farm is damaged.
11. **No `evidencePruned` entry for `notInBackup` or `missingFile` in a
    backup.** Appending timeline rows to the copy would make a backup's
    Incident timelines differ from the source's; the snapshot row carries
    the pruned state.
12. **The installer's blocker re-check opens the live database without a
    checkpoint on close** (`SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE`,
    `query_only`), so a rollback can still be byte-exact for the main
    file and its `-wal`. `-shm` is excluded from byte comparison.
13. **The lease is retried for up to 10 s at startup when a journal
    exists**, because `AppHandle::restart` can start the new process
    before the old one releases the metadata lock. Without a journal,
    F1's immediate failure is unchanged.
14. **The log starts after the installer**, because tier (c) moves
    `log_root`; the installer's report is logged once the log is open.
15. **`log_root` defaults to `<app_data_root>/logs`**, and production
    sets it to `app_log_dir()` with `with_log_root`, so the 102 existing
    `StoragePaths::new` calls don't change. `backup_root` is
    `<app_data_root>/farm3d-backups/v1`.
16. **Log lines split `ids` from `fields`.** A bundle rewrites every
    `ids` value to its bundle pseudonym, so no raw id (an F0 Printer id
    can be any string) leaves the machine.
17. **Bundle pseudonyms are ordinals over a salted hash** (`printer-3`),
    stable within a bundle and different between bundles, using SHA-256
    only (no new dependency).
18. **Name terms are scanned only in non-enum string values**, so a
    Printer named after a state word can't make every export fail; secret,
    host, URL, and path terms are scanned in every byte.
19. **Restore's retry budget is two attempts.** A `validate` failure
    fails at once, since the same candidate can't pass on a retry.
20. **Tier (c) is not refused for work in flight.** It is the escape
    hatch; its preview warns instead. Restore stays refused (decision 10).
21. **`reset` is an automatically eligible cleanup reason**, below every
    other automatic reason and above `import_orphan`.
22. **`list_backups` lists safety backups only**, and the last outcome
    moves to `restore_status`. farm3d doesn't track where operator
    backups were saved.
23. **New codes beyond the plan's list:** `RESTORE_FAILED` (Task 7's
    bootstrap state needed a code), `BACKUP_SOURCE_DAMAGED`, and
    `STORAGE_IN_USE` (Task 10's `orcaCache` refusal). New recovery code
    `OPEN_QUEUE`, which `RESTORE_BLOCKED` names and `RecoveryCode` lacked.
24. **Storage usage uses apparent file length**, so Task 10's tolerance
    is zero for a quiescent Farm.
25. **Spool movements stay off the Job timeline.** The umbrella lists
    movements under History, but `spool_movements` has no Job column; the
    timeline shows the reservation, deductions, and corrections.
26. **Rename:** the history row's Printer name is `printerSnapshotName`,
    to say which name it is.
27. **Preflight rulings carried in** (controller, 2026-09-29): the
    every-domain Farm builder `tests/p9_farm/mod.rs` is first landed by
    Task 5 with fixed ids and timestamps, and Task 11 extends it without
    renaming; Task 4 may amend migration 0010 in place to add history
    indexes; the shared corpus's camera URL with userinfo is used only on
    error and log paths, and a *stored* camera URL seed carries only a
    query token (P8 rejects userinfo at save).
28. **(Fix round 1) Rollback is two-phase with a durable marker**
    (`journal.rollback`), so a crash inside a rollback never deletes an
    original database file or separates the WAL from its database; there
    is no `rollback` `InstallerStep`.
29. **(Fix round 1) `swapMedia` records whether `<media_root>/snapshots`
    existed**, because P8 creates it lazily; rollback restores its absence.
30. **(Fix round 1) Byte-exact is measured before `recheckBlockers`**, with
    `-shm` ignored and an empty `-wal` allowed, since opening a cleanly
    closed WAL database creates both.
31. **(Fix round 1) The backup corpus check is scoped** to
    `BACKUP_FORBIDDEN`; hosts, names, paths, and the stored camera URL are
    Farm data a whole-Farm backup carries.
32. **(Fix round 1) Carried credential-cleanup rows whose ref the restored
    Printers use are dropped**, not downgraded, so F1's startup retry can't
    delete a restored Printer's credential.
33. **(Fix round 1) `RESTART_PENDING`** refuses a second restore or any
    reset while an unfinished journal exists, and `apply_restore` keeps
    the lease until the process exits.
34. **(Fix round 1) History queries name their index** (`INDEXED BY`), and
    the egress corpus adds credential refs and every known absolute path.

## Residual risks

- **`AppHandle::restart` in the installed bundle.** Restart must
  relaunch the same binary with the same environment (deb, AppImage).
  Task 19 checks it. If restart is unreliable, `apply_restore` asks the
  operator to quit and reopen instead, and the journal makes that equally
  safe.
- **No backup progress or cancel.** A Farm with many large Models can
  make a multi-gigabyte archive. The free-space check prevents a
  half-written file, but not a slow one; the UI shows only a pending
  state.
- **The egress scan uses exact matching** plus a few variants. It
  catches known values, not every transformation. The typed log and the
  typed collectors are the primary control.
- **The egress scan fails open on an unreadable credential store.** A
  ref whose value can't be read (a locked keychain, a damaged fallback
  file) contributes no term, and the export goes on; the count is logged
  as `diagnostics.credentialCorpusUnavailable`. No collector reads a
  credential, so the scan is only the second control for those values.
- **Linked-path portability is only surfaced.** Restoring onto another
  machine leaves linked Models `missing` until **Locate source**.
- **Printer delete still drops unlinked Spool movement history**
  (decision 20). It is intentional and documented, not an integrity
  defect.
- **Printers import stays blocked once any Job exists** (decision 3).
  Whole-Farm restore replaces that workflow.
- **Writes between the safety backup and the restart are lost.** A Job
  command or capture committed after `apply_restore`'s safety backup and
  before the process exits (the journal write plus 500 ms) is in neither
  the safety backup nor the restored Farm. The lease stops every other
  P9 operation in that window, and the blocker re-check at install
  refuses the restore if the write started new work; a non-blocking write
  (a note, a setting) is accepted as lost.
- **A restore on another platform** (paths, keyring) is unverified:
  Linux x86_64 only (decision 21).
- **Notes are verbatim operator text** in a backup, as in the Printers
  export. They never reach a diagnostics bundle.
