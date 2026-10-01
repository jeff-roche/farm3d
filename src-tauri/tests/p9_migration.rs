//! Migration 0010 (P9): the v9 -> v10 upgrade keeps every row; the four
//! rebuilt tables (`camera_snapshots`, `incident_events`,
//! `pending_credential_cleanup`, `operations`) keep their columns,
//! indexes, triggers, and foreign keys; the rebuild works on a database
//! holding an `evidenceCaptured` Incident event (the RESTRICT foreign key
//! from `incident_events` to `camera_snapshots`); the new CHECK values are
//! accepted; the history indexes exist; a v11 database is refused. See the
//! P9 design spec's "Schema" section.

use sha2::{Digest, Sha256};

use farm3d_lib::persistence::test_support::apply_through;
use farm3d_lib::persistence::{
    MetadataRootLease, Storage, StorageError, StoragePaths, CURRENT_SCHEMA_VERSION,
};

#[path = "common/farm_seed.rs"]
mod farm_seed;
use farm_seed::*;

fn paths_in(temp: &tempfile::TempDir) -> StoragePaths {
    StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).expect("paths")
}

/// A v9 database with foreign keys on, holding a populated fixture: a Job
/// chain, an Incident with an `evidenceCaptured` event, an operations row,
/// and a pending credential cleanup row.
fn populated_v9(paths: &StoragePaths) -> rusqlite::Connection {
    let mut connection = rusqlite::Connection::open(paths.database()).expect("v9 database");
    connection
        .execute_batch("PRAGMA foreign_keys = ON")
        .expect("foreign keys on");
    apply_through(&mut connection, 9).expect("v9 migrations");
    seed_job_chain(&connection, "a");
    seed_incident(&connection, "inc-a", "prn-a");
    seed_incident_snapshot(
        &connection,
        "snp-a",
        "prn-a",
        "inc-a",
        "snapshots/2026/01/snp-a.jpg",
    );
    seed_evidence_captured_event(&connection, "iev-a", "inc-a", "snp-a");
    seed_manual_snapshot(&connection, "snp-b", "prn-a", "snapshots/2026/01/snp-b.jpg");
    seed_operation(&connection, "op-a", "moveSpool");
    seed_pending_credential_cleanup(&connection, "cred-a", "cleared");
    connection
}

const REBUILT: [&str; 4] = [
    "camera_snapshots",
    "incident_events",
    "pending_credential_cleanup",
    "operations",
];

/// Everything about `table`'s shape except CHECK text and name quoting:
/// columns, foreign keys, index columns (by origin/uniqueness/partiality),
/// and the SQL of named indexes and triggers with quotes stripped.
fn shape(connection: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut collect = |sql: String, columns: usize| {
        let mut statement = connection.prepare(&sql).expect("prepare");
        let rows = statement
            .query_map([], |row| {
                Ok((0..columns)
                    .map(|index| {
                        row.get::<_, rusqlite::types::Value>(index)
                            .map(|value| format!("{value:?}"))
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join("|"))
            })
            .expect("query");
        for row in rows {
            lines.push(row.expect("row"));
        }
    };
    collect(
        format!("SELECT 'col', name, type, \"notnull\", dflt_value, pk, hidden FROM pragma_table_xinfo('{table}') ORDER BY cid"),
        7,
    );
    collect(
        format!("SELECT 'fk', \"table\", \"from\", \"to\", on_update, on_delete FROM pragma_foreign_key_list('{table}') ORDER BY \"from\""),
        6,
    );
    // Index columns, independent of the auto-generated index names.
    let index_names: Vec<(String, String, i64, i64)> = {
        let mut statement = connection
            .prepare(&format!(
                "SELECT name, origin, \"unique\", partial FROM pragma_index_list('{table}')"
            ))
            .expect("index_list");
        let rows = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("rows");
        rows.map(|row| row.expect("index row")).collect()
    };
    for (name, origin, unique, partial) in index_names {
        let mut statement = connection
            .prepare(&format!(
                "SELECT name FROM pragma_index_info('{name}') ORDER BY seqno"
            ))
            .expect("index_info");
        let columns: Vec<String> = statement
            .query_map([], |row| row.get::<_, Option<String>>(0))
            .expect("rows")
            .map(|row| row.expect("column").unwrap_or_default())
            .collect();
        lines.push(format!(
            "idx|{origin}|{unique}|{partial}|{}",
            columns.join(",")
        ));
    }
    let mut statement = connection
        .prepare("SELECT type, name, sql FROM sqlite_master WHERE tbl_name = ?1 AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY name")
        .expect("master");
    let objects = statement
        .query_map([table], |row| {
            Ok(format!(
                "{}|{}|{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?.replace('"', "")
            ))
        })
        .expect("rows");
    for object in objects {
        lines.push(object.expect("object"));
    }
    lines.sort();
    lines
}

fn table_counts(connection: &rusqlite::Connection) -> Vec<(String, i64)> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .expect("tables");
    let names: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .expect("rows")
        .map(|row| row.expect("name"))
        .collect();
    names
        .into_iter()
        .map(|name| {
            let rows = count(connection, &format!("SELECT count(*) FROM \"{name}\""));
            (name, rows)
        })
        .collect()
}

#[test]
fn upgrade_keeps_every_row_and_records_the_v10_ledger_row() {
    let temp = tempfile::tempdir().expect("temp");
    let paths = paths_in(&temp);
    let lease = MetadataRootLease::acquire(&paths).expect("lease");
    let before = {
        let connection = populated_v9(&paths);
        table_counts(&connection)
    };

    let storage = Storage::open(paths.clone(), &lease).expect("v10 storage");
    let (after, version, name, checksum, violations) = storage
        .read(|connection| {
            let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            let (name, checksum): (String, String) = connection.query_row(
                "SELECT name, checksum FROM schema_migrations WHERE version = 10",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let violations = connection
                .prepare("PRAGMA foreign_key_check")?
                .query_map([], |_| Ok(()))?
                .count();
            Ok((
                table_counts(connection),
                version,
                name,
                checksum,
                violations,
            ))
        })
        .expect("read");

    assert_eq!(CURRENT_SCHEMA_VERSION, 10);
    assert_eq!(version, 10);
    assert_eq!(name, "0010_p9_portability");
    assert_eq!(
        checksum,
        format!(
            "{:x}",
            Sha256::digest(include_str!("../migrations/0010_p9_portability.sql").as_bytes())
        )
    );
    assert_eq!(violations, 0);
    let mut expected = before;
    for (table, rows) in &mut expected {
        if table == "schema_migrations" {
            *rows += 1;
        }
    }
    assert_eq!(after, expected, "every table keeps every row");
    assert!(after.iter().any(|(t, n)| t == "incident_events" && *n == 1));
    assert!(after
        .iter()
        .any(|(t, n)| t == "camera_snapshots" && *n == 2));
}

#[test]
fn rebuilt_tables_keep_columns_indexes_triggers_and_foreign_keys() {
    let temp = tempfile::tempdir().expect("temp");
    let paths = paths_in(&temp);
    let mut connection = populated_v9(&paths);
    let before: Vec<Vec<String>> = REBUILT.iter().map(|t| shape(&connection, t)).collect();
    assert!(
        before[0]
            .iter()
            .any(|line| line.contains("camera_snapshots_prunable")),
        "the shape probe sees the partial index"
    );
    assert!(
        before[1]
            .iter()
            .any(|line| line.contains("incident_events_append_only_u")),
        "the shape probe sees the triggers"
    );

    apply_through(&mut connection, 10).expect("v10");

    for (index, table) in REBUILT.iter().enumerate() {
        assert_eq!(
            shape(&connection, table),
            before[index],
            "{table} shape changed"
        );
    }
    // The new incident_events references the renamed table by its final name.
    let references: String = connection
        .query_row(
            "SELECT \"table\" FROM pragma_foreign_key_list('incident_events') WHERE \"from\" = 'snapshot_id'",
            [],
            |row| row.get(0),
        )
        .expect("fk");
    assert_eq!(references, "camera_snapshots");
}

#[test]
fn rebuild_succeeds_with_an_evidence_captured_incident_event() {
    let temp = tempfile::tempdir().expect("temp");
    let paths = paths_in(&temp);
    let mut connection = populated_v9(&paths);
    assert_eq!(
        count(
            &connection,
            "SELECT count(*) FROM incident_events WHERE kind = 'evidenceCaptured'"
        ),
        1
    );
    apply_through(&mut connection, 10).expect("the rebuild passes with the RESTRICT foreign key");
    assert_eq!(
        count(
            &connection,
            "SELECT count(*) FROM incident_events WHERE snapshot_id = 'snp-a'"
        ),
        1
    );
    assert_eq!(
        count(&connection, "SELECT count(*) FROM pragma_foreign_key_check"),
        0
    );
    // The append-only triggers were recreated.
    assert!(connection
        .execute("DELETE FROM incident_events", [])
        .is_err());
}

#[test]
fn new_check_values_are_accepted_and_old_rejections_stay() {
    let temp = tempfile::tempdir().expect("temp");
    let paths = paths_in(&temp);
    let mut connection = populated_v9(&paths);
    apply_through(&mut connection, 10).expect("v10");

    // A pinned snapshot may be pruned 'reset' or 'notInBackup', not 'age'.
    for (reason, ok) in [
        ("reset", true),
        ("notInBackup", true),
        ("missingFile", true),
        ("age", false),
    ] {
        seed_manual_snapshot(
            &connection,
            &format!("snp-{reason}"),
            "prn-a",
            &format!("snapshots/{reason}.jpg"),
        );
        let result = connection.execute(
            "UPDATE camera_snapshots SET pinned_at = ?2, pruned_at = ?2, prune_reason = ?3 WHERE id = ?1",
            rusqlite::params![format!("snp-{reason}"), NOW, reason],
        );
        assert_eq!(result.is_ok(), ok, "pinned + pruned {reason}");
    }
    // An unpinned snapshot may be pruned for the new reasons too.
    seed_manual_snapshot(&connection, "snp-plain", "prn-a", "snapshots/plain.jpg");
    connection
        .execute(
            "UPDATE camera_snapshots SET pruned_at = ?1, prune_reason = 'notInBackup' WHERE id = 'snp-plain'",
            [NOW],
        )
        .expect("unpinned notInBackup");
    assert!(connection
        .execute(
            "UPDATE camera_snapshots SET prune_reason = 'bogus', pruned_at = ?1 WHERE id = 'snp-b'",
            [NOW]
        )
        .is_err());

    seed_pending_credential_cleanup(&connection, "cred-reset", "reset");
    assert!(connection
        .execute(
            "INSERT INTO pending_credential_cleanup(credential_ref, reason, created_at) VALUES ('x', 'bogus', ?1)",
            [NOW]
        )
        .is_err());

    for kind in [
        "resetSettings",
        "resetCameraMedia",
        "moveSpool",
        "captureSnapshot",
    ] {
        seed_operation(&connection, &format!("op-{kind}"), kind);
    }
    assert!(connection
        .execute(
            "INSERT INTO operations(id, kind, request_digest, created_at) VALUES ('op-bad', 'bogus', 'd', ?1)",
            [NOW]
        )
        .is_err());
}

#[test]
fn history_indexes_exist_and_are_partial_on_terminal_states() {
    let temp = tempfile::tempdir().expect("temp");
    let paths = paths_in(&temp);
    let mut connection = populated_v9(&paths);
    apply_through(&mut connection, 10).expect("v10");
    for name in ["jobs_history", "jobs_history_printer"] {
        let sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name = ?1",
                [name],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| panic!("{name} exists"));
        assert!(
            sql.contains("outcomeUnknown"),
            "{name} is partial on the terminal states"
        );
    }
}

#[test]
fn a_v11_database_is_refused() {
    let temp = tempfile::tempdir().expect("temp");
    let paths = paths_in(&temp);
    let lease = MetadataRootLease::acquire(&paths).expect("lease");
    drop(Storage::open(paths.clone(), &lease).expect("fresh v10"));
    rusqlite::Connection::open(paths.database())
        .expect("raw")
        .execute_batch("PRAGMA user_version = 11")
        .expect("bump");
    match Storage::open(paths.clone(), &lease) {
        Err(StorageError::UnsupportedSchemaVersion) => {}
        Err(other) => panic!("wrong refusal: {other:?}"),
        Ok(_) => panic!("a v11 database must be refused"),
    }
}
