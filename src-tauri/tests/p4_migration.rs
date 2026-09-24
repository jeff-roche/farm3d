//! Migration 0005 (P4 library persistence and Projects). Covers: a fresh
//! database's ledger row, the v4->v5 upgrade (Printers and Spools
//! unchanged), the Project/Model `CHECK` constraints, revision
//! immutability and the delete cascade, Project-membership set semantics,
//! the `content_blobs` delete restrict, and the migration's crash-boundary
//! behaviour. See "Migration `0005_p4_library.sql`" in the P4 design spec
//! and Step 1 of the task-2 brief.

use sha2::{Digest, Sha256};

use farm3d_lib::persistence::test_support::{apply_through, apply_through_failing_before_commit};
use farm3d_lib::persistence::{
    MetadataRootLease, Storage, StorageError, StoragePaths, CURRENT_SCHEMA_VERSION,
};

fn open_storage() -> (tempfile::TempDir, StoragePaths, MetadataRootLease, Storage) {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let storage = Storage::open(paths.clone(), &lease).expect("storage");
    (temp, paths, lease, storage)
}

/// Opens a second, raw connection to an already-migrated (v5) database with
/// foreign keys enabled explicitly. `rusqlite::Connection::open` defaults
/// to foreign keys off, unlike every `Storage`-owned connection
/// (`configure_connection`), so the FK-cascade and FK-restrict tests below
/// (5, 6, 7) must turn it on themselves. Used instead of `Storage::write`
/// for the raw-SQL tests in this file because `StorageError::from` collapses
/// every `CHECK`/trigger/FK failure to `StorageError::Database`, discarding
/// the SQLite message some of these tests need to assert on.
fn raw_connection(paths: &StoragePaths) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(paths.database()).expect("raw connection");
    connection
        .execute_batch("PRAGMA foreign_keys = ON")
        .expect("foreign keys on");
    connection
}

/// Builds a v4-only database (migrations 1-4, via `apply_through`) at
/// `paths.database()` and returns the raw connection so the caller can seed
/// v4-shaped Printer/Spool rows before ever letting `Storage::open` see it.
fn v4_database(paths: &StoragePaths) -> rusqlite::Connection {
    let mut connection = rusqlite::Connection::open(paths.database()).expect("v4 database");
    apply_through(&mut connection, CURRENT_SCHEMA_VERSION - 1).expect("v4 migrations");
    connection
}

fn insert_v4_printer(connection: &rusqlite::Connection, id: &str, created_at: &str) {
    connection
        .execute(
            "INSERT INTO printers(
                id, revision, name, catalog_vendor, catalog_model, catalog_variant,
                catalog_model_id, catalog_printer_variant, notes, overrides_json,
                created_at, updated_at
             ) VALUES (?1, 1, 'Printer', '', '', '', '', '', '', '{}', ?2, ?2)",
            rusqlite::params![id, created_at],
        )
        .expect("insert v4 printer");
}

fn insert_v4_spool(connection: &rusqlite::Connection, id: &str, spool_number: i64) {
    connection
        .execute(
            "INSERT INTO spools(
                id, revision, spool_number, manufacturer, material_family, material_other,
                color_name, diameter, nominal_mg, current_mg, confidence, lifecycle,
                archived_from, slot_id, storage_label, created_at, updated_at
             ) VALUES (?1, 1, ?2, 'Polymaker', 'PLA', NULL, 'Black', '1.75', 1000000, 1000000,
                       'measured', 'active', NULL, NULL, NULL,
                       '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![id, spool_number],
        )
        .expect("insert v4 spool");
}

fn insert_content_blob(connection: &rusqlite::Connection, sha256: &str, size_bytes: i64) {
    connection
        .execute(
            "INSERT INTO content_blobs(sha256, size_bytes, created_at)
             VALUES (?1, ?2, '2026-01-01T00:00:00.000Z')",
            rusqlite::params![sha256, size_bytes],
        )
        .expect("insert content blob");
}

fn insert_model(connection: &rusqlite::Connection, id: &str) {
    connection
        .execute(
            "INSERT INTO library_models(
                id, revision, name, format, storage_mode, created_at, updated_at
             ) VALUES (?1, 1, 'Model', 'stl', 'managed',
                       '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
            [id],
        )
        .expect("insert model");
}

fn insert_project(connection: &rusqlite::Connection, id: &str, name: &str) {
    connection
        .execute(
            "INSERT INTO library_projects(id, revision, name, created_at, updated_at)
             VALUES (?1, 1, ?2, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![id, name],
        )
        .expect("insert project");
}

fn insert_membership(connection: &rusqlite::Connection, project_id: &str, model_id: &str) {
    connection
        .execute(
            "INSERT INTO project_models(project_id, model_id, added_at)
             VALUES (?1, ?2, '2026-01-01T00:00:00.000Z')",
            rusqlite::params![project_id, model_id],
        )
        .expect("insert membership");
}

fn insert_revision(
    connection: &rusqlite::Connection,
    id: &str,
    model_id: &str,
    sequence: i64,
    sha256: &str,
) {
    connection
        .execute(
            "INSERT INTO model_source_revisions(
                id, model_id, sequence, content_sha256, size_bytes, format, origin,
                source_file_name, source_path, captured_at, inspector_version, inspection_json
             ) VALUES (?1, ?2, ?3, ?4, 100, 'stl', 'import', 'model.stl', '/tmp/model.stl',
                       '2026-01-01T00:00:00.000Z', 1, '{}')",
            rusqlite::params![id, model_id, sequence, sha256],
        )
        .expect("insert revision");
}

fn insert_thumbnail(connection: &rusqlite::Connection, revision_id: &str, sha256: &str) {
    connection
        .execute(
            "INSERT INTO model_revision_thumbnails(
                revision_id, source, origin_part, media_type, width, height, content_sha256
             ) VALUES (?1, 'embedded', 'Metadata/thumbnail.png', 'image/png', 100, 100, ?2)",
            rusqlite::params![revision_id, sha256],
        )
        .expect("insert thumbnail");
}

/// 1. A fresh database reaches `CURRENT_SCHEMA_VERSION`, with a ledger row
///    for `0005_p4_library` whose checksum matches the migration SQL on
///    disk.
#[test]
fn fresh_database_records_the_v5_ledger_row_with_a_matching_checksum() {
    let (_temp, _paths, _lease, storage) = open_storage();

    let (version, name, checksum) = storage
        .read(|connection| {
            let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            let (name, checksum): (String, String) = connection.query_row(
                "SELECT name, checksum FROM schema_migrations WHERE version = 5",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok((version, name, checksum))
        })
        .expect("schema state");

    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    assert_eq!(name, "0005_p4_library");
    let expected_checksum = format!(
        "{:x}",
        Sha256::digest(include_str!("../migrations/0005_p4_library.sql").as_bytes())
    );
    assert_eq!(checksum, expected_checksum);
}

/// 2. Upgrading a v4 database with one Printer and one Spool reaches v5
///    without changing either row.
#[test]
fn upgrading_v4_to_v5_leaves_printers_and_spools_unchanged() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    {
        let connection = v4_database(&paths);
        insert_v4_printer(&connection, "prn-a", "2026-01-01T00:00:00.000Z");
        insert_v4_spool(&connection, "spl-a", 1);
    }

    let storage = Storage::open(paths, &lease).expect("v5 storage");

    let (printer_row, spool_row) = storage
        .read(|connection| {
            let printer: (String, i64, String) = connection.query_row(
                "SELECT id, revision, name FROM printers WHERE id = 'prn-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let spool: (String, i64, i64) = connection.query_row(
                "SELECT id, revision, spool_number FROM spools WHERE id = 'spl-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            Ok((printer, spool))
        })
        .expect("post-upgrade state");

    assert_eq!(printer_row, ("prn-a".to_string(), 1, "Printer".to_string()));
    assert_eq!(spool_row, ("spl-a".to_string(), 1, 1));
}

/// 3. `library_projects`' `CHECK (name = trim(name))` rejects an untrimmed
///    name, and `library_projects_name` (the case-insensitive unique index)
///    rejects a duplicate.
#[test]
fn project_name_check_and_unique_index_reject_invalid_and_duplicate_names() {
    let (_temp, paths, _lease, storage) = open_storage();
    drop(storage);
    let connection = raw_connection(&paths);

    let leading_space = connection.execute(
        "INSERT INTO library_projects(id, revision, name, created_at, updated_at)
         VALUES ('prj-a', 1, ' x', '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
        [],
    );
    assert!(
        leading_space.is_err(),
        "an untrimmed name must fail the CHECK"
    );

    insert_project(&connection, "prj-b", "Brackets");
    let duplicate = connection.execute(
        "INSERT INTO library_projects(id, revision, name, created_at, updated_at)
         VALUES ('prj-c', 1, 'brackets', '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
        [],
    );
    assert!(
        duplicate.is_err(),
        "a case-insensitive duplicate name must fail the unique index"
    );
}

/// 4. `library_models`' storage-mode `CHECK` rejects a managed Model with a
///    `linked_path`, and a linked Model with a NULL `link_state`.
#[test]
fn model_storage_mode_check_rejects_mismatched_link_fields() {
    let (_temp, paths, _lease, storage) = open_storage();
    drop(storage);
    let connection = raw_connection(&paths);

    let managed_with_path = connection.execute(
        "INSERT INTO library_models(
            id, revision, name, format, storage_mode, linked_path, created_at, updated_at
         ) VALUES ('mdl-a', 1, 'Model', 'stl', 'managed', '/tmp/x.stl',
                   '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
        [],
    );
    assert!(
        managed_with_path.is_err(),
        "a managed Model with a linked_path must fail the CHECK"
    );

    let linked_without_state = connection.execute(
        "INSERT INTO library_models(
            id, revision, name, format, storage_mode, linked_path, link_state,
            created_at, updated_at
         ) VALUES ('mdl-b', 1, 'Model', 'stl', 'linked', '/tmp/x.stl', NULL,
                   '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
        [],
    );
    assert!(
        linked_without_state.is_err(),
        "a linked Model with a NULL link_state must fail the CHECK"
    );
}

/// 5. `model_source_revisions_immutable` blocks any `UPDATE`, and deleting
///    a Model cascades to its revisions, their thumbnails, and its
///    Project memberships.
#[test]
fn revisions_are_immutable_and_deleting_a_model_cascades_everything() {
    let (_temp, paths, _lease, storage) = open_storage();
    drop(storage);
    let connection = raw_connection(&paths);

    let source_hash = "a".repeat(64);
    let thumbnail_hash = "b".repeat(64);
    insert_content_blob(&connection, &source_hash, 100);
    insert_model(&connection, "mdl-a");
    insert_revision(&connection, "msr-a", "mdl-a", 1, &source_hash);
    insert_content_blob(&connection, &thumbnail_hash, 10);
    insert_thumbnail(&connection, "msr-a", &thumbnail_hash);
    insert_project(&connection, "prj-a", "Project");
    insert_membership(&connection, "prj-a", "mdl-a");

    let update_error = connection
        .execute(
            "UPDATE model_source_revisions SET source_file_name = 'x' WHERE id = 'msr-a'",
            [],
        )
        .expect_err("revisions must be immutable");
    let message = update_error.to_string();
    assert!(
        message.contains("model source revisions are immutable"),
        "unexpected error message: {message}"
    );

    connection
        .execute("DELETE FROM library_models WHERE id = 'mdl-a'", [])
        .expect("delete model");

    let revisions: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM model_source_revisions WHERE model_id = 'mdl-a'",
            [],
            |row| row.get(0),
        )
        .expect("revision count");
    let thumbnails: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM model_revision_thumbnails WHERE revision_id = 'msr-a'",
            [],
            |row| row.get(0),
        )
        .expect("thumbnail count");
    let memberships: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM project_models WHERE model_id = 'mdl-a'",
            [],
            |row| row.get(0),
        )
        .expect("membership count");
    assert_eq!(revisions, 0, "revisions must cascade-delete");
    assert_eq!(thumbnails, 0, "thumbnails must cascade-delete");
    assert_eq!(memberships, 0, "memberships must cascade-delete");
}

/// 6. One Model in two Projects gives two `project_models` rows; a
///    duplicate `(project_id, model_id)` fails the primary key; deleting a
///    Project removes only its own membership row, leaving the Model and
///    its other membership intact.
#[test]
fn membership_is_many_to_many_and_deleting_a_project_removes_only_its_own_row() {
    let (_temp, paths, _lease, storage) = open_storage();
    drop(storage);
    let connection = raw_connection(&paths);

    insert_model(&connection, "mdl-a");
    insert_project(&connection, "prj-a", "A");
    insert_project(&connection, "prj-b", "B");
    insert_membership(&connection, "prj-a", "mdl-a");
    insert_membership(&connection, "prj-b", "mdl-a");

    let membership_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM project_models WHERE model_id = 'mdl-a'",
            [],
            |row| row.get(0),
        )
        .expect("membership count");
    assert_eq!(membership_count, 2);

    let duplicate = connection.execute(
        "INSERT INTO project_models(project_id, model_id, added_at)
         VALUES ('prj-a', 'mdl-a', '2026-01-01T00:00:00.000Z')",
        [],
    );
    assert!(
        duplicate.is_err(),
        "a duplicate (project_id, model_id) must fail the primary key"
    );

    connection
        .execute("DELETE FROM library_projects WHERE id = 'prj-a'", [])
        .expect("delete project a");

    let remaining_projects: Vec<String> = {
        let mut statement = connection
            .prepare("SELECT project_id FROM project_models WHERE model_id = 'mdl-a'")
            .expect("prepare");
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect")
    };
    assert_eq!(remaining_projects, vec!["prj-b".to_string()]);

    let model_still_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM library_models WHERE id = 'mdl-a')",
            [],
            |row| row.get(0),
        )
        .expect("model existence check");
    assert!(model_still_exists, "the Model itself must remain");
}

/// 7. `content_blobs` has no `ON DELETE` action from its referrers, so the
///    default `RESTRICT` blocks deleting a blob a revision still
///    references.
#[test]
fn deleting_a_referenced_content_blob_is_restricted() {
    let (_temp, paths, _lease, storage) = open_storage();
    drop(storage);
    let connection = raw_connection(&paths);

    let hash = "c".repeat(64);
    insert_content_blob(&connection, &hash, 100);
    insert_model(&connection, "mdl-a");
    insert_revision(&connection, "msr-a", "mdl-a", 1, &hash);

    let result = connection.execute("DELETE FROM content_blobs WHERE sha256 = ?1", [&hash]);
    assert!(
        result.is_err(),
        "a referenced content blob must not be deletable"
    );
}

/// 8. Crash boundary: a failure after the v5 migration's SQL/ledger row but
///    before commit leaves the database exactly as it was at v4.
#[test]
fn a_crash_before_commit_leaves_the_database_unchanged_at_v4() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let _lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let mut connection = v4_database(&paths);
    insert_v4_printer(&connection, "prn-a", "2026-01-01T00:00:00.000Z");

    let error = apply_through_failing_before_commit(&mut connection, CURRENT_SCHEMA_VERSION)
        .expect_err("the injected failure must surface");
    assert!(matches!(error, StorageError::MigrationFailed));

    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("user_version");
    assert_eq!(
        user_version,
        CURRENT_SCHEMA_VERSION - 1,
        "the schema version must roll back to v4"
    );

    let v5_ledger_rows: i64 = connection
        .query_row(
            "SELECT count(*) FROM schema_migrations WHERE version = ?1",
            [CURRENT_SCHEMA_VERSION],
            |row| row.get(0),
        )
        .expect("ledger rows");
    assert_eq!(
        v5_ledger_rows, 0,
        "no v5 ledger row must survive the rollback"
    );

    let has_library_projects: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'library_projects')",
            [],
            |row| row.get(0),
        )
        .expect("table check");
    assert!(!has_library_projects, "the v5 tables must roll back too");
}
