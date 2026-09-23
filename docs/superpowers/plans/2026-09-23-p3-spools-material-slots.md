# P3 Spools and Material Slots Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver durable Spool inventory and per-Printer Material Slots,
with these parts:

- Atomic load, swap, and unload.
- Append-only amount and movement history with measured and estimated
  confidence.
- Reusable tares.
- Reservation primitives for P7.
- Slot layouts in single and batch setup.
- Archive with Spool dispositions, and delete that cascades slot history.

**Architecture:** Rust owns persisted truth in a new `src-tauri/src/spools/`
module:

- Six new tables in migration 0004.
- DB-enforced occupancy through a partial unique index on `spools.slot_id`.
- One movement routine, shared by `move_spool`, archive dispositions, and
  initial loads.
- An append-only ledger, protected by triggers.
- Transaction-scoped reservation functions.
- A new `inventory` event stream, plus an in-process broadcast for P7.

Printer code gains `materialSlots`, and a `LoadedSpoolBlockers` lifecycle
source.

The SolidJS side adds:

- Three design-system primitives: `DataTable`, `Timeline`, and `ColorSwatch`.
- A separate `spool-store.ts` with optimistic moves.
- A Spools destination, and a shared `MaterialSlotsEditor` used by the
  wizard, batch Shared, and the Setup tab.
- An archive disposition dialog.

**Tech Stack:** Rust, Tauri 2, rusqlite/SQLite, tokio, ts-rs, SolidJS,
TypeScript, Kobalte, CSS Modules, Vitest, and Rust unit/integration tests.

**Spec:** `docs/superpowers/specs/2026-09-23-p3-spools-material-slots-design.md`
(decisions D1–D13 are cited below by number).

## Global Constraints

**Data and ownership**

- Rust remains persisted truth. The frontend never derives facets,
  availability, or eligibility. It only filters on the facets in
  `SpoolRecord`.
- Amounts are integer milligrams everywhere below the UI. Grams exist only
  in `src/spools/weight.ts` and in display (D1).
- A Spool is in exactly one slot or in storage. The DB enforces this through
  `spools_slot_occupancy` and table CHECKs, not only Rust (D5).
- Every movement goes through `spools::movement`. No other code writes
  `spools.slot_id` or `spool_movements` (D6).
- `spool_amount_events` is append-only, enforced by triggers. The cached
  `spools.current_mg` and `confidence` are written only by `spools::ledger`,
  in the same transaction as the event (D7).
- Reservations are touched only through `spools::reservations` functions
  that take `&Transaction`. P3 exposes no reservation command (D8).
- Slot layout is copied at creation. There is no batch or shared layout
  entity, and batch never loads Spools (D12, user decision 3).
- Archive never leaves a Spool loaded, because dispositions apply in the
  archive transaction (D10, user decision 1).
- Printer delete cascades its slots and every movement row that touches
  them (D10, user decision 2).

**Concurrency and ordering**

- Mutations take `expectedRevision`. A mismatch is `CONFLICT`, following
  `classify_entity_write`.
- `move_spool` also guards the destination with `expectedOccupantSpoolId`,
  and is idempotent by `operationId`.
- Events and the broadcast fire after commit only. They never fire on
  rollback or replay (D11).
- The P2 archive order after commit is unchanged: guard, `stop_and_wait`,
  then `printer.status.removed`.

**Out of scope:** Queue and dispatch, real Job reservations, deferred
reconciliation, Attention Events, AMS auto-topology, slot constraints, Spool
hard-delete, inventory backup, and notifications.

**Repo conventions**

- Use Kobalte primitives for interactive behaviour, CSS Modules, and only
  `--f3d-*` color/type/radius tokens. Follow the editor aesthetic.
- New design-system components go in `components/index.ts` and
  `Showcase.tsx`.
- Kobalte `Select`/`DropdownMenu` tests use `fireEvent.pointerDown` and
  `fireEvent.pointerUp`.
- Never hand-edit `src/generated/contracts/**`. Run `just gen-contracts`.
- Every new command goes in `lib.rs` `COMMAND_NAMES` and
  `generate_handler!`, in `contracts/inventory.rs` `COMMAND_CONTRACTS`
  (update the array length), in the `CommandContracts` decl and visitor,
  and in `src/ipc/client.ts` `CommandMap`.
- Prefix Rust commands with `source "$HOME/.cargo/env" &&` in
  non-interactive shells.

**Validation**

- Frontend is done when `just build` and `just test` pass.
- Rust is done when `just test-rust` passes and `just gen-contracts` leaves
  no diff.

## Known spec clarifications found while planning

1. **Slots on `StoredPrinter`.**
   - `resolve_printer(catalog, stored)` (`catalog/resolve.rs:198`) is pure
     and cannot query the DB.
   - So `StoredPrinter` gains `material_slots: Vec<MaterialSlot>`, filled by
     `PrinterRepository` with one extra query per read (`WHERE removed_at IS
     NULL ORDER BY position`, joined to `spools` for `occupantSpoolId`).
   - `resolve_printer` copies it onto `ResolvedPrinter`.
   - Legacy JSON helpers in `printers/mod.rs` default it to empty. Import
     replaces empty with the default layout (Task 5).
2. **Reordering against the unique position index.** Reordering in place
   would hit `material_slots_live_position` part-way through.
   `set_material_slot_layout` avoids this in two passes: it first sets every
   live slot of the Printer to `position + 100`, then writes the final
   positions. The CHECK allows 0–15, so the migration widens the CHECK to
   `position BETWEEN 0 AND 115`. Rust enforces the real 0–15 range after the
   second pass.
3. **Archive request shape.** `archive_printer` gains two required fields,
   so the frontend `archivePrinter` call in `printer-store.ts` changes in
   the same task (Task 6). It sends `spoolDispositions: []` when no Spool is
   loaded.
4. **"Unresolved movements."** Per D5, P3 has none. The phase plan's
   delete-blocker requirement is met by the `SPOOLS_LOADED` source. The
   verification doc records this mapping.
5. **Spool numbers.** The number is allocated with
   `SELECT COALESCE(MAX(spool_number), 0) + 1` inside the IMMEDIATE create
   transaction. Numbers are never reused, because Spools are never deleted.

## File and module map

### Backend

- **Create** `src-tauri/migrations/0004_p3_spools_material_slots.sql`: the
  spec schema, with the clarification 2 CHECK change.
- **Modify** `src-tauri/src/persistence/migrations.rs`: schema v4, and the
  post-step `backfill_main_slots`.
- **Modify** `src-tauri/src/persistence/mod.rs`: the schema pin tests.
- **Create** `src-tauri/src/spools/`:
  - `mod.rs`: types and validation.
  - `weight.rs`: mg range guards.
  - `repository.rs`
  - `ledger.rs`
  - `tares.rs`
  - `movement.rs`
  - `reservations.rs`
  - `slots.rs`
  - `lifecycle_blockers.rs`
  - `events.rs`
  - `commands.rs`
- **Modify** `src-tauri/src/printers/mod.rs`: `StoredPrinter.material_slots`.
- **Modify** `src-tauri/src/printers/repository.rs`: load slots; archive
  with dispositions.
- **Modify** `src-tauri/src/printers/create.rs`: `slot_layout` and
  `initial_loads` in `CreatePrinterOptions`.
- **Modify** `src-tauri/src/printers/batch.rs`: `BatchShared.slot_layout`.
- **Modify** `src-tauri/src/printers/lifecycle.rs`: new blocker codes, the
  registry entry, and `loaded_spools`.
- **Modify** `src-tauri/src/printers/commands.rs`: the archive request, and
  export/import v3.
- **Modify** `src-tauri/src/catalog/resolve.rs`: `ResolvedPrinter.material_slots`.
- **Modify** `src-tauri/src/contracts/command.rs`: `ErrorCode::SlotOccupied`,
  and a general `lifecycle_blocked` message.
- **Modify** `src-tauri/src/contracts/inventory.rs` and `src-tauri/src/lib.rs`:
  registration, and `RuntimeServices.inventory_changes`.
- **Create** `src-tauri/tests/`:
  - `p3_migration.rs`
  - `p3_movement.rs`
  - `p3_ledger.rs`
  - `p3_reservations.rs`
  - `p3_setup.rs`
  - `p3_lifecycle.rs`
  - `p3_contract_path.rs`
  - `p3_tracer.rs`

### Frontend

- **Create** `src/design-system/components/DataTable/`, `Timeline/`, and
  `ColorSwatch/`, each with its `.tsx`, `.module.css`, and `.test.tsx`.
- **Create** `src/spools/`:
  - `weight.ts`
  - `materials.ts`
  - `facets.ts`
  - `spool-store.ts`
  - `web-fixtures.ts`
  - Tests for each.
- **Create** `src/screens/`:
  - `SpoolInventory.tsx` and `SpoolDetailDock.tsx`.
  - `SpoolFormDialog.tsx`, `RecordAmountDialog.tsx`, `MoveSpoolDialog.tsx`,
    and `TareManagerDialog.tsx`.
  - `MaterialSlotsEditor.tsx` and `ArchivePrinterDialog.tsx`.
  - CSS modules and tests for each.
- **Modify** `src/App.tsx`, `src/screens/ActivityBar.tsx`, and
  `ActivityBar.test.tsx`.
- **Modify** `src/printers/printer-store.ts`: `archivePrinter` dispositions,
  `setSlotLayout`, `createPrinter` options, and web fixture slots.
- **Modify** `src/screens/PrinterStatusPanel.tsx`,
  `PrinterDetailDock.tsx`, `PrinterSetupWizard.tsx`,
  `PrinterBatchDialog.tsx`, `BatchRowsTable.tsx`, and
  `DeletePrinterDialog.tsx`.
- **Modify** `src/ipc/client.ts`: `CommandMap`.

### Docs

- **Modify** `CONTEXT.md`: Tare, Storage label, and Spool number.
- **Modify** `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`:
  mark the P3 unknowns resolved.
- **Create** `docs/verification/2026-09-2x-p3-spools-material-slots.md`,
  plus `docs/screenshots/p3-*.png`.

---

### Task 1: Schema v4 and Spool domain types

**Owner:** Backend
**Prerequisites:** none.
**Handoff:** Tasks 2–6 build on these types and tables.

**Files:**
- Create: `src-tauri/migrations/0004_p3_spools_material_slots.sql`
- Create: `src-tauri/src/spools/mod.rs`
- Create: `src-tauri/src/spools/weight.rs`
- Create: `src-tauri/tests/p3_migration.rs`
- Modify: `src-tauri/src/persistence/migrations.rs`
- Modify: `src-tauri/src/persistence/mod.rs` (the pin tests)
- Modify: `src-tauri/src/lib.rs` (`mod spools;`)

**Interfaces:**
- Produces (every type derives `Serialize, Deserialize, Clone, Debug, TS`
  with camelCase, exported under `domain/`):

```rust
pub enum MaterialFamily { Pla, PlaCf, Petg, PetCf, Abs, Asa, Tpu, Pa, PaCf, Pc, Pva, Hips, Pp, Other }
// serde/ts rename each to the D2 wire string ("PLA", "PLA-CF", ...)
pub enum FilamentDiameter { #[serde(rename = "1.75")] D175, #[serde(rename = "2.85")] D285 }
pub enum AmountConfidence { Measured, Estimated }
pub enum SpoolLifecycle { Active, Empty, Archived }
pub enum SpoolLocation { Slot { slot_id: String, printer_id: String }, Storage { storage_label: Option<String> } } // tag = "kind"
pub struct SpoolFacets { pub loaded: bool, pub reserved: bool, pub low: bool, pub confidence: AmountConfidence }
pub struct Availability { pub current_mg: i64, pub reserved_mg: i64, pub available_mg: i64 } // #[ts(type = "number")] on each
pub struct SpoolRecord {
    pub id: String, pub revision: i64, pub spool_number: i64,
    pub manufacturer: String, pub product: Option<String>,
    pub material_family: MaterialFamily, pub material_other: Option<String>,
    pub color_name: String, pub color_hex: Option<String>, pub diameter: FilamentDiameter,
    pub nominal_mg: i64, pub low_threshold_mg: i64, pub tare_id: Option<String>,
    pub lifecycle: SpoolLifecycle, pub location: SpoolLocation,
    pub availability: Availability, pub facets: SpoolFacets,
    pub last_measured_at: Option<String>, pub notes: Option<String>,
    pub created_at: String, pub updated_at: String,
}
pub struct MaterialSlot { pub id: String, pub position: u8, pub name: String, pub feeder_label: Option<String>, pub occupant_spool_id: Option<String> }
pub struct SpoolFields { /* manufacturer..notes: every user-editable field above */ }
pub fn validate_fields(f: &SpoolFields) -> Result<(), RepositoryError>; // Validation { field_path }
```

- [ ] **Step 1: Confirm the D2 material list**

Fetch `src/libslic3r/PrintConfig.cpp` at OrcaSlicer tag `v2.4.2`:

```sh
curl -sL https://raw.githubusercontent.com/SoftFever/OrcaSlicer/v2.4.2/src/libslic3r/PrintConfig.cpp | grep -n -A60 '"filament_type"'
```

Keep only D2 families present in that option's `enum_values`, plus
`OTHER`. If the list differs from the spec, update the spec's D2 line in
this commit and note the change in the PR.

- [ ] **Step 2: Write failing migration tests in `tests/p3_migration.rs`**

Mirror `tests/p2_migration.rs`. Build a v3 fixture with `apply_through(conn, 3)`
holding two Printers, one of them archived.

1. **Upgrade:** after `Storage::open`, `user_version == 4`, the ledger has
   the 0004 row, and each Printer has exactly one live slot `Main` at
   position 0 with a `slt-` id.
2. **Occupancy constraints** (raw SQL, no Rust checks):
   - Inserting two Spools with the same `slot_id` fails.
   - A Spool with both `slot_id` and `storage_label` fails.
   - An archived Spool with a `slot_id` fails.
   - `OTHER` without `material_other` fails.
3. **Ledger immutability:** `UPDATE` and `DELETE` on `spool_amount_events`
   fail with "append-only".
4. **Cascade:** deleting a Printer removes its `material_slots` and any
   `spool_movements` row whose `from_slot_id` or `to_slot_id` references
   them. `PRAGMA foreign_key_check` is then empty.
5. **Crash boundary:** `apply_through_failing_before_commit` from v3 leaves
   the database byte-identical at v3.

Run `cargo test --test p3_migration`. Expected: FAIL.

- [ ] **Step 3: Write the migration**

Use the spec schema verbatim, with two changes:

- The confirmed family list.
- `position BETWEEN 0 AND 115` (clarification 2).

Add `MIGRATIONS[3]`, and set `CURRENT_SCHEMA_VERSION = 4`. Add the post-step
`backfill_main_slots(tx)`, which inserts `(slt-<uuid>, printer_id, 0,
'Main', NULL, NULL, now)` for every row in `printers`.

- [ ] **Step 4: Update the schema pin tests**

Update `current_schema_inventory_is_exact_and_strict` and
`current_schema_columns_defaults_nullability_and_warning_index_are_exact` in
`persistence/mod.rs` with the six tables, the indexes, and the two triggers.
Rename `printers_schema_has_no_batch_or_later_domain_columns` to assert that
there are still no Job or Queue columns.

- [ ] **Step 5: Implement the domain types and `validate_fields`**

Add unit tests for every D1 and D2 range and trim rule, `OTHER` requiring
`materialOther`, and `colorHex` being uppercase `#RRGGBB` (normalize the case
on input).

- [ ] **Step 6: Run the tests**

Run `just test-rust`. Expected: PASS.

- [ ] **Step 7: Commit**

Message: `feat: add the P3 spool and material slot schema`.

---

### Task 2: Spool repository, amount ledger, and tares

**Owner:** Backend
**Prerequisites:** Task 1.
**Handoff:** Task 3 reuses `load_spool` and `bump_revision`. Task 7 wraps
these functions in commands.

**Files:**
- Create: `src-tauri/src/spools/repository.rs`
- Create: `src-tauri/src/spools/ledger.rs`
- Create: `src-tauri/src/spools/tares.rs`
- Create: `src-tauri/tests/p3_ledger.rs`

**Interfaces:**

```rust
pub enum AmountEntry { Net { net_mg: i64, confidence: AmountConfidence }, Scale { gross_mg: i64, tare_id: Option<String>, tare_mg: Option<i64> } } // tag = "kind"
pub enum AmountEventKind { Initial, Measurement, Estimate, Consumption, MarkedEmpty }
pub struct AmountEvent { id, spool_id, sequence: i64, kind, before_mg: Option<i64>, after_mg: i64, confidence_after, gross_mg, tare_mg, reservation_id, note, occurred_at, is_correction: bool }
pub struct Tare { id, revision: i64, name, weight_mg: i64, created_at, updated_at }

// repository.rs — all take the caller's transaction
pub fn load_spool(tx: &Transaction, id: &str) -> Result<Option<StoredSpool>, StorageError>;
pub fn list_spools(tx: &Transaction) -> Result<Vec<SpoolRecord>, StorageError>; // availability + facets derived here
pub fn insert_spool(tx: &Transaction, fields: &SpoolFields, initial: &AmountEntry, storage_label: Option<&str>) -> Result<StoredSpool, RepositoryError>;
pub fn update_spool_fields(tx, id, expected_revision, patch: &SpoolPatch) -> Result<StoredSpool, RepositoryError>;
pub fn check_and_bump_revision(tx, id, expected_revision) -> Result<StoredSpool, RepositoryError>;

// ledger.rs
pub fn append(tx: &Transaction, spool_id: &str, kind: AmountEventKind, after_mg: i64, confidence: AmountConfidence, snapshot: LedgerSnapshot) -> Result<AmountEvent, RepositoryError>;
pub fn resolve_entry(tx: &Transaction, entry: &AmountEntry) -> Result<(i64, AmountConfidence, LedgerSnapshot), RepositoryError>; // D3 scale math
pub fn history(tx: &Transaction, spool_id: &str) -> Result<Vec<AmountEvent>, StorageError>;

// tares.rs: create / update(expected_revision) / delete / list
```

- [ ] **Step 1: Write failing tests in `tests/p3_ledger.rs`**

Use `common::storage()`.

1. **Estimated create:** `insert_spool` with `Net{1_000_000, Estimated}`
   writes one `initial` event (`before_mg = None`). The cache matches, and
   `spool_number == 1`. A second Spool gets number 2.
2. **Scale entry:**
   - Tare "Cardboard" of 140 g, then `Scale{gross 752_300, tare_id}`
     gives 612.3 g net (`612_300`), `measured`, with snapshots
     `gross_mg = 752_300` and `tare_mg = 140_000`.
   - Updating the tare to 150 g leaves that event's `tare_mg` unchanged.
   - Deleting the tare sets the Spool's `tare_id = None`.
3. **Validation:**
   - Gross below the tare gives `Validation{field_path: "entry.grossMg"}`.
   - Giving both `tare_id` and `tare_mg`, or neither, is a validation
     error.
4. **Corrections:** the sequence initial (estimated) → `Consumption`
   (appended directly with `ledger::append`) → `Measurement` gives
   `history()[2].is_correction == true`, and its `before_mg` equals the
   consumption row's `after_mg`.
5. **Restart:** reopen `Storage` on the same paths. The history is equal and
   ordered by `sequence`, and the cache equals the last row.
6. **Facets:**
   - `low` when `current_mg <= low_threshold_mg` and the lifecycle is
     active. It is false when the lifecycle is empty.
   - `confidence` mirrors the cache.
7. **Duplicate tare name:** a tare name that differs only by case gives
   `Validation{field_path: "name"}`.

Run `cargo test --test p3_ledger`. Expected: FAIL.

- [ ] **Step 2: Implement `resolve_entry`, `append`, and `history`**

`append` reads the current cache for `before_mg`, inserts the event with
`sequence = max + 1`, and updates `spools.current_mg`, `confidence`, and
`updated_at`. For measured confidence it also updates `last_measured_at`.

`is_correction` is computed in `history`: the row is a measurement or
estimate and an earlier row is a consumption.

- [ ] **Step 3: Implement the repository, derivation, and tares**

The `list_spools` query joins `material_slots` for `printer_id` and sums the
reservations in `{active, unresolved}` in one query.

- [ ] **Step 4: Run the tests**

Run `just test-rust`. Expected: PASS.

- [ ] **Step 5: Commit**

Message: `feat: record spool amounts in an append-only ledger with tares`.

---

### Task 3: Atomic movement and occupancy

**Owner:** Backend (the wiring owner reviews concurrency)
**Prerequisites:** Task 2.
**Handoff:** Task 5 (initial loads), Task 6 (dispositions), and Task 7
(`move_spool`) all call `apply_move`.

**Files:**
- Create: `src-tauri/src/spools/movement.rs`
- Create: `src-tauri/tests/p3_movement.rs`

**Interfaces:**

```rust
pub enum MoveDestination {
    Slot { slot_id: String, expected_occupant_spool_id: Option<String>, displaced_storage_label: Option<String> },
    Storage { storage_label: Option<String> },
} // tag = "kind", camelCase
pub enum MovementReason { Load, Unload, Displaced, Relocate, Consumed, PrinterArchived }
pub struct SpoolMovement { id, operation_id, spool_id, reason, from: SpoolLocationSnapshot, to: SpoolLocationSnapshot, occurred_at }
pub struct MoveOutcome { pub spool_ids: Vec<String>, pub printer_ids: Vec<String>, pub movements: Vec<SpoolMovement>, pub replayed: bool }

pub fn apply_move(tx: &Transaction, operation_id: &str, spool_id: &str, expected_spool_revision: i64,
                  dest: &MoveDestination, reason_override: Option<MovementReason>) -> Result<MoveOutcome, RepositoryError>;
pub fn find_operation(tx: &Transaction, operation_id: &str) -> Result<Option<MoveOutcome>, StorageError>;
pub fn history(tx: &Transaction, spool_id: &str) -> Result<Vec<SpoolMovement>, StorageError>;
```

`apply_move` follows D6 steps 1–6. It bumps each touched Printer's revision
and `updated_at`. The reason is derived as follows:

| Move | Reason |
|---|---|
| storage → slot | `Load` |
| slot → storage | `Unload` |
| slot → slot | `Load` |
| storage → storage | `Relocate` |
| the displaced Spool | `Displaced` |

`reason_override` is used by archive and `markEmpty`.

`CONFLICT` details use `CommandError` details
`{ slotId, currentOccupantSpoolId }`, carried as a new `RepositoryError::OccupancyConflict`
that maps to `ErrorCode::Conflict`.

- [ ] **Step 1: Write failing tests in `tests/p3_movement.rs`**

1. **Load:** a storage Spool loads into an empty `Main`. The Spool's
   location is that slot, and one `Load` row exists. Its revision and the
   Printer's revision are both +1.
2. **Swap:** Spool B loads into a slot occupied by A with
   `expected_occupant = A` and `displaced_storage_label = "Shelf"`.
   - A is in storage "Shelf" and B is in the slot.
   - There are two rows with the same `operation_id`, and the `Displaced`
     row is first.
3. **Atomic swap under failure:** the same swap with
   `Storage::inject_failure_once` on a new `FailurePoint::AfterDisplacement`
   (add the variant). Both Spools and the movement table are unchanged.
4. **Wrong occupant:** `expected_occupant = None` on an occupied slot gives
   `OccupancyConflict` naming A. Nothing changes.
5. **Concurrent race:** two threads each call `storage.write(|tx|
   apply_move(...))` into the same empty slot with `expected_occupant =
   None` and different Spools. Exactly one is `Ok`, the other is
   `OccupancyConflict`, and the slot holds the winner.
6. **Replay:** the same `operation_id` again returns `replayed = true`, and
   the row count is unchanged.
7. **Rejected moves:**
   - A stale `expected_spool_revision` gives `Conflict`.
   - Loading into a removed slot, a slot on an archived Printer, or its
     current slot gives `Validation{field_path: "destination.slotId"}`.
   - Loading an archived Spool gives `Validation{field_path: "spoolId"}`.
8. **Between Printers:** a direct slot→slot move from Printer X to Printer
   Y writes one `Load` row with `from_slot_id = X.slot` and `to_slot_id =
   Y.slot`, and both Printers are bumped.

Run `cargo test --test p3_movement`. Expected: FAIL.

- [ ] **Step 2: Implement `apply_move`, `find_operation`, `history`, and the failure point**

- [ ] **Step 3: Map the new error**

Map `RepositoryError::OccupancyConflict` in `CommandError::from_repository`.

- [ ] **Step 4: Run the tests**

Run `just test-rust`. Expected: PASS.

- [ ] **Step 5: Commit**

Message: `feat: move spools between slots and storage atomically`.

---

### Task 4: Reservation primitives and the availability signal

**Owner:** Backend
**Prerequisites:** Task 2.
**Handoff:** P7 consumes these functions. Task 7 adds the broadcast to
`RuntimeServices`.

**Files:**
- Create: `src-tauri/src/spools/reservations.rs`
- Create: `src-tauri/src/spools/events.rs` (the `InventoryChange` type
  only; event emission comes in Task 7)
- Create: `src-tauri/tests/p3_reservations.rs`

**Interfaces:**

```rust
pub struct ReservationHolder { pub kind: String, pub id: String }
pub enum ReservationState { Active, Unresolved, Released, Consumed }
pub struct Reservation { id, spool_id, holder: ReservationHolder, amount_mg: i64, state, operation_id, created_at, settled_at: Option<String> }
pub enum ReservationError { InsufficientAvailable { available_mg: i64 }, SpoolNotReservable { lifecycle: SpoolLifecycle }, InvalidTransition { from: ReservationState }, NotFound, Storage(StorageError) }

pub fn reserve(tx, spool_id, holder: &ReservationHolder, amount_mg: i64, operation_id: &str) -> Result<String, ReservationError>;
pub fn release(tx, reservation_id) -> Result<(), ReservationError>;
pub fn consume(tx, reservation_id, used_mg: i64, note: Option<&str>) -> Result<AmountEvent, ReservationError>;
pub fn mark_unresolved(tx, reservation_id) -> Result<(), ReservationError>;
pub fn availability(tx, spool_id) -> Result<Availability, ReservationError>;
pub fn open_reservations(tx, spool_id) -> Result<Vec<Reservation>, StorageError>;

#[derive(Clone, Debug)] pub struct InventoryChange { pub spool_ids: Vec<String>, pub printer_ids: Vec<String> }
```

- [ ] **Step 1: Write failing tests in `tests/p3_reservations.rs`**

Start from a 1000 g Spool.

1. **Reserve:** reserve 300 g, then 600 g. Availability is 100 g. Reserving
   101 g gives `InsufficientAvailable{available_mg: 100_000}`.
2. **Release:** releasing the 300 g reservation brings availability to
   400 g. Releasing it again gives `InvalidTransition{Released}`.
3. **Consume:** consuming the 600 g reservation with `used_mg = 640_000`
   writes one `Consumption` event (`reservation_id` set, estimated) and
   leaves the current amount at 360 g.
4. **Clamp:** consuming 500 g against a 100 g current amount leaves the
   current amount at 0, with a note that records the 400 g shortfall.
5. **Unresolved:** `mark_unresolved` keeps the amount unavailable.
6. **Over-reservation:** a measurement that lowers the current amount below
   the reserved total gives a negative `available_mg`, and later reserves
   fail.
7. **Not reservable:** reserving on an empty or archived Spool gives
   `SpoolNotReservable`.
8. **Rollback:** a reservation inside a `storage.write` closure that
   returns `Err` afterwards leaves no row.

Run `cargo test --test p3_reservations`. Expected: FAIL.

- [ ] **Step 2: Implement the functions**

Implement them against `spool_reservations` and `ledger::append`.

- [ ] **Step 3: Run the tests**

Run `just test-rust`. Expected: PASS.

- [ ] **Step 4: Commit**

Message: `feat: add transaction-scoped spool reservation primitives`.

---

### Task 5: Slot layouts in Printer reads, create, batch, and export

**Owner:** Backend
**Prerequisites:** Task 3.
**Handoff:** Tasks 9 and 11 render `materialSlots` and call
`set_material_slot_layout`.

**Files:**
- Create: `src-tauri/src/spools/slots.rs`
- Create: `src-tauri/tests/p3_setup.rs`
- Modify:
  - `src-tauri/src/printers/{mod.rs,repository.rs,create.rs,batch.rs,commands.rs}`
  - `src-tauri/src/catalog/resolve.rs`

**Interfaces:**

```rust
pub struct SlotSpec { pub id: Option<String>, pub name: String, pub feeder_label: Option<String> }
pub struct InitialLoad { pub slot_index: usize, pub spool_id: String, pub expected_spool_revision: i64 }
pub fn default_layout() -> Vec<SlotSpec>; // [Main]
pub fn insert_layout(tx, printer_id: &str, layout: &[SlotSpec]) -> Result<Vec<MaterialSlot>, RepositoryError>;
pub fn set_layout(tx, printer_id: &str, layout: &[SlotSpec]) -> Result<Vec<MaterialSlot>, RepositoryError>; // clarification 2; SLOT_OCCUPIED
pub fn live_slots(tx_or_conn, printer_id: &str) -> Result<Vec<MaterialSlot>, StorageError>;
```

- `CreatePrinterOptions` gains `slot_layout: Vec<SlotSpec>` and
  `initial_loads: Vec<InitialLoad>`.
- `BatchShared` gains `slot_layout: Option<Vec<SlotSpec>>`.
- `RepositoryError` gains `SlotOccupied { slot_id, spool_id }`, which maps
  to `ErrorCode::SlotOccupied`.

- [ ] **Step 1: Write failing tests in `tests/p3_setup.rs`**

Use `common::runtime` and `invoke`.

1. **Default layout:** `create_printer` without a layout gives
   `materialSlots == [Main]`.
2. **Layout and initial load:** `create_printer` with 4 slots
   ("1".."4", feeder "AMS 1") and an initial load of a storage Spool into
   index 2.
   - Slot 3 has that occupant, with one `Load` movement.
   - A Spool that is already loaded elsewhere fails the whole create with
     `Validation{field_path: "initialLoads[0].spoolId"}`, and no Printer
     row is created.
3. **Batch copies the layout:** `create_printers_batch` with 3 Profile-only
   rows and a shared 4-slot layout.
   - Each Printer has 4 slots, and the slot ids are pairwise disjoint.
   - No slot has an occupant.
   - Renaming a slot on one Printer through `set_material_slot_layout`
     leaves the other two unchanged.
4. **Layout editing:**
   - Reorder, rename, add, and remove an empty slot.
   - The removed slot keeps its `removed_at` row, and its history is still
     readable by `movement::history`.
   - Removing an occupied slot gives `SLOT_OCCUPIED`.
   - More than 16 slots, fewer than 1, or duplicate names (differing only
     by case) give `VALIDATION`.
5. **Export and import:** export v3 contains `materialSlots` without
   occupants. Importing v2 and v1 documents gives each Printer `[Main]`.
   Importing v3 recreates the layout with new ids.

Run `cargo test --test p3_setup`. Expected: FAIL.

- [ ] **Step 2: Implement `slots.rs` and load slots in `PrinterRepository`**

Load slots in every read path: `list`, `get`, and the post-write re-read.
Copy them in `resolve_printer`.

- [ ] **Step 3: Thread the layout through create**

Thread `slot_layout` and `initial_loads` through `create_printer_with`.
`insert_layout` and each `apply_move` run in the create transaction, sharing
one generated `operationId`. Batch builds each row's options with
`shared.slot_layout.clone().unwrap_or_else(default_layout)`, and always with
empty `initial_loads`.

- [ ] **Step 4: Add `set_material_slot_layout`**

The command takes `{ printerId, expectedRevision, slots }` and returns
`PrinterMutationResult`, bumping the Printer's revision. Leave registration
for Task 7, but make sure it compiles.

- [ ] **Step 5: Bump export to schema v3, with import accepting v1–v3**

- [ ] **Step 6: Run the tests**

Run `just test-rust`. Expected: PASS.

- [ ] **Step 7: Commit**

Message: `feat: give every Printer a configurable material slot layout`.

---

### Task 6: Archive with Spool dispositions, lifecycle blockers, and delete cascade

**Owner:** Backend (the wiring owner reviews ordering)
**Prerequisites:** Tasks 3, 4, and 5.
**Handoff:** Task 11 builds `ArchivePrinterDialog` on `loadedSpools` and the
new request.

**Files:**
- Create: `src-tauri/src/spools/lifecycle_blockers.rs`
- Create: `src-tauri/tests/p3_lifecycle.rs`
- Modify:
  - `src-tauri/src/printers/{lifecycle.rs,repository.rs,commands.rs}`
  - `src-tauri/src/contracts/command.rs`
- Modify: `src/printers/printer-store.ts` (the `archivePrinter` call shape,
  per clarification 3)

**Interfaces:**

```rust
pub enum LifecycleBlockerCode { NotArchived, AlreadyArchived, SpoolsLoaded, SpoolReserved }
pub struct LifecycleEligibility { /* P2 fields */ pub loaded_spools: Vec<SpoolRecord> }
pub struct LoadedSpoolBlockers; // registered in blocker_sources()

pub enum SpoolDisposition {
    Storage { storage_label: Option<String> },
    Slot { slot_id: String, expected_occupant_spool_id: Option<String>, displaced_storage_label: Option<String> },
    MarkEmpty { storage_label: Option<String> },
} // tag = "kind"
pub struct SpoolDispositionInput { pub spool_id: String, pub expected_spool_revision: i64, pub disposition: SpoolDisposition }
pub fn apply_dispositions(tx, printer_id: &str, operation_id: &str, inputs: &[SpoolDispositionInput]) -> Result<MoveOutcome, RepositoryError>;

// Spool lifecycle (used here for MarkEmpty and in Task 7's command)
pub enum SpoolLifecycleAction { MarkEmpty, Reactivate, Archive, Unarchive }
pub fn apply_lifecycle(tx, spool_id, expected_revision, action, storage_label: Option<&str>, operation_id: &str) -> Result<MoveOutcome, RepositoryError>;
```

- [ ] **Step 1: Write failing tests in `tests/p3_lifecycle.rs`**

Set up Printer P (3 slots, holding Spools A, B, and C), Printer Q (with slot
Q1 holding D), and an empty Printer R.

1. **Eligibility:** `printer_lifecycle_eligibility(P)` gives
   `canArchive = false`, a `SPOOLS_LOADED` archive blocker, and
   `loadedSpools = [A, B, C]`.
2. **Missing dispositions:**
   - `archive_printer(P, rev, op, [])` gives `LIFECYCLE_BLOCKED`
     (`SPOOLS_LOADED`).
   - Dispositions covering only A and B give
     `Validation{field_path: "spoolDispositions"}`.
3. **Atomic archive:** dispositions A→storage "Shelf", B→Q1 (expected
   occupant D, D displaced to "Bin"), and C→MarkEmpty.
   - P is archived.
   - A is in "Shelf", B in Q1, D in "Bin", and C is empty in storage with a
     `MarkedEmpty` event.
   - All movement rows share `op`. A and B have reason `PrinterArchived`,
     C has `Consumed`, and D has `Displaced`.
   - The supervisor is stopped for P after commit, following the P2
     assertion style.
4. **Rollback:** the same setup with B→Q1 but a stale expected occupant
   (`None`) gives `CONFLICT`. P is not archived and A, B, and C are
   unmoved.
5. **Slot on the same or an archived Printer:** a disposition into another
   of P's own slots, or into an archived Printer's slot, gives
   `VALIDATION`.
6. **Restart:** reopen storage. P's `materialSlots` and each Spool's
   movement history, including the rows touching P's slots, are intact.
7. **Delete cascade:** `delete_printer(P)`.
   - Every movement row touching P's slots is gone, B's row included.
   - Spools A–D, their ledgers, and their reservations are unchanged.
   - `PRAGMA foreign_key_check` is clean.
8. **Reservation guards:** the Spool lifecycle actions `archive` and
   `markEmpty` on a Spool with an active reservation give
   `LIFECYCLE_BLOCKED` (`SPOOL_RESERVED`). The same applies to a
   `MarkEmpty` disposition in archive.
9. **No dispositions needed:** archiving R with `spoolDispositions: []`
   succeeds exactly as in P2.

Run `cargo test --test p3_lifecycle`. Expected: FAIL.

- [ ] **Step 2: Implement `LoadedSpoolBlockers` and register it**

Add the new blocker codes, register the source in `blocker_sources()`, and
compute `loaded_spools` in `evaluate`. Replace the Printer-only message in
`CommandError::lifecycle_blocked` with "This action is blocked:" plus the
blocker messages.

- [ ] **Step 3: Implement `apply_lifecycle` and `apply_dispositions`**

Both call `movement::apply_move` with `reason_override`, and
`ledger::append` for `MarkedEmpty`.

- [ ] **Step 4: Extend archive**

`PrinterRepository::archive` applies the dispositions, re-runs `evaluate`,
then sets `archived_at`, all in one transaction. `archive_printer` now takes
`{ id, expectedRevision, operationId, spoolDispositions }`. The post-commit
order is unchanged.

- [ ] **Step 5: Update the frontend call**

`printer-store.ts` `archivePrinter(id, dispositions = [])` generates the
`operationId` with `crypto.randomUUID()`. Update the existing
`printer-store.test.ts` expectations.

- [ ] **Step 6: Run the tests**

Run `just test-rust` and `just test`. Expected: PASS.

- [ ] **Step 7: Commit**

Message: `feat: relocate loaded spools when archiving a Printer`.

---

### Task 7: Inventory commands, events, contracts, and the Tauri path

**Owner:** Wiring
**Prerequisites:** Tasks 2–6.
**Handoff:** Tasks 9–11 consume the generated contracts. P7 subscribes to
`inventory_changes`.

**Files:**
- Create: `src-tauri/src/spools/commands.rs`
- Modify: `src-tauri/src/spools/events.rs`
- Create: `src-tauri/tests/p3_contract_path.rs`
- Modify:
  - `src-tauri/src/lib.rs`: `RuntimeServices.inventory_changes:
    tokio::sync::broadcast::Sender<InventoryChange>` (capacity 64), plus
    registration.
  - `src-tauri/src/contracts/inventory.rs`
  - `src-tauri/tests/export_contracts.rs`
  - `src/ipc/client.ts`

**Interfaces:**
- Commands, per spec D13:
  - `list_spools`
  - `spool_history`
  - `create_spool`
  - `update_spool`
  - `record_spool_amount`
  - `move_spool`
  - `set_spool_lifecycle`
  - `create_tare`, `update_tare`, `delete_tare`
  - `set_material_slot_layout`, now registered
- Results:
  - `SpoolMutationResult { spool, printers, warnings }`
  - `MoveSpoolResult { spools, printers, movements }`
  - `TareMutationResult { tare }`
  - `SpoolHistory { movements, amountEvents, reservations }`
  - `InventorySnapshot { streamId, snapshotSequence, spools, tares }`
- Events are emitted on `STATUS_EVENT` with `EventEnvelope`:
  - `spool.changed`
  - `printer.slots.changed`
  - `spool.availability.changed`
- They use a new `InventoryEventType` and a tagged `InventoryEventPayload`.
  An `InventoryStream` owns `stream_id` (a UUID per process) and an
  `AtomicU64` sequence.

- [ ] **Step 1: Write failing tests in `tests/p3_contract_path.rs`**

Use `common::runtime` and `invoke` with a recording event listener.

1. **Create:** `create_spool` returns the Spool. Exactly one
   `spool.changed` event follows, with `sequence > snapshotSequence` of an
   earlier `list_spools`.
2. **Move and replay:** `move_spool` into an occupied slot returns both
   Spools and the Printer.
   - The events are two `spool.changed`, one `printer.slots.changed`, and
     two `spool.availability.changed`.
   - A subscriber to `inventory_changes` receives one `InventoryChange` with
     both Spool ids and the Printer id.
   - Replaying the same `operationId` emits nothing and broadcasts nothing.
3. **Failures emit nothing:** a `CONFLICT` move emits no events.
4. **Error shapes:**
   - `record_spool_amount` scale with gross < tare gives `VALIDATION` with
     `fieldErrors[0].fieldPath == "entry.grossMg"`.
   - `set_material_slot_layout` removing an occupied slot gives
     `SLOT_OCCUPIED` with `details.spoolId`.
5. **Lifecycle:** `set_spool_lifecycle` `markEmpty` on a loaded Spool
   unloads it and returns the updated Printer in `printers`.
6. **Contracts:** the generated contracts include each new type, and
   `COMMAND_NAMES` has the expected new length.

Run `cargo test --test p3_contract_path`. Expected: FAIL.

- [ ] **Step 2: Implement the commands**

Each command does `contract_version.validate()?`, `bootstrap.ready()?`, and
one `storage.write`. After `Ok`, it emits events and broadcasts the
`InventoryChange`. It never emits inside the closure.

Printer records in results are resolved with `resolve_printer`, and the
command also emits `printer.slots.changed` for them.

- [ ] **Step 3: Register every command**

Register in all five places, including the `CommandContracts` decl TS lines
and `visitor.visit` for each result type, and add the entries to
`CommandMap` in `src/ipc/client.ts`.

- [ ] **Step 4: Add the debug fixture reservation**

Add a `#[cfg(debug_assertions)]` command, `debug_seed_reservation { spoolId,
amountMg }`. It is registered only in debug builds, is excluded from
`COMMAND_CONTRACTS`, and is invoked from the frontend only behind
`import.meta.env.DEV`. It exists for manual verification of the `reserved`
facet (D8).

- [ ] **Step 5: Run the tests**

Run `just gen-contracts`, `just test-rust`, and `just build`. Expected: PASS
and no unexpected diff in `src/generated`.

- [ ] **Step 6: Commit**

Message: `feat: expose inventory commands and events over IPC`.

---

### Task 8: `DataTable`, `Timeline`, and `ColorSwatch` primitives

**Owner:** Frontend
**Prerequisites:** none. This can run in parallel with Tasks 1–7.
**Handoff:** Task 10 uses all three. Task 11 uses `ColorSwatch`.

**Files:**
- Create: `src/design-system/components/{DataTable,Timeline,ColorSwatch}/`
  with `*.tsx`, `*.module.css`, and `*.test.tsx`
- Modify: `src/design-system/components/index.ts`
- Modify: `src/design-system/Showcase.tsx`

**Interfaces:**

```ts
type DataTableColumn<T> = { id: string; header: string; cell: (row: T) => JSX.Element;
  sortValue?: (row: T) => string | number; width?: string; align?: "start" | "end" };
function DataTable<T>(props: { rows: T[]; rowId: (row: T) => string; columns: DataTableColumn<T>[];
  selectedId?: string | null; onSelect?: (id: string) => void; onActivate?: (id: string) => void;
  sort?: { columnId: string; direction: "asc" | "desc" }; onSortChange?: (s) => void;
  label: string; empty?: JSX.Element }): JSX.Element;
function Timeline(props: { items: { id: string; at: string; title: string; detail?: JSX.Element; marker?: "default" | "muted" }[]; label: string }): JSX.Element;
function ColorSwatch(props: { hex: string | null; name: string; size?: "sm" | "md" }): JSX.Element;
```

- [ ] **Step 1: Write failing component tests**

- `DataTable`:
  - It renders `role="grid"` and `aria-rowcount`.
  - ArrowDown, ArrowUp, Home, and End move selection with `aria-selected`,
    and Enter calls `onActivate`.
  - A header click toggles sort with `aria-sort`.
  - It shows `empty` when there are no rows.
- `Timeline`: renders an ordered list with `<time datetime>`.
- `ColorSwatch`: has an accessible name of `name`. A null `hex` renders the
  "no color" pattern.

Run `npx vitest run src/design-system`. Expected: FAIL.

- [ ] **Step 2: Implement the components**

Use tokens only: dense row height from `--f3d-type-*` line height, a sticky
header, and horizontal overflow on the table wrapper, not the page.
`ColorSwatch` uses an inline `background-color` from its `hex` prop. This is
data, not theme, and is the one allowed inline color. Its border uses a
token.

- [ ] **Step 3: Export and add to the Showcase**

Export all three, and add Showcase sections with sample rows and events.

- [ ] **Step 4: Run the checks**

Run `just build` and `just test`. Expected: PASS.

- [ ] **Step 5: Commit**

Message: `feat: add DataTable, Timeline, and ColorSwatch components`.

---

### Task 9: Spool store, pure modules, and web fixtures

**Owner:** Frontend (the wiring owner reviews settlement)
**Prerequisites:** Task 7 contracts. This can start against the Task 7
interface with generated types stubbed, but it lands after Task 7.
**Handoff:** Tasks 10 and 11.

**Files:**
- Create: `src/spools/{weight,materials,facets,spool-store,web-fixtures}.ts`,
  each with a test
- Modify: `src/printers/printer-store.ts` (export `spliceResolved` for the
  spool store; web fixture Printers get `materialSlots`; `setSlotLayout`;
  `createPrinter` gains `slotLayout` and `initialLoads`)

**Interfaces:**

```ts
// weight.ts
export function parseGrams(input: string): { ok: true; mg: number } | { ok: false; reason: "empty" | "format" | "precision" | "range" };
export function formatGrams(mg: number, precision: 0 | 1): string; // "612 g" / "612.3 g"
// facets.ts
export type SpoolFilter = { lifecycle: "active" | "empty" | "archived" | "all"; facets: Set<"loaded"|"reserved"|"low"|"measured"|"estimated">;
  materials: Set<MaterialFamily>; printerId: string | null; query: string };
export function applySpoolFilter(spools: SpoolRecord[], printers: PrinterRecord[], f: SpoolFilter): SpoolRecord[];
export function isFiltered(f: SpoolFilter): boolean;
// spool-store.ts
export const spoolState: { spools: SpoolRecord[]; tares: Tare[]; loaded: boolean; pending: Record<string, string[]> };
export async function loadInventory(): Promise<void>; // listen-before-backfill
export async function moveSpool(req: Omit<MoveSpoolRequest, "operationId">): Promise<MoveSpoolResult>; // optimistic
export async function createSpool / updateSpool / recordAmount / setLifecycle / createTare / updateTare / deleteTare / loadHistory
```

- [ ] **Step 1: Write failing tests**

- `weight.test.ts`: `"612.3"` gives `612_300`; `"612.34"` is `precision`;
  `"-1"` and `"50001000"` are `range`; `"1,5"` is `format`. The formatter
  rounds half away from zero.
- `facets.test.ts`: composing Low + Estimated + PLA + Printer X returns only
  their intersection. The search matches `#12`, color, manufacturer, and
  storage label. `isFiltered` is false for the default filter.
- `spool-store.test.ts`, with `@tauri-apps/api/core` and `event` mocked:
  1. The listener is registered before the `list_spools` backfill, and
     events at or below `snapshotSequence` are dropped.
  2. `moveSpool` updates both Spools locally before the invoke resolves.
     It settles from the result and calls `spliceResolved` for the
     returned Printers.
  3. On a `CONFLICT` rejection, it restores the pre-move snapshot, refetches
     through `list_spools`, and rethrows for inline handling.
  4. A late `spool.changed` with a lower revision than the settled one is
     ignored.
  5. In web mode, fixture moves enforce one Spool per slot and swap with
     displacement.

Run `npx vitest run src/spools`. Expected: FAIL.

- [ ] **Step 2: Implement the modules**

The web fixture has 8 Spools:

- Active and loaded, measured.
- Active in storage, estimated and low.
- One seeded as `reserved`.
- One empty.
- One archived.
- Three more active Spools.

It also has 2 tares. Fixture Printers get layouts: one with `[Main]`, and
one Centauri Carbon with 4 slots.

- [ ] **Step 3: Run the checks**

Run `just build` and `just test`. Expected: PASS.

- [ ] **Step 4: Commit**

Message: `feat: add the spool store with optimistic movement`.

---

### Task 10: Spools destination, inventory, detail, and dialogs

**Owner:** Frontend
**Prerequisites:** Tasks 8 and 9.
**Handoff:** Task 11 reuses `MoveSpoolDialog`.

**Files:**
- Create:
  - `src/screens/SpoolInventory.tsx`
  - `src/screens/SpoolDetailDock.tsx`
  - `src/screens/SpoolFormDialog.tsx`
  - `src/screens/RecordAmountDialog.tsx`
  - `src/screens/MoveSpoolDialog.tsx`
  - `src/screens/TareManagerDialog.tsx`
  - A CSS module and test for each
- Modify:
  - `src/App.tsx`: add `spools` to `availableDestinations` and render
    `SpoolInventory`.
  - `src/screens/ActivityBar.tsx` and `ActivityBar.test.tsx`: add the
    Spools button and its low-count badge.

- [ ] **Step 1: Write failing tests**

1. **ActivityBar:** it shows Spools, with a badge equal to the number of
   `low` Spools. There is still no Queue button.
2. **Inventory:**
   - It shows the fixture rows.
   - Toggling the **Low** and **Estimated** chips (`aria-pressed`) narrows
     the rows.
   - When filters match nothing, it shows "No Spools match" and **Clear
     filters**. With no Spools at all, it shows "No Spools yet" and **Add
     Spool**.
   - Selecting a row by keyboard sets the deep link to
     `#nav=v1/spools/spool/<id>` and opens the detail dock.
3. **Detail:**
   - The history `Timeline` merges movements and amount events newest
     first.
   - A correction reads "corrected +18 g from the estimate".
   - An estimated amount shows "est." as text.
4. **`SpoolFormDialog`:**
   - Choosing `OTHER` reveals and requires the material text.
   - The nominal quick-pick fills 1 kg.
   - "I weighed it" switches to measured or scale entry.
   - Submitting calls `createSpool` with mg values.
5. **`RecordAmountDialog`:**
   - Scale mode with the Cardboard tare shows a live net preview.
   - Gross below tare shows the inline error returned by Rust.
6. **`MoveSpoolDialog`:**
   - Choosing a Printer and then an occupied slot shows the swap line and a
     required-label field for the displaced Spool.
   - Submit calls `moveSpool` with `expectedOccupantSpoolId`.
   - On a mocked `CONFLICT`, the dialog stays open showing the new
     occupant.
7. **`TareManagerDialog`:** add, rename, and delete a tare.

Use `fireEvent.pointerDown` and `pointerUp` for Select and DropdownMenu.

- [ ] **Step 2: Implement the screens**

Follow the spec §Components. Use `Dialog`, `Select`, `Combobox`,
`NumberField`, `RadioGroup`, `Chip`, and `DataTable`. Storage label
suggestions are the distinct labels already in the store.

- [ ] **Step 3: Check the shell at 1024 wide**

Confirm the dock overlays and does not squeeze the table. Use the existing
dock rule; don't add a new one.

- [ ] **Step 4: Run the checks**

Run `just build` and `just test`. Expected: PASS.

- [ ] **Step 5: Commit**

Message: `feat: add the Spools inventory destination`.

---

### Task 11: Printer integration

This covers the Status tab slots, the Setup slot editor, wizard Equip, batch
layout and Equip, and the archive dialog.

**Owner:** Frontend (the wiring owner reviews the archive flow)
**Prerequisites:** Tasks 6, 9, and 10.
**Handoff:** Task 12.

**Files:**
- Create:
  - `src/screens/MaterialSlotsEditor.tsx`
  - `src/screens/ArchivePrinterDialog.tsx`
  - A CSS module and test for each
- Modify:
  - `src/screens/PrinterStatusPanel.tsx`
  - `src/screens/PrinterDetailDock.tsx`
  - `src/screens/PrinterSetupWizard.tsx`
  - `src/screens/PrinterBatchDialog.tsx`
  - `src/screens/BatchRowsTable.tsx`
  - `src/screens/DeletePrinterDialog.tsx`
  - The tests for each

**Interfaces:**

```ts
function MaterialSlotsEditor(props: {
  mode: "layout" | "occupancy";
  slots: SlotDraft[];                       // layout mode: controlled drafts
  onChange?: (slots: SlotDraft[]) => void;
  printer?: PrinterRecord;                  // occupancy mode: live slots + load/swap/unload
  initialLoads?: { slotIndex: number; spoolId: string }[]; onInitialLoadsChange?: (...) => void; // wizard only
  multiMaterialHint: boolean;
}): JSX.Element;
```

- [ ] **Step 1: Write failing tests**

1. **Editor, layout mode:**
   - It supports add, rename, feeder label, up and down (buttons and
     keyboard), and remove.
   - The 17th add is disabled.
   - A duplicate name (differing only by case) shows an inline error.
   - The multi-material hint appears only when `multiMaterialHint` is set.
2. **Editor, occupancy mode:**
   - Remove is disabled on an occupied slot, with "Unload first".
   - **Load…** opens `MoveSpoolDialog` preset to that slot.
   - **Unload** calls `moveSpool` to storage.
3. **Status tab:** each slot shows the occupant's number, material,
   swatch, color name, and remaining amount with "est.", plus Low and
   Reserved chips, or "Empty". Activating an occupant navigates to
   `#nav=v1/spools/spool/<id>`.
4. **Wizard:**
   - The steps are Identify, Connect, Equip, Operate, Review.
   - Equip defaults to `[Main]`.
   - An initial load picks only active storage Spools.
   - Review lists the slots and loads.
   - Save calls `createPrinter` with `slotLayout` and `initialLoads`.
5. **Batch:**
   - Shared includes the layout editor, and the create request carries
     `shared.slotLayout`.
   - Results rows with `created` or `createdSetupIncomplete` show
     **Equip**, which closes the dialog and opens that Printer's Setup tab
     at Material Slots.
6. **Archive dialog:**
   - With loaded Spools, **Archive** is disabled until each row has a
     disposition.
   - The slot picker excludes this Printer and archived Printers, and
     handles swap labels.
   - Submit sends the dispositions with an `operationId`.
   - A mocked `CONFLICT` keeps the dialog open with the row error.
   - With no loaded Spools, the P2 confirm is used.
7. **Delete dialog:** it includes "Spool movement history involving this
   Printer will be deleted."

- [ ] **Step 2: Implement the editor**

Implement `MaterialSlotsEditor`, then wire it into the Setup tab's
**Material Slots** section. Saving calls `setSlotLayout`, with the P2
`CONFLICT` reload handling. Deep-link the section with an anchor id that the
batch **Equip** can target.

- [ ] **Step 3: Implement the rest**

Implement the Status tab slots, the wizard Equip step, and the batch Shared
and Results changes. Implement `ArchivePrinterDialog`, and route the Setup
tab's **Archive** through eligibility's `loadedSpools`.

- [ ] **Step 4: Run the checks**

Run `just build` and `just test`. Expected: PASS.

- [ ] **Step 5: Commit**

Message: `feat: equip Printers with material slots and loaded spools`.

---

### Task 12: Tracer, docs, and verification evidence

**Owner:** Wiring and Verification
**Prerequisites:** Tasks 1–11.

**Files:**
- Create: `src-tauri/tests/p3_tracer.rs`
- Create: `docs/verification/<date>-p3-spools-material-slots.md`
- Create: `docs/screenshots/p3-*.png`
- Modify: `CONTEXT.md`
- Modify: `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`

- [ ] **Step 1: Write the tracer `tests/p3_tracer.rs`, through `invoke` only**

1. Create a Profile-only Printer X, which gets `[Main]`.
2. `create_spool` S1 at 1 kg, estimated, in storage "Shelf".
3. `move_spool` S1 into X's Main.
4. `create_spool` S2 and load it into Main with `expectedOccupant = S1`
   and `displacedStorageLabel = "Shelf"`. This is the swap.
5. `move_spool` S2 to storage "Dry box". This moves an existing loaded
   Spool to storage.
6. `record_spool_amount` S1 by scale, which makes it measured.
7. Rebuild the runtime over the same storage paths (restart).
8. `spool_history` for S1 and S2. Movements and amount events are present,
   ordered, and show the confidence transitions (estimated → measured), and
   `list_spools` locations match.

- [ ] **Step 2: Update `CONTEXT.md`**

Add Tare, Storage label, and Spool number, worded as in spec §Product
vocabulary. Also add one line under **Material Slot**: layouts are
user-configured, and there is no automatic AMS topology.

- [ ] **Step 3: Update the approach doc's known-unknowns rows**

Mark "Slot count/topology source" and "Weight precision/material taxonomy"
resolved, pointing to spec D1, D2, and D4.

- [ ] **Step 4: Run the full verification**

Run each of these:

- `just build`
- `just test`
- `source "$HOME/.cargo/env" && just test-rust`
- `just gen-contracts`, then `git diff --exit-code src/generated`

- [ ] **Step 5: Run manual checks with a display**

Run `just dev`. Check the tracer at 1440 × 900 and 1024 × 700, and check
keyboard-only operation through the inventory, Move with swap, the wizard
Equip step, and the archive dialog.

Save screenshots of these states: inventory, filtered-empty, detail
history, move swap, Status slots, Setup slot editor, wizard Equip, and
archive dispositions.

If there's no display, record these checks as **unavailable**, not passed.

- [ ] **Step 6: Write the verification doc**

Mirror `docs/verification/2026-09-22-p2-printer-lifecycle.md`:

- Automated evidence mapped to spec acceptance criteria 1–15.
- The tracer transcript.
- Manual evidence and unavailable checks.
- The clarification 4 mapping ("unresolved movements" → `SPOOLS_LOADED`).

- [ ] **Step 7: Commit, push, and open a draft PR**

Commit with `docs: record P3 verification evidence`. Push and open a draft
PR that references #13.

---

## Delivery order and parallel work

```text
Backend:   T1 → T2 → T3 → T4 (T4 may run parallel to T3) → T5 → T6
Wiring:    T7 (needs T2–T6) → T12
Frontend:  T8 (parallel with T1–T7) → T9 (needs T7 contracts) → T10 → T11
```

With a single implementer, run them strictly in numeric order. The
concurrency tests (T3 test 5) and the archive ordering (T6) get a
wiring-owner review before T7 starts.

## External dependencies and blockers

- The D2 family list needs network access to OrcaSlicer `v2.4.2` source
  (Task 1, step 1). If it's unavailable, keep the spec list and flag it in
  the PR as unverified.
- No printer hardware is needed. Every Printer in the P3 tests is
  Profile-only or uses the injected connection factory.
- `tokio::sync::broadcast` is already available through `tokio`. Check that
  the `sync` feature is enabled in `src-tauri/Cargo.toml`.

## Expected deliverables

- Schema v4, with six inventory tables, DB-enforced occupancy, and an
  append-only ledger.
- 11 new registered commands, the extended `create_printer`,
  `create_printers_batch`, `archive_printer`, and
  `printer_lifecycle_eligibility`, and a debug-only fixture command.
- The `inventory` event stream and the `InventoryChange` broadcast for P7.
- Reservation primitives with direct tests.
- The Spools destination, the Printer Status and Setup slot integration,
  wizard Equip, batch layout, and the archive disposition dialog.
- The `DataTable`, `Timeline`, and `ColorSwatch` primitives.
- `CONTEXT.md` updates and the P3 verification doc.
