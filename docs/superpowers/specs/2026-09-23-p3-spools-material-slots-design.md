# P3 Spools and Material Slots Design

## Status

Approved focused design for GitHub issue #13. The user approved this design
in conversation on 2026-09-23, including the decisions it proposed beyond the
four listed below. It narrows the
complete-v1 interaction design
(`2026-09-16-complete-v1-ui-workflows-design.md`, "Spool inventory and
Material Slots") to the work P3 owns. It also resolves the P3 decision gate in
the phase plan (`2026-09-16-complete-v1-implementation-approach.md`, P3
section): weight units and precision, material taxonomy, tare reuse, slot
capability sources, reservation arithmetic, and correction-history semantics.

The user decided four questions on 2026-09-23, and this document treats them
as fixed:

1. **Archiving a Printer with loaded Spools asks where each Spool goes:** to
   another Printer's slot, to storage, or marked empty (used up). Archive
   never leaves a Spool loaded on an archived Printer.
2. **A Printer's slot and movement history is archived with it and deleted
   with it.** Permanent deletion removes that history instead of preserving
   name snapshots.
3. **Batch setup copies the slot layout only.** Loading Spools happens per
   Printer after creation.
4. **Material family is a fixed list plus Other**, taken from OrcaSlicer's
   `filament_type` values.

## Goal

Make physical material a first-class, durable part of the Farm. A user can:

- Record Spools with a measured or estimated amount.
- Give each Printer named Material Slots.
- Load, swap, and unload Spools, with displacement and loading committed
  together.
- See how each Spool's amount and location changed over time.

P7 gets reservation primitives it can compose into Job assignment. Printer
archive and delete handle loaded Spools and slot history without orphaning
anything.

## Scope

### In scope

- Spool records: identity, material, color, diameter, nominal and current
  net weight, low threshold, tare, lifecycle, and location.
- Reusable empty-spool tares, with scale entry (gross minus tare).
- An append-only amount ledger with measured and estimated confidence, where
  a correction never rewrites history.
- Per-Printer Material Slot layouts that the user configures, with a default
  single slot.
- Atomic movement: load, unload, swap with displacement, and moves from one
  Printer to another, protected by optimistic concurrency.
- An append-only movement history.
- Reservation primitives as a Rust API (reserve, release, consume, mark
  unresolved) and derived availability.
- Inventory events, a backfill command, and an in-process availability signal
  for P7.
- Printer archive with Spool dispositions, and deletion that cascades slot
  history.
- Frontend:
  - A Spools destination with an inventory table, composable facets, and a
    detail dock.
  - Add, edit, record-amount, move, unload, and lifecycle flows.
  - Slots on the Printer Status tab, and a slot editor on the Setup tab.
  - An Equip step in the single wizard, and the slot layout in batch Shared.
  - Spool dispositions in the archive dialog.
- Printers export v3 carries the slot layout.

### Non-goals

- Queue order, eligibility, dispatch, and the automatic evaluator (P7).
- Reservations created by real Jobs, deferred reconciliation, and its
  Attention Event (P7/P8). P3 ships the primitives and tests them directly.
- Automatic AMS/MMU/tool-changer topology (see D4).
- Slot constraints beyond name and feeder label, such as per-slot material
  restrictions or tool mapping.
- Material compatibility between a Slice and a Spool (P5/P7).
- Barcode or NFC Spool handling, and tracking filament length.
- Hard deletion of Spools (they are archive-only in P3).
- Spool inventory export and backup (P9).
- Low-amount desktop notifications (P8).

## Product vocabulary

This design uses `CONTEXT.md` terms. It adds three terms, which are proposed
for `CONTEXT.md` in the same change:

- **Tare:** a reusable, named empty-spool weight (for example "Polymaker
  cardboard 1 kg"). Subtracting it from a scale reading gives the net amount.
- **Storage label:** an optional free-text name for where a Spool sits when
  it is not in a Material Slot (for example "Dry box 2"). Like Printer
  Location, it is a label, not an entity.
- **Spool number:** a small sequential integer shown as `#12`, meant for
  writing on the physical Spool. The stable id stays internal.

## Decisions

### D1. Weight units and precision

- Rust, SQLite, and the wire store every amount as **integer milligrams**
  (`i64` in Rust, a `number` in TypeScript). The largest valid value (50 kg,
  which is 5 × 10⁷ mg) is well inside JS-safe range.
- Integer storage keeps reservation arithmetic exact. When P7 converts a
  slicer estimate (fractional grams) into a reservation, it rounds **up** to
  the next milligram.
- The UI shows grams:
  - Tables round to whole grams.
  - Detail and entry fields show one decimal.
  - Input accepts at most one decimal (0.1 g) and is parsed by one pure
    module, `src/spools/weight.ts`.
- Valid ranges:
  - Nominal net weight: 1 g to 50 kg.
  - Current net weight: 0 to 50 kg.
  - Tare: 0 to 5 kg.
  - Low threshold: 0 to 50 kg.
- Grams is the only unit in v1. Settings has no unit preference.
- Diameter is an enum, `"1.75" | "2.85"` (mm). It is not a free number.

### D2. Material taxonomy

`MaterialFamily` is a closed enum drawn from the OrcaSlicer `filament_type`
values, restricted to those at the catalog's pinned OrcaSlicer tag
(`v2.4.2`, the `just gen-catalog` default):

`PLA`, `PLA-CF`, `PETG`, `PET-CF`, `ABS`, `ASA`, `TPU`, `PA`, `PA-CF`, `PC`,
`PVA`, `HIPS`, `PP`, `OTHER`.

The implementation task confirms this list against the `filament_type`
option definition in `src/libslic3r/PrintConfig.cpp` at `v2.4.2` before it
writes the migration CHECK constraint. Any family missing from that version is dropped, not
invented.

- `OTHER` requires `materialOther` (1–32 chars, trimmed). Every other family
  forbids it.
- Manufacturer (required) and product (optional) are free text, 1–64 chars
  each.
- The color name is required, 1–32 chars. The color swatch is optional,
  `#RRGGBB`. Color is supplemental: every list shows the color name, never the
  swatch alone.
- Facet filtering is by family. Later slicer matching (P5/P7) uses the family
  plus diameter.

### D3. Tares

- `spool_tares` holds named reusable tares: `{id, revision, name, weightMg}`.
  Names are unique, compared case-insensitively.
- A Spool can reference a default tare (`tareId`). Scale entry uses it unless
  the user picks another.
- Each measurement **snapshots** the gross and tare values it used in its
  ledger row. Editing or deleting a tare changes only future measurements.
- A tare can be deleted at any time. Spools that referenced it get
  `tareId = null` through `ON DELETE SET NULL`. History keeps its snapshots.
- Scale entry rejects gross < tare with a `VALIDATION` error on `grossMg`.

### D4. Slot capability source

`supportsMultiFilament` does not establish slot count, names, or topology.
Only 12 of 971 catalog variants set it (all Elegoo Centauri). The catalog has
no AMS/MMU data, and a multi-nozzle array means tool heads, not feeder slots.
So **P3 derives no slot layout automatically.**

- Every Printer has an ordered layout of **1 to 16** Material Slots.
- A slot has an internal `id`, a `position`, a `name`, and an optional
  `feederLabel` (for example "AMS 1"), each 1–32 chars trimmed.
  - Names are unique per Printer, compared case-insensitively.
  - `feederLabel` only groups slots in the UI. It has no behavior.
- The default layout is one slot named **"Main"**.
  - Migration 0004 backfills it for every existing Printer.
  - `create_printer` and batch create use it when no layout is supplied.
- When the resolved Profile has `supportsMultiFilament`, Equip and the Setup
  tab show one informational line: "This model can feed more than one
  material. Add a slot for each spool position on your feeder." Nothing is
  added for the user.
- Slot constraints (material restrictions, tool mapping) are out of scope.
  A later phase may add them with hardware evidence and a migration.

### D5. Location and occupancy

A Spool is always in exactly one place: **one Material Slot**, or
**storage** (with an optional storage label).

- `spools.slot_id` is nullable and references `material_slots(id)`.
  - A partial unique index on `slot_id WHERE slot_id IS NOT NULL` makes the
    database enforce both rules: a Spool occupies at most one slot, and a
    slot holds at most one Spool.
  - `CHECK (slot_id IS NULL OR storage_label IS NULL)`.
- **Loading is blocked in these cases:**
  - The target slot is removed, or its Printer is archived
    (`VALIDATION`, `fieldPath: "destination.slotId"`).
  - The Spool is archived.
- An **empty** Spool may stay loaded, because a Spool that just ran out is
  still physically on the Printer. It cannot be reserved (see D8).
- **P3 has no "unresolved movement" state.** Every movement commits
  atomically or not at all. The phase plan asks for a blocker for unresolved
  movements. For P3, that blocker is loaded Spools (D10). A Job awaiting an
  explicit load action is a P7 state, and P7 adds its own blocker.

### D6. Movement

`move_spool` is the single movement command. Load, unload, swap, and a move
from one Printer to another are all forms of it.

```text
MoveSpoolRequest {
  operationId: string,              // client-generated UUID; idempotency key
  spoolId: string,
  expectedSpoolRevision: number,
  destination:
    | { kind: "slot", slotId: string,
        expectedOccupantSpoolId: string | null,
        displacedStorageLabel?: string | null }
    | { kind: "storage", storageLabel?: string | null }
}
MoveSpoolResult {
  spools: SpoolRecord[],            // the moved Spool, then the displaced one if any
  printers: PrinterRecord[],        // each Printer whose slots changed
  movements: SpoolMovement[]
}
```

The command runs as one IMMEDIATE transaction:

1. If a movement with this `operationId` already exists, return the current
   state of the Spools it recorded. This is an idempotent replay and writes
   nothing.
2. Check `expectedSpoolRevision`. A mismatch fails with `CONFLICT`.
3. For a slot destination, compare the slot's current occupant with
   `expectedOccupantSpoolId`. A mismatch fails with `CONFLICT`, with
   `details: { slotId, currentOccupantSpoolId }`. This is the
   concurrent-movement guard: two loads into the same empty slot cannot both
   win.
4. If the slot is occupied (and expected to be), move the displaced Spool to
   storage with `displacedStorageLabel`, and write a `displaced` movement row.
5. Move the Spool and write its movement row. Both rows share `operationId`.
6. Bump the revision of each Spool touched, and the revision of each Printer
   whose occupancy changed. Slot occupancy is part of `PrinterRecord`.
7. Commit, then emit events (D11).

Moving a Spool into the slot it already occupies is `VALIDATION`. A move from
one storage label to another is allowed and writes a movement row.

`spool_movements` is append-only through the API. Each row has:

- `id`, `operationId`, `spoolId`, `reason` (`load`, `unload`, `displaced`,
  `relocate`, `consumed`, `printerArchived`), `occurredAt`.
- The source: `fromSlotId` or `fromStorageLabel`.
- The destination: `toSlotId` or `toStorageLabel`.

The only thing that removes movement rows is Printer deletion (D10).

### D7. Amount ledger, confidence, and corrections

`spools.current_mg` and `spools.confidence` are a cache. They always equal
the latest row in `spool_amount_events` for that Spool. Both are written in
the same transaction.

Each `spool_amount_events` row has:

- `id`, `spoolId`, and `sequence` (unique per Spool, increasing).
- `kind`.
- `beforeMg`, `afterMg`, and `confidenceAfter`.
- Optional `grossMg`, `tareMg` (the D3 snapshots), `reservationId` (D8), and
  `note`.
- `occurredAt`.

| Kind | Written by | Confidence after |
|---|---|---|
| `initial` | `create_spool` | `measured` if weighed or entered as measured; otherwise `estimated` |
| `measurement` | `record_spool_amount` with a direct measured net or scale entry | `measured` |
| `estimate` | `record_spool_amount` with an estimated net | `estimated` |
| `consumption` | P7 through `consume` (D8) | `estimated` |
| `markedEmpty` | the `markEmpty` lifecycle action | `measured` (0 g) |

**Correction semantics.** A correction is any `measurement` or `estimate`
row that follows a `consumption` row. It is not a separate kind. Its
`afterMg − beforeMg` is the correction amount. Earlier rows are never updated
or deleted:

- SQLite triggers reject `UPDATE` and `DELETE` on `spool_amount_events`.
- Spools are archive-only, so no cascade reaches the table.

History shows a correction as "Measured 612 g (corrected +18 g from the
estimate)".

A new sealed Spool is usually entered at its nominal weight as `estimated`.
The Add dialog defaults to that and offers "I weighed it" to switch to
measured or scale entry.

### D8. Reservations and availability

Reservations belong to P7, but P3 owns the arithmetic and the table.

```text
spool_reservations(
  id, spool_id → spools, holder_kind TEXT, holder_id TEXT,
  amount_mg > 0, state: active | unresolved | released | consumed,
  created_at, settled_at, operation_id)
```

- `holder_kind` and `holder_id` are opaque in P3. P7 uses
  `("job", <jobId>)` and may rebuild the table to add a foreign key.
- **Availability:**
  `availableMg = currentMg − Σ amount_mg of reservations in {active, unresolved}`.
  It can be negative after an estimate or measurement lowers `currentMg`
  below the reserved amount. The UI shows that as an over-reserved warning.
- **The primitives** live in `spools::reservations`. Each takes the caller's
  `&Transaction`, so P7 can reserve atomically with Job creation.
  - `reserve(tx, spool_id, holder, amount_mg) -> Result<ReservationId, ReservationError>`
    - Fails with `InsufficientAvailable { available_mg }` if the amount
      exceeds `availableMg`.
    - Fails with `SpoolNotReservable` if the Spool is empty or archived.
  - `release(tx, reservation_id)`: `active | unresolved` → `released`.
  - `consume(tx, reservation_id, used_mg, note)`:
    - Moves the reservation to `consumed` and writes one `consumption` ledger
      row.
    - `afterMg = max(0, currentMg − used_mg)`. When it clamps, the row
      records the shortfall in `note`.
    - `used_mg` may differ from the reserved amount.
  - `mark_unresolved(tx, reservation_id)`: `active` → `unresolved`. This is
    P7's deferred reconciliation. The amount stays unavailable.
  - `availability(tx, spool_id) -> Availability { current_mg, reserved_mg, available_mg }`.
- P3 ships no Tauri command that creates reservations. Integration tests call
  the primitives directly. A debug-only fixture seeds a reservation, so the
  `reserved` facet can be demonstrated.
- **Spool lifecycle actions are guarded by reservations.** Archiving or
  marking empty a Spool with `active` or `unresolved` reservations fails with
  `LIFECYCLE_BLOCKED` (code `SPOOL_RESERVED`).
- Moving a reserved Spool is allowed, because reservation is on the Spool,
  not the slot. P7 decides whether its eligibility cares.

### D9. Spool lifecycle and derived facets

Lifecycle is `active | empty | archived`, set by `set_spool_lifecycle` with
one of these actions:

- `markEmpty`:
  - Allowed from `active` only, with no active or unresolved reservations.
  - Writes a `markedEmpty` ledger row (to 0 g). If the Spool is loaded, it
    also unloads it to storage (the optional `storageLabel`) with a `consumed`
    movement. Both happen in one transaction.
  - The UI calls it **Mark empty (used up)**.
- `reactivate`: `empty` → `active`, for a Spool that was marked empty by
  mistake. The user then records an amount.
- `archive`:
  - Allowed from `active` or `empty`.
  - The Spool must be in storage and have no active or unresolved
    reservations.
- `unarchive`: `archived` → the lifecycle it had before, recorded in
  `archived_from`.

Rust derives these facets, which `SpoolRecord` carries:

- `loaded`: `slotId` is not null.
- `reserved`: `reservedMg > 0`.
- `low`: lifecycle is `active` and `currentMg ≤ lowThresholdMg`. This uses
  the physical amount. P7 eligibility uses `availableMg`.
- `confidence`: `measured` or `estimated`.

The facets are orthogonal and can appear together. The default low threshold
is 100 g, and each Spool can edit its own.

`spool_number` is a unique integer. Each new Spool gets
`max(spool_number) + 1`, allocated in the create transaction, so it is never
reused.

### D10. Printer archive and delete

This decision follows the user's decisions 1 and 2.

**Archive.** `archive_printer` gains `spoolDispositions` and
`operationId`:

```text
ArchivePrinterRequest {
  id, expectedRevision, operationId,
  spoolDispositions: [{ spoolId, expectedSpoolRevision,
    disposition:
      | { kind: "storage", storageLabel?: string | null }
      | { kind: "slot", slotId, expectedOccupantSpoolId: string | null,
          displacedStorageLabel?: string | null }
      | { kind: "markEmpty", storageLabel?: string | null } }]
}
```

- A new `LoadedSpoolBlockers` source is registered in `blocker_sources()`.
  It contributes `SPOOLS_LOADED` for `archive` while any slot on the Printer
  is occupied, and for `delete` as a safety check.
- `LifecycleEligibility` gains `loadedSpools: SpoolRecord[]`, so the archive
  dialog can prompt without a second call.
- Inside the archive transaction, before eligibility is evaluated:
  1. Validate that the dispositions cover every currently loaded Spool
     exactly once. Otherwise fail with `VALIDATION` on `spoolDispositions`.
  2. Validate that each `slot` destination is on a different, non-archived
     Printer.
  3. Apply each disposition with the D6 and D9 logic. All movement rows share
     `operationId` and use reason `printerArchived`, except `markEmpty`,
     which uses `consumed`.
  4. Re-evaluate eligibility. `SPOOLS_LOADED` is now clear, so set
     `archived_at`.
- The P2 order after commit is unchanged: reconciliation guard,
  `stop_and_wait`, then `printer.status.removed`.
- A disposition that fails (for example a `CONFLICT` on the destination slot)
  rolls back the whole archive. Nothing is half-applied.
- An archived Printer keeps its slot layout and its movement history, and
  can be viewed.
- Unarchive restores it with empty slots.

**Delete.** Only archived Printers can be deleted (P2). An archived Printer
cannot hold Spools, and the `SPOOLS_LOADED` delete check proves it. On
delete:

- `material_slots` rows cascade (`ON DELETE CASCADE` from `printers`).
- `spool_movements` rows that reference any of its slots cascade too
  (`from_slot_id` and `to_slot_id` are both `ON DELETE CASCADE`).
- Spools, their amount ledgers, and their reservations are untouched. Every
  Spool is already in storage or on another Printer.

**Stated consequence.** A single movement row from this Printer's slot
straight to another Printer's slot is deleted with this Printer. The other
Printer's history then starts that Spool's stay at its next movement. This is
accepted under decision 2, and the delete confirmation says so: "Spool
movement history involving this Printer will be deleted."

### D11. Events, backfill, and availability signal

**Stream.** Inventory events use the existing `farm3d-event-v1` channel and
envelope, on their own `inventory` stream with its own `streamId` and
sequence:

| Type | Subject | Payload |
|---|---|---|
| `spool.changed` | `spool/<id>` | `SpoolRecord` |
| `printer.slots.changed` | `printer/<id>` | `{ printerId, revision, materialSlots }` |
| `spool.availability.changed` | `spool/<id>` | `{ spoolId, availability }` |

**Backfill.** `list_spools` returns
`InventorySnapshot { streamId, snapshotSequence, spools, tares }`. The
frontend subscribes before it backfills, as `printer-status-store` does, and
drops events at or below `snapshotSequence`.

**Availability signal for P7.** The same moments publish
`InventoryChange { spool_ids, printer_ids }` on a `tokio::sync::broadcast`
held in `RuntimeServices`. P7's evaluator subscribes to it. P3 has only a test
subscriber.

**When events go out.** Events and the broadcast are sent after commit only,
and never for a rolled-back transaction or an idempotent replay.

### D12. Printer setup integration

- `PrinterRecord` (`ResolvedPrinter`) gains
  `materialSlots: MaterialSlot[]`.
  - `MaterialSlot` is `{ id, position, name, feederLabel, occupantSpoolId }`.
  - Removed slots are excluded.
- `create_printer` gains two options:
  - `slotLayout?: [{ name, feederLabel? }]`, defaulting to `[{ name: "Main" }]`.
  - `initialLoads?: [{ slotIndex, spoolId, expectedSpoolRevision }]`. Each
    Spool must be `active` and in storage. They load in the create
    transaction, with `load` movements sharing one generated `operationId`.
- `set_material_slot_layout { printerId, expectedRevision, slots: [{ id?, name, feederLabel? }] }`
  sets the layout:
  - Array order is the new order.
  - An entry without an `id` creates a slot.
  - An existing slot missing from the array is **soft-removed**
    (`removed_at`), so its history survives until the Printer is deleted.
  - Removing an occupied slot fails with `SLOT_OCCUPIED`
    (`details: { slotId, spoolId }`). The UI offers "Unload first".
  - Returns `PrinterMutationResult`.
- **Batch:** `BatchShared` gains `slotLayout?`. It is copied into each row's
  `CreatePrinterOptions` as its own independent rows with fresh slot ids.
  There is no batch or shared layout entity. Batch never loads Spools (user
  decision 3). The Results step shows **Equip** on each created row, which
  opens that Printer's Setup tab at its Material Slots section.
- **Export and import:** the Printers export becomes `schemaVersion: 3`, with
  `materialSlots: [{ name, feederLabel }]` per Printer and no occupancy.
  Import accepts v1, v2 (default layout), and v3.

### D13. Commands and errors

New commands are registered in `generate_handler!`, `COMMAND_NAMES`,
`COMMAND_CONTRACTS`, and the ts-rs export registry:

| Command | Args → Result |
|---|---|
| `list_spools` | `{}` → `InventorySnapshot` |
| `spool_history` | `{ spoolId }` → `{ movements, amountEvents, reservations }` |
| `create_spool` | `{ fields, initialAmount, storageLabel? }` → `SpoolMutationResult` |
| `update_spool` | `{ id, expectedRevision, patch }` → `SpoolMutationResult` |
| `record_spool_amount` | `{ id, expectedRevision, entry, note? }` → `SpoolMutationResult` |
| `move_spool` | `MoveSpoolRequest` → `MoveSpoolResult` |
| `set_spool_lifecycle` | `{ id, expectedRevision, action, storageLabel? }` → `SpoolMutationResult` |
| `create_tare`, `update_tare`, `delete_tare` | the usual revisioned shapes → `TareMutationResult` |
| `set_material_slot_layout` | see D12 → `PrinterMutationResult` |

These existing commands change:

- `create_printer` and `create_printers_batch` gain `slotLayout` (D12), and
  `create_printer` also gains `initialLoads`.
- `archive_printer` gains `operationId` and `spoolDispositions` (D10).
- `printer_lifecycle_eligibility` gains `loadedSpools` (D10).

`initialAmount` and `entry` have these shapes:

- `{ kind: "net", netMg, confidence }`
- `{ kind: "scale", grossMg, tareId? , tareMg? }`

Exactly one of `tareId` or `tareMg` is given.

`SpoolMutationResult` is `{ spool: SpoolRecord, printers: PrinterRecord[], warnings }`.
It includes every Printer whose occupancy changed.

`ErrorCode` gains `SLOT_OCCUPIED`. `LifecycleBlockerCode` gains
`SPOOLS_LOADED` and `SPOOL_RESERVED`. `LIFECYCLE_BLOCKED` gets a message
that isn't specific to Printers.

The revision `CONFLICT` shape follows P2 (`classify_entity_write`).
`ReservationError` is a Rust-only type until P7 maps it to a command error.

## Backend model

### Module layout

`src-tauri/src/spools/`:

- `mod.rs`: domain types and validation.
- `weight.rs`
- `repository.rs`: SQL.
- `movement.rs`: D6, shared by `move_spool`, archive dispositions, and
  initial loads.
- `ledger.rs`: D7.
- `reservations.rs`: D8.
- `slots.rs`: layout, D12.
- `lifecycle_blockers.rs`: D10 source.
- `events.rs`: D11.
- `commands.rs`.

Printer code changes only where it has to:

- `create.rs` and `batch.rs`: pass the layout through.
- `lifecycle.rs`: registry entry and the eligibility field.
- `repository.rs`: the archive transaction calls
  `spools::movement::apply_dispositions`.
- `commands.rs`: export v3.

Following the phase rule, `printers.rs` must not absorb Spool logic.

### Migration `0004_p3_spools_material_slots.sql`

```sql
CREATE TABLE material_slots (
  id TEXT PRIMARY KEY,
  printer_id TEXT NOT NULL REFERENCES printers(id) ON DELETE CASCADE,
  -- 0–15 live; up to 115 transiently while reordering (Rust enforces 0–15)
  position INTEGER NOT NULL CHECK (position BETWEEN 0 AND 115),
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 32 AND name = trim(name)),
  feeder_label TEXT CHECK (feeder_label IS NULL OR (length(feeder_label) BETWEEN 1 AND 32
                           AND feeder_label = trim(feeder_label))),
  removed_at TEXT,
  created_at TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX material_slots_live_name
  ON material_slots(printer_id, lower(name)) WHERE removed_at IS NULL;
CREATE UNIQUE INDEX material_slots_live_position
  ON material_slots(printer_id, position) WHERE removed_at IS NULL;

CREATE TABLE spool_tares (
  id TEXT PRIMARY KEY, revision INTEGER NOT NULL CHECK (revision >= 1),
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64 AND name = trim(name)),
  weight_mg INTEGER NOT NULL CHECK (weight_mg BETWEEN 0 AND 5000000),
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX spool_tares_name ON spool_tares(lower(name));

CREATE TABLE spools (
  id TEXT PRIMARY KEY, revision INTEGER NOT NULL CHECK (revision >= 1),
  spool_number INTEGER NOT NULL UNIQUE CHECK (spool_number >= 1),
  manufacturer TEXT NOT NULL, product TEXT,
  material_family TEXT NOT NULL CHECK (material_family IN (/* D2 list */)),
  material_other TEXT,
  color_name TEXT NOT NULL, color_hex TEXT CHECK (color_hex IS NULL OR color_hex GLOB '#[0-9A-F][0-9A-F][0-9A-F][0-9A-F][0-9A-F][0-9A-F]'),
  diameter TEXT NOT NULL CHECK (diameter IN ('1.75', '2.85')),
  nominal_mg INTEGER NOT NULL CHECK (nominal_mg BETWEEN 1000 AND 50000000),
  current_mg INTEGER NOT NULL CHECK (current_mg BETWEEN 0 AND 50000000),
  confidence TEXT NOT NULL CHECK (confidence IN ('measured', 'estimated')),
  low_threshold_mg INTEGER NOT NULL DEFAULT 100000 CHECK (low_threshold_mg BETWEEN 0 AND 50000000),
  tare_id TEXT REFERENCES spool_tares(id) ON DELETE SET NULL,
  lifecycle TEXT NOT NULL CHECK (lifecycle IN ('active', 'empty', 'archived')),
  archived_from TEXT CHECK (archived_from IS NULL OR archived_from IN ('active', 'empty')),
  slot_id TEXT REFERENCES material_slots(id),
  storage_label TEXT CHECK (storage_label IS NULL OR (length(storage_label) BETWEEN 1 AND 64
                            AND storage_label = trim(storage_label))),
  last_measured_at TEXT, notes TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
  CHECK ((material_family = 'OTHER') = (material_other IS NOT NULL)),
  CHECK (slot_id IS NULL OR storage_label IS NULL),
  CHECK (lifecycle <> 'archived' OR slot_id IS NULL),
  CHECK ((lifecycle = 'archived') = (archived_from IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX spools_slot_occupancy ON spools(slot_id) WHERE slot_id IS NOT NULL;

CREATE TABLE spool_movements (
  id TEXT PRIMARY KEY, operation_id TEXT NOT NULL,
  spool_id TEXT NOT NULL REFERENCES spools(id),
  reason TEXT NOT NULL CHECK (reason IN ('load','unload','displaced','relocate','consumed','printerArchived')),
  from_slot_id TEXT REFERENCES material_slots(id) ON DELETE CASCADE, from_storage_label TEXT,
  to_slot_id TEXT REFERENCES material_slots(id) ON DELETE CASCADE, to_storage_label TEXT,
  occurred_at TEXT NOT NULL
) STRICT;
CREATE INDEX spool_movements_spool ON spool_movements(spool_id, occurred_at);
CREATE INDEX spool_movements_operation ON spool_movements(operation_id);

CREATE TABLE spool_amount_events (
  id TEXT PRIMARY KEY, spool_id TEXT NOT NULL REFERENCES spools(id),
  sequence INTEGER NOT NULL, kind TEXT NOT NULL
    CHECK (kind IN ('initial','measurement','estimate','consumption','markedEmpty')),
  before_mg INTEGER, after_mg INTEGER NOT NULL,
  confidence_after TEXT NOT NULL CHECK (confidence_after IN ('measured','estimated')),
  gross_mg INTEGER, tare_mg INTEGER, reservation_id TEXT, note TEXT,
  occurred_at TEXT NOT NULL,
  UNIQUE (spool_id, sequence)
) STRICT;
CREATE TRIGGER spool_amount_events_no_update BEFORE UPDATE ON spool_amount_events
  BEGIN SELECT RAISE(ABORT, 'spool_amount_events is append-only'); END;
CREATE TRIGGER spool_amount_events_no_delete BEFORE DELETE ON spool_amount_events
  BEGIN SELECT RAISE(ABORT, 'spool_amount_events is append-only'); END;

CREATE TABLE spool_reservations (
  id TEXT PRIMARY KEY, spool_id TEXT NOT NULL REFERENCES spools(id),
  holder_kind TEXT NOT NULL, holder_id TEXT NOT NULL,
  amount_mg INTEGER NOT NULL CHECK (amount_mg > 0),
  state TEXT NOT NULL CHECK (state IN ('active','unresolved','released','consumed')),
  operation_id TEXT NOT NULL, created_at TEXT NOT NULL, settled_at TEXT
) STRICT;
CREATE INDEX spool_reservations_open ON spool_reservations(spool_id)
  WHERE state IN ('active','unresolved');
```

- **Rust post-step:** insert one `Main` slot (`slt-<uuid>`, position 0) for
  every existing Printer, archived ones included, in the same exclusive
  transaction.
- **`spool_movements` delete behavior:** `from_slot_id` and `to_slot_id` use
  `ON DELETE CASCADE`. Soft removal never deletes a slot row, so only Printer
  deletion reaches them.
- **Tests:** the ledger, checksum, crash-boundary, and v3→v4 upgrade tests
  follow `tests/p2_migration.rs`. The schema-inventory pin tests in
  `persistence/mod.rs` are updated.

## Frontend architecture

### State

**`src/spools/spool-store.ts`** is a module-level store, separate from
`printer-store.ts`. It holds:

- `spools`, `tares`, `loaded`, `error`, and `pending` (keyed by
  `operationId`).
- The inventory stream listener.

Actions are `createSpool`, `updateSpool`, `recordAmount`, `moveSpool`,
`setLifecycle`, the tare CRUD actions, and `loadHistory(spoolId)`.

**Movement is optimistic.** `moveSpool` does the following:

1. Applies the expected placement locally and marks both Spools `pending`.
2. Calls `move_spool`.
3. Settles:
   - On success, it replaces the Spools from the result and hands the
     returned `PrinterRecord`s to `printer-store` through its existing
     `spliceResolved`.
   - On error, it restores the pre-move snapshot. A `CONFLICT` reloads the
     affected Spool, then surfaces the error inline.
4. Late events at or below the settled revision are ignored.

Other mutations are not optimistic, which matches P2.

**Pure modules:**

- `src/spools/weight.ts`: parse and format grams ⇄ mg (D1).
- `src/spools/facets.ts`: compose filters over the Rust-derived facets
  (lifecycle, location, loaded, reserved, low, confidence, material, Printer).
  No facet is re-derived in TypeScript.
- `src/spools/materials.ts`: the D2 display labels.

**Web fallback:** an in-memory fixture of about 8 Spools across states,
2 tares, and loads onto the existing fixture Printers. It covers moves and
lifecycle changes. The fixture enforces one Spool per slot so the UI behaves
honestly. It does not model reservations beyond one seeded `reserved` Spool.

### Components

**Design-system additions.** Each has at least two consumers, now or planned:

- `DataTable`:
  - Column definitions, sortable headers, and single-row selection with
    arrow keys and Enter.
  - A sticky header, horizontal overflow inside the table, and dense row
    height.
  - Consumers: Spool inventory now, P7 Queue and P4 Library later. The P2
    `BatchRowsTable` stays as it is.
- `Timeline`:
  - A vertical event list with a timestamp, severity-free marker, title, and
    detail.
  - Consumers: Spool history now, Job and Incident timelines later.
- `ColorSwatch`:
  - A small token-bordered square with an accessible name. It never stands
    alone.
  - Consumers: Spool rows, slots, and pickers.

All three use tokens only, and each is added to `components/index.ts` and
`Showcase.tsx`.

**Shell:**

- `ActivityBar` gains **Spools** (a physical-spool icon), and `App.tsx`
  adds `spools` to `availableDestinations`. The P1 test that asserts no
  spools button is updated.
- The badge counts `low` Spools. The reconciliation-needed count joins it in
  P7.

**`screens/SpoolInventory.tsx`:**

- A toolbar with search (manufacturer, product, color, number, storage
  label), facet chips (Loaded, Reserved, Low, Measured, Estimated), and
  filters for lifecycle (Active by default, Empty, Archived, All),
  Material, and Printer.
- A `DataTable` with these columns: `#`, Material, Color, Manufacturer and
  product, Remaining (with an estimated marker), Available (when it differs),
  Location, and Facets.
- The empty state ("No Spools yet — Add Spool") is distinct from the
  filtered-empty state ("No Spools match" with **Clear filters**).
- Selecting a row opens the right dock and updates the deep link:
  `#nav=v1/spools/spool/<id>`.

**`screens/SpoolDetailDock.tsx`:** its sections are summary, amount and
availability, location, and history (a `Timeline` that merges movements and
amount events by time). Its actions are **Record amount**, **Move…**,
**Unload**, **Edit**, and the lifecycle menu.

**Dialogs:**

- `SpoolFormDialog`: add and edit, including a nominal-weight quick pick
  (250 g, 500 g, 750 g, 1 kg, 2 kg, 3 kg, 5 kg) and initial amount entry.
- `RecordAmountDialog`: a **Net** / **Scale** segmented choice. Scale shows
  the tare select and a live net preview.
- `MoveSpoolDialog`:
  - The destination is Storage (with a storage-label combobox of existing
    labels) or a Printer, then a Slot. Occupied slots show their occupant.
  - Choosing an occupied slot shows "Swap: #7 PETG Black goes to storage"
    with a label field.
- `TareManagerDialog`: opened from the inventory toolbar menu.

**`screens/MaterialSlotsEditor.tsx`** is one component in two modes.

- In layout mode it edits a list of slots: add, rename, feeder label,
  move up and down, and remove.
  - Removing an occupied slot is disabled, with "Unload first".
  - It shows the D4 multi-material hint.
- In occupancy mode each slot row also gets **Load…**, **Swap…**, and
  **Unload**.

It is used in three places:

- Wizard **Equip**: layout, plus optional initial loads from storage.
- Batch **Shared**: layout only.
- The Setup tab's **Material Slots** section: layout and occupancy.

**Printer Status tab:** a Material Slots list showing each slot's name, its
occupant (number, material, color swatch and name, remaining with a
confidence marker, and Low and Reserved chips) or "Empty". Selecting an
occupant deep-links to the Spool.

**Wizard:** `STEP_ORDER` becomes
`["identify", "connect", "equip", "operate", "review"]`. Review lists the
slots and initial loads.

**Batch dialog:**

- Shared gains the slot layout editor.
- Results rows gain **Equip** for `created` and `createdSetupIncomplete`
  outcomes. It closes the dialog and opens that Printer's Setup tab at
  Material Slots.

**Archive flow:**

- The Setup tab's **Archive** fetches eligibility. If `loadedSpools` is
  non-empty, it opens `ArchivePrinterDialog`.
- The dialog has one row per loaded Spool, each with a disposition select:
  - **Storage** (a label field).
  - **Another Printer's slot** (a Printer/slot picker, with swap handling as
    in Move).
  - **Mark empty (used up)**.
- **Archive** submits the dispositions and `operationId`, and is disabled
  until every row has a choice.
- With no loaded Spools, the P2 confirm remains.

**Delete confirmation** adds: "Spool movement history involving this Printer
will be deleted."

## Errors and recovery

| Situation | Behavior |
|---|---|
| Two loads race for one slot | The loser gets `CONFLICT` with the current occupant. Its optimistic move reverts, and the dialog stays open showing the new occupant. |
| Spool revision stale | `CONFLICT`. The Spool reloads and the user retries. |
| Move retried after a lost response | The same `operationId` replays and returns current state without a duplicate movement. |
| Scale gross < tare | `VALIDATION` on `grossMg`, inline. |
| Removing an occupied slot | `SLOT_OCCUPIED`. **Unload first** is offered. |
| Archive a Printer with loaded Spools, no dispositions | `LIFECYCLE_BLOCKED` / `SPOOLS_LOADED`. The UI always prompts first, so this is a safety check. |
| One archive disposition fails | The whole archive rolls back and the dialog shows the row error. |
| Archive or mark empty a reserved Spool | `LIFECYCLE_BLOCKED` / `SPOOL_RESERVED`. |
| Load into an archived Printer's slot | `VALIDATION` on `destination.slotId`. |
| Web mode | All flows work against fixtures. No path claims desktop persistence. |

## Accessibility and adaptation

- `DataTable` row selection works by keyboard (arrow keys, Home and End,
  Enter opens detail), with `aria-selected`.
- Facet chips are toggle buttons with `aria-pressed`.
- Confidence is text plus a marker ("est."), never color alone. Color
  swatches always sit next to the color name.
- Move, Record amount, and Archive dispositions work by keyboard alone.
- Checked at 1440 × 900 and 1024 × 700:
  - At 1024 wide, the dock overlays the table under the existing dock rule.
  - The table scrolls horizontally within its panel.

## Acceptance criteria

1. Migration v3→v4 applies, is ledgered, and survives crash-boundary
   injection. It backfills one `Main` slot per existing Printer, including
   archived ones.
2. The DB rejects a Spool in two slots and two Spools in one slot, even when
   the Rust check is bypassed.
3. Swapping into an occupied slot writes the displaced and loaded movements
   with one `operationId` in one transaction. An injected failure between
   them leaves both Spools where they were.
4. Two concurrent `move_spool` calls into the same empty slot: exactly one
   succeeds, and the other gets `CONFLICT` with the winner as occupant.
5. Replaying a `move_spool` `operationId` writes nothing new and returns
   current state.
6. After restart, a Spool's amount ledger (initial, estimate, measurement
   correction) and its movement history are intact and ordered. The cached
   `current_mg` and `confidence` equal the last ledger row.
7. Ledger rows cannot be updated or deleted (trigger test). A tare edit
   after a measurement doesn't change that measurement's snapshot.
8. Reservation primitives:
   - Reserving beyond `availableMg` fails.
   - `release` restores availability.
   - `consume` writes one `consumption` row and clamps at 0.
   - `unresolved` stays unavailable.
   - A reserved Spool cannot be archived or marked empty.
   - The availability broadcast fires after commit only.
9. Single create with a 4-slot layout plus one initial load, and batch create
   of 3 rows with a shared 4-slot layout, produce independent slot rows (ids
   differ). No batch row has an occupant, and editing one Printer's layout
   leaves the others unchanged.
10. Archive with dispositions (one to storage, one to another Printer's
    occupied slot with displacement, one marked empty) commits atomically.
    The Printer's slot and movement history remain viewable after restart. A
    failing disposition rolls back everything.
11. Deleting that archived Printer removes its slots and every movement row
    touching them, leaves every Spool, ledger, and reservation intact, and
    leaves no dangling reference (`PRAGMA foreign_key_check` is clean).
12. Printers export v3 round-trips the slot layout without occupancy, and v1
    and v2 imports get the default layout.
13. Frontend tests cover:
    - Inventory facets, composed filters, and empty versus filtered-empty.
    - Add, record-amount (net and scale), move (plain, swap, conflict
      revert), and lifecycle.
    - The slot editor in all three hosts, and the Status tab slots.
    - The wizard Equip step, batch Shared layout and Results Equip, and the
      archive disposition dialog.
14. The tracer completes through the Tauri command path. It creates a Spool,
    loads it into one Material Slot, and moves an existing loaded Spool to
    storage. It then reopens storage on the same paths (simulated restart)
    and reads movement and confidence history.
15. Keyboard and viewport checks pass at 1440 × 900 and 1024 × 700, and
    screenshots are recorded in the P3 verification document.

## Delivery strategy

Build it in contract-first vertical slices, each with red and green tests:

1. Migration 0004 and domain types (D1–D5, D9).
2. Ledger and tares (D3, D7).
3. Movement and occupancy (D6).
4. Reservation primitives and availability (D8).
5. Slot layouts in Printer create, batch, and export (D12).
6. Archive dispositions and delete cascade (D10).
7. Commands, events, contracts, and the tracer backend path (D11, D13).
8. Design-system additions (`DataTable`, `Timeline`, `ColorSwatch`).
9. Spool store, pure modules, and web fixtures.
10. The Spools destination: inventory, detail, and dialogs.
11. Printer integration: the Status tab, Setup slots, wizard Equip, batch
    Shared and Results, and the archive dialog.
12. The tracer, `CONTEXT.md` terms, and verification evidence.

The task-level plan will be
`docs/superpowers/plans/2026-09-23-p3-spools-material-slots.md`, written once
this design is approved.
