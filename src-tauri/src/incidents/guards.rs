//! P8 D8 "Lifecycle guards": what keeps a Printer archive, delete, or
//! import from orphaning an Incident or its camera evidence. Every check
//! here runs inside the caller's own write transaction, beside the other
//! guards (`printers::lifecycle`, `jobs::guards`).
//!
//! - [`IncidentBlockers`], the Printer lifecycle blocker source. Delete is
//!   blocked by any Incident of the Printer (`INCIDENT_HISTORY_EXISTS`;
//!   owner decision 3: archive it instead), and by a pinned, unpruned
//!   `manual` snapshot with no Incident and no Job
//!   (`PINNED_EVIDENCE_EXISTS`). Archive is never blocked here.
//! - [`check_import`], for `PrinterRepository::replace_all`
//!   (`EVIDENCE_EXISTS`). Any Incident or any `camera_snapshots` row
//!   rejects the whole import before anything is written.
//! - [`resolve_printer_events`], the delete-time half. A deleted (or
//!   replaced) Printer's open Events resolve `sourceRemoved` before its
//!   row goes, so none is left open against a source that no longer
//!   exists.
//!
//! Every other snapshot is covered already: an `incident` snapshot by
//! `INCIDENT_HISTORY_EXISTS`, and a `completion` snapshot, or a `manual`
//! one linked to a Job, by P7's `JOB_HISTORY_EXISTS`. So a delete that
//! passes these guards leaves no evidence that references the Printer
//! except the unattached manual rows it removes
//! (`cameras::media::remove_unattached_manual`).

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use rusqlite::Transaction;

use crate::attention::{repository as attention_repository, AttentionEvent};
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::lifecycle::{
    LifecycleAction, LifecycleBlocker, LifecycleBlockerCode, LifecycleBlockerSource,
};
use crate::printers::StoredPrinter;

use super::repository as incidents_repository;
use super::{Incident, IncidentEntryDetail};

/// The most ids `EVIDENCE_EXISTS` names in each list.
const IMPORT_ID_LIMIT: usize = 20;

/// D8: blocks deleting a Printer that has Incident history, or pinned
/// camera evidence no Incident or Job holds. `incidents.printer_id` and
/// `camera_snapshots.printer_id` are both `ON DELETE RESTRICT`; this turns
/// the database's refusal into a reason the operator can act on.
pub struct IncidentBlockers;

impl LifecycleBlockerSource for IncidentBlockers {
    fn blockers(
        &self,
        printer: &StoredPrinter,
        tx: &Transaction<'_>,
    ) -> Result<Vec<LifecycleBlocker>, StorageError> {
        let mut blockers = Vec::new();

        let has_incidents: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM incidents WHERE printer_id = ?1)",
            [&printer.id],
            |row| row.get(0),
        )?;
        if has_incidents {
            blockers.push(LifecycleBlocker {
                action: LifecycleAction::Delete,
                code: LifecycleBlockerCode::IncidentHistoryExists,
                message: "This Printer has Incident history. Archive it instead.".to_string(),
            });
        }

        // A pruned row never counts, whatever its reason (a pinned
        // `missingFile` one included): it has no image left to protect.
        let has_pinned: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM camera_snapshots
                 WHERE printer_id = ?1 AND trigger = 'manual'
                   AND incident_id IS NULL AND job_id IS NULL
                   AND pinned_at IS NOT NULL AND pruned_at IS NULL
             )",
            [&printer.id],
            |row| row.get(0),
        )?;
        if has_pinned {
            blockers.push(LifecycleBlocker {
                action: LifecycleAction::Delete,
                code: LifecycleBlockerCode::PinnedEvidenceExists,
                message: "This Printer has pinned camera evidence. Unpin it or archive the \
                          Printer instead."
                    .to_string(),
            });
        }

        Ok(blockers)
    }
}

/// D8 `EVIDENCE_EXISTS` for `import_printers` (`PrinterRepository::
/// replace_all`): rejects the whole import, nothing written, while any
/// Incident or any `camera_snapshots` row exists. `replace_all` deletes
/// every Printer, and both tables' `printer_id` is `ON DELETE RESTRICT`,
/// so either would otherwise fail the delete mid-import. Each list is
/// capped at 20.
pub fn check_import(tx: &Transaction<'_>) -> Result<(), RepositoryError> {
    let mut printer_ids = BTreeSet::new();
    let incident_ids = ids_with_printer(
        tx,
        "SELECT id, printer_id FROM incidents ORDER BY opened_at, id",
        &mut printer_ids,
    )?;
    let snapshot_ids = ids_with_printer(
        tx,
        "SELECT id, printer_id FROM camera_snapshots ORDER BY captured_at, id",
        &mut printer_ids,
    )?;
    if incident_ids.is_empty() && snapshot_ids.is_empty() {
        return Ok(());
    }
    Err(RepositoryError::EvidenceExists {
        printer_ids: printer_ids.into_iter().take(IMPORT_ID_LIMIT).collect(),
        incident_ids: incident_ids.into_iter().take(IMPORT_ID_LIMIT).collect(),
        snapshot_ids: snapshot_ids.into_iter().take(IMPORT_ID_LIMIT).collect(),
    })
}

/// Every `(id, printer_id)` row of `sql`: the ids in order, and each
/// Printer id added to `printer_ids`.
fn ids_with_printer(
    tx: &Transaction<'_>,
    sql: &str,
    printer_ids: &mut BTreeSet<String>,
) -> Result<Vec<String>, RepositoryError> {
    let mut statement = tx.prepare(sql)?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut ids = Vec::new();
    for row in rows {
        let (id, printer_id) = row?;
        ids.push(id);
        printer_ids.insert(printer_id);
    }
    Ok(ids)
}

/// What [`resolve_printer_events`] changed, to publish after commit.
#[derive(Debug, Default)]
pub struct ResolvedPrinterEvents {
    pub events: Vec<AttentionEvent>,
    pub incidents: Vec<Incident>,
}

/// D8, Printer delete step 1 (and `replace_all`, for every replaced
/// Printer): every open Event with `printer_id` = `printer_id` resolves
/// `sourceRemoved`. A linked one gains `eventResolved` on its Incident,
/// which closes if that was its last open actionable Event. In practice
/// none is linked, since any Incident of the Printer blocks the delete.
pub fn resolve_printer_events(
    tx: &Transaction<'_>,
    printer_id: &str,
    now: DateTime<Utc>,
) -> Result<ResolvedPrinterEvents, RepositoryError> {
    let events = attention_repository::resolve_for_printer(tx, printer_id, now)?;
    let mut touched: Vec<String> = Vec::new();
    for event in &events {
        let Some(incident_id) = &event.incident_id else {
            continue;
        };
        incidents_repository::append_entry(
            tx,
            incident_id,
            &IncidentEntryDetail::EventResolved {
                event_id: event.id.clone(),
                resolution: crate::attention::AttentionResolution::SourceRemoved,
            },
            None,
            now,
        )?;
        if !touched.contains(incident_id) {
            touched.push(incident_id.clone());
        }
    }
    let mut incidents = Vec::with_capacity(touched.len());
    for incident_id in &touched {
        incidents.push(incidents_repository::close_if_settled(
            tx,
            incident_id,
            now,
        )?);
    }
    Ok(ResolvedPrinterEvents { events, incidents })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attention::{
        AttentionDetail, AttentionOrigin, AttentionResolution, AttentionSubject, Condition,
        ConditionKind,
    };
    use crate::incidents::{IncidentKind, IncidentState};
    use crate::persistence::{MetadataRootLease, Storage, StoragePaths};

    const NOW_TEXT: &str = "2026-09-28T12:00:00Z";

    fn now() -> DateTime<Utc> {
        NOW_TEXT.parse().unwrap()
    }

    fn storage() -> (tempfile::TempDir, MetadataRootLease, Storage) {
        let temp = tempfile::tempdir().unwrap();
        let paths =
            StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
        let lease = MetadataRootLease::acquire(&paths).unwrap();
        let storage = Storage::open(paths, &lease).unwrap();
        (temp, lease, storage)
    }

    fn seed_printer(tx: &Transaction<'_>, id: &str) {
        tx.execute_batch(&format!(
            "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
               catalog_variant, catalog_model_id, catalog_printer_variant, notes,
               overrides_json, created_at, updated_at)
             VALUES ('{id}', 1, 'Voron', '', '', '', '', '', '', '{{}}',
                     '{NOW_TEXT}', '{NOW_TEXT}');"
        ))
        .unwrap();
    }

    fn host_failed(printer_id: &str) -> Condition {
        Condition {
            kind: ConditionKind::PrinterHostFailed,
            source_id: printer_id.to_string(),
            printer_id: Some(printer_id.to_string()),
            job_id: None,
            spool_id: None,
            requirement_id: None,
            subject: AttentionSubject {
                printer_name: Some("Voron".into()),
                printer_location: Some("Bay A".into()),
                job_label: None,
                spool_number: None,
                spool_label: None,
            },
            detail: AttentionDetail::PrinterHostFailed,
            acknowledge: false,
        }
    }

    fn printer_snapshot() -> crate::jobs::PrinterSnapshot {
        use crate::catalog::{BedShape, PrinterProfile};
        crate::jobs::PrinterSnapshot {
            name: "Voron".to_string(),
            location: Some("Bay A".to_string()),
            catalog_ref: None,
            adapter_kind: None,
            profile: PrinterProfile {
                bed_shape: BedShape::Rectangular {
                    width_mm: 250.0,
                    depth_mm: 250.0,
                    origin_x_mm: 0.0,
                    origin_y_mm: 0.0,
                },
                printable_height_mm: 250.0,
                bed_exclude_areas: Vec::new(),
                default_bed_type: "4".to_string(),
                nozzle_diameter_mm: vec![0.4],
                nozzle_type: "brass".to_string(),
                gcode_flavor: "marlin".to_string(),
                has_auxiliary_fan: false,
                supports_air_filtration: false,
                supports_multi_filament: false,
                suggested_host_type: None,
            },
        }
    }

    /// The guard makes this unreachable through a delete; the helper still
    /// keeps a linked Incident's timeline and state right if it ever runs
    /// over one.
    #[test]
    fn a_linked_event_resolved_source_removed_writes_its_incident_timeline_and_closes_it() {
        let (_temp, _lease, storage) = storage();
        let (resolved, incident_id) = storage
            .write_repo(|tx| {
                seed_printer(tx, "prn-1");
                let event = attention_repository::insert(
                    tx,
                    &host_failed("prn-1"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;
                let incident = incidents_repository::open(
                    tx,
                    IncidentKind::PrinterHostFailed,
                    "prn-1",
                    None,
                    &printer_snapshot(),
                    &event.id,
                    now(),
                )?;
                let resolved = resolve_printer_events(tx, "prn-1", now())?;
                Ok((resolved, incident.id))
            })
            .unwrap();
        assert_eq!(resolved.events.len(), 1);
        assert_eq!(
            resolved.events[0].resolution,
            Some(AttentionResolution::SourceRemoved)
        );
        assert_eq!(resolved.incidents.len(), 1);
        assert_eq!(resolved.incidents[0].id, incident_id);
        assert_eq!(resolved.incidents[0].state, IncidentState::Closed);
        let entries = storage
            .read(|conn| Ok(incidents_repository::entries(conn, &incident_id)))
            .unwrap()
            .unwrap();
        let kinds: Vec<_> = entries.iter().map(|entry| entry.detail.kind()).collect();
        assert_eq!(
            kinds,
            [
                crate::incidents::IncidentEntryKind::Opened,
                crate::incidents::IncidentEntryKind::EventResolved,
                crate::incidents::IncidentEntryKind::Closed,
            ]
        );
    }

    #[test]
    fn a_printer_without_incidents_or_pinned_evidence_has_no_blockers_here() {
        let (_temp, _lease, storage) = storage();
        let blockers = storage
            .write_repo(|tx| {
                seed_printer(tx, "prn-1");
                let printer = StoredPrinter {
                    id: "prn-1".to_string(),
                    ..Default::default()
                };
                Ok(IncidentBlockers.blockers(&printer, tx)?)
            })
            .unwrap();
        assert!(blockers.is_empty(), "{blockers:?}");
        storage.write_repo(check_import).unwrap();
    }
}
