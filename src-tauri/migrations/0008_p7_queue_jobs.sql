-- P7 D1/D2/D3: Queue Entries, Jobs, the Job timeline, and Reconciliation
-- Requirements. See the P7 design spec's "Schema (migration
-- 0008_p7_queue_jobs.sql)" section for the authoritative column and CHECK
-- list this migration reproduces verbatim.
CREATE TABLE queue_entries (
  id TEXT PRIMARY KEY CHECK (id GLOB 'qen-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  slice_revision_id TEXT NOT NULL REFERENCES slice_revisions(id) ON DELETE RESTRICT,
  lineage_id TEXT NOT NULL CHECK (lineage_id GLOB 'qln-*'),
  copy_index INTEGER NOT NULL CHECK (copy_index BETWEEN 1 AND 50),
  origin_entry_id TEXT REFERENCES queue_entries(id) ON DELETE RESTRICT,
  origin_kind TEXT CHECK (origin_kind IN ('retry','release')),
  state TEXT NOT NULL CHECK (state IN ('queued','assigned','closed')),
  close_reason TEXT CHECK (close_reason IN ('completed','failed','cancelled','released','removed')),
  position INTEGER CHECK (position >= 1),
  policy TEXT NOT NULL CHECK (policy IN ('manual','recommended','automatic')),
  preference TEXT NOT NULL CHECK (preference IN ('loadedFirst','leastRecentlyUsed')),
  estimate_mg INTEGER NOT NULL CHECK (estimate_mg > 0),
  estimate_source TEXT NOT NULL CHECK (estimate_source IN ('sliceEstimate','fileClaimConfirmed','operatorEntered')),
  manual_printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL,
  job_id TEXT REFERENCES jobs(id) ON DELETE RESTRICT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  closed_at TEXT,
  CHECK ((state = 'closed') = (close_reason IS NOT NULL)),
  CHECK ((state = 'closed') = (position IS NULL)),
  CHECK ((state = 'closed') = (closed_at IS NOT NULL)),
  CHECK ((origin_entry_id IS NULL) = (origin_kind IS NULL)),
  CHECK (state <> 'assigned' OR job_id IS NOT NULL),
  CHECK (state <> 'queued' OR job_id IS NULL),
  CHECK (manual_printer_id IS NULL OR policy = 'manual' OR state = 'closed')
) STRICT;
CREATE UNIQUE INDEX queue_entries_open_position ON queue_entries(position) WHERE position IS NOT NULL;
CREATE INDEX queue_entries_lineage ON queue_entries(lineage_id, copy_index);
CREATE INDEX queue_entries_slice_revision ON queue_entries(slice_revision_id);
CREATE UNIQUE INDEX queue_entries_one_successor ON queue_entries(origin_entry_id) WHERE origin_entry_id IS NOT NULL;
CREATE INDEX queue_entries_manual_printer ON queue_entries(manual_printer_id) WHERE manual_printer_id IS NOT NULL;

CREATE TABLE jobs (
  id TEXT PRIMARY KEY CHECK (id GLOB 'job-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  queue_entry_id TEXT NOT NULL UNIQUE REFERENCES queue_entries(id) ON DELETE RESTRICT,
  slice_revision_id TEXT NOT NULL REFERENCES slice_revisions(id) ON DELETE RESTRICT,
  printer_id TEXT NOT NULL REFERENCES printers(id) ON DELETE RESTRICT,
  printer_snapshot_json TEXT NOT NULL CHECK (json_valid(printer_snapshot_json)),
  spool_id TEXT NOT NULL REFERENCES spools(id) ON DELETE RESTRICT,
  reservation_id TEXT NOT NULL UNIQUE REFERENCES spool_reservations(id) ON DELETE RESTRICT,
  estimate_mg INTEGER NOT NULL CHECK (estimate_mg > 0),
  state TEXT NOT NULL CHECK (state IN ('assigned','staging','awaitingStart','starting','printing',
                                       'paused','completed','failed','cancelled','outcomeUnknown')),
  cancel_reason TEXT CHECK (cancel_reason IN ('releasedBeforeStart','cancelledBeforeStart',
                                              'cancelledByOperator','hostCancelled','operatorDeclared')),
  settlement TEXT NOT NULL CHECK (settlement IN ('open','notRequired','pending','deferred','settled')),
  settlement_method TEXT CHECK (settlement_method IN ('estimated','measured')),
  assigned_by TEXT NOT NULL CHECK (assigned_by IN ('operator','automatic')),
  manual_facts_acknowledged INTEGER NOT NULL DEFAULT 0 CHECK (manual_facts_acknowledged IN (0,1)),
  start_confirmation TEXT CHECK (start_confirmation IN ('bedClear','unattended')),
  upload_host_operation_id TEXT REFERENCES host_operations(id) ON DELETE RESTRICT,
  active_host_operation_id TEXT REFERENCES host_operations(id) ON DELETE RESTRICT,
  start_host_operation_id TEXT REFERENCES host_operations(id) ON DELETE RESTRICT,
  host_path TEXT,
  history_mark INTEGER CHECK (history_mark IS NULL OR history_mark >= 0),
  host_job_id INTEGER CHECK (host_job_id IS NULL OR host_job_id >= 0),
  max_progress_pct INTEGER NOT NULL DEFAULT 0 CHECK (max_progress_pct BETWEEN 0 AND 100),
  inconclusive_checks INTEGER NOT NULL DEFAULT 0 CHECK (inconclusive_checks >= 0),
  host_unreachable_since TEXT,
  last_failure_json TEXT CHECK (last_failure_json IS NULL OR json_valid(last_failure_json)),
  correction_event_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  started_at TEXT,
  ended_at TEXT,
  CHECK ((state = 'cancelled') = (cancel_reason IS NOT NULL)),
  CHECK ((state IN ('completed','failed','cancelled')) = (ended_at IS NOT NULL)),
  CHECK ((state IN ('completed','failed','cancelled')) = (settlement <> 'open')),
  CHECK ((settlement = 'settled') = (settlement_method IS NOT NULL)),
  CHECK (settlement <> 'notRequired' OR cancel_reason IN ('releasedBeforeStart','cancelledBeforeStart')),
  CHECK (state NOT IN ('awaitingStart','starting','printing','paused') OR upload_host_operation_id IS NOT NULL),
  CHECK (state NOT IN ('starting','printing','paused') OR start_confirmation IS NOT NULL),
  CHECK (state NOT IN ('printing','paused') OR (started_at IS NOT NULL AND history_mark IS NOT NULL)),
  CHECK (state NOT IN ('starting','printing','paused') OR start_host_operation_id IS NOT NULL),
  CHECK (host_unreachable_since IS NULL OR state IN ('printing','paused')),
  CHECK (correction_event_id IS NULL OR state = 'completed')
) STRICT;
CREATE UNIQUE INDEX jobs_one_active_per_printer ON jobs(printer_id)
  WHERE state NOT IN ('completed','failed','cancelled');
CREATE INDEX jobs_printer_ended ON jobs(printer_id, ended_at);
CREATE INDEX jobs_slice_revision ON jobs(slice_revision_id);
CREATE INDEX jobs_state ON jobs(state);

CREATE TABLE job_events (
  id TEXT PRIMARY KEY CHECK (id GLOB 'jev-*'),
  job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE RESTRICT,
  sequence INTEGER NOT NULL CHECK (sequence >= 1),
  kind TEXT NOT NULL,
  from_state TEXT,
  to_state TEXT NOT NULL,
  operation_id TEXT,
  host_operation_id TEXT REFERENCES host_operations(id) ON DELETE RESTRICT,
  detail_json TEXT CHECK (detail_json IS NULL OR json_valid(detail_json)),
  at TEXT NOT NULL,
  UNIQUE (job_id, sequence)
) STRICT;
CREATE TRIGGER job_events_append_only_u BEFORE UPDATE ON job_events
  BEGIN SELECT RAISE(ABORT, 'job_events is append-only'); END;
CREATE TRIGGER job_events_append_only_d BEFORE DELETE ON job_events
  BEGIN SELECT RAISE(ABORT, 'job_events is append-only'); END;

CREATE TABLE reconciliation_requirements (
  id TEXT PRIMARY KEY CHECK (id GLOB 'rrq-*'),
  job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE RESTRICT,
  kind TEXT NOT NULL CHECK (kind IN ('materialReconciliation','jobOutcomeUnknown')),
  status TEXT NOT NULL CHECK (status IN ('pending','deferred','resolved')),
  spool_id TEXT REFERENCES spools(id) ON DELETE RESTRICT,
  reservation_id TEXT REFERENCES spool_reservations(id) ON DELETE RESTRICT,
  opened_at TEXT NOT NULL,
  deferred_at TEXT,
  resolved_at TEXT,
  resolution_json TEXT CHECK (resolution_json IS NULL OR json_valid(resolution_json)),
  UNIQUE (job_id, kind),
  CHECK (kind <> 'materialReconciliation' OR (spool_id IS NOT NULL AND reservation_id IS NOT NULL)),
  CHECK (kind <> 'jobOutcomeUnknown' OR status <> 'deferred'),
  CHECK ((status = 'resolved') = (resolved_at IS NOT NULL AND resolution_json IS NOT NULL)),
  CHECK (status <> 'deferred' OR deferred_at IS NOT NULL)
) STRICT;
CREATE INDEX reconciliation_requirements_open ON reconciliation_requirements(status) WHERE status <> 'resolved';

-- The new column defaults to NULL, so this is legal without rebuilding
-- the table, and P6's `host_operations_terminal_immutable` trigger is
-- unaffected: it doesn't list `job_id`, so any UPDATE of `job_id` on a
-- terminal row still raises. P7 only ever writes `job_id` at insert
-- (`NewHostOperation.job_id`), never as an UPDATE.
ALTER TABLE host_operations ADD COLUMN job_id TEXT REFERENCES jobs(id);
CREATE INDEX host_operations_job ON host_operations(job_id) WHERE job_id IS NOT NULL;
CREATE INDEX spool_reservations_holder ON spool_reservations(holder_kind, holder_id);

-- P6's operations ledger gains P7's 15 idempotent commands. SQLite can't
-- alter a CHECK, so the table is rebuilt: create, copy every row, drop
-- the old table, rename (as 0007 does for 0006's table).
CREATE TABLE operations_p7 (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('moveSpool','archivePrinter','spoolLifecycle','startSlice','createExternalSliceRevision','stageSliceRevision','startStagedArtifact','pauseHostPrint','resumeHostPrint','cancelHostPrint','abandonHostOperation','addToQueue','updateQueueEntry','moveQueueEntry','removeQueueEntry','assignQueueEntry','stageJob','startJob','pauseJob','resumeJob','cancelJob','releaseJob','retryJob','declareJobOutcome','settleJobMaterial','correctJobMaterial')),
  request_digest TEXT NOT NULL,
  created_at TEXT NOT NULL
) STRICT;
INSERT INTO operations_p7(id, kind, request_digest, created_at)
  SELECT id, kind, request_digest, created_at FROM operations;
DROP TABLE operations;
ALTER TABLE operations_p7 RENAME TO operations;
