use rusqlite::{Connection, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};

use super::error::StorageError;

pub const CURRENT_SCHEMA_VERSION: i64 = 7;

/// A migration-specific Rust step, run in the same exclusive transaction
/// right after its SQL. Only 0003 uses this: `host_identity` backfill needs
/// `canonical_host_identity`, which is Rust-owned (D2), not SQL.
type MigrationPostStep = fn(&Transaction<'_>) -> Result<(), StorageError>;

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
    post: Option<MigrationPostStep>,
}

const MIGRATIONS: [Migration; 7] = [
    Migration {
        version: 1,
        name: "0001_foundation",
        sql: include_str!("../../migrations/0001_foundation.sql"),
        post: None,
    },
    Migration {
        version: 2,
        name: "0002_p1_monitor",
        sql: include_str!("../../migrations/0002_p1_monitor.sql"),
        post: None,
    },
    Migration {
        version: 3,
        name: "0003_p2_printer_lifecycle",
        sql: include_str!("../../migrations/0003_p2_printer_lifecycle.sql"),
        post: Some(backfill_host_identity),
    },
    Migration {
        version: 4,
        name: "0004_p3_spools_material_slots",
        sql: include_str!("../../migrations/0004_p3_spools_material_slots.sql"),
        post: Some(backfill_main_slots),
    },
    Migration {
        version: 5,
        name: "0005_p4_library",
        sql: include_str!("../../migrations/0005_p4_library.sql"),
        post: None,
    },
    Migration {
        version: 6,
        name: "0006_p5_slicing",
        sql: include_str!("../../migrations/0006_p5_slicing.sql"),
        post: None,
    },
    Migration {
        version: 7,
        name: "0007_p6_host_operations",
        sql: include_str!("../../migrations/0007_p6_host_operations.sql"),
        post: None,
    },
];

pub(crate) fn apply(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(|error| match StorageError::from(error) {
            StorageError::PersistenceUnavailable => StorageError::PersistenceUnavailable,
            _ => StorageError::MigrationFailed,
        })?;
    let user_version: i64 = transaction
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| StorageError::MigrationFailed)?;
    if user_version > CURRENT_SCHEMA_VERSION {
        return Err(StorageError::UnsupportedSchemaVersion);
    }

    validate_applied_migrations(&transaction, user_version)?;
    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version > user_version)
    {
        apply_migration_step(&transaction, migration)?;
    }
    validate_applied_migrations(&transaction, CURRENT_SCHEMA_VERSION)?;
    if has_foreign_key_violation(&transaction).map_err(|_| StorageError::MigrationFailed)? {
        return Err(StorageError::MigrationFailed);
    }
    transaction
        .commit()
        .map_err(|_| StorageError::MigrationFailed)
}

/// Runs one migration's SQL, its optional Rust post-step, and records it in
/// the ledger — the unit shared by `apply` and the `apply_through*` test
/// helpers below.
fn apply_migration_step(
    transaction: &Transaction<'_>,
    migration: &Migration,
) -> Result<(), StorageError> {
    transaction
        .execute_batch(migration.sql)
        .map_err(|_| StorageError::MigrationFailed)?;
    if let Some(post) = migration.post {
        post(transaction).map_err(|_| StorageError::MigrationFailed)?;
    }
    transaction
        .execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at)
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            (
                migration.version,
                migration.name,
                migration_checksum(migration),
            ),
        )
        .map_err(|_| StorageError::MigrationFailed)?;
    transaction
        .execute_batch(&format!("PRAGMA user_version = {}", migration.version))
        .map_err(|_| StorageError::MigrationFailed)
}

/// Applies migrations up to (and including) `max_version`, adding whichever
/// of them are still missing from `connection`'s ledger. Lets integration
/// tests build a fixture database pinned at an older schema version (e.g. a
/// v2 database, to exercise the v2→v3 upgrade) without hand-maintaining a
/// second copy of the migration SQL.
pub fn apply_through(connection: &mut Connection, max_version: i64) -> Result<(), StorageError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(|error| match StorageError::from(error) {
            StorageError::PersistenceUnavailable => StorageError::PersistenceUnavailable,
            _ => StorageError::MigrationFailed,
        })?;
    let user_version: i64 = transaction
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| StorageError::MigrationFailed)?;
    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version > user_version && migration.version <= max_version)
    {
        apply_migration_step(&transaction, migration)?;
    }
    transaction
        .commit()
        .map_err(|_| StorageError::MigrationFailed)
}

/// Like [`apply_through`], but injects a failure after every migration step
/// up to `max_version` has run — SQL, post-step, and ledger row all
/// executed — and just before the transaction would commit. Lets a test
/// confirm that a crash at that boundary rolls back the whole upgrade,
/// leaving the database exactly as it was.
pub fn apply_through_failing_before_commit(
    connection: &mut Connection,
    max_version: i64,
) -> Result<(), StorageError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(|error| match StorageError::from(error) {
            StorageError::PersistenceUnavailable => StorageError::PersistenceUnavailable,
            _ => StorageError::MigrationFailed,
        })?;
    let user_version: i64 = transaction
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| StorageError::MigrationFailed)?;
    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version > user_version && migration.version <= max_version)
    {
        apply_migration_step(&transaction, migration)?;
    }
    Err(StorageError::MigrationFailed)
}

/// D2/D3 backfill for the 0003 migration: computes `host_identity` for every
/// Printer with a Connection, archiving every Printer but the oldest in each
/// duplicate-identity group first so the partial unique index this migration
/// creates never sees a conflict. See "Migration 0003" in the P2 design
/// spec.
fn backfill_host_identity(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    struct Row {
        id: String,
        host: Option<String>,
        port: Option<i64>,
    }

    let mut statement = transaction.prepare(
        "SELECT id, json_extract(connection_json, '$.host'), json_extract(connection_json, '$.port')
         FROM printers WHERE connection_json IS NOT NULL
         ORDER BY created_at, CAST(id AS BLOB)",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(Row {
                id: row.get(0)?,
                host: row.get(1)?,
                port: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    let identities: Vec<(String, Option<String>)> = rows
        .iter()
        .map(|row| {
            let identity = match (&row.host, row.port) {
                (Some(host), Some(port)) => u16::try_from(port).ok().and_then(|port| {
                    crate::printers::host_identity::canonical_host_identity(host, port)
                }),
                _ => None,
            };
            (row.id.clone(), identity)
        })
        .collect();

    // Group by identity, preserving the created_at/id ordering already
    // established by the query above — the first id seen per identity is
    // the one that keeps it.
    let mut groups: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    for (id, identity) in &identities {
        if let Some(identity) = identity {
            groups
                .entry(identity.as_str())
                .or_default()
                .push(id.as_str());
        }
    }

    let now = crate::printers::now_rfc3339();
    for ids in groups.values().filter(|ids| ids.len() > 1) {
        let kept = ids[0];
        for archived_id in &ids[1..] {
            transaction.execute(
                "UPDATE printers SET archived_at = ?2 WHERE id = ?1",
                rusqlite::params![archived_id, now],
            )?;
            let details = serde_json::json!({
                "keptPrinterId": kept,
                "archivedPrinterId": archived_id,
            })
            .to_string();
            let message = format!(
                "Printer {archived_id} was archived during the P2 upgrade because it shares a host with Printer {kept}."
            );
            transaction.execute(
                "INSERT INTO migration_warnings(id, code, source_name, source_sha256, message, details_json, created_at)
                 VALUES (?1, 'DUPLICATE_HOST_ARCHIVED', ?2, NULL, ?3, ?4, ?5)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    archived_id,
                    message,
                    details,
                    now,
                ],
            )?;
        }
    }

    for (id, identity) in &identities {
        if let Some(identity) = identity {
            transaction.execute(
                "UPDATE printers SET host_identity = ?2 WHERE id = ?1",
                rusqlite::params![id, identity],
            )?;
        }
    }

    Ok(())
}

/// Rust post-step for the 0004 migration (D4/D12): every existing Printer,
/// archived ones included, gets one live `Main` slot at position 0. See
/// "Migration 0004" in the P3 design spec and Step 3 of the task-1 brief.
/// `create_printer`/batch create (Task 5) use the same default layout going
/// forward; this only backfills Printers that predate Material Slots.
fn backfill_main_slots(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    let printer_ids: Vec<String> = {
        let mut statement =
            transaction.prepare("SELECT id FROM printers ORDER BY created_at, CAST(id AS BLOB)")?;
        let ids = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids
    };

    let now = crate::printers::now_rfc3339();
    for printer_id in printer_ids {
        let slot_id = format!("slt-{}", uuid::Uuid::new_v4());
        transaction.execute(
            "INSERT INTO material_slots(id, printer_id, position, name, created_at)
             VALUES (?1, ?2, 0, 'Main', ?3)",
            rusqlite::params![slot_id, printer_id, now],
        )?;
    }

    Ok(())
}

#[cfg(test)]
fn apply_foundation_sql(
    connection: &mut Connection,
    sql: &str,
    fail_after_metadata: bool,
) -> Result<(), StorageError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(|error| match StorageError::from(error) {
            StorageError::PersistenceUnavailable => StorageError::PersistenceUnavailable,
            _ => StorageError::MigrationFailed,
        })?;
    let user_version: i64 = transaction
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| StorageError::MigrationFailed)?;
    if user_version > 1 {
        return Err(StorageError::UnsupportedSchemaVersion);
    }

    if user_version == 1 {
        transaction
            .commit()
            .map_err(|_| StorageError::MigrationFailed)?;
        return Ok(());
    }

    transaction
        .execute_batch(sql)
        .map_err(|_| StorageError::MigrationFailed)?;
    transaction
        .execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at)\n             VALUES (1, ?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            (MIGRATIONS[0].name, migration_checksum(&MIGRATIONS[0])),
        )
        .map_err(|_| StorageError::MigrationFailed)?;
    transaction
        .execute_batch("PRAGMA user_version = 1")
        .map_err(|_| StorageError::MigrationFailed)?;
    if fail_after_metadata {
        return Err(StorageError::MigrationFailed);
    }
    if has_foreign_key_violation(&transaction).map_err(|_| StorageError::MigrationFailed)? {
        return Err(StorageError::MigrationFailed);
    }
    transaction
        .commit()
        .map_err(|_| StorageError::MigrationFailed)?;
    Ok(())
}

#[cfg(test)]
pub(super) fn apply_sql_for_test(
    connection: &mut Connection,
    sql: &str,
) -> Result<(), StorageError> {
    apply_foundation_sql(connection, sql, false)
}

#[cfg(test)]
pub(super) fn apply_sql_failing_after_metadata_for_test(
    connection: &mut Connection,
    sql: &str,
) -> Result<(), StorageError> {
    apply_foundation_sql(connection, sql, true)
}

pub(crate) fn validate(connection: &Connection) -> Result<(), StorageError> {
    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| StorageError::InvalidSnapshot)?;
    if user_version > CURRENT_SCHEMA_VERSION {
        return Err(StorageError::UnsupportedSchemaVersion);
    }
    if user_version != CURRENT_SCHEMA_VERSION {
        return Err(StorageError::InvalidSnapshot);
    }
    validate_applied_migrations(connection, user_version).map_err(|error| match error {
        StorageError::UnsupportedSchemaVersion => StorageError::UnsupportedSchemaVersion,
        _ => StorageError::InvalidSnapshot,
    })
}

fn validate_applied_migrations(
    connection: &Connection,
    user_version: i64,
) -> Result<(), StorageError> {
    let table_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'schema_migrations')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| StorageError::MigrationFailed)?;
    if !table_exists {
        return if user_version == 0 {
            Ok(())
        } else {
            Err(StorageError::MigrationFailed)
        };
    }

    let mut statement = connection
        .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")
        .map_err(|_| StorageError::MigrationFailed)?;
    let applied = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| StorageError::MigrationFailed)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| StorageError::MigrationFailed)?;
    let expected = MIGRATIONS
        .iter()
        .filter(|migration| migration.version <= user_version)
        .map(|migration| {
            (
                migration.version,
                migration.name.to_string(),
                migration_checksum(migration),
            )
        })
        .collect::<Vec<_>>();

    if applied == expected {
        Ok(())
    } else {
        Err(StorageError::MigrationFailed)
    }
}

pub(crate) fn has_foreign_key_violation(connection: &Connection) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement.query([])?;
    Ok(rows.next()?.is_some())
}

fn migration_checksum(migration: &Migration) -> String {
    format!("{:x}", Sha256::digest(migration.sql.as_bytes()))
}
