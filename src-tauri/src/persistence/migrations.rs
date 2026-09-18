use rusqlite::{Connection, TransactionBehavior};
use sha2::{Digest, Sha256};

use super::error::StorageError;

pub(crate) const CURRENT_SCHEMA_VERSION: i64 = 2;

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: [Migration; 2] = [
    Migration {
        version: 1,
        name: "0001_foundation",
        sql: include_str!("../../migrations/0001_foundation.sql"),
    },
    Migration {
        version: 2,
        name: "0002_p1_monitor",
        sql: include_str!("../../migrations/0002_p1_monitor.sql"),
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
        transaction
            .execute_batch(migration.sql)
            .map_err(|_| StorageError::MigrationFailed)?;
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
            .map_err(|_| StorageError::MigrationFailed)?;
    }
    validate_applied_migrations(&transaction, CURRENT_SCHEMA_VERSION)?;
    if has_foreign_key_violation(&transaction).map_err(|_| StorageError::MigrationFailed)? {
        return Err(StorageError::MigrationFailed);
    }
    transaction
        .commit()
        .map_err(|_| StorageError::MigrationFailed)
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
