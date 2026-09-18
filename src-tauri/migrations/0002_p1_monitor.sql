ALTER TABLE settings
ADD COLUMN monitor_section TEXT NOT NULL DEFAULT 'printerModel'
CHECK (monitor_section IN ('location', 'printerModel', 'operationalState', 'none'));

ALTER TABLE settings
ADD COLUMN monitor_density TEXT NOT NULL DEFAULT 'comfortable'
CHECK (monitor_density IN ('comfortable', 'compact'));

CREATE TABLE printer_status_snapshots (
    printer_id TEXT PRIMARY KEY
        REFERENCES printers(id) ON DELETE CASCADE,
    telemetry_json TEXT NOT NULL
        CHECK (json_valid(telemetry_json) AND json_type(telemetry_json) = 'object'),
    last_observed_at TEXT NOT NULL,
    persisted_at TEXT NOT NULL
) STRICT;
