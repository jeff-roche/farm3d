use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};

use super::error::StorageError;

pub(crate) const CURRENT_SCHEMA_VERSION: i64 = 1;

const FOUNDATION_NAME: &str = "0001_foundation";
const FOUNDATION_SQL: &str = include_str!("../../migrations/0001_foundation.sql");

pub(crate) fn apply(connection: &mut Connection) -> Result<(), StorageError> {
    apply_foundation_sql(connection, FOUNDATION_SQL, false)
}

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
    if user_version > CURRENT_SCHEMA_VERSION {
        return Err(StorageError::UnsupportedSchemaVersion);
    }

    validate_applied_migrations(&transaction, user_version)?;
    if user_version == CURRENT_SCHEMA_VERSION {
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
            (FOUNDATION_NAME, foundation_checksum()),
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

    let record = connection
        .query_row(
            "SELECT name, checksum FROM schema_migrations WHERE version = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|_| StorageError::MigrationFailed)?;
    let count: i64 = connection
        .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .map_err(|_| StorageError::MigrationFailed)?;

    match (user_version, record) {
        (0, None) if count == 0 => Ok(()),
        (1, Some((name, checksum)))
            if count == 1 && name == FOUNDATION_NAME && checksum == foundation_checksum() =>
        {
            Ok(())
        }
        _ => Err(StorageError::MigrationFailed),
    }
}

pub(crate) fn has_foreign_key_violation(connection: &Connection) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement.query([])?;
    Ok(rows.next()?.is_some())
}

fn foundation_checksum() -> String {
    format!("{:x}", Sha256::digest(FOUNDATION_SQL.as_bytes()))
}
