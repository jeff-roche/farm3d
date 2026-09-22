# P2 Printer Lifecycle and Batch Setup Design

## Status

Approved focused design for GitHub issue #12. The user approved this design
in conversation on 2026-09-22, including the decisions this document proposed
beyond the four listed below. This document
narrows the approved complete-v1 interaction design
(`2026-09-16-complete-v1-ui-workflows-design.md`) to the work owned by P2 and
resolves the decision gate recorded in the phase plan
(`2026-09-16-complete-v1-implementation-approach.md`, P2 section).

The user decided four questions on 2026-09-22, and this document treats them
as fixed:

1. How incomplete batch rows are stored (Printer records, not drafts).
2. That deletion requires archiving first.
3. That location is a single free-text label.
4. That duplicate hosts are blocked.

## Goal

Deliver durable Printer setup: one Printer at a time or a batch of physically
identical Printers across locations. Both paths can save a Printer without a
Connection (Profile-only) or with a tested Connection, retain failed input for
retry, and apply shared setup by copying it at creation. After creation, a
Printer can be relocated, given a start-safety rule, archived without losing
identity, and deleted only through a guarded path.

## Scope

### In scope

- Single Printer setup: Identify, Connect, Operate, and Review.
- Batch Printer setup: shared setup, row generation, paste/CSV intake,
  discovery mapping, row selection, explicit shared-field application,
  bounded probing, per-row results, cancellation, and retry.
- Connection testing that neither saves nor starts supervision.
- Durable `location`, `startSafety`, and `archivedAt` on Printers.
- Setup-incomplete derivation owned by Rust.
- Canonical host identity and duplicate-host rejection.
- Connection replacement that validates before discarding a working
  configuration.
- Archive, unarchive, and archived-only deletion behind one eligibility
  interface that later phases extend.
- The Setup tab of the detail dock gains location, safety, Connection
  replacement, archive, and guarded deletion.
- Monitor: a Location section, an Archived filter, and location search.
- Printers export/import schema v2.

### Non-goals

- Material Slot layout, loaded Spools, and the Equip step (P3).
- Camera templates, camera steps, and alert defaults (P8).
- Enforcing the start-safety rule at dispatch (P7). P2 stores it and exposes
  it only.
- Job, reservation, Spool, or Incident delete blockers (P3/P7/P8). P2 claims
  no final cross-domain delete integrity.
- A permanent shared-configuration or batch entity.
- Later bulk editing of existing Printers.
- New adapters. Moonraker stays the only supported adapter kind, and
  discovered OctoPrint candidates show as unsupported.
- New Printer Profile fields. Only existing catalog-backed override fields
  are used.

## Product vocabulary

These terms are added to `CONTEXT.md`:

- **Location** — an optional free-text label for where a Printer physically
  sits, such as "Bay A" or "Rack 2". It is durable Printer data. Monitor
  Sections may group by it, but it is not a grouping entity itself.
  Batch-setup "bays" are location values; they are not Material Slots.
- **Profile-only Printer** — a Printer saved with a resolved Printer Profile
  and no Connection. It is valid, durable, and shown as Setup incomplete.
- **Setup incomplete** — the derived state of a Printer whose durable
  configuration cannot support monitoring. It differs from Offline.
- **Start-safety rule** — a per-Printer rule for whether farm3d may start a
  Job unattended. The default is to confirm the bed is clear.
- **Archive** — the default way to retire a Printer. It keeps identity and
  history, and it removes the Printer from monitoring and scheduling.

## Decisions

### D1. Failed and incomplete batch rows

Rows with valid identity become real Printer records:

- The name must be valid and the shared catalog reference must resolve.
- If the row's Connection fails validation, probing, or duplicate-host
  checks, the Printer is still created, without a Connection. The row result
  is `createdSetupIncomplete` and carries the row errors.
- The frontend keeps the row's Connection input so a retry can call
  `set_printer_connection` against the created `printerId`. A retry never
  creates a second Printer.

Rows with invalid identity are rejected and not persisted. They stay in the
batch table with machine-readable errors. No drafts table exists.

### D2. Canonical host identity

`canonical_host_identity(host, port) -> String` is built as follows:

1. Trim ASCII whitespace.
2. Strip one pair of enclosing `[`…`]` from an IPv6 literal.
3. Lowercase.
4. Strip one trailing `.`.
5. If the result parses as an IP address, use its canonical text form (IPv6
   compressed, with no zone-id normalisation).
6. Format as `host:port`, re-bracketing IPv6 as `[addr]:port`.

Protocol and TLS are **not** part of the identity. Two adapters on the same
host:port are the same physical endpoint.

The identity is not DNS-resolved. `printer.local` and `192.168.1.20` are
treated as distinct, and discovery mapping flags these cases as ambiguous
when a discovered candidate lists both.

The test vectors live in `src-tauri/tests/fixtures/host-identity.json`, and
both the Rust and TypeScript implementations are tested against that file.

### D3. Duplicate hosts

No two **non-archived** Printers may have the same host identity.

- **Enforcement:** the repository enforces this inside the write transaction,
  and a partial unique index on `printers(host_identity) WHERE archived_at IS
  NULL AND host_identity IS NOT NULL` backs it up.
- **Error:** a violation returns `DUPLICATE_HOST` with
  `details.conflictingPrinterId`.
- **Within a batch:** a later row that duplicates an earlier row gets
  `DUPLICATE_HOST` with `conflictingRowId`.
- **Archived Printers** do not reserve their host. Unarchiving re-checks, and
  if another active Printer now owns the host, unarchive fails with
  `DUPLICATE_HOST`.

### D4. Setup-incomplete derivation

There is one Rust function, `derive_setup_facts(&StoredPrinter,
&ProfileResolution) -> PrinterSetupFacts`.

- `hasUsableConnection` means a Connection exists and its `kind` is a
  supported adapter (today only `moonraker`).
- `profileResolved` means `catalog_status` is `ok` or `rematched`.
- `setupGaps` returns an ordered list, possibly empty, of `missingConnection`,
  `unsupportedAdapter`, and `unresolvedProfile`.

The facts are derived from persisted columns on every read, on every
supervisor start or reconcile, and at restart; they are never stored. Every
current ad hoc construction of `PrinterSetupFacts` in `lib.rs` and the
command modules is replaced with this function. `ResolvedPrinter` exposes
`setupGaps` so the frontend never re-derives them.

A missing credential is **not** a setup gap. It surfaces at runtime as an
authentication error, as it does today.

### D5. Start safety

`startSafety: "confirmBedClear" | "unattended"`, defaulting to
`confirmBedClear`. It is stored per Printer, set during single and batch
setup, and editable in the Setup tab. P2 has no enforcement. P7 reads it as a
separate gate after assignment.

### D6. Archive

`archive_printer(id, expectedRevision)` does the following in order:

1. Sets `archived_at` and bumps the revision in one transaction.
2. Takes the reconciliation guard.
3. Calls `stop_and_wait`.
4. Publishes `printer.status.removed`.

The Connection, credential, overrides, and telemetry snapshot row are kept.
Archived Printers:

- Are not supervised at startup, because `restore_persisted_connections`
  skips them.
- Are excluded from the Monitor's default view.
- Appear under an **Archived** filter.

`unarchive_printer(id, expectedRevision)` clears `archived_at` after the
duplicate-host check, then starts supervision if `hasUsableConnection` holds.
Otherwise it reconciles to Setup incomplete.

### D7. Eligibility and guarded deletion

`printer_lifecycle_eligibility(id) -> LifecycleEligibility` returns:

```text
{ canArchive, canUnarchive, canDelete,
  blockers: [{ action: archive|unarchive|delete, code, message }] }
```

Blockers come from a registry of `LifecycleBlockerSource` implementations
(`fn blockers(&self, printer: &StoredPrinter, tx: &Transaction) ->
Vec<LifecycleBlocker>`). P2 registers one source, which contributes:

- `NOT_ARCHIVED` for `delete` when `archived_at` is null.
- `ALREADY_ARCHIVED` / `NOT_ARCHIVED` for archive and unarchive.

P3, P7, and P8 add their own sources, for loaded Spools, active Jobs,
reservations, and Incidents.

`delete_printer` evaluates eligibility inside its transaction and fails with
`LIFECYCLE_BLOCKED` (details: blockers) if blocked. When allowed, it keeps
the existing order:

1. DB delete plus `printer_deleted` cleanup intent, committed together.
2. Reconciliation guard.
3. `stop_and_wait`.
4. Credential cleanup retry.

The UI requires the user to type the Printer's name before calling delete.

### D8. Connection testing and replacement

- **`probe_connection(submission)`:** a new command with no Printer id. It
  probes the submitted config with the credential given in the submission,
  never reads or writes storage, never touches the credential store, and
  never starts, stops, or publishes supervision. It returns a `ProbeResult`.
- **`test_printer_connection(id, submission)`:** unchanged. It may fall back
  to the stored credential.
- **`set_printer_connection` with a Connection already present:** the backend
  probes the new submission before replacing it. On probe failure the command
  fails with the probe's error code and the stored Connection is untouched,
  unless the request carries `acceptUnverified: true`. The UI offers
  `acceptUnverified` only as an explicit "Save anyway" after showing the
  failure.
- **First-time sets and clears:** these do not probe.
- **Duplicate host:** checked on every set.

### D9. Shared bed setup

The shared bed field is `defaultBedType`, the catalog-backed
`default_bed_type` override that already exists in `PrinterProfileOverrides`:

- The shared step offers the variant's bed types. Each row shows the value it
  will receive.
- At creation, the value is written as an ordinary per-Printer override, and
  only when it differs from the catalog default.
- No batch or shared entity is stored. A later edit to one Printer affects
  only that Printer.
- Bed shape and exclude areas are not offered in batch setup. They remain
  per-Printer overrides in the Profile panel.

### D10. CSV and paste intake

Intake runs in the frontend (`src/printers/batch-intake.ts`) as a pure
parser.

**Input format:**

- The delimiter is auto-detected from the header row: tab, then comma.
- The parser supports quoted fields (`"a, b"`, `""` escapes).
- A header row is required. Header names are case-insensitive and trimmed.

**Columns:**

| Column     | Required | Meaning                                  |
|------------|----------|------------------------------------------|
| `name`     | yes      | Printer name                             |
| `location` | no       | Location label                           |
| `host`     | no       | Connection host (blank = Profile-only)   |
| `port`     | no       | Integer 1–65535, defaults per protocol   |
| `protocol` | no       | Adapter kind, default `moonraker`        |
| `tls`      | no       | `true`/`false`/`yes`/`no`/`1`/`0`         |

**Handling rules:**

- Unknown columns are ignored and reported as one intake warning.
- Credential columns are **never** read. A column named like `apikey`,
  `api_key`, `password`, `token`, or `secret` is rejected with an intake
  error, so secrets do not end up in files or the clipboard history of this
  flow.
- Blank lines are skipped.
- Parse errors are reported per line and never discard other lines.

### D11. Row generation

The user enters a quantity, a naming pattern, and one or more location
labels:

- The pattern contains `{n}` (1-based) or `{nn}` (zero-padded to 2 digits).
- `quantity` means rows **per location**, since that matches "N printers in
  each bay". Rows are generated location by location, and `{n}` keeps
  counting across locations so names stay unique.
- A generated name that collides with an existing Printer or another row is
  flagged as a warning, matching today's add-dialog behaviour. It is not
  blocked.

### D12. Discovery mapping

`discover_printers` is reused unchanged. The frontend maps candidates to rows
by canonical host identity, using the candidate's host and each of its
addresses:

- A candidate whose identity matches an existing non-archived Printer is
  marked **already configured** and is not selectable.
- A candidate matching more than one row, or a row matching more than one
  candidate, is marked **ambiguous** and is never auto-assigned.
- An unsupported kind (`octoprint`) is shown but not assignable.
- Mapping only fills a row's `host`, `port`, and `protocol` after an explicit
  per-row choice or an "Assign in order to selected rows" action.

### D13. Shared credentials

The batch request may carry one `sharedCredential` secret. Each row chooses
its credential source: `none`, `shared`, or `row(value)`.

For each created Printer with a credential, the backend writes the secret
under that Printer's own new `credentialRef`, using the existing
provisional → write → commit ordering. Refs are never shared between
Printers.

Secrets appear in no response, error message, error details, warning,
diagnostic log line, SQLite column, or generated contract. The batch response
reports only `credentialStored: bool` per row.

### D14. Bounded probing, concurrency, and cancellation

`create_printers_batch` runs in three phases:

1. **Validate all rows.** This covers identity, the Connection schema,
   in-batch duplicates, and duplicates against the DB. No I/O happens yet.
2. **Probe.** When `probe: true`, rows with a valid Connection are probed
   with at most **4** in flight (`buffer_unordered(4)`). Each probe uses the
   adapter's existing 10 s `PROBE_TIMEOUT`. The batch as a whole has no extra
   timeout.
3. **Commit.** Each row commits independently, as its own transaction,
   through the same code path as single create. Rows commit as their probes
   finish; they don't wait for the whole batch.

Cancellation works as follows:

- `cancel_printer_batch(batchId)` signals a `watch` channel held in a
  registry keyed by `batchId`.
- In-flight probes are dropped. Rows not yet committed come back as
  `cancelled` and are not persisted, even if their probe had finished.
- Rows already committed stay committed.
- Cancelling an unknown or finished `batchId` is a no-op success.
- `batchId` is generated by the client and must be unique among running
  batches; otherwise the call gets `CONFLICT`.

Probe mismatch between the reported bed size and the profile, using the
existing 1 mm rule, is a row **warning**. It does not block.

Supervision for created connected Printers starts after each row commits,
exactly as it does for single create.

## Backend model

### Migration `0003_p2_printer_lifecycle.sql`

```sql
ALTER TABLE printers ADD COLUMN location TEXT
  CHECK (location IS NULL OR (length(location) BETWEEN 1 AND 128
         AND location = trim(location)));
ALTER TABLE printers ADD COLUMN start_safety TEXT NOT NULL
  DEFAULT 'confirmBedClear'
  CHECK (start_safety IN ('confirmBedClear', 'unattended'));
ALTER TABLE printers ADD COLUMN archived_at TEXT;
ALTER TABLE printers ADD COLUMN host_identity TEXT;
CREATE UNIQUE INDEX printers_active_host_identity
  ON printers(host_identity)
  WHERE archived_at IS NULL AND host_identity IS NOT NULL;
```

**Backfill.** `host_identity` is backfilled by Rust in the migration step
(not SQL), because canonicalisation is Rust-owned. This is the one migration
with a Rust post-step, run in the same exclusive transaction before the index
is created.

**Pre-existing duplicate hosts.** For each duplicate group, the oldest
Printer (by `created_at`, then `id`) keeps its identity. Every other Printer
in the group:

- Is **archived**, with `archived_at` set to the migration time.
- Keeps its Connection.
- Gets a `migration_warnings` row, `DUPLICATE_HOST_ARCHIVED`, that names both
  Printers.

This preserves every Printer's identity and data, removes the ambiguity from
supervision, and the user can resolve it by editing and unarchiving. Nothing
is deleted.

The ledger, checksum, crash-boundary, and v2→v3 upgrade tests follow the
existing migration tests.

### Domain types

- `StoredPrinter` gains `location: Option<String>`, `start_safety:
  StartSafety`, and `archived_at: Option<String>`.
- `host_identity` is derived from `connection` whenever the repository
  writes. Callers never set it directly.
- `PrinterPatch` gains `location: Option<Option<String>>`, where `null`
  clears it, and `start_safety: Option<StartSafety>`.
- `ResolvedPrinter`, exported to TypeScript as `PrinterRecord`, gains
  `location`, `startSafety`, `archivedAt`, and `setupGaps`.
- `ReadinessReason` gains `archived`. The archive check runs before every
  other check, so an archived Printer is `notReady/archived` regardless of
  its setup gaps. No live status is published for an archived Printer; the
  reason exists so that P7 eligibility can consume it.

### Commands

These are added to `COMMAND_NAMES`, `COMMAND_CONTRACTS`, and the ts-rs
exports:

| Command | Change |
|---|---|
| `create_printer` | Args become `{ name, catalogRef, location?, startSafety?, defaultBedType?, connection?: ConnectionSubmission }`. Validates a non-empty trimmed name ≤ 128 chars. |
| `update_printer` | `patch` gains `location`, `startSafety`. |
| `probe_connection` | New. `{ submission }` → `ProbeResult`. |
| `set_printer_connection` | Gains `acceptUnverified?: bool`, plus the probe-before-replace and duplicate-host checks. |
| `create_printers_batch` | New; see below. |
| `cancel_printer_batch` | New. `{ batchId }` → `{}`. |
| `printer_lifecycle_eligibility` | New. `{ id }` → `LifecycleEligibility`. |
| `archive_printer` | New. `{ id, expectedRevision }` → `PrinterMutationResult`. |
| `unarchive_printer` | New. `{ id, expectedRevision }` → `PrinterMutationResult`. |
| `delete_printer` | Enforces eligibility. |

The batch contract:

```text
CreatePrintersBatchRequest {
  batchId: string,
  shared: { catalogRef, defaultBedType?, startSafety },
  sharedCredential?: string,
  probe: bool,
  rows: [{ rowId: string, name, location?,
           connection?: { kind, host, port, useTls,
                          credential: { source: "none" }
                                    | { source: "shared" }
                                    | { source: "row", value: string } } }]
}
CreatePrintersBatchResult {
  batchId,
  rows: [{ rowId,
           outcome: "created" | "createdSetupIncomplete" | "rejected" | "cancelled",
           printer?: PrinterRecord,
           credentialStored: bool,
           probe?: ProbeResult,
           errors: BatchRowError[],
           warnings: BatchRowWarning[] }]
}
BatchRowError { code, fieldPath?, message, conflictingPrinterId?, conflictingRowId? }
```

- **Result rows:** `rows` is returned in request order, and every `rowId` in
  the request appears exactly once. `rowId`s must be unique and non-empty;
  otherwise the whole request fails with `VALIDATION`, because correlation
  would be undefined.
- **`BatchRowErrorCode` values:** `VALIDATION`, `DUPLICATE_HOST`,
  `UNSUPPORTED_ADAPTER`, `AUTHENTICATION_FAILED`, `PRINTER_UNREACHABLE`,
  `TIMEOUT`, `PROTOCOL_ERROR`, `CREDENTIAL_UNAVAILABLE`,
  `PERSISTENCE_UNAVAILABLE`, `CANCELLED`.
- **`BatchRowWarningCode` values:** `CAPABILITY_MISMATCH`, `DUPLICATE_NAME`,
  `CREDENTIAL_CLEANUP_PENDING`, `SUPERVISOR_RECONCILIATION_FAILED`.
- **Top-level `ErrorCode`** gains `DUPLICATE_HOST` and `LIFECYCLE_BLOCKED`.

### Export and import

- **Export:** the Printers export uses `schemaVersion: 2`, with `location`,
  `startSafety`, and `archivedAt` per Printer.
- **Import:** accepts v1 (with defaults `null`, `confirmBedClear`, and
  `null`) and v2.
- **Duplicate hosts on import:** handled exactly as in the migration rule.
  The later duplicate is imported archived, with a warning.

## Frontend architecture

### State

`printer-store.ts` gains `probeCandidate`, `createPrinter(options)`,
`createPrintersBatch`, `cancelBatch`, `archivePrinter`, `unarchivePrinter`,
and `lifecycleEligibility`. Like `testConnection` and `discoverPrinters`
today:

- `probeCandidate` and `createPrintersBatch` reject instead of routing to the
  banner, because their callers render the errors inline.

In web mode:

- Creation and lifecycle mutations change local state only.
- `probeCandidate` throws "needs the desktop app".
- The batch call simulates Profile-only creation for every row.

The batch dialog owns batch rows as component-local state (a `createStore`).
They are not domain state. Results are merged into rows by `rowId`, and
failed rows keep their input.

Pure modules:

- `src/printers/batch-intake.ts`: parse, generate, and map discovery.
- `src/printers/host-identity.ts`: the TypeScript mirror of D2.

### Components

**Design-system additions** (each is used by more than one screen):

- `Stepper`: an ordered step list with current, complete, and error states,
  keyboard-navigable to completed steps.
- `Textarea`: backs the paste field and Setup notes.

Both use tokens only and are added to `components/index.ts` and
`Showcase.tsx`.

**`screens/PrinterSetupWizard.tsx`** replaces `PrinterAddDialog.tsx`, which
is removed once nothing depends on it. Its steps:

1. **Identify:** the existing brand/model/nozzle/name logic, plus location.
2. **Connect:** discovery candidates or manual entry. **Test** uses
   `probeCandidate`. **Skip — save Profile-only** is always available.
3. **Operate:** start-safety `RadioGroup` and bed type `Select`.
4. **Review:**
   - Shows the summary, capability mismatches (`buildMismatches`), the
     credential-store location, and a "Setup incomplete" notice if no
     Connection is given.
   - **Save** is the only step that persists.

`PrinterConnectionPanel` field logic is extracted into a
`ConnectionFields` component, shared by the wizard (candidate mode) and the
Setup tab (existing-Printer mode).

**`screens/PrinterBatchDialog.tsx`** and its screen-local `BatchRowsTable`
(a CSS-module grid with row selection checkboxes). Its steps:

1. **Shared:** model, variant, bed type, and safety.
2. **Rows:** generate, paste, or import a CSV file (a file input read as
   text). Rows are editable inline.
3. **Connect:**
   - Discovery mapping panel.
   - Row selection, with an explicit "Apply to selected" for protocol, port,
     TLS, and credential source.
   - A shared credential field, never pre-filled.
   - A per-row preview of copied values.
4. **Review & results:**
   - A **Create** button with a probe toggle, on by default.
   - Per-row status while running, and **Cancel** while running.
   - Afterwards: per-row outcome and errors, and **Retry failed**, which
     re-submits rejected and cancelled rows as a new batch and calls
     `set_printer_connection` for `createdSetupIncomplete` rows with
     Connection input.

**Entry points:** add "Add Printers…" next to "Add Printer" in the Monitor
toolbar and first-run state.

**Setup tab** (`PrinterSetupPanel` and the dock):

- Location field and start-safety radio.
- `ConnectionFields` with replace semantics. A failed replacement probe
  shows the error plus **Save anyway**.
- **Archive** / **Unarchive**.
- **Delete…**, shown only when `canDelete`, opens a confirm dialog that
  requires typing the name.
- Blockers from eligibility are listed when an action is unavailable.

**Monitor:**

- `location` becomes a working Monitor Section. Printers without a location
  go in a "No location" group.
  - *Planning clarification:* the persisted `monitorSection` preference
    defaults to `printerModel`, and the app cannot tell "never chosen" apart
    from "chose Printer Model".
  - So P2 does not switch the section to Location automatically. This
    departs from the umbrella rule that Location is the default once
    locations exist, and the P2 verification record notes it.
- Search matches location.
- An **Archived** filter shows only archived Printers. Every other filter
  excludes them.

## Errors and recovery

| Situation | Behaviour |
|---|---|
| Probe fails during single setup | Error shown inline at Connect. The user can edit, retry, or skip to Profile-only. |
| Batch row identity invalid | `rejected`, not persisted, input kept, row flagged with `fieldPath`. |
| Batch row Connection fails | `createdSetupIncomplete`, Connection input kept for retry through `set_printer_connection`. |
| Cancel mid-batch | Committed rows kept, the rest `cancelled` and retained. |
| Credential store unavailable | Row `createdSetupIncomplete` with `CREDENTIAL_UNAVAILABLE`, no secret persisted. |
| Replacement probe fails | Stored Connection untouched, and "Save anyway" offered. |
| Delete while not archived | `LIFECYCLE_BLOCKED`, with blockers listed. |
| Unarchive into a host conflict | `DUPLICATE_HOST`, naming the conflicting Printer. |
| Revision conflict | Existing `CONFLICT` handling: reload and retry. |

## Accessibility and adaptation

- Both wizards are keyboard-operable end to end. The Stepper uses
  `aria-current="step"`.
- The batch grid supports row selection by keyboard (Space on the row
  checkbox) and announces per-row outcomes with icon, text, and colour,
  following the P1 severity rule.
- Everything works at 1440 × 900 and 1024 × 700. At 1024 wide, the batch grid
  scrolls horizontally inside the dialog, not the page.

## Acceptance criteria

1. Migration v2→v3 applies, is ledgered, survives crash-boundary injection,
   backfills host identity, and archives pre-existing duplicate hosts with a
   warning.
2. A Profile-only Printer can be created, persists across restart, and shows
   as Setup incomplete, not Offline.
3. A connected Printer can be created in one step. The credential is stored
   under its own ref, and supervision starts only after commit.
4. `probe_connection` leaves storage byte-identical and produces no
   supervisor task or status event.
5. A batch with one failing probe creates the other rows. The failing row is
   `createdSetupIncomplete` with its `rowId`. An invalid-identity row is
   `rejected`, not persisted, and its input is retained in the UI.
6. Retrying a `createdSetupIncomplete` row attaches a Connection to the same
   Printer without creating a duplicate.
7. A seeded shared secret appears in no batch result, error, warning, log,
   SQLite file, or generated contract.
8. Probes never exceed 4 concurrent, and cancellation leaves uncommitted rows
   `cancelled` and unpersisted.
9. Duplicate hosts are rejected against the DB, within a batch, and on
   unarchive. Archived Printers do not reserve hosts.
10. Shared bed type is copied as an independent per-Printer override. Editing
    one Printer later does not affect the others.
11. Archive stops supervision, survives restart unsupervised, and keeps
    identity. Unarchive restores supervision.
12. Deleting a non-archived Printer fails with `LIFECYCLE_BLOCKED`. Deleting
    an archived Printer keeps the delete/credential-cleanup ordering.
13. Replacing a working Connection with a failing one is rejected unless
    `acceptUnverified`.
14. Location, start safety, and archive state round-trip through export v2,
    and v1 imports still load.
15. Frontend tests cover the wizard, batch intake, generation, mapping,
    retry, Setup tab guards, and Monitor location and archive views.
16. The tracer completes through the Tauri path: one Profile-only Printer,
    one connected Printer, a batch across "Bay A" and "Bay B" with one failed
    row retained, and one Printer archived and still identifiable after
    restart.
17. Keyboard and viewport checks pass at 1440 × 900 and 1024 × 700.

## Delivery strategy

Build contract-first vertical slices:

1. Migration and domain fields.
2. Setup-facts derivation.
3. Lifecycle (archive, eligibility, and delete).
4. Probe and create with options.
5. Batch backend.
6. Contracts and export.
7. Design-system primitives.
8. Single wizard.
9. Batch dialog.
10. Setup tab and Monitor.
11. Tracer and verification evidence.

Each slice lands with its red/green tests. The task-level plan is
`docs/superpowers/plans/2026-09-22-p2-printer-lifecycle-batch-setup.md`.
