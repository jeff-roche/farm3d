//! P7 Task 11 (spec D8, ruling R5(b), owner decision 7): the lifecycle
//! guards that keep a Printer archive/delete/import, a Connection
//! endpoint change, a Slice Revision delete, or a Spool lifecycle change
//! from ever orphaning a Queue Entry, a Job, a reservation, or a history
//! record.
//!
//! Queue Entries and Jobs are created through the real commands
//! (`p7_rig::Rig`), so every fixture is a legitimately reachable row, not
//! a hand-built one that happens to satisfy the schema's `CHECK`s. The
//! guards themselves are exercised directly against `PrinterRepository`,
//! `slicing::repository::delete_slice_revision`, and
//! `spools::lifecycle::apply_lifecycle`, the same way P6's `p6_guards.rs`
//! and `host_ops::guards`'s own unit tests do.

mod common;
mod p7_rig;

use std::collections::HashMap;
use std::sync::Arc;

use farm3d_lib::connections::MOONRAKER_KIND;
use farm3d_lib::host_ops::repository::{self as host_ops_repo, NewHostOperation};
use farm3d_lib::host_ops::{HostOperationEndpoint, HostOperationKind};
use farm3d_lib::library;
use farm3d_lib::persistence::RepositoryError;
use farm3d_lib::printers::lifecycle::{LifecycleAction, LifecycleBlocker, LifecycleBlockerCode};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::slicing::blockers::slice_revision_blocker_sources;
use farm3d_lib::slicing::repository::delete_slice_revision;
use farm3d_lib::spools::lifecycle::{apply_lifecycle, SpoolLifecycleAction};
use farm3d_lib::spools::operations::OperationKind;
use farm3d_lib::spools::repository as spools_repository;
use serde_json::json;

use p7_rig::{Rig, PRINTER_A, PRINTER_B, SLR, SLR_EXTERNAL};

fn revision(rig: &Rig, printer_id: &str) -> i64 {
    PrinterRepository::new(Arc::clone(&rig.storage))
        .get(printer_id)
        .unwrap()
        .unwrap()
        .revision
}

fn repo(rig: &Rig) -> PrinterRepository {
    PrinterRepository::new(Arc::clone(&rig.storage))
}

/// Assigns one copy of [`SLR`] to `printer_id` with a fresh Spool, and
/// returns `(entryId, jobId)`.
fn assign_job(rig: &Rig, operation_id: &str, printer_id: &str) -> (String, String) {
    let spool = rig.spool(1_000_000);
    let entry_id = rig.add(&format!("{operation_id}-add"), 1)[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    rig.assign(operation_id, &entry_id, printer_id, &spool)
        .unwrap_or_else(|error| panic!("assign: {error}"));
    let job_id = rig.entry(&entry_id)["jobId"].as_str().unwrap().to_string();
    (entry_id, job_id)
}

fn only_blocker(error: &RepositoryError) -> &LifecycleBlocker {
    let RepositoryError::LifecycleBlocked(blockers) = error else {
        panic!("expected LifecycleBlocked, got {error:?}");
    };
    assert_eq!(blockers.len(), 1, "{blockers:?}");
    &blockers[0]
}

/// A `dispatching` (unresolved) Host Operation row on `printer_id`,
/// optionally linked to a Job. Returns its `hop-*` id.
fn unresolved_op(rig: &Rig, printer_id: &str, job_id: Option<&str>, operation_id: &str) -> String {
    let printer = repo(rig).get(printer_id).unwrap().unwrap();
    let connection = printer
        .connection
        .as_ref()
        .expect("Printer has a Connection");
    rig.storage
        .write_repo(|tx| {
            host_ops_repo::insert_dispatching(
                tx,
                &NewHostOperation {
                    operation_id: operation_id.to_string(),
                    operation_kind: OperationKind::StageSliceRevision,
                    request_digest: format!("digest-{operation_id}"),
                    printer_id: printer_id.to_string(),
                    kind: HostOperationKind::Upload,
                    slice_revision_id: Some(SLR.to_string()),
                    source_host_operation_id: None,
                    gcode_sha256: Some("a".repeat(64)),
                    gcode_size: Some(100),
                    host_path: format!("farm3d/{SLR}.gcode"),
                    history_mark: None,
                    endpoint: HostOperationEndpoint {
                        kind: MOONRAKER_KIND.to_string(),
                        host: connection.host.clone(),
                        port: connection.port,
                    },
                    job_id: job_id.map(str::to_string),
                },
            )
        })
        .expect("insert dispatching")
        .id
}

// --- Printer archive/delete: JOB_ACTIVE, JOB_HISTORY_EXISTS, QUEUE_ENTRY_PINNED ---

#[test]
fn archive_is_blocked_by_an_active_job_and_allowed_after_terminal() {
    let rig = Rig::new();
    let (_entry_id, job_id) = assign_job(&rig, "op-assign", PRINTER_A);

    let error = repo(&rig)
        .archive(PRINTER_A, revision(&rig, PRINTER_A), "op-archive-1", &[])
        .expect_err("blocked by the active Job");
    let blocker = only_blocker(&error);
    assert_eq!(blocker.action, LifecycleAction::Archive);
    assert_eq!(blocker.code, LifecycleBlockerCode::JobActive);
    assert_eq!(
        blocker.message,
        "Finish, cancel, or release this Printer's Job before archiving."
    );
    assert!(
        repo(&rig)
            .get(PRINTER_A)
            .unwrap()
            .unwrap()
            .archived_at
            .is_none(),
        "nothing written"
    );

    rig.job_command("cancel_job", "op-cancel", &job_id)
        .unwrap_or_else(|error| panic!("cancel_job: {error}"));

    let archived = repo(&rig)
        .archive(PRINTER_A, revision(&rig, PRINTER_A), "op-archive-2", &[])
        .expect("allowed once the Job is terminal");
    assert!(archived.archived_at.is_some());
}

#[test]
fn archive_keeps_job_identity_and_printer_snapshot() {
    let rig = Rig::new();
    let (_entry_id, job_id) = assign_job(&rig, "op-assign", PRINTER_A);
    rig.job_command("cancel_job", "op-cancel", &job_id)
        .unwrap_or_else(|error| panic!("cancel_job: {error}"));

    repo(&rig)
        .archive(PRINTER_A, revision(&rig, PRINTER_A), "op-archive", &[])
        .expect("allowed once the Job is terminal");

    let history = rig
        .call("get_job_history", json!({"jobId": job_id}))
        .expect("get_job_history");
    assert_eq!(history["job"]["printerId"], PRINTER_A);
    assert_eq!(history["job"]["printerSnapshot"]["name"], "Alpha");

    let printer = repo(&rig).get(PRINTER_A).unwrap().unwrap();
    assert!(printer.archived_at.is_some());
}

#[test]
fn delete_is_blocked_by_job_history() {
    let rig = Rig::new();
    let (_entry_id, job_id) = assign_job(&rig, "op-assign", PRINTER_A);
    rig.job_command("cancel_job", "op-cancel", &job_id)
        .unwrap_or_else(|error| panic!("cancel_job: {error}"));
    repo(&rig)
        .archive(PRINTER_A, revision(&rig, PRINTER_A), "op-archive", &[])
        .expect("archive");

    let error = repo(&rig)
        .delete(PRINTER_A, revision(&rig, PRINTER_A))
        .expect_err("blocked by Job history");
    let blocker = only_blocker(&error);
    assert_eq!(blocker.action, LifecycleAction::Delete);
    assert_eq!(blocker.code, LifecycleBlockerCode::JobHistoryExists);
    assert_eq!(
        blocker.message,
        "This Printer has Job history. Archive it instead."
    );
    assert!(repo(&rig).get(PRINTER_A).unwrap().is_some(), "not deleted");
}

#[test]
fn delete_is_blocked_by_a_pinned_queued_entry() {
    let rig = Rig::new();
    let pinned = rig
        .call(
            "add_to_queue",
            json!({
                "operationId": "op-pin",
                "sliceRevisionId": SLR,
                "quantity": 1,
                "policy": "manual",
                "preference": "loadedFirst",
                "manualPrinterId": PRINTER_B,
            }),
        )
        .expect("add_to_queue");
    let entry_id = pinned["entries"][0]["id"].as_str().unwrap().to_string();
    let entry_revision = pinned["entries"][0]["revision"].as_i64().unwrap();

    repo(&rig)
        .archive(PRINTER_B, revision(&rig, PRINTER_B), "op-archive", &[])
        .expect("archive (no Job on this Printer)");

    let error = repo(&rig)
        .delete(PRINTER_B, revision(&rig, PRINTER_B))
        .expect_err("blocked by the pinned entry");
    let blocker = only_blocker(&error);
    assert_eq!(blocker.action, LifecycleAction::Delete);
    assert_eq!(blocker.code, LifecycleBlockerCode::QueueEntryPinned);
    assert_eq!(
        blocker.message,
        "A Queue Entry is pinned to this Printer. Remove it or change its Printer first."
    );

    // Releasing the pin (closing the entry) lets the delete through.
    rig.call(
        "remove_queue_entry",
        json!({"operationId": "op-unpin", "entryId": entry_id, "expectedRevision": entry_revision}),
    )
    .expect("remove_queue_entry");
    repo(&rig)
        .delete(PRINTER_B, revision(&rig, PRINTER_B))
        .expect("allowed once the entry is closed");
}

// --- Slice Revision delete / Model delete: QUEUE_REFERENCES_REVISION ------

#[test]
fn slice_revision_delete_is_blocked_by_any_entry_or_job() {
    let rig = Rig::new();
    let entry_id = rig.add("op-add", 1)[0]["id"].as_str().unwrap().to_string();

    let assert_blocked = |error: RepositoryError| {
        let blocker = only_blocker(&error);
        assert_eq!(blocker.action, LifecycleAction::Delete);
        assert_eq!(blocker.code, LifecycleBlockerCode::QueueReferencesRevision);
        assert_eq!(
            blocker.message,
            "Queue Entries or Jobs use this Slice Revision."
        );
    };

    // A queued (unassigned) entry alone blocks it.
    let error = rig
        .storage
        .write_repo(|tx| delete_slice_revision(tx, SLR, slice_revision_blocker_sources()))
        .expect_err("blocked by the queued entry");
    assert_blocked(error);

    // Once a Job exists, the entry closes (cancelled before start) but the
    // Job -- and the closed entry, in any state -- still reference it.
    let spool = rig.spool(1_000_000);
    rig.assign("op-assign", &entry_id, PRINTER_A, &spool)
        .unwrap_or_else(|error| panic!("assign: {error}"));
    let job_id = rig.entry(&entry_id)["jobId"].as_str().unwrap().to_string();
    rig.job_command("cancel_job", "op-cancel", &job_id)
        .unwrap_or_else(|error| panic!("cancel_job: {error}"));

    let error = rig
        .storage
        .write_repo(|tx| delete_slice_revision(tx, SLR, slice_revision_blocker_sources()))
        .expect_err("still blocked by Job history and the closed entry");
    assert_blocked(error);

    assert_eq!(
        rig.count("SELECT COUNT(*) FROM slice_revisions WHERE id = 'slr-farm3d'"),
        1
    );
}

#[test]
fn model_delete_stays_blocked_transitively() {
    let rig = Rig::new();
    rig.add("op-add", 1);

    // The Model still has its one Slice Revision, so it's blocked the same
    // way it always was (`SLICE_REVISIONS_EXIST`) -- the Queue Entry
    // referencing that revision changes nothing about the Model's own
    // guard, because the revision itself can't be deleted out from under
    // the Model in the first place.
    let blockers = rig
        .storage
        .write_repo(|tx| {
            let model = library::repository::load_model(tx, "mdl-stl")
                .map_err(RepositoryError::from)?
                .expect("model");
            library::blockers::evaluate(&model, tx, library::blockers::blocker_sources())
                .map_err(RepositoryError::from)
        })
        .expect("evaluate");

    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0].action, LifecycleAction::Delete);
    assert_eq!(blockers[0].code, LifecycleBlockerCode::SliceRevisionsExist);
    assert_eq!(
        blockers[0].message,
        "Delete this Model's 1 Slice Revision first."
    );
}

// --- Printer import: JOBS_EXIST --------------------------------------------

#[test]
fn printer_import_is_rejected_whole_while_any_job_exists() {
    let rig = Rig::new();
    let (_entry_id, job_id) = assign_job(&rig, "op-assign", PRINTER_A);

    let printer_a = repo(&rig).get(PRINTER_A).unwrap().unwrap();
    let printer_b = repo(&rig).get(PRINTER_B).unwrap().unwrap();
    let expected = vec![
        (PRINTER_A.to_string(), printer_a.revision),
        (PRINTER_B.to_string(), printer_b.revision),
    ];

    let error = repo(&rig)
        .replace_all(
            &expected,
            vec![printer_a.clone(), printer_b.clone()],
            &HashMap::new(),
        )
        .expect_err("rejected whole");
    let RepositoryError::JobsExist {
        printer_ids,
        job_ids,
        queue_entry_ids,
    } = error
    else {
        panic!("expected JobsExist, got {error:?}");
    };
    assert_eq!(job_ids, vec![job_id]);
    assert_eq!(printer_ids, vec![PRINTER_A.to_string()]);
    assert!(queue_entry_ids.is_empty());

    // Nothing written.
    assert_eq!(
        repo(&rig).get(PRINTER_A).unwrap().unwrap().revision,
        printer_a.revision
    );
    assert_eq!(
        repo(&rig).get(PRINTER_B).unwrap().unwrap().revision,
        printer_b.revision
    );
    assert_eq!(rig.count("SELECT COUNT(*) FROM printers"), 2);
}

#[test]
fn printer_import_is_rejected_whole_while_a_queue_entry_is_pinned_with_no_job() {
    let rig = Rig::new();
    rig.call(
        "add_to_queue",
        json!({
            "operationId": "op-pin",
            "sliceRevisionId": SLR,
            "quantity": 1,
            "policy": "manual",
            "preference": "loadedFirst",
            "manualPrinterId": PRINTER_A,
        }),
    )
    .expect("add_to_queue");

    let printer_a = repo(&rig).get(PRINTER_A).unwrap().unwrap();
    let printer_b = repo(&rig).get(PRINTER_B).unwrap().unwrap();
    let expected = vec![
        (PRINTER_A.to_string(), printer_a.revision),
        (PRINTER_B.to_string(), printer_b.revision),
    ];

    let error = repo(&rig)
        .replace_all(&expected, vec![printer_a, printer_b], &HashMap::new())
        .expect_err("rejected whole");
    let RepositoryError::JobsExist {
        printer_ids,
        job_ids,
        queue_entry_ids,
    } = error
    else {
        panic!("expected JobsExist, got {error:?}");
    };
    assert!(job_ids.is_empty());
    assert_eq!(printer_ids, vec![PRINTER_A.to_string()]);
    assert_eq!(queue_entry_ids.len(), 1);
}

// --- Connection endpoint change: CONNECTION_IN_USE (ruling R5(b)) ---------

#[test]
fn endpoint_change_is_blocked_only_by_an_unresolved_host_operation() {
    let rig = Rig::new();
    let (_entry_id, job_id) = assign_job(&rig, "op-assign", PRINTER_A);

    // R5: an active Job with no unresolved Host Operation never blocks a
    // Connection change.
    let mut config = repo(&rig)
        .get(PRINTER_A)
        .unwrap()
        .unwrap()
        .connection
        .unwrap();
    config.host = "192.0.2.20".to_string();
    repo(&rig)
        .set_connection(
            PRINTER_A,
            revision(&rig, PRINTER_A),
            Some(config.clone()),
            None,
            "changed",
        )
        .expect("an active Job alone does not block");

    // A Job-linked unresolved Host Operation does, with the Job's own
    // message, recovery, and `jobId`.
    unresolved_op(&rig, PRINTER_A, Some(&job_id), "op-linked");
    let mut linked_config = config.clone();
    linked_config.host = "192.0.2.21".to_string();
    let error = repo(&rig)
        .set_connection(
            PRINTER_A,
            revision(&rig, PRINTER_A),
            Some(linked_config),
            None,
            "changed",
        )
        .expect_err("blocked by the Job-linked op");
    let RepositoryError::ConnectionInUse {
        printer_id,
        job_id: linked_job_id,
        ..
    } = &error
    else {
        panic!("expected ConnectionInUse, got {error:?}");
    };
    assert_eq!(printer_id, PRINTER_A);
    assert_eq!(linked_job_id.as_deref(), Some(job_id.as_str()));
    let command_error = farm3d_lib::contracts::command::CommandError::from_repository(error);
    assert_eq!(
        command_error.message,
        "This Printer's Job is waiting on a printer operation. Let it finish, or abandon the \
         check from the Job, before changing this Connection."
    );
    assert_eq!(
        command_error.recovery,
        vec![farm3d_lib::contracts::command::RecoveryCode::OpenJob]
    );
    assert_eq!(
        command_error.details.unwrap().get("jobId"),
        Some(&farm3d_lib::contracts::command::JsonValue::String(job_id))
    );

    // A raw (unlinked) unresolved op on another Printer keeps P6's own
    // message -- unchanged.
    let mut b_config = repo(&rig)
        .get(PRINTER_B)
        .unwrap()
        .unwrap()
        .connection
        .unwrap();
    unresolved_op(&rig, PRINTER_B, None, "op-raw");
    b_config.host = "192.0.2.22".to_string();
    let error = repo(&rig)
        .set_connection(
            PRINTER_B,
            revision(&rig, PRINTER_B),
            Some(b_config),
            None,
            "changed",
        )
        .expect_err("blocked by the raw op");
    let RepositoryError::ConnectionInUse { job_id, .. } = &error else {
        panic!("expected ConnectionInUse, got {error:?}");
    };
    assert!(job_id.is_none());
    let command_error = farm3d_lib::contracts::command::CommandError::from_repository(error);
    assert_eq!(
        command_error.message,
        "Finish or abandon the pending printer operation before changing this Connection."
    );
    assert_eq!(
        command_error.recovery,
        vec![farm3d_lib::contracts::command::RecoveryCode::OpenPrinterJob]
    );
}

// --- Spool archive: SPOOL_RESERVED (unchanged, P3) -------------------------

#[test]
fn spool_archive_is_blocked_by_an_unresolved_job_reservation() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entry_id = rig.add("op-add", 1)[0]["id"].as_str().unwrap().to_string();
    rig.assign("op-assign", &entry_id, PRINTER_A, &spool)
        .unwrap_or_else(|error| panic!("assign: {error}"));

    let error = rig
        .storage
        .write_repo(|tx| {
            let record =
                spools_repository::load_spool(tx, &spool).map_err(RepositoryError::from)?;
            let record = record.expect("spool");
            apply_lifecycle(
                tx,
                &spool,
                record.revision,
                SpoolLifecycleAction::Archive,
                None,
                "op-archive-spool",
            )
        })
        .expect_err("blocked by the Job's reservation");
    let blocker = only_blocker(&error);
    assert_eq!(blocker.action, LifecycleAction::Archive);
    assert_eq!(blocker.code, LifecycleBlockerCode::SpoolReserved);
    assert_eq!(
        blocker.message,
        "This Spool is reserved. Release its reservations first."
    );
}

// --- No orphan after every allowed action ----------------------------------

#[test]
fn no_row_is_orphaned_after_every_allowed_lifecycle_action() {
    let rig = Rig::new();

    // Import: allowed while nothing references either Printer yet.
    let printer_a = repo(&rig).get(PRINTER_A).unwrap().unwrap();
    let printer_b = repo(&rig).get(PRINTER_B).unwrap().unwrap();
    let expected = vec![
        (PRINTER_A.to_string(), printer_a.revision),
        (PRINTER_B.to_string(), printer_b.revision),
    ];
    let mut renamed_a = printer_a.clone();
    renamed_a.name = "Alpha Renamed".to_string();
    repo(&rig)
        .replace_all(&expected, vec![renamed_a, printer_b], &HashMap::new())
        .expect("import allowed (no Jobs yet)");

    // Connection change: allowed while unreferenced by any Job/op.
    let mut config = repo(&rig)
        .get(PRINTER_A)
        .unwrap()
        .unwrap()
        .connection
        .unwrap();
    config.host = "192.0.2.30".to_string();
    repo(&rig)
        .set_connection(
            PRINTER_A,
            revision(&rig, PRINTER_A),
            Some(config),
            None,
            "changed",
        )
        .expect("connection change allowed");

    // Assign + cancel before start: Job goes terminal, entry closes.
    let (_entry_id, job_id) = assign_job(&rig, "op-assign", PRINTER_B);
    rig.job_command("cancel_job", "op-cancel", &job_id)
        .unwrap_or_else(|error| panic!("cancel_job: {error}"));

    // Archive: allowed once the Job is terminal (it stays Job history).
    repo(&rig)
        .archive(PRINTER_B, revision(&rig, PRINTER_B), "op-archive-b", &[])
        .expect("archive allowed");

    // Mark an unreserved Spool empty: allowed.
    let free_spool = rig.spool(500_000);
    rig.storage
        .write_repo(|tx| {
            let record = spools_repository::load_spool(tx, &free_spool)
                .map_err(RepositoryError::from)?
                .expect("spool");
            apply_lifecycle(
                tx,
                &free_spool,
                record.revision,
                SpoolLifecycleAction::MarkEmpty,
                None,
                "op-mark-empty",
            )
        })
        .expect("mark empty allowed");

    // Pin then unpin a Queue Entry on the untouched Printer, then archive
    // and delete it (no Job, no open pin left).
    let pinned = rig
        .call(
            "add_to_queue",
            json!({
                "operationId": "op-pin",
                "sliceRevisionId": SLR,
                "quantity": 1,
                "policy": "manual",
                "preference": "loadedFirst",
                "manualPrinterId": PRINTER_A,
            }),
        )
        .expect("add_to_queue");
    rig.call(
        "remove_queue_entry",
        json!({
            "operationId": "op-unpin",
            "entryId": pinned["entries"][0]["id"],
            "expectedRevision": pinned["entries"][0]["revision"],
        }),
    )
    .expect("remove_queue_entry");
    repo(&rig)
        .archive(PRINTER_A, revision(&rig, PRINTER_A), "op-archive-a", &[])
        .expect("archive allowed");
    repo(&rig)
        .delete(PRINTER_A, revision(&rig, PRINTER_A))
        .expect("delete allowed (no Job history)");

    // Delete an unreferenced Slice Revision: allowed.
    rig.storage
        .write_repo(|tx| delete_slice_revision(tx, SLR_EXTERNAL, slice_revision_blocker_sources()))
        .expect("delete allowed (unreferenced)");

    let violations: i64 = rig
        .storage
        .read(|connection| {
            connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
        })
        .expect("foreign_key_check");
    assert_eq!(violations, 0, "no row should be left dangling");
}
