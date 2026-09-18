# Use SQLite for transactional persistence

**Status:** Approved

> [!NOTE]
> The user delegated approval authority, and approval of this ADR and the
> focused F1 design was granted in this conversation on 2026-09-17. This does
> not claim approval by any external stakeholder. The delegated approval
> satisfies the requirement that approval precede production implementation,
> so Task 2 may begin.

## Context

F0 stores Settings and Printers in separate JSON files. Each mutation rewrites
one whole file, writes are not atomic, there is no general schema-migration
mechanism, and coordination across Printer state and credential cleanup is
recoverable only through operation-specific ordering.

Later phases require invariants across records. Loading one Spool into a
Material Slot may need to displace it from another location atomically. Creating
a Job will need to assign a Printer and reserve one or more Spools without
exposing a half-applied state. This ADR does not define Spool, Material Slot,
Queue Entry, Job, reservation, or history schemas; it selects the persistence
engine those phases can use when they define their own models.

farm3d remains a single-user local desktop app. Credentials remain in the
platform keychain or the owner-only fallback credential file and are referenced
from general persistence only by opaque ID.

## Decision

Use bundled SQLite through `rusqlite` as the authoritative transactional store
for farm3d metadata.

Only one farm3d process may own a metadata root at a time. Before opening
SQLite or either credential-store implementation, startup acquires a
non-blocking exclusive advisory lock on a fixed lock file under that root and
holds the lock for the process lifetime. Contention from another farm3d process,
or another transient lock-acquisition failure, is a retryable structured
bootstrap failure; that process does not open SQLite or the fallback credential
file. If the platform or backing filesystem cannot provide the advisory-lock
primitive, setup fails closed before storage or credential access and exits
through a fatal unsupported-platform path rather than offering an ineffective
command retry. The operating system releases the lock when the process exits or
crashes; the lock file may remain and is not itself evidence of a live owner.

F1 stores Settings and Printers in `farm3d.sqlite3`, applies ordered embedded
schema migrations, serializes writes with one application writer lock and
`BEGIN IMMEDIATE`, enables foreign keys and WAL, and uses `synchronous = FULL`.
Reads use separate read-only connections and SQLite snapshots. Consistent
safety snapshots use SQLite's online backup API rather than copying a live
database and WAL files.

The F0 JSON files are one-time migration inputs and become non-authoritative
archives. Editable Settings and Printers JSON documents remain explicit
import/export formats, not live database files. Each import carries the
revision precondition captured before its file dialog. After creating the
safety snapshot, its final `BEGIN IMMEDIATE` transaction rechecks that
precondition before replacing data, so a concurrent mutation wins rather than
being overwritten.

Model Source Revision and Slice Revision content will use a separate
application-data root. SQLite will eventually hold metadata and references to
those entities and their content, but F1 defines no later-domain tables.
Values submitted through credential-bearing fields or credential-store APIs
are excluded from SQLite, snapshots, exports, events, errors, and logs.
Arbitrary user-authored text is not treated as a credential value merely
because it happens to contain the same characters.

## Options considered

### Continue using direct JSON files

This is the smallest change for today's Settings and Printers. The documents
are easy to inspect and edit.

It does not provide atomic writes across domains, foreign keys, compare-and-set
revisions, ordered schema migrations, concurrent read snapshots, or a safe
consistent snapshot while writes continue. Implementing future Spool movement
or Job/Spool reservation would require a journal and recovery protocol outside
the files themselves. Direct JSON remains suitable for the explicit editable
import/export boundary, not authoritative live state.

### Build a custom journaled document store

A versioned manifest, append-only journal, atomic rename protocol, checksums,
compaction, and crash replay could preserve human-readable documents while
supporting multi-document commits.

That approach would make farm3d responsible for a database's hardest parts:
write serialization, torn-write recovery, journal replay, checksummed schema
migration, snapshot isolation, compaction, cross-document constraints, and
backup consistency on every supported filesystem. The implementation and test
surface would grow again when Spool location and Job/Spool reservation need
atomic compare-and-update behavior. The product has no requirement that live
storage remain hand-editable, because F1 provides separate import/export
documents.

### Use SQLite

SQLite provides local transactions, rollback, constraints, indexed lookup,
snapshot reads, mature crash recovery, and an online backup API in one embedded
file. It fits the single-user local architecture and can atomically support
future Spool movement and Job/Spool reservations without requiring those
schemas now.

The costs are an additional Rust dependency, explicit migrations, SQL/repository
code, and loss of direct editing of authoritative files. WAL also means the
database cannot be backed up safely by copying only the main file. The focused
design addresses those costs with bundled SQLite, embedded migration checksums,
narrow repositories, and validated online snapshots plus separate editable
exports.

## Consequences

- Rust repositories become the only route to durable Settings and Printer
  state; frontend caches settle from command results or authoritative events.
- Multi-record changes can be committed or rolled back as one unit, and later
  phases may add their own tables and constraints through reviewed migrations.
- Future Spool movement and Job/Spool reservation can share a transaction with
  their affected records. This is capability, not approval of their schemas.
- Schema migrations are immutable, ordered, checksummed, and tested at crash
  boundaries. A newer database is rejected rather than downgraded. A migration
  failure or checksum mismatch requires safe support investigation; restarting
  is not advertised as corrective.
- Live database files are not user-editable. Settings and Printers each have a
  versioned JSON import/export format.
- Backup code must use SQLite's online backup API or a validated equivalent;
  copying a live `.sqlite3` file without its WAL is forbidden.
- Credential updates remain cross-store operations. They prefer temporary
  credential orphans over committed dangling references and use durable cleanup
  references, never values received through credential-bearing fields or the
  credential-store API.
- Printer deletion commits the row deletion and credential-cleanup intent
  before supervisor reconciliation. A post-commit supervisor failure cannot
  undo the deletion or turn its authoritative result into an error; farm3d
  reports a warning, forcibly terminates any task that did not stop cleanly,
  removes live status, and lets startup reconcile committed state after a crash.
- Cross-store coordination depends on the lifetime-held farm3d ownership lock.
  SQLite locking still protects the database from accidental access by a
  non-farm3d process, but it cannot by itself coordinate SQLite transactions
  with the separate credential store.
- F1 may validate and stage a restore candidate, but it does not install or
  swap a database. P9 owns sidecar-safe replacement, rollback, and full
  backup/restore. Diagnostics, immutable history, and all later-domain schemas
  remain deferred to their owning phases.

## Approval gate

The user delegated approval authority, and approval of ADR-0008 and
`docs/superpowers/specs/2026-09-17-f1-foundation-design.md` was granted in this
conversation on 2026-09-17. This does not claim approval by any external
stakeholder. Approval was granted before production implementation, as
required, and Task 2 may begin.
