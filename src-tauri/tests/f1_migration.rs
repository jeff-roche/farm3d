use std::fs;

use farm3d_lib::persistence::{migrate_legacy, MetadataRootLease, Storage, StoragePaths};

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/persistence/v1")
        .join(name)
}

#[test]
fn canonical_f0_data_migrates_once_without_reading_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let metadata = temp.path().join("metadata");
    fs::create_dir_all(&metadata).unwrap();
    fs::copy(fixture("settings.json"), metadata.join("settings.json")).unwrap();
    fs::copy(fixture("printers.json"), metadata.join("printers.json")).unwrap();
    fs::copy(
        fixture("credentials.json"),
        metadata.join("credentials.json"),
    )
    .unwrap();
    let paths = StoragePaths::new(&metadata, temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Storage::open(paths.clone(), &lease).unwrap();

    let outcome = migrate_legacy(&storage).unwrap();
    assert_eq!((outcome.settings_rows, outcome.printer_rows), (1, 2));
    migrate_legacy(&storage).unwrap();
    let state = storage
        .read(|connection| {
            let theme: String = connection.query_row(
                "SELECT theme_mode FROM settings WHERE singleton_id = 1",
                [],
                |row| row.get(0),
            )?;
            let rows: i64 =
                connection.query_row("SELECT count(*) FROM printers", [], |row| row.get(0))?;
            let connection_json: String = connection.query_row(
                "SELECT connection_json FROM printers WHERE id = 'prn-f0-connected'",
                [],
                |row| row.get(0),
            )?;
            let overrides: String = connection.query_row(
                "SELECT overrides_json FROM printers WHERE id = 'prn-f0-profile-only'",
                [],
                |row| row.get(0),
            )?;
            Ok((theme, rows, connection_json, overrides))
        })
        .unwrap();
    assert_eq!(state.0, "farm3d-dark");
    assert_eq!(state.1, 2);
    assert!(state.2.contains("farm3d/printer/prn-f0-connected/apikey"));
    assert!(state.3.contains("futureBaselineField"));

    let database = fs::read(paths.database()).unwrap();
    assert!(!String::from_utf8_lossy(&database).contains("F0_FIXTURE_SENTINEL"));
}

#[test]
fn corrupt_printers_roll_back_both_domains() {
    let temp = tempfile::tempdir().unwrap();
    let metadata = temp.path().join("metadata");
    fs::create_dir_all(&metadata).unwrap();
    fs::copy(fixture("settings.json"), metadata.join("settings.json")).unwrap();
    fs::write(metadata.join("printers.json"), b"not json").unwrap();
    let paths = StoragePaths::new(&metadata, temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Storage::open(paths, &lease).unwrap();

    assert!(migrate_legacy(&storage).is_err());
    let count: i64 = storage
        .read(|connection| {
            connection.query_row("SELECT count(*) FROM settings", [], |row| row.get(0))
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn corrupt_settings_default_and_monitor_groups_are_recorded_once() {
    let temp = tempfile::tempdir().unwrap();
    let metadata = temp.path().join("metadata");
    fs::create_dir_all(&metadata).unwrap();
    fs::write(metadata.join("settings.json"), b"not json").unwrap();
    fs::copy(fixture("printers.json"), metadata.join("printers.json")).unwrap();
    let paths = StoragePaths::new(&metadata, temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Storage::open(paths, &lease).unwrap();

    migrate_legacy(&storage).unwrap();
    migrate_legacy(&storage).unwrap();

    let warnings = storage
        .read(|connection| {
            let mut statement = connection
                .prepare("SELECT code, details_json FROM migration_warnings ORDER BY code")?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>();
            rows
        })
        .unwrap();
    assert!(warnings
        .iter()
        .any(|row| row.0 == "LEGACY_SETTINGS_DEFAULTED"));
    assert_eq!(
        warnings
            .iter()
            .filter(|row| row.0 == "LEGACY_MONITOR_GROUP_IGNORED")
            .count(),
        1
    );
    assert!(warnings
        .iter()
        .any(|row| row.1.contains(r#""printerCount":2"#)));
}

#[test]
fn a_source_that_appears_after_completed_migration_is_archived_without_import() {
    let temp = tempfile::tempdir().unwrap();
    let metadata = temp.path().join("metadata");
    fs::create_dir_all(&metadata).unwrap();
    let paths = StoragePaths::new(&metadata, temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Storage::open(paths.clone(), &lease).unwrap();
    migrate_legacy(&storage).unwrap();
    fs::write(
        metadata.join("printers.json"),
        br#"{"schemaVersion":1,"printers":[]}"#,
    )
    .unwrap();

    migrate_legacy(&storage).unwrap();

    assert!(!metadata.join("printers.json").exists());
    let archives = fs::read_dir(paths.legacy_root())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(archives
        .iter()
        .any(|name| name.starts_with("printers.ignored-after-migration-")));
}

#[cfg(unix)]
#[test]
fn legacy_import_rejects_symlink_inputs_without_following_them() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let metadata = temp.path().join("metadata");
    fs::create_dir_all(&metadata).unwrap();
    let outside = temp.path().join("outside.json");
    fs::write(&outside, br#"{"themeMode":"farm3d-dark"}"#).unwrap();
    symlink(&outside, metadata.join("settings.json")).unwrap();
    let paths = StoragePaths::new(&metadata, temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Storage::open(paths, &lease).unwrap();

    assert!(migrate_legacy(&storage).is_err());
    assert_eq!(
        fs::read(&outside).unwrap(),
        br#"{"themeMode":"farm3d-dark"}"#
    );
}
