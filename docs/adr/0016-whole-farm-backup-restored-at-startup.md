# Whole-Farm backups, replace-only restore, installed at startup

**Status:** Draft, pending controller review, 2026-09-29, with the P9
spec.

## Context

P9 must let an operator back up a Farm, move it to another machine or
roll it back, and trust the result (the umbrella spec's "Backup and
restore"; issue #19). The facts that shape the choice:

- **The Farm is one SQLite database plus files.** Thirty-odd STRICT
  tables with foreign keys, append-only timelines, and partial unique
  indexes hold every domain from P2 to P8. Content blobs (Model sources,
  thumbnails, G-code, slice artifacts, slicer logs) live in a
  content-addressed store, and camera frames in a media store. Both are
  referenced from the database by hash or relative path.
- **The database runs in WAL mode** with one writer and many readers.
  ADR-0008 forbids copying a live `farm3d.sqlite3` without its `-wal`,
  and leaves "sidecar-safe replacement, rollback, and full
  backup/restore" to P9.
- **While farm3d runs, many services hold the database**: Connection
  supervisors, the P7 dispatch driver and evaluator, the P8 projector,
  the media janitor, notifications, and link watchers.
- **Some state belongs to the machine, not the Farm**: credential values
  (in the keychain or `credentials.json`), the OrcaSlicer engine and
  preset paths, the telemetry cache, and pending cleanup rows that name
  this machine's credential refs and files.
- **Job history is immutable and never pruned** (owner decision 3), and
  Printers import is blocked once any Job exists (P7). Whole-Farm restore
  is how a Farm moves between machines.

The owner fixed the policy on 2026-09-29: a restore replaces the whole
Farm and never merges (decision 1), and a backup is the whole Farm plus
optional camera media (decision 2).

This is hard to reverse: the archive format becomes a compatibility
promise (every committed fixture must keep restoring), and the startup
order gains a step before `Storage::open`.

| Option | For | Against |
|---|---|---|
| A. Copy the data directories (database, WAL, content, media) as files | Trivial to write | A live copy can tear the database from its WAL (ADR-0008 forbids it); a naive copy of `metadata_root` includes `credentials.json`; no checksums, no manifest, no compatibility window |
| B. Merge a backup into the live Farm, row by row | The operator could keep local changes | Every domain needs a merge rule, and most have none that is safe: append-only timelines, Job and reservation state machines, partial unique indexes, and polymorphic references would each need conflict resolution, and a half-applied merge is a mixed Farm |
| C. Swap the database in the running process: stop every service, close `Storage`, replace the files, reopen | No restart | Every service (supervisors, projector, evaluator, janitor, link watchers, notifications) needs a stop-and-restart path it doesn't have today, and any one that keeps a handle breaks the swap; a crash mid-swap has no recovery point |
| D. A versioned, checksummed archive of the whole Farm, taken through the online backup API; restore stages and verifies it, then a journal drives the swap at the next startup, before `Storage::open` | Point-in-time and consistent; nothing is open during the swap, so the WAL moves with its database; every crash point is recoverable by the same installer; integrity holds by construction because the database is swapped whole | Restore needs a restart; staging needs disk space for the whole backup; local changes since the backup are replaced (the preview lists them and a safety backup keeps them) |

## Decision

**Option D.**

- **The archive.** One zip file, `.farm3d-backup`, `formatVersion` 1: a
  closed JSON manifest (format and schema versions, migration checksums,
  per-table counts, excluded classes, and a SHA-256 per entry), the
  database copy, every content blob, and the selected camera media.
- **Consistency.** The database is copied with SQLite's online backup API
  and validated. A process-wide lease stops every blob and media deletion
  while the backup runs, so every file the copy lists still exists when
  it is read.
- **Machine data stays out.** The copy is sanitized before it is hashed:
  Slicer runtime paths nulled, the telemetry cache and both pending-
  cleanup tables emptied, then `VACUUM`ed. Credential values are never
  read into or out of a backup.
- **Replace only.** Restore swaps the whole database, so there is nothing
  to merge and referential integrity holds by construction. The preview
  lists what the swap changes (`onlyLocal`, `changed`, `uniqueClash`), and
  restore is refused while local host or slicer work is in flight.
- **Install at the next startup.** `apply_restore` writes a safety
  backup and a journal, then restarts farm3d. After the metadata-root
  lease and before `Storage::open`, the installer moves the live database
  with its `-wal` and `-shm` as a set, places the staged candidate, adds
  the content, swaps the media directory, and validates the result
  (integrity, foreign keys, the integrity catalogue, and counts). It
  records each step in the journal before running it.
- **Every crash finishes or rolls back.** Before the install is recorded
  as installed, any crash or error rolls back to the moved-aside Farm and
  retries once; a second failure, or any verification failure, leaves
  the pre-restore Farm exactly as it was and records the failure. After
  it, the install rolls forward.
- **Resets reuse it.** An entire-Farm reset writes the same journal and
  only rolls forward.
- **Partial portability stays.** The Settings and Printers JSON exports
  are unchanged.

The P9 spec
(`docs/superpowers/specs/2026-09-29-p9-history-settings-backup-diagnostics-design.md`)
holds the manifest schema, the archive rules, the conflict fixtures, the
journal schema, the installer steps, and the fault-point table.

## Consequences

- A restore always restarts farm3d. If `AppHandle::restart` proves
  unreliable in an installed bundle, the operator quits and reopens
  instead, and the journal makes that equally safe.
- Local changes made since the backup are replaced. The preview names
  them and the safety backup keeps them, so restoring the safety backup
  undoes a restore.
- A restore needs free space for the staged backup and the safety backup.
  Both are checked before anything is written.
- `formatVersion` 1 and schemas from 10 on are a promise: committed
  fixture backups must keep restoring for as long as the format is
  accepted, and an older schema is migrated on the staged copy with the
  same embedded migrations.
- Linked Model paths are restored verbatim and may be missing on another
  machine; P4's source state and **Locate source** handle that. Printers
  whose credential isn't in the local store report `CREDENTIAL_REQUIRED`
  until it is re-entered.
- The startup order gains the installer, and the metadata-root lease is
  retried briefly when a journal exists, because the restarting process
  may still hold it.
- The backup isn't signed or encrypted. Someone who can rewrite the
  manifest can alter a backup; the checksums catch corruption, not
  tampering.
