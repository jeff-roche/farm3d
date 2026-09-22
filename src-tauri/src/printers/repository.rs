use std::sync::Arc;

use rusqlite::{params, OptionalExtension};

use crate::persistence::{RepositoryError, Storage, StorageError};

use super::host_identity::canonical_host_identity;
use super::lifecycle::{evaluate, LifecycleAction};
use super::{StartSafety, StoredPrinter};
use crate::connections::ConnectionConfig;

/// Shared column list for every `SELECT ... FROM printers` — keeps the four
/// P2 columns (`location`, `start_safety`, `archived_at`, `host_identity`)
/// in lockstep with `decode` across `list`/`get`/`update`/`delete`/
/// `set_connection` instead of four separately hand-maintained strings.
const PRINTER_COLUMNS: &str = "id, revision, name, catalog_vendor, catalog_model, catalog_variant, catalog_model_id, catalog_printer_variant, notes, overrides_json, last_known_good_json, connection_json, location, start_safety, archived_at, host_identity, created_at, updated_at";

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
            let mut statement = connection.prepare(&format!(
                "SELECT {PRINTER_COLUMNS} FROM printers ORDER BY CAST(id AS BLOB)"
            ))?;
            let rows = statement.query_map([], decode)?.collect();
            rows
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<StoredPrinter>, StorageError> {
        self.storage.read(|connection| {
            connection
                .query_row(
                    &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
                    [id],
                    decode,
                )
                .optional()
        })
    }

    /// `printer_lifecycle_eligibility`'s read path: loads the Printer and
    /// evaluates its lifecycle eligibility inside the same read transaction,
    /// so a caller never observes a state in between.
    pub fn lifecycle_eligibility(
        &self,
        id: &str,
    ) -> Result<Option<super::lifecycle::LifecycleEligibility>, StorageError> {
        self.storage
            .read_transaction(|transaction| {
                let printer = transaction
                    .query_row(
                        &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
                        [id],
                        decode,
                    )
                    .optional()?;
                Ok(match printer {
                    None => Ok(None),
                    Some(printer) => evaluate(&printer, transaction).map(Some),
                })
            })
            .and_then(|inner| inner)
    }

    /// The active (non-archived) Printer, if any, currently holding `identity`
    /// — the repository-side half of D3's duplicate-host enforcement.
    /// `excluding` is the printer being written, so it never conflicts with
    /// its own not-yet-committed row.
    pub fn find_active_by_host_identity(
        &self,
        identity: &str,
        excluding: Option<&str>,
    ) -> Result<Option<String>, StorageError> {
        self.storage
            .read(|connection| active_printer_with_identity(connection, identity, excluding))
    }

    pub fn create(&self, mut printer: StoredPrinter) -> Result<StoredPrinter, RepositoryError> {
        validate_id(&printer.id).map_err(|_| RepositoryError::Validation { field_path: "id" })?;
        let now = crate::printers::now_rfc3339();
        printer.revision = 1;
        printer.created_at = now.clone();
        printer.updated_at = now;
        self.storage
            .write(|transaction| {
                precheck_duplicate_host(transaction, &printer)?;
                insert(transaction, &printer)?;
                Ok(printer.clone())
            })
            .map_err(duplicate_host_or_storage)
    }

    /// `create`'s sibling for the credential-provisioning path
    /// (`create_printer_with`): inserts the Printer and, in the SAME
    /// transaction, deletes `provisional_reference`'s
    /// `pending_credential_cleanup` row — mirroring how `set_connection`
    /// removes a provisional row on a successful commit. On failure the
    /// provisional row is left in place (its secret is orphaned, and the
    /// caller retries cleanup for it) rather than deleted here, since the
    /// insert itself rolled back.
    pub fn create_in(
        &self,
        mut printer: StoredPrinter,
        provisional_reference: Option<&str>,
    ) -> Result<StoredPrinter, RepositoryError> {
        validate_id(&printer.id).map_err(|_| RepositoryError::Validation { field_path: "id" })?;
        let now = crate::printers::now_rfc3339();
        printer.revision = 1;
        printer.created_at = now.clone();
        printer.updated_at = now;
        self.storage
            .write(|transaction| {
                precheck_duplicate_host(transaction, &printer)?;
                insert(transaction, &printer)?;
                if let Some(reference) = provisional_reference {
                    transaction.execute(
                        "DELETE FROM pending_credential_cleanup WHERE credential_ref=?1",
                        [reference],
                    )?;
                }
                Ok(printer.clone())
            })
            .map_err(duplicate_host_or_storage)
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
                    &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
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
            precheck_duplicate_host(transaction, &printer)?;
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
        let mut blocked: Option<Vec<super::lifecycle::LifecycleBlocker>> = None;
        let result = self.storage.write(|transaction| {
            let printer = transaction
                .query_row(
                    &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
                    [id],
                    decode,
                )
                .optional()?
                .ok_or(StorageError::OperationFailed)?;
            if printer.revision != expected_revision {
                return Err(StorageError::OperationFailed);
            }
            let eligibility = evaluate(&printer, transaction)?;
            if !eligibility.can_delete {
                blocked = Some(blockers_for(eligibility, LifecycleAction::Delete));
                return Err(StorageError::OperationFailed);
            }
            transaction.execute("DELETE FROM printers WHERE id = ?1", [id])?;
            if let Some(reference) = printer
                .connection
                .as_ref()
                .and_then(|connection| connection.credential_ref.as_deref())
            {
                enqueue_credential_cleanup(transaction, reference, Some(id), "printer_deleted")?;
            }
            Ok(printer)
        });
        if let Some(blockers) = blocked {
            return Err(RepositoryError::LifecycleBlocked(blockers));
        }
        result.map_err(|error| classify_entity_write(error, self, id, expected_revision))
    }

    /// D6: moves a Printer into the archived state. Excluded from
    /// supervision and the Monitor's default view once archived, but keeps
    /// its Connection and data. Blocked when the Printer is already
    /// archived.
    pub fn archive(&self, id: &str, expected_revision: i64) -> Result<StoredPrinter, RepositoryError> {
        self.transition(id, expected_revision, LifecycleAction::Archive, |printer| {
            printer.archived_at = Some(crate::printers::now_rfc3339());
        })
    }

    /// D6: moves an archived Printer back to active. Goes through the same
    /// duplicate-host-identity precheck as any other write (`update`'s), so
    /// a host another active Printer has since claimed surfaces as
    /// `RepositoryError::DuplicateHost`, not a silent takeover. Blocked when
    /// the Printer isn't archived.
    pub fn unarchive(
        &self,
        id: &str,
        expected_revision: i64,
    ) -> Result<StoredPrinter, RepositoryError> {
        self.transition(
            id,
            expected_revision,
            LifecycleAction::Unarchive,
            |printer| {
                printer.archived_at = None;
            },
        )
    }

    /// Shared machinery for `archive`/`unarchive`: optimistic-concurrency
    /// load, a `lifecycle::evaluate` gate for `action` inside the same
    /// transaction as the write, then `mutate` + the usual duplicate-host
    /// precheck and replace.
    fn transition(
        &self,
        id: &str,
        expected_revision: i64,
        action: LifecycleAction,
        mutate: impl FnOnce(&mut StoredPrinter),
    ) -> Result<StoredPrinter, RepositoryError> {
        if expected_revision <= 0 {
            return Err(RepositoryError::Validation {
                field_path: "expectedRevision",
            });
        }
        let mut blocked: Option<Vec<super::lifecycle::LifecycleBlocker>> = None;
        let result = self.storage.write(|transaction| {
            let mut printer = transaction
                .query_row(
                    &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
                    [id],
                    decode,
                )
                .optional()?
                .ok_or(StorageError::OperationFailed)?;
            if printer.revision != expected_revision {
                return Err(StorageError::OperationFailed);
            }
            let eligibility = evaluate(&printer, transaction)?;
            let eligible = match action {
                LifecycleAction::Archive => eligibility.can_archive,
                LifecycleAction::Unarchive => eligibility.can_unarchive,
                LifecycleAction::Delete => eligibility.can_delete,
            };
            if !eligible {
                blocked = Some(blockers_for(eligibility, action));
                return Err(StorageError::OperationFailed);
            }
            mutate(&mut printer);
            printer.revision += 1;
            printer.updated_at = crate::printers::now_rfc3339();
            precheck_duplicate_host(transaction, &printer)?;
            replace(transaction, &printer)?;
            Ok(printer)
        });
        if let Some(blockers) = blocked {
            return Err(RepositoryError::LifecycleBlocked(blockers));
        }
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
            let mut printer = transaction
                .query_row(
                    &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
                    [id],
                    decode,
                )
                .optional()?
                .ok_or(StorageError::OperationFailed)?;
            if printer.revision != expected_revision || expected_revision <= 0 {
                return Err(StorageError::OperationFailed);
            }
            let old_reference = printer
                .connection
                .as_ref()
                .and_then(|value| value.credential_ref.clone());
            let new_reference = connection
                .as_ref()
                .and_then(|value| value.credential_ref.clone());
            printer.connection = connection;
            printer.revision += 1;
            printer.updated_at = crate::printers::now_rfc3339();
            precheck_duplicate_host(transaction, &printer)?;
            replace(transaction, &printer)?;
            if let Some(reference) = provisional_reference {
                transaction.execute(
                    "DELETE FROM pending_credential_cleanup WHERE credential_ref=?1",
                    [reference],
                )?;
            }
            if old_reference != new_reference {
                if let Some(reference) = old_reference {
                    enqueue_credential_cleanup(transaction, &reference, Some(id), removed_reason)?;
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
    if let StorageError::DuplicateHost(conflicting_printer_id) = error {
        return RepositoryError::DuplicateHost {
            conflicting_printer_id,
        };
    }
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

/// `create`'s error path has no expected revision to classify a conflict
/// against, so it only needs to single out `DuplicateHost` before falling
/// back to a plain storage error.
fn duplicate_host_or_storage(error: StorageError) -> RepositoryError {
    match error {
        StorageError::DuplicateHost(conflicting_printer_id) => RepositoryError::DuplicateHost {
            conflicting_printer_id,
        },
        other => RepositoryError::Storage(other),
    }
}

/// The blockers relevant to `action` alone — a caller attempting one action
/// (e.g. delete) shouldn't be told about blockers for a different one (e.g.
/// unarchive) that happens to also be blocked right now.
fn blockers_for(
    eligibility: super::lifecycle::LifecycleEligibility,
    action: LifecycleAction,
) -> Vec<super::lifecycle::LifecycleBlocker> {
    eligibility
        .blockers
        .into_iter()
        .filter(|blocker| blocker.action == action)
        .collect()
}

/// D3: no two non-archived Printers may share a host identity. Checked
/// inside the write transaction before every insert/replace; the partial
/// unique index created by migration 0003 is the backstop (see
/// `is_host_identity_violation`/`map_write_error`) for anything that skips
/// this precheck.
fn precheck_duplicate_host(
    transaction: &rusqlite::Transaction<'_>,
    printer: &StoredPrinter,
) -> Result<(), StorageError> {
    if printer.archived_at.is_some() {
        return Ok(());
    }
    let Some(identity) = connection_host_identity(printer) else {
        return Ok(());
    };
    if let Some(conflicting_id) =
        active_printer_with_identity(transaction, &identity, Some(&printer.id))?
    {
        return Err(StorageError::DuplicateHost(conflicting_id));
    }
    Ok(())
}

fn connection_host_identity(printer: &StoredPrinter) -> Option<String> {
    printer
        .connection
        .as_ref()
        .and_then(|connection| canonical_host_identity(&connection.host, connection.port))
}

fn active_printer_with_identity(
    connection: &rusqlite::Connection,
    identity: &str,
    excluding: Option<&str>,
) -> rusqlite::Result<Option<String>> {
    connection
        .query_row(
            "SELECT id FROM printers
             WHERE host_identity = ?1 AND archived_at IS NULL AND (?2 IS NULL OR id != ?2)
             LIMIT 1",
            params![identity, excluding],
            |row| row.get::<_, String>(0),
        )
        .optional()
}

/// Whether `error` is the partial unique index (`printers_active_host_identity`,
/// migration 0003) rejecting a write — SQLite's own message for it names the
/// column, not the (partial) index, so matching on `host_identity` is what
/// distinguishes it from any other constraint failure (e.g. the `id` primary
/// key, or the `location`/`start_safety` CHECK constraints).
fn is_host_identity_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, Some(message))
            if code.code == rusqlite::ErrorCode::ConstraintViolation
                && message.contains("host_identity")
    )
}

/// Backstop for `precheck_duplicate_host`: if a write still hits the
/// partial unique index (e.g. a caller that skips the precheck, such as
/// `replace_all`'s import path), look up who holds the identity in the same
/// transaction rather than surface a bare constraint error. Per ruling R1,
/// this reuses `StorageError::DuplicateHost` rather than adding a separate
/// constraint-error variant.
fn map_write_error(
    transaction: &rusqlite::Transaction<'_>,
    error: rusqlite::Error,
    identity: Option<&str>,
    excluding_id: &str,
) -> StorageError {
    if is_host_identity_violation(&error) {
        if let Some(identity) = identity {
            let conflicting_id =
                active_printer_with_identity(transaction, identity, Some(excluding_id))
                    .ok()
                    .flatten()
                    .unwrap_or_default();
            return StorageError::DuplicateHost(conflicting_id);
        }
    }
    StorageError::from(error)
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

/// Column order matches `PRINTER_COLUMNS`. `host_identity` (index 15) is
/// selected for consistency with `insert`/`replace` but never decoded onto
/// `StoredPrinter` — it's derived from `connection` on every write, never a
/// field callers set directly.
fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredPrinter> {
    let overrides: String = row.get(9)?;
    let last_known_good: Option<String> = row.get(10)?;
    let connection: Option<String> = row.get(11)?;
    let start_safety: String = row.get(13)?;
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
        location: row.get(12)?,
        start_safety: decode_start_safety(&start_safety).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                13,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        archived_at: row.get(14)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

/// `start_safety` round-trips through `StoredPrinter`/`StartSafety`'s own
/// serde camelCase mapping rather than a hand-maintained string match, so
/// the DB text and the wire representation can never drift apart.
fn decode_start_safety(text: &str) -> Result<StartSafety, serde_json::Error> {
    serde_json::from_value(serde_json::Value::String(text.to_string()))
}

fn encode_start_safety(value: StartSafety) -> String {
    match serde_json::to_value(value).expect("StartSafety always serializes") {
        serde_json::Value::String(text) => text,
        other => unreachable!("StartSafety serializes to a string, got {other:?}"),
    }
}

struct PrinterColumnValues {
    overrides: String,
    last_known_good: Option<String>,
    connection: Option<String>,
    host_identity: Option<String>,
    start_safety: String,
}

fn values(printer: &StoredPrinter) -> Result<PrinterColumnValues, StorageError> {
    Ok(PrinterColumnValues {
        overrides: serde_json::to_string(&printer.overrides)
            .map_err(|_| StorageError::OperationFailed)?,
        last_known_good: printer
            .last_known_good
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| StorageError::OperationFailed)?,
        connection: printer
            .connection
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| StorageError::OperationFailed)?,
        host_identity: connection_host_identity(printer),
        start_safety: encode_start_safety(printer.start_safety),
    })
}

fn insert(
    transaction: &rusqlite::Transaction<'_>,
    printer: &StoredPrinter,
) -> Result<(), StorageError> {
    let columns = values(printer)?;
    let result = transaction.execute(
        "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model, catalog_variant, catalog_model_id, catalog_printer_variant, notes, overrides_json, last_known_good_json, connection_json, location, start_safety, archived_at, host_identity, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            printer.id,
            printer.revision,
            printer.name,
            printer.catalog_ref.vendor,
            printer.catalog_ref.model,
            printer.catalog_ref.variant,
            printer.catalog_ref.model_id,
            printer.catalog_ref.printer_variant,
            printer.notes,
            columns.overrides,
            columns.last_known_good,
            columns.connection,
            printer.location,
            columns.start_safety,
            printer.archived_at,
            columns.host_identity,
            printer.created_at,
            printer.updated_at,
        ],
    );
    result.map(|_| ()).map_err(|error| {
        map_write_error(
            transaction,
            error,
            columns.host_identity.as_deref(),
            &printer.id,
        )
    })
}

fn replace(
    transaction: &rusqlite::Transaction<'_>,
    printer: &StoredPrinter,
) -> Result<(), StorageError> {
    let columns = values(printer)?;
    let result = transaction.execute(
        "UPDATE printers SET revision=?2, name=?3, catalog_vendor=?4, catalog_model=?5, catalog_variant=?6, catalog_model_id=?7, catalog_printer_variant=?8, notes=?9, overrides_json=?10, last_known_good_json=?11, connection_json=?12, location=?13, start_safety=?14, archived_at=?15, host_identity=?16, updated_at=?17 WHERE id=?1",
        params![
            printer.id,
            printer.revision,
            printer.name,
            printer.catalog_ref.vendor,
            printer.catalog_ref.model,
            printer.catalog_ref.variant,
            printer.catalog_ref.model_id,
            printer.catalog_ref.printer_variant,
            printer.notes,
            columns.overrides,
            columns.last_known_good,
            columns.connection,
            printer.location,
            columns.start_safety,
            printer.archived_at,
            columns.host_identity,
            printer.updated_at,
        ],
    );
    result.map(|_| ()).map_err(|error| {
        map_write_error(
            transaction,
            error,
            columns.host_identity.as_deref(),
            &printer.id,
        )
    })
}
