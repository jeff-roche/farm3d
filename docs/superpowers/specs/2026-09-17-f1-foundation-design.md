# F1 Persistence and Cross-Tier Contract Design

**Status:** Approved

> [!NOTE]
> The user delegated approval authority, and approval of this design and
> ADR-0008 was granted in this conversation on 2026-09-17. This does not claim
> approval by any external stakeholder. The delegated approval satisfies the
> requirement that approval precede production implementation, so Task 2 may
> begin. The cross-tier `JsonValue` numeric clarification in this design is part
> of that already user-delegated approval; it does not claim separate or external
> approval.

## Purpose

F1 replaces the current `settings.json` and `printers.json` persistence paths
with transactional SQLite. It also gives every Tauri command and live Printer
status update one generated, versioned Rust-to-TypeScript contract.

The design preserves the F0 behavior that matters during migration:

- the canonical F0 settings and Printer fixtures are migration inputs;
- imported Printer IDs, Profile data, unknown override fields, Connection
  kinds, and credential references survive;
- credentials remain in the existing keychain or owner-only fallback file;
- Rust remains the owner of durable state; and
- the frontend listens before requesting status backfill.

The terms Printer, Printer Profile, Connection, Credential, Model, Slice,
Spool, Queue Entry, and Job have the meanings in `CONTEXT.md`.

## Scope

F1 delivers:

1. a SQLite storage core and ordered schema migrations;
2. one-time migration of the F0 settings and Printer documents;
3. revisioned Settings and Printer repositories;
4. recoverable coordination with the separate credential store;
5. generated command, error, event, import/export, and navigation contracts;
6. a versioned Printer-status event stream with race-free backfill;
7. separate editable Settings and Printers import/export documents; and
8. a narrow, internal SQLite safety-snapshot and restore-staging seam.

F1 does **not** define schemas for Models, Projects, Model Source Revisions,
Slice Revisions, Spools, Material Slots, Queue Entries, Jobs, Attention Events,
Incidents, or immutable history. It does not add full-Farm backup/restore UI,
diagnostics export, reset, OS deep-link registration, or new adapter protocols.
Those remain with their owning phases. P1 supersedes the earlier raw host
`jobState` treatment: adapters normalize it to canonical telemetry
`hostActivity` (while retaining an optional `hostActivityName`) and `jobName`
remains telemetry rather than becoming a Job record.

## Storage layout and injection

Production constructs one `StoragePaths` value during Tauri setup:

```text
metadata_root = app.path().app_config_dir()
database      = <metadata_root>/farm3d.sqlite3
ownership_lock = <metadata_root>/farm3d.lock
legacy_root   = <metadata_root>/legacy/
snapshot_root = <metadata_root>/snapshots/
content_root  = app.path().app_data_dir()/farm3d-content/v1/
```

`content_root` reserves a stable root for later large immutable content. F1
creates the directory but stores nothing in it and defines no content schema.
The SQLite database, legacy archives, and safety snapshots are metadata and
stay under `metadata_root`. The existing `credentials.json` fallback remains
beside the database for compatibility but is not part of SQLite.

After path validation, bootstrap acquires a `MetadataRootLease` before calling
`Storage::open(paths, &lease)` or opening either credential-store
implementation. Those are the only production construction paths. Tests pass
temporary `metadata_root` and `content_root` paths directly and acquire the same
lease unless they are specifically testing lock failure. Storage code must not
discover Tauri paths, mutate `XDG_CONFIG_HOME`, or touch a developer's keychain.
`app_config_dir()` and `app_data_dir()` may resolve to the same directory on
macOS or Windows; equal platform roots are valid. After creating the directories
needed for canonicalization, `StoragePaths` requires absolute paths and rejects
collisions among the concrete owned paths: `database` must not equal or sit
inside `legacy_root`, `snapshot_root`, or `content_root`, and those three
directory trees must be pairwise equal-or-nested-free. The fixed
`farm3d-content/v1/` suffix therefore remains disjoint even when both platform
roots are equal. Tests cover equal roots, normalization, symlink aliases, and
each rejected file/tree or tree/tree collision. Test helpers may accept relative
inputs only by canonicalizing them before lease acquisition and `Storage::open`.

## Metadata-root process ownership

Exactly one farm3d process may own a canonical `metadata_root`. Bootstrap opens
or creates `<metadata_root>/farm3d.lock` and attempts a non-blocking exclusive
advisory lock. It moves the resulting `MetadataRootLease` into managed bootstrap
state and holds the open locked handle until process shutdown, including while
the state is `Failed`. The lock file is never deleted as an unlock mechanism:
normal exit, forced termination, and crashes close the handle, so the operating
system releases the lock even though the file remains. A later process locks
that existing file normally; it does not treat the file as a stale owner marker.

If another process owns the root, or lock acquisition has another transient I/O
failure, bootstrap enters `Failed` with the safe, retryable structured
`PERSISTENCE_UNAVAILABLE` error. It does not open SQLite, initialize a keychain
service, open or inspect `credentials.json`, or run legacy import. A retryable
command may attempt the non-blocking lock again; no later bootstrap step runs
until it succeeds.

An unavailable advisory-lock primitive is not ordinary contention. If the
platform or backing filesystem reports that it cannot provide the required
primitive, setup fails before SQLite or credential-store access with a fatal
`Unsupported metadata locking` startup error and exits. This setup error is not
a `CommandError`, does not launch the normal command recovery UI, and offers no
retry action; its safe text tells the user that the selected platform or data
location cannot run farm3d and to contact support with the platform and
filesystem type, not a local path. farm3d never weakens this rule to a
process-local mutex.

This ownership lock is the cross-store serialization boundary for SQLite and
the credential store. SQLite locking remains enabled and still protects against
accidental database access by a non-farm3d process, but cross-store credential
coordination assumes every farm3d service holds the same metadata-root lease.

## SQLite configuration and connection ownership

F1 uses bundled SQLite through `rusqlite` with backup support. Every connection
sets:

```text
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
```

The writer additionally establishes and verifies:

```text
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
```

`Storage` owns one long-lived read/write connection behind an application
writer mutex. Every write callback acquires that mutex and begins `BEGIN
IMMEDIATE`; callbacks either commit completely or roll back completely. The
mutex is the application-level ordering rule, while SQLite still protects
against accidental database access outside farm3d. farm3d-to-farm3d exclusion
and coordination with the separate credential store come from the
metadata-root lease. A five-second busy timeout returns
`PERSISTENCE_UNAVAILABLE`; it does not spin indefinitely.

Reads open a fresh read-only connection to `farm3d.sqlite3`. A one-statement
read uses SQLite's implicit snapshot. A multi-statement read uses a deferred
read transaction so all statements see one WAL snapshot. A read never uses the
writer connection merely for convenience.

No SQLite transaction or writer lock may span a dialog, filesystem copy,
network request, credential-store call, supervisor stop/start, event emit, or
other await point. Code that coordinates those systems stages intent, performs
the external operation, and then uses a short transaction as described below.

## Initial schema

Migration `0001_foundation` creates the following tables. Timestamps are UTC
RFC 3339 strings. JSON columns have both `json_valid` and expected
`json_type(...)= 'object'` checks. Empty strings are rejected where noted.

### `schema_migrations`

| Column | Definition |
|---|---|
| `version` | `INTEGER PRIMARY KEY CHECK (version > 0)` |
| `name` | `TEXT NOT NULL UNIQUE` |
| `checksum` | lowercase SHA-256 hex, `TEXT NOT NULL` |
| `applied_at` | `TEXT NOT NULL` |

`PRAGMA user_version` equals the largest applied version. Both records must
agree at open time.

### `settings`

| Column | Definition |
|---|---|
| `singleton_id` | `INTEGER PRIMARY KEY CHECK (singleton_id = 1)` |
| `revision` | `INTEGER NOT NULL CHECK (revision >= 1)` |
| `theme_mode` | non-empty `TEXT NOT NULL` |
| `updated_at` | `TEXT NOT NULL` |

The table always contains exactly one row after initialization. `theme_mode`
remains a non-empty string because the current frontend permits registered
custom theme names as well as `system`, `farm3d-light`, and `farm3d-dark`.

### `printers`

| Column | Definition |
|---|---|
| `id` | `TEXT PRIMARY KEY CHECK (length(CAST(id AS BLOB)) BETWEEN 1 AND 512)`; repository validation rejects Unicode control characters |
| `revision` | `INTEGER NOT NULL CHECK (revision >= 1)` |
| `name` | `TEXT NOT NULL` |
| `catalog_vendor` | `TEXT NOT NULL` |
| `catalog_model` | `TEXT NOT NULL` |
| `catalog_variant` | `TEXT NOT NULL` |
| `catalog_model_id` | `TEXT NOT NULL` |
| `catalog_printer_variant` | `TEXT NOT NULL` |
| `notes` | `TEXT NOT NULL` |
| `overrides_json` | validated JSON object, `TEXT NOT NULL` |
| `last_known_good_json` | validated JSON object or `NULL` |
| `connection_json` | validated JSON object or `NULL` |
| `created_at` | `TEXT NOT NULL` |
| `updated_at` | `TEXT NOT NULL` |

Catalog identity has typed columns so it can be indexed and compared without
parsing JSON. Printer Profile overrides, last-known-good Profile data, and the
Connection remain nested structures. Repository decoding validates those
structures after SQLite's shape checks. Override decoding uses a flattened map
and re-encodes unknown keys unchanged. Monitor Section grouping is a temporary
frontend view preference, not Printer or Farm data, so this schema contains no
grouping column. `Connection.kind` is a non-empty, control-character-free
string, not an enum; unknown future kinds round-trip.

The Printer ID byte limit is enforced with the UTF-8 byte length, not Unicode
scalar count. SQLite enforces the non-empty and byte-length bounds; repository
validation rejects characters in Unicode general category `Cc`. IDs are never
trimmed, normalized, case-folded, split on separators, or used as path
components.

`connection_json` may contain an opaque `credentialRef`. It may never contain
an API key, password, token, authorization header, or other credential value.

### `legacy_imports`

| Column | Definition |
|---|---|
| `source_name` | `TEXT PRIMARY KEY`, limited to `settings.json` or `printers.json` |
| `source_present` | `INTEGER NOT NULL CHECK (... IN (0,1))` |
| `source_sha256` | lowercase SHA-256 hex or `NULL` when absent |
| `source_schema_version` | integer or `NULL` for unversioned settings |
| `imported_rows` | `INTEGER NOT NULL CHECK (imported_rows >= 0)` |
| `completed_at` | `TEXT NOT NULL` |

The two rows are committed in the same transaction as imported Settings and
Printers. Both rows together are the authoritative F0-import completion marker.

### `migration_warnings`

| Column | Definition |
|---|---|
| `id` | canonical UUID string, `TEXT PRIMARY KEY` |
| `code` | non-empty `TEXT NOT NULL` |
| `source_name` | `TEXT` or `NULL` |
| `source_sha256` | lowercase SHA-256 hex or `NULL` |
| `message` | safe, non-secret `TEXT NOT NULL` |
| `details_json` | validated JSON object, `TEXT NOT NULL` |
| `created_at` | `TEXT NOT NULL` |

`source_name` and `source_sha256` reject empty strings when non-null. The unique
expression index below treats nulls as one deduplication key instead of relying
on SQLite's distinct-null behavior:

```sql
CREATE UNIQUE INDEX migration_warnings_dedup
ON migration_warnings (
  code,
  COALESCE(source_name, ''),
  COALESCE(source_sha256, '')
);
```

This table is the durable location for a corrupt-settings default, an ignored
legacy Monitor Section group, a post-commit source change, archive failure, and
other non-blocking migration warnings. Warning details contain hashes and
counts, never source contents or credential values.

### `pending_credential_cleanup`

| Column | Definition |
|---|---|
| `credential_ref` | `TEXT PRIMARY KEY CHECK (length(CAST(credential_ref AS BLOB)) BETWEEN 1 AND 512)` |
| `printer_id` | `TEXT CHECK (printer_id IS NULL OR length(CAST(printer_id AS BLOB)) BETWEEN 1 AND 512)`; no foreign key because deletion may remove the Printer |
| `reason` | `TEXT NOT NULL CHECK (reason IN ('provisional','replaced','cleared','printer_deleted','import_orphan'))` |
| `attempt_count` | `INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0)` |
| `last_error_code` | `TEXT` or `NULL` |
| `created_at` | `TEXT NOT NULL` |
| `last_attempt_at` | `TEXT` or `NULL` |

Only opaque references and safe error codes are stored. Secret values and
credential-store error strings are forbidden. Repository validation applies the
same current-or-legacy credential-reference forms used by Connection data and
the same Unicode-control rule used for Printer IDs.

The table has one row per `credential_ref`, so every enqueue uses one atomic
`INSERT ... ON CONFLICT DO UPDATE` in the transaction that records the cleanup
intent. Conflicts choose the highest-precedence reason:

```text
printer_deleted > cleared > replaced > provisional > import_orphan
```

On conflict, the higher-precedence reason wins. For equal reasons, a non-null
`printer_id` wins over `NULL`, then the UTF-8 bytewise-lowest `printer_id` wins.
The winning tuple supplies both `reason` and `printer_id`, making the stored row
independent of enqueue order. `created_at`, `attempt_count`, `last_attempt_at`,
and `last_error_code` are retained.

`import_orphan` is ineligible for automatic cleanup only while it remains the
stored reason. A later `provisional`, `replaced`, `cleared`, or
`printer_deleted` enqueue atomically promotes it to eligible automatic cleanup.
An `import_orphan` enqueue never demotes an automatic reason. Precedence among
automatic reasons affects the retained explanation only: before every deletion,
committed Printer reachability is still the final gate.

## IDs and revisions

- New Printer IDs are `prn-` followed by a lowercase hyphenated UUID v4.
- Existing F0 and editable-import Printer IDs use one exact rule: the decoded
  JSON string must be 1–512 UTF-8 bytes and contain no character in Unicode
  general category `Cc`. Every accepted byte is preserved exactly. Spaces,
  non-ASCII characters, and separators such as `/`, `\\`, `:`, `.`, and `-`
  are allowed. IDs are never trimmed, normalized, case-folded, split, or
  rewritten into UUIDs.
- Event IDs, stream IDs, warning IDs, snapshot IDs, and the UUID in a newly
  generated credential reference are lowercase hyphenated UUID v4 values.
- Settings and Printer revisions start at `1`.
- Every successful durable mutation increments the affected revision exactly
  once, even when the submitted values are equal. Failed and cancelled
  operations do not increment it.
- Settings save and every Printer update, override, rebind, drift-resolution,
  Connection mutation, and delete request carry `expectedRevision`. A mismatch
  returns `CONFLICT` with the safe `entityId`, `expectedRevision`, and
  `currentRevision` details and no write. Successful `save_settings` returns
  the committed authoritative Settings record, including its incremented
  revision and `updatedAt`; callers replace their cached record with that
  result rather than predicting the revision.
- F0 imports create revision `1`. A Printers document import gives a new ID
  revision `1` and gives an existing ID `existing.revision + 1`; the document's
  exported revision is validated as a positive integer but is provenance only
  and is not copied into local concurrency state.
- Editable Settings import carries the Settings revision captured before its
  dialog. Editable Printers import carries the complete Printer ID/revision set
  captured before its dialog, sorted by exact ID bytes. After the safety
  snapshot, the final `BEGIN IMMEDIATE` transaction rechecks the applicable
  precondition before any replacement write. A mismatch returns `CONFLICT` and
  leaves the concurrent state unchanged.

UUID generation failure returns `INTERNAL`; F1 never falls back to clocks,
counters, names, array positions, or catalog IDs.

## Embedded schema migrations

Migrations are ordered SQL assets compiled into the binary. On open, Storage:

1. opens the writer and configures its pragmas;
2. reads `user_version` and `schema_migrations`;
3. rejects a version newer than the binary with
   `UNSUPPORTED_SCHEMA_VERSION`;
4. rejects a missing, reordered, or checksum-changed applied migration with
   `MIGRATION_FAILED`;
5. applies all pending migrations in one exclusive startup transaction;
6. inserts each migration record and updates `user_version` in that same
   transaction; and
7. runs `PRAGMA foreign_key_check` before commit.

A crash before commit leaves the previous schema. A crash after commit leaves
the new schema and matching migration records. F1 never edits an applied
migration and never attempts a downgrade.

## F0 import

### Inputs and preparation

Only `<metadata_root>/settings.json` and
`<metadata_root>/printers.json` are legacy data inputs. Migration must not open,
hash, copy, rename, or inspect `credentials.json`. Credential references are
read only as strings inside Printer Connection data.

Before the database import transaction, the importer reads each present input
once, computes SHA-256 over those bytes, and writes the exact bytes plus a small
manifest to `<legacy_root>/.staging/<uuid>/` using same-directory temporary
files, file sync, atomic rename, and directory sync where supported. Parsing is
performed from the staged bytes, not by reopening a changing source path.

The settings F0 format is unversioned. A missing `schemaVersion` is therefore
the supported legacy form. A present positive `schemaVersion` is a future
format and returns `UNSUPPORTED_SCHEMA_VERSION`. The Printers input must have
`schemaVersion: 1`; a larger integer returns
`UNSUPPORTED_SCHEMA_VERSION`, while a missing, zero, negative, or non-integer
value is `CORRUPT_DATA`.

### Missing, corrupt, and valid inputs

| Input state | Result |
|---|---|
| Both missing | Insert default Settings at revision 1 and no Printers; mark both sources absent. |
| Settings missing | Import Printers and insert default Settings; mark Settings absent. |
| Printers missing | Import Settings and no Printers; mark Printers absent. |
| Settings malformed or structurally invalid | Use default Settings, preserve/archive the raw input, and insert `LEGACY_SETTINGS_DEFAULTED` in `migration_warnings`. |
| Printers malformed, structurally invalid, or has duplicate IDs | Do not import either domain and do not mark completion. Preserve both canonical source paths and return `CORRUPT_DATA`. After rollback, commit one deduplicated `LEGACY_PRINTERS_BLOCKED` warning in a separate short transaction. |
| Either input has a future schema | Do not import either domain, leave canonical sources untouched, and return `UNSUPPORTED_SCHEMA_VERSION`. |
| Both supported | Import Settings and all Printers in one transaction. |

Printer validation preserves every unknown override key and every unknown
Connection kind. Before typed decoding, a recursive key scan visits every
object reachable through arbitrary or unknown imported JSON, including objects
inside arrays and every flattened override value. It case-folds each key and
removes non-alphanumeric characters. A normalized key equal to `password`,
`authorization`, `authheader`, `cookie`, or `setcookie`, or ends in `password`,
`passwd`, `secret`, `token`, `apikey`, `privatekey`, `accesskey`, or
`credential`, rejects the document; this catches forms such as `clientSecret`
and `refresh_token` without inspecting values. The typed
`connection.credentialRef` field is the sole exception and is allowed only at
that exact path. A `credentialRef` key in arbitrary JSON is also rejected. F0
maps a rejection to blocking `CORRUPT_DATA`; editable import maps it to
`VALIDATION`. The rejected value is never included in the error. Other unknown
keys and supported values inside extension-preserving maps round-trip unchanged.
For arbitrary JSON numbers, “supported” means the cross-tier numeric subset
defined under “Generated wire contracts.” An unsupported number in an unknown
extension blocks F0 import with `CORRUPT_DATA`; the importer never silently
rounds it.

Every F0 Printer ID is validated with the exact rule in “IDs and revisions”
before duplicate detection. Accepted IDs, including IDs with separators, are
inserted and later exported byte-for-byte; the importer never derives a path or
credential reference from them.

F0 `group` fields are recognized only as obsolete Monitor Section view data.
They are not inserted into Printer state. If one or more are present, import
adds one `LEGACY_MONITOR_GROUP_IGNORED` warning for the Printers source hash,
with only the affected Printer count in `details_json`. The normalized warning
index makes retries deduplicate it. This rule is covered by the canonical F0
fixture, which has two `group` fields; migration asserts both are absent from
stored and exported Printers and exactly one warning reports `printerCount: 2`.

An F0 `credentialRef` is preserved when it is no more than 512 UTF-8 bytes,
contains no character in Unicode general category `Cc`, and has this exact
legacy structure:

```text
farm3d/printer/<owner>/apikey
```

`<owner>` is the non-empty opaque string between the exact prefix and suffix.
It may contain `/`, `\\`, `:`, spaces, non-ASCII characters, or other
non-control separators. It is not parsed as path segments, normalized, or
required to equal a Printer ID. A reference may therefore be shared by multiple
Printers or carry an owner from an earlier Printer identity. Anything outside
this form is malformed and blocks F0 import with `CORRUPT_DATA`. Editable
schema-1 imports accept this legacy form and the current canonical form defined
below so every migrated reference can be exported and imported unchanged.

### Transaction and crash boundaries

The single F0 transaction:

1. requires that neither `legacy_imports` completion row exists;
2. inserts the singleton Settings row;
3. inserts every Printer at revision 1;
4. inserts all migration warnings, including settings-default and ignored-group
   warnings;
5. inserts both `legacy_imports` rows with hashes, source versions, and counts;
6. checks expected row counts and foreign keys; and
7. commits.

Crash behavior is exact:

- Before staging completes: no database write occurred; retry reads the
  canonical source again and removes incomplete staging directories.
- After staging but before database commit: retry reuses staging only when its
  manifest still matches the canonical source hash; otherwise it discards the
  staging directory and starts again.
- During the SQLite transaction: rollback leaves no Settings, Printer, or
  completion-marker subset.
- After commit but before archival: the two completion rows prevent re-import;
  retry performs archival only.
- During archival: completed renames are accepted and missing ones are resumed;
  collision-safe names make the operation idempotent.
- After archival: SQLite is authoritative and legacy files are never read as
  live state.

### Archival and changed sources

Final archives live in `<legacy_root>/` and use:

```text
settings.migrated-<YYYYMMDDTHHMMSSZ>-<first-12-sha256>.json
printers.migrated-<YYYYMMDDTHHMMSSZ>-<first-12-sha256>.json
settings.changed-after-import-<YYYYMMDDTHHMMSSZ>-<first-12-sha256>.json
printers.changed-after-import-<YYYYMMDDTHHMMSSZ>-<first-12-sha256>.json
settings.ignored-after-migration-<YYYYMMDDTHHMMSSZ>-<first-12-sha256>.json
printers.ignored-after-migration-<YYYYMMDDTHHMMSSZ>-<first-12-sha256>.json
```

If a name exists, append `-2`, `-3`, and so on before `.json`; never overwrite
an archive. If the canonical source still matches the committed hash, rename it
to the `migrated` name and remove the staged duplicate. If it changed after the
staged read, archive the staged imported bytes as `migrated`, archive the current
canonical bytes as `changed-after-import`, and record a warning. The changed
bytes are not imported automatically.

If a source was absent when completion committed but appears later, archive it
as `ignored-after-migration` and record a warning. If staging is unexpectedly
missing after commit, preserve the current source under the appropriate changed
or ignored name, record `LEGACY_ARCHIVE_INCOMPLETE`, and never undo or repeat
the committed import.

Archive failure does not make SQLite non-authoritative. It records a warning
when possible and retries archival at the next startup. A matching canonical
legacy file must not be edited or deleted while archival is pending.

## Safety snapshots and restore staging

Before either editable import, Storage creates a consistent SQLite snapshot
with SQLite's online backup API. It never copies a live database, `-wal`, or
`-shm` file directly.

The temporary name is:

```text
<snapshot_root>/.farm3d-pre-import-<settings|printers>-<timestamp>-<uuid>.sqlite3.partial
```

After backup, a separate read-only connection must confirm:

- the expected supported `user_version` and migration checksums;
- `PRAGMA integrity_check` returns exactly `ok`; and
- `PRAGMA foreign_key_check` returns no rows.

The file is then synced and atomically renamed without `.partial`. Snapshot
creation or validation failure aborts the import before any domain write.
Incomplete `.partial` files are deleted at startup. Keep the five newest valid
pre-import snapshots across both domains. Retention runs only after a new
snapshot is accepted; deletion failure records a warning but does not invalidate
the new snapshot or a completed import.

F1 exposes no restore command or UI. Its narrow internal restore seam accepts a
selected snapshot path under `snapshot_root`, validates it as above, and uses
the online backup API to copy it to
`<snapshot_root>/.restore-staging/<uuid>/candidate.sqlite3`. It validates the
staged copy again and returns only its opaque staging ID and validation summary.
F1 never closes the active writer to install a candidate, never renames or
replaces `farm3d.sqlite3`, and never manipulates database `-wal` or `-shm`
sidecars for restore. Startup removes incomplete or invalid restore-staging
directories but does not consume a valid candidate. P9 owns selection UI,
sidecar-safe database replacement, post-install validation, rollback, and
cleanup. Credentials and large content are outside the staged database.

## Credential coordination

The credential store stays outside SQLite. The safe invariant is: a harmless
orphan may exist temporarily, but a committed Printer must never point to a
credential value that was not stored successfully.

New credential values receive a new reference:

```text
farm3d/credential/<uuid-v4>
```

The UUID is canonical lowercase hyphenated UUID v4. The reference is opaque and
independent of every Printer ID. Imported legacy references remain unchanged.
Credential-changing operations are serialized by one credential coordinator
mutex and follow this order:

1. In a short database transaction, insert the new reference into
   `pending_credential_cleanup` with reason `provisional`.
2. Write the new secret under that reference in the credential store.
3. If the write fails, leave the Printer unchanged. Best-effort cleanup may run;
   the durable provisional row guarantees startup retry.
4. In a second short database transaction, recheck `expectedRevision`, update
   the Printer to the new reference, increment its revision, remove the new
   provisional row, and enqueue the old reference as `replaced`.
5. After commit, reconcile the supervisor from committed state, then attempt old
   reference cleanup. Remove a cleanup row only after deletion succeeds.

If step 4 fails, the old Printer state remains valid and the provisional new
reference remains queued for deletion. Explicit credential clear, Connection
clear, Printer delete, and Printers replacement commit the reference removal
and cleanup row in one database transaction, then delete the credential after
commit. A cleanup failure therefore leaves an orphan, never a dangling
reference. Startup retries automatically eligible cleanup before restoring
supervisors.

Startup automatically retries `provisional`, `replaced`, `cleared`, and
`printer_deleted` rows. It does not process `import_orphan`: bulk replacement
orphans remain available for later explicit cleanup because deleting them would
make a user-directed rollback or corrected re-import irreversible. F1 adds no
general credential-cleanup UI.

Legacy imports may contain the same credential reference on more than one
Printer. Every deletion attempt therefore runs under the credential coordinator
mutex and performs this ordering: read committed Printer rows in a short SQLite
transaction; if any Connection still reaches the reference, retain the secret
and its cleanup row without incrementing `attempt_count`; otherwise delete the
secret; then remove the cleanup row in a short transaction only after deletion
succeeds. A Printer or Connection mutation that removes a reference commits its
cleanup intent through the precedence upsert before releasing the coordinator
and triggers another check. Thus a reference first retained as an
`import_orphan` becomes automatically eligible if a later clear, replacement,
provisional rollback, or Printer deletion records stronger cleanup intent, while
a later import cannot make an automatically eligible row ineligible. Startup
checks occur under the same coordinator before supervisors start. Thus
a cleanup enqueued by one deleted Printer cannot remove a value still shared by
another committed Printer, and concurrent rebind/delete operations are ordered
against both reachability checks and credential deletion.

For `set_printer_connection`, an omitted credential field preserves the
existing reference and performs no credential-store call. An explicitly empty
submitted credential clears it. `test_printer_connection` does not mutate the
Connection: an omitted credential tells the probe to load the identified
Printer's committed `credentialRef` and look up that credential, while an
explicit empty string probes without a credential. A non-empty probe credential
is used only from transient request memory. A missing stored reference or value
when the adapter requires one returns `CREDENTIAL_REQUIRED`. Database access or
decoding during probe lookup can return `PERSISTENCE_UNAVAILABLE` or
`CORRUPT_DATA`. A probe never returns, logs, or persists a credential value.

### Printer deletion reconciliation

`delete_printer` first uses one short transaction to recheck
`expectedRevision`, delete the Printer, and enqueue any removed credential
reference with reason `printer_deleted`. After commit, under the Connection
manager reconciliation mutex, it removes the supervisor handle from the manager,
requests a graceful stop, and awaits the task. A bounded graceful-stop failure
aborts the cancellable supervisor task and awaits cancellation, so the deleted
Printer cannot continue polling or publishing silently. It then removes the
Printer's entry from the live status map under the status-map mutex.
Credential cleanup is attempted after supervisor and status reconciliation.

The committed deletion is authoritative even if graceful supervisor shutdown or
status reconciliation reports a post-commit failure. The command returns
`DeletePrinterResult` with `SUPERVISOR_RECONCILIATION_FAILED` in `warnings`
instead of returning an error that invites a duplicate delete. Credential
cleanup failure sets `credentialCleanupPending` and adds
`CREDENTIAL_CLEANUP_PENDING`. A crash at any point after commit is repaired at
startup: only committed Printer rows receive supervisors, and the in-memory
status map is rebuilt from that set.

### Fallback credential file

The fallback credential service requires the metadata-root lease, and all of
its reads and writes also share one process-wide mutex. A mutation reads and
validates the current map, writes the complete replacement to a random
same-directory temporary file created owner-only, syncs the file, reapplies
owner-only permissions, atomically replaces `credentials.json` using the
platform replace primitive, and syncs the parent directory where supported. It
never truncates the live file in place and never implements replacement as
delete-then-rename. On Unix the file mode is `0600` from its first byte and after
every replacement. A leftover temporary file is ignored and removed on the next
credential-store open.

## Generated wire contracts

Rust wire types are the source. `ts-rs` version 12 generates committed files
under `src/generated/contracts/`. A deterministic Rust export test writes to a
temporary directory and byte-compares every generated file with the committed
directory. `just gen-contracts` is the only regeneration entry point.

Wire structs use `#[serde(rename_all = "camelCase")]` and matching ts-rs names.
String enums use explicit `#[serde(rename_all = "SCREAMING_SNAKE_CASE")]` for
error/recovery codes or explicit lower-camel values for domain enums. Optional
fields use `Option<T>`, `skip_serializing_if = "Option::is_none"`, and an
optional TypeScript property; `null` is used only where the contract explicitly
needs it. Tagged unions use `#[serde(tag = "status", rename_all =
"camelCase")]`. Maps that must generate deterministically use `BTreeMap`.

Wire JSON uses a generated recursive `JsonValue` union rather than TypeScript
`any`. Persistence structs, SQLite rows, stored credential values, adapters,
Tauri handles, and supervisor internals are not exported. The generated
secret-bearing `ConnectionSubmission` type is input-only. Its `credential`
value may exist transiently in form-local/request memory and Tauri IPC
serialization, but it is never copied into a persistent or reactive frontend
store, cache, success or error object, event, log, generated output fixture, or
durable state. A `finally` block clears the credential form state and releases
all application-held references to the submission and IPC request after
`invoke` settles, whether it succeeds or fails. Where the backend owns a mutable
Rust buffer that holds the submitted value, its secret wrapper zeroizes that
buffer when its use ends. Browser input state, immutable JavaScript strings,
and copies owned by the WebView,
Tauri IPC, serializer, operating system, or credential-store framework cannot
be guaranteed zeroized; the guarantee for those copies is non-retention by
farm3d application code, not in-place erasure. Compile fixtures cover type shape
without using a credential value, plus renamed fields, omitted options,
flattened unknown override fields, tagged unions, generic envelopes, events,
and navigation.

### `JsonValue` numeric subset

The approved F1 cross-tier `JsonValue` contract supports only arbitrary JSON
numbers that are finite and round-trip through a JavaScript `number`. Integers
must be within JavaScript's safe range, `-(2^53 - 1)` through `2^53 - 1`,
inclusive. Non-finite values are not JSON and are unsupported. Decimal values
whose precision would change when parsed as a JavaScript `number`, including
unsupported high-precision decimals, are also rejected. This restriction
applies recursively to arbitrary or unknown JSON, including flattened override
values, error details, and future extension maps.

Validation occurs before a value crosses the Rust-to-TypeScript boundary or is
persisted. Unsupported numbers are never silently rounded. Legacy F0 import
reports blocking `CORRUPT_DATA`; editable import reports `VALIDATION` and
identifies the rejected value by safe field path. Supported unknown values keep
the existing byte- or semantic-preservation guarantee appropriate to their type,
including accepted `JsonValue` number semantics through export and re-import.

Every command returns one of:

```ts
type CommandSuccess<T> = {
  contractVersion: 1;
  data: T;
};

type CommandError = {
  contractVersion: 1;
  code: ErrorCode;
  message: string;
  recovery: RecoveryCode[];
  retryable: boolean;
  correlationId?: CorrelationId;
  fieldErrors?: Record<string, string>;
  details?: Record<string, JsonValue>;
};

type CorrelationId = string; // `err-` plus a lowercase hyphenated UUID v4
```

Commands with no domain result return `data: null`. The frontend's
`src/ipc/client.ts` is the only module that imports Tauri `invoke`. It validates
`contractVersion`, unwraps success, and preserves a rejected `CommandError`
object. Frontend behavior switches on `code` and `recovery`, never message text.

### Exact error vocabulary

`ErrorCode` has exactly these F1 members:

```text
VALIDATION
NOT_FOUND
CONFLICT
PERSISTENCE_UNAVAILABLE
CORRUPT_DATA
MIGRATION_FAILED
UNSUPPORTED_SCHEMA_VERSION
CREDENTIAL_UNAVAILABLE
CREDENTIAL_REQUIRED
UNSUPPORTED_ADAPTER
PRINTER_UNREACHABLE
AUTHENTICATION_FAILED
PROTOCOL_ERROR
TIMEOUT
INCOMPATIBLE_CONTRACT_VERSION
INTERNAL
```

`RecoveryCode` has exactly these F1 members:

```text
RETRY
EDIT_FIELDS
RELOAD
REENTER_CREDENTIAL
CHOOSE_SUPPORTED_ADAPTER
CHECK_CONNECTION
CHECK_CREDENTIALS
RESTART_APPLICATION
UPGRADE_FARM3D
```

The error mapping is normative. A command may return only the codes listed for
it in the command matrix below. Within a code, `retryable` and `recovery` use
this table; bracketed lists are the complete allowed recovery set, ordered as
sent to the frontend.

| `ErrorCode` | `retryable` | `recovery` | Allowed `details` |
|---|---:|---|---|
| `VALIDATION` | false | [`EDIT_FIELDS`] | `fieldPath` |
| `NOT_FOUND` | false | [`RELOAD`] | `entityId` |
| `CONFLICT` | false | [`RELOAD`] | `entityId`, `expectedRevision`, `currentRevision`, `expectedCount`, `currentCount` |
| `PERSISTENCE_UNAVAILABLE` | true | [`RETRY`] | none |
| `CORRUPT_DATA` | false | [`EDIT_FIELDS`] for a selected editable import; `[]` for blocking legacy or database corruption | `sourceName`, `sourceSha256` |
| `MIGRATION_FAILED` | false | `[]` | `migrationVersion` |
| `UNSUPPORTED_SCHEMA_VERSION` | false | [`UPGRADE_FARM3D`] | `supportedVersion`, `receivedVersion` |
| `CREDENTIAL_UNAVAILABLE` | true | [`RETRY`] | `credentialStoreKind` |
| `CREDENTIAL_REQUIRED` | false | [`REENTER_CREDENTIAL`] | `entityId` |
| `UNSUPPORTED_ADAPTER` | false | [`CHOOSE_SUPPORTED_ADAPTER`] | `adapterKind` |
| `PRINTER_UNREACHABLE` | true | [`CHECK_CONNECTION`, `RETRY`] | `entityId` |
| `AUTHENTICATION_FAILED` | false | [`CHECK_CREDENTIALS`, `REENTER_CREDENTIAL`] | `entityId` |
| `PROTOCOL_ERROR` | true | [`CHECK_CONNECTION`, `RETRY`] | `entityId`, `adapterKind` |
| `TIMEOUT` | true | [`CHECK_CONNECTION`, `RETRY`] | `entityId` |
| `INCOMPATIBLE_CONTRACT_VERSION` | false | [`UPGRADE_FARM3D`] | `supportedVersion`, `receivedVersion` |
| `INTERNAL` | false | [`RESTART_APPLICATION`] | none; use `correlationId` |

`correlationId` is an optional typed top-level field, not a free-form detail. It
is present only for `INTERNAL` errors generated from unexpected failures and is
absent for every other code. Details, messages, field errors, correlation IDs,
and logs must not contain credential values, authorization material, full
filesystem paths, source-document contents, database SQL, raw adapter responses,
or URLs containing user information. Unexpected errors map to a generic
`INTERNAL` message and a newly generated correlation ID; underlying
display/debug text does not cross IPC.

For selected editable import, `sourceName` is the fixed value
`settingsImport` or `printersImport`; no selected basename or path crosses IPC.
For blocking F0 corruption it is `settings.json` or `printers.json`, and the
staged input hash may be supplied as `sourceSha256`. For authoritative database
corruption it is `database`, with no hash or path. Blocking `CORRUPT_DATA` has
an empty recovery list because F1 has no archive-opening or database-repair
command. Its fixed safe message and the failed-bootstrap recovery UI tell the
user to preserve the data directory and contact support; they do not render a
callable recovery action. These values are captured while bootstrap still has
the source context, so every handler can return the same error from `Failed`
without opening Storage or assuming that an archive exists.

`MIGRATION_FAILED`, including an applied-migration checksum mismatch, also has
an empty recovery list. Its fixed safe message tells the user to preserve the
data directory and contact support; it does not claim that restart, retry, or
automatic repair can correct the database. The message does not expose a path,
SQL, checksum, or migration contents.

User cancellation of an open/save dialog is a successful tagged `cancelled`
outcome, not an error. An incompatible incoming contract version is
`INCOMPATIBLE_CONTRACT_VERSION`; a future persisted/import schema is
`UNSUPPORTED_SCHEMA_VERSION`.

## Command inventory

F0 registers 21 commands. F1 removes `open_settings_file` and
`open_printers_file`, retains the other 19, and adds four import/export
commands. The final inventory is exactly 23:

1. `load_settings`
2. `save_settings`
3. `export_settings`
4. `import_settings`
5. `list_printers`
6. `create_printer`
7. `update_printer`
8. `delete_printer`
9. `set_printer_override`
10. `rebind_printer`
11. `resolve_profile_drift`
12. `export_printers`
13. `import_printers`
14. `list_catalog_models`
15. `list_catalog_variants`
16. `preview_profile`
17. `catalog_info`
18. `set_printer_connection`
19. `clear_printer_connection`
20. `test_printer_connection`
21. `credential_store_info`
22. `discover_printers`
23. `printer_statuses`

All 23 return `Result<CommandSuccess<T>, CommandError>`, including catalog
lookups, discovery, and status backfill. Discovery task failure is an error,
not an empty successful result. Mutations return the committed authoritative
record and revision. Import/export return the tagged outcomes defined below.

### Command request and result aliases

Every request includes `contractVersion: 1`, including commands with no domain
arguments. The IPC client rejects another version before dispatch where
possible; Rust still validates it. These generated aliases make omitted fields
and no-argument commands explicit:

```ts
type NoArgsRequest = { contractVersion: 1 };
type EntityRequest = { contractVersion: 1; id: string };
type RevisionedEntityRequest = EntityRequest & { expectedRevision: number };
type SettingsImportRequest = {
  contractVersion: 1;
  expectedRevision: number;
};
type PrinterRevisionPrecondition = { id: string; revision: number };
type PrintersImportRequest = {
  contractVersion: 1;
  expectedRevisions: PrinterRevisionPrecondition[];
};

type SettingsRecord = {
  revision: number;
  themeMode: string;
  updatedAt: string;
};

type CatalogRef = {
  vendor: string;
  model: string;
  variant: string;
  modelId: string;
  printerVariant: string;
};

type ConnectionConfig = {
  kind: string;
  host: string;
  port: number;
  useTls: boolean;
  credentialRef?: string;
};

type ProfileResolution = {
  catalogStatus: "ok" | "rematched" | "variantMissing" | "modelMissing" | "vendorMissing";
  modelLabel: string;
  variantLabel: string;
  profile: PrinterProfile;
  overriddenFields: string[];
  inherited: Record<string, JsonValue>;
  profileDrift: Array<{ field: string; from: JsonValue; to: JsonValue }>;
  unknownOverrideKeys: string[];
};

type CatalogModelSummary = { modelId: string; vendor: string; model: string };
type CatalogVariantSummary = { variant: string; printerVariant: string };

type PrinterRecord = {
  id: string;
  revision: number;
  name: string;
  catalogRef: CatalogRef;
  notes: string;
  overrides: Record<string, JsonValue>;
  lastKnownGood?: LastKnownGood;
  connection?: ConnectionConfig;
  profileResolution: ProfileResolution;
  createdAt: string;
  updatedAt: string;
};

type OperationWarning = {
  code:
    | "CREDENTIAL_CLEANUP_PENDING"
    | "CREDENTIAL_REQUIRED"
    | "SUPERVISOR_RECONCILIATION_FAILED";
  entityId?: string;
};

type PrinterMutationResult = {
  printer: PrinterRecord;
  warnings: OperationWarning[];
};

type DeletePrinterResult = {
  deletedId: string;
  deletedRevision: number;
  credentialCleanupPending: boolean;
  warnings: OperationWarning[];
};

type ExportResult =
  | { status: "cancelled" }
  | { status: "exported"; exportedAt: string; recordCount: number }
  | { status: "unsupported"; reason: "desktopRequired" };

type SettingsImportResult =
  | { status: "cancelled" }
  | { status: "applied"; settings: SettingsRecord; warnings: OperationWarning[] }
  | { status: "unsupported"; reason: "desktopRequired" };

type PrintersImportResult =
  | { status: "cancelled" }
  | {
      status: "applied";
      printers: PrinterRecord[];
      createdCount: number;
      updatedCount: number;
      deletedCount: number;
      warnings: OperationWarning[];
    }
  | { status: "unsupported"; reason: "desktopRequired" };
```

`PrinterRecord` deliberately has no `group` or Monitor Section field. Catalog,
Profile, Connection, status, probe, and discovery shapes are generated named
types; none contains a credential value. `ConnectionSubmission.credential` is
the one secret-bearing request field. For a Connection mutation, omission
preserves the committed reference and an empty string clears it. For a probe,
omission requests lookup through the identified Printer's committed reference
and an empty string probes without a credential. A non-empty value is consumed
from transient form/request memory in either command. IPC serialization may
hold the value for the duration of the request. The frontend must not place the
submission or its credential in a persistent or reactive store, cache, success
or error object,
event, log, generated output fixture, closure, or retained IPC capture. In a
`finally` block it clears form state and releases application request references
after `invoke` settles. The backend never echoes the value or places it in
durable state. Where it owns a mutable secret buffer, its input wrapper zeroizes
that buffer on drop. Immutable JavaScript strings and framework/runtime-owned
copies are outside the zeroization guarantee.

For compact error sets in the matrix, the following names are exact unions:

- **Wire** = `INCOMPATIBLE_CONTRACT_VERSION | INTERNAL`.
- **Storage** = Wire + `PERSISTENCE_UNAVAILABLE | CORRUPT_DATA`.
- **Edit** = Storage + `VALIDATION | NOT_FOUND | CONFLICT`.
- **Editable import** = `INCOMPATIBLE_CONTRACT_VERSION | INTERNAL |
  PERSISTENCE_UNAVAILABLE | CORRUPT_DATA | VALIDATION | CONFLICT |
  UNSUPPORTED_SCHEMA_VERSION`.
- **Probe** = Storage + `VALIDATION | NOT_FOUND | CREDENTIAL_UNAVAILABLE |
  CREDENTIAL_REQUIRED | UNSUPPORTED_ADAPTER | PRINTER_UNREACHABLE |
  AUTHENTICATION_FAILED | PROTOCOL_ERROR | TIMEOUT`.

The matrix lists errors after bootstrap reaches `Ready`. Independently, every
registered command can return the one captured bootstrap error while managed
state is `Failed`: `PERSISTENCE_UNAVAILABLE`, startup `CORRUPT_DATA`,
`MIGRATION_FAILED`, `UNSUPPORTED_SCHEMA_VERSION`, or generic `INTERNAL`. A
request with an incompatible contract version is still rejected before
managed-state lookup.

### Command contract matrix

The fields shown are the complete domain fields in addition to
`contractVersion: 1`. `expectedRevision` must be a positive integer. Error
retryability, recovery, and allowed details come from the normative error table
above; the final column narrows context-dependent details or recovery.

| Command | Request fields | Success `data` | Applicable errors | Special details and recovery |
|---|---|---|---|---|
| `load_settings` | none (`NoArgsRequest`) | `SettingsRecord` | Storage | Runtime `CORRUPT_DATA` is blocking database corruption with `recovery: []` because the authoritative store can no longer be trusted. |
| `save_settings` | `expectedRevision`, `themeMode` | authoritative committed `SettingsRecord` | Edit minus `NOT_FOUND` | `CONFLICT` identifies Settings as `entityId: "settings"`; no write occurs. |
| `export_settings` | none (`NoArgsRequest`) | `ExportResult`; `recordCount` is `1` when exported | Storage | Dialog cancellation is success. Destination I/O failure is `PERSISTENCE_UNAVAILABLE`; no path is returned. |
| `import_settings` | `expectedRevision` (`SettingsImportRequest`) | `SettingsImportResult` | Editable import | Selected-document `CORRUPT_DATA` uses [`EDIT_FIELDS`] and `sourceName: "settingsImport"`; database corruption uses `[]`. A future document schema returns `UNSUPPORTED_SCHEMA_VERSION` with [`UPGRADE_FARM3D`]. The command may reject a stale revision before snapshot creation, but it must recheck the same revision inside the final post-snapshot `BEGIN IMMEDIATE`; `CONFLICT` identifies Settings as `entityId: "settings"`, and no import write occurs. |
| `list_printers` | none (`NoArgsRequest`) | `PrinterRecord[]`, sorted by `id` | Storage | No Printers is successful `[]`. |
| `create_printer` | `name`, `catalogRef` | `PrinterMutationResult` | Storage + `VALIDATION` | No revision is expected because the ID is generated. An unresolved catalog reference is `VALIDATION` at `catalogRef`. |
| `update_printer` | `id`, `expectedRevision`, `patch: { name?: string; notes?: string }` | `PrinterMutationResult` | Edit | Empty `patch` is `VALIDATION`, not a successful no-op. |
| `delete_printer` | `id`, `expectedRevision` | `DeletePrinterResult` | Edit | The deleted revision is the matched pre-delete revision. Deferred credential deletion sets `credentialCleanupPending` and a cleanup warning. A post-commit supervisor failure adds `SUPERVISOR_RECONCILIATION_FAILED`; neither condition turns a committed delete into an error. |
| `set_printer_override` | `id`, `expectedRevision`, `field`, `value: JsonValue | null` | `PrinterMutationResult` | Edit | `null` removes the named override. `fieldPath` identifies an unsupported field or invalid value. |
| `rebind_printer` | `id`, `expectedRevision`, `catalogRef` | `PrinterMutationResult` | Edit | An unresolved reference is `VALIDATION` at `catalogRef`. |
| `resolve_profile_drift` | `id`, `expectedRevision`, `action: "accept" | "pin"` | `PrinterMutationResult` | Edit | No current drift is `CONFLICT` with the current revision; it is not silently accepted. |
| `export_printers` | none (`NoArgsRequest`) | `ExportResult`; `recordCount` is the exported Printer count | Storage | Export is sorted by `id`; cancellation and destination failures follow `export_settings`. |
| `import_printers` | `expectedRevisions: PrinterRevisionPrecondition[]` (`PrintersImportRequest`) | `PrintersImportResult` | Editable import | The request must be sorted by exact ID bytes, contain each ID once, and represent the complete set captured before the dialog; malformed vectors are `VALIDATION`. Selected-document `CORRUPT_DATA` uses [`EDIT_FIELDS`] and `sourceName: "printersImport"`; database corruption uses `[]`. A future document schema returns `UNSUPPORTED_SCHEMA_VERSION` with [`UPGRADE_FARM3D`]. The command may reject a stale set before snapshot creation, but inside the final post-snapshot `BEGIN IMMEDIATE` it must compare the complete sorted committed ID/revision set again. Any addition, update, or deletion returns `CONFLICT` before a write; `details` uses `entityId` plus `expectedRevision`/`currentRevision` only when one shared ID's revision differs and otherwise only `expectedCount`/`currentCount`. |
| `list_catalog_models` | none (`NoArgsRequest`) | `CatalogModelSummary[]` sorted by vendor, model, then model ID | Wire | Empty catalog is a packaging `INTERNAL`, not successful `[]`. |
| `list_catalog_variants` | `vendor`, `model` | `CatalogVariantSummary[]` sorted by variant then printer variant | Wire + `VALIDATION | NOT_FOUND` | Missing exact vendor/model pair is `NOT_FOUND`; empty strings are `VALIDATION`. |
| `preview_profile` | `catalogRef` | `PrinterProfile` | Wire + `VALIDATION | NOT_FOUND` | Malformed identity is `VALIDATION`; a well-formed unresolved identity is `NOT_FOUND`. |
| `catalog_info` | none (`NoArgsRequest`) | `{ generatedAt: string; sourceTag: string; modelCount: number; variantCount: number }` | Wire | This read has no successful no-op variant. |
| `set_printer_connection` | `id`, `expectedRevision`, `submission: { kind: string; host: string; port: number; useTls: boolean; credential?: string }` | `PrinterMutationResult` | Edit + `CREDENTIAL_UNAVAILABLE | UNSUPPORTED_ADAPTER` | A non-empty credential receives `farm3d/credential/<uuid-v4>`, independent of `id`. Credential-store failure occurs before commit. Supervisor start failure after commit is a safe warning in the success result. |
| `clear_printer_connection` | `id`, `expectedRevision` | `PrinterMutationResult` | Edit | An already-clear Connection is `CONFLICT`, not a revision-incrementing no-op. Cleanup after commit is represented by a warning. |
| `test_printer_connection` | `id`, `submission: { kind: string; host: string; port: number; useTls: boolean; credential?: string }` | `ProbeResult { kind, hostSoftware, firmware, reportedName, state, stateMessage, reported: ReportedCapabilities }` | Probe | The command never persists. Omitted `credential` loads the Printer's committed reference and credential; empty probes without one; non-empty uses only the submitted transient value. Post-bootstrap lookup may return `PERSISTENCE_UNAVAILABLE` or blocking database `CORRUPT_DATA`. Adapter/network error details contain only `entityId` and `adapterKind`, never host, URL, response text, reference, or credential. |
| `credential_store_info` | none (`NoArgsRequest`) | `{ kind: "keychain" | "fallbackFile" | "unavailable"; reasonCode?: string }` | Wire | Store unavailability is successful typed state; `reasonCode` is allowlisted and contains no platform error text or path. |
| `discover_printers` | none (`NoArgsRequest`) | `DiscoveredPrinter[]` sorted by kind, host, then port; each item is `{ kind, name, host, port, addresses: string[] }` | Wire + `TIMEOUT` | Normal expiry of the scan window, including no candidates, is successful. `TIMEOUT` means the worker exceeded its hard completion guard; worker failure is `INTERNAL` with `correlationId`. |
| `printer_statuses` | none (`NoArgsRequest`) | `PrinterStatusBackfill` | Wire | This is authoritative only for its stream/cursor and currently durable Printer IDs. |

All reads described as “none” use `NoArgsRequest`; there is no bare, unversioned
invoke. All errors not listed for a command are contract violations and map at
the command boundary to generic `INTERNAL` with a correlation ID. A failure
after a durable mutation commits must return its authoritative success plus a
safe warning, never an error that invites the caller to repeat an already
committed write. No final F1 command has a successful `data: null` result;
future commands with no domain result must use that envelope form rather than
inventing an empty object.

## Versioned Printer-status events

The only F1 frontend event name is `farm3d-event-v1`. Printer status uses:

```ts
type PrinterStatusEvent = EventEnvelope<
  "printer.status.changed" | "printer.status.removed",
  { type: "changed"; status: PrinterStatus }
  | { type: "removed" }
>;

type EventEnvelope<T extends string, P> = {
  contractVersion: 1;
  streamId: string;
  sequence: number;
  eventId: string;
  occurredAt: string;
  type: T;
  subject: { kind: "printer"; id: string };
  payload: P;
};

type PrinterStatusBackfill = {
  streamId: string;
  snapshotSequence: number;
  statuses: Array<{ printerId: string; status: PrinterStatus }>;
  // Recoverable cache failures; status remains authoritative and non-error.
  cacheWarnings: Array<{ printerId?: string; operation: "hydrate" | "save" | "delete" }>;
};
```

One backend process creates one stream UUID and starts its safe-integer sequence
at `0`. Under the same status-map mutex, publication updates the status, adds
one to the sequence, and captures the envelope. Emit occurs after unlocking.
Backfill clones the status map and current sequence under that same mutex, sorts
rows by `printerId`, and returns that sequence as `snapshotSequence`. Sequence
numbers reset only when a new backend stream starts and may not exceed
JavaScript's maximum safe integer; reaching that guard creates a new stream ID
under the lock before publishing.

`eventId` is unique per publication. The stream is in memory and is not an
immutable event history. Backend restart creates a new stream ID and rebuilds
statuses through supervisor restoration.

### Frontend reconciliation

The status store follows this exact lifecycle:

1. Load durable Printers.
2. Start registering `farm3d-event-v1`.
3. Once listening, buffer matching status events without applying them.
4. Invoke `printer_statuses`.
5. Apply the snapshot only to currently loaded Printer IDs and remove status
   entries for IDs absent from durable Printers.
6. Replay buffered events with the same `streamId` and `sequence` greater than
   `snapshotSequence`, in ascending sequence order. Discard buffered events for
   Printer IDs absent from the current durable Printer records.
7. Deduplicate by `eventId`; also discard a sequence at or below the last
   applied sequence.
8. Apply contiguous later events directly only when their Printer ID still
   exists in the current durable Printer records; otherwise discard them.

The store keeps the newest 4,096 event IDs for the current stream. An event from
a different stream, or a gap greater than one in the current stream, is buffered
and triggers a new backfill instead of being applied over an uncertain base.
Successful resynchronization discards old-stream events and events at or below
the new snapshot cursor.

Each backfill trigger increments a monotonically increasing request generation
and captures both that generation and the currently observed stream ID. At most
one generation is active: a newer trigger marks every older in-flight request
stale, even if cancellation cannot stop its Rust work. A response may replace
state only when its generation is still active and either no stream has yet
been accepted or its `streamId` matches the stream captured for that generation.
Accepting a response establishes its `streamId` as active before replay. A
different-stream event received while a request is in flight increments the
generation and starts a new request; the older response is ignored when it
arrives. If a latest-generation response itself has a stream ID different from
the captured non-null stream, the store does not apply it; it increments the
generation and requests backfill again using the returned stream as the new
hint. Failure/retry timers also carry the generation and do nothing when stale.
This prevents a slow response from overwriting newer reconciliation.

While backfill is failing, the existing statuses remain visible but marked
stale. The listener stays active and buffers at most 1,024 events. The store
retries backfill after 1, 2, 4, 8, 16, then 30 seconds while mounted. Buffer
overflow clears the buffer, keeps state stale, and forces another full
backfill; it never applies an arbitrary tail. A retryable structured error is
shown without unregistering the listener.

Disposal sets a flag before awaiting listener registration. If registration
finishes after disposal, the returned unlisten function runs immediately and no
backfill starts. Otherwise disposal cancels retry timers, clears buffers, and
unlistens exactly once.

## Editable Settings import/export

The Settings document is UTF-8 JSON:

```json
{
  "schemaVersion": 1,
  "exportedAt": "2026-09-17T00:00:00Z",
  "settings": {
    "themeMode": "farm3d-dark"
  }
}
```

Only the listed top-level keys and Settings keys are accepted in schema 1.
`themeMode` must be a non-empty string. Import ignores `exportedAt` for state
ordering. `SettingsImportRequest.expectedRevision` is the Settings revision the
frontend captured before invoking the command and before the command opens its
dialog. After parsing and creating a validated pre-import snapshot, the command
starts `BEGIN IMMEDIATE`, rechecks that exact revision, and only then updates the
singleton with one revision increment. A mismatch returns `CONFLICT` and leaves
the concurrent Settings row untouched. A `schemaVersion` greater than 1 returns
`UNSUPPORTED_SCHEMA_VERSION`; malformed, missing, or non-positive versions are
`VALIDATION`.

## Editable Printers import/export

The Printers document is UTF-8 JSON:

```json
{
  "schemaVersion": 1,
  "exportedAt": "2026-09-17T00:00:00Z",
  "printers": [
    {
      "id": "legacy/Bay:A Printer α",
      "revision": 1,
      "name": "Bay 1",
      "catalogRef": {
        "vendor": "Example",
        "model": "Example Printer",
        "variant": "Example Printer 0.4 nozzle",
        "modelId": "Example-1",
        "printerVariant": "0.4"
      },
      "notes": "",
      "overrides": {},
      "lastKnownGood": null,
      "connection": {
        "kind": "moonraker",
        "host": "printer.invalid",
        "port": 7125,
        "useTls": false,
        "credentialRef": "farm3d/credential/6ba7b810-9dad-4f83-a131-2a6f44cbbf89"
      }
    }
  ]
}
```

The document replaces the complete Printer set. Validation occurs before the
safety snapshot or supervisor changes. Imported IDs use the exact F0-compatible
rule from “IDs and revisions”: 1–512 UTF-8 bytes with no character in Unicode
general category `Cc`. Validation preserves accepted bytes exactly, including
spaces, non-ASCII text, and separators. It rejects duplicate or invalid IDs,
non-positive revisions, malformed Profiles or Connections, invalid port ranges,
credential-value fields, malformed credential references, non-object override
values, and invalid schema versions. A `schemaVersion` greater than 1 returns
`UNSUPPORTED_SCHEMA_VERSION`; a missing, zero, negative, or non-integer version
is `VALIDATION`. Limits are 32 MiB and 10,000 Printer rows; Settings documents
are limited to 1 MiB.

Schema 1 has no `group` field. Unlike the one-time F0 migration rule, an
editable import carrying `group` is not legacy input and is rejected as an
unknown Printer field. Monitor Section grouping remains an unpersisted frontend
view preference. The recursive credential-name scan defined for F0 also applies
to every arbitrary or unknown JSON value in editable imports before any
snapshot or write.

Unknown override fields are retained as recursive JSON values. Unknown,
non-empty Connection kinds are retained without an adapter allowlist. An
unresolved but structurally valid `catalogRef` is imported and later resolves
to the existing missing/rematched catalog status; import does not rewrite or
drop it. Recursive numeric validation applies the cross-tier `JsonValue` subset
before the snapshot or any write. An unsupported number anywhere in an unknown
extension returns `VALIDATION` with its field path and is never rounded.
Supported unknown values retain the existing byte- or semantic-preservation
behavior through export and re-import.

A credential reference must be no more than 512 UTF-8 bytes, contain no
character in Unicode general category `Cc`, and use one of these exact forms:

```text
farm3d/credential/<uuid-v4>
farm3d/printer/<opaque-owner>/apikey
```

The first form requires one canonical lowercase hyphenated UUID v4 segment and
is the only form generated by F1. In the legacy form, `<opaque-owner>` is the
non-empty string between the exact prefix and suffix. It may contain separators,
including `/`, and need not equal the imported Printer ID. Import preserves the
entire accepted reference exactly. This permits an exported migrated Printer to
round-trip a shared or mismatched legacy reference unchanged. Import never
checks for or requests the credential value. Empty references, control
characters, oversized references, malformed prefixes or suffixes, and a
current-form UUID with a non-v4 value or non-canonical spelling produce
`VALIDATION`. A missing local credential does not roll back valid Printer data;
after commit its supervisor reports
`CREDENTIAL_REQUIRED` until the user re-enters the credential. Cleanup retains
the reachability-before-delete rule for both accepted forms, including shared
references.

`PrintersImportRequest.expectedRevisions` is the complete Printer ID/revision
set the frontend captured before invoking the command and before the command
opens its dialog. The request sorts IDs by exact UTF-8 bytes and contains each
ID exactly once; an unsorted, duplicate, or otherwise malformed precondition is
`VALIDATION`.

After a valid snapshot, one `BEGIN IMMEDIATE` transaction first loads and sorts
the complete committed Printer ID/revision set and compares it with the request.
Only an exact match permits the transaction to replace all Printer rows, apply
the revision rules above, and upsert references removed by the replacement into
`pending_credential_cleanup` with reason `import_orphan` and the precedence rule
above. Any concurrent addition, update, or deletion returns `CONFLICT` and rolls
back without an import write. An existing automatic cleanup reason remains
automatic.
Credential values are deliberately not deleted during import; preserving an
orphan is safer than making rollback or re-import irreversible. Later explicit
cleanup owns deletion.

After commit, under the Connection-manager reconciliation mutex, stop and await
every old supervisor, then start supervisors from the committed Printer rows.
Credential lookup happens only for rows carrying a reference. A crash after
commit but before reconciliation is repaired by normal startup. A reconciliation
failure cannot roll back committed rows; the applied result includes safe
per-Printer warning codes and the status store exposes the error state.

## Dialog ownership, cancellation, and export writes

The Rust import/export commands own native open/save dialogs through
`tauri-plugin-dialog`. Frontend stores do not pass arbitrary paths and do not
read or write files. Dialog services are injected in command tests.

- `import_settings` and `import_printers` open one-file dialogs filtered to
  `.json`.
- `export_settings` suggests `farm3d-settings.json`.
- `export_printers` suggests `farm3d-printers.json`.
- Closing a dialog returns `{ status: "cancelled" }` with no snapshot, file
  access, revision change, or supervisor change.
- Applied Settings import returns the authoritative `settings` record and
  `warnings`; applied Printers import returns authoritative `printers`, exact
  created/updated/deleted counts, and `warnings`.
- Export returns `{ status: "exported", exportedAt, recordCount }`; it does not
  return the selected path.
- A frontend wrapper running without Tauri returns
  `{ status: "unsupported", reason: "desktopRequired" }` without invoking IPC.

The generated import/export result union includes the `cancelled`, `applied`,
`exported`, and `unsupported` variants. Each command's result type includes only
the variants it can produce, plus `unsupported` for parity with the frontend
wrapper; production Rust commands never return `unsupported` because they run
only under Tauri.

Export first serializes a complete document in memory. It writes a random
same-directory temporary file, syncs it, atomically replaces the selected file
with the platform replace primitive, and syncs the directory where supported.
Failure leaves the previous destination intact and cleans the temporary file on
a best-effort basis. Export never writes directly to the destination and never
includes credential values. Under `just web`, all four operations return an
explicit unsupported result before pretending to open or change a file.

## Navigation identity

The generated contract is:

```ts
type NavigationTarget = {
  version: 1;
  destination: "monitor" | "queue" | "library" | "spools" | "settings";
  selection?: {
    kind: "printer" | "job" | "model" | "project" | "spool" | "incident" | "attention";
    id: string;
  };
};
```

Valid combinations are:

| Destination | Selection kinds |
|---|---|
| `monitor` | none, `printer`, `incident`, `attention` |
| `queue` | none, `job` |
| `library` | none, `model`, `project` |
| `spools` | none, `spool` |
| `settings` | none |

For `selection.kind: "printer"`, `id` uses the exact Printer ID rule: 1–512
UTF-8 bytes with no character in Unicode general category `Cc`. Every other
selection ID remains an opaque, non-empty UTF-8 string up to 256 bytes. The
canonical internal serialization is a URL fragment:

```text
#nav=v1/<destination>
#nav=v1/<destination>/<selection-kind>/<percent-encoded-id>
```

Serialization encodes the ID's UTF-8 bytes as one path segment. It leaves only
RFC 3986 unreserved bytes (`ALPHA`, `DIGIT`, `-`, `.`, `_`, and `~`) literal and
uses uppercase percent escapes for every other byte, including `/`, `\\`, `:`,
spaces, and every byte of non-ASCII text. Parsing splits the structural segments
before decoding the ID exactly once, requires valid UTF-8, applies the limit for
the parsed selection kind, and never normalizes or rewrites the decoded string.
This makes `parse(serialize(target))` preserve every accepted Printer ID
byte-for-byte through its full 512-byte limit. Parsing rejects unknown versions,
extra segments, malformed percent encoding, empty or oversized IDs, control
characters in Printer IDs, and invalid destination/selection combinations; it
never partially accepts them. Serialization always emits the canonical form.
F1 wires `monitor` to the current Printers surface and `library` to the current
Library surface. The store can parse and retain the other valid targets but
reports `destinationUnavailable` until their owning phases add screens.

If a destination exists but the selected object does not, navigation keeps the
attempted target, opens the destination without selecting another object, and
reports `selectionUnavailable`. It never silently selects the first row or
converts an ID to a name. OS protocol registration and externally opening these
fragments remain out of F1.

## Startup order

The Tauri builder registers all 23 command handlers unconditionally. During
setup it also registers one managed bootstrap state with `Starting`, `Ready`,
and `Failed(CommandError)` variants. Handlers are therefore callable in the
recovery UI even when storage cannot open; they do not assume that repositories
or the Connection manager exist. Setup captures a blocking bootstrap
`CommandError` and still launches the recovery UI instead of terminating the
application. The fatal unsupported-metadata-locking setup error is the sole
exception: it is outside `CommandError`, prevents the normal application and
recovery UI from launching, presents the safe fatal message, and exits.

Bootstrap then performs these steps in order:

1. Resolve and validate `StoragePaths`; create roots with user-only permissions
   where the platform supports them.
2. Acquire the exclusive metadata-root ownership lock. Do not open SQLite or a
   credential service before this succeeds. Contention or transient failure
   enters `Failed(PERSISTENCE_UNAVAILABLE)`; an unavailable lock primitive takes
   the fatal setup path and stops startup.
3. Remove incomplete or invalid restore-staging directories; leave valid staged
   candidates untouched for P9.
4. Open SQLite and apply embedded schema migrations.
5. Complete or resume F0 import and legacy archival.
6. Load and validate the bundled derived Printer catalog.
7. Construct Storage, catalog, repositories, and command services, but do not
   publish them as ready.
8. Retry the automatically eligible `pending_credential_cleanup` reasons under
   the credential coordinator; retain `import_orphan` rows.
9. Construct the Connection manager and restore supervisors from committed
   Printer rows, resolving credentials only when a row has a reference.
10. Atomically replace `Starting` with `Ready` and allow domain operations and
   status-event traffic.

A blocking ownership, storage, F0-import, or catalog failure in steps 1–8 stops
the sequence before supervisor restoration starts or managed state becomes
`Ready`. Failure to acquire ownership stops before any SQLite or credential-store
access. Unsupported metadata locking terminates through the setup path described
above and is never stored in `Failed`. Individual supervisor restoration
failures in step 9 become safe status and warning state rather than a
failed-storage bootstrap result. Command-visible bootstrap stores one safe
structured `PERSISTENCE_UNAVAILABLE`, startup `CORRUPT_DATA`, `MIGRATION_FAILED`,
`UNSUPPORTED_SCHEMA_VERSION`, or generic `INTERNAL` error in `Failed`. Every
valid-version command consults this state before domain work, so no command
performs a partial domain operation. A non-retryable failure is returned
unchanged by every command for the life of the process. For a retryable
failure, the next command invocation serializes one complete bootstrap retry;
if the retry fails, every waiting command receives the newly captured error,
and if it succeeds, the triggering command continues against `Ready` state.
Bootstrap never reads legacy JSON as live-state fallback. The recovery UI
remains outside domain managed state and can render these errors without
Settings, Printers, catalog, or supervisor data.

## Security boundaries

In this design, a **credential value** is data received through a designated
secret-bearing field, currently `ConnectionSubmission.credential`, or through a
credential-store API. farm3d must never copy or derive that value into a general
data path. The value is excluded from:

- SQLite tables, WAL, restore staging, and safety snapshots;
- legacy staging and archives other than unchanged Printer credential
  references;
- Settings and Printers exports;
- command successes and errors, event envelopes, backfill, generated output
  fixtures, persistent or reactive frontend stores, caches, logs, and warnings;
  and
- large-content storage.

The value may exist transiently in form-local/request memory, Tauri IPC
serialization, adapter probe memory, and the credential-store implementation.
After the operation settles, a `finally` block clears frontend form state and
releases farm3d's request references. Where the backend owns a mutable Rust
buffer containing the value, its secret wrapper zeroizes that buffer when its
use ends. The application does not retain or capture the value in success or
error objects, closures, stores, logs, or test output.
Immutable JavaScript strings and copies owned by browser controls, the WebView,
Tauri IPC, serializers, the operating system, adapters, or credential-store
frameworks cannot be guaranteed zeroized. Generated TypeScript may define the
input-only field; generated fixtures must not contain a concrete submitted
value.

Arbitrary user-authored notes and extension strings cannot be semantically
classified as secret-free. Import therefore recursively rejects the
credential-bearing key names defined in the F0 import rules, but it does not
inspect or classify values under otherwise allowed keys. This limitation does
not weaken the rule above: farm3d must not route, copy, or derive an actual
submitted credential value into those strings or any other general path.

F1 migration never reads the credential fixture. Verification submits a unique
synthetic marker through the designated credential field and asserts that the
submitted value appears only in the credential-store fixture and
credential-store test path, never in database pages, snapshots, archives,
exports, retained IPC captures, errors, events, logs, generated output fixtures,
or generated contracts. This seeded-marker scan verifies handling of the known
submitted value; it does not prove that arbitrary free-text or extension
strings are secret-free.

File dialogs are limited to the four explicit commands. No general filesystem
read/write command is introduced. Import errors identify a safe field path and
code, not rejected content. This includes unsupported `JsonValue` numbers: the
error may identify their field path but never includes the numeric token or
surrounding source content. Archive and export results expose basenames or
counts, not full local paths. SQLite parameters are bound; imported strings are
never interpolated into SQL.

## Verification and exit criteria

| Decision or invariant | Required evidence |
|---|---|
| Metadata-root ownership | Two-process tests prove the first process holds one root for its lifetime and the second enters retryable `Failed(PERSISTENCE_UNAVAILABLE)` without opening SQLite or the fallback credential file; retry succeeds only after normal owner exit or simulated crash releases the OS lock; an existing unlocked lock file is accepted; an unavailable advisory-lock primitive fails before SQLite or credential access through the fatal unsupported-metadata-locking setup path, with no command recovery UI or retry action |
| Schema and connection policy | Tests for creation, migration idempotency/checksums, future version, foreign keys, serialized concurrent writers, read snapshots, rollback, and reopen after an uncommitted transaction; SQLite contention from accidental non-farm3d access still returns `PERSISTENCE_UNAVAILABLE`; migration execution failure and checksum mismatch return non-retryable `MIGRATION_FAILED` with `recovery: []` and safe support guidance |
| F0 import | Canonical fixtures preserve Settings, both Printers, IDs, Profile data, supported unknown override fields and their existing byte/semantic guarantees, free-form Connection kinds, and structurally valid credential references; unsupported integers outside JavaScript's safe range, non-finite values, and decimals that do not round-trip through a JavaScript number in unknown extensions block import with `CORRUPT_DATA` rather than rounding; fixture `group` values are omitted with one deduplicated `LEGACY_MONITOR_GROUP_IGNORED` warning; matrix covers exact-byte Printer IDs with spaces, non-ASCII text, and separators, the empty/control/over-512-byte ID rejections, shared and mismatched opaque-owner legacy references including owner separators, malformed references, recursive credential-name rejection, missing/corrupt/future/changed sources, and every crash boundary |
| Archives | Tests pin exact names, collision suffixes, resumed archival, changed-after-commit handling, and absent-then-appearing inputs |
| Snapshots | Concurrent-write online backup test; schema/integrity/foreign-key rejection; five-file retention; validated restore staging, interrupted staging cleanup, and proof that F1 never replaces the active database or handles its sidecars |
| Revisions and IDs | Stale-revision conflicts, exactly-once increments, imported revision rules, Settings-import revision and complete sorted Printer-set preconditions rechecked inside the final post-snapshot `BEGIN IMMEDIATE`, concurrent additions/updates/deletions rejected without overwrite, exact-byte F0/editable-import ID preservation and bounds, generated UUID format, and rapid creation uniqueness |
| Credentials | Failure matrix for provisional write, secret-write failure, database failure, cleanup failure/retry, explicit clear/delete/import orphans, every `pending_credential_cleanup` precedence transition (including import-orphan promotion, automatic-reason non-demotion, and deterministic automatic-to-automatic conflicts), reachability gating after each transition, canonical `farm3d/credential/<uuid-v4>` generation independent of Printer ID, shared imported-reference reachability, mutation omission preserving the reference without lookup, probe omission loading the stored reference and credential, explicit-empty probe behavior, form clearing and application-reference release in `finally`, application-owned mutable-buffer zeroization where available, explicit acknowledgment that immutable JavaScript and framework/runtime copies cannot be zeroized, concurrent fallback writes, interrupted replacement, permissions, and submitted-marker scans |
| Contracts | Deterministic ts-rs 12 export test for every wire type and enum; recursive `JsonValue` fixtures prove finite JavaScript-number round trips, both safe-integer boundaries, rejection immediately outside those boundaries, rejection of non-finite values and precision-changing decimals, and preservation of supported unknown values; CI regeneration leaves no diff; no direct `invoke` outside `src/ipc/client.ts` |
| Commands | Registration test compares the exact 23-name list; each handler returns the captured managed-state error during failed bootstrap; request/result fixtures cover every post-ready matrix row, both editable imports returning `UNSUPPORTED_SCHEMA_VERSION` with [`UPGRADE_FARM3D`], selected-import `CORRUPT_DATA` with [`EDIT_FIELDS`], legacy/database `CORRUPT_DATA` with `[]` and safe source metadata, Probe `PERSISTENCE_UNAVAILABLE` and `CORRUPT_DATA`, all expected-revision conflicts, authoritative post-commit results, and every `ErrorCode`/`RecoveryCode`/`correlationId` serialization rule; delete tests prove commit plus cleanup intent precedes supervisor shutdown, graceful stop is awaited, a failed stop aborts and awaits the task, live status is removed, the authoritative result carries `SUPERVISOR_RECONCILIATION_FAILED`, no later event comes from the deleted supervisor, and startup after each crash boundary restores no deleted supervisor |
| Events | Exact envelope/backfill serialization; same-lock cursor test; event-during-backfill, stale response generation, duplicate, gap, stream restart, absent-Printer buffered/live discard, failure retry, overflow, and late-unlisten tests |
| Import/export | Separate format round trips with no Printer grouping field; exact-byte F0-compatible Printer IDs; migrated legacy and canonical current credential-reference round trips, including shared and mismatched owners with separators; cancellation; size/schema/duplicate/catalog/reference/recursive credential-name validation; unsupported numbers in unknown extensions return `VALIDATION` with the exact safe field path and no rounded write, while supported unknown values retain their existing byte/semantic guarantees; snapshot-before-write; concurrent Settings mutation and each Printer-set addition/update/deletion between snapshot and final transaction return `CONFLICT` without overwrite; rollback; atomic export replacement; committed supervisor reconciliation; explicit web unsupported outcomes |
| Navigation | Round trips for every valid combination and rejection of every invalid combination; byte-for-byte percent-encoded round trips for accepted Printer IDs at the 512-byte boundary, including spaces, non-ASCII text, and every allowed separator; 513-byte and control-character rejection; unavailable destination/selection identity preservation; and no OS registration |
| Vertical tracer | Start from F0 fixtures, migrate, invoke an enveloped Printer mutation, restart, prove persistence, reconcile an event racing backfill exactly once, and exercise both import/export domains through mock IPC |
| Security | A marker submitted through the designated credential field is absent from SQLite, WAL, snapshots, staging, archives, exports, retained IPC captures, errors, events, logs, generated output fixtures, and generated contracts; recursive key-name rejection is tested separately; unsupported-number errors expose only a safe field path and never the rejected token or source content; no scan claims arbitrary free-text or extension strings are secret-free, and no claim extends to diagnostics because F1 has none |

Final F1 validation runs `source "$HOME/.cargo/env" && just gen-contracts`,
`just build`, `just test`,
`source "$HOME/.cargo/env" && just test-rust`, and
`source "$HOME/.cargo/env" && just package`, followed by package inventory,
`git diff --check`, and the security scans. Desktop verification covers the
existing Printer, Connection, status, and four import/export flows. Any absent
live Printer or unsupported OS evidence is recorded as absent rather than
inferred from fixtures.

The user delegated approval authority, and approval of this design and ADR-0008
was granted in this conversation on 2026-09-17 before production
implementation, as required. This does not claim approval by any external
stakeholder. The `JsonValue` numeric clarification above is part of that same
user-delegated approval, not a claim of separate or external approval, and Task
2 may begin. F1 exits only when all evidence rows above pass or have an approved
documented exception, the final command inventory is exactly 23, and no later-
domain schema or deferred UI has entered the change.
