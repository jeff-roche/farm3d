use std::sync::Arc;

use rusqlite::{params, OptionalExtension};

use crate::persistence::{RepositoryError, Storage, StorageError};

use super::StoredPrinter;
use crate::connections::ConnectionConfig;

pub struct PrinterRepository {
    storage: Arc<Storage>,
}

impl PrinterRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn generate_id() -> String {
        format!("prn-{}", uuid::Uuid::new_v4())
    }

    pub fn enqueue_credential_cleanup(
        &self,
        reference: &str,
        printer_id: Option<&str>,
        reason: &str,
    ) -> Result<(), StorageError> {
        self.storage.write(|transaction| {
            enqueue_credential_cleanup(transaction, reference, printer_id, reason)
        })
    }

    pub fn list(&self) -> Result<Vec<StoredPrinter>, StorageError> {
        self.storage.read(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, revision, name, catalog_vendor, catalog_model, catalog_variant, catalog_model_id, catalog_printer_variant, notes, overrides_json, last_known_good_json, connection_json, created_at, updated_at FROM printers ORDER BY CAST(id AS BLOB)",
            )?;
            let rows = statement.query_map([], decode)?.collect();
            rows
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<StoredPrinter>, StorageError> {
        self.storage.read(|connection| {
            connection
                .query_row(
                    "SELECT id, revision, name, catalog_vendor, catalog_model, catalog_variant, catalog_model_id, catalog_printer_variant, notes, overrides_json, last_known_good_json, connection_json, created_at, updated_at FROM printers WHERE id = ?1",
                    [id],
                    decode,
                )
                .optional()
        })
    }

    pub fn create(&self, mut printer: StoredPrinter) -> Result<StoredPrinter, RepositoryError> {
        validate_id(&printer.id).map_err(|_| RepositoryError::Validation { field_path: "id" })?;
        let now = crate::printers::now_rfc3339();
        printer.revision = 1;
        printer.created_at = now.clone();
        printer.updated_at = now;
        self.storage
            .write(|transaction| {
                insert(transaction, &printer)?;
                Ok(printer.clone())
            })
            .map_err(RepositoryError::Storage)
    }

    pub fn update(
        &self,
        id: &str,
        expected_revision: i64,
        mutate: impl FnOnce(&mut StoredPrinter),
    ) -> Result<StoredPrinter, RepositoryError> {
        if expected_revision <= 0 {
            return Err(RepositoryError::Validation {
                field_path: "expectedRevision",
            });
        }
        let result = self.storage.write(|transaction| {
            let mut printer = transaction
                .query_row(
                    "SELECT id, revision, name, catalog_vendor, catalog_model, catalog_variant, catalog_model_id, catalog_printer_variant, notes, overrides_json, last_known_good_json, connection_json, created_at, updated_at FROM printers WHERE id = ?1",
                    [id],
                    decode,
                )
                .optional()?
                .ok_or(StorageError::OperationFailed)?;
            if printer.revision != expected_revision || expected_revision == 0 {
                return Err(StorageError::OperationFailed);
            }
            mutate(&mut printer);
            printer.revision += 1;
            printer.updated_at = crate::printers::now_rfc3339();
            replace(transaction, &printer)?;
            Ok(printer)
        });
        result.map_err(|error| classify_entity_write(error, self, id, expected_revision))
    }

    pub fn delete(
        &self,
        id: &str,
        expected_revision: i64,
    ) -> Result<StoredPrinter, RepositoryError> {
        if expected_revision <= 0 {
            return Err(RepositoryError::Validation {
                field_path: "expectedRevision",
            });
        }
        let result = self.storage.write(|transaction| {
            let printer = transaction
                .query_row(
                    "SELECT id, revision, name, catalog_vendor, catalog_model, catalog_variant, catalog_model_id, catalog_printer_variant, notes, overrides_json, last_known_good_json, connection_json, created_at, updated_at FROM printers WHERE id = ?1",
                    [id],
                    decode,
                )
                .optional()?
                .ok_or(StorageError::OperationFailed)?;
            if printer.revision != expected_revision {
                return Err(StorageError::OperationFailed);
            }
            transaction.execute("DELETE FROM printers WHERE id = ?1", [id])?;
            if let Some(reference) = printer.connection.as_ref().and_then(|connection| connection.credential_ref.as_deref()) {
                enqueue_credential_cleanup(
                    transaction,
                    reference,
                    Some(id),
                    "printer_deleted",
                )?;
            }
            Ok(printer)
        });
        result.map_err(|error| classify_entity_write(error, self, id, expected_revision))
    }

    pub fn set_connection(
        &self,
        id: &str,
        expected_revision: i64,
        connection: Option<ConnectionConfig>,
        provisional_reference: Option<&str>,
        removed_reason: &str,
    ) -> Result<StoredPrinter, RepositoryError> {
        if expected_revision <= 0 {
            return Err(RepositoryError::Validation {
                field_path: "expectedRevision",
            });
        }
        let result = self.storage.write(|transaction| {
            let mut printer = transaction.query_row(
                "SELECT id, revision, name, catalog_vendor, catalog_model, catalog_variant, catalog_model_id, catalog_printer_variant, notes, overrides_json, last_known_good_json, connection_json, created_at, updated_at FROM printers WHERE id = ?1",
                [id], decode,
            ).optional()?.ok_or(StorageError::OperationFailed)?;
            if printer.revision != expected_revision || expected_revision <= 0 { return Err(StorageError::OperationFailed); }
            let old_reference = printer.connection.as_ref().and_then(|value| value.credential_ref.clone());
            let new_reference = connection.as_ref().and_then(|value| value.credential_ref.clone());
            printer.connection = connection;
            printer.revision += 1;
            printer.updated_at = crate::printers::now_rfc3339();
            replace(transaction, &printer)?;
            if let Some(reference) = provisional_reference {
                transaction.execute("DELETE FROM pending_credential_cleanup WHERE credential_ref=?1", [reference])?;
            }
            if old_reference != new_reference {
                if let Some(reference) = old_reference {
                    enqueue_credential_cleanup(
                        transaction,
                        &reference,
                        Some(id),
                        removed_reason,
                    )?;
                }
            }
            Ok(printer)
        });
        result.map_err(|error| classify_entity_write(error, self, id, expected_revision))
    }

    pub fn replace_all(
        &self,
        expected: &[(String, i64)],
        mut imported: Vec<StoredPrinter>,
    ) -> Result<Vec<StoredPrinter>, RepositoryError> {
        let result = self.storage.write(|transaction| {
            let mut statement = transaction
                .prepare("SELECT id, revision FROM printers ORDER BY CAST(id AS BLOB)")?;
            let current = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            if current != expected {
                return Err(StorageError::OperationFailed);
            }
            let revisions: std::collections::HashMap<_, _> = current.into_iter().collect();
            let old_references = transaction.prepare("SELECT DISTINCT json_extract(connection_json, '$.credentialRef') FROM printers WHERE json_extract(connection_json, '$.credentialRef') IS NOT NULL")?
                .query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
            let imported_references = imported.iter().filter_map(|printer| printer.connection.as_ref()?.credential_ref.clone()).collect::<std::collections::HashSet<_>>();
            transaction.execute("DELETE FROM printers", [])?;
            let now = crate::printers::now_rfc3339();
            for printer in &mut imported {
                validate_id(&printer.id)?;
                printer.revision = revisions
                    .get(&printer.id)
                    .map_or(1, |revision| revision + 1);
                if printer.created_at.is_empty() {
                    printer.created_at = now.clone();
                }
                printer.updated_at = now.clone();
                insert(transaction, printer)?;
            }
            for reference in old_references.difference(&imported_references) {
                enqueue_credential_cleanup(
                    transaction,
                    reference,
                    None,
                    "import_orphan",
                )?;
            }
            imported.sort_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));
            Ok(imported)
        });
        result.map_err(|error| match error {
            StorageError::OperationFailed => RepositoryError::SetConflict {
                expected_count: expected.len(),
                current_count: self.list().map(|rows| rows.len()).unwrap_or(0),
            },
            other => RepositoryError::Storage(other),
        })
    }
}

fn classify_entity_write(
    error: StorageError,
    repository: &PrinterRepository,
    id: &str,
    expected_revision: i64,
) -> RepositoryError {
    if !matches!(error, StorageError::OperationFailed) {
        return RepositoryError::Storage(error);
    }
    match repository.get(id) {
        Ok(None) => RepositoryError::NotFound {
            entity_id: id.to_string(),
        },
        Ok(Some(current)) => RepositoryError::Conflict {
            entity_id: id.to_string(),
            expected_revision,
            current_revision: current.revision,
        },
        Err(storage) => RepositoryError::Storage(storage),
    }
}

fn cleanup_precedence(reason: &str) -> Option<u8> {
    match reason {
        "import_orphan" => Some(0),
        "provisional" => Some(1),
        "replaced" => Some(2),
        "cleared" => Some(3),
        "printer_deleted" => Some(4),
        _ => None,
    }
}

fn enqueue_credential_cleanup(
    transaction: &rusqlite::Transaction<'_>,
    reference: &str,
    printer_id: Option<&str>,
    reason: &str,
) -> Result<(), StorageError> {
    let incoming_precedence = cleanup_precedence(reason).ok_or(StorageError::OperationFailed)?;
    let current = transaction
        .query_row(
            "SELECT reason, printer_id FROM pending_credential_cleanup WHERE credential_ref=?1",
            [reference],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    if let Some((current_reason, current_printer)) = current {
        let current_precedence =
            cleanup_precedence(&current_reason).ok_or(StorageError::OperationFailed)?;
        let incoming_wins = incoming_precedence > current_precedence
            || (incoming_precedence == current_precedence
                && match (printer_id, current_printer.as_deref()) {
                    (Some(_), None) => true,
                    (Some(incoming), Some(current)) => incoming.as_bytes() < current.as_bytes(),
                    (None, _) => false,
                });
        if incoming_wins {
            transaction.execute(
                "UPDATE pending_credential_cleanup SET reason=?2, printer_id=?3 WHERE credential_ref=?1",
                params![reference, reason, printer_id],
            )?;
        }
    } else {
        transaction.execute(
            "INSERT INTO pending_credential_cleanup(credential_ref, printer_id, reason, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![reference, printer_id, reason, crate::printers::now_rfc3339()],
        )?;
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), StorageError> {
    if id.is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
        Err(StorageError::OperationFailed)
    } else {
        Ok(())
    }
}

fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredPrinter> {
    let overrides: String = row.get(9)?;
    let last_known_good: Option<String> = row.get(10)?;
    let connection: Option<String> = row.get(11)?;
    Ok(StoredPrinter {
        id: row.get(0)?,
        revision: row.get(1)?,
        name: row.get(2)?,
        catalog_ref: super::CatalogRef {
            vendor: row.get(3)?,
            model: row.get(4)?,
            variant: row.get(5)?,
            model_id: row.get(6)?,
            printer_variant: row.get(7)?,
        },
        notes: row.get(8)?,
        overrides: serde_json::from_str(&overrides).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        last_known_good: last_known_good
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    10,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        connection: connection
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    11,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn values(
    printer: &StoredPrinter,
) -> Result<(String, Option<String>, Option<String>), StorageError> {
    Ok((
        serde_json::to_string(&printer.overrides).map_err(|_| StorageError::OperationFailed)?,
        printer
            .last_known_good
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| StorageError::OperationFailed)?,
        printer
            .connection
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| StorageError::OperationFailed)?,
    ))
}

fn insert(
    transaction: &rusqlite::Transaction<'_>,
    printer: &StoredPrinter,
) -> Result<(), StorageError> {
    let (overrides, last_known_good, connection) = values(printer)?;
    transaction.execute(
        "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model, catalog_variant, catalog_model_id, catalog_printer_variant, notes, overrides_json, last_known_good_json, connection_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![printer.id, printer.revision, printer.name, printer.catalog_ref.vendor, printer.catalog_ref.model, printer.catalog_ref.variant, printer.catalog_ref.model_id, printer.catalog_ref.printer_variant, printer.notes, overrides, last_known_good, connection, printer.created_at, printer.updated_at],
    )?;
    Ok(())
}

fn replace(
    transaction: &rusqlite::Transaction<'_>,
    printer: &StoredPrinter,
) -> Result<(), StorageError> {
    let (overrides, last_known_good, connection) = values(printer)?;
    transaction.execute(
        "UPDATE printers SET revision=?2, name=?3, catalog_vendor=?4, catalog_model=?5, catalog_variant=?6, catalog_model_id=?7, catalog_printer_variant=?8, notes=?9, overrides_json=?10, last_known_good_json=?11, connection_json=?12, updated_at=?13 WHERE id=?1",
        params![printer.id, printer.revision, printer.name, printer.catalog_ref.vendor, printer.catalog_ref.model, printer.catalog_ref.variant, printer.catalog_ref.model_id, printer.catalog_ref.printer_variant, printer.notes, overrides, last_known_good, connection, printer.updated_at],
    )?;
    Ok(())
}
