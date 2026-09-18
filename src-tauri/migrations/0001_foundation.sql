CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY CHECK (version > 0),
    name TEXT NOT NULL UNIQUE,
    checksum TEXT NOT NULL CHECK (
        length(checksum) = 64
        AND checksum NOT GLOB '*[^0-9a-f]*'
    ),
    applied_at TEXT NOT NULL
) STRICT;

CREATE TABLE settings (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    theme_mode TEXT NOT NULL CHECK (theme_mode <> ''),
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE printers (
    id TEXT PRIMARY KEY CHECK (length(CAST(id AS BLOB)) BETWEEN 1 AND 512),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    name TEXT NOT NULL,
    catalog_vendor TEXT NOT NULL,
    catalog_model TEXT NOT NULL,
    catalog_variant TEXT NOT NULL,
    catalog_model_id TEXT NOT NULL,
    catalog_printer_variant TEXT NOT NULL,
    notes TEXT NOT NULL,
    overrides_json TEXT NOT NULL CHECK (
        json_valid(overrides_json)
        AND json_type(overrides_json) = 'object'
    ),
    last_known_good_json TEXT CHECK (
        last_known_good_json IS NULL
        OR (json_valid(last_known_good_json) AND json_type(last_known_good_json) = 'object')
    ),
    connection_json TEXT CHECK (
        connection_json IS NULL
        OR (json_valid(connection_json) AND json_type(connection_json) = 'object')
    ),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE legacy_imports (
    source_name TEXT PRIMARY KEY CHECK (source_name IN ('settings.json', 'printers.json')),
    source_present INTEGER NOT NULL CHECK (source_present IN (0, 1)),
    source_sha256 TEXT CHECK (
        source_sha256 IS NULL
        OR (
            length(source_sha256) = 64
            AND source_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    source_schema_version INTEGER,
    imported_rows INTEGER NOT NULL CHECK (imported_rows >= 0),
    completed_at TEXT NOT NULL,
    CHECK (
        (source_present = 0 AND source_sha256 IS NULL)
        OR (source_present = 1 AND source_sha256 IS NOT NULL)
    )
) STRICT;

CREATE TABLE migration_warnings (
    id TEXT PRIMARY KEY CHECK (
        length(id) = 36
        AND id = lower(id)
        AND substr(id, 9, 1) = '-'
        AND substr(id, 14, 1) = '-'
        AND substr(id, 15, 1) = '4'
        AND substr(id, 19, 1) = '-'
        AND substr(id, 20, 1) IN ('8', '9', 'a', 'b')
        AND substr(id, 24, 1) = '-'
        AND substr(id, 1, 8) NOT GLOB '*[^0-9a-f]*'
        AND substr(id, 10, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(id, 15, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(id, 20, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(id, 25, 12) NOT GLOB '*[^0-9a-f]*'
    ),
    code TEXT NOT NULL CHECK (code <> ''),
    source_name TEXT CHECK (source_name IS NULL OR source_name <> ''),
    source_sha256 TEXT CHECK (
        source_sha256 IS NULL
        OR (
            length(source_sha256) = 64
            AND source_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    message TEXT NOT NULL,
    details_json TEXT NOT NULL CHECK (
        json_valid(details_json)
        AND json_type(details_json) = 'object'
    ),
    created_at TEXT NOT NULL
) STRICT;

CREATE UNIQUE INDEX migration_warnings_dedup
ON migration_warnings (
    code,
    COALESCE(source_name, ''),
    COALESCE(source_sha256, '')
);

CREATE TABLE pending_credential_cleanup (
    credential_ref TEXT PRIMARY KEY CHECK (
        length(CAST(credential_ref AS BLOB)) BETWEEN 1 AND 512
    ),
    printer_id TEXT CHECK (
        printer_id IS NULL
        OR length(CAST(printer_id AS BLOB)) BETWEEN 1 AND 512
    ),
    reason TEXT NOT NULL CHECK (
        reason IN ('provisional', 'replaced', 'cleared', 'printer_deleted', 'import_orphan')
    ),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    last_error_code TEXT,
    created_at TEXT NOT NULL,
    last_attempt_at TEXT
) STRICT;
