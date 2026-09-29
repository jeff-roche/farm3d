# P9 History, Settings, Backup/Restore, and Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. One fresh implementer per `### Task N` section, and a review after each task. Every task section is written to stand alone. It still binds you to **§Global Constraints**, which the controller hands to every implementer with the task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Approved, ready for Task 1. GitHub issue #19. Written on
2026-09-29 against `main` at `727d9e2` (the P8 merge, PR #32). Its
blockers (#12–#18) are closed, so every product schema from P2–P8 is
stable. The owner fixed decisions 1–4 on 2026-09-29 and confirmed
§Planner defaults 5–21 as written on the same day.

**Approval.** After the owner confirms §Planner defaults, there are no
more user approval stops, with one exception: Task 19 needs someone at
a desktop session to drive the installed bundle. Where this plan says
**controller approves**, the controller (the agent orchestrating
subagent-driven development) reviews the output, records the approval in
that document's `Status` line, and continues. That applies to the
focused spec, ADR-0016, and ADR-0017 (Task 1).

**Goal:** An operator can:

- search and page through every finished Job, including Jobs on
  archived Printers, and open an immutable timeline that joins the Job's
  events, material ledger, host work, Incident, and surviving evidence;
- manage farm3d from a full **Settings workspace**, which replaces the
  gear menu and keeps theme preview;
- back up the whole Farm to one versioned, checksummed archive, with
  credentials excluded;
- preview a restore (counts, conflicts, and what needs re-entry), then
  restore after an automatic safety backup, and trust that a crash
  mid-restore either finishes or rolls back and never leaves a mixed
  Farm;
- see and reclaim disk use;
- export a selectable diagnostics bundle that holds no credential, host,
  URL, path, or user-authored name;
- deliberately reset settings, camera media, or the whole Farm behind
  typed confirmation.

After P9, a final cross-domain matrix proves that no archive, delete,
restore, or reset path leaves a dangling reference.

**Architecture:** four new Rust modules, one new persistence helper, and
no new product-domain behavior (issue #19's explicit non-scope).

- `persistence/integrity.rs` is the single reference-integrity checker:
  `PRAGMA foreign_key_check`, plus a named catalogue of the soft
  references SQLite can't enforce (polymorphic ids, FK-less columns,
  blob files, media files). Restore validation, the diagnostics storage
  section, and the final matrix all call it.
- `history` is a read model over the P7/P8 tables. It adds no writes.
- `backup` owns the archive format, the inventory, the writer, restore
  staging and preview, the **restore journal**, and the startup
  installer. Restore swaps in the entire Farm (decision 1) at the next
  startup, *before* `Storage::open`. That point has no live writer, no
  supervisors, and no WAL in use, so replacing the database and its
  sidecars is safe. A crash anywhere is recovered by the same installer
  on the next start.
- `diagnostics` owns a structured, **safe-by-construction** log (typed
  fields only, no free text, rotated files), the bundle collectors,
  pseudonymization, the egress scan, the storage-usage view, and the
  tiered reset.

The frontend:

- adds `src/history/`, `src/backup/`, and `src/diagnostics/` stores;
- adds a lazy-loaded `SettingsWorkspace` on the existing `settings`
  navigation destination, with eight categories;
- replaces the Queue screen's 100-row History tab with a paged,
  searchable Job history and a full Job timeline.

It presents what Rust returns and never decides a conflict, a redaction,
or a reset scope.

**Tech Stack:** Rust (Tauri 2, rusqlite with the `backup` feature and
STRICT tables, `zip` 8.6 and `sha2` already in `Cargo.toml`, tokio,
ts-rs), SolidJS + TypeScript, Kobalte, CSS Modules, vitest +
`@solidjs/testing-library`. The test doubles are the P7 rig, the P8
fakes, and a new every-domain Farm builder.

**Spec:** Task 1 writes
`docs/superpowers/specs/2026-09-29-p9-history-settings-backup-diagnostics-design.md`.
**Once the spec is approved, the spec wins wherever the two differ**, and
Task 1 updates the affected tasks in the same commit. The umbrella inputs
are:

- `docs/superpowers/specs/2026-09-16-complete-v1-ui-workflows-design.md`
  ("History", "Settings and local data", "Backup and restore",
  "Diagnostics and reset");
- `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`
  ("Phase P9", and the "Backup format/conflict policy" and "Diagnostics
  redaction" known-unknown rows);
- ADR-0008 (online backup API only; P9 owns sidecar-safe replacement,
  rollback, and full backup/restore);
- the F1 design's "Safety snapshots and restore staging", "Security
  boundaries", and "Startup order".

## Owner decisions (fixed)

The owner chose these on 2026-09-29. Don't reopen them.

1. **Restore replaces the whole Farm. There is no merge.**
   - Before any write, the preview lists every conflict in three
     classes:
     - `onlyLocal`: the row will be removed, and the safety backup keeps
       it;
     - `changed`: the same id with different content;
     - `uniqueClash`: a different id with the same natural key (Printer
       host identity, Spool number, Project name, tare name).
   - Restore is **refused** while local work is in flight (decision 10).
   - Referential integrity holds by construction, because the database
     is swapped whole.

   (Tasks 6 and 7.)
2. **A backup is the whole Farm plus optional camera media.**
   - The database and every referenced content blob (Model sources,
     thumbnails, G-code, slice artifacts, and slicer logs) are always
     included.
   - Camera snapshots are optional: `none`, `pinned`, or `all`. A
     snapshot left out is restored as pruned with reason `notInBackup`,
     so its textual history stays.
   - Machine-specific paths (the OrcaSlicer engine and preset source)
     are never restored (decision 11).
   - The Settings and Printers JSON exports stay unchanged for partial
     portability.

   (Tasks 5 and 6.)
3. **Job history is never pruned in v1.**
   - Jobs, Incidents, Attention Events, and their timelines stay
     immutable.
   - Disk management targets content blobs, camera media, safety
     backups, logs, pre-import snapshots, and the OrcaSlicer profile
     cache.
   - Printers import stays blocked by `JOBS_EXIST` (P7). Whole-Farm
     restore is how a Farm moves between machines.

   (Tasks 4 and 10.)
4. **Resets are tiered, and each needs typed confirmation.**
   - (a) **Settings:** restore the defaults.
   - (b) **Camera media:** delete unpinned media, or all media. Rows are
     kept as pruned.
   - (c) **Entire Farm:** delete the database, content, media, logs,
     and farm3d's own credentials, with an optional safety backup first.
   - Each tier names every affected data class. Tier (c) runs through
     the restore journal, so a crash rolls it forward to completion.

   (Tasks 8 and 15.)

## Planner defaults (owner confirms before Task 1)

These are recommendations, not approved decisions. Each names what
changes if the owner picks differently.

5. **Archive format.**
   - One zip file, `farm3d-<UTC yyyymmddThhmmssZ>.farm3d-backup`,
     written through the native save dialog with the P1 atomic-write
     rule (`document_io.rs`).
   - Entries, in order:

     | Entry | Contents | Compression |
     |---|---|---|
     | `manifest.json` | the format, versions, contents, counts, excluded classes, and per-entry checksums | deflate |
     | `database/farm3d.sqlite3` | an online-backup-API copy, sanitized (decision 11) and `VACUUM`ed | deflate |
     | `content/sha256/<hh>/<hex>` | one entry per `content_blobs` row | deflate, or stored for 3MF |
     | `media/snapshots/<yyyy>/<mm>/<snp-id>.<ext>` | the selected camera frames | stored |

   - `manifest.json` has these fields:
     - `format: "farm3d-backup"`, `formatVersion: 1`, `createdAt`,
       `appVersion`, `schemaVersion`, and `migrations` (the version,
       name, and checksum of each);
     - `platform` (OS and architecture only);
     - `contents` (`media: none|pinned|all`);
     - `counts` (one per table);
     - `excluded`: `credentials`, `slicerRuntimePaths`,
       `printerStatusCache`, `pendingCredentialCleanup`, and
       `logs`;
     - `credentialRefCount`;
     - `entries[] {path, bytes, sha256}`.

   *If changed:* a directory format or tar would drop the zip dependency
   reuse but not change any other decision.
6. **Checksums and archive safety.** The threat model is corruption,
   truncation, and a hostile or malformed file. Tampering by someone who
   can also rewrite the manifest is out of scope: the archive is not
   signed.
   - Every entry's SHA-256 is verified against the manifest twice: at
     preview, while streaming, and again at extraction.
   - An entry missing from the manifest, a manifest entry missing from
     the archive, a duplicate path, or a path that is absolute, holds
     `..`, holds a backslash, or holds a control character are each
     `BACKUP_INVALID` with a safe field path.
   - Limits:
     - 64 MiB for the manifest;
     - the sum of the manifest's `bytes` must not exceed free space
       minus 10 %;
     - each entry's uncompressed stream is capped at its declared
       `bytes`, which defeats a zip bomb;
     - at most 1,000,000 entries.
   - A content entry's hash must equal its path's hex.
7. **Compatibility window.**
   - Restore accepts `formatVersion` 1, and any `schemaVersion` from 10
     (the P9 schema, the first one a backup can carry) through the
     current version.
   - An older schema is migrated forward *on the staged copy* with the
     same embedded migrations. The live database is untouched until
     install.
   - A newer `schemaVersion` gives `UNSUPPORTED_SCHEMA_VERSION`, and a
     newer `formatVersion` gives `UNSUPPORTED_BACKUP_FORMAT`.
   - A migration-checksum mismatch in the manifest or in the staged
     `schema_migrations` gives `BACKUP_INVALID`.
   - Every committed fixture backup must keep restoring for as long as
     its format version is accepted (Task 17).
8. **Consistency.** A backup is a point-in-time Farm, not a copy of live
   files.
   - The database copy comes from the online backup API (`snapshot.rs`'s
     machinery, generalized) and is validated: `integrity_check`,
     `foreign_key_check`, and `integrity::check`.
   - Blobs are content-addressed and immutable, so copying exactly the
     hashes the *snapshot* lists is consistent, provided none is deleted
     mid-copy. While a backup runs, a `BackupLease` pauses
     `pending_blob_cleanup` processing and the `MediaJanitor`. Both are
     already deferred and retried, so a pause is safe.
   - A media file missing at copy time is recorded in the manifest as
     `missingFile` and restores as pruned `missingFile`. It is not a
     backup failure.
9. **Install happens at the next startup, driven by a journal.**
   - `apply_restore` does three things:
     1. It writes a safety backup.
     2. It writes `<metadata_root>/restore/journal.json` with the phase
        `pending`, the staging id, the safety-backup path, the expected
        counts, and the credential refs that will become orphans.
     3. It restarts farm3d (`AppHandle::restart`).
   - At startup, after the ownership lease and before `Storage::open`,
     `backup::installer::run` does the rest:
     1. It sets `installing`.
     2. It moves the live `farm3d.sqlite3` with its `-wal` and `-shm` as
        a set into `restore/<id>/previous/`.
     3. It places the staged candidate.
     4. It extracts blobs into `content_root`. This step adds files and
        never removes any.
     5. It stages the media and swaps it in by directory rename.
     6. It validates the result: open, migrate, `integrity_check`,
        `foreign_key_check`, `integrity::check`, and counts equal to the
        manifest's.
     7. It records `installed`, deletes `previous/`, and sets the phase
        `done {outcome}`, which stays until `restore_status` is
        acknowledged.
   - Recovery:
     - Any crash before `installed` restores the `previous/` set and
       retries once from `pending`.
     - A second failure, or any validation failure, rolls back to
       `previous/` and records `failed {code}`. The Farm is then exactly
       the pre-restore Farm.
     - Blobs added by a rolled-back install become orphans, and the
       existing startup sweep removes them.

   *If changed:* a live in-process swap would have to stop every
   service (supervisors, projector, evaluator, janitor, notifications)
   and reopen `Storage`. That is a much larger change for no user
   benefit.
10. **Refusal policy.**
    - Restore is refused with `RESTORE_BLOCKED`, which lists blockers,
      while the **local** Farm has any of these:
      - a non-terminal Job (including `outcomeUnknown`);
      - an unresolved Host Operation;
      - a queued or running slice operation.

      Replacing any of these would orphan live host work.
    - A **backup** taken with active Jobs is allowed. The preview warns
      that farm3d will reconcile those Jobs with their Printers at
      start. That reuses P7's restart recovery (`p7_restart_matrix.rs`).
    - Blockers are re-checked inside `apply_restore`, and again by the
      installer before `installing`. If the check fails there, the
      journal is `failed {RESTORE_BLOCKED}` and nothing is moved.
11. **Machine-specific data.**
    - The backup copy is sanitized before it is hashed:
      - `slicer_runtime_config` paths are set to NULL;
      - `printer_status_snapshots` rows are deleted, because they are a
        telemetry cache;
      - `pending_credential_cleanup` rows are deleted, because they are
        refs into *this* machine's store;
      - then the copy is `VACUUM`ed, so no freed page keeps old values.
    - At install, the local `slicer_runtime_config` row is carried into
      the new database.
    - `library_models.linked_path` and
      `model_source_revisions.source_path` are restored verbatim. A path
      that doesn't exist on this machine surfaces through P4's existing
      source state and **Locate source**. The preview counts such paths.
12. **Credentials on restore.**
    - Credential values are never read into or out of a backup.
    - After install, every restored `credentialRef` that the local store
      lacks gives that Printer `CREDENTIAL_REQUIRED` (existing
      behavior). The preview counts these by checking the store for each
      ref; the value is dropped immediately (`Zeroizing`).
    - Local refs that the new Farm no longer references are queued in
      the new database's `pending_credential_cleanup` with reason
      `import_orphan`, which F1 never deletes automatically. So a
      rollback, or restoring the safety backup, still finds them.
13. **Safety backups.**
    - They use the same format with `media: all`, written to
      `<app_data_root>/farm3d-backups/v1/safety/`.
    - The newest 3 are kept, and older ones are deleted only after a new
      one validates.
    - Before starting, farm3d checks free space for the estimated size
      plus 10 % (`INSUFFICIENT_SPACE`).
    - Safety backups are listed in Storage and can be restored or
      deleted there. They survive every reset unless the operator ticks
      "also delete safety backups".
14. **Camera snapshot URLs stay in the database copy.**
    - They are Printer configuration: P8 carries them in the Printers
      export, and the backup is the same kind of owner-held file.
    - Diagnostics never carry them (decision 16).
    - *If changed:* the backup nulls `printer_cameras.snapshot_url` and
      the operator re-enters the URL after restore, like a credential.
15. **Logging.** Today there is no log file, only 20-odd `eprintln!`
    sites.
    - A new `diagnostics::log` writes JSON lines to `<log_root>/farm3d.log`,
      where `log_root = app_log_dir()`. It rotates at 2 MiB and keeps 5
      files.
    - Its macro, `f3d_log!(level, "domain.code", key = value, …)`,
      accepts only values that implement a sealed `LogSafe` trait:
      - ids;
      - `ErrorCode` and domain enums;
      - integers, durations, and timestamps;
      - `Pseudonym`.

      Plain `String`/`&str` are not `LogSafe`, so free text can't compile
      into a log line.
    - Every `eprintln!` in `src-tauri/src` (outside `bin/`) moves to it,
      and a test forbids new ones.
    - `log_root` joins `StoragePaths`' collision checks.
16. **Diagnostics bundle and redaction threat model.**
    - The reader is a third party (a support thread or a public issue).
      These are protected:
      - credential values and refs;
      - hosts, IPs, and ports;
      - every URL;
      - absolute paths and usernames;
      - user-authored names and notes;
      - host-supplied strings (job and file names).
    - The bundle is `farm3d-diagnostics-<UTC>.zip`, with selectable
      sections:

      | Section | Contents |
      |---|---|
      | `about` | versions, platform, schema, catalog, OrcaSlicer version (no path), credential-store tier |
      | `health` | per Printer: a pseudonym (`printer-3`), adapter kind, connection state, error code, freshness, capability states, host software version |
      | `storage` | usage by class, the `integrity::check` summary, `migration_warnings` codes, pending cleanup counts |
      | `configuration` | the settings values (none secret), row counts per table, camera source *kind* only |
      | `logs` | the rotated log files, safe by construction |
      | `recentProblems` | the last 50 Attention Events: condition, severity, times, pseudonymous source |

    - **Egress scan.** Before anything is written, the serialized bundle
      is scanned for:
      - every credential value behind every ref in the database (read,
        compared, and dropped, as `Zeroizing`);
      - every Connection and endpoint host;
      - every camera URL and its query values;
      - the home directory path;
      - every Printer, Spool, Model, and Project name longer than 3
        characters.

      A hit aborts with `DIAGNOSTICS_REDACTION_FAILED` naming the
      section, and nothing is written.
17. **Reset mechanics and partial-reset recovery.**
    - **Tier (a):** one transaction resets the `settings` row to its
      defaults, after an F1 pre-import-style snapshot
      (`SnapshotKind::Settings`). It reuses the existing path. The
      Slicer runtime and per-Printer alert defaults are *not* touched.
    - **Tier (b):** prunes through the retention path with reason
      `reset`. Rows and Incident timeline entries stay (P8 rule).
      Choosing `all` includes pinned media.
    - **Tier (c):** writes a `reset` journal and restarts. The installer
      then:
      1. moves the database set, `content_root`, `media_root`, and
         `log_root` into `restore/<id>/previous/`;
      2. opens a fresh, migrated database;
      3. deletes each journaled credential ref from the store, and
         queues any failure in the new database's
         `pending_credential_cleanup` (reason `reset`);
      4. deletes `previous/`.

      Tier (c) **only rolls forward**. Every step is idempotent, and a
      crash resumes at the recorded step.
    - A tier (c) safety backup is optional, and on by default.
    - Migration 0010 widens `camera_snapshots.prune_reason` with
      `reset` and `notInBackup`, and widens
      `pending_credential_cleanup.reason` with `reset`.
18. **History query.**
    - `list_job_history` takes these filters:
      - `states`: terminal states by default, with `outcomeUnknown`
        allowed;
      - `printerLifecycle`: `any`, `active`, or `archived`;
      - `printerId`, `spoolId`, `modelId`;
      - `endedAfter` / `endedBefore`;
      - `text`: up to 200 characters, matched with `LIKE … ESCAPE`
        over the Printer snapshot name, Model name, Slice Revision
        label, Spool number, and Job id prefix.

      It pages by the keyset `(ended_at DESC, id DESC)` with a limit of
      1–200 (default 50), backed by a new partial index.
    - `get_job_timeline` returns one ordered, typed union built from:
      - `job_events` (with typed detail);
      - Host Operations;
      - reservations;
      - `spool_amount_events` for the Job's reservation and correction;
      - Reconciliation Requirements;
      - the Job's Incident and its `incident_events`;
      - its Attention Events;
      - its camera snapshots (with their pruned state).

      It also carries the Printer snapshot and the Slice Revision's
      target and runtime.
    - FTS5 is not used: the volume doesn't justify it.
19. **Settings workspace.**
    - The rail gear becomes a normal activity-bar destination, and the
      existing `NavigationDestination::Settings` stops showing "not
      available".
    - A new selection kind `settingsCategory` gives deep links such as
      `#nav=v1/settings/settingsCategory/storage`.
    - The categories (umbrella order) are: General, Appearance, Slicing,
      Notifications and retention, Storage and backup, Connections,
      Diagnostics, About.
    - They are laid out as vertical Kobalte `Tabs`. The design system's
      `Tabs` gains `orientation`.
    - **Appearance** keeps preview semantics: selecting a theme
      previews it, and Apply commits it. Leaving the category or the
      workspace, or pressing Revert, calls `cancelPreview()`.
    - `SettingsMenu.tsx` and `ThemePopover.tsx` are deleted, and their
      tests are ported.
    - The Slicer and Notifications dialog bodies are extracted into
      forms that the workspace hosts. The Slicer dialog stays for its
      Preparation panel openers.
    - Settings and Printers import/export move to General and
      Connections. Settings import now re-applies the imported theme,
      which fixes a gap on `main`.
    - About replaces `AppShell`'s hard-coded `VERSION`.
20. **The P3 movement cascade stays.**
    - Deleting a Printer cascades `spool_movements` rows through its
      Material Slots (P3, intentional, and tested in `p3_lifecycle.rs`).
    - Printer delete is already blocked by any Job, Incident, or pinned
      evidence, so only movement history with no Job attached is lost.
    - The final matrix records this as a named, allowed loss, not a
      dangling reference.
    - *If changed:* Printer delete also requires no `spool_movements`
      that reference its slots (`MOVEMENT_HISTORY_EXISTS`). That is a P3
      behavior change and needs its own guard task.
21. **Verification is Linux x86_64 only** (F0's only supported
    platform). Windows and macOS are recorded as unverified.

## Evidence: the code on `main` at `727d9e2`

Gathered for this plan. Paths are under `src-tauri/` unless marked.

**Storage layout** (`src/lib.rs:504-620`, `persistence/database.rs:29-67`):

| Root | Path | Holds |
|---|---|---|
| `metadata_root` | `app_config_dir()` | `farm3d.sqlite3` (+ `-wal`/`-shm`), `farm3d.lock`, `legacy/`, `snapshots/` (pre-import snapshots and `.restore-staging/`), and `credentials.json` (the plaintext fallback store, 0600). **A naive copy of this directory includes secrets.** |
| `content_root` | `app_data_dir()/farm3d-content/v1` | `blobs/sha256/<hh>/<hex>`, `staging/`, and the transient `slicing-work/` |
| `media_root` | `app_data_dir()/farm3d-media/v1` | `snapshots/<yyyy>/<mm>/<snp-id>.<ext>` and `tmp/` |
| cache | `app_cache_dir()` | `orca-profiles/<sha256>/` |

**Persistence:**

- The schema is at version 9 (`persistence/migrations.rs:6`).
- Migrations are forward-only, with `user_version` and a checksummed
  `schema_migrations` ledger. A newer database is refused.
- There is one writer mutex with `BEGIN IMMEDIATE`; reads use fresh
  read-only connections.
- `persistence/snapshot.rs`:
  - creates validated online-backup-API copies for `SnapshotKind
    {Settings, Printers}` and keeps 5;
  - `stage_restore` (`:122-155`) exists but only tests call it;
  - `Storage::open` calls `cleanup_restore_staging`.

**Tables** (all STRICT, 30+):

- Content is referenced by SHA-256 only: `content_blobs`, plus the FKs
  from model source revisions, thumbnails, slice revisions, slice
  revision blobs, and `slice_operations.log_sha256`.
- Media is referenced by `camera_snapshots.rel_path`.
- Absolute paths are stored in `library_models.linked_path`,
  `model_source_revisions.source_path`, and `slicer_runtime_config`.
- Secret-adjacent columns:
  - `printers.connection_json` (host, port, `credentialRef`);
  - `host_operations.endpoint_json`;
  - `printer_cameras.snapshot_url` (plain HTTP, and may carry a query
    token);
  - `pending_credential_cleanup.credential_ref`.
- Natural-key unique indexes that decide `uniqueClash`:
  - `printers_active_host_identity`;
  - `spools.spool_number`;
  - `library_projects lower(name)`;
  - `spool_tares lower(name)`.

**Credentials** (`connections/credentials.rs`): the keyring service is
`farm3d` with the ref as the account, and the fallback file is
`credentials.json`. Refs have two forms, `farm3d/credential/<uuidv4>` and
the legacy `farm3d/printer/<id>/apikey`. There is no general secret
newtype. Secrets are kept out of serializable structs, carried in
`Zeroizing` buffers, and wrapped by hand-written redacting `Debug`
impls.

**Logging:** there is no log crate and no file. Twenty-odd `eprintln!`
sites exist (supervisor, content, links, slicing, cameras, jobs,
host_ops, queue, attention, notifications). Some print `{error:?}`.
The Moonraker WebSocket errors keep `e.to_string()`, which can hold a
URL (`moonraker/mod.rs:91,133,141`). Those strings stay internal today;
P9's `LogSafe` rule keeps them out of the log.

**Secret-corpus tests:** each phase has its own corpus and scanner, and
there is no shared helper. Examples:

- `p8_tracer.rs:132-142` (`CORPUS`);
- `p8_notifications.rs:49` (`assert_no_corpus`);
- `p8_cameras.rs:226` (`assert_clean`);
- `p2_batch.rs:27` (`assert_no_secret`, plus a child-process
  stdout/stderr scan);
- `f1_residual_acceptance.rs:328-400` (a whole-tree file walk).

**History:** there is no Job list command.

- `list_queue` returns open entries plus the newest 100 closed ones
  (`queue/commands.rs:47,165`).
- `get_job_history(jobId)` returns one Job's events, reservations, host
  ops, requirements, and lineage (`jobs/repository.rs:773`), with no
  Incident or evidence.
- The Queue History tab has no search, state, Printer, or date filter,
  and no paging.
- `JobPanel` renders only event titles and times.
- `list_incidents` has a cursor, but no screen uses it.

**Delete guards:**

- Printer delete is blocked by `NOT_ARCHIVED`, `SPOOLS_LOADED`,
  `HOST_OPERATION_UNRESOLVED`, `JOB_HISTORY_EXISTS`,
  `QUEUE_ENTRY_PINNED`, `INCIDENT_HISTORY_EXISTS`, and
  `PINNED_EVIDENCE_EXISTS` (`printers/lifecycle.rs:55-144`).
- Model delete is blocked by `SLICE_REVISIONS_EXIST`.
- Revision delete is blocked by `QUEUE_REFERENCES_REVISION` and
  `HOST_OPERATION_UNRESOLVED`.
- Spools, Jobs, Queue Entries, Attention Events, and Incidents have no
  delete.
- The only cross-domain check is
  `p7_guards.rs:476 no_row_is_orphaned_after_every_allowed_lifecycle_action`
  (`foreign_key_check`).
- References that SQLite doesn't enforce:
  - `jobs.correction_event_id`, `spool_amount_events.reservation_id`,
    and `spool_reservations.holder_id` (no FK);
  - `attention_events.source_id` (polymorphic);
  - the `operation_id` columns → `operations`;
  - `slice_revisions.target_json`'s `printerId` (label fallback by
    design);
  - blob files and media files.

**Settings:**

- The `settings` singleton has `revision`, `theme_mode`,
  `monitor_section`, `monitor_density`, six `notify_*` columns,
  `snapshot_retention_days`, and `snapshot_disk_cap_mb`. The Slicer
  runtime is its own singleton (`slicer_runtime_config`).
- Commands: `load_settings`, `save_settings`, `export_settings` (v3),
  and `import_settings` (v1–v3). No settings event exists.
- Frontend:
  - `SettingsMenu.tsx` is a gear `DropdownMenu` that opens
    `ThemePopover` (preview, Apply, and revert on any close), the Slicer
    dialog, the lazy `NotificationSettingsDialog`, and export/import.
  - `NavigationDestination::Settings` already exists, but `App.tsx:120`
    leaves it out of `availableDestinations`.
  - `DeletePrinterDialog` is the only typed-confirmation dialog.
- The main chunk is ~583 kB against vite's 600 kB warning (bfab050), so
  the Settings workspace must be lazy.

**Contracts:** there are 129 commands. The count expression is
`58 + 21 + 2 + 8 + 11 + 4 + 1 + 2 + 7 + 6 + 5 + 4`
(`tests/f1_contract_path.rs:6`).

## Design reference

The spec (Task 1) turns this into final names and payloads.

### Commands (17; the spec fixes the final list)

| Domain | Command | Kind |
|---|---|---|
| History | `list_job_history(query)` | read |
| History | `get_job_timeline(jobId)` | read |
| Backup | `backup_inventory()`: size and count by class for each media choice | read |
| Backup | `create_backup(media, operationId)`: save dialog, writes the archive | write (file) |
| Backup | `list_backups()`: safety backups and the last restore outcome | read |
| Backup | `delete_backup(backupId, operationId)` | write |
| Restore | `preview_restore(source)`: a file dialog, or a `safetyBackupId`; stages, verifies, migrates the staged copy, returns a `RestorePreview` | write (staging) |
| Restore | `discard_restore_preview(stagingId)` | write |
| Restore | `apply_restore(stagingId, confirmation, operationId)`: re-checks blockers, writes the safety backup and journal, restarts | write |
| Restore | `restore_status()` / acknowledged via `acknowledge_restore_status()` | read / write |
| Storage | `storage_usage()`: bytes by class | read |
| Storage | `clear_storage(target, operationId)`: `orcaCache`, `rotatedLogs`, `preImportSnapshots`, or `unreferencedContent` | write |
| Diagnostics | `diagnostics_preview()`: sections with estimated sizes | read |
| Diagnostics | `export_diagnostics(sections)`: save dialog, collects, redacts, egress scan, atomic write | write (file) |
| Reset | `reset_preview(tier)`: affected data classes with counts and bytes | read |
| Reset | `reset_farm(tier, options, confirmation, operationId)` | write |
| About | `about_farm3d()` | read |

- Every dialog-owning command follows F1's "Dialog ownership": the
  frontend never passes a path; results never return a full path; and
  closing the dialog returns `cancelled`.
- Under `just web`, every command returns `unsupported {desktopRequired}`
  or a web fixture.
- `confirmation` is the exact typed phrase from the spec (for example
  `restore`, `reset settings`, `reset media`, `reset farm`), checked in
  Rust. The frontend check is only a convenience.

### New error codes

- `BACKUP_INVALID` (with a safe field path, never content)
- `UNSUPPORTED_BACKUP_FORMAT`
- `UNSUPPORTED_SCHEMA_VERSION` (existing)
- `RESTORE_BLOCKED` (`details.blockers[]`, `recovery: [OPEN_QUEUE]`)
- `RESTORE_STAGING_EXPIRED`
- `INSUFFICIENT_SPACE`
- `BACKUP_IN_PROGRESS`
- `DIAGNOSTICS_REDACTION_FAILED`
- `CONFIRMATION_MISMATCH`

### Migration 0010 (`0010_p9_portability.sql`)

- `CREATE INDEX jobs_history ON jobs(ended_at DESC, id DESC) WHERE ended_at IS NOT NULL;`
  plus the indexes the spec's query plans need (check with `EXPLAIN
  QUERY PLAN` in Task 4).
- Rebuild `camera_snapshots` to widen the `prune_reason` CHECK with
  `reset` and `notInBackup`, following the `operations` rebuild pattern
  (`0006`–`0009`). Keep every index, trigger, and FK.
- Rebuild `pending_credential_cleanup` to widen `reason` with `reset`.
- Widen the `operations.kind` CHECK for the new mutating commands.
- No new product tables. Backups and journals live on disk, because
  they must survive a database swap.

### Restore journal (`<metadata_root>/restore/journal.json`)

```text
{ journalVersion: 1, id, kind: "restore"|"reset",
  phase: "pending"|"installing"|"installed"|"done"|"failed",
  step?: <installer step name>, attempts, stagingId?, safetyBackupId?,
  expectedCounts?, orphanCredentialRefs[], resetTier?, outcome?, failure? }
```

- Written with the atomic-replace rule (temp file, fsync, rename, fsync
  the directory).
- Installer steps are named so that failure injection can target each
  one (Task 7).

### Integrity catalogue (`persistence/integrity.rs`)

`check(conn, roots) -> IntegrityReport` runs these rules:

| Rule | What it checks | Outcome |
|---|---|---|
| `fk` | `PRAGMA foreign_key_check` | violation |
| `jobCorrectionEvent` | `jobs.correction_event_id` → `spool_amount_events` | violation |
| `amountEventReservation` | `spool_amount_events.reservation_id` → `spool_reservations` | violation |
| `reservationHolder` | `spool_reservations(holder_kind, holder_id)` → Jobs | violation |
| `attentionSource` | `attention_events(source_kind, source_id)` → its table. A Printer source may be gone only when the Event is resolved `sourceRemoved` | violation |
| `operationLedger` | every `*.operation_id` → `operations` | violation |
| `blobFile` | every `content_blobs` row has its file | violation |
| `mediaFile` | every unpruned `camera_snapshots` row has its file | violation |
| `sliceTargetPrinter` | `slice_revisions.target_json` `printerId` | **tolerated** (label fallback) |
| `orphanBlobFile` / `orphanMediaFile` | files with no row | **tolerated** (the startup sweep's job) |

The report counts rows per table, so restore can compare them with the
manifest. The spec may add rules found by the Task 11 audit
(`slice_preparations.document_json` is suspected to carry a Printer id).

## Global Constraints

These rules bind every task. The controller gives this section to every
implementer.

1. **No writes to real printers.** P9 needs no printer I/O. Tests that
   boot supervisors use `FakeMoonraker` or the loopback simulators.
2. **No owner network details in the repository.** Never commit an IP,
   hostname, serial, MAC, token, local path, camera URL, real backup, or
   real diagnostics bundle. Fixtures use RFC 5737 hosts (`192.0.2.x`)
   and synthetic content. Run `just check-hosts` before every commit.
3. **Credentials never enter a backup, a diagnostics bundle, a log
   line, a journal, an error, an event, or frontend state.**
   - Backup and restore never read a credential *value*, except that
     the preview checks whether a ref exists and drops the value
     (`Zeroizing`) at once.
   - Diagnostics also exclude hosts, URLs, paths, and user-authored
     names (decision 16).
   - Every new file format or payload gets a seeded-corpus test through
     the shared helper from Task 2.
4. **Online backup API only.** Never copy a live `farm3d.sqlite3`,
   `-wal`, or `-shm` (ADR-0008). The only exception is the installer,
   which *moves* the three as a set while no connection is open.
5. **Rust is persisted truth.** TypeScript never decides a conflict, a
   blocker, a redaction, a reset scope, or what is safe to delete. It
   presents what Rust returns.
6. **No new product-domain behavior** (issue #19). History reads
   existing tables. Restore reuses P7's restart recovery for restored
   active Jobs. Reset tier (b) reuses P8 retention.
7. **Frontend conventions** (AGENTS.md, DESIGN.md):
   - use `--f3d-*` tokens only, with no hard-coded colors, font sizes,
     or radii;
   - put CSS Modules next to their components;
   - use Kobalte for anything interactive, and read
     `node_modules/@kobalte/core/src/<name>/` for props;
   - keep the dense editor aesthetic;
   - in Select and DropdownMenu tests, use `fireEvent.pointerDown` and
     `pointerUp`;
   - add new design-system components to `components/index.ts` and
     `Showcase.tsx`;
   - lazy-load the Settings workspace and its heavy categories (the
     main chunk is ~583 kB of 600).
8. **Test-first.** Write the failing test, watch it fail, make it pass,
   refactor, and commit, with one conventional commit per task step
   group.
9. **Gates** before a task is done. Run the ones the task touches; the
   controller runs all of them at review.

   ```sh
   source "$HOME/.cargo/env"   # cargo is not on PATH in non-interactive shells
   just build
   just test
   just test-rust
   just check-hosts
   ```

   If a Rust type exported to TypeScript changed, also run
   `just gen-contracts && git diff --exit-code src/generated`. Never
   hand-edit `src/generated/**`.
10. **Contract registration.** A new command goes in:
    - `lib.rs` `COMMAND_NAMES` and `generate_handler!`;
    - `contracts/inventory.rs` (the array length, the decl string, and
      the visitor);
    - `tests/export_contracts.rs`;
    - `tests/f1_contract_path.rs` and `tests/p3_contract_path.rs`, and
      `tests/f1_residual_acceptance.rs` (its count);
    - `src/ipc/client.ts` `CommandMap`.

    Count assertions append P9's term to `main`'s expression
    (`58 + 21 + 2 + 8 + 11 + 4 + 1 + 2 + 7 + 6 + 5 + 4 + <P9>`). A task
    that registers only some commands adds its own term.
11. **Idempotency.** Every mutating command takes a client
    `operationId`, claimed through `spools::operations::claim`. File
    writes (`create_backup`, `export_diagnostics`) are idempotent per
    `operationId` within the process only. A replay after a restart
    writes a fresh file, and the spec says so.
12. **Simulators never run in CI.** P9's CI tests use in-process fakes.
    The tracer's simulator leg is `#[ignore]` behind `require_sim!`.
13. **Scope.** P9 leaves these out:
    - merge restore and per-domain backups (decisions 1 and 2);
    - Job history pruning (decision 3);
    - signing or encrypting backups;
    - cloud or scheduled backups;
    - moving storage roots to another disk;
    - log upload;
    - any new printer capability.

## Test tiers

| Tier | What | How it runs | Writes |
|---|---|---|---|
| Pure unit | the manifest codec and validation, zip path rules, the compatibility window, conflict classification, the redactor/pseudonymizer, `LogSafe`, journal transitions, the history query builder | `just test-rust`, CI | none |
| Repository | migration 0010, `integrity::check` over seeded violations, history queries and index use, reset tiers (a) and (b) | `just test-rust` over `test_storage()` | SQLite, temp dir |
| Installer | install, rollback, and roll-forward with an injected crash at every named step; sidecar handling; free-space refusal | `just test-rust` over temp roots | temp roots |
| Fixture compatibility | committed `formatVersion` 1 backups restore; tampered, truncated, traversal, bomb, newer-format, and newer-schema archives are refused | `just test-rust`, CI | temp roots |
| Cross-domain matrix | every archive/delete/prune/reset/restore action over an every-domain Farm, then `integrity::check` | `just test-rust`, CI | temp roots |
| Tracer | issue #19's tracer on the fakes (CI), plus a simulator leg with a live Moonraker Printer | CI plus `just test-sim` locally | temp roots, loopback |
| Frontend | stores, history view, timeline, Settings workspace and categories, restore preview, diagnostics selection, typed confirmation | `just test`, CI | none |
| Installed bundle | the deb: backup, restore with restart, the log file location, diagnostics export through the real dialogs | Task 19, at a desktop | local only |

## File map

**Backend (new):**

- `src-tauri/migrations/0010_p9_portability.sql`
- `src-tauri/src/persistence/integrity.rs`
- `src-tauri/src/history/`: `mod.rs` (wire types `JobHistoryQuery`,
  `JobHistoryRow`, `JobHistoryPage`, `JobTimeline`, `JobTimelineItem`),
  `repository.rs`, `commands.rs`
- `src-tauri/src/backup/`:
  - `mod.rs`: wire types (`BackupMediaChoice`, `BackupInventory`,
    `BackupSummary`, `RestorePreview`, `RestoreConflict`,
    `RestoreBlocker`, `RestoreStatus`)
  - `manifest.rs`: the codec, validation, and compatibility window
  - `archive.rs`: the zip reader/writer, path rules, limits, streaming
    SHA-256
  - `inventory.rs`: what goes in: blobs listed by the snapshot, media by
    choice, sanitization
  - `lease.rs`: `BackupLease` (pauses blob cleanup and the
    `MediaJanitor`)
  - `writer.rs`
  - `staging.rs`: extends F1's `stage_restore`; migrates the staged copy
  - `preview.rs`: counts, conflicts, blockers, notices
  - `journal.rs`
  - `installer.rs`: the startup install, rollback, reset roll-forward,
    and fault points
  - `safety.rs`: safety-backup location and retention
  - `commands.rs`
- `src-tauri/src/diagnostics/`: `log.rs` (`f3d_log!`, `LogSafe`,
  rotation), `pseudonym.rs`, `collect.rs`, `egress.rs`, `bundle.rs`,
  `storage.rs` (usage and cleanup), `reset.rs`, `about.rs`,
  `commands.rs`

**Backend (modified):**

- `persistence/{migrations.rs,mod.rs,database.rs,snapshot.rs}`: version
  10, `log_root` in `StoragePaths`, generalized snapshot creation, and
  the installer hook before `Storage::open`
- `lib.rs`: modules, startup order (installer → `Storage::open`), the
  log init, commands, and the restart path
- `library/content.rs` and `cameras/retention.rs`: honor `BackupLease`;
  `prune_reason` `reset` and `notInBackup`
- every `eprintln!` site (decision 15)
- `contracts/{command,inventory,navigation}.rs`: error codes, contracts,
  and `NavigationSelectionKind::SettingsCategory`
- `settings/commands.rs`: the tier (a) reset path

**Tests (new):**

- `tests/common/secrets.rs`: the shared seeded corpus and
  `assert_no_corpus(bytes, label)`, which also handles zip entries.
  Earlier phase tests are not migrated.
- `tests/p9_farm/mod.rs`: `Farm::with_every_domain()` builds one entity
  per domain, with every link, on the fakes.
- `tests/p9_migration.rs`, `tests/p9_integrity.rs`,
  `tests/p9_history.rs`, `tests/p9_log.rs`
- `tests/p9_backup.rs`, `tests/p9_restore_preview.rs`,
  `tests/p9_installer.rs`, `tests/p9_reset.rs`
- `tests/p9_diagnostics.rs`, `tests/p9_storage.rs`
- `tests/p9_fixture_compat.rs` and
  `tests/fixtures/backup/v1/*.farm3d-backup` (generated; small)
- `tests/p9_reference_matrix.rs`
- `tests/p9_tracer.rs`

**Frontend (new):**

- `src/history/`: `history-store.ts` (a debounced paged query that drops
  stale responses by request sequence), `presentation.ts`,
  `web-fixtures.ts`, tests
- `src/backup/`: `backup-store.ts` (inventory, create, list, preview,
  apply, status), `presentation.ts` (conflict grouping, notices),
  `web-fixtures.ts`, tests
- `src/diagnostics/`: `diagnostics-store.ts` (preview, export, storage
  usage, cleanup, reset preview, reset, about), `web-fixtures.ts`, tests
- `src/design-system/components/TypedConfirmDialog.tsx` (+ CSS, test),
  generalized from `DeletePrinterDialog`
- `src/screens/`, each with a `.module.css` and a `.test.tsx`:
  - `SettingsWorkspace.tsx`
  - the categories under `settings/`: `GeneralSettings.tsx`,
    `AppearanceSettings.tsx`, `SlicingSettings.tsx`,
    `NotificationSettings.tsx`, `StorageSettings.tsx`,
    `ConnectionsSettings.tsx`, `DiagnosticsSettings.tsx`,
    `AboutSettings.tsx`
  - `RestorePreviewPanel.tsx`
  - `ResetPanel.tsx`
  - `JobHistoryView.tsx`
  - `JobTimelinePanel.tsx`

**Frontend (modified):**

- `design-system/components/Tabs.tsx` (`orientation`), `index.ts`,
  `Showcase.tsx`
- `App.tsx` (the settings destination, lazy screen, available
  destinations), `ActivityBar.tsx` (the Settings button),
  `AppShell.tsx` (version from About)
- `navigation/navigation-store.ts` (the `settingsCategory` kind)
- `SlicerSettingsDialog.tsx` and `NotificationSettingsDialog.tsx`
  (extract their forms; the notifications dialog is retired)
- `settings/settings-store.ts` (import re-applies the theme)
- `QueueScreen.tsx`, `JobPanel.tsx`, `QueueEntryDetail.tsx` (history
  view and timeline)
- `DeletePrinterDialog.tsx` (uses `TypedConfirmDialog`)
- `src/ipc/client.ts`
- **Delete** `SettingsMenu.tsx` and `ThemePopover.tsx`, with their CSS
  and tests, once the ported tests pass.

**Docs:**

- the spec
- `docs/adr/0016-whole-farm-backup-restored-at-startup.md`
- `docs/adr/0017-safe-by-construction-diagnostics-log.md`
- `CONTEXT.md`
- `docs/verification/2026-09-xx-p9-portability.md`
- screenshots `docs/screenshots/p9-*`
- the v1 approach's known-unknowns rows "Backup format/conflict policy"
  and "Diagnostics redaction"
- `README.md` (where data, backups, and logs live)

## Tasks

### Task 1: Focused spec, ADR-0016, ADR-0017, and CONTEXT.md

**Owner:** Wiring, with Backend. **Depends on:** the owner confirming
§Planner defaults.

**Files:**

- Create the spec in the P8 spec's shape: status, goal, scope and
  non-goals, vocabulary, decisions D1–Dn, the backend model (migration
  0010 exactly, the manifest and journal schemas, wire types, commands,
  error codes), the frontend, accessibility, acceptance criteria, and
  delivery. It must resolve every item of the umbrella decision gate by
  name:
  - archive format;
  - checksums;
  - compatibility window;
  - linked-path portability;
  - conflict identity;
  - merge/replace policy;
  - safety-backup cleanup;
  - log rotation;
  - redaction threat model;
  - partial-reset recovery.
- Create ADR-0016: whole-Farm archives, replace-only restore, and
  install at the next startup through a journal. Record the rejected
  alternatives: live swap, merge, and file copy.
- Create ADR-0017: the diagnostics log accepts only typed `LogSafe`
  fields, and bundles are pseudonymized, then egress-scanned.
- Modify `CONTEXT.md`: add **Backup**, **Safety backup**, **Restore
  preview**, **Diagnostics bundle**, and **Reset**, each with an
  _Avoid_ note (for example, avoid "snapshot" for a backup, because
  **Snapshot** is a camera frame).

- [ ] **Step 1:** Draft the spec from §Design reference and decisions
  1–21.
- [ ] **Step 2:** Write the conflict classification (decision 1) as a
  fixture table (natural key × local/backup presence × content equal),
  and the installer steps as a fault-point table. Tasks 6 and 7 copy
  them verbatim.
- [ ] **Step 3:** Run a standards review against AGENTS.md, DESIGN.md,
  CONTEXT.md, and ADRs 0004, 0008, 0013, and 0014. Then run a spec
  review against the umbrella P9 section. **Controller approves.**
- [ ] **Step 4:** Update this plan wherever a name or signature
  changed. Commit:
  `docs(p9): add the focused spec, ADR-0016, ADR-0017, and vocabulary`.

**Acceptance criteria:** every decision-gate item has a named decision.
The conflict and fault-point tables are complete. No task below refers
to a name the spec renamed.

### Task 2: Migration 0010, the integrity checker, and the shared secret corpus

**Owner:** Backend. **Depends on:** Task 1.

**Files:**

- Create `0010_p9_portability.sql`, `persistence/integrity.rs`, and
  `tests/common/secrets.rs`
- Modify `persistence/migrations.rs` (version 10) and the exact-schema
  tests in `persistence/mod.rs`
- Test: `tests/p9_migration.rs`, `tests/p9_integrity.rs`

- [ ] **Step 1:** Write the failing migration tests:
  - v9 → v10 on a populated fixture keeps every row;
  - each rebuilt table keeps its indexes, triggers, and FKs (compare
    `sqlite_master` before and after, excluding the CHECK text);
  - the new CHECK values are accepted;
  - a v11 database is refused.
- [ ] **Step 2:** Implement the migration.
- [ ] **Step 3:** Write a failing test per integrity rule. Seed exactly
  one violation, and expect exactly that rule to fire. The tolerated
  rules report as tolerated. A clean every-domain Farm reports nothing.
  The counts match `SELECT count(*)`.
- [ ] **Step 4:** Implement `integrity::check`. Its reads run in one
  deferred read transaction, so they all see one WAL snapshot.
- [ ] **Step 5:** Write the shared corpus:
  - a credential value;
  - `http://operator:s3cr3t-P9@192.0.2.19:8080/webcam?token=tok-P9-19`
    and its parts;
  - a header value (`X-Api-Key: …`);
  - a host;
  - a home-directory path;
  - a Printer name.

  `assert_no_corpus` scans raw bytes and every zip entry, both
  compressed and decompressed.
- [ ] **Step 6:** Commit:
  `feat(persistence): add migration 0010 and the reference-integrity checker`.

**Acceptance criteria:** every catalogue rule has a positive test and a
negative test. The migration preserves data and schema objects.

### Task 3: The safe-by-construction log

**Owner:** Backend. **Depends on:** Task 1.

**Files:**

- Create `diagnostics/log.rs` and `diagnostics/pseudonym.rs`
- Modify `persistence/database.rs` (`log_root` in `StoragePaths` and
  its collision tests) and `lib.rs` (log init after path validation;
  stderr mirroring in debug builds)
- Modify every `eprintln!` site listed in the Evidence section
- Test: `tests/p9_log.rs`

- [ ] **Step 1:** Write the failing tests:
  - A `trybuild`-style compile-fail test, or a sealed-trait unit test,
    shows that a `String` can't be logged. If adding `trybuild` isn't
    worth it, assert the trait's implementors in a unit test instead
    and document the choice.
  - Rotation at 2 MiB keeps 5 files.
  - A write failure never panics and never blocks the caller: it drops
    the line and counts the drop.
  - Each line is one JSON object: `ts`, `level`, `code`, and the
    fields.
- [ ] **Step 2:** Implement the log. Use a bounded channel and a writer
  thread, so logging never holds a lock across I/O on a hot path.
- [ ] **Step 3:** Migrate every `eprintln!`. Add a test that walks
  `src-tauri/src` (excluding `bin/`) and fails on `eprintln!` or
  `println!`.
- [ ] **Step 4:** Add a corpus test: drive the P6/P8 error paths that
  used to print `{error:?}` (a Moonraker WebSocket failure against a
  userinfo URL, a camera fetch error, a content sweep failure), then
  `assert_no_corpus` over the log files.
- [ ] **Step 5:** Commit:
  `feat(diagnostics): add a rotated, typed-field log and retire eprintln`.

**Acceptance criteria:** free text can't reach the log by construction.
No `eprintln!` remains. The corpus scan passes.

### Task 4: History read model and commands

**Owner:** Backend. **Depends on:** Task 2.

**Files:**

- Create `history/{mod.rs,repository.rs,commands.rs}`
- Register 2 commands (§Global Constraints 10)
- Test: `tests/p9_history.rs` (the P7 rig plus P8 incidents and
  snapshots)

- [ ] **Step 1:** Write the failing query tests:
  - every filter alone and in combination;
  - `printerLifecycle: archived` finds Jobs on an archived Printer, by
    its snapshot name;
  - text search with `%` and `_` in the input (escaped) and non-ASCII
    input;
  - keyset paging is stable while new Jobs finish between pages;
  - the limit bounds;
  - `outcomeUnknown` is excluded by default.
- [ ] **Step 2:** Assert index use with `EXPLAIN QUERY PLAN` for the
  default query and for the text and Printer filters. Add indexes to
  0010 only if the plan needs them (amend Task 2's migration before
  merge).
- [ ] **Step 3:** Write the failing timeline tests:
  - The order is deterministic: by time, then source, then sequence.
  - Every item kind appears for a settled Job with a correction,
    requirement, Incident, and snapshot.
  - Pruned evidence shows as pruned.
  - The timeline is byte-identical before and after archiving the
    Printer, archiving the Spool, and pruning media (except the pruned
    flag). This is the immutability proof.
- [ ] **Step 4:** Implement both commands. Each uses one read
  transaction.
- [ ] **Step 5:** Commit:
  `feat(history): add searchable Job history and the full Job timeline`.

**Acceptance criteria:** a finished Job is findable by every documented
filter. Its timeline carries every linked record, and archive or prune
doesn't change it.

### Task 5: Backup inventory, lease, and writer

**Owner:** Backend. **Depends on:** Task 2.

**Files:**

- Create `backup/{mod.rs,manifest.rs,archive.rs,inventory.rs,lease.rs,writer.rs,safety.rs}`
- Modify:
  - `persistence/snapshot.rs`: a generalized `create_snapshot_to(path)`
    that returns the validated copy;
  - `library/content.rs` and `cameras/retention.rs`: honor
    `BackupLease`.
- Register `backup_inventory`, `create_backup`, `list_backups`, and
  `delete_backup`
- Test: `tests/p9_backup.rs`

- [ ] **Step 1:** Write the failing manifest and archive unit tests:
  - the codec round-trips;
  - each path rule rejects its case;
  - streaming SHA-256 matches;
  - a content entry whose hash doesn't match its name is rejected.
- [ ] **Step 2:** Write the failing writer tests over
  `Farm::with_every_domain()` (Task 11's builder; land its first cut
  here if Task 11 hasn't started):
  - For each media choice, the archive holds exactly the expected
    entries.
  - The database copy is sanitized (decision 11) and passes
    `integrity::check`.
  - The counts equal the source's.
  - `assert_no_corpus` (excluding the camera URL, per decision 14)
    passes over the archive and over the free pages of the database
    copy.
- [ ] **Step 3:** Write the failing consistency tests:
  - A blob deleted (by `delete_model`) *during* a backup is still in the
    archive, and its cleanup runs after the lease drops.
  - Media pruning waits for the lease.
  - A second concurrent backup gives `BACKUP_IN_PROGRESS`.
  - A missing media file is recorded as `missingFile`.
- [ ] **Step 4:** Implement the writer. Stream into a same-directory
  temp file, fsync it, then atomically replace (`document_io.rs`).
  Implement safety-backup retention (keep 3, delete only after the new
  one validates), and the free-space check.
- [ ] **Step 5:** Commit:
  `feat(backup): write versioned, checksummed whole-Farm backups`.

**Acceptance criteria:** backups are point-in-time, checksummed, and
secret-free, and they never race blob or media cleanup.

### Task 6: Restore staging, preview, and refusal

**Owner:** Backend. **Depends on:** Task 5.

**Files:**

- Create `backup/{staging.rs,preview.rs}`
- Register `preview_restore` and `discard_restore_preview`
- Test: `tests/p9_restore_preview.rs`

- [ ] **Step 1:** Write the failing staging tests:
  - The candidate is extracted to `snapshots/.restore-staging/<id>/`
    with checksums verified.
  - An older-schema candidate is migrated on the staged copy, and the
    live database is untouched (compare its checksum).
  - Every rejection case from decisions 6 and 7 fails.
  - Staging older than 24 h, or left from a previous run, is removed
    at startup (extends F1's cleanup), except the one the journal
    names.
- [ ] **Step 2:** Write the failing preview tests from the spec's
  conflict fixture table:
  - per-table counts, local versus backup;
  - `onlyLocal`, `changed`, and `uniqueClash`, grouped by domain and
    capped at 200 items per class, with totals;
  - notices: credentials to re-enter, linked paths missing on this
    machine, Jobs active at backup time, media left out, and the Slicer
    runtime kept local;
  - blockers from decision 10.
- [ ] **Step 3:** Implement. The preview reads the live Farm in one read
  transaction and the candidate through a read-only connection.
- [ ] **Step 4:** Commit:
  `feat(backup): stage, verify, and preview a restore with conflicts and blockers`.

**Acceptance criteria:** the preview is complete and read-only for the
live Farm. Every refusal is typed and carries a safe field path.

### Task 7: The restore journal and the startup installer

**Owner:** Backend; Wiring accountable. **Depends on:** Task 6.

**Files:**

- Create `backup/{journal.rs,installer.rs}`
- Modify:
  - `lib.rs`: run `installer::run(paths, lease)` after the ownership
    lease and before `Storage::open`; failures enter a
    `Failed(RESTORE_FAILED)` bootstrap state with a retry that re-runs
    the installer;
  - the restart path.
- Register `apply_restore`, `restore_status`, and
  `acknowledge_restore_status`
- Test: `tests/p9_installer.rs`

- [ ] **Step 1:** Write the failing journal tests: legal phase
  transitions only, atomic write, a corrupt journal gives
  `Failed(RESTORE_FAILED)` without touching data, and an unknown
  `journalVersion` is refused.
- [ ] **Step 2:** Write the failing fault-injection matrix from the
  spec's table. For each named step, crash (inject an error and drop
  everything), then run the installer again. The outcome is either:
  - installed: counts equal the manifest, `integrity::check` is clean,
    and credential orphans are queued; or
  - rolled back: the database, WAL set, content, and media equal the
    pre-restore Farm byte for byte, and the journal is `failed`.

  It is never mixed. Include these cases:
  - a live `-wal` with uncheckpointed frames at `pending` (the WAL must
    move with its database);
  - validation failure (a candidate corrupted after staging);
  - the disk filling during blob extraction;
  - two consecutive crashes.
- [ ] **Step 3:** Write the failing `apply_restore` tests:
  - the blocker re-check;
  - `CONFIRMATION_MISMATCH`;
  - an expired staging id;
  - the safety backup is written before the journal;
  - the restart is requested through an injected `Restarter`, so tests
    don't restart;
  - an `operationId` replay.
- [ ] **Step 4:** Implement the three commands.
- [ ] **Step 5:** Test that restored active Jobs go through P7 startup
  recovery exactly as on a normal restart. Reuse one
  `p7_restart_matrix` row with a restored database.
- [ ] **Step 6:** Commit:
  `feat(backup): install restores at startup with rollback and crash recovery`.

**Acceptance criteria:** there's no mixed-Farm outcome under any
injected crash. Rollback is byte-exact. The WAL is never separated from
its database.

### Task 8: Tiered reset

**Owner:** Backend. **Depends on:** Task 7.

**Files:**

- Create `diagnostics/reset.rs`
- Modify `settings/commands.rs`, `cameras/retention.rs` (reason
  `reset`), and `backup/installer.rs` (the `reset` journal kind)
- Register `reset_preview` and `reset_farm`
- Test: `tests/p9_reset.rs`

- [ ] **Step 1:** Write the failing tests for tier (a):
  - the defaults are applied;
  - a pre-reset snapshot exists;
  - the Slicer runtime and alert defaults are untouched;
  - a revision conflict gives `CONFLICT`.
- [ ] **Step 2:** Write the failing tests for tier (b):
  - unpinned media, or all media, is pruned with reason `reset`;
  - the rows and Incident timeline entries stay;
  - the files are gone;
  - `integrity::check` is clean.
- [ ] **Step 3:** Write the failing tests for tier (c):
  - the preview lists every data class with counts and bytes;
  - the optional safety backup is written first;
  - roll-forward after a crash at every step;
  - every journaled credential ref is deleted from a fake credential
    store, and failures are queued with reason `reset`;
  - safety backups are kept unless the option is ticked;
  - the fresh Farm opens at schema 10, empty, and clean.
- [ ] **Step 4:** Implement the reset.
- [ ] **Step 5:** Commit:
  `feat(diagnostics): add tiered, typed-confirmed resets with roll-forward recovery`.

**Acceptance criteria:** each tier touches exactly its named classes. A
crash in tier (c) always finishes the reset.

### Task 9: Diagnostics bundle and About

**Owner:** Backend. **Depends on:** Tasks 2 and 3.

**Files:**

- Create `diagnostics/{collect.rs,egress.rs,bundle.rs,about.rs,commands.rs}`
- Register `diagnostics_preview`, `export_diagnostics`, and
  `about_farm3d`
- Test: `tests/p9_diagnostics.rs`

- [ ] **Step 1:** Write the failing collector tests, one per section,
  over the every-domain Farm with the full corpus seeded into:
  - the credential store;
  - Connection hosts and endpoints;
  - the camera URL;
  - Printer, Model, Spool, and Project names;
  - Printer notes;
  - host telemetry job names;
  - linked paths;
  - log lines produced by driving error paths.

  Each section holds the expected facts and `assert_no_corpus` passes.
  Pseudonyms are stable within one bundle and differ between bundles.
- [ ] **Step 2:** Write the failing egress tests. A collector
  deliberately broken to leak (a test-only hook) is caught, gives
  `DIAGNOSTICS_REDACTION_FAILED` naming the section, and writes no file.
- [ ] **Step 3:** Implement. `about_farm3d` reads the version from
  `app.package_info()`, together with the schema version, catalog info,
  OrcaSlicer version (never its path), and the credential tier.
- [ ] **Step 4:** Commit:
  `feat(diagnostics): export a selectable, pseudonymized, egress-scanned bundle`.

**Acceptance criteria:** no corpus item reaches any section. A leak is
caught before the file is written.

### Task 10: Storage usage and cleanup

**Owner:** Backend. **Depends on:** Tasks 5 and 3.

**Files:**

- Create `diagnostics/storage.rs`
- Register `storage_usage` and `clear_storage`
- Test: `tests/p9_storage.rs`

- [ ] **Step 1:** Write the failing tests. `storage_usage` reports these
  classes:
  - database (with WAL);
  - content, split by role (model sources, thumbnails, G-code,
    artifacts, logs), using the P4 `library_content_info` basis;
  - camera media, pinned versus unpinned (P8's `media_usage` basis);
  - safety backups;
  - pre-import snapshots;
  - logs;
  - the Orca cache.
- [ ] **Step 2:** Write failing tests for each `clear_storage` target.
  - `unreferencedContent` runs the existing content sweep now.
  - `preImportSnapshots` keeps the newest 1.
  - `rotatedLogs` never removes the active file.
  - `orcaCache` is refused while a slice operation runs.
  - Each target leaves `integrity::check` clean.
- [ ] **Step 3:** Implement. Commit:
  `feat(diagnostics): report storage use and clear reclaimable data`.

**Acceptance criteria:** usage matches the bytes on disk (within the
tolerance the spec sets for filesystem block rounding), and no cleanup
touches referenced data.

### Task 11: The every-domain Farm and the final reference matrix

**Owner:** Wiring. **Depends on:** Tasks 2, 7, 8, and 10.

**Files:**

- Create `tests/p9_farm/mod.rs` and `tests/p9_reference_matrix.rs`
- Modify production code only for a defect the matrix finds. Each fix
  is its own commit with a regression test.

- [ ] **Step 1:** Build `Farm::with_every_domain()`. It holds:
  - an active and an archived Printer, each with a camera, alert
    defaults, and slots;
  - a loaded and an archived Spool, and a tare;
  - a Project;
  - a managed Model and a linked Model, each with revisions and
    thumbnails;
  - a preparation;
  - a farm3d Slice Revision and an external one;
  - Queue Entries: open, closed, and one with a successor;
  - Jobs: completed with a correction, failed, and cancelled;
  - Reconciliation Requirements;
  - an Incident with notes;
  - pinned and unpinned snapshots;
  - open and resolved Attention Events, including a recurrence;
  - terminal Host Operations.

  Use fixed ids and timestamps so Task 17 can reuse it.
- [ ] **Step 2:** Audit the references. Grep every `*_json` column and
  every `TEXT` column that ends `_id` for references the catalogue
  misses (for example `slice_preparations.document_json`). Add a rule,
  or a named tolerance, for each one in the spec and in `integrity.rs`.
- [ ] **Step 3:** Write the matrix as a table-driven test. It has one
  row per action:
  - Printer archive/unarchive/delete, both allowed and blocked;
  - Spool archive, mark empty, and reactivate;
  - tare delete;
  - Project delete;
  - Model delete, allowed and blocked;
  - Slice Revision delete, allowed and blocked;
  - Queue Entry remove;
  - media prune, pin, and unpin;
  - the retention sweep;
  - each `clear_storage` target;
  - reset tiers (a), (b), and (c);
  - restore of the Farm's own backup, and restore of a conflicting
    backup.

  After each row, check the expected blocker code (or success),
  `integrity::check` with no violations, and each named allowed loss
  (decision 20) as expected.
- [ ] **Step 4:** Commit: `test(p9): prove no archive, delete, restore, or reset leaves a dangling reference`.

**Acceptance criteria:** every archive/delete/reset/restore path in
v1 is a row, and every row passes.

### Task 12: Frontend stores and web fixtures

**Owner:** Frontend. **Depends on:** contracts from Tasks 4–10
(`just gen-contracts`).

**Files:**

- Create `src/history/`, `src/backup/`, and `src/diagnostics/` (see the
  File map), each with `web-fixtures.ts` and tests
- Modify `src/ipc/client.ts`

- [ ] **Step 1:** Write the failing store tests.
  - The history query is debounced (250 ms). An out-of-order response is
    dropped, and a filter change resets the cursor.
  - The backup store's restore flow state machine: idle → choosing →
    previewing → confirming → restarting, plus the failed and cancelled
    branches.
  - The `restore_status` banner appears once and is acknowledged.
  - Reset and export results are presented without their paths.
  - Web mode returns fixtures for reads and `needsDesktopError` for
    writes.
- [ ] **Step 2:** Implement. Commit:
  `feat(ui): add history, backup, and diagnostics stores`.

### Task 13: Design system: vertical Tabs and TypedConfirmDialog

**Owner:** Frontend. **Depends on:** Task 1.

**Files:**

- Modify `Tabs.tsx` (`orientation`, with arrow-key direction from
  Kobalte), `index.ts`, and `Showcase.tsx`
- Create `TypedConfirmDialog.tsx`. Its props are `title`,
  `consequences: string[]`, `phrase`, `confirmLabel`, `pending`,
  `onConfirm`, and `onOpenChange`. Matching is exact, with no trim and
  no case folding (the `DeletePrinterDialog` rule).
- Modify `DeletePrinterDialog.tsx` to use it, keeping its tests green.

- [ ] **Step 1:** Write failing tests:
  - vertical Tabs change with ArrowDown and ArrowUp, and expose
    `aria-orientation`;
  - the confirm button is disabled until the typed text matches;
  - the input resets on open;
  - the consequence list is read by the dialog's description.
- [ ] **Step 2:** Implement. Commit:
  `feat(design-system): add vertical Tabs and a typed-confirmation dialog`.

### Task 14: The Settings workspace shell and the simple categories

**Owner:** Frontend. **Depends on:** Tasks 12 and 13.

**Files:**

- Create `SettingsWorkspace.tsx` and the General, Appearance, Slicing,
  Notifications and retention, Connections, and About categories
- Modify:
  - `App.tsx`: the `settings` destination is available and lazy;
  - `ActivityBar.tsx`: a Settings button, with the gear icon, as a
    destination;
  - `navigation-store.ts`: `settingsCategory`;
  - `contracts/navigation.rs`: the new kind, then `gen-contracts`;
  - `AppShell.tsx`: the version from About;
  - `settings-store.ts`: import re-applies the theme.
- Extract `SlicerSettingsForm` and `NotificationSettingsForm` from their
  dialogs. The Slicer dialog wraps the form, and its Preparation openers
  and focus home are unchanged. Retire the Notifications dialog.
- Delete `SettingsMenu.tsx` and `ThemePopover.tsx` after porting their
  tests.

- [ ] **Step 1:** Port the theme tests to `AppearanceSettings`. Each of
  these must fail first:
  - selecting previews;
  - Apply commits;
  - Revert restores;
  - navigating to another category or screen with a pending preview
    restores the committed theme;
  - System follows the OS.
- [ ] **Step 2:** Write failing tests:
  - `#nav=v1/settings/settingsCategory/<slug>` selects the category, and
    an unknown slug falls back to General with the standard
    availability notice;
  - the rail button is marked current on Settings;
  - the Slicer dialog still opens from Preparation and returns focus;
  - Settings and Printers import/export work from their new homes.
- [ ] **Step 3:** Implement. Confirm the main chunk doesn't grow (record
  the `just build` chunk sizes in the commit body). Commit:
  `feat(settings-ui): replace the gear menu with the Settings workspace`.

**Acceptance criteria:** every function of the old gear menu is
reachable from the workspace, theme preview behaves exactly as before,
and the main chunk doesn't grow.

### Task 15: Storage and backup, restore preview, Diagnostics, and reset UI

**Owner:** Frontend. **Depends on:** Task 14.

**Files:**

- Create `StorageSettings.tsx`, `DiagnosticsSettings.tsx`,
  `RestorePreviewPanel.tsx`, and `ResetPanel.tsx`

- [ ] **Step 1:** Write failing tests:
  - **Storage:** the usage table and cleanup actions, with a pending
    state and the result.
  - **Backup:** the media choice shows its estimated size, and Create
    calls the store.
  - **Safety backups:** a list with restore and delete.
  - **Restore preview:**
    - counts side by side;
    - conflicts grouped by class and domain, with totals beyond the cap;
    - notices;
    - blockers disable Restore and link to the Queue;
    - Restore opens `TypedConfirmDialog` with the phrase `restore` and
      states that farm3d will restart.
  - **Diagnostics:** section checkboxes with descriptions and sizes, a
    statement of what is never included, and Export.
  - **Reset:** three tiers. Each lists its data classes from
    `reset_preview`, has its own phrase, and tier (c) has the
    safety-backup option (on) and the "also delete safety backups"
    option (off).
- [ ] **Step 2:** Implement. Commit:
  `feat(settings-ui): add storage, backup and restore, diagnostics, and reset`.

### Task 16: History UI

**Owner:** Frontend. **Depends on:** Task 12.

**Files:**

- Create `JobHistoryView.tsx` and `JobTimelinePanel.tsx`
- Modify `QueueScreen.tsx` (the History view uses `JobHistoryView`) and
  `JobPanel.tsx` / `QueueEntryDetail.tsx` (use `JobTimelinePanel`)

- [ ] **Step 1:** Write failing tests:
  - the search field;
  - the state, Printer (archived Printers included and labelled), and
    date filters;
  - a `DataTable` with a "Load more" row;
  - empty, filtered-empty, loading, and error states;
  - selecting a row goes to `queue/job/<id>`;
  - the timeline renders every item kind with a textual label, time,
    and detail;
  - the material section shows the reservation, deduction, and
    correction;
  - Incident and evidence links open `monitor/incident/<id>` and the
    snapshot viewer;
  - pruned evidence shows as text.
- [ ] **Step 2:** Implement. Commit:
  `feat(queue-ui): add searchable Job history and the full timeline`.

### Task 17: Versioned fixture backups and compatibility tests

**Owner:** Backend. **Depends on:** Tasks 7 and 11.

**Files:**

- Create a deterministic generator: a `gen-backup-fixture` bin, or a
  test behind a feature flag, following the `gen-library-fixtures`
  pattern. Add a matching `just gen-backup-fixtures` recipe
  (AGENTS.md), and use fixed zip timestamps.
- Create `tests/fixtures/backup/v1/farm-v1-schema10.farm3d-backup` and
  its tampered siblings, generated from it by the test
- Test: `tests/p9_fixture_compat.rs`

- [ ] **Step 1:** Write the failing tests:
  - the committed fixture restores (preview, apply, install) with counts
    and `integrity::check` clean;
  - each tampered variant is refused with its code: a flipped byte, a
    truncated archive, an extra entry, a traversal path, a bomb, format
    2, schema 99, and a checksum mismatch in `schema_migrations`.

  A README next to the fixtures states the rule: never regenerate an
  existing version's fixture; add a new one.
- [ ] **Step 2:** Generate the fixture (it must stay small, under 1 MiB)
  and implement any fix the tests find. Commit:
  `test(backup): add versioned fixture backups and compatibility tests`.

### Task 18: The tracer

**Owner:** Wiring. **Depends on:** Tasks 4–11 (and 14–16 for the
manual pass).

**Files:** `tests/p9_tracer.rs`; a `sim_moonraker.rs` P9 section
(ignored).

- [ ] **Step 1:** Write the tracer on the fakes (CI), following issue
  #19:
  1. Build the every-domain Farm and seed the corpus into credentials,
     URLs, headers, errors (driven failures), and logs.
  2. Back up with `media: all`, then run `assert_no_corpus` over the
     archive. The only exception is the camera URL, which is allowed
     (decision 14) and asserted present in the database copy only.
  3. Mutate the local Farm to conflict: rename a Printer, add a Spool
     with a clashing number, delete a Project, and finish a new Job.
  4. Preview, and assert every conflict class and the notices.
  5. Apply through the installer with an empty fake credential store
     (the "new machine" case). Counts equal the manifest,
     `integrity::check` is clean, and each Printer with a ref reports
     `CREDENTIAL_REQUIRED`.
  6. Export diagnostics with every section and run `assert_no_corpus`
     with no exceptions.
  7. Reset tier (c), and assert an empty, clean Farm with the safety
     backup kept.
- [ ] **Step 2:** Write the simulator leg (`#[ignore]`, `require_sim!`,
  `sim::exclusive()`, `reset()` first). It is the same flow, with the
  Printer on the Moonraker sim. After restore, it reconnects once the
  credential is re-entered. Commit the manifest copy under
  `docs/superpowers/baselines/`.
- [ ] **Step 3:** Commit:
  `test(p9): add the backup, restore, diagnostics, and reset tracer`.

### Task 19: Packaging and installed-bundle verification

**Owner:** Wiring. **Depends on:** Tasks 14–18.

- [ ] `just package`. Install the deb on Linux x86_64 and check:
  - the log file lands in `app_log_dir()` and rotates;
  - a backup is created through the real save dialog;
  - a restore through the real open dialog restarts the app and shows
    the outcome banner;
  - killing the app during install (`kill -9` while the journal says
    `installing`) recovers on the next launch;
  - diagnostics export writes a zip that a manual `grep` for the seeded
    test values doesn't match;
  - a tier (c) reset.
- [ ] Record the results in the verification record. Mark Windows and
  macOS unverified.

### Task 20: Documentation and the verification record

**Owner:** Wiring. **Depends on:** all.

- [ ] Write `docs/verification/2026-09-xx-p9-portability.md` (the P8
  record's shape): gates, the tracer, the matrix, fault injection,
  fixture compatibility, the installed bundle, and known follow-ups.
- [ ] Capture screenshots at 1440×900 and 1024×700: every Settings
  category, the restore preview with conflicts, typed confirmation,
  diagnostics selection, the history list and filters, and the Job
  timeline.
- [ ] Update `README.md` (where data, backups, and logs live; how to
  restore), the v1 approach's known-unknowns rows (resolved, with
  links), and `CONTEXT.md` if names changed.
- [ ] Commit: `docs(p9): record P9 verification`.

## Delivery order

```text
Task 1 ─┬─> Task 2 (0010+integrity) ─┬─> Task 4 (history) ─────────────────────────────┐
        │                            ├─> Task 5 (writer) ─> Task 6 (preview) ─> Task 7 (installer) ─> Task 8 (reset)
        │                            │                                         │
        ├─> Task 3 (log) ────────────┴─> Task 9 (diagnostics)                  │
        │                  Tasks 3+5 ─> Task 10 (storage)                      │
        │                  Tasks 2,7,8,10 ─> Task 11 (matrix) ─> Task 17 (fixtures)
        └─> Task 13 (DS) ─┐
Contracts 4–10 ─> Task 12 ┴─> Task 14 (workspace) ─> Task 15 (storage/diag UI)
                  Task 12 ─> Task 16 (history UI)
Tasks 4–11 (+14–16) ─> Task 18 (tracer) ─> Task 19 (installed) ─> Task 20 (docs)
```

| # | Task | Depends on | Parallel with |
|---|---|---|---|
| 1 | Spec, ADRs, CONTEXT | owner confirms the defaults | — |
| 2 | Migration, integrity, corpus | 1 | 3, 13 |
| 3 | Log | 1 | 2, 13 |
| 4 | History backend | 2 | 5, 9 |
| 5 | Backup writer | 2 | 4, 9 |
| 6 | Restore preview | 5 | 4, 9, 10 |
| 7 | Installer | 6 | 9, 10 |
| 8 | Reset | 7 | 9, 10 |
| 9 | Diagnostics | 2, 3 | 4–8 |
| 10 | Storage | 3, 5 | 6–9 |
| 11 | Matrix | 2, 7, 8, 10 | 12–16 |
| 12 | Frontend stores | contracts from 4–10 | 11 |
| 13 | Design system | 1 | 2–12 |
| 14 | Settings workspace | 12, 13 | 16 |
| 15 | Storage/diagnostics UI | 14 | 16 |
| 16 | History UI | 12 | 14, 15 |
| 17 | Fixture backups | 7, 11 | 14–16 |
| 18 | Tracer | 4–11 (+14–16) | — |
| 19 | Installed bundle | 14–18 | — |
| 20 | Docs and verification | all | — |

Tasks that run in parallel touch disjoint files, with two exceptions:
the contract registration files (`lib.rs`, `inventory.rs`,
`export_contracts.rs`, `f1_contract_path.rs`, `p3_contract_path.rs`,
`f1_residual_acceptance.rs`, and `client.ts`), and migration 0010, which
Task 4 may amend. The controller serializes merges of those. If two
implementers run at once, each uses its own worktree.

## Exit gate (issue #19)

- [ ] Versioned fixture backups and compatibility tests pass (Task 17:
  `p9_fixture_compat.rs`).
- [ ] Restore failure injection and interrupted-restore recovery pass
  (Task 7: `p9_installer.rs`; Task 8: the tier (c) roll-forward).
- [ ] Post-restore reference counts and integrity checks pass (Tasks 7,
  17, and 18).
- [ ] Diagnostics contain none of the seeded credential corpus (Task 9:
  `p9_diagnostics.rs`; Task 18).
- [ ] Settings/history UI tests pass (Tasks 13–16).
- [ ] The final archive/delete reference matrix proves that no deletion
  or reset leaves dangling references (Task 11:
  `p9_reference_matrix.rs`).
- [ ] The tracer completes with secrets excluded, on the fakes (CI) and
  the simulator, with the manifest committed (Task 18).
- [ ] `just package` and the installed-bundle restore/restart check
  pass on Linux x86_64 (Task 19).

## Residual risks

- **`AppHandle::restart` in the installed bundle.** Restart must
  relaunch the same binary with the same environment (AppImage, deb).
  Task 19 checks it. If restart is unreliable, `apply_restore` asks the
  operator to quit and reopen instead, and the journal makes that
  equally safe.
- **Uncapped backup size.** A Farm with many large Models can make a
  multi-gigabyte archive. The free-space check prevents a half-written
  file, but not a slow one. The writer reports progress through the
  existing operation-event pattern if the spec asks for it; otherwise
  it's a follow-up.
- **The egress scan uses exact matching.** It catches known values, not
  a transformed or partial leak (a URL-encoded token, for example). The
  typed log and the pseudonymized collectors are the primary control,
  and the scan is defense in depth. The corpus includes URL-encoded
  and case-changed variants of each value.
- **Linked-path portability is only surfaced.** Restoring onto another
  machine leaves linked Models "missing" until **Locate source**. That
  is intended, and the preview counts them.
- **Printer delete still drops unlinked Spool movement history**
  (decision 20). It is intentional and documented, not an integrity
  defect.
- **Printers import stays blocked once any Job exists** (decision 3).
  Whole-Farm restore replaces that workflow.
