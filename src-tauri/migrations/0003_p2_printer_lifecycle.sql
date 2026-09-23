ALTER TABLE printers ADD COLUMN location TEXT
  CHECK (location IS NULL OR (length(location) BETWEEN 1 AND 128 AND location = trim(location)));
ALTER TABLE printers ADD COLUMN start_safety TEXT NOT NULL DEFAULT 'confirmBedClear'
  CHECK (start_safety IN ('confirmBedClear', 'unattended'));
ALTER TABLE printers ADD COLUMN archived_at TEXT;
ALTER TABLE printers ADD COLUMN host_identity TEXT;
CREATE UNIQUE INDEX printers_active_host_identity
  ON printers(host_identity)
  WHERE archived_at IS NULL AND host_identity IS NOT NULL;
