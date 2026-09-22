# P2 Printer Lifecycle and Batch Setup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver durable single-Printer and batch-Printer setup, with
Profile-only and connected paths, bounded probing, partial success and
retry, shared bed type, location, start safety, archive, and archived-only
deletion.

**Architecture:** Rust owns persisted truth. That covers four columns on
`printers`, one setup-facts derivation, canonical host identity with a
partial unique index, a lifecycle-eligibility registry, and a batch command
that commits each row independently. Rows are probed with at most 4 in
flight and can be cancelled. The SolidJS side adds two design-system
primitives (`Stepper`, `Textarea`), pure intake/host-identity modules, a
four-step single wizard, a four-step batch dialog, and Setup-tab and
Monitor extensions. Generated ts-rs contracts carry every wire type.

**Tech Stack:** Rust, Tauri 2, rusqlite/SQLite, tokio, futures, ts-rs,
SolidJS, TypeScript, Kobalte, CSS Modules, Vitest, and Rust unit/integration
tests.

**Spec:** `docs/superpowers/specs/2026-09-22-p2-printer-lifecycle-batch-setup-design.md`
(decisions D1–D14 are cited below by number).

## Global Constraints

**Data and ownership**

- Rust remains persisted truth. Frontend batch rows are component-local
  view state, not domain state.
- Location is optional, trimmed, and 1–128 chars.
- `startSafety` is `"confirmBedClear" | "unattended"`, default
  `"confirmBedClear"`. P2 stores it and does not enforce it.
- Host identity is `canonical_host_identity(host, port)` (D2). Protocol and
  TLS are excluded, and there is no DNS resolution.
- No two non-archived Printers may share a host identity (D3).
- Only archived Printers can be deleted, and the UI requires typing the name
  (D7).
- Shared setup is copied at creation. There is no batch or shared entity.
  The only shared bed field is `defaultBedType` (D9).

**Batches and probing**

- Batch probing runs at most 4 in flight, uses the adapter's own 10 s probe
  timeout, and is cancellable by `batchId` (D14).
- Rows with valid identity whose Connection fails become Printers with
  outcome `createdSetupIncomplete`. Rows with invalid identity are
  `rejected` and never persisted (D1).

**Secrets**

- Secrets never appear in responses, errors, warnings, `eprintln!` output,
  SQLite, or generated contracts.
- Each Printer gets its own `credentialRef` (D13).
- CSV intake never reads credentials and rejects credential-like columns
  (D10).

**Supervision**

- Connection testing (`probe_connection`) never touches storage, the
  credential store, or the supervisor (D8).
- Delete keeps its order:
  1. DB delete plus cleanup intent in one transaction.
  2. `reconciliation_guard`.
  3. `stop_and_wait`.
  4. Credential cleanup retry.
- Supervision starts only after the row's commit.

**Out of scope**

- Material Slots, Spools, Equip, cameras, alerts, and Job/reservation
  blockers.
- Enforcing start safety.
- New adapters (Moonraker only).
- Later bulk edit.

**Repo conventions**

- Use Kobalte primitives for interactive behaviour, CSS Modules, and only
  `--f3d-*` color/type/radius tokens. Follow the editor aesthetic: no
  elevation or ripple.
- New design-system components go in `components/index.ts` and
  `Showcase.tsx`.
- Kobalte `Select`/`DropdownMenu` tests use `fireEvent.pointerDown` and
  `fireEvent.pointerUp`.
- Never hand-edit `src/generated/contracts/**`. Run `just gen-contracts`.
- Every new command goes in `lib.rs` `COMMAND_NAMES`, in
  `generate_handler!`, in `contracts/inventory.rs` `COMMAND_CONTRACTS` (and
  its array length), and in the `CommandContracts` decl and visitor.

**Validation**

- Frontend is done when `just build` and `just test` pass.
- Rust is done when `source "$HOME/.cargo/env" && just test-rust` passes and
  `just gen-contracts` leaves no diff.

## Known spec clarifications found while planning

These are recorded here and applied by the named tasks:

1. **D4, missing credential.** `restore_persisted_connections` and
   `set_printer_connection` today reconcile a Printer to Setup incomplete
   when its referenced credential is missing from the store, and they emit
   `CREDENTIAL_REQUIRED`. P2 keeps that runtime behaviour.
   - `derive_setup_facts` covers persisted facts only.
   - A missing secret is a runtime refinement applied by the shared
     `supervise_printer` helper (Task 2).
   - `setupGaps` doesn't report a missing secret.
2. **Monitor default section.** Settings persists `monitorSection`,
   defaulting to `printerModel`. The app can't distinguish "never chosen"
   from "chose Printer Model". P2 therefore makes Location a working
   section, with a "No location" group, but does **not** auto-switch the
   persisted preference.
   - This deviates from the umbrella's "Location is the default once
     locations exist".
   - It is recorded in the verification doc for the user.
3. **Archived hydration.** The supervisor hydrates snapshot rows for every
   Printer at startup. `restore_persisted_connections` must call
   `manager.forget(id)` for archived Printers so they never appear in the
   backfill (Task 3).

## File and module map

### Backend

- **Create** `src-tauri/migrations/0003_p2_printer_lifecycle.sql`: columns
  plus the partial unique index.
- **Modify** `src-tauri/src/persistence/migrations.rs`: schema v3 and the
  Rust post-step hook for backfilling host identity and archiving
  duplicates.
- **Create** `src-tauri/src/printers/host_identity.rs`: `canonical_host_identity`.
- **Create** `src-tauri/tests/fixtures/host-identity.json`: shared test
  vectors, also read by the TypeScript tests.
- **Modify** `src-tauri/src/printers/mod.rs`: new `StoredPrinter` fields, the
  `StartSafety` enum, and an extended `PrinterPatch`.
- **Modify** `src-tauri/src/printers/repository.rs`: new columns, host-identity
  maintenance, the duplicate check, `create_in`, and archive/unarchive
  writes.
- **Create** `src-tauri/src/printers/setup.rs`: `derive_setup_facts`,
  `SetupGap`, and the `supervise_printer` helper.
- **Create** `src-tauri/src/printers/lifecycle.rs`: eligibility types, the
  blocker registry, and the archive/unarchive/eligibility commands.
- **Create** `src-tauri/src/printers/create.rs`: `create_printer_with`, the
  single create path shared by `create_printer` and batch.
- **Create** `src-tauri/src/printers/batch.rs`: batch contracts, the
  cancellation registry, and `create_printers_batch` / `cancel_printer_batch`.
- **Modify** `src-tauri/src/printers/commands.rs`: `create_printer` args,
  `update_printer` patch, guarded `delete_printer`, and export/import v2.
- **Modify** `src-tauri/src/printers/operational.rs`: the `archived`
  readiness reason.
- **Modify** `src-tauri/src/catalog/resolve.rs`: `ResolvedPrinter` gains
  `location`, `startSafety`, `archivedAt`, and `setupGaps`.
- **Modify** `src-tauri/src/connections/commands.rs`: `probe_connection`,
  `acceptUnverified`, probe-before-replace, the duplicate check, and
  `probe_error` extracted from `test_printer_connection`.
- **Modify** `src-tauri/src/connections/supervisor.rs`: `connection_for`,
  which exposes the injectable factory for probes, plus `forget`.
- **Modify** `src-tauri/src/contracts/command.rs`: `ErrorCode::DuplicateHost`,
  `ErrorCode::LifecycleBlocked`, and their constructors.
- **Modify** `src-tauri/src/contracts/inventory.rs` and `src-tauri/src/lib.rs`:
  command registration and startup restore through `supervise_printer`.
- **Tests:**
  - `src-tauri/tests/p2_migration.rs`
  - `src-tauri/tests/p2_lifecycle.rs`
  - `src-tauri/tests/p2_batch.rs`
  - `src-tauri/tests/p2_contract_path.rs`
  - Unit tests beside each new module.

### Frontend

- **Generated:** `src/generated/contracts/**`, via `just gen-contracts` only.
- **Create** `src/design-system/components/{Stepper,Textarea}.{tsx,module.css}`,
  plus updates to `index.ts`, `components.test.tsx`, and `Showcase.tsx`.
- **Create** `src/printers/host-identity.ts` and its test (reads the Rust
  fixture).
- **Create** `src/printers/batch-intake.ts` and its test: parse, generate,
  and map discovery.
- **Modify** `src/printers/{types.ts,printer-store.ts}` and their tests: new
  actions and web fallbacks.
- **Create** `src/screens/ConnectionFields.{tsx,module.css}` and its test,
  extracted from `PrinterConnectionPanel`.
- **Create** `src/screens/PrinterSetupWizard.{tsx,module.css}` and its test.
  This replaces `PrinterAddDialog.*`, which is deleted.
- **Create** `src/screens/PrinterBatchDialog.{tsx,module.css}`,
  `src/screens/BatchRowsTable.{tsx,module.css}`, and their tests.
- **Modify** `src/screens/{PrinterSetupPanel,PrinterConnectionPanel,PrinterDetailDock,PrinterDashboard,MonitorToolbar}.tsx`
  and their tests.
- **Create** `src/screens/DeletePrinterDialog.{tsx,module.css}` and its test.
- **Modify** `src/monitor/monitor-store.ts` and its test: Location section,
  the Archived filter, and location search.

### Docs

- **Modify** `CONTEXT.md`: Location, Profile-only Printer, Setup incomplete,
  Start-safety rule, and Archive.
- **Modify** `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`:
  close the two known-unknown rows.
- **Create** `docs/verification/2026-09-2x-p2-printer-lifecycle.md`.

---

### Task 1: Schema v3, host identity, and domain fields

**Owner:** Backend
**Prerequisites:** Approved spec.
**Handoff:** Every later backend task relies on the columns, `StartSafety`,
and `canonical_host_identity`.

**Files:**
- Create: `src-tauri/migrations/0003_p2_printer_lifecycle.sql`
- Create: `src-tauri/src/printers/host_identity.rs`
- Create: `src-tauri/tests/fixtures/host-identity.json`
- Create: `src-tauri/tests/p2_migration.rs`
- Modify: `src-tauri/src/persistence/migrations.rs`
- Modify: `src-tauri/src/printers/{mod.rs,repository.rs}`
- Modify: `src-tauri/src/catalog/resolve.rs` (fields on `ResolvedPrinter`)

**Interfaces:**
- Produces:
  - `pub fn canonical_host_identity(host: &str, port: u16) -> Option<String>`,
    which returns `None` for an empty host.
  - `#[derive(Serialize, Deserialize, TS, Clone, Copy, PartialEq, Eq, Debug, Default)] #[serde(rename_all = "camelCase")] pub enum StartSafety { #[default] ConfirmBedClear, Unattended }`,
    exported to `domain/StartSafety.ts`.
  - `StoredPrinter` gains `location: Option<String>`,
    `start_safety: StartSafety`, and `archived_at: Option<String>`, all with
    `#[serde(default)]`.
  - `RepositoryError::DuplicateHost { conflicting_printer_id: String }`.
  - `PrinterRepository::find_active_by_host_identity(&self, identity: &str, excluding: Option<&str>) -> Result<Option<String>, StorageError>`.

- [ ] **Step 1: Write the host-identity vectors and failing unit tests**

`src-tauri/tests/fixtures/host-identity.json` holds an array of
`{ "host", "port", "expected" }` entries. `expected` is `null` for an empty
host. Include at least these cases:

```json
[
  { "host": "Printer.Local.", "port": 7125, "expected": "printer.local:7125" },
  { "host": "  192.168.1.20 ", "port": 80, "expected": "192.168.1.20:80" },
  { "host": "[FE80::0001]", "port": 7125, "expected": "[fe80::1]:7125" },
  { "host": "fe80::1", "port": 7125, "expected": "[fe80::1]:7125" },
  { "host": "", "port": 7125, "expected": null },
  { "host": "voron-a", "port": 7125, "expected": "voron-a:7125" }
]
```

In `host_identity.rs`, add a `#[cfg(test)]` test that
`include_str!`s the fixture and asserts every vector. Run
`source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml host_identity`.
Expected: FAIL to compile, because the function doesn't exist.

- [ ] **Step 2: Implement `canonical_host_identity` (D2)**

```rust
pub fn canonical_host_identity(host: &str, port: u16) -> Option<String> {
    let trimmed = host.trim();
    let unbracketed = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(trimmed);
    let lowered = unbracketed.to_ascii_lowercase();
    let host = lowered.strip_suffix('.').unwrap_or(&lowered);
    if host.is_empty() {
        return None;
    }
    Some(match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(v6)) => format!("[{v6}]:{port}"),
        Ok(std::net::IpAddr::V4(v4)) => format!("{v4}:{port}"),
        Err(_) => format!("{host}:{port}"),
    })
}
```

Register `pub mod host_identity;` in `printers/mod.rs`. Run the test again.
Expected: PASS.

- [ ] **Step 3: Write failing migration tests in `tests/p2_migration.rs`**

Follow `tests/f1_migration.rs` for the storage setup. The tests must cover:

1. A fresh DB reaches `PRAGMA user_version = 3`, with ledger row
   `(3, "0003_p2_printer_lifecycle", <checksum>)`.
2. A v2 DB with two Printers whose `connection_json` hosts are
   `"Voron.local"` and `"voron.local."`, both on port 7125, upgrades to v3.
   - The older Printer (by `created_at`, then `id`) has
     `host_identity = "voron.local:7125"` and `archived_at IS NULL`.
   - The newer one has `archived_at` set and `host_identity` set, and it
     keeps its `connection_json`.
   - One `migration_warnings` row exists with code
     `DUPLICATE_HOST_ARCHIVED`.
3. After the upgrade, inserting a third active Printer with the same
   `host_identity` fails with a unique-constraint error.
4. `location = ' x'` (untrimmed) and `start_safety = 'yolo'` are rejected
   by the CHECK constraints.
5. A crash-boundary test: inject a failure after the v3 SQL and before
   commit, using the pattern in `persistence/mod.rs` tests, and confirm the
   DB is still v2 and unchanged.

To build the v2 fixture DB, open storage with migrations 1–2 only. Use the
`#[cfg(test)]` helper in `migrations.rs`, which applies migrations up to a
given version, adding it if it's missing:
`pub(crate) fn apply_through(connection, max_version)`. It's exposed to
integration tests through the existing `persistence::test_support` module,
or you can add one following how `f1_migration.rs` reaches internals.

Look up the `migration_warnings` column names in `0001_foundation.sql` and
use them exactly. Run the tests. Expected: FAIL (schema version 2).

- [ ] **Step 4: Add the SQL migration**

`0003_p2_printer_lifecycle.sql`:

```sql
ALTER TABLE printers ADD COLUMN location TEXT
  CHECK (location IS NULL OR (length(location) BETWEEN 1 AND 128 AND location = trim(location)));
ALTER TABLE printers ADD COLUMN start_safety TEXT NOT NULL DEFAULT 'confirmBedClear'
  CHECK (start_safety IN ('confirmBedClear', 'unattended'));
ALTER TABLE printers ADD COLUMN archived_at TEXT;
ALTER TABLE printers ADD COLUMN host_identity TEXT;
CREATE UNIQUE INDEX printers_active_host_identity
  ON printers(host_identity)
  WHERE archived_at IS NULL AND host_identity IS NOT NULL;
```

The index is safe to create here because every `host_identity` is NULL at
this point.

- [ ] **Step 5: Add the migration post-step hook**

Extend `struct Migration` with
`post: Option<fn(&rusqlite::Transaction<'_>) -> Result<(), StorageError>>`.
Set it to `None` for migrations 1 and 2, and to `Some(backfill_host_identity)`
for 3. In `apply`, call it right after `execute_batch(migration.sql)`. The
checksum stays SQL-only.

Bump `CURRENT_SCHEMA_VERSION` to 3 and `MIGRATIONS` to `[Migration; 3]`.
Update every hard-coded `2` in the schema-version error paths:
`connections/commands.rs` `storage_command_error` and `lib.rs`
`startup_command_error` pass the current version, so use
`persistence::CURRENT_SCHEMA_VERSION` rather than a literal (re-export it
`pub`).

`backfill_host_identity` works as follows:

1. `SELECT id, created_at, json_extract(connection_json,'$.host'), json_extract(connection_json,'$.port') FROM printers WHERE connection_json IS NOT NULL ORDER BY created_at, CAST(id AS BLOB)`.
2. Compute `canonical_host_identity` for each row, then group by identity.
3. For every row after the first in a group:
   - `UPDATE printers SET archived_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?`.
   - Insert a `migration_warnings` row with code `DUPLICATE_HOST_ARCHIVED`,
     whose details name the kept Printer id and the archived one.
4. Only then, `UPDATE printers SET host_identity=? WHERE id=?` for every row.
   Archived rows are excluded from the partial index, so this can't
   conflict.

Run the migration tests. Expected: PASS.

- [ ] **Step 6: Carry the new columns through the repository**

- Extend every `SELECT` column list, `decode`, `insert`, and `replace` in
  `repository.rs` with `location`, `start_safety`, `archived_at`, and
  `host_identity`.
  - Pull the repeated SELECT list into one `const PRINTER_COLUMNS: &str`.
  - `decode` maps `start_safety` text through `StartSafety`'s serde name.
- `host_identity` is **computed** in `insert`/`replace` from
  `printer.connection` (`canonical_host_identity(&c.host, c.port)`). It is
  never a field on `StoredPrinter`.
- Map a SQLite unique violation on `printers_active_host_identity` in
  `insert`/`replace` to a new `StorageError::Constraint` variant. Callers
  pre-check (Step 7), so this is a backstop.
- Add `find_active_by_host_identity`.

Add `RepositoryError::DuplicateHost { conflicting_printer_id }` in
`persistence/error.rs`, and extend `CommandError::from_repository` so it
maps to the new `ErrorCode::DuplicateHost`.

Add `ErrorCode::DuplicateHost` and `ErrorCode::LifecycleBlocked` to
`contracts/command.rs`, plus these constructors:

```rust
pub fn duplicate_host(conflicting_printer_id: &str) -> Self // recovery [EditFields], retryable false, details {"conflictingPrinterId": id}
pub fn lifecycle_blocked(blockers: serde_json::Value) -> Self // recovery [], retryable false, details {"blockers": [...]}
```

- [ ] **Step 7: Check duplicates before writes**

In `create`, `update`, and `set_connection`, inside the same `write`
closure, before the insert or replace:

```rust
if printer.archived_at.is_none() {
    if let Some(identity) = identity_of(&printer) {
        if let Some(other) = active_with_identity(transaction, &identity, Some(&printer.id))? {
            return Err(StorageError::DuplicateHost(other));
        }
    }
}
```

Carry the conflicting id out through a `StorageError::DuplicateHost(String)`
variant, and map it in `classify_entity_write` / `create` to
`RepositoryError::DuplicateHost`.

Add unit tests in `repository.rs` (or `tests/f1_repositories.rs`-style) for:

- Creating a second active Printer with the same host → `DuplicateHost`.
- The same host when the first Printer is archived → OK.
- A new-field round trip.

- [ ] **Step 8: Expose the fields on `ResolvedPrinter`**

Add `location: Option<String>` (`#[ts(optional)]`), `start_safety:
StartSafety`, and `archived_at: Option<String>` (`#[ts(optional)]`), and
fill them in `resolve_printer`. (`setupGaps` arrives in Task 2.)

Run `just gen-contracts`, then
`source "$HOME/.cargo/env" && just test-rust`. Expected: PASS, and the
contract drift test passes after regeneration.

- [ ] **Step 9: Commit**

```bash
git add src-tauri src/generated
git commit -m "feat: add P2 printer lifecycle schema and host identity"
```

---

### Task 2: One setup-facts derivation and a supervise helper

**Owner:** Backend
**Prerequisites:** Task 1.
**Handoff:** Tasks 3–6 call `supervise_printer` instead of hand-building
`PrinterSetupFacts`.

**Files:**
- Create: `src-tauri/src/printers/setup.rs`
- Modify: `src-tauri/src/printers/{mod.rs,commands.rs,operational.rs}`
- Modify: `src-tauri/src/catalog/resolve.rs`
- Modify: `src-tauri/src/connections/{commands.rs,supervisor.rs}`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "domain/SetupGap.ts")]
pub enum SetupGap { MissingConnection, UnsupportedAdapter, UnresolvedProfile }

pub fn derive_setup_facts(printer: &StoredPrinter, catalog: &Catalog) -> (PrinterSetupFacts, Vec<SetupGap>);

pub enum SupervisionOutcome { Started, Reconciled, CredentialRequired, Archived }

pub async fn supervise_printer<R: tauri::Runtime>(
    manager: &ConnectionManager<R>,
    credentials: &dyn CredentialBackend,
    catalog: &Catalog,
    printer: &StoredPrinter,
) -> SupervisionOutcome;
```

- `ResolvedPrinter.setup_gaps: Vec<SetupGap>`.
- `ReadinessReason::Archived`.

- [ ] **Step 1: Write failing unit tests for `derive_setup_facts`**

In `setup.rs`, use the test catalog fixture used by `resolve.rs` tests.
Cover these cases:

- No connection → `(has_usable_connection=false, profile_resolved=true)`,
  gaps `[MissingConnection]`.
- `kind="octoprint"` → gaps `[UnsupportedAdapter]`.
- Unresolvable catalog ref plus no connection → gaps
  `[MissingConnection, UnresolvedProfile]`, in that order.
- A Moonraker connection with a resolved profile → no gaps, and
  `setup_complete()` is true.

Profile resolution uses `resolve_catalog_ref(catalog, &printer.catalog_ref)`.
The status is `ok` or `rematched` exactly when the returned variant is
`Some`; check how `CatalogStatus` values map before assuming.

- [ ] **Step 2: Implement `derive_setup_facts` and add `setup_gaps` to `resolve_printer`**

- [ ] **Step 3: Write failing tests for `supervise_printer`**

Use the supervisor's `with_clock_and_factory` with a fake connection and a
file-backed `CredentialStore` in a tempdir. Existing supervisor tests show
how to build a mock `AppHandle`. Cover these cases:

| Printer | Expected outcome | Expected status |
|---|---|---|
| Archived | `Archived` | No status in `manager.statuses()`, no task |
| No connection | `Reconciled` | `setupIncomplete` |
| Connection with `credential_ref` missing from store | `CredentialRequired` | `setupIncomplete` |
| Connection, secret present | `Started` | Fake connection factory was called |

- [ ] **Step 4: Implement `supervise_printer`, `ConnectionManager::forget`, and `connection_for`**

`forget(id)` removes the status from `StatusMap` and publishes
`printer.status.removed` without touching the snapshot row. Implement it by
reusing `publish_removed` and a `StatusMap::remove`.

`connection_for(&self, config, key) -> Option<Box<dyn PrinterConnection>>`
calls `self.connection_factory`. It is used in Task 4 so probes are
injectable in tests.

```rust
pub async fn supervise_printer<R: tauri::Runtime>(manager: &ConnectionManager<R>, credentials: &dyn CredentialBackend, catalog: &Catalog, printer: &StoredPrinter) -> SupervisionOutcome {
    if printer.archived_at.is_some() {
        let _ = manager.stop(&printer.id).await; // stop also publishes removed
        return SupervisionOutcome::Archived;
    }
    let (facts, _) = derive_setup_facts(printer, catalog);
    let Some(config) = printer.connection.clone().filter(|_| facts.has_usable_connection) else {
        manager.reconcile_printer(&printer.id, facts);
        return SupervisionOutcome::Reconciled;
    };
    let secret = match config.credential_ref.as_deref() {
        None => None,
        Some(reference) => match credentials.get(reference) {
            Ok(Some(value)) => Some(zeroize::Zeroizing::new(value)),
            _ => {
                manager.reconcile_printer(&printer.id, PrinterSetupFacts { has_usable_connection: false, ..facts });
                return SupervisionOutcome::CredentialRequired;
            }
        },
    };
    manager.start(printer.id.clone(), config, secret, facts).await;
    SupervisionOutcome::Started
}
```

Check `CredentialBackend::get`'s actual return type in `credentials.rs`
(`Result<Option<String>, ()>`) and adapt to it.

- [ ] **Step 5: Replace every ad hoc `PrinterSetupFacts { .. }` construction**

The call sites are:
- `lib.rs` `restore_persisted_connections`
- `printers/commands.rs` `create_printer` and `import_printers`
- `connections/commands.rs` `set_printer_connection` and
  `clear_printer_connection`

Each becomes `supervise_printer` or `derive_setup_facts`.

In `restore_persisted_connections` (sync; keep `block_on`), iterate Printers
and call `supervise_printer`. Archived Printers therefore get
`stop` + removed.

Map `CredentialRequired` to the existing `OperationWarning::credential_required`.

Delete the unused legacy `restore_stored_connections` only if nothing
references it. Grep first, and leave it if tests use it.

- [ ] **Step 6: Add the `ReadinessReason::Archived` enum variant**

No policy change is needed, because archived Printers get no live status.
Add a doc comment saying the reason exists for P7 eligibility. Regenerate
contracts.

- [ ] **Step 7: Run the whole Rust suite**

Run `source "$HOME/.cargo/env" && just test-rust && just gen-contracts`.
Expected: PASS and no diff.

- [ ] **Step 8: Commit**

Message: `refactor: derive Printer setup facts in one place`.

---

### Task 3: Lifecycle eligibility, archive, unarchive, and guarded delete

**Owner:** Backend (wiring owner reviews ordering)
**Prerequisites:** Task 2.
**Handoff:** Task 10 renders eligibility. P3, P7, and P8 register blocker
sources.

**Files:**
- Create: `src-tauri/src/printers/lifecycle.rs`
- Create: `src-tauri/tests/p2_lifecycle.rs`
- Modify: `src-tauri/src/printers/{mod.rs,commands.rs,repository.rs}`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/contracts/inventory.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
pub enum LifecycleAction { Archive, Unarchive, Delete }

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LifecycleBlockerCode { NotArchived, AlreadyArchived }

#[derive(Serialize, Clone, Debug, TS)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleBlocker { pub action: LifecycleAction, pub code: LifecycleBlockerCode, pub message: String }

#[derive(Serialize, Clone, Debug, TS)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleEligibility { pub can_archive: bool, pub can_unarchive: bool, pub can_delete: bool, pub blockers: Vec<LifecycleBlocker> }

pub trait LifecycleBlockerSource: Send + Sync {
    fn blockers(&self, printer: &StoredPrinter, tx: &rusqlite::Transaction<'_>) -> Result<Vec<LifecycleBlocker>, StorageError>;
}
pub fn blocker_sources() -> &'static [&'static dyn LifecycleBlockerSource]; // P2: [&ArchiveStateBlockers]
pub fn evaluate(printer: &StoredPrinter, tx: &rusqlite::Transaction<'_>) -> Result<LifecycleEligibility, StorageError>;
```

- Commands:
  - `printer_lifecycle_eligibility(id) -> LifecycleEligibility`
  - `archive_printer(id, expectedRevision) -> PrinterMutationResult`
  - `unarchive_printer(id, expectedRevision) -> PrinterMutationResult`
- `LifecycleBlockerCode` is exported to `domain/LifecycleBlockerCode.ts`.
  Later phases add variants.

- [ ] **Step 1: Write failing integration tests in `tests/p2_lifecycle.rs`**

Use `tauri::test::{mock_builder, mock_context, get_ipc_response}` with
`RuntimeServices::for_test`, as in `tests/f1_contract_path.rs`. Use a fake
connection factory that records starts.

1. **Eligibility of an active Printer:** `canArchive=true`,
   `canUnarchive=false`, `canDelete=false`, with blockers
   `[{delete, NOT_ARCHIVED}, {unarchive, NOT_ARCHIVED}]`.
2. **Delete of an active Printer:** fails with `LIFECYCLE_BLOCKED`,
   `details.blockers[0].code == "NOT_ARCHIVED"`, and the row still exists.
3. **Archive of a connected Printer:**
   - Revision +1 and `archivedAt` set.
   - The supervisor task is stopped and `printer_statuses` omits the
     Printer.
   - `connection` and `credentialRef` are unchanged, and the credential
     still exists in the store.
4. **Archive, then restart:** rebuild `RuntimeServices` over the same
   storage and call `restore_persisted_connections`. The fake factory is
   not called for the archived Printer, the backfill omits it, and
   `list_printers` still returns it with the same `id`, `name`, and
   `createdAt`.
5. **Unarchive:** supervision restarts, since the fake factory was called
   again.
6. **Unarchive into a conflict:**
   - Archive A (host X).
   - Create B with host X; this succeeds, because archived Printers don't
     reserve their host.
   - Unarchive A. It fails with `DUPLICATE_HOST`, and
     `details.conflictingPrinterId == B`.
7. **Delete of an archived Printer:** succeeds.
   - The ordering is observable: the row is gone before the supervisor is
     stopped. Assert it with an instrumented fake as the existing delete
     tests do, or at minimum that the cleanup row was enqueued and then
     processed.
   - The credential is removed from the store.

Run `cargo test --test p2_lifecycle`. Expected: FAIL (commands missing).

- [ ] **Step 2: Implement the lifecycle types, the `ArchiveStateBlockers` source, and `evaluate`**

- [ ] **Step 3: Add the repository functions `archive` and `unarchive`**

Signature: `(id, expected_revision) -> Result<StoredPrinter, RepositoryError>`.
Both use `update`-style optimistic concurrency.

- `archive` sets `archived_at = now_rfc3339()`.
- `unarchive` sets it to `None`. The Task 1 duplicate pre-check then applies
  automatically.
- Both reject when the eligibility for their action is blocked, returning
  `RepositoryError::LifecycleBlocked(Vec<LifecycleBlocker>)`, a new variant
  mapped to `CommandError::lifecycle_blocked`.

`delete` evaluates `lifecycle::evaluate` inside its transaction and returns
`LifecycleBlocked` before deleting.

- [ ] **Step 4: Implement the commands**

`archive_printer` runs in this order:
1. Repository archive (a committed transaction).
2. `let _r = manager.reconciliation_guard().await;`
3. `supervise_printer(...)`, which for an archived Printer performs `stop`,
   publishes the removal, and returns `Archived`. If the stop was not
   graceful, push `OperationWarning::supervisor`.

`unarchive_printer` runs repository unarchive, then the guard, then
`supervise_printer`.

`printer_lifecycle_eligibility` runs `storage.read_transaction` and
`evaluate`.

`delete_printer` is unchanged apart from the repository-level guard.

- [ ] **Step 5: Register the three commands**

Add them to `lib.rs` `COMMAND_NAMES` (now 26) and `generate_handler!`, to
`inventory.rs` (array length 26), and to the `CommandContracts` decl lines:

```ts
export type PrinterLifecycleEligibilityRequest = ContractRequest & { id: string };
export type PrinterLifecycleEligibilityResult = CommandSuccess<LifecycleEligibility>;
export type ArchivePrinterRequest = ContractRequest & { id: string; expectedRevision: number };
export type ArchivePrinterResult = CommandSuccess<PrinterMutationResult>;
export type UnarchivePrinterRequest = ContractRequest & { id: string; expectedRevision: number };
export type UnarchivePrinterResult = CommandSuccess<PrinterMutationResult>;
```

Add `visitor.visit::<crate::printers::lifecycle::LifecycleEligibility>();`.
Update `tests/export_contracts.rs` if it asserts the command count.

- [ ] **Step 6: Run the tests**

Run `just test-rust` and `just gen-contracts`. Expected: PASS and no diff.

- [ ] **Step 7: Commit**

Message: `feat: archive Printers and guard deletion by eligibility`.

---

### Task 4: Create with options, `probe_connection`, and safe Connection replacement

**Owner:** Backend (Protocol owner consulted on probe errors)
**Prerequisites:** Task 3.
**Handoff:** Task 5 reuses `create_printer_with` and `probe_error`. Task 9
calls `probe_connection` and the new `create_printer`.

**Files:**
- Create: `src-tauri/src/printers/create.rs`
- Modify: `src-tauri/src/printers/{mod.rs,commands.rs}`
- Modify: `src-tauri/src/connections/commands.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/contracts/inventory.rs`
- Test: `src-tauri/tests/p2_contract_path.rs`

**Interfaces:**
- Produces:

```rust
pub struct CreatePrinterOptions {
    pub name: String,
    pub catalog_ref: CatalogRef,
    pub location: Option<String>,
    pub start_safety: StartSafety,
    pub default_bed_type: Option<String>,
    pub connection: Option<(ConnectionConfig /* credential_ref None */, Option<zeroize::Zeroizing<String>>)>,
}
pub struct CreateOutcome { pub printer: StoredPrinter, pub credential_stored: bool, pub warnings: Vec<OperationWarning> }

/// Validates, writes the credential (provisional → secret → row commit),
/// then supervises. Never starts supervision before commit.
pub async fn create_printer_with<R: tauri::Runtime>(services: &RuntimeServices<R>, options: CreatePrinterOptions) -> Result<CreateOutcome, CommandError>;

pub fn validate_name(name: &str) -> Result<String, CommandError>;           // trimmed, 1..=128, no control chars; fieldPath "name"
pub fn validate_location(value: Option<&str>) -> Result<Option<String>, CommandError>; // trimmed, empty→None, ≤128; fieldPath "location"
pub fn probe_error(error: ConnectionError, adapter_kind: &str, entity: Option<&str>) -> CommandError; // extracted from test_printer_connection
pub async fn probe_submission<R: tauri::Runtime>(manager: &ConnectionManager<R>, config: &ConnectionConfig, secret: Option<zeroize::Zeroizing<String>>) -> Result<ProbeResult, CommandError>;
```

- Command `create_printer(name, catalogRef, location?, startSafety?, defaultBedType?, connection?: ConnectionSubmission)`.
- Command `probe_connection(submission) -> ProbeResult`.
- `set_printer_connection` gains `accept_unverified: Option<bool>`.
- `PrinterPatch` gains:
  - `#[serde(default, deserialize_with = "double_option")] location: Option<Option<String>>`,
    which TypeScript sees as `location?: string | null`.
  - `start_safety: Option<StartSafety>`.

- [ ] **Step 1: Write failing tests in `tests/p2_contract_path.rs`**

Use a fake factory whose probe returns `Ok(ProbeResult{..})` for host
`"ok.local"`, `Err(ConnectionError::Auth)` for `"auth.local"`, and
`Err(ConnectionError::Timeout)` for `"slow.local"`.

1. **`probe_connection` has no side effects:** with a submission to
   `ok.local` carrying credential `"s3cret-FIXTURE"`, the command returns a
   ProbeResult, and the storage file bytes are identical before and after.
   Hash the DB file after a `PRAGMA wal_checkpoint(TRUNCATE)` on both sides,
   or compare `SELECT * FROM printers` plus
   `pending_credential_cleanup` row dumps. Also check:
   - The credential store directory is unchanged.
   - `manager.statuses()` is unchanged.
   - No `farm3d-event-v1` event was emitted. Listen with a mock app
     listener.
2. **`probe_connection` to `auth.local`** returns `AUTHENTICATION_FAILED`,
   and the error JSON does not contain `s3cret-FIXTURE`.
3. **`create_printer` Profile-only** with location `" Bay A "` stores
   `"Bay A"`, `startSafety` `confirmBedClear`, `setupGaps
   ["missingConnection"]`, and the status is `setupIncomplete`.
4. **`create_printer` with a connection and credential:**
   - The Printer has `credentialRef` matching `farm3d/credential/<uuid>`.
   - The store holds the secret.
   - The supervisor started after the row exists. The fake factory records
     that `repository.get(id)` was `Some` at call time.
   - No `provisional` cleanup row remains.
5. **`create_printer` with `defaultBedType`:** if it differs from the
   catalog default, it's stored in `overrides`; if equal, overrides stay
   empty.
6. **`create_printer` with an empty or whitespace name** → `VALIDATION`
   with `fieldPath "name"`.
7. **`create_printer` with a duplicate active host** → `DUPLICATE_HOST`.
   Nothing is persisted, and the provisional credential is queued for
   cleanup and then removed.
8. **Replacing a working connection with `auth.local`:**
   - `set_printer_connection` → `AUTHENTICATION_FAILED`; the stored
     connection is unchanged, and the revision is unchanged.
   - Retrying with `acceptUnverified: true` succeeds.
9. **First-time `set_printer_connection` to `slow.local`** succeeds without
   probing. The factory's probe count is unchanged.
10. **`update_printer`:** `patch {location: null}` clears the location, and
    `patch {startSafety: "unattended"}` persists it.

Run the tests. Expected: FAIL.

- [ ] **Step 2: Extract `probe_error` and `probe_submission`**

Move the `ConnectionError → CommandError` match out of
`test_printer_connection` into `probe_error`. `probe_submission` builds the
connection through `manager.connection_for(config, secret)`. If the factory
returns `None`, it returns `unsupported_adapter`. Rewire
`test_printer_connection` onto both, with no behaviour change, and confirm
the existing tests pass.

- [ ] **Step 3: Implement `probe_connection`**

It validates host and port as `test_printer_connection` does, trims the
credential, and ignores an empty credential. It calls `probe_submission`.
It must not take `with_credential_coordination`, read `PrinterRepository`,
or read `services.credentials`.

- [ ] **Step 4: Implement `create_printer_with`**

It runs in this order:

1. Validate the name and location, resolve `catalog_ref` (`catalogRef`
   validation error), and set `last_known_good` as today's `create_printer`
   does.
2. If `default_bed_type` is `Some(v)` and `v != variant.default_bed_type`,
   set `overrides.default_bed_type`.
3. If there's a connection:
   - Reject `kind != MOONRAKER_KIND` with `unsupported_adapter`.
   - Validate host and port.
   - Pre-check duplicates with
     `repository.find_active_by_host_identity` → `CommandError::duplicate_host`.
4. Under `with_credential_coordination`:
   - If there's a secret:
     1. `reference = format!("farm3d/credential/{}", uuid)`.
     2. `enqueue_credential_cleanup(reference, None, "provisional")`.
     3. `store.set(reference, secret)`, which on failure returns
        `CommandError::credential_unavailable(kind)`.
     4. `config.credential_ref = Some(reference)`.
   - `repository.create_in(printer, provisional_reference)`, a new variant
     of `create` that also deletes the provisional cleanup row in the same
     transaction, mirroring `set_connection`.
   - On create failure, leave the provisional row, so startup cleanup
     removes the orphan secret. Call `retry_pending_credential_cleanup` and
     return the error.
5. After commit: `reconciliation_guard`, then `supervise_printer`, mapping
   `CredentialRequired` to a warning.

The `create_printer` command builds `CreatePrinterOptions` from its args.
The `connection` submission's `credential` follows today's rules: `None` or
`""` means no credential.

- [ ] **Step 5: Add replacement validation to `set_printer_connection`**

Replacement validation runs only when the existing connection is `Some`
**and** differs from the submission (kind/host/port/useTls, or a new
secret), and `accept_unverified != Some(true)`. In that case:

1. Build the candidate config.
2. Resolve the secret: the submitted one, else the stored one through
   `credentials.get`.
3. `probe_submission`. On `Err`, return that error before touching storage.

The duplicate-host check comes from the repository (Task 1).

Update the `SetPrinterConnectionRequest` decl to add
`acceptUnverified?: boolean`.

- [ ] **Step 6: Extend `PrinterPatch` and `update_printer`**

`update_printer` now requires at least one of `name`, `notes`, `location`,
or `startSafety`. It validates the name (when present) and the location.

Add the `double_option` helper:

```rust
fn double_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Ok(Some(Option::deserialize(d)?))
}
```

Use `#[ts(optional, type = "string | null")]` on `location`.

- [ ] **Step 7: Register `probe_connection` and update the decls**

The command count becomes 27. Update the decls:

```ts
export type CreatePrinterRequest = ContractRequest & { name: string; catalogRef: CatalogRef; location?: string; startSafety?: StartSafety; defaultBedType?: string; connection?: ConnectionSubmission };
export type ProbeConnectionRequest = ContractRequest & { submission: ConnectionSubmission };
export type ProbeConnectionResult = CommandSuccess<ProbeResult>;
```

Add `visitor.visit::<crate::printers::StartSafety>();`.

- [ ] **Step 8: Run the tests**

Run `just test-rust` and `just gen-contracts`. Expected: PASS and no diff.

- [ ] **Step 9: Commit**

Message: `feat: create Printers with options and probe without saving`.

---

### Task 5: Batch creation with bounded probing, partial commits, and cancellation

**Owner:** Wiring owner (primary), Backend
**Prerequisites:** Task 4.
**Handoff:** Task 10 calls `create_printers_batch` and `cancel_printer_batch`.

**Files:**
- Create: `src-tauri/src/printers/batch.rs`
- Create: `src-tauri/tests/p2_batch.rs`
- Modify: `src-tauri/src/printers/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/contracts/inventory.rs`
- Modify: `src-tauri/Cargo.toml` (add `futures` only if it isn't already a
  dependency; check with `cargo tree -p futures`)

**Interfaces:**
- Produces the ts-rs types. Each is `#[serde(rename_all = "camelCase")]`,
  exported under `command/`:

```rust
pub struct CreatePrintersBatchInput { pub batch_id: String, pub shared: BatchShared, #[ts(optional)] pub shared_credential: Option<String>, pub probe: bool, pub rows: Vec<BatchRowInput> }
pub struct BatchShared { pub catalog_ref: CatalogRef, #[ts(optional)] pub default_bed_type: Option<String>, pub start_safety: StartSafety }
pub struct BatchRowInput { pub row_id: String, pub name: String, #[ts(optional)] pub location: Option<String>, #[ts(optional)] pub connection: Option<BatchRowConnection> }
pub struct BatchRowConnection { pub kind: String, pub host: String, pub port: u16, #[serde(default)] pub use_tls: bool, pub credential: BatchCredentialSource }
#[serde(tag = "source", rename_all = "camelCase")] pub enum BatchCredentialSource { None, Shared, Row { value: String } }
pub struct CreatePrintersBatchOutput { pub batch_id: String, pub rows: Vec<BatchRowResult> }
pub struct BatchRowResult { pub row_id: String, pub outcome: BatchRowOutcome, #[ts(optional)] pub printer: Option<ResolvedPrinter>, pub credential_stored: bool, #[ts(optional)] pub probe: Option<ProbeResult>, pub errors: Vec<BatchRowError>, pub warnings: Vec<BatchRowWarning> }
pub enum BatchRowOutcome { Created, CreatedSetupIncomplete, Rejected, Cancelled }
pub struct BatchRowError { pub code: BatchRowErrorCode, #[ts(optional)] pub field_path: Option<String>, pub message: String, #[ts(optional)] pub conflicting_printer_id: Option<String>, #[ts(optional)] pub conflicting_row_id: Option<String> }
#[serde(rename_all = "SCREAMING_SNAKE_CASE")] pub enum BatchRowErrorCode { Validation, DuplicateHost, UnsupportedAdapter, AuthenticationFailed, PrinterUnreachable, Timeout, ProtocolError, CredentialUnavailable, PersistenceUnavailable, Cancelled }
pub struct BatchRowWarning { pub code: BatchRowWarningCode, pub message: String }
#[serde(rename_all = "SCREAMING_SNAKE_CASE")] pub enum BatchRowWarningCode { CapabilityMismatch, DuplicateName, CredentialCleanupPending, SupervisorReconciliationFailed }
```

- `BatchRegistry` is a process-wide registry:
  `OnceLock<Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>>>`,
  with `register(batch_id) -> Result<watch::Receiver<bool>, CommandError>`
  (returns `CONFLICT` if the id is present), `cancel(batch_id)`, and
  `finish(batch_id)`. `finish` is called on a drop guard.
- Secrets in the request (`shared_credential`, `Row.value`) are moved into
  `Zeroizing` immediately on entry. Give the request types a manual `Debug`
  impl that redacts them.

- [ ] **Step 1: Write failing integration tests in `tests/p2_batch.rs`**

Use the fake factory from Task 4, plus a `"gate.local"` host whose probe
waits on a `tokio::sync::Notify`, and an `AtomicUsize` in-flight gauge that
records the maximum concurrency.

1. **Partial success:** rows `r1` (ok.local, shared credential), `r2`
   (auth.local), `r3` (name `""`), and `r4` (no connection).
   - `r1` is `created`, with `credentialStored=true`.
   - `r2` is `createdSetupIncomplete`, with errors
     `[AUTHENTICATION_FAILED]`, `printer.connection` null, and the printer
     persisted.
   - `r3` is `rejected`, with `[VALIDATION fieldPath "rows[2].name"]`, and
     is not persisted.
   - `r4` is `createdSetupIncomplete` with no errors.
   - Results come back in request order, and `list_printers` has exactly
     3 new rows.
2. **Correlation:** duplicate `rowId`s or an empty `rowId` make the whole
   request fail with `VALIDATION`, and nothing is persisted.
3. **In-batch duplicate host:** `r1` and `r2` both use `ok.local:7125`.
   - `r1` is `created`.
   - `r2` is `createdSetupIncomplete` with
     `[DUPLICATE_HOST conflictingRowId "r1"]`.
4. **DB duplicate host:** an existing active Printer on `ok.local` makes the
   row `createdSetupIncomplete` with
   `[DUPLICATE_HOST conflictingPrinterId <id>]`.
5. **Bounded concurrency:** 10 `gate.local:<distinct ports>` rows. Assert
   `max_in_flight == 4`, then release the gate.
6. **Cancellation:**
   - 6 `gate.local` rows. Release one, wait for its commit, call
     `cancel_printer_batch`, and release the rest.
   - The committed row stays; the other 5 are `cancelled` with
     `[CANCELLED]` and not persisted.
   - Cancelling an unknown id returns success.
   - A second request with a running `batchId` gets `CONFLICT`.
7. **Redaction:**
   - Shared credential `"SHARED-s3cret-FIXTURE"` and row credential
     `"ROW-s3cret-FIXTURE"`.
   - Serialize the whole result, the error JSON from test 2, and every
     warning, and scan the DB file bytes, the WAL, and the test's captured
     stderr. Use the existing secret-scan helper in `f1_contract_path.rs`
     if there is one; otherwise read the files.
   - Neither secret appears. The credential store holds one entry per
     created connected Printer, and their refs are distinct.
8. **Independence of shared bed:**
   - With `defaultBedType` set, each created Printer has its own override.
   - After `set_printer_override(one, "defaultBedType", null)`, the others
     still have it.
9. **Supervision:** created connected rows get started (fake factory
   subscribe called). `createdSetupIncomplete` rows are reconciled
   `setupIncomplete`.
10. **Capability mismatch:** a probe reporting `bed_width_mm` 10 mm off the
    profile gives a `CAPABILITY_MISMATCH` warning and the row is still
    `created`. Implement the 1 mm tolerance rule in Rust as
    `fn capability_mismatches(profile, reported) -> Vec<String>`, mirroring
    the frontend `buildMismatches`.

Run the tests. Expected: FAIL.

- [ ] **Step 2: Implement validation (phase 1, no I/O)**

- Check `rowId` uniqueness first; it fails the whole request.
- Resolve `shared.catalog_ref` once. If it's unresolvable, every row is
  `rejected` with `VALIDATION fieldPath "shared.catalogRef"`.
- Per row, run `validate_name` / `validate_location`. Failure means
  `Rejected`, with `fieldPath` `rows[i].name` / `rows[i].location`.
- Per-row connection checks:
  - The kind must be moonraker; otherwise `UNSUPPORTED_ADAPTER`.
  - Host non-empty and port > 0; otherwise `VALIDATION`.
  - Compute the canonical identity and check it against earlier rows
    (`conflictingRowId`) and against `find_active_by_host_identity`
    (`conflictingPrinterId`).
  - A `Shared` source with no `shared_credential` → `VALIDATION`
    (`rows[i].connection.credential`).
  - A connection error drops the connection and marks the row
    `setupIncomplete`-bound, with the error kept.
- A `DUPLICATE_NAME` warning applies when the name equals an existing
  Printer or an earlier row, compared case-insensitively.

- [ ] **Step 3: Implement probe and commit (phases 2 and 3)**

```rust
let mut cancel = registry.register(&request.batch_id)?; // guard calls finish on drop
let results = futures::stream::iter(planned.into_iter().enumerate())
    .map(|(index, row)| async move {
        if *cancel_rx.borrow() { return (index, cancelled(row)); }
        let probe = match (&row.connection, request_probe) {
            (Some(conn), true) => tokio::select! {
                result = probe_submission(&services.manager, &conn.config, conn.secret.clone()) => Some(result),
                _ = wait_cancelled(cancel_rx.clone()) => return (index, cancelled(row)),
            },
            _ => None,
        };
        if *cancel_rx.borrow() { return (index, cancelled(row)); }
        (index, commit_row(services, &shared, row, probe).await)
    })
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
// sort by index to return request order
```

`commit_row` works as follows:

- A probe `Err(e)` maps to a `BatchRowError` through `probe_error(e, ..)`
  (convert its `ErrorCode` to `BatchRowErrorCode`), and the row is created
  **without** a connection.
- A probe `Ok(p)` keeps the connection and adds mismatch warnings.
- The row then goes through `create_printer_with`.
  - An error there maps to `DUPLICATE_HOST`, `CREDENTIAL_UNAVAILABLE`, or
    `PERSISTENCE_UNAVAILABLE`.
  - For `DUPLICATE_HOST` and `CREDENTIAL_UNAVAILABLE`, retry once without
    the connection, so the row becomes `createdSetupIncomplete`.
  - For `PERSISTENCE_UNAVAILABLE`, the row is `Rejected`.
- Rows planned without a connection skip probing.
- The outcome is `Created` when the committed Printer has a connection;
  otherwise it's `CreatedSetupIncomplete`.

`wait_cancelled` loops on `rx.changed()` until the value is true.

`cancel_printer_batch(batch_id)` calls `registry.cancel`, which is a no-op
for unknown ids.

- [ ] **Step 4: Register both commands**

The count becomes 29.

The `CommandContracts` decl already uses the names `*Request` and `*Result`
for the Tauri argument wrappers. To avoid a clash, rename the ts-rs structs
from the Interfaces block: `CreatePrintersBatchRequest` becomes
**`CreatePrintersBatchInput`**, and `CreatePrintersBatchResult` becomes
**`CreatePrintersBatchOutput`**. Use these names everywhere, including
Task 8. The command takes one argument, `input: CreatePrintersBatchInput`.

`cancel_printer_batch` returns an empty `CancelPrinterBatchData {}` struct,
exported to `command/CancelPrinterBatchData.ts`.

```ts
export type CreatePrintersBatchRequest = ContractRequest & { input: CreatePrintersBatchInput };
export type CreatePrintersBatchResult = CommandSuccess<CreatePrintersBatchOutput>;
export type CancelPrinterBatchRequest = ContractRequest & { batchId: string };
export type CancelPrinterBatchResult = CommandSuccess<CancelPrinterBatchData>;
```

- [ ] **Step 5: Run the tests**

Run `just test-rust` and `just gen-contracts`. Expected: PASS and no diff.

- [ ] **Step 6: Commit**

Message: `feat: batch-create Printers with bounded probing and retryable rows`.

---

### Task 6: Printers export/import v2

**Owner:** Backend
**Prerequisites:** Task 1. (It can run in parallel with Tasks 2–5, but
touches `printers/commands.rs`, so sequence it after Task 5 when only one
backend worker is available.)
**Handoff:** None.

**Files:**
- Modify: `src-tauri/src/printers/commands.rs` (`export_printer`,
  `parse_printers_document`, `import_printers`)
- Modify: `src-tauri/src/printers/repository.rs` (`replace_all`)
- Modify: `src-tauri/tests/f1_import_export.rs`
- Create: `src-tauri/tests/fixtures/persistence/v2/printers-export.json`

- [ ] **Step 1: Write failing tests**

1. Export writes `schemaVersion: 2`, and each Printer has `location`,
   `startSafety`, and `archivedAt` (null when unset).
2. A v2 fixture round-trips.
3. A v1 fixture still imports, with location null, `confirmBedClear`, and
   not archived.
4. `schemaVersion: 3` → `UNSUPPORTED_SCHEMA_VERSION`.
5. An import with two active Printers on the same host imports the later
   one (in document order) archived and returns a warning. Reuse the
   migration's grouping logic, factored into
   `printers::host_identity::archive_duplicates(&mut [StoredPrinter]) -> Vec<(kept, archived)>`.
6. An invalid location in the document → `VALIDATION`.

- [ ] **Step 2: Implement**

- Accept `version == 1 || version == 2`.
- The allowed row `FIELDS` are extended with `location`, `startSafety`, and
  `archivedAt`, which are optional for both versions and defaulted by serde.
- Export with version 2.
- In `import_printers`, call `supervise_printer` for each stored Printer
  (replacing the Task 2 edit), and map the duplicate-archive results to a
  new `OperationWarningCode::DuplicateHostArchived` warning with
  `entity_id`.

- [ ] **Step 3: Run the tests**

Run `just test-rust` and `just gen-contracts`. Expected: PASS and no diff.

- [ ] **Step 4: Commit**

Message: `feat: round-trip Printer lifecycle fields through export v2`.

---

### Task 7: `Stepper` and `Textarea` design-system primitives

**Owner:** Frontend
**Prerequisites:** None. This can run in parallel with Tasks 1–6.
**Handoff:** Tasks 9–11 use them.

**Files:**
- Create: `src/design-system/components/{Stepper,Textarea}.{tsx,module.css}`
- Modify: `src/design-system/components/{index.ts,components.test.tsx}`
- Modify: `src/design-system/Showcase.tsx`

Read `DESIGN.md` first, then an existing pair such as `TextField.tsx` /
`TextField.module.css`.

**Interfaces:**

```ts
export interface StepperStep { id: string; label: string; state?: "complete" | "error" }
export interface StepperProps { steps: StepperStep[]; current: string; onSelect?: (id: string) => void; "aria-label"?: string }
// Renders <ol>; current step has aria-current="step"; only complete steps are buttons (keyboard reachable) when onSelect is set.

export interface TextareaProps { label: string; value: string; onChange: (value: string) => void; rows?: number; placeholder?: string; description?: string; errorMessage?: string; class?: string }
// Built on @kobalte/core/text-field with TextField.TextArea (check node_modules/@kobalte/core/src/text-field/ for the exact export name).
```

- [ ] **Step 1: Write failing tests in `components.test.tsx`**

- The Stepper marks the current step `aria-current="step"`.
- Clicking a complete step calls `onSelect("identify")`.
- Incomplete future steps are not buttons.
- The Textarea renders its label, typing calls `onChange`, and
  `errorMessage` renders with `aria-invalid`.

Run `npm test -- components`. Expected: FAIL.

- [ ] **Step 2: Implement both components with token-only CSS**

Use `--f3d-color-*`, `--f3d-type-*`, and `--f3d-radius-*`, with no shadows.
Export them from `index.ts` and add a Showcase section for each.

- [ ] **Step 3: Run the checks**

Run `just test` and `just build`. Expected: PASS.

- [ ] **Step 4: Commit**

Message: `feat: add Stepper and Textarea primitives`.

---

### Task 8: Frontend state, host identity mirror, and batch intake

**Owner:** Frontend (Wiring reviews the parity test)
**Prerequisites:** Tasks 3–5 (generated contracts).
**Handoff:** Tasks 9–11.

**Files:**
- Create: `src/printers/host-identity.ts` and its test
- Create: `src/printers/batch-intake.ts` and its test
- Modify: `src/printers/{types.ts,printer-store.ts,printer-store.test.ts}`

**Interfaces:**

```ts
// host-identity.ts
export function canonicalHostIdentity(host: string, port: number): string | null;

// batch-intake.ts
export interface BatchRowDraft {
  rowId: string; name: string; location: string;
  host: string; port: number | null; protocol: string; useTls: boolean;
  credential: { source: "none" } | { source: "shared" } | { source: "row"; value: string };
  selected: boolean;
  printerId?: string;                     // set once created; retries target it
  result?: BatchRowResult;                // latest result for this row
}
export interface IntakeIssue { line: number; message: string }
export interface IntakeResult { rows: BatchRowDraft[]; errors: IntakeIssue[]; warnings: IntakeIssue[] }
export function parseIntake(text: string, newRowId: () => string): IntakeResult;
export function generateRows(opts: { quantity: number; pattern: string; locations: string[]; startAt?: number }, newRowId: () => string): BatchRowDraft[];
export type CandidateMatch = { kind: "assignable"; rowId?: string } | { kind: "alreadyConfigured"; printerId: string } | { kind: "ambiguous"; rowIds: string[] } | { kind: "unsupported" };
export function mapDiscovery(candidates: DiscoveredPrinter[], rows: BatchRowDraft[], existing: ResolvedPrinter[]): Map<string /* candidate key host:port */, CandidateMatch>;
export function defaultPort(protocol: string): number; // moonraker 7125, octoprint 80
export function toBatchInput(rows: BatchRowDraft[], shared: BatchShared, sharedCredential: string | undefined, probe: boolean, batchId: string): CreatePrintersBatchInput;

// printer-store.ts additions
export async function probeCandidate(submission: ConnectionSubmission): Promise<ProbeResult>; // rejects
export async function createPrinter(options: CreatePrinterOptions): Promise<ResolvedPrinter | undefined>; // banner on error, like addPrinter
export async function createPrintersBatch(input: CreatePrintersBatchInput): Promise<CreatePrintersBatchOutput>; // rejects; merges created printers into the store
export async function cancelBatch(batchId: string): Promise<void>;
export async function archivePrinter(id: string): Promise<void>;
export async function unarchivePrinter(id: string): Promise<void>;
export async function lifecycleEligibility(id: string): Promise<LifecycleEligibility>; // rejects
export async function setConnection(id: string, submission: ConnectionSubmission, acceptUnverified?: boolean): Promise<void>; // extended; now rejects so callers can offer "Save anyway"
// types.ts
export interface CreatePrinterOptions { name: string; catalogRef: CatalogRef; location?: string; startSafety?: StartSafety; defaultBedType?: string; connection?: ConnectionSubmission }
```

`addPrinter` / `PrinterDraft` are removed in Task 9 once the wizard replaces
them.

- [ ] **Step 1: Write the host-identity parity test**

Import the Rust fixture with
`import vectors from "../../src-tauri/tests/fixtures/host-identity.json"`.
Check that `tsconfig` has `resolveJsonModule`; if it doesn't, read the file
with `fs` in the test. Assert every vector. Expected: FAIL.

- [ ] **Step 2: Implement `canonicalHostIdentity`, mirroring the Rust function**

IPv6 canonical compression isn't built into JavaScript. Use
`new URL("http://[" + h + "]").hostname`, which returns compressed,
lowercased IPv6, and detect IPv4 with a strict dotted-quad regex. Run the
test. Expected: PASS.

- [ ] **Step 3: Write failing `batch-intake` tests**

**`parseIntake`:**
- A TSV header `name\tlocation\thost` with 2 rows.
- CSV with a quoted comma (`"Voron, A",Bay A,v1.local`).
- A missing `name` header → error.
- A `tls` of `yes` → true.
- A bad port → line error, and the other lines survive.
- An unknown column → one warning.
- An `api_key` column → an error, and **no rows** are returned for that
  input.
- Blank lines are skipped.
- `protocol` defaults to `moonraker`, and the port defaults per protocol
  when a host is set.

**`generateRows`:**
- `{quantity: 2, pattern: "Voron {nn}", locations: ["Bay A", "Bay B"]}` →
  `Voron 01`/`Voron 02` in Bay A and `Voron 03`/`Voron 04` in Bay B.
- A pattern with no `{n}` gets ` {n}` appended.
- `quantity` 0 → `[]`.

**`mapDiscovery`:**
- A candidate matching an existing Printer → `alreadyConfigured`.
- A candidate matching two rows → `ambiguous`.
- An `octoprint` candidate → `unsupported`.
- A candidate whose `addresses[1]` matches a row's host → `assignable`
  with that `rowId`.

**`toBatchInput`:**
- Rows with an empty host have no `connection`.
- A row credential `value` passes through.
- The shared credential is included only when provided.

Expected: FAIL.

- [ ] **Step 4: Implement `batch-intake.ts`**

It is pure, with no Solid imports. Run the tests. Expected: PASS.

- [ ] **Step 5: Write failing store tests and implement the store actions**

Follow the existing `printer-store.test.ts` `invoke` mock pattern, and
cover:

- `createPrinter` sends the new args and splices in the result.
- `createPrintersBatch` merges every returned `printer` into the store and
  returns the output unchanged.
- `archivePrinter` replaces the record.
- `setConnection` rejects on error and passes `acceptUnverified`.
- In web mode (`desktopAvailable` false):
  - `createPrinter` appends locally.
  - `createPrintersBatch` returns `createdSetupIncomplete` for valid rows,
    with local records.
  - `probeCandidate` rejects with "needs the desktop app".
  - `archivePrinter` sets `archivedAt` locally.

- [ ] **Step 6: Run the checks**

Run `just test` and `just build`. Expected: PASS.

- [ ] **Step 7: Commit**

Message: `feat: add P2 printer store actions and batch intake`.

---

### Task 9: Single Printer setup wizard

**Owner:** Frontend
**Prerequisites:** Tasks 7 and 8.
**Handoff:** Task 10 reuses `ConnectionFields`, and Task 11 reuses its
existing-Printer mode.

**Files:**
- Create: `src/screens/ConnectionFields.{tsx,module.css,test.tsx}`
- Create: `src/screens/PrinterSetupWizard.{tsx,module.css,test.tsx}`
- Modify: `src/screens/PrinterConnectionPanel.{tsx,test.tsx}`
- Modify: `src/screens/PrinterDashboard.{tsx,test.tsx}`
- Modify: `src/App.tsx`
- Delete: `src/screens/PrinterAddDialog.{tsx,module.css,test.tsx}`, after
  porting its tests into the wizard test
- Modify: `src/printers/{types.ts,printer-store.ts}` to remove
  `PrinterDraft`/`addPrinter` once nothing uses them

**Interfaces:**

```ts
export interface ConnectionDraft { kind: string; host: string; port: number; useTls: boolean; credential: string }
export interface ConnectionFieldsProps {
  value: ConnectionDraft; onChange: (next: ConnectionDraft) => void;
  suggestedKind?: string;
  onTest: () => Promise<ProbeResult>;          // wizard: probeCandidate; dock: testConnection(id, …)
  profile?: PrinterProfile;                     // for buildMismatches
  credentialHint?: string;                      // "leave blank to keep the stored key" in dock mode
}
export function ConnectionFields(props: ConnectionFieldsProps): JSX.Element; // owns discovery list, Test button, ProbeResult chip, mismatch list
export function toSubmission(draft: ConnectionDraft, mode: "create" | "edit"): ConnectionSubmission; // create: blank credential → omitted; edit: blank → omitted (keep), explicit clear handled by panel

export interface PrinterSetupWizardProps {
  open: boolean; onOpenChange: (open: boolean) => void;
  prefillModel?: { vendor: string; model: string };
  existingPrinters: ResolvedPrinter[];
  onCreated?: (printer: ResolvedPrinter) => void;
}
```

- [ ] **Step 1: Extract `ConnectionFields` from `PrinterConnectionPanel`**

Move the kind/host/port/key fields, discovery, Test, the ProbeResult chip,
and `buildMismatches` into the new component. `PrinterConnectionPanel`
composes it with Save and Disconnect. Every existing
`PrinterConnectionPanel` test must pass unchanged, which is the refactor
safety net. Add a `ConnectionFields.test.tsx` covering:

- A discovery click fills kind, host, and port.
- Test shows the `online` chip.
- Test failure shows the error text inline.

Run `just test`. Expected: PASS.

- [ ] **Step 2: Write failing wizard tests**

Port every `PrinterAddDialog.test.tsx` case to the Identify step. New
cases:

1. The Stepper shows Identify, Connect, Operate, and Review, with Identify
   current.
2. Next is disabled until the model, nozzle, and name are valid.
3. The location input is trimmed in the review summary.
4. **Connect:**
   - "Skip — save Profile-only" goes to Operate.
   - Review shows the notice "This Printer will be Setup incomplete until
     it has a Connection".
5. **Connect Test** calls the mocked `probeCandidate` with the draft
   submission, and `createPrinter` has **not** been called.
6. **Operate:**
   - The start-safety radio defaults to "Confirm the bed is clear before
     each start".
   - The bed-type Select lists the variant bed types. Open it with
     `pointerDown`.
7. **Review:**
   - Shows the mismatches from the last probe and the credential-store
     location (mocked `credentialStoreInfo`).
   - Save calls `createPrinter` once with
     `{name, catalogRef, location, startSafety, defaultBedType, connection}`.
   - The dialog closes and `onCreated` fires.
8. **Save failure:** the dialog stays on Review, and nothing is lost.
9. **Keyboard:** Tab reaches Next, and Enter advances.

Expected: FAIL.

- [ ] **Step 3: Implement `PrinterSetupWizard`**

Keep the Identify logic from `PrinterAddDialog`: `suggestUniqueName`,
`stripBrandPrefix`, and nozzle defaulting. Move the pure helpers into
`src/screens/printer-identity.ts` if both wizards need them; the batch
Shared step does.

Use the design-system `Dialog`, `Stepper`, `TextField`, `Combobox`,
`Select`, and `RadioGroup`.

Wire it in `PrinterDashboard` wherever `PrinterAddDialog` was, then delete
`PrinterAddDialog.*`.

- [ ] **Step 4: Run the checks**

Run `just test` and `just build`. Expected: PASS.

- [ ] **Step 5: Commit**

Message: `feat: replace the add dialog with the Printer setup wizard`.

---

### Task 10: Batch setup dialog

**Owner:** Frontend (Wiring reviews row correlation and retry)
**Prerequisites:** Task 9.
**Handoff:** Task 11 adds the toolbar entry point.

**Files:**
- Create: `src/screens/PrinterBatchDialog.{tsx,module.css,test.tsx}`
- Create: `src/screens/BatchRowsTable.{tsx,module.css,test.tsx}`

**Interfaces:**

```ts
export interface PrinterBatchDialogProps { open: boolean; onOpenChange: (open: boolean) => void; existingPrinters: ResolvedPrinter[] }
export interface BatchRowsTableProps {
  rows: BatchRowDraft[];
  onChange: (rowId: string, patch: Partial<BatchRowDraft>) => void;
  onToggle: (rowId: string, selected: boolean) => void;
  onToggleAll: (selected: boolean) => void;
  onRemove: (rowId: string) => void;
  mode: "edit" | "connect" | "results";
  preview: { defaultBedType?: string; startSafety: StartSafety };  // shown per row (D9)
}
```

- [ ] **Step 1: Write failing tests**

1. **Shared step:** choosing the model, nozzle, bed type, and safety
   enables Next.
2. **Rows step:**
   - Generate `{quantity 2, pattern "Voron {nn}", locations "Bay A, Bay B"}`
     gives 4 rows with correct names and locations.
   - Paste into the Textarea and "Add pasted rows" appends the parsed rows.
   - Intake errors are listed and nothing is appended for a
     credential-column paste.
   - The CSV file input (`fireEvent.change` with a `File`) appends rows.
   - Rows can be edited inline and removed.
3. **Connect step:**
   - Mocked `discoverPrinters` returns 3 candidates: one already
     configured, one ambiguous, and one assignable.
   - Only the assignable one has an Assign control, and assigning fills
     the row's host and port.
   - Select two rows, set protocol/port/TLS/credential source "shared", and
     click "Apply to selected". Only those two rows change.
   - The shared credential field is `type=password` and not pre-filled.
4. **Each row shows its preview** of the copied bed type and safety.
5. **Review & results:**
   - Create calls `createPrintersBatch` with `toBatchInput(...)`: stable
     `rowId`s and a generated `batchId`. The mocked result is
     created / createdSetupIncomplete (AUTHENTICATION_FAILED) / rejected
     (VALIDATION name).
   - The grid shows each outcome with icon, text, and color.
   - The rejected row keeps its typed name, and the failed row keeps its
     host.
6. **Retry failed:**
   - Rejected rows are re-sent in a new batch with the same `rowId`s.
   - The `createdSetupIncomplete` row with a host calls
     `setConnection(printerId, submission)` and **not** the batch.
   - After success, the row outcome updates to created.
7. **Cancel:**
   - While `createPrintersBatch` is pending, Cancel calls
     `cancelBatch(batchId)`.
   - When the result arrives with `cancelled` rows, they are retryable.
8. **Row selection by keyboard:** Space on a row checkbox toggles it.

Expected: FAIL.

- [ ] **Step 2: Implement `BatchRowsTable`**

It's a CSS-module grid with a header row, a row checkbox (design-system
`Checkbox`), inline `TextField`s in edit mode, and outcome cells using
`SeverityMarker`. It scrolls horizontally inside the dialog at narrow
widths.

- [ ] **Step 3: Implement `PrinterBatchDialog`**

- Rows are a local `createStore<BatchRowDraft[]>`. `rowId` comes from
  `crypto.randomUUID()`.
- Keep the shared credential in a signal and clear it when the dialog
  closes.
- Merge results into rows by `rowId`. Set `printerId` from `printer.id` for
  created rows.
- Retry follows test 6.
- Closing the dialog while rows failed asks for confirmation, with a
  design-system `Dialog`, not `window.confirm`.

- [ ] **Step 4: Run the checks**

Run `just test` and `just build`. Expected: PASS.

- [ ] **Step 5: Commit**

Message: `feat: add batch Printer setup with per-row results and retry`.

---

### Task 11: Setup tab lifecycle and Monitor location and archive

**Owner:** Frontend
**Prerequisites:** Tasks 9 and 10.
**Handoff:** Task 12.

**Files:**
- Create: `src/screens/DeletePrinterDialog.{tsx,module.css,test.tsx}`
- Modify: `src/screens/{PrinterSetupPanel,PrinterConnectionPanel,PrinterDetailDock}.{tsx,test.tsx}`
- Modify: `src/screens/{MonitorToolbar,PrinterDashboard}.{tsx,test.tsx}`
- Modify: `src/monitor/monitor-store.{ts,test.ts}`

- [ ] **Step 1: Write failing Monitor store tests**

- `MonitorFilter` gains `"archived"`. Every filter except `archived`
  excludes Printers with `archivedAt`, and `archived` includes only them.
- Counts and rosters exclude archived Printers.
- Section `location` groups by trimmed, case-insensitive location, labeled
  with the first-seen casing. A "No location" group sorts last.
- Search matches location.

Expected: FAIL. Implement it: add `location` and `archived` to
`MonitorPrinterView`, and change `sectionFor`'s `location` branch.

- [ ] **Step 2: Write failing Setup tab tests**

1. The location TextField saves via `updatePrinter(id, {location})`,
   debounced 300 ms like name and notes. Clearing it sends
   `{location: null}`.
2. The start-safety RadioGroup saves `{startSafety}` immediately.
3. **Connection replacement:** when `setConnection` rejects with
   `AUTHENTICATION_FAILED`, the error shows plus "Save anyway". Clicking it
   calls `setConnection(id, submission, true)`.
4. **An active Printer** shows Archive. Clicking it calls `archivePrinter`.
   Delete is not shown, and the eligibility blocker text "Archive this
   Printer before deleting it" is.
5. **An archived Printer** shows Unarchive and Delete…. Delete… opens
   `DeletePrinterDialog`, whose confirm button is disabled until the typed
   name matches exactly. Confirm calls `removePrinter`.
6. **An unarchive failure** with `DUPLICATE_HOST` shows the conflicting
   Printer's name.

Expected: FAIL. Implement it, replacing P1's direct "Remove Printer" danger
button with the guarded flow.

- [ ] **Step 3: Add the Monitor toolbar and first-run entry points**

Add "Add Printers…" next to "Add Printer" in the toolbar and first-run
state, opening `PrinterBatchDialog`. Add the "Archived" filter chip to the
toolbar and a Location option to the section control. Also update
`MonitorToolbar.test.tsx` and `PrinterDashboard.test.tsx`.

- [ ] **Step 4: Run the checks**

Run `just test` and `just build`. Expected: PASS.

- [ ] **Step 5: Commit**

Message: `feat: add location, safety, archive, and guarded delete to Printer setup`.

---

### Task 12: Tracer, docs, and verification evidence

**Owner:** Wiring owner
**Prerequisites:** Tasks 1–11.

**Files:**
- Modify: `CONTEXT.md`
- Modify: `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`
- Create: `docs/verification/2026-09-2x-p2-printer-lifecycle.md`
- Create or modify: `src-tauri/tests/p2_contract_path.rs` (the tracer test)

- [ ] **Step 1: Write the automated tracer test**

It runs through `tauri::test` IPC in one test:

1. Create a Profile-only Printer.
2. Create a connected Printer (fake factory).
3. `create_printers_batch` with 4 rows across "Bay A" and "Bay B", one of
   them `auth.local`, and assert its row is retained as
   `createdSetupIncomplete`.
4. Archive one Printer.
5. Rebuild the services over the same storage (restart).
6. Assert the archived Printer is listed with the same id and name,
   `archivedAt` set, and no status. Every other Printer is present with the
   correct setup state.

- [ ] **Step 2: Update `CONTEXT.md`**

Add Location, Profile-only Printer, Setup incomplete, Start-safety rule, and
Archive, worded as in spec §Product vocabulary. Note that batch "bays" are
location values.

- [ ] **Step 3: Update the approach doc's known-unknowns rows**

Mark "`group` to location migration" resolved (no migration; P2 location is
new data) and "Batch CSV and host matching" resolved (see spec D2/D10/D12).

- [ ] **Step 4: Run the full verification**

Run each of these:

- `just build`
- `just test`
- `source "$HOME/.cargo/env" && just test-rust`
- `just gen-contracts`, then `git diff --exit-code src/generated`

- [ ] **Step 5: Run manual checks with a display**

Run `just dev`. Check the tracer at 1440 × 900 and 1024 × 700, and check
keyboard-only operation through the wizard, the batch dialog, and the Setup
tab. Save screenshots to `docs/screenshots/p2-*.png`. If there's no display
or Moonraker hardware, record those checks as **unavailable**, not passed.

- [ ] **Step 6: Write the verification doc**

Mirror `docs/verification/2026-09-18-p1-shell-monitor.md`: automated
evidence tables mapped to spec acceptance criteria 1–17, manual evidence,
unavailable checks, and the recorded deviation (Monitor default section,
clarification 2).

- [ ] **Step 7: Commit, push, and open a draft PR**

Commit with `docs: record P2 verification evidence`. Push and open a draft
PR referencing #12.

---

## Delivery order and parallel work

```text
Backend:   T1 → T2 → T3 → T4 → T5 → T6
Frontend:  T7 (parallel with T1–T6) → T8 (needs T5 contracts) → T9 → T10 → T11
Wiring:    reviews T3 ordering, T5 correlation/redaction, T8 parity → T12
```

With a single implementer, run them strictly in numeric order.

## External dependencies and blockers

- No Moonraker hardware is assumed. Every probe path is covered by the
  injected connection factory, and the live connected tracer is recorded as
  unavailable if no printer is reachable.
- `futures` may need adding to `src-tauri/Cargo.toml`. Check first.

## Expected deliverables

- Schema v3 with lifecycle columns and the active-host unique index.
- 29 commands, 6 of them new: `probe_connection`, `create_printers_batch`,
  `cancel_printer_batch`, `printer_lifecycle_eligibility`,
  `archive_printer`, and `unarchive_printer`.
- The Printer setup wizard, the batch setup dialog, and the extended Setup
  tab and Monitor.
- Stepper and Textarea primitives.
- `CONTEXT.md` updates and the P2 verification doc.
