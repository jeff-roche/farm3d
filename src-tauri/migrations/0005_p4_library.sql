CREATE TABLE library_projects (
  id TEXT PRIMARY KEY CHECK (id GLOB 'prj-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL CHECK (revision >= 1),
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 128 AND name = trim(name)),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX library_projects_name ON library_projects(lower(name));

CREATE TABLE content_blobs (
  sha256 TEXT PRIMARY KEY CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
  size_bytes INTEGER NOT NULL CHECK (size_bytes BETWEEN 0 AND 4294967296),
  created_at TEXT NOT NULL
) STRICT;

CREATE TABLE library_models (
  id TEXT PRIMARY KEY CHECK (id GLOB 'mdl-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL CHECK (revision >= 1),
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 255 AND name = trim(name)),
  format TEXT NOT NULL CHECK (format IN ('stl', '3mf', 'gcode')),
  storage_mode TEXT NOT NULL CHECK (storage_mode IN ('managed', 'linked')),
  linked_path TEXT,
  link_state TEXT CHECK (link_state IS NULL OR link_state IN
    ('ok', 'missing', 'unreadable', 'notAFile', 'invalidContent', 'changing')),
  link_checked_at TEXT,
  link_observed_size INTEGER,
  link_observed_mtime_ns INTEGER,
  link_observed_file_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK (
    (storage_mode = 'managed' AND linked_path IS NULL AND link_state IS NULL)
    OR (storage_mode = 'linked' AND linked_path IS NOT NULL AND length(linked_path) > 0
        AND link_state IS NOT NULL)
  )
) STRICT;
CREATE INDEX library_models_linked ON library_models(storage_mode) WHERE storage_mode = 'linked';

CREATE TABLE project_models (
  project_id TEXT NOT NULL REFERENCES library_projects(id) ON DELETE CASCADE,
  model_id TEXT NOT NULL REFERENCES library_models(id) ON DELETE CASCADE,
  added_at TEXT NOT NULL,
  PRIMARY KEY (project_id, model_id)
) WITHOUT ROWID, STRICT;
CREATE INDEX project_models_model ON project_models(model_id);

CREATE TABLE model_source_revisions (
  id TEXT PRIMARY KEY CHECK (id GLOB 'msr-*' AND length(id) BETWEEN 5 AND 64),
  model_id TEXT NOT NULL REFERENCES library_models(id) ON DELETE CASCADE,
  sequence INTEGER NOT NULL CHECK (sequence >= 1),
  content_sha256 TEXT NOT NULL REFERENCES content_blobs(sha256),
  size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
  format TEXT NOT NULL CHECK (format IN ('stl', '3mf', 'gcode')),
  origin TEXT NOT NULL CHECK (origin IN ('import', 'linkedChange', 'relocate', 'addedRevision')),
  source_file_name TEXT NOT NULL CHECK (length(source_file_name) BETWEEN 1 AND 1024),
  source_path TEXT NOT NULL,
  source_mtime TEXT,
  captured_at TEXT NOT NULL,
  inspector_version INTEGER NOT NULL CHECK (inspector_version >= 1),
  inspection_json TEXT NOT NULL CHECK (json_valid(inspection_json)
                                       AND json_type(inspection_json) = 'object'),
  UNIQUE (model_id, sequence)
) STRICT;
CREATE INDEX model_source_revisions_content ON model_source_revisions(content_sha256);

CREATE TRIGGER model_source_revisions_immutable
BEFORE UPDATE ON model_source_revisions
BEGIN
  SELECT RAISE(ABORT, 'model source revisions are immutable');
END;

CREATE TABLE model_revision_thumbnails (
  revision_id TEXT PRIMARY KEY REFERENCES model_source_revisions(id) ON DELETE CASCADE,
  source TEXT NOT NULL CHECK (source IN ('embedded')),
  origin_part TEXT NOT NULL,
  media_type TEXT NOT NULL CHECK (media_type = 'image/png'),
  width INTEGER NOT NULL CHECK (width BETWEEN 1 AND 1024),
  height INTEGER NOT NULL CHECK (height BETWEEN 1 AND 1024),
  content_sha256 TEXT NOT NULL REFERENCES content_blobs(sha256)
) STRICT;
CREATE INDEX model_revision_thumbnails_content ON model_revision_thumbnails(content_sha256);

CREATE TABLE pending_blob_cleanup (
  sha256 TEXT PRIMARY KEY CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
  attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
  last_error_code TEXT,
  created_at TEXT NOT NULL,
  last_attempt_at TEXT
) STRICT;
