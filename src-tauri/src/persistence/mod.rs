mod database;
mod error;
mod legacy;
mod migrations;
pub mod snapshot;
pub mod validation;

pub(crate) use database::{create_contained_directory, normalize_absolute};
pub use database::{
    take_transaction_failure, FailurePoint, MetadataRootLease, Storage, StoragePaths,
};
pub use error::{RepositoryError, StorageError};
pub use legacy::{migrate_legacy, LegacyMigrationOutcome};
pub use migrations::CURRENT_SCHEMA_VERSION;
pub use snapshot::{SnapshotKind, ValidationSummary};

/// Migration internals integration tests need to build a fixture database
/// pinned at an older schema version (e.g. a v2 database, to exercise the
/// v2→v3 upgrade) without duplicating the migration SQL.
pub mod test_support {
    pub use super::migrations::{apply_through, apply_through_failing_before_commit};
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Child, Command, Stdio};
    use std::sync::{mpsc, Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::*;

    fn open_storage() -> (TempDir, StoragePaths, MetadataRootLease, Arc<Storage>) {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        let storage = Arc::new(Storage::open(paths.clone(), &lease).expect("storage"));
        (temp, paths, lease, storage)
    }

    #[test]
    fn storage_paths_allow_equal_platform_roots_without_owned_path_collisions() {
        let temp = tempfile::tempdir().expect("temporary root");

        let paths = StoragePaths::new(temp.path(), temp.path()).expect("storage paths");

        assert_eq!(
            paths.content_root(),
            temp.path()
                .canonicalize()
                .expect("canonical root")
                .join("farm3d-content/v1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn storage_paths_reject_symlink_alias_collisions() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary root");
        let metadata = temp.path().join("metadata");
        fs::create_dir_all(metadata.join("snapshots/farm3d-content")).expect("snapshot tree");
        symlink(
            metadata.join("snapshots/farm3d-content"),
            temp.path().join("aliased-data"),
        )
        .expect("symlink");

        let error = StoragePaths::new(&metadata, temp.path().join("aliased-data"))
            .expect_err("collision must fail");

        assert!(matches!(error, StorageError::PathCollision));
    }

    #[test]
    fn storage_paths_reject_a_content_tree_nested_under_a_metadata_tree() {
        let temp = tempfile::tempdir().expect("temporary root");
        let metadata = temp.path().join("metadata");

        let error = StoragePaths::new(&metadata, metadata.join("snapshots"))
            .expect_err("nested content tree must fail");

        assert!(matches!(error, StorageError::PathCollision));
    }

    #[cfg(unix)]
    #[test]
    fn storage_paths_reject_a_database_symlink_into_an_owned_tree() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary root");
        let metadata = temp.path().join("metadata");
        let data = temp.path().join("data");
        let content = data.join("farm3d-content/v1");
        fs::create_dir_all(&metadata).expect("metadata root");
        fs::create_dir_all(&content).expect("content root");
        let aliased_database = content.join("database.sqlite3");
        fs::write(&aliased_database, []).expect("database target");
        symlink(&aliased_database, metadata.join("farm3d.sqlite3")).expect("database symlink");

        let error =
            StoragePaths::new(metadata, data).expect_err("database/tree collision must fail");

        assert!(matches!(error, StorageError::PathCollision));
    }

    #[test]
    fn storage_paths_reject_relative_roots() {
        let error = StoragePaths::new("relative-metadata", "relative-data")
            .expect_err("relative roots must fail");

        assert!(matches!(error, StorageError::PathCollision));
    }

    #[test]
    fn storage_paths_normalize_dot_and_parent_components_before_creating_directories() {
        let temp = tempfile::tempdir().expect("temporary root");
        let metadata = temp.path().join("metadata");
        let paths = StoragePaths::new(
            metadata.join("unused/.."),
            temp.path().join("data/./nested/.."),
        )
        .expect("normalized storage paths");

        assert_eq!(
            paths.metadata_root(),
            metadata.canonicalize().expect("canonical metadata")
        );
        assert!(!metadata.join("unused").exists());
        assert!(!temp.path().join("data/nested").exists());
    }

    #[test]
    fn storage_paths_reject_content_nested_directly_under_legacy() {
        let temp = tempfile::tempdir().expect("temporary root");
        let metadata = temp.path().join("metadata");

        let error = StoragePaths::new(&metadata, metadata.join("legacy"))
            .expect_err("legacy/content collision must fail");

        assert!(matches!(error, StorageError::PathCollision));
    }

    #[test]
    fn metadata_lease_rejects_a_second_owner_until_the_first_is_dropped() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let first = MetadataRootLease::acquire(&paths).expect("first lease");

        let error = MetadataRootLease::acquire(&paths).expect_err("second lease must fail");
        assert!(matches!(error, StorageError::PersistenceUnavailable));

        drop(first);
    }

    #[test]
    fn storage_rejects_a_lease_for_a_different_metadata_root() {
        let temp = tempfile::tempdir().expect("temporary root");
        let first_paths = StoragePaths::new(
            temp.path().join("first-metadata"),
            temp.path().join("first-data"),
        )
        .expect("first storage paths");
        let second_paths = StoragePaths::new(
            temp.path().join("second-metadata"),
            temp.path().join("second-data"),
        )
        .expect("second storage paths");
        let lease = MetadataRootLease::acquire(&first_paths).expect("first metadata lease");

        let error = Storage::open(second_paths, &lease)
            .err()
            .expect("mismatched lease must fail");

        assert!(matches!(error, StorageError::PersistenceUnavailable));
    }

    #[test]
    fn open_creates_the_foundation_schema_without_initializing_settings() {
        let (_temp, _paths, _lease, storage) = open_storage();

        let state = storage
            .read(|connection| {
                let version: i64 =
                    connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
                let settings =
                    connection.query_row("SELECT count(*) FROM settings", [], |row| row.get(0))?;
                Ok((version, settings))
            })
            .expect("schema state");

        assert_eq!(state, (CURRENT_SCHEMA_VERSION, 0_i64));
    }

    #[test]
    fn opening_a_v1_database_applies_monitor_preferences_without_changing_settings() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        let connection = rusqlite::Connection::open(paths.database()).expect("v1 database");
        let foundation = include_str!("../../migrations/0001_foundation.sql");
        connection
            .execute_batch(foundation)
            .expect("foundation schema");
        connection
            .execute(
                "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES (1, '0001_foundation', ?1, '2026-09-18T00:00:00Z')",
                [format!("{:x}", Sha256::digest(foundation.as_bytes()))],
            )
            .expect("foundation ledger entry");
        connection
            .execute(
                "INSERT INTO settings(singleton_id, revision, theme_mode, updated_at) VALUES (1, 7, 'farm3d-dark', '2026-09-18T00:00:00Z')",
                [],
            )
            .expect("v1 settings");
        connection
            .execute_batch("PRAGMA user_version = 1")
            .expect("v1 version");
        drop(connection);

        let storage = Storage::open(paths, &lease).expect("migrated storage");
        let state = storage
            .read(|connection| {
                let version: i64 =
                    connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
                let settings = connection.query_row(
                    "SELECT revision, theme_mode, monitor_section, monitor_density FROM settings WHERE singleton_id = 1",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)),
                )?;
                let cache_exists = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'printer_status_snapshots')",
                    [],
                    |row| row.get::<_, bool>(0),
                )?;
                Ok((version, settings, cache_exists))
            })
            .expect("migrated state");

        assert_eq!(state.0, CURRENT_SCHEMA_VERSION);
        assert_eq!(
            state.1,
            (
                7_i64,
                "farm3d-dark".to_string(),
                "printerModel".to_string(),
                "comfortable".to_string()
            )
        );
        assert!(state.2);
    }

    #[test]
    fn reopening_storage_keeps_the_migration_ledger_idempotent() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        drop(Storage::open(paths.clone(), &lease).expect("first open"));

        let reopened = Storage::open(paths, &lease).expect("second open");
        let migration_count: i64 = reopened
            .read(|connection| {
                connection.query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
            })
            .expect("migration count");

        assert_eq!(migration_count, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn open_rejects_a_changed_v1_migration_checksum() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        drop(Storage::open(paths.clone(), &lease).expect("first open"));
        let connection = rusqlite::Connection::open(paths.database()).expect("database");
        connection
            .execute(
                "UPDATE schema_migrations SET checksum = ?1 WHERE version = 1",
                ["0".repeat(64)],
            )
            .expect("tamper checksum");
        drop(connection);

        let error = Storage::open(paths, &lease)
            .err()
            .expect("checksum mismatch must fail");

        assert!(matches!(error, StorageError::MigrationFailed));
    }

    #[test]
    fn open_rejects_a_changed_v2_migration_checksum() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        drop(Storage::open(paths.clone(), &lease).expect("first open"));
        let connection = rusqlite::Connection::open(paths.database()).expect("database");
        connection
            .execute(
                "UPDATE schema_migrations SET checksum = ?1 WHERE version = 2",
                ["0".repeat(64)],
            )
            .expect("tamper v2 checksum");
        drop(connection);

        let error = Storage::open(paths, &lease)
            .err()
            .expect("v2 checksum mismatch must fail");

        assert!(matches!(error, StorageError::MigrationFailed));
    }

    #[test]
    fn open_rejects_a_missing_migration_ledger() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        drop(Storage::open(paths.clone(), &lease).expect("first open"));
        let connection = rusqlite::Connection::open(paths.database()).expect("database");
        connection
            .execute("DROP TABLE schema_migrations", [])
            .expect("drop migration ledger");
        drop(connection);

        let error = Storage::open(paths, &lease)
            .err()
            .expect("missing ledger must fail");

        assert!(matches!(error, StorageError::MigrationFailed));
    }

    #[test]
    fn open_rejects_a_changed_v1_migration_name() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        drop(Storage::open(paths.clone(), &lease).expect("first open"));
        let connection = rusqlite::Connection::open(paths.database()).expect("database");
        connection
            .execute(
                "UPDATE schema_migrations SET name = '0001_changed' WHERE version = 1",
                [],
            )
            .expect("tamper migration name");
        drop(connection);

        let error = Storage::open(paths, &lease)
            .err()
            .expect("migration name mismatch must fail");

        assert!(matches!(error, StorageError::MigrationFailed));
    }

    #[test]
    fn open_rejects_a_changed_v2_migration_name() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        drop(Storage::open(paths.clone(), &lease).expect("first open"));
        let connection = rusqlite::Connection::open(paths.database()).expect("database");
        connection
            .execute(
                "UPDATE schema_migrations SET name = '0002_changed' WHERE version = 2",
                [],
            )
            .expect("tamper v2 migration name");
        drop(connection);

        let error = Storage::open(paths, &lease)
            .err()
            .expect("v2 migration name mismatch must fail");

        assert!(matches!(error, StorageError::MigrationFailed));
    }

    #[test]
    fn open_rejects_user_version_behind_the_migration_ledger() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        drop(Storage::open(paths.clone(), &lease).expect("first open"));
        let connection = rusqlite::Connection::open(paths.database()).expect("database");
        connection
            .execute_batch("PRAGMA user_version = 0")
            .expect("tamper user version");
        drop(connection);

        let error = Storage::open(paths, &lease)
            .err()
            .expect("version mismatch must fail");

        assert!(matches!(error, StorageError::MigrationFailed));
    }

    #[test]
    fn failed_migration_rolls_back_every_schema_change() {
        let mut connection = rusqlite::Connection::open_in_memory().expect("database");
        let sql = format!(
            "{}\nCREATE TABLE should_not_exist(id INTEGER) STRICT;\nTHIS IS NOT SQL;",
            include_str!("../../migrations/0001_foundation.sql")
        );

        let error = migrations::apply_sql_for_test(&mut connection, &sql)
            .expect_err("failed migration must roll back");
        let state = connection
            .query_row(
                "SELECT
                    (SELECT count(*) FROM sqlite_schema
                     WHERE type = 'table' AND name NOT LIKE 'sqlite_%'),
                    (SELECT user_version FROM pragma_user_version)",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .expect("migration state");

        assert!(matches!(error, StorageError::MigrationFailed));
        assert_eq!(state, (0, 0));
    }

    #[test]
    fn failure_after_migration_metadata_rolls_back_version_and_ledger() {
        let mut connection = rusqlite::Connection::open_in_memory().expect("database");

        let error = migrations::apply_sql_failing_after_metadata_for_test(
            &mut connection,
            include_str!("../../migrations/0001_foundation.sql"),
        )
        .expect_err("injected post-metadata failure must roll back");
        let state = connection
            .query_row(
                "SELECT
                    (SELECT user_version FROM pragma_user_version),
                    EXISTS(SELECT 1 FROM sqlite_schema
                           WHERE type = 'table' AND name = 'schema_migrations')",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?)),
            )
            .expect("migration state");

        assert!(matches!(error, StorageError::MigrationFailed));
        assert_eq!(state, (0, false));
    }

    #[test]
    fn migration_validation_runs_under_the_exclusive_migration_transaction() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        drop(Storage::open(paths.clone(), &lease).expect("initial migration"));
        let external = rusqlite::Connection::open(paths.database()).expect("external writer");
        external
            .execute_batch("BEGIN IMMEDIATE")
            .expect("external write transaction");
        let mut contender = rusqlite::Connection::open(paths.database()).expect("contender");
        contender
            .busy_timeout(Duration::from_millis(1))
            .expect("short busy timeout");

        let error = migrations::apply(&mut contender)
            .expect_err("validation must acquire the migration transaction");
        external
            .execute_batch("ROLLBACK")
            .expect("external rollback");

        assert!(matches!(error, StorageError::PersistenceUnavailable));
    }

    #[test]
    fn open_rejects_a_schema_version_newer_than_the_binary() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        drop(Storage::open(paths.clone(), &lease).expect("first open"));
        let connection = rusqlite::Connection::open(paths.database()).expect("database");
        connection
            .execute_batch(&format!(
                "PRAGMA user_version = {}",
                CURRENT_SCHEMA_VERSION + 1
            ))
            .expect("future version");
        drop(connection);

        let error = Storage::open(paths, &lease)
            .err()
            .expect("future schema must fail");

        assert!(matches!(error, StorageError::UnsupportedSchemaVersion));
    }

    #[test]
    fn writer_uses_wal_full_synchronous_and_a_five_second_busy_timeout() {
        let (_temp, _paths, _lease, storage) = open_storage();

        let pragmas = storage
            .write(|transaction| {
                let journal: String =
                    transaction.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
                let synchronous: i64 =
                    transaction.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
                let busy_timeout: i64 =
                    transaction.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
                Ok((journal, synchronous, busy_timeout))
            })
            .expect("writer pragmas");

        assert_eq!(pragmas, ("wal".to_string(), 2_i64, 5_000_i64));
    }

    #[test]
    fn every_connection_enforces_foreign_keys() {
        let (_temp, _paths, _lease, storage) = open_storage();
        storage
            .write(|transaction| {
                transaction.execute_batch(
                    "CREATE TABLE parent(id INTEGER PRIMARY KEY);\n                     CREATE TABLE child(parent_id INTEGER REFERENCES parent(id));",
                )?;
                Ok(())
            })
            .expect("test schema");

        let error = storage
            .write(|transaction| {
                transaction.execute("INSERT INTO child(parent_id) VALUES (99)", [])?;
                Ok(())
            })
            .expect_err("foreign key must fail");

        assert!(matches!(error, StorageError::Database));
    }

    #[test]
    fn failed_write_rolls_back_the_whole_transaction() {
        let (_temp, _paths, _lease, storage) = open_storage();

        let error = storage
            .write(|transaction| {
                transaction.execute(
                    "INSERT INTO settings(singleton_id, revision, theme_mode, updated_at)\n                     VALUES (1, 1, 'system', '2026-09-17T00:00:00Z')",
                    [],
                )?;
                Err::<(), _>(StorageError::OperationFailed)
            })
            .expect_err("operation fails");
        assert!(matches!(error, StorageError::OperationFailed));

        let count: i64 = storage
            .read(|connection| {
                connection.query_row("SELECT count(*) FROM settings", [], |row| row.get(0))
            })
            .expect("settings count");
        assert_eq!(count, 0_i64);
    }

    #[test]
    fn write_reports_an_explicit_rollback_failure() {
        let (_temp, _paths, _lease, storage) = open_storage();

        let error = storage
            .write(|transaction| {
                transaction.execute_batch("ROLLBACK")?;
                Err::<(), _>(StorageError::OperationFailed)
            })
            .expect_err("rollback failure must replace callback error");

        assert!(matches!(error, StorageError::Database));
    }

    #[test]
    fn writer_callbacks_never_overlap() {
        let (_temp, _paths, _lease, storage) = open_storage();
        let (first_entered_tx, first_entered_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let (second_started_tx, second_started_rx) = mpsc::channel();
        let (second_entered_tx, second_entered_rx) = mpsc::channel();
        let first_storage = Arc::clone(&storage);
        let first = thread::spawn(move || {
            first_storage.write(|_| {
                first_entered_tx.send(()).expect("first callback entered");
                release_first_rx.recv().expect("release first callback");
                Ok(())
            })
        });
        first_entered_rx.recv().expect("first callback");
        let second_storage = Arc::clone(&storage);
        let second = thread::spawn(move || {
            second_started_tx.send(()).expect("second writer started");
            second_storage.write(|_| {
                second_entered_tx.send(()).expect("second callback entered");
                Ok(())
            })
        });

        second_started_rx.recv().expect("second writer start");
        let overlapped = second_entered_rx
            .recv_timeout(Duration::from_millis(100))
            .is_ok();
        release_first_tx.send(()).expect("release first");
        first.join().expect("first writer").expect("first write");
        second.join().expect("second writer").expect("second write");

        assert!(!overlapped);
    }

    #[test]
    fn external_begin_immediate_contention_maps_to_persistence_unavailable() {
        let (_temp, paths, _lease, storage) = open_storage();
        let external = rusqlite::Connection::open(paths.database()).expect("external connection");
        external
            .execute_batch("BEGIN IMMEDIATE")
            .expect("external writer transaction");

        let error = storage
            .write(|_| Ok(()))
            .expect_err("external contention must time out");
        external
            .execute_batch("ROLLBACK")
            .expect("external rollback");

        assert!(matches!(error, StorageError::PersistenceUnavailable));
    }

    #[test]
    fn database_reopens_after_an_uncommitted_transaction() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        let storage = Storage::open(paths.clone(), &lease).expect("storage");
        storage
            .write(|transaction| {
                transaction.execute("CREATE TABLE reopen_probe(value INTEGER NOT NULL)", [])?;
                Ok(())
            })
            .expect("probe table");
        let _ = storage.write(|transaction| {
            transaction.execute("INSERT INTO reopen_probe(value) VALUES (1)", [])?;
            Err::<(), _>(StorageError::OperationFailed)
        });
        drop(storage);

        let reopened = Storage::open(paths, &lease).expect("reopened storage");
        let count: i64 = reopened
            .read(|connection| {
                connection.query_row("SELECT count(*) FROM reopen_probe", [], |row| row.get(0))
            })
            .expect("probe count");

        assert_eq!(count, 0_i64);
    }

    #[test]
    fn concurrent_writes_are_serialized_and_committed() {
        let (_temp, _paths, _lease, storage) = open_storage();
        storage
            .write(|transaction| {
                transaction.execute("CREATE TABLE write_probe(value INTEGER NOT NULL)", [])?;
                Ok(())
            })
            .expect("probe table");
        let barrier = Arc::new(Barrier::new(9));
        let handles: Vec<_> = (0..8)
            .map(|value| {
                let storage = Arc::clone(&storage);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    storage.write(|transaction| {
                        transaction
                            .execute("INSERT INTO write_probe(value) VALUES (?1)", [value])?;
                        Ok(())
                    })
                })
            })
            .collect();
        barrier.wait();

        for handle in handles {
            handle.join().expect("writer thread").expect("write");
        }
        let count: i64 = storage
            .read(|connection| {
                connection.query_row("SELECT count(*) FROM write_probe", [], |row| row.get(0))
            })
            .expect("probe count");

        assert_eq!(count, 8_i64);
    }

    #[test]
    fn read_transaction_keeps_one_wal_snapshot_while_a_writer_commits() {
        let (_temp, _paths, _lease, storage) = open_storage();
        storage
            .write(|transaction| {
                transaction.execute("CREATE TABLE read_probe(value INTEGER NOT NULL)", [])?;
                transaction.execute("INSERT INTO read_probe(value) VALUES (1)", [])?;
                Ok(())
            })
            .expect("probe seed");
        let (first_read_tx, first_read_rx) = mpsc::channel();
        let (writer_done_tx, writer_done_rx) = mpsc::channel();
        let reader_storage = Arc::clone(&storage);
        let reader = thread::spawn(move || {
            reader_storage.read_transaction(|transaction| {
                let first: i64 =
                    transaction
                        .query_row("SELECT count(*) FROM read_probe", [], |row| row.get(0))?;
                first_read_tx.send(()).expect("signal first read");
                writer_done_rx.recv().expect("writer completion");
                let second: i64 =
                    transaction
                        .query_row("SELECT count(*) FROM read_probe", [], |row| row.get(0))?;
                Ok((first, second))
            })
        });
        first_read_rx.recv().expect("first read");
        storage
            .write(|transaction| {
                transaction.execute("INSERT INTO read_probe(value) VALUES (2)", [])?;
                Ok(())
            })
            .expect("concurrent write");
        writer_done_tx.send(()).expect("signal writer completion");

        let observed = reader
            .join()
            .expect("reader thread")
            .expect("read transaction");

        assert_eq!(observed, (1_i64, 1_i64));
    }

    #[test]
    fn reads_use_fresh_connections() {
        let (_temp, _paths, _lease, storage) = open_storage();
        storage
            .read(|connection| {
                connection.execute("CREATE TEMP TABLE connection_probe(value INTEGER)", [])?;
                Ok(())
            })
            .expect("temporary table");

        let exists = storage
            .read(|connection| {
                connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE name = 'connection_probe')",
                    [],
                    |row| row.get::<_, bool>(0),
                )
            })
            .expect("temporary schema query");

        assert!(!exists);
    }

    #[test]
    fn online_snapshot_excludes_an_uncommitted_concurrent_write() {
        let (_temp, _paths, _lease, storage) = open_storage();
        storage
            .write(|transaction| {
                transaction.execute("CREATE TABLE snapshot_probe(value INTEGER NOT NULL)", [])?;
                transaction.execute("INSERT INTO snapshot_probe(value) VALUES (1)", [])?;
                Ok(())
            })
            .expect("probe seed");
        let (inserted_tx, inserted_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let writer_storage = Arc::clone(&storage);
        let writer = thread::spawn(move || {
            writer_storage.write(|transaction| {
                transaction.execute("INSERT INTO snapshot_probe(value) VALUES (2)", [])?;
                inserted_tx.send(()).expect("signal uncommitted insert");
                finish_rx.recv().expect("finish write");
                Ok(())
            })
        });
        inserted_rx.recv().expect("uncommitted insert");

        let snapshot = storage
            .create_snapshot(SnapshotKind::Settings)
            .expect("online snapshot");
        finish_tx.send(()).expect("finish writer");
        writer.join().expect("writer thread").expect("write");
        let snapshot_connection = rusqlite::Connection::open(snapshot.path()).expect("snapshot");
        let count: i64 = snapshot_connection
            .query_row("SELECT count(*) FROM snapshot_probe", [], |row| row.get(0))
            .expect("snapshot count");

        assert_eq!(count, 1_i64);
    }

    #[test]
    fn snapshot_retention_keeps_only_the_five_newest_files() {
        let (_temp, paths, _lease, storage) = open_storage();

        for _ in 0..6 {
            storage
                .create_snapshot(SnapshotKind::Printers)
                .expect("snapshot");
        }
        let snapshots = fs::read_dir(paths.snapshot_root())
            .expect("snapshot directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "sqlite3")
            })
            .count();

        assert_eq!(snapshots, 5);
    }

    #[test]
    fn snapshot_retention_counts_only_valid_snapshots() {
        let (_temp, paths, _lease, storage) = open_storage();
        let invalid = paths.snapshot_root().join(
            ".farm3d-pre-import-settings-99999999999999999999-ffffffff-ffff-4fff-8fff-ffffffffffff.sqlite3",
        );
        fs::write(&invalid, b"not a database").expect("invalid snapshot fixture");

        for _ in 0..6 {
            storage
                .create_snapshot(SnapshotKind::Settings)
                .expect("snapshot");
        }
        let valid_snapshots = fs::read_dir(paths.snapshot_root())
            .expect("snapshot directory")
            .filter_map(Result::ok)
            .filter(|entry| snapshot::validate_database(&entry.path()).is_ok())
            .count();

        assert_eq!(valid_snapshots, 5);
    }

    #[test]
    fn restore_staging_validates_a_copy_without_replacing_the_active_database() {
        let (_temp, paths, _lease, storage) = open_storage();
        let snapshot = storage
            .create_snapshot(SnapshotKind::Settings)
            .expect("snapshot");
        let active_before = fs::read(paths.database()).expect("active database");

        let staged = storage
            .stage_restore(snapshot.path())
            .expect("staged restore");

        assert!(paths
            .snapshot_root()
            .join(".restore-staging")
            .join(staged.staging_id())
            .join("candidate.sqlite3")
            .exists());
        assert_eq!(staged.validation(), &ValidationSummary::CURRENT);
        assert_eq!(
            fs::read(paths.database()).expect("active database"),
            active_before
        );
    }

    #[test]
    fn restore_staging_rejects_a_snapshot_with_a_changed_migration_checksum() {
        let (_temp, _paths, _lease, storage) = open_storage();
        let snapshot = storage
            .create_snapshot(SnapshotKind::Settings)
            .expect("snapshot");
        let connection = rusqlite::Connection::open(snapshot.path()).expect("snapshot database");
        connection
            .execute(
                "UPDATE schema_migrations SET checksum = ?1",
                ["0".repeat(64)],
            )
            .expect("tamper checksum");
        drop(connection);

        let error = storage
            .stage_restore(snapshot.path())
            .err()
            .expect("invalid snapshot must fail");

        assert!(matches!(error, StorageError::InvalidSnapshot));
    }

    #[test]
    fn restore_staging_rejects_a_snapshot_with_foreign_key_violations() {
        let (_temp, _paths, _lease, storage) = open_storage();
        storage
            .write(|transaction| {
                transaction.execute_batch(
                    "CREATE TABLE snapshot_parent(id INTEGER PRIMARY KEY);\n                     CREATE TABLE snapshot_child(parent_id INTEGER REFERENCES snapshot_parent(id));",
                )?;
                Ok(())
            })
            .expect("foreign key schema");
        let snapshot = storage
            .create_snapshot(SnapshotKind::Printers)
            .expect("snapshot");
        let connection = rusqlite::Connection::open(snapshot.path()).expect("snapshot database");
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .expect("disable foreign keys for corruption fixture");
        connection
            .execute("INSERT INTO snapshot_child(parent_id) VALUES (99)", [])
            .expect("foreign key violation");
        drop(connection);

        let error = storage
            .stage_restore(snapshot.path())
            .err()
            .expect("invalid snapshot must fail");

        assert!(matches!(error, StorageError::InvalidSnapshot));
    }

    #[test]
    fn restore_staging_rejects_a_database_outside_the_snapshot_root() {
        let (temp, _paths, _lease, storage) = open_storage();
        let outside = temp.path().join("outside.sqlite3");
        fs::write(&outside, []).expect("outside database");

        let error = storage
            .stage_restore(&outside)
            .err()
            .expect("outside database must fail");

        assert!(matches!(error, StorageError::InvalidSnapshot));
    }

    #[test]
    fn open_removes_incomplete_restore_staging_but_keeps_valid_candidates() {
        let (temp, paths, lease, storage) = open_storage();
        let snapshot = storage
            .create_snapshot(SnapshotKind::Settings)
            .expect("snapshot");
        let staged = storage
            .stage_restore(snapshot.path())
            .expect("staged restore");
        let valid_directory = paths
            .snapshot_root()
            .join(".restore-staging")
            .join(staged.staging_id());
        let incomplete = paths.snapshot_root().join(".restore-staging/incomplete");
        fs::create_dir_all(&incomplete).expect("incomplete staging");
        drop(storage);

        let reopened = Storage::open(paths, &lease).expect("reopen storage");

        assert!(valid_directory.exists());
        assert!(!incomplete.exists());
        drop((reopened, temp));
    }

    #[test]
    fn open_removes_incomplete_snapshot_files() {
        let (temp, paths, lease, storage) = open_storage();
        let partial = paths
            .snapshot_root()
            .join(".farm3d-pre-import-settings-incomplete.sqlite3.partial");
        fs::write(&partial, b"incomplete").expect("partial snapshot");
        drop(storage);

        let reopened = Storage::open(paths, &lease).expect("reopen storage");

        assert!(!partial.exists());
        drop((reopened, temp));
    }

    #[test]
    fn storage_errors_do_not_expose_paths_or_sql() {
        let temp = tempfile::tempdir().expect("temporary root");
        let metadata = temp.path().join("private-metadata");
        let paths = StoragePaths::new(&metadata, temp.path().join("data")).expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        let storage = Storage::open(paths, &lease).expect("storage");

        let error = storage
            .read(|connection| connection.execute("SELECT definitely_invalid", []))
            .expect_err("invalid SQL");
        let message = error.to_string();

        assert!(!message.contains("private-metadata") && !message.contains("definitely_invalid"));
    }

    #[cfg(unix)]
    #[test]
    fn storage_files_and_directories_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let (_temp, paths, _lease, storage) = open_storage();
        let snapshot = storage
            .create_snapshot(SnapshotKind::Settings)
            .expect("snapshot");

        let modes = (
            fs::metadata(paths.metadata_root())
                .expect("metadata root")
                .permissions()
                .mode()
                & 0o777,
            fs::metadata(paths.database())
                .expect("database")
                .permissions()
                .mode()
                & 0o777,
            fs::metadata(snapshot.path())
                .expect("snapshot")
                .permissions()
                .mode()
                & 0o777,
        );

        assert_eq!(modes, (0o700, 0o600, 0o600));
    }

    #[cfg(unix)]
    #[test]
    fn storage_paths_reject_legacy_directory_symlink_escape() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary root");
        let metadata = temp.path().join("metadata");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&metadata).expect("metadata root");
        fs::create_dir_all(&outside).expect("outside root");
        symlink(&outside, metadata.join("legacy")).expect("legacy symlink");

        let error = StoragePaths::new(&metadata, temp.path().join("data"))
            .expect_err("escaped legacy directory must fail");

        assert!(matches!(error, StorageError::PathCollision));
    }

    #[cfg(unix)]
    #[test]
    fn storage_paths_reject_snapshot_directory_symlink_escape() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary root");
        let metadata = temp.path().join("metadata");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&metadata).expect("metadata root");
        fs::create_dir_all(&outside).expect("outside root");
        symlink(&outside, metadata.join("snapshots")).expect("snapshot symlink");

        let error = StoragePaths::new(&metadata, temp.path().join("data"))
            .expect_err("escaped snapshot directory must fail");

        assert!(matches!(error, StorageError::PathCollision));
    }

    #[cfg(unix)]
    #[test]
    fn storage_paths_reject_database_hardlink_alias() {
        let temp = tempfile::tempdir().expect("temporary root");
        let metadata = temp.path().join("metadata");
        let outside = temp.path().join("outside.sqlite3");
        fs::create_dir_all(&metadata).expect("metadata root");
        fs::write(&outside, []).expect("outside database");
        fs::hard_link(&outside, metadata.join("farm3d.sqlite3")).expect("database hardlink");

        let error = StoragePaths::new(&metadata, temp.path().join("data"))
            .expect_err("database hardlink must fail");

        assert!(matches!(error, StorageError::PathCollision));
    }

    #[cfg(unix)]
    #[test]
    fn storage_paths_reject_database_alias_of_the_lock_file() {
        let temp = tempfile::tempdir().expect("temporary root");
        let metadata = temp.path().join("metadata");
        fs::create_dir_all(&metadata).expect("metadata root");
        let lock = metadata.join("farm3d.lock");
        fs::write(&lock, []).expect("lock file");
        fs::hard_link(&lock, metadata.join("farm3d.sqlite3")).expect("database hardlink");

        let error = StoragePaths::new(&metadata, temp.path().join("data"))
            .expect_err("database/lock alias must fail");

        assert!(matches!(error, StorageError::PathCollision));
    }

    #[cfg(unix)]
    #[test]
    fn storage_paths_reject_content_directory_symlink_escape() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary root");
        let data = temp.path().join("data");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&data).expect("data root");
        fs::create_dir_all(&outside).expect("outside root");
        symlink(&outside, data.join("farm3d-content")).expect("content symlink");

        let error = StoragePaths::new(temp.path().join("metadata"), &data)
            .expect_err("escaped content directory must fail");

        assert!(matches!(error, StorageError::PathCollision));
    }

    #[cfg(unix)]
    #[test]
    fn storage_paths_reject_database_symlink_escape() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary root");
        let metadata = temp.path().join("metadata");
        let outside = temp.path().join("outside.sqlite3");
        fs::create_dir_all(&metadata).expect("metadata root");
        fs::write(&outside, []).expect("outside database");
        symlink(&outside, metadata.join("farm3d.sqlite3")).expect("database symlink");

        let error = StoragePaths::new(&metadata, temp.path().join("data"))
            .expect_err("escaped database must fail");

        assert!(matches!(error, StorageError::PathCollision));
    }

    #[test]
    fn current_schema_inventory_is_exact_and_strict() {
        let (_temp, _paths, _lease, storage) = open_storage();

        let inventory = storage
            .read(|connection| {
                let mut statement = connection.prepare(
                    "SELECT type, name, sql FROM sqlite_schema
                     WHERE name NOT LIKE 'sqlite_%'
                     ORDER BY type, name",
                )?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .expect("schema inventory");
        let names: Vec<_> = inventory
            .iter()
            .map(|(kind, name, _)| (kind.as_str(), name.as_str()))
            .collect();
        // SQLite's own `strict` flag, rather than the CREATE text's suffix:
        // `slice_revision_blobs` is `STRICT, WITHOUT ROWID` (P5 D14).
        let non_strict_tables: Vec<String> = storage
            .read(|connection| {
                let mut statement = connection.prepare(
                    "SELECT name FROM pragma_table_list
                     WHERE schema = 'main' AND type = 'table' AND name NOT LIKE 'sqlite_%'
                       AND strict = 0",
                )?;
                let rows = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .expect("table strictness");
        let all_tables_are_strict = non_strict_tables.is_empty();

        assert_eq!(
            (names, all_tables_are_strict),
            (
                vec![
                    ("index", "host_operations_printer_created"),
                    ("index", "host_operations_printer_unresolved"),
                    ("index", "library_models_linked"),
                    ("index", "library_projects_name"),
                    ("index", "material_slots_live_name"),
                    ("index", "material_slots_live_position"),
                    ("index", "migration_warnings_dedup"),
                    ("index", "model_revision_thumbnails_content"),
                    ("index", "model_source_revisions_content"),
                    ("index", "printers_active_host_identity"),
                    ("index", "project_models_model"),
                    ("index", "slice_operations_active"),
                    ("index", "slice_revision_blobs_sha"),
                    ("index", "slice_revisions_gcode"),
                    ("index", "slice_revisions_model"),
                    ("index", "spool_movements_operation"),
                    ("index", "spool_movements_spool"),
                    ("index", "spool_reservations_open"),
                    ("index", "spool_tares_name"),
                    ("index", "spools_slot_occupancy"),
                    ("table", "content_blobs"),
                    ("table", "host_operations"),
                    ("table", "legacy_imports"),
                    ("table", "library_models"),
                    ("table", "library_projects"),
                    ("table", "material_slots"),
                    ("table", "migration_warnings"),
                    ("table", "model_revision_thumbnails"),
                    ("table", "model_source_revisions"),
                    ("table", "operations"),
                    ("table", "pending_blob_cleanup"),
                    ("table", "pending_credential_cleanup"),
                    ("table", "printer_status_snapshots"),
                    ("table", "printers"),
                    ("table", "project_models"),
                    ("table", "schema_migrations"),
                    ("table", "settings"),
                    ("table", "slice_operations"),
                    ("table", "slice_preparations"),
                    ("table", "slice_revision_blobs"),
                    ("table", "slice_revisions"),
                    ("table", "slicer_runtime_config"),
                    ("table", "spool_amount_events"),
                    ("table", "spool_movements"),
                    ("table", "spool_reservations"),
                    ("table", "spool_tares"),
                    ("table", "spools"),
                    ("trigger", "host_operations_terminal_immutable"),
                    ("trigger", "model_source_revisions_immutable"),
                    ("trigger", "slice_revision_blobs_immutable"),
                    ("trigger", "slice_revisions_immutable"),
                    ("trigger", "spool_amount_events_no_delete"),
                    ("trigger", "spool_amount_events_no_update"),
                ],
                true,
            )
        );
    }

    #[test]
    fn current_schema_columns_defaults_nullability_and_warning_index_are_exact() {
        let (_temp, _paths, _lease, storage) = open_storage();

        let (columns, warning_index_sql) = storage
            .read(|connection| {
                let mut statement = connection.prepare(
                    "SELECT m.name, p.name, p.type, p.[notnull], p.dflt_value, p.pk
                     FROM sqlite_schema AS m, pragma_table_info(m.name) AS p
                     WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%'
                     ORDER BY m.name, p.cid",
                )?;
                let columns = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let index_sql = connection.query_row(
                    "SELECT sql FROM sqlite_schema
                     WHERE type = 'index' AND name = 'migration_warnings_dedup'",
                    [],
                    |row| row.get::<_, String>(0),
                )?;
                Ok((columns, index_sql))
            })
            .expect("exact schema shape");
        let expected = [
            ("content_blobs", "sha256", "TEXT", 1, None, 1),
            ("content_blobs", "size_bytes", "INTEGER", 1, None, 0),
            ("content_blobs", "created_at", "TEXT", 1, None, 0),
            ("host_operations", "id", "TEXT", 1, None, 1),
            ("host_operations", "operation_id", "TEXT", 1, None, 0),
            ("host_operations", "printer_id", "TEXT", 1, None, 0),
            ("host_operations", "kind", "TEXT", 1, None, 0),
            ("host_operations", "slice_revision_id", "TEXT", 0, None, 0),
            (
                "host_operations",
                "source_host_operation_id",
                "TEXT",
                0,
                None,
                0,
            ),
            ("host_operations", "gcode_sha256", "TEXT", 0, None, 0),
            ("host_operations", "gcode_size", "INTEGER", 0, None, 0),
            ("host_operations", "host_path", "TEXT", 1, None, 0),
            ("host_operations", "history_mark", "INTEGER", 0, None, 0),
            ("host_operations", "endpoint_json", "TEXT", 1, None, 0),
            ("host_operations", "state", "TEXT", 1, None, 0),
            ("host_operations", "failure_json", "TEXT", 0, None, 0),
            ("host_operations", "resolution_json", "TEXT", 0, None, 0),
            ("host_operations", "attempts", "INTEGER", 1, Some("0"), 0),
            ("host_operations", "last_attempt_at", "TEXT", 0, None, 0),
            ("host_operations", "last_attempt_reason", "TEXT", 0, None, 0),
            (
                "host_operations",
                "no_longer_pending",
                "INTEGER",
                1,
                Some("0"),
                0,
            ),
            ("host_operations", "abandoned_at", "TEXT", 0, None, 0),
            ("host_operations", "abandon_note", "TEXT", 0, None, 0),
            ("host_operations", "created_at", "TEXT", 1, None, 0),
            ("host_operations", "dispatched_at", "TEXT", 0, None, 0),
            ("host_operations", "uncertain_since", "TEXT", 0, None, 0),
            ("host_operations", "resolved_at", "TEXT", 0, None, 0),
            ("legacy_imports", "source_name", "TEXT", 1, None, 1),
            ("legacy_imports", "source_present", "INTEGER", 1, None, 0),
            ("legacy_imports", "source_sha256", "TEXT", 0, None, 0),
            (
                "legacy_imports",
                "source_schema_version",
                "INTEGER",
                0,
                None,
                0,
            ),
            ("legacy_imports", "imported_rows", "INTEGER", 1, None, 0),
            ("legacy_imports", "completed_at", "TEXT", 1, None, 0),
            ("library_models", "id", "TEXT", 1, None, 1),
            ("library_models", "revision", "INTEGER", 1, None, 0),
            ("library_models", "name", "TEXT", 1, None, 0),
            ("library_models", "format", "TEXT", 1, None, 0),
            ("library_models", "storage_mode", "TEXT", 1, None, 0),
            ("library_models", "linked_path", "TEXT", 0, None, 0),
            ("library_models", "link_state", "TEXT", 0, None, 0),
            ("library_models", "link_checked_at", "TEXT", 0, None, 0),
            (
                "library_models",
                "link_observed_size",
                "INTEGER",
                0,
                None,
                0,
            ),
            (
                "library_models",
                "link_observed_mtime_ns",
                "INTEGER",
                0,
                None,
                0,
            ),
            (
                "library_models",
                "link_observed_file_id",
                "TEXT",
                0,
                None,
                0,
            ),
            ("library_models", "created_at", "TEXT", 1, None, 0),
            ("library_models", "updated_at", "TEXT", 1, None, 0),
            ("library_projects", "id", "TEXT", 1, None, 1),
            ("library_projects", "revision", "INTEGER", 1, None, 0),
            ("library_projects", "name", "TEXT", 1, None, 0),
            ("library_projects", "created_at", "TEXT", 1, None, 0),
            ("library_projects", "updated_at", "TEXT", 1, None, 0),
            ("material_slots", "id", "TEXT", 1, None, 1),
            ("material_slots", "printer_id", "TEXT", 1, None, 0),
            ("material_slots", "position", "INTEGER", 1, None, 0),
            ("material_slots", "name", "TEXT", 1, None, 0),
            ("material_slots", "feeder_label", "TEXT", 0, None, 0),
            ("material_slots", "removed_at", "TEXT", 0, None, 0),
            ("material_slots", "created_at", "TEXT", 1, None, 0),
            ("migration_warnings", "id", "TEXT", 1, None, 1),
            ("migration_warnings", "code", "TEXT", 1, None, 0),
            ("migration_warnings", "source_name", "TEXT", 0, None, 0),
            ("migration_warnings", "source_sha256", "TEXT", 0, None, 0),
            ("migration_warnings", "message", "TEXT", 1, None, 0),
            ("migration_warnings", "details_json", "TEXT", 1, None, 0),
            ("migration_warnings", "created_at", "TEXT", 1, None, 0),
            (
                "model_revision_thumbnails",
                "revision_id",
                "TEXT",
                1,
                None,
                1,
            ),
            ("model_revision_thumbnails", "source", "TEXT", 1, None, 0),
            (
                "model_revision_thumbnails",
                "origin_part",
                "TEXT",
                1,
                None,
                0,
            ),
            (
                "model_revision_thumbnails",
                "media_type",
                "TEXT",
                1,
                None,
                0,
            ),
            ("model_revision_thumbnails", "width", "INTEGER", 1, None, 0),
            ("model_revision_thumbnails", "height", "INTEGER", 1, None, 0),
            (
                "model_revision_thumbnails",
                "content_sha256",
                "TEXT",
                1,
                None,
                0,
            ),
            ("model_source_revisions", "id", "TEXT", 1, None, 1),
            ("model_source_revisions", "model_id", "TEXT", 1, None, 0),
            ("model_source_revisions", "sequence", "INTEGER", 1, None, 0),
            (
                "model_source_revisions",
                "content_sha256",
                "TEXT",
                1,
                None,
                0,
            ),
            (
                "model_source_revisions",
                "size_bytes",
                "INTEGER",
                1,
                None,
                0,
            ),
            ("model_source_revisions", "format", "TEXT", 1, None, 0),
            ("model_source_revisions", "origin", "TEXT", 1, None, 0),
            (
                "model_source_revisions",
                "source_file_name",
                "TEXT",
                1,
                None,
                0,
            ),
            ("model_source_revisions", "source_path", "TEXT", 1, None, 0),
            ("model_source_revisions", "source_mtime", "TEXT", 0, None, 0),
            ("model_source_revisions", "captured_at", "TEXT", 1, None, 0),
            (
                "model_source_revisions",
                "inspector_version",
                "INTEGER",
                1,
                None,
                0,
            ),
            (
                "model_source_revisions",
                "inspection_json",
                "TEXT",
                1,
                None,
                0,
            ),
            ("operations", "id", "TEXT", 1, None, 1),
            ("operations", "kind", "TEXT", 1, None, 0),
            ("operations", "request_digest", "TEXT", 1, None, 0),
            ("operations", "created_at", "TEXT", 1, None, 0),
            ("pending_blob_cleanup", "sha256", "TEXT", 1, None, 1),
            (
                "pending_blob_cleanup",
                "attempt_count",
                "INTEGER",
                1,
                Some("0"),
                0,
            ),
            (
                "pending_blob_cleanup",
                "last_error_code",
                "TEXT",
                0,
                None,
                0,
            ),
            ("pending_blob_cleanup", "created_at", "TEXT", 1, None, 0),
            (
                "pending_blob_cleanup",
                "last_attempt_at",
                "TEXT",
                0,
                None,
                0,
            ),
            (
                "pending_credential_cleanup",
                "credential_ref",
                "TEXT",
                1,
                None,
                1,
            ),
            (
                "pending_credential_cleanup",
                "printer_id",
                "TEXT",
                0,
                None,
                0,
            ),
            ("pending_credential_cleanup", "reason", "TEXT", 1, None, 0),
            (
                "pending_credential_cleanup",
                "attempt_count",
                "INTEGER",
                1,
                Some("0"),
                0,
            ),
            (
                "pending_credential_cleanup",
                "last_error_code",
                "TEXT",
                0,
                None,
                0,
            ),
            (
                "pending_credential_cleanup",
                "created_at",
                "TEXT",
                1,
                None,
                0,
            ),
            (
                "pending_credential_cleanup",
                "last_attempt_at",
                "TEXT",
                0,
                None,
                0,
            ),
            ("printer_status_snapshots", "printer_id", "TEXT", 1, None, 1),
            (
                "printer_status_snapshots",
                "telemetry_json",
                "TEXT",
                1,
                None,
                0,
            ),
            (
                "printer_status_snapshots",
                "last_observed_at",
                "TEXT",
                1,
                None,
                0,
            ),
            (
                "printer_status_snapshots",
                "persisted_at",
                "TEXT",
                1,
                None,
                0,
            ),
            ("printers", "id", "TEXT", 1, None, 1),
            ("printers", "revision", "INTEGER", 1, None, 0),
            ("printers", "name", "TEXT", 1, None, 0),
            ("printers", "catalog_vendor", "TEXT", 1, None, 0),
            ("printers", "catalog_model", "TEXT", 1, None, 0),
            ("printers", "catalog_variant", "TEXT", 1, None, 0),
            ("printers", "catalog_model_id", "TEXT", 1, None, 0),
            ("printers", "catalog_printer_variant", "TEXT", 1, None, 0),
            ("printers", "notes", "TEXT", 1, None, 0),
            ("printers", "overrides_json", "TEXT", 1, None, 0),
            ("printers", "last_known_good_json", "TEXT", 0, None, 0),
            ("printers", "connection_json", "TEXT", 0, None, 0),
            ("printers", "created_at", "TEXT", 1, None, 0),
            ("printers", "updated_at", "TEXT", 1, None, 0),
            ("printers", "location", "TEXT", 0, None, 0),
            (
                "printers",
                "start_safety",
                "TEXT",
                1,
                Some("'confirmBedClear'"),
                0,
            ),
            ("printers", "archived_at", "TEXT", 0, None, 0),
            ("printers", "host_identity", "TEXT", 0, None, 0),
            ("project_models", "project_id", "TEXT", 1, None, 1),
            ("project_models", "model_id", "TEXT", 1, None, 2),
            ("project_models", "added_at", "TEXT", 1, None, 0),
            ("schema_migrations", "version", "INTEGER", 0, None, 1),
            ("schema_migrations", "name", "TEXT", 1, None, 0),
            ("schema_migrations", "checksum", "TEXT", 1, None, 0),
            ("schema_migrations", "applied_at", "TEXT", 1, None, 0),
            ("settings", "singleton_id", "INTEGER", 0, None, 1),
            ("settings", "revision", "INTEGER", 1, None, 0),
            ("settings", "theme_mode", "TEXT", 1, None, 0),
            ("settings", "updated_at", "TEXT", 1, None, 0),
            (
                "settings",
                "monitor_section",
                "TEXT",
                1,
                Some("'printerModel'"),
                0,
            ),
            (
                "settings",
                "monitor_density",
                "TEXT",
                1,
                Some("'comfortable'"),
                0,
            ),
            ("slice_operations", "id", "TEXT", 1, None, 1),
            ("slice_operations", "preparation_id", "TEXT", 1, None, 0),
            ("slice_operations", "source_revision_id", "TEXT", 1, None, 0),
            ("slice_operations", "plate_key", "TEXT", 1, None, 0),
            (
                "slice_operations",
                "plate_snapshot_json",
                "TEXT",
                1,
                None,
                0,
            ),
            ("slice_operations", "state", "TEXT", 1, None, 0),
            ("slice_operations", "failure_json", "TEXT", 0, None, 0),
            ("slice_operations", "pid", "INTEGER", 0, None, 0),
            ("slice_operations", "pid_started_at", "INTEGER", 0, None, 0),
            ("slice_operations", "log_sha256", "TEXT", 0, None, 0),
            ("slice_operations", "slice_revision_id", "TEXT", 0, None, 0),
            ("slice_operations", "queued_at", "TEXT", 1, None, 0),
            ("slice_operations", "started_at", "TEXT", 0, None, 0),
            ("slice_operations", "finished_at", "TEXT", 0, None, 0),
            ("slice_preparations", "id", "TEXT", 1, None, 1),
            ("slice_preparations", "model_id", "TEXT", 1, None, 0),
            (
                "slice_preparations",
                "source_revision_id",
                "TEXT",
                1,
                None,
                0,
            ),
            ("slice_preparations", "revision", "INTEGER", 1, None, 0),
            ("slice_preparations", "document_json", "TEXT", 1, None, 0),
            ("slice_preparations", "created_at", "TEXT", 1, None, 0),
            ("slice_preparations", "updated_at", "TEXT", 1, None, 0),
            ("slice_revision_blobs", "revision_id", "TEXT", 1, None, 1),
            ("slice_revision_blobs", "role", "TEXT", 1, None, 2),
            ("slice_revision_blobs", "sha256", "TEXT", 1, None, 0),
            ("slice_revisions", "id", "TEXT", 1, None, 1),
            ("slice_revisions", "kind", "TEXT", 1, None, 0),
            ("slice_revisions", "model_id", "TEXT", 1, None, 0),
            ("slice_revisions", "source_revision_id", "TEXT", 1, None, 0),
            ("slice_revisions", "plate_key", "TEXT", 0, None, 0),
            ("slice_revisions", "plate_index", "INTEGER", 0, None, 0),
            ("slice_revisions", "plate_name", "TEXT", 0, None, 0),
            ("slice_revisions", "gcode_sha256", "TEXT", 1, None, 0),
            ("slice_revisions", "gcode_size", "INTEGER", 1, None, 0),
            ("slice_revisions", "target_json", "TEXT", 1, None, 0),
            ("slice_revisions", "facts_json", "TEXT", 1, None, 0),
            (
                "slice_revisions",
                "requires_manual_printer_selection",
                "INTEGER",
                1,
                None,
                0,
            ),
            ("slice_revisions", "estimates_json", "TEXT", 1, None, 0),
            ("slice_revisions", "runtime_json", "TEXT", 0, None, 0),
            ("slice_revisions", "created_at", "TEXT", 1, None, 0),
            (
                "slicer_runtime_config",
                "singleton_id",
                "INTEGER",
                0,
                None,
                1,
            ),
            ("slicer_runtime_config", "revision", "INTEGER", 1, None, 0),
            ("slicer_runtime_config", "engine_path", "TEXT", 0, None, 0),
            (
                "slicer_runtime_config",
                "preset_source_path",
                "TEXT",
                0,
                None,
                0,
            ),
            ("slicer_runtime_config", "updated_at", "TEXT", 1, None, 0),
            ("spool_amount_events", "id", "TEXT", 1, None, 1),
            ("spool_amount_events", "spool_id", "TEXT", 1, None, 0),
            ("spool_amount_events", "sequence", "INTEGER", 1, None, 0),
            ("spool_amount_events", "kind", "TEXT", 1, None, 0),
            ("spool_amount_events", "before_mg", "INTEGER", 0, None, 0),
            ("spool_amount_events", "after_mg", "INTEGER", 1, None, 0),
            (
                "spool_amount_events",
                "confidence_after",
                "TEXT",
                1,
                None,
                0,
            ),
            ("spool_amount_events", "gross_mg", "INTEGER", 0, None, 0),
            ("spool_amount_events", "tare_mg", "INTEGER", 0, None, 0),
            ("spool_amount_events", "reservation_id", "TEXT", 0, None, 0),
            ("spool_amount_events", "note", "TEXT", 0, None, 0),
            ("spool_amount_events", "occurred_at", "TEXT", 1, None, 0),
            ("spool_movements", "id", "TEXT", 1, None, 1),
            ("spool_movements", "operation_id", "TEXT", 1, None, 0),
            ("spool_movements", "spool_id", "TEXT", 1, None, 0),
            ("spool_movements", "reason", "TEXT", 1, None, 0),
            ("spool_movements", "from_slot_id", "TEXT", 0, None, 0),
            ("spool_movements", "from_storage_label", "TEXT", 0, None, 0),
            ("spool_movements", "to_slot_id", "TEXT", 0, None, 0),
            ("spool_movements", "to_storage_label", "TEXT", 0, None, 0),
            ("spool_movements", "occurred_at", "TEXT", 1, None, 0),
            ("spool_reservations", "id", "TEXT", 1, None, 1),
            ("spool_reservations", "spool_id", "TEXT", 1, None, 0),
            ("spool_reservations", "holder_kind", "TEXT", 1, None, 0),
            ("spool_reservations", "holder_id", "TEXT", 1, None, 0),
            ("spool_reservations", "amount_mg", "INTEGER", 1, None, 0),
            ("spool_reservations", "state", "TEXT", 1, None, 0),
            ("spool_reservations", "operation_id", "TEXT", 1, None, 0),
            ("spool_reservations", "created_at", "TEXT", 1, None, 0),
            ("spool_reservations", "settled_at", "TEXT", 0, None, 0),
            ("spool_tares", "id", "TEXT", 1, None, 1),
            ("spool_tares", "revision", "INTEGER", 1, None, 0),
            ("spool_tares", "name", "TEXT", 1, None, 0),
            ("spool_tares", "weight_mg", "INTEGER", 1, None, 0),
            ("spool_tares", "created_at", "TEXT", 1, None, 0),
            ("spool_tares", "updated_at", "TEXT", 1, None, 0),
            ("spools", "id", "TEXT", 1, None, 1),
            ("spools", "revision", "INTEGER", 1, None, 0),
            ("spools", "spool_number", "INTEGER", 1, None, 0),
            ("spools", "manufacturer", "TEXT", 1, None, 0),
            ("spools", "product", "TEXT", 0, None, 0),
            ("spools", "material_family", "TEXT", 1, None, 0),
            ("spools", "material_other", "TEXT", 0, None, 0),
            ("spools", "color_name", "TEXT", 1, None, 0),
            ("spools", "color_hex", "TEXT", 0, None, 0),
            ("spools", "diameter", "TEXT", 1, None, 0),
            ("spools", "nominal_mg", "INTEGER", 1, None, 0),
            ("spools", "current_mg", "INTEGER", 1, None, 0),
            ("spools", "confidence", "TEXT", 1, None, 0),
            (
                "spools",
                "low_threshold_mg",
                "INTEGER",
                1,
                Some("100000"),
                0,
            ),
            ("spools", "tare_id", "TEXT", 0, None, 0),
            ("spools", "lifecycle", "TEXT", 1, None, 0),
            ("spools", "archived_from", "TEXT", 0, None, 0),
            ("spools", "slot_id", "TEXT", 0, None, 0),
            ("spools", "storage_label", "TEXT", 0, None, 0),
            ("spools", "last_measured_at", "TEXT", 0, None, 0),
            ("spools", "notes", "TEXT", 0, None, 0),
            ("spools", "created_at", "TEXT", 1, None, 0),
            ("spools", "updated_at", "TEXT", 1, None, 0),
        ];
        let actual: Vec<_> = columns
            .iter()
            .map(|(table, name, kind, not_null, default, primary_key)| {
                (
                    table.as_str(),
                    name.as_str(),
                    kind.as_str(),
                    *not_null,
                    default.as_deref(),
                    *primary_key,
                )
            })
            .collect();

        assert_eq!(actual, expected);
        assert_eq!(
            warning_index_sql,
            "CREATE UNIQUE INDEX migration_warnings_dedup\nON migration_warnings (\n    code,\n    COALESCE(source_name, ''),\n    COALESCE(source_sha256, '')\n)"
        );
    }

    #[test]
    fn printers_schema_has_no_job_or_queue_columns() {
        // Pins the printers columns to exactly the P1 baseline plus P2's
        // lifecycle/host-identity columns (0003). P3 (0004) adds its own
        // tables (material_slots, spools, ...) without touching printers at
        // all, which the two schema-inventory pin tests above already
        // guard — this one is a forward guard against a later phase's Job
        // or Queue columns landing on `printers` early.
        let (_temp, _paths, _lease, storage) = open_storage();

        let columns = storage
            .read(|connection| {
                let mut statement = connection.prepare("PRAGMA table_info(printers)")?;
                let rows = statement
                    .query_map([], |row| row.get::<_, String>(1))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .expect("printer columns");

        assert_eq!(
            columns,
            vec![
                "id",
                "revision",
                "name",
                "catalog_vendor",
                "catalog_model",
                "catalog_variant",
                "catalog_model_id",
                "catalog_printer_variant",
                "notes",
                "overrides_json",
                "last_known_good_json",
                "connection_json",
                "created_at",
                "updated_at",
                "location",
                "start_safety",
                "archived_at",
                "host_identity",
            ]
        );
    }

    #[test]
    fn strict_schema_rejects_wrong_storage_classes_and_invalid_warning_ids() {
        let (_temp, _paths, _lease, storage) = open_storage();

        let failures = storage
            .write(|transaction| {
                let blob_id = transaction.execute(
                    "INSERT INTO migration_warnings(
                        id, code, message, details_json, created_at
                     ) VALUES (?1, 'CODE', 'message', '{}', '2026-09-17T00:00:00Z')",
                    [rusqlite::types::Value::Blob(vec![1, 2, 3])],
                );
                let invalid_uuid = transaction.execute(
                    "INSERT INTO migration_warnings(
                        id, code, message, details_json, created_at
                     ) VALUES ('NOT-A-UUID', 'CODE', 'message', '{}', '2026-09-17T00:00:00Z')",
                    [],
                );
                let blob_revision = transaction.execute(
                    "INSERT INTO settings(singleton_id, revision, theme_mode, updated_at)
                     VALUES (1, ?1, 'system', '2026-09-17T00:00:00Z')",
                    [rusqlite::types::Value::Blob(vec![1])],
                );
                let invalid_json = transaction.execute(
                    "INSERT INTO printers(
                        id, revision, name, catalog_vendor, catalog_model, catalog_variant,
                        catalog_model_id, catalog_printer_variant, notes, overrides_json,
                        created_at, updated_at
                     ) VALUES (
                        'printer', 1, '', '', '', '', '', '', '', '[]',
                        '2026-09-17T00:00:00Z', '2026-09-17T00:00:00Z'
                     )",
                    [],
                );
                Ok((
                    blob_id.is_err(),
                    invalid_uuid.is_err(),
                    blob_revision.is_err(),
                    invalid_json.is_err(),
                ))
            })
            .expect("validation transaction");

        assert_eq!(failures, (true, true, true, true));
    }

    #[test]
    fn warning_ids_accept_only_canonical_lowercase_uuid_v4() {
        let (_temp, _paths, _lease, storage) = open_storage();

        let outcomes = storage
            .write(|transaction| {
                let valid = transaction.execute(
                    "INSERT INTO migration_warnings(
                        id, code, message, details_json, created_at
                     ) VALUES ('123e4567-e89b-42d3-a456-426614174000', 'VALID', 'message', '{}', '2026-09-17T00:00:00Z')",
                    [],
                );
                let uppercase = transaction.execute(
                    "INSERT INTO migration_warnings(
                        id, code, message, details_json, created_at
                     ) VALUES ('123E4567-E89B-42D3-A456-426614174001', 'UPPER', 'message', '{}', '2026-09-17T00:00:00Z')",
                    [],
                );
                let wrong_version = transaction.execute(
                    "INSERT INTO migration_warnings(
                        id, code, message, details_json, created_at
                     ) VALUES ('123e4567-e89b-12d3-a456-426614174002', 'VERSION', 'message', '{}', '2026-09-17T00:00:00Z')",
                    [],
                );
                let extra_hyphen = transaction.execute(
                    "INSERT INTO migration_warnings(
                        id, code, message, details_json, created_at
                     ) VALUES ('123e4567-e89b-42d3-a456-426614174-00', 'HYPHEN', 'message', '{}', '2026-09-17T00:00:00Z')",
                    [],
                );
                Ok((
                    valid.is_ok(),
                    uppercase.is_err(),
                    wrong_version.is_err(),
                    extra_hyphen.is_err(),
                ))
            })
            .expect("warning ID validation");

        assert_eq!(outcomes, (true, true, true, true));
    }

    #[test]
    fn reader_uses_foreign_keys_and_a_five_second_busy_timeout() {
        let (_temp, _paths, _lease, storage) = open_storage();

        let pragmas = storage
            .read(|connection| {
                let foreign_keys: i64 =
                    connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
                let busy_timeout: i64 =
                    connection.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
                Ok((foreign_keys, busy_timeout))
            })
            .expect("reader pragmas");

        assert_eq!(pragmas, (1_i64, 5_000_i64));
    }

    #[test]
    fn snapshot_retention_orders_mixed_domains_by_timestamp() {
        let (_temp, paths, _lease, storage) = open_storage();
        let fixtures = [
            (SnapshotKind::Settings, "settings", 1_u128),
            (SnapshotKind::Settings, "settings", 2),
            (SnapshotKind::Printers, "printers", 3),
            (SnapshotKind::Printers, "printers", 4),
            (SnapshotKind::Printers, "printers", 5),
        ];
        let mut renamed = Vec::new();
        for (index, (kind, domain, timestamp)) in fixtures.into_iter().enumerate() {
            let snapshot = storage.create_snapshot(kind).expect("snapshot");
            let destination = paths.snapshot_root().join(format!(
                ".farm3d-pre-import-{domain}-{timestamp:020}-00000000-0000-4000-8000-{index:012}.sqlite3"
            ));
            fs::rename(snapshot.path(), &destination).expect("rename snapshot fixture");
            renamed.push(destination);
        }

        storage
            .create_snapshot(SnapshotKind::Settings)
            .expect("newest snapshot");

        assert!(!renamed[0].exists() && renamed[2].exists());
    }

    #[test]
    fn snapshot_retention_never_deletes_the_just_accepted_snapshot() {
        let (_temp, paths, _lease, storage) = open_storage();
        for index in 0..5 {
            let snapshot = storage
                .create_snapshot(SnapshotKind::Settings)
                .expect("snapshot");
            let future = paths.snapshot_root().join(format!(
                ".farm3d-pre-import-settings-9999999999999999999{index}-00000000-0000-4000-8000-{index:012}.sqlite3"
            ));
            fs::rename(snapshot.path(), future).expect("future snapshot fixture");
        }

        let accepted = storage
            .create_snapshot(SnapshotKind::Printers)
            .expect("accepted snapshot");

        assert!(accepted.path().exists());
    }

    #[test]
    fn snapshot_operations_are_serialized_by_the_storage_mutex() {
        let (_temp, _paths, _lease, storage) = open_storage();
        let guard = storage.lock_snapshot().expect("snapshot guard");
        let worker_storage = Arc::clone(&storage);
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            started_tx.send(()).expect("snapshot worker started");
            let result = worker_storage.create_snapshot(SnapshotKind::Settings);
            done_tx.send(result.is_ok()).expect("snapshot result");
        });

        started_rx.recv().expect("snapshot worker start");
        let overlapped = done_rx.recv_timeout(Duration::from_millis(100)).is_ok();
        drop(guard);
        let completed = done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("serialized snapshot completion");
        worker.join().expect("snapshot worker");

        assert!(!overlapped && completed);
    }

    #[test]
    fn concurrent_mixed_snapshots_retain_exactly_five_valid_files() {
        let (_temp, paths, _lease, storage) = open_storage();
        let barrier = Arc::new(Barrier::new(9));
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let storage = Arc::clone(&storage);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    let kind = if index % 2 == 0 {
                        SnapshotKind::Settings
                    } else {
                        SnapshotKind::Printers
                    };
                    storage.create_snapshot(kind)
                })
            })
            .collect();
        barrier.wait();
        for worker in workers {
            worker.join().expect("snapshot worker").expect("snapshot");
        }

        let valid = fs::read_dir(paths.snapshot_root())
            .expect("snapshot root")
            .filter_map(Result::ok)
            .filter(|entry| snapshot::validate_database(&entry.path()).is_ok())
            .count();

        assert_eq!(valid, 5);
    }

    #[test]
    fn retention_deletion_failure_records_warning_without_rejecting_snapshot() {
        let (_temp, paths, _lease, storage) = open_storage();
        for _ in 0..5 {
            storage
                .create_snapshot(SnapshotKind::Settings)
                .expect("snapshot");
        }
        storage.fail_retention_deletion();

        let accepted = storage
            .create_snapshot(SnapshotKind::Printers)
            .expect("accepted snapshot");
        let warning_count: i64 = storage
            .read(|connection| {
                connection.query_row(
                    "SELECT count(*) FROM migration_warnings
                     WHERE code = 'SNAPSHOT_RETENTION_FAILED'",
                    [],
                    |row| row.get(0),
                )
            })
            .expect("retention warning");
        let valid_count = fs::read_dir(paths.snapshot_root())
            .expect("snapshot root")
            .filter_map(Result::ok)
            .filter(|entry| snapshot::validate_database(&entry.path()).is_ok())
            .count();

        assert!(accepted.path().exists() && warning_count == 1 && valid_count == 6);
    }

    #[test]
    fn snapshot_with_future_schema_is_rejected() {
        let (_temp, _paths, _lease, storage) = open_storage();
        let snapshot = storage
            .create_snapshot(SnapshotKind::Settings)
            .expect("snapshot");
        let connection = rusqlite::Connection::open(snapshot.path()).expect("snapshot database");
        connection
            .execute_batch(&format!(
                "PRAGMA user_version = {}",
                CURRENT_SCHEMA_VERSION + 1
            ))
            .expect("future schema");
        drop(connection);

        let error = storage
            .stage_restore(snapshot.path())
            .err()
            .expect("future snapshot must fail");

        assert!(matches!(error, StorageError::UnsupportedSchemaVersion));
    }

    #[test]
    fn snapshot_with_corrupt_header_is_rejected() {
        use std::io::{Seek, SeekFrom};

        let (_temp, _paths, _lease, storage) = open_storage();
        let snapshot = storage
            .create_snapshot(SnapshotKind::Settings)
            .expect("snapshot");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(snapshot.path())
            .expect("snapshot file");
        file.seek(SeekFrom::Start(0)).expect("snapshot header");
        file.write_all(b"not a sqlite db!")
            .expect("corrupt snapshot header");
        file.sync_all().expect("sync corruption");
        drop(file);

        let error = storage
            .stage_restore(snapshot.path())
            .err()
            .expect("corrupt snapshot must fail");

        assert!(matches!(error, StorageError::InvalidSnapshot));
    }

    #[test]
    fn snapshot_with_non_ok_integrity_check_is_rejected() {
        use std::io::{Seek, SeekFrom};

        let (_temp, _paths, _lease, storage) = open_storage();
        let snapshot = storage
            .create_snapshot(SnapshotKind::Settings)
            .expect("snapshot");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(snapshot.path())
            .expect("snapshot file");
        file.seek(SeekFrom::Start(36)).expect("freelist count");
        file.write_all(&1_u32.to_be_bytes())
            .expect("corrupt freelist count");
        file.sync_all().expect("sync corruption");
        drop(file);

        let connection = rusqlite::Connection::open(snapshot.path()).expect("openable snapshot");
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .expect("integrity result");
        drop(connection);
        let error = storage
            .stage_restore(snapshot.path())
            .err()
            .expect("failed integrity check must reject snapshot");

        assert_ne!(integrity, "ok");
        assert!(matches!(error, StorageError::InvalidSnapshot));
    }

    #[test]
    fn snapshot_validation_uses_one_read_snapshot_during_concurrent_mutation() {
        let (_temp, _paths, _lease, storage) = open_storage();
        storage
            .write(|transaction| {
                transaction.execute_batch(
                    "CREATE TABLE validation_parent(id INTEGER PRIMARY KEY) STRICT;
                     CREATE TABLE validation_child(
                         parent_id INTEGER REFERENCES validation_parent(id)
                     ) STRICT;",
                )?;
                Ok(())
            })
            .expect("foreign key schema");
        let snapshot = storage
            .create_snapshot(SnapshotKind::Settings)
            .expect("snapshot");

        let validation = snapshot::validate_database_during_test(snapshot.path(), || {
            let connection = rusqlite::Connection::open(snapshot.path())?;
            connection.pragma_update(None, "foreign_keys", "OFF")?;
            connection.execute("INSERT INTO validation_child(parent_id) VALUES (99)", [])?;
            Ok(())
        });
        let connection = rusqlite::Connection::open(snapshot.path()).expect("mutated snapshot");
        let has_violation = migrations::has_foreign_key_violation(&connection)
            .expect("foreign key check after mutation");

        assert_eq!(
            validation.expect("stable validation snapshot"),
            ValidationSummary::CURRENT
        );
        assert!(has_violation);
    }

    #[test]
    fn staged_restore_candidate_matches_snapshot_data() {
        let (_temp, paths, _lease, storage) = open_storage();
        storage
            .write(|transaction| {
                transaction.execute(
                    "INSERT INTO settings(singleton_id, revision, theme_mode, updated_at)
                     VALUES (1, 7, 'farm3d-dark', '2026-09-17T00:00:00Z')",
                    [],
                )?;
                Ok(())
            })
            .expect("settings fixture");
        let snapshot = storage
            .create_snapshot(SnapshotKind::Settings)
            .expect("snapshot");

        let staged = storage
            .stage_restore(snapshot.path())
            .expect("staged restore");
        let candidate = paths
            .snapshot_root()
            .join(".restore-staging")
            .join(staged.staging_id())
            .join("candidate.sqlite3");
        let connection = rusqlite::Connection::open(candidate).expect("candidate database");
        let row: (i64, String) = connection
            .query_row(
                "SELECT revision, theme_mode FROM settings WHERE singleton_id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("staged settings");

        assert_eq!(row, (7, "farm3d-dark".to_string()));
    }

    #[test]
    fn lock_file_open_failure_is_persistence_unavailable() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        fs::create_dir(paths.metadata_root().join("farm3d.lock")).expect("invalid lock path");

        let error = MetadataRootLease::acquire(&paths).expect_err("lock open must fail");

        assert!(matches!(error, StorageError::PersistenceUnavailable));
    }

    #[test]
    fn unsupported_lock_errors_remain_distinct() {
        let unsupported = database::classify_lock_error(std::fs::TryLockError::Error(
            std::io::Error::from(std::io::ErrorKind::Unsupported),
        ));
        let other = database::classify_lock_error(std::fs::TryLockError::Error(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        ));

        assert!(matches!(unsupported, StorageError::UnsupportedLocking));
        assert!(matches!(other, StorageError::PersistenceUnavailable));
    }

    #[test]
    #[ignore = "subprocess helper"]
    fn metadata_lease_process_helper() {
        let metadata = std::env::var_os("FARM3D_LEASE_METADATA").expect("metadata path");
        let data = std::env::var_os("FARM3D_LEASE_DATA").expect("data path");
        let paths = StoragePaths::new(metadata, data).expect("storage paths");
        let _lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        println!("FARM3D_LEASE_READY");
        std::io::stdout().flush().expect("flush readiness");
        let mut input = Vec::new();
        std::io::stdin()
            .read_to_end(&mut input)
            .expect("wait for release");
    }

    fn spawn_lease_holder(temp: &TempDir) -> Child {
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--ignored",
                "--exact",
                "persistence::tests::metadata_lease_process_helper",
                "--nocapture",
            ])
            .env("FARM3D_LEASE_METADATA", temp.path().join("metadata"))
            .env("FARM3D_LEASE_DATA", temp.path().join("data"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("lease child");
        let stdout = child.stdout.take().expect("child stdout");
        let (ready_tx, ready_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut sent = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line == "FARM3D_LEASE_READY" && !sent {
                    let _ = ready_tx.send(());
                    sent = true;
                }
            }
        });
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("lease child readiness");
        child
    }

    #[test]
    fn metadata_lease_is_released_after_normal_child_exit() {
        let temp = tempfile::tempdir().expect("temporary root");
        let mut child = spawn_lease_holder(&temp);
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let contention = MetadataRootLease::acquire(&paths).expect_err("child owns lease");
        drop(child.stdin.take());
        assert!(child.wait().expect("child exit").success());

        let reacquired = MetadataRootLease::acquire(&paths);

        assert!(matches!(contention, StorageError::PersistenceUnavailable) && reacquired.is_ok());
    }

    #[test]
    fn metadata_lease_is_released_after_abrupt_child_termination() {
        let temp = tempfile::tempdir().expect("temporary root");
        let mut child = spawn_lease_holder(&temp);
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let contention = MetadataRootLease::acquire(&paths).expect_err("child owns lease");
        child.kill().expect("terminate child");
        child.wait().expect("reap child");

        let reacquired = MetadataRootLease::acquire(&paths);

        assert!(matches!(contention, StorageError::PersistenceUnavailable) && reacquired.is_ok());
    }

    #[test]
    fn metadata_lease_accepts_a_preexisting_unlocked_lock_file() {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        fs::write(paths.metadata_root().join("farm3d.lock"), []).expect("lock file");

        let mut child = spawn_lease_holder(&temp);
        drop(child.stdin.take());
        let status = child.wait().expect("child exit");

        assert!(status.success());
    }
}
