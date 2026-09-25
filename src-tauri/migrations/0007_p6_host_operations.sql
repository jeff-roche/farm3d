-- P6 D2: the durable Host Operation record — one row per write-ahead
-- commit of an upload, start, pause, resume, or cancel command. See D2 for
-- the exact columns and CHECKs, D3 for the state machine, and D8 for
-- abandon.
CREATE TABLE host_operations (
  id TEXT PRIMARY KEY CHECK (id GLOB 'hop-*' AND length(id) BETWEEN 5 AND 64),
  operation_id TEXT NOT NULL UNIQUE,
  printer_id TEXT NOT NULL REFERENCES printers(id) ON DELETE RESTRICT,
  kind TEXT NOT NULL CHECK (kind IN ('upload','start','pause','resume','cancel')),
  slice_revision_id TEXT REFERENCES slice_revisions(id) ON DELETE SET NULL,
  source_host_operation_id TEXT REFERENCES host_operations(id) ON DELETE SET NULL,
  gcode_sha256 TEXT,
  gcode_size INTEGER CHECK (gcode_size IS NULL OR gcode_size > 0),
  host_path TEXT NOT NULL,
  history_mark INTEGER CHECK (history_mark IS NULL OR history_mark >= 0),
  endpoint_json TEXT NOT NULL CHECK (json_valid(endpoint_json)),
  state TEXT NOT NULL CHECK (state IN ('dispatching','uncertain','reconciling','succeeded','failed','abandoned')),
  failure_json TEXT CHECK (failure_json IS NULL OR json_valid(failure_json)),
  resolution_json TEXT CHECK (resolution_json IS NULL OR json_valid(resolution_json)),
  attempts INTEGER NOT NULL DEFAULT 0,
  last_attempt_at TEXT,
  last_attempt_reason TEXT,
  no_longer_pending INTEGER NOT NULL DEFAULT 0 CHECK (no_longer_pending IN (0,1)),
  abandoned_at TEXT,
  abandon_note TEXT CHECK (abandon_note IS NULL OR length(abandon_note) <= 500),
  created_at TEXT NOT NULL,
  dispatched_at TEXT,
  uncertain_since TEXT,
  resolved_at TEXT,
  -- gcode_sha256/gcode_size are required exactly for upload and start.
  CHECK (
    (kind IN ('upload','start') AND gcode_sha256 IS NOT NULL AND gcode_size IS NOT NULL)
    OR (kind NOT IN ('upload','start') AND gcode_sha256 IS NULL AND gcode_size IS NULL)
  ),
  -- history_mark is required exactly for start.
  CHECK ((kind = 'start') = (history_mark IS NOT NULL)),
  -- source_host_operation_id is start-only.
  CHECK (kind = 'start' OR source_host_operation_id IS NULL)
) STRICT;

-- Answer 2: at most one unresolved (non-terminal) row per Printer.
CREATE UNIQUE INDEX host_operations_printer_unresolved
  ON host_operations(printer_id)
  WHERE state IN ('dispatching','uncertain','reconciling');

CREATE INDEX host_operations_printer_created ON host_operations(printer_id, created_at);

-- D3: a terminal row (succeeded, failed, abandoned) never accepts another
-- UPDATE, with one exception D2 relies on: the ON DELETE SET NULL actions
-- may unlink a terminal row from a deleted Slice Revision or source row
-- (D7: a revision, or a Printer's terminal rows, can be deleted once they
-- are terminal). Every other column must stay exactly as it was, and a
-- link may only ever become NULL.
CREATE TRIGGER host_operations_terminal_immutable
BEFORE UPDATE ON host_operations
WHEN OLD.state IN ('succeeded','failed','abandoned')
  AND NOT (
    (NEW.slice_revision_id IS OLD.slice_revision_id OR NEW.slice_revision_id IS NULL)
    AND (NEW.source_host_operation_id IS OLD.source_host_operation_id
         OR NEW.source_host_operation_id IS NULL)
    AND NEW.id IS OLD.id
    AND NEW.operation_id IS OLD.operation_id
    AND NEW.printer_id IS OLD.printer_id
    AND NEW.kind IS OLD.kind
    AND NEW.gcode_sha256 IS OLD.gcode_sha256
    AND NEW.gcode_size IS OLD.gcode_size
    AND NEW.host_path IS OLD.host_path
    AND NEW.history_mark IS OLD.history_mark
    AND NEW.endpoint_json IS OLD.endpoint_json
    AND NEW.state IS OLD.state
    AND NEW.failure_json IS OLD.failure_json
    AND NEW.resolution_json IS OLD.resolution_json
    AND NEW.attempts IS OLD.attempts
    AND NEW.last_attempt_at IS OLD.last_attempt_at
    AND NEW.last_attempt_reason IS OLD.last_attempt_reason
    AND NEW.no_longer_pending IS OLD.no_longer_pending
    AND NEW.abandoned_at IS OLD.abandoned_at
    AND NEW.abandon_note IS OLD.abandon_note
    AND NEW.created_at IS OLD.created_at
    AND NEW.dispatched_at IS OLD.dispatched_at
    AND NEW.uncertain_since IS OLD.uncertain_since
    AND NEW.resolved_at IS OLD.resolved_at
  )
BEGIN
  SELECT RAISE(ABORT, 'host operation is terminal');
END;

-- P5's operations ledger gains P6's six idempotent commands. SQLite can't
-- alter a CHECK, so the table is rebuilt: create, copy every row, drop the
-- old table, rename. Nothing holds a foreign key to it.
CREATE TABLE operations_p6 (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('moveSpool','archivePrinter','spoolLifecycle','startSlice','createExternalSliceRevision','stageSliceRevision','startStagedArtifact','pauseHostPrint','resumeHostPrint','cancelHostPrint','abandonHostOperation')),
  request_digest TEXT NOT NULL,
  created_at TEXT NOT NULL
) STRICT;
INSERT INTO operations_p6(id, kind, request_digest, created_at)
  SELECT id, kind, request_digest, created_at FROM operations;
DROP TABLE operations;
ALTER TABLE operations_p6 RENAME TO operations;
