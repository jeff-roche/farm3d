CREATE TABLE slicer_runtime_config (
  singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
  revision INTEGER NOT NULL CHECK (revision >= 1),
  engine_path TEXT CHECK (engine_path IS NULL OR length(engine_path) > 0),
  preset_source_path TEXT CHECK (preset_source_path IS NULL OR length(preset_source_path) > 0),
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE slice_preparations (
  id TEXT PRIMARY KEY CHECK (id GLOB 'prp-*' AND length(id) BETWEEN 5 AND 64),
  model_id TEXT NOT NULL UNIQUE REFERENCES library_models(id) ON DELETE CASCADE,
  source_revision_id TEXT NOT NULL REFERENCES model_source_revisions(id) ON DELETE RESTRICT,
  revision INTEGER NOT NULL CHECK (revision >= 1),
  document_json TEXT NOT NULL CHECK (json_valid(document_json) AND json_type(document_json) = 'object'),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE slice_operations (
  id TEXT PRIMARY KEY CHECK (id GLOB 'sop-*' AND length(id) BETWEEN 5 AND 64),
  preparation_id TEXT NOT NULL REFERENCES slice_preparations(id) ON DELETE CASCADE,
  source_revision_id TEXT NOT NULL REFERENCES model_source_revisions(id) ON DELETE RESTRICT,
  plate_key TEXT NOT NULL,
  plate_snapshot_json TEXT NOT NULL CHECK (json_valid(plate_snapshot_json)),
  state TEXT NOT NULL CHECK (state IN ('queued','running','succeeded','failed','cancelled','interrupted')),
  failure_json TEXT CHECK (failure_json IS NULL OR json_valid(failure_json)),
  pid INTEGER,
  pid_started_at INTEGER,
  log_sha256 TEXT REFERENCES content_blobs(sha256),
  slice_revision_id TEXT REFERENCES slice_revisions(id) ON DELETE SET NULL,
  queued_at TEXT NOT NULL,
  started_at TEXT,
  finished_at TEXT,
  CHECK ((state = 'failed') = (failure_json IS NOT NULL)),
  CHECK (state <> 'succeeded' OR slice_revision_id IS NOT NULL OR finished_at IS NOT NULL)
) STRICT;
CREATE INDEX slice_operations_active ON slice_operations(state) WHERE state IN ('queued','running');

CREATE TABLE slice_revisions (
  id TEXT PRIMARY KEY CHECK (id GLOB 'slr-*' AND length(id) BETWEEN 5 AND 64),
  kind TEXT NOT NULL CHECK (kind IN ('farm3d','external')),
  model_id TEXT NOT NULL REFERENCES library_models(id) ON DELETE RESTRICT,
  source_revision_id TEXT NOT NULL REFERENCES model_source_revisions(id) ON DELETE RESTRICT,
  plate_key TEXT,
  plate_index INTEGER,
  plate_name TEXT,
  gcode_sha256 TEXT NOT NULL REFERENCES content_blobs(sha256),
  gcode_size INTEGER NOT NULL CHECK (gcode_size > 0),
  target_json TEXT NOT NULL CHECK (json_valid(target_json)),
  facts_json TEXT NOT NULL CHECK (json_valid(facts_json)),
  requires_manual_printer_selection INTEGER NOT NULL CHECK (requires_manual_printer_selection IN (0,1)),
  estimates_json TEXT NOT NULL CHECK (json_valid(estimates_json)),
  runtime_json TEXT CHECK (runtime_json IS NULL OR json_valid(runtime_json)),
  created_at TEXT NOT NULL,
  -- Spec D14 wrote `plate_index >= 1` alone, which a NULL index passes
  -- (the comparison is NULL, and a NULL CHECK succeeds). D1 requires the
  -- plate identity, so the index is also required to be non-NULL.
  CHECK (
    (kind = 'farm3d' AND plate_key IS NOT NULL AND plate_index IS NOT NULL AND plate_index >= 1
     AND runtime_json IS NOT NULL)
    OR (kind = 'external' AND plate_key IS NULL AND plate_index IS NULL AND plate_name IS NULL AND runtime_json IS NULL)
  )
) STRICT;
CREATE INDEX slice_revisions_model ON slice_revisions(model_id, created_at);
CREATE INDEX slice_revisions_gcode ON slice_revisions(gcode_sha256);

CREATE TRIGGER slice_revisions_immutable
BEFORE UPDATE ON slice_revisions
BEGIN
  SELECT RAISE(ABORT, 'slice revisions are immutable');
END;

CREATE TABLE slice_revision_blobs (
  revision_id TEXT NOT NULL REFERENCES slice_revisions(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK (role IN ('plate3mf','machinePreset','processPreset','filamentPreset','manifest','log')),
  sha256 TEXT NOT NULL REFERENCES content_blobs(sha256),
  PRIMARY KEY (revision_id, role)
) STRICT, WITHOUT ROWID;
CREATE INDEX slice_revision_blobs_sha ON slice_revision_blobs(sha256);

CREATE TRIGGER slice_revision_blobs_immutable
BEFORE UPDATE ON slice_revision_blobs
BEGIN
  SELECT RAISE(ABORT, 'slice revision blobs are immutable');
END;

-- P3's operations ledger gains the two P5 idempotent commands. SQLite
-- can't alter a CHECK, so the table is rebuilt: create, copy every row,
-- drop the old table, rename. Nothing holds a foreign key to it.
CREATE TABLE operations_p5 (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('moveSpool','archivePrinter','spoolLifecycle','startSlice','createExternalSliceRevision')),
  request_digest TEXT NOT NULL,
  created_at TEXT NOT NULL
) STRICT;
INSERT INTO operations_p5(id, kind, request_digest, created_at)
  SELECT id, kind, request_digest, created_at FROM operations;
DROP TABLE operations;
ALTER TABLE operations_p5 RENAME TO operations;
