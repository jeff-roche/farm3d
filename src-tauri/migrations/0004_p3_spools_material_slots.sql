-- P3: Spools and Material Slots (spec "Migration 0004"). The Rust post-step
-- (`backfill_main_slots`) inserts one live `Main` slot at position 0 for
-- every existing Printer, archived ones included.
--
-- The `material_family` CHECK list is D2's fixed list, confirmed against
-- OrcaSlicer's `filament_type` option at the pinned catalog tag `v2.4.2`
-- (see the Task 1 report): all 13 real families are present verbatim in
-- that option's values, so the D2 list is unchanged.
--
-- `position` allows 0-115 rather than the live 0-15 range (global
-- constraints clarification 2): `set_material_slot_layout` (Task 5)
-- reorders in two passes, first bumping every live slot to `position + 100`
-- so the reorder never collides with `material_slots_live_position`. Rust
-- enforces the real 0-15 range after the second pass.
CREATE TABLE material_slots (
  id TEXT PRIMARY KEY,
  printer_id TEXT NOT NULL REFERENCES printers(id) ON DELETE CASCADE,
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

-- D3: reusable, named empty-spool tares. Each measurement snapshots the
-- gross/tare values it used, so editing or deleting a tare never rewrites
-- past ledger rows.
CREATE TABLE spool_tares (
  id TEXT PRIMARY KEY, revision INTEGER NOT NULL CHECK (revision >= 1),
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64 AND name = trim(name)),
  weight_mg INTEGER NOT NULL CHECK (weight_mg BETWEEN 0 AND 5000000),
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX spool_tares_name ON spool_tares(lower(name));

-- D1-D5, D9: the Spool record. `current_mg`/`confidence` are a cache that
-- always equals the latest `spool_amount_events` row for the Spool (D7),
-- written in the same transaction as that row.
CREATE TABLE spools (
  id TEXT PRIMARY KEY, revision INTEGER NOT NULL CHECK (revision >= 1),
  spool_number INTEGER NOT NULL UNIQUE CHECK (spool_number >= 1),
  manufacturer TEXT NOT NULL, product TEXT,
  material_family TEXT NOT NULL CHECK (material_family IN (
    'PLA', 'PLA-CF', 'PETG', 'PET-CF', 'ABS', 'ASA', 'TPU', 'PA', 'PA-CF', 'PC', 'PVA', 'HIPS', 'PP', 'OTHER'
  )),
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

-- D6: the append-only movement history. Only Printer deletion ever removes
-- rows from this table (via the `from_slot_id`/`to_slot_id` cascades).
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

-- D7: the append-only amount ledger. Triggers below reject UPDATE/DELETE so
-- a correction is always a new row, never a rewrite of history.
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

-- D8: reservation primitives P3 owns but does not expose as a command.
CREATE TABLE spool_reservations (
  id TEXT PRIMARY KEY, spool_id TEXT NOT NULL REFERENCES spools(id),
  holder_kind TEXT NOT NULL, holder_id TEXT NOT NULL,
  amount_mg INTEGER NOT NULL CHECK (amount_mg > 0),
  state TEXT NOT NULL CHECK (state IN ('active','unresolved','released','consumed')),
  operation_id TEXT NOT NULL, created_at TEXT NOT NULL, settled_at TEXT
) STRICT;
CREATE INDEX spool_reservations_open ON spool_reservations(spool_id)
  WHERE state IN ('active','unresolved');
