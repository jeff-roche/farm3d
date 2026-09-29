-- P9 D11: Job history paging, keyset on the history time, then id.
CREATE INDEX jobs_history ON jobs(COALESCE(ended_at, created_at) DESC, id DESC)
  WHERE state IN ('completed','failed','cancelled','outcomeUnknown');
CREATE INDEX jobs_history_printer ON jobs(printer_id, COALESCE(ended_at, created_at) DESC, id DESC)
  WHERE state IN ('completed','failed','cancelled','outcomeUnknown');

-- P9 D5/D15: camera_snapshots gains the prune reasons 'reset' and
-- 'notInBackup', and a pinned row may be pruned for either. SQLite can't
-- alter a CHECK, and incident_events holds an ON DELETE RESTRICT foreign
-- key to camera_snapshots, so dropping camera_snapshots alone fails once
-- any Incident has evidence. Both tables are rebuilt: the new
-- incident_events references the new camera_snapshots by its temporary
-- name, and the rename rewrites that reference.
CREATE TABLE camera_snapshots_p9 (
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
  prune_reason TEXT CHECK (prune_reason IN ('age','diskCap','missingFile','reset','notInBackup')),
  CHECK ((pruned_at IS NULL) = (prune_reason IS NULL)),
  CHECK (pinned_at IS NULL OR pruned_at IS NULL
         OR prune_reason IN ('missingFile','reset','notInBackup')),
  CHECK (trigger <> 'incident' OR incident_id IS NOT NULL),
  CHECK (trigger <> 'completion' OR (job_id IS NOT NULL AND incident_id IS NULL)),
  CHECK ((trigger = 'manual') = (operation_id IS NOT NULL))
) STRICT;
INSERT INTO camera_snapshots_p9(id, revision, printer_id, incident_id, job_id, trigger, operation_id,
    captured_at, content_type, byte_len, sha256, rel_path, pinned_at, pruned_at, prune_reason)
  SELECT id, revision, printer_id, incident_id, job_id, trigger, operation_id,
    captured_at, content_type, byte_len, sha256, rel_path, pinned_at, pruned_at, prune_reason
  FROM camera_snapshots;

CREATE TABLE incident_events_p9 (
  id TEXT PRIMARY KEY CHECK (id GLOB 'iev-*' AND length(id) BETWEEN 5 AND 64),
  incident_id TEXT NOT NULL REFERENCES incidents(id) ON DELETE RESTRICT,
  sequence INTEGER NOT NULL CHECK (sequence >= 1),
  kind TEXT NOT NULL CHECK (kind IN ('opened','eventLinked','reopened','eventAcknowledged',
    'eventResolved','evidenceCaptured','evidenceSkipped','evidencePruned','evidencePinned',
    'evidenceUnpinned','noteAdded','closed')),
  attention_event_id TEXT REFERENCES attention_events(id) ON DELETE RESTRICT,
  snapshot_id TEXT REFERENCES camera_snapshots_p9(id) ON DELETE RESTRICT,
  detail_json TEXT NOT NULL CHECK (json_valid(detail_json)),
  operation_id TEXT,
  at TEXT NOT NULL,
  UNIQUE (incident_id, sequence),
  CHECK (kind NOT IN ('opened','eventLinked','reopened','eventAcknowledged','eventResolved')
         OR attention_event_id IS NOT NULL),
  CHECK (kind NOT IN ('evidenceCaptured','evidencePruned','evidencePinned','evidenceUnpinned')
         OR snapshot_id IS NOT NULL)
) STRICT;
INSERT INTO incident_events_p9(id, incident_id, sequence, kind, attention_event_id, snapshot_id,
    detail_json, operation_id, at)
  SELECT id, incident_id, sequence, kind, attention_event_id, snapshot_id,
    detail_json, operation_id, at
  FROM incident_events;

-- DROP TABLE's implicit delete fires no trigger, so the append-only
-- triggers don't block it; they are recreated below.
DROP TABLE incident_events;
DROP TABLE camera_snapshots;
ALTER TABLE camera_snapshots_p9 RENAME TO camera_snapshots;
ALTER TABLE incident_events_p9 RENAME TO incident_events;

CREATE INDEX camera_snapshots_printer ON camera_snapshots(printer_id, captured_at);
CREATE INDEX camera_snapshots_incident ON camera_snapshots(incident_id) WHERE incident_id IS NOT NULL;
CREATE INDEX camera_snapshots_job ON camera_snapshots(job_id) WHERE job_id IS NOT NULL;
CREATE INDEX camera_snapshots_prunable ON camera_snapshots(captured_at, id)
  WHERE pruned_at IS NULL AND pinned_at IS NULL;
CREATE TRIGGER incident_events_append_only_u BEFORE UPDATE ON incident_events
  BEGIN SELECT RAISE(ABORT, 'incident_events is append-only'); END;
CREATE TRIGGER incident_events_append_only_d BEFORE DELETE ON incident_events
  BEGIN SELECT RAISE(ABORT, 'incident_events is append-only'); END;

-- P9 D15: pending_credential_cleanup gains the reason 'reset'. Nothing
-- holds a foreign key to it.
CREATE TABLE pending_credential_cleanup_p9 (
    credential_ref TEXT PRIMARY KEY CHECK (
        length(CAST(credential_ref AS BLOB)) BETWEEN 1 AND 512
    ),
    printer_id TEXT CHECK (
        printer_id IS NULL
        OR length(CAST(printer_id AS BLOB)) BETWEEN 1 AND 512
    ),
    reason TEXT NOT NULL CHECK (
        reason IN ('provisional', 'replaced', 'cleared', 'printer_deleted', 'import_orphan', 'reset')
    ),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    last_error_code TEXT,
    created_at TEXT NOT NULL,
    last_attempt_at TEXT
) STRICT;
INSERT INTO pending_credential_cleanup_p9(credential_ref, printer_id, reason, attempt_count,
    last_error_code, created_at, last_attempt_at)
  SELECT credential_ref, printer_id, reason, attempt_count,
    last_error_code, created_at, last_attempt_at
  FROM pending_credential_cleanup;
DROP TABLE pending_credential_cleanup;
ALTER TABLE pending_credential_cleanup_p9 RENAME TO pending_credential_cleanup;

-- P9 D18: the operations ledger gains the two database-claimed reset
-- tiers (as 0009 rebuilt 0008's table).
CREATE TABLE operations_p9 (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('moveSpool','archivePrinter','spoolLifecycle','startSlice',
    'createExternalSliceRevision','stageSliceRevision','startStagedArtifact','pauseHostPrint',
    'resumeHostPrint','cancelHostPrint','abandonHostOperation','addToQueue','updateQueueEntry',
    'moveQueueEntry','removeQueueEntry','assignQueueEntry','stageJob','startJob','pauseJob',
    'resumeJob','cancelJob','releaseJob','retryJob','declareJobOutcome','settleJobMaterial',
    'correctJobMaterial',
    'markAttentionRead','acknowledgeAttention','resolveAttention','addIncidentNote',
    'setPrinterCamera','clearPrinterCamera','captureSnapshot','setSnapshotPinned',
    'setPrinterAlertDefaults',
    'resetSettings','resetCameraMedia')),
  request_digest TEXT NOT NULL,
  created_at TEXT NOT NULL
) STRICT;
INSERT INTO operations_p9(id, kind, request_digest, created_at)
  SELECT id, kind, request_digest, created_at FROM operations;
DROP TABLE operations;
ALTER TABLE operations_p9 RENAME TO operations;
