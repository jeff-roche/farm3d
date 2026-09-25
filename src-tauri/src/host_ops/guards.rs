//! D7 guards: while a Printer has an unresolved Host Operation
//! (`dispatching`, `uncertain`, or `reconciling`), no mutation may orphan
//! it. Every check here runs inside the caller's own write transaction, and
//! pre-checks the rows rather than relying on a constraint to fire: the
//! partial unique index and `ON DELETE RESTRICT` are backstops that would
//! only surface as an opaque `StorageError::Database`.
//!
//! - [`HostOperationBlockers`]: the Printer lifecycle blocker source
//!   (archive, delete).
//! - [`UnresolvedHostOperationBlocksRevisionDeletion`]: the Slice Revision
//!   deletion blocker.
//! - [`check_connection_change`]: `set_printer_connection` and
//!   `clear_printer_connection` (`CONNECTION_IN_USE`).
//! - [`check_import`]: `import_printers` (`HOST_OPERATION_PENDING`).
//!
//! `succeeded`, `failed`, and `abandoned` rows never block (D8: abandoning
//! releases every guard).

use rusqlite::{Connection, Transaction};

use crate::connections::ConnectionConfig;
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::lifecycle::{
    LifecycleAction, LifecycleBlocker, LifecycleBlockerCode, LifecycleBlockerSource,
};
use crate::printers::StoredPrinter;
use crate::slicing::blockers::SliceRevisionDeletionBlocker;

use super::repository;

/// A [`repository`] error inside a `StorageError`-typed transaction (the
/// blocker traits, `PrinterRepository::delete`): its reads and deletes only
/// ever fail with a storage error.
pub(crate) fn storage_error(error: RepositoryError) -> StorageError {
    match error {
        RepositoryError::Storage(error) => error,
        _ => StorageError::Database,
    }
}

/// Blocks **Archive** and **Delete** while the Printer has an unresolved
/// Host Operation.
pub struct HostOperationBlockers;

impl LifecycleBlockerSource for HostOperationBlockers {
    fn blockers(
        &self,
        printer: &StoredPrinter,
        tx: &Transaction<'_>,
    ) -> Result<Vec<LifecycleBlocker>, StorageError> {
        if !repository::has_unresolved(tx, &printer.id).map_err(storage_error)? {
            return Ok(Vec::new());
        }
        Ok(vec![
            LifecycleBlocker {
                action: LifecycleAction::Archive,
                code: LifecycleBlockerCode::HostOperationUnresolved,
                message: "Finish or abandon the pending printer operation before archiving."
                    .to_string(),
            },
            LifecycleBlocker {
                action: LifecycleAction::Delete,
                code: LifecycleBlockerCode::HostOperationUnresolved,
                message: "Finish or abandon the pending printer operation before deleting."
                    .to_string(),
            },
        ])
    }
}

/// Blocks deleting a Slice Revision that an unresolved `upload` or `start`
/// row references.
pub struct UnresolvedHostOperationBlocksRevisionDeletion;

impl SliceRevisionDeletionBlocker for UnresolvedHostOperationBlocksRevisionDeletion {
    fn blockers(
        &self,
        slice_revision_id: &str,
        tx: &Transaction<'_>,
    ) -> Result<Vec<LifecycleBlocker>, StorageError> {
        let referenced: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM host_operations
                 WHERE slice_revision_id = ?1
                   AND kind IN ('upload','start')
                   AND state IN ('dispatching','uncertain','reconciling')
             )",
            [slice_revision_id],
            |row| row.get(0),
        )?;
        Ok(if referenced {
            vec![LifecycleBlocker {
                action: LifecycleAction::Delete,
                code: LifecycleBlockerCode::HostOperationUnresolved,
                message: "A printer operation using this Slice Revision is still pending. \
                          Finish or abandon it first."
                    .to_string(),
            }]
        } else {
            Vec::new()
        })
    }
}

/// Whether replacing Connection `old` with `new` would take the endpoint or
/// credential out from under a pending operation: a change to `kind`,
/// `host`, `port`, or `use_tls`, clearing the Connection, or clearing the
/// credential reference. Replacing the credential reference with another
/// one, endpoint unchanged, is allowed (owner decision 4).
pub fn connection_change_orphans(
    old: Option<&ConnectionConfig>,
    new: Option<&ConnectionConfig>,
) -> bool {
    match (old, new) {
        (None, None) => false,
        (Some(_), None) | (None, Some(_)) => true,
        (Some(old), Some(new)) => {
            old.kind != new.kind
                || old.host != new.host
                || old.port != new.port
                || old.use_tls != new.use_tls
                || (old.credential_ref.is_some() && new.credential_ref.is_none())
        }
    }
}

/// D7 `CONNECTION_IN_USE`: refuses a Connection change that
/// [`connection_change_orphans`] while `printer_id` has an unresolved row.
pub fn check_connection_change(
    tx: &Transaction<'_>,
    printer_id: &str,
    old: Option<&ConnectionConfig>,
    new: Option<&ConnectionConfig>,
) -> Result<(), RepositoryError> {
    if !connection_change_orphans(old, new) {
        return Ok(());
    }
    match repository::list_unresolved(tx, Some(printer_id))?
        .into_iter()
        .next()
    {
        Some(pending) => Err(RepositoryError::ConnectionInUse {
            printer_id: printer_id.to_string(),
            host_operation_id: pending.id,
        }),
        None => Ok(()),
    }
}

/// D7 `HOST_OPERATION_PENDING` for `import_printers`: an import replaces
/// every Printer, so any unresolved row anywhere rejects the whole import.
pub fn check_import(connection: &Connection) -> Result<(), RepositoryError> {
    let pending = repository::list_unresolved(connection, None)?;
    if pending.is_empty() {
        return Ok(());
    }
    let mut printer_ids: Vec<String> = pending.iter().map(|row| row.printer_id.clone()).collect();
    printer_ids.sort();
    printer_ids.dedup();
    Err(RepositoryError::HostOperationsPending {
        printer_ids,
        host_operation_ids: pending.into_iter().map(|row| row.id).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::capabilities::InconclusiveReason;
    use crate::host_ops::repository::{insert_dispatching, transition, NewHostOperation, Outcome};
    use crate::host_ops::{HostOperationEndpoint, HostOperationKind};
    use crate::printers::repository::PrinterRepository;
    use crate::slicing::blockers::{
        evaluate_slice_revision_deletion, slice_revision_blocker_sources,
    };
    use crate::slicing::repository::fixtures::{a_farm3d_revision, seed};
    use crate::slicing::repository::{delete_slice_revision, insert_farm3d_revision};
    use crate::spools::operations::OperationKind;
    use std::sync::Arc;

    fn config() -> ConnectionConfig {
        ConnectionConfig {
            kind: "moonraker".to_string(),
            host: "192.0.2.1".to_string(),
            port: 7125,
            use_tls: false,
            credential_ref: Some("farm3d/credential/a".to_string()),
        }
    }

    #[test]
    fn only_an_endpoint_change_or_a_clear_orphans_a_pending_operation() {
        let old = config();
        let changed = |mutate: fn(&mut ConnectionConfig)| {
            let mut new = config();
            mutate(&mut new);
            connection_change_orphans(Some(&old), Some(&new))
        };

        assert!(changed(|c| c.kind = "octoprint".to_string()));
        assert!(changed(|c| c.host = "192.0.2.2".to_string()));
        assert!(changed(|c| c.port = 7126));
        assert!(changed(|c| c.use_tls = true));
        assert!(changed(|c| c.credential_ref = None), "credential cleared");
        assert!(
            connection_change_orphans(Some(&old), None),
            "Connection cleared"
        );
        assert!(connection_change_orphans(None, Some(&old)));

        assert!(!changed(|_| {}), "unchanged");
        assert!(
            !changed(|c| c.credential_ref = Some("farm3d/credential/b".to_string())),
            "a same-endpoint credential replacement is allowed"
        );
        let mut no_credential = config();
        no_credential.credential_ref = None;
        assert!(
            !connection_change_orphans(Some(&no_credential), Some(&old)),
            "adding a credential clears nothing"
        );
        assert!(!connection_change_orphans(None, None));
    }

    fn storage_with_printer() -> (
        tempfile::TempDir,
        crate::persistence::MetadataRootLease,
        Arc<crate::persistence::Storage>,
    ) {
        let (temp, lease, storage) = crate::test_storage();
        storage
            .write(|tx| {
                seed(tx);
                Ok(())
            })
            .expect("seed");
        PrinterRepository::new(Arc::clone(&storage))
            .create(StoredPrinter {
                id: "prn-a".to_string(),
                name: "Printer".to_string(),
                ..Default::default()
            })
            .expect("printer");
        storage
            .write_repo(|tx| insert_farm3d_revision(tx, &a_farm3d_revision("slr-a", 1)))
            .expect("revision");
        (temp, lease, storage)
    }

    fn row(kind: HostOperationKind, slice_revision_id: &str) -> NewHostOperation {
        let (operation_kind, history_mark) = match kind {
            HostOperationKind::Upload => (OperationKind::StageSliceRevision, None),
            _ => (OperationKind::StartStagedArtifact, Some(0)),
        };
        NewHostOperation {
            operation_id: format!("op-{kind:?}"),
            operation_kind,
            request_digest: "digest".to_string(),
            printer_id: "prn-a".to_string(),
            kind,
            slice_revision_id: Some(slice_revision_id.to_string()),
            source_host_operation_id: None,
            gcode_sha256: Some("a".repeat(64)),
            gcode_size: Some(100),
            host_path: format!("farm3d/{slice_revision_id}.gcode"),
            history_mark,
            endpoint: HostOperationEndpoint {
                kind: "moonraker".to_string(),
                host: "192.0.2.1".to_string(),
                port: 7125,
            },
        }
    }

    fn uncertain() -> Outcome {
        Outcome::Uncertain {
            reason: InconclusiveReason::ResponseLost,
            no_longer_pending: false,
        }
    }

    fn delete_revision(
        storage: &crate::persistence::Storage,
    ) -> Result<crate::slicing::repository::DeletedSliceRevision, RepositoryError> {
        storage
            .write_repo(|tx| delete_slice_revision(tx, "slr-a", slice_revision_blocker_sources()))
    }

    #[test]
    fn deleting_a_revision_an_unresolved_upload_or_start_uses_is_blocked() {
        for kind in [HostOperationKind::Upload, HostOperationKind::Start] {
            // dispatching, uncertain, reconciling
            for steps in [
                vec![],
                vec![uncertain()],
                vec![uncertain(), Outcome::Reconciling],
            ] {
                let (_temp, _lease, storage) = storage_with_printer();
                let id = storage
                    .write_repo(|tx| insert_dispatching(tx, &row(kind, "slr-a")))
                    .expect("insert")
                    .id;
                for step in steps {
                    storage
                        .write_repo(|tx| transition(tx, &id, step.clone()))
                        .expect("step");
                }

                let error = delete_revision(&storage).expect_err("blocked");

                let RepositoryError::LifecycleBlocked(blockers) = error else {
                    panic!("expected LifecycleBlocked, got {error:?}");
                };
                assert_eq!(blockers.len(), 1);
                assert_eq!(blockers[0].action, LifecycleAction::Delete);
                assert_eq!(
                    blockers[0].code,
                    LifecycleBlockerCode::HostOperationUnresolved
                );
                assert_eq!(
                    blockers[0].message,
                    "A printer operation using this Slice Revision is still pending. \
                     Finish or abandon it first."
                );
                let revisions: i64 = storage
                    .read(|c| c.query_row("SELECT COUNT(*) FROM slice_revisions", [], |r| r.get(0)))
                    .expect("count");
                assert_eq!(revisions, 1, "nothing deleted");
            }
        }
    }

    #[test]
    fn deleting_a_revision_is_allowed_after_a_terminal_row_and_unlinks_it() {
        use crate::connections::capabilities::HostOperationFailureCode;
        use crate::host_ops::{HostOperationFailure, HostOperationResolution};
        let terminal: [Vec<Outcome>; 3] = [
            vec![Outcome::Succeeded {
                resolution: HostOperationResolution::ArtifactVerified { reconciled: false },
            }],
            vec![Outcome::Failed {
                failure: HostOperationFailure::for_code(HostOperationFailureCode::HostUnreachable),
            }],
            vec![uncertain(), Outcome::Abandoned { note: None }],
        ];
        for steps in terminal {
            let (_temp, _lease, storage) = storage_with_printer();
            let id = storage
                .write_repo(|tx| insert_dispatching(tx, &row(HostOperationKind::Upload, "slr-a")))
                .expect("insert")
                .id;
            for step in steps {
                storage
                    .write_repo(|tx| transition(tx, &id, step.clone()))
                    .expect("step");
            }

            delete_revision(&storage).expect("allowed");

            let kept = storage
                .read(|c| Ok(repository::load(c, &id)))
                .expect("read")
                .expect("load")
                .expect("the terminal row is kept");
            assert_eq!(kept.slice_revision_id, None);
        }
    }

    #[test]
    fn another_revisions_unresolved_row_does_not_block() {
        let (_temp, _lease, storage) = storage_with_printer();
        storage
            .write_repo(|tx| insert_farm3d_revision(tx, &a_farm3d_revision("slr-b", 2)))
            .expect("second revision");
        storage
            .write_repo(|tx| insert_dispatching(tx, &row(HostOperationKind::Upload, "slr-b")))
            .expect("insert");

        let blockers = storage
            .write_repo(|tx| {
                evaluate_slice_revision_deletion(
                    "slr-a",
                    tx,
                    &[&UnresolvedHostOperationBlocksRevisionDeletion],
                )
                .map_err(RepositoryError::from)
            })
            .expect("evaluate");

        assert!(blockers.is_empty());
    }
}
