-- P8 D1/D2/D9: Attention Events, Incidents, camera snapshots, printer
-- cameras, and per-Printer alert defaults. See the P8 design spec's
-- "Schema (migration 0009_p8_attention.sql)" section for the
-- authoritative column and CHECK list this migration reproduces
-- verbatim.
--
-- `incidents` is created before `attention_events`, and `camera_snapshots`
-- before `incident_events`, so every foreign key's target exists.
CREATE TABLE incidents (
  id TEXT PRIMARY KEY CHECK (id GLOB 'inc-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  kind TEXT NOT NULL CHECK (kind IN ('printer.hostFailed','job.failed','job.hostCancelled',
                                     'requirement.jobOutcomeUnknown')),
  printer_id TEXT NOT NULL REFERENCES printers(id) ON DELETE RESTRICT,
  job_id TEXT REFERENCES jobs(id) ON DELETE RESTRICT,
  printer_snapshot_json TEXT NOT NULL CHECK (json_valid(printer_snapshot_json)),
  opened_at TEXT NOT NULL,
  closed_at TEXT,
  CHECK ((kind = 'printer.hostFailed') = (job_id IS NULL))
) STRICT;
CREATE UNIQUE INDEX incidents_one_per_job ON incidents(job_id) WHERE job_id IS NOT NULL;
CREATE INDEX incidents_printer ON incidents(printer_id, opened_at);
CREATE INDEX incidents_open ON incidents(opened_at) WHERE closed_at IS NULL;

CREATE TABLE attention_events (
  id TEXT PRIMARY KEY CHECK (id GLOB 'att-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  dedup_key TEXT NOT NULL CHECK (length(dedup_key) BETWEEN 5 AND 700),
  condition TEXT NOT NULL CHECK (condition IN ('printer.offline','printer.connectionError',
    'printer.hostFailed','job.startConfirmation','job.failed','job.hostCancelled',
    'requirement.materialReconciliation','requirement.jobOutcomeUnknown','spool.low',
    'job.completed')),
  severity TEXT NOT NULL CHECK (severity IN ('fatal','warning','info')),
  requires_action INTEGER NOT NULL CHECK (requires_action IN (0,1)),
  resolution_mode TEXT NOT NULL CHECK (resolution_mode IN ('auto','action','manual')),
  notification_class TEXT NOT NULL CHECK (notification_class IN ('fatal','confirmation',
    'completion','reconciliation','connectivity','inventory')),
  source_kind TEXT NOT NULL CHECK (source_kind IN ('printer','job','reconciliationRequirement','spool')),
  source_id TEXT NOT NULL CHECK (length(CAST(source_id AS BLOB)) BETWEEN 1 AND 512),
  printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL,
  job_id TEXT REFERENCES jobs(id) ON DELETE RESTRICT,
  spool_id TEXT REFERENCES spools(id) ON DELETE RESTRICT,
  requirement_id TEXT REFERENCES reconciliation_requirements(id) ON DELETE RESTRICT,
  incident_id TEXT REFERENCES incidents(id) ON DELETE RESTRICT,
  subject_snapshot_json TEXT NOT NULL CHECK (json_valid(subject_snapshot_json)),
  detail_json TEXT NOT NULL CHECK (json_valid(detail_json)),
  summary TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 500),
  origin TEXT NOT NULL CHECK (origin IN ('live','backfill')),
  first_observed_at TEXT NOT NULL,
  last_observed_at TEXT NOT NULL,
  observation_count INTEGER NOT NULL DEFAULT 1 CHECK (observation_count >= 1),
  recurrence_of TEXT REFERENCES attention_events(id) ON DELETE RESTRICT,
  read_at TEXT,
  acknowledged_at TEXT,
  resolved_at TEXT,
  resolution TEXT CHECK (resolution IN ('conditionCleared','actionCompleted','operatorResolved',
                                        'sourceRemoved')),
  notified_at TEXT,
  evidence_json TEXT CHECK (evidence_json IS NULL
                            OR (json_valid(evidence_json) AND condition = 'job.completed')),
  CHECK (dedup_key = condition || ':' || source_kind || ':' || source_id),
  CHECK (acknowledged_at IS NULL OR read_at IS NOT NULL),
  CHECK (resolved_at IS NULL OR read_at IS NOT NULL),
  CHECK ((resolved_at IS NULL) = (resolution IS NULL)),
  CHECK (resolution <> 'operatorResolved' OR resolution_mode = 'manual'),
  CHECK (source_kind <> 'printer' OR printer_id IS NULL OR printer_id = source_id),
  CHECK (source_kind <> 'job' OR (job_id IS NOT NULL AND job_id = source_id)),
  CHECK (source_kind <> 'reconciliationRequirement'
         OR (requirement_id IS NOT NULL AND requirement_id = source_id)),
  CHECK (source_kind <> 'spool' OR (spool_id IS NOT NULL AND spool_id = source_id)),
  CHECK (recurrence_of IS NULL OR recurrence_of <> id)
) STRICT;
CREATE UNIQUE INDEX attention_events_one_open_per_key ON attention_events(dedup_key)
  WHERE resolved_at IS NULL;
CREATE INDEX attention_events_key_resolved ON attention_events(dedup_key, resolved_at);
CREATE INDEX attention_events_resolved ON attention_events(resolved_at, id);
CREATE INDEX attention_events_open_severity ON attention_events(severity, first_observed_at)
  WHERE resolved_at IS NULL;
CREATE INDEX attention_events_printer ON attention_events(printer_id) WHERE printer_id IS NOT NULL;
CREATE INDEX attention_events_job ON attention_events(job_id) WHERE job_id IS NOT NULL;
CREATE INDEX attention_events_incident ON attention_events(incident_id) WHERE incident_id IS NOT NULL;

CREATE TABLE camera_snapshots (
  id TEXT PRIMARY KEY CHECK (id GLOB 'snp-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  printer_id TEXT NOT NULL REFERENCES printers(id) ON DELETE RESTRICT,
  incident_id TEXT REFERENCES incidents(id) ON DELETE RESTRICT,
  job_id TEXT REFERENCES jobs(id) ON DELETE RESTRICT,
  trigger TEXT NOT NULL CHECK (trigger IN ('incident','completion','manual')),
  operation_id TEXT UNIQUE,
  captured_at TEXT NOT NULL,
  content_type TEXT NOT NULL CHECK (content_type IN ('image/jpeg','image/png')),
  byte_len INTEGER NOT NULL CHECK (byte_len BETWEEN 1 AND 10485760),
  sha256 TEXT NOT NULL CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
  rel_path TEXT NOT NULL UNIQUE CHECK (rel_path GLOB 'snapshots/*' AND rel_path NOT GLOB '*..*'),
  pinned_at TEXT,
  pruned_at TEXT,
  prune_reason TEXT CHECK (prune_reason IN ('age','diskCap','missingFile')),
  CHECK ((pruned_at IS NULL) = (prune_reason IS NULL)),
  CHECK (pinned_at IS NULL OR pruned_at IS NULL OR prune_reason = 'missingFile'),
  CHECK (trigger <> 'incident' OR incident_id IS NOT NULL),
  CHECK (trigger <> 'completion' OR (job_id IS NOT NULL AND incident_id IS NULL)),
  CHECK ((trigger = 'manual') = (operation_id IS NOT NULL))
) STRICT;
CREATE INDEX camera_snapshots_printer ON camera_snapshots(printer_id, captured_at);
CREATE INDEX camera_snapshots_incident ON camera_snapshots(incident_id) WHERE incident_id IS NOT NULL;
CREATE INDEX camera_snapshots_job ON camera_snapshots(job_id) WHERE job_id IS NOT NULL;
CREATE INDEX camera_snapshots_prunable ON camera_snapshots(captured_at, id)
  WHERE pruned_at IS NULL AND pinned_at IS NULL;

CREATE TABLE incident_events (
  id TEXT PRIMARY KEY CHECK (id GLOB 'iev-*' AND length(id) BETWEEN 5 AND 64),
  incident_id TEXT NOT NULL REFERENCES incidents(id) ON DELETE RESTRICT,
  sequence INTEGER NOT NULL CHECK (sequence >= 1),
  kind TEXT NOT NULL CHECK (kind IN ('opened','eventLinked','reopened','eventAcknowledged',
    'eventResolved','evidenceCaptured','evidenceSkipped','evidencePruned','evidencePinned',
    'evidenceUnpinned','noteAdded','closed')),
  attention_event_id TEXT REFERENCES attention_events(id) ON DELETE RESTRICT,
  snapshot_id TEXT REFERENCES camera_snapshots(id) ON DELETE RESTRICT,
  detail_json TEXT NOT NULL CHECK (json_valid(detail_json)),
  operation_id TEXT,
  at TEXT NOT NULL,
  UNIQUE (incident_id, sequence),
  CHECK (kind NOT IN ('opened','eventLinked','reopened','eventAcknowledged','eventResolved')
         OR attention_event_id IS NOT NULL),
  CHECK (kind NOT IN ('evidenceCaptured','evidencePruned','evidencePinned','evidenceUnpinned')
         OR snapshot_id IS NOT NULL)
) STRICT;
CREATE TRIGGER incident_events_append_only_u BEFORE UPDATE ON incident_events
  BEGIN SELECT RAISE(ABORT, 'incident_events is append-only'); END;
CREATE TRIGGER incident_events_append_only_d BEFORE DELETE ON incident_events
  BEGIN SELECT RAISE(ABORT, 'incident_events is append-only'); END;

CREATE TABLE printer_cameras (
  printer_id TEXT PRIMARY KEY REFERENCES printers(id) ON DELETE CASCADE,
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  source_kind TEXT NOT NULL CHECK (source_kind IN ('hostWebcam','snapshotUrl')),
  webcam_name TEXT CHECK (webcam_name IS NULL OR length(webcam_name) BETWEEN 1 AND 128),
  webcam_service TEXT CHECK (webcam_service IS NULL OR length(webcam_service) BETWEEN 1 AND 64),
  web_port INTEGER CHECK (web_port IS NULL OR web_port BETWEEN 1 AND 65535),
  snapshot_url TEXT CHECK (snapshot_url IS NULL
                           OR (length(snapshot_url) BETWEEN 8 AND 2048 AND snapshot_url GLOB 'http://*')),
  updated_at TEXT NOT NULL,
  CHECK (source_kind <> 'hostWebcam' OR (webcam_name IS NOT NULL AND snapshot_url IS NULL)),
  CHECK (source_kind <> 'snapshotUrl' OR (snapshot_url IS NOT NULL AND webcam_name IS NULL
                                          AND webcam_service IS NULL AND web_port IS NULL))
) STRICT;

CREATE TABLE printer_alert_defaults (
  printer_id TEXT PRIMARY KEY REFERENCES printers(id) ON DELETE CASCADE,
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  offline_after_minutes INTEGER CHECK (offline_after_minutes IS NULL OR offline_after_minutes IN (1,5,15)),
  notifications TEXT NOT NULL CHECK (notifications IN ('follow','muted')),
  snapshot_on_incident INTEGER NOT NULL CHECK (snapshot_on_incident IN (0,1)),
  snapshot_on_completion INTEGER NOT NULL CHECK (snapshot_on_completion IN (0,1)),
  updated_at TEXT NOT NULL
) STRICT;

ALTER TABLE settings ADD COLUMN notify_fatal INTEGER NOT NULL DEFAULT 1 CHECK (notify_fatal IN (0,1));
ALTER TABLE settings ADD COLUMN notify_confirmation INTEGER NOT NULL DEFAULT 1 CHECK (notify_confirmation IN (0,1));
ALTER TABLE settings ADD COLUMN notify_completion INTEGER NOT NULL DEFAULT 1 CHECK (notify_completion IN (0,1));
ALTER TABLE settings ADD COLUMN notify_reconciliation INTEGER NOT NULL DEFAULT 0 CHECK (notify_reconciliation IN (0,1));
ALTER TABLE settings ADD COLUMN notify_connectivity INTEGER NOT NULL DEFAULT 0 CHECK (notify_connectivity IN (0,1));
ALTER TABLE settings ADD COLUMN notify_inventory INTEGER NOT NULL DEFAULT 0 CHECK (notify_inventory IN (0,1));
ALTER TABLE settings ADD COLUMN snapshot_retention_days INTEGER NOT NULL DEFAULT 30
  CHECK (snapshot_retention_days BETWEEN 1 AND 365);
ALTER TABLE settings ADD COLUMN snapshot_disk_cap_mb INTEGER NOT NULL DEFAULT 2048
  CHECK (snapshot_disk_cap_mb BETWEEN 100 AND 102400);

-- P7's operations ledger gains P8's 9 idempotent commands. SQLite can't
-- alter a CHECK, so the table is rebuilt: create, copy every row, drop
-- the old table, rename (as 0008 does for 0007's table).
CREATE TABLE operations_p8 (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('moveSpool','archivePrinter','spoolLifecycle','startSlice',
    'createExternalSliceRevision','stageSliceRevision','startStagedArtifact','pauseHostPrint',
    'resumeHostPrint','cancelHostPrint','abandonHostOperation','addToQueue','updateQueueEntry',
    'moveQueueEntry','removeQueueEntry','assignQueueEntry','stageJob','startJob','pauseJob',
    'resumeJob','cancelJob','releaseJob','retryJob','declareJobOutcome','settleJobMaterial',
    'correctJobMaterial',
    'markAttentionRead','acknowledgeAttention','resolveAttention','addIncidentNote',
    'setPrinterCamera','clearPrinterCamera','captureSnapshot','setSnapshotPinned',
    'setPrinterAlertDefaults')),
  request_digest TEXT NOT NULL,
  created_at TEXT NOT NULL
) STRICT;
INSERT INTO operations_p8(id, kind, request_digest, created_at)
  SELECT id, kind, request_digest, created_at FROM operations;
DROP TABLE operations;
ALTER TABLE operations_p8 RENAME TO operations;
