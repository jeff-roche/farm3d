//! Printer lifecycle eligibility (spec D6/D7): whether archive, unarchive,
//! or delete is currently allowed for a Printer, and why not when it isn't.
//!
//! `evaluate` is the single source of truth every lifecycle command (and the
//! `printer_lifecycle_eligibility` query) runs through, always inside the
//! same transaction as any write it gates — so the check and the write it
//! guards never observe different data. P2 registers only
//! `ArchiveStateBlockers`; P3, P7, and P8 register their own
//! `LifecycleBlockerSource`s without this module changing.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::persistence::StorageError;

use super::StoredPrinter;

/// A lifecycle action a Printer can be moved through.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/LifecycleAction.ts")]
pub enum LifecycleAction {
    Archive,
    Unarchive,
    Delete,
}

/// Why a `LifecycleAction` is currently blocked. P2 only ever produces
/// `NotArchived`/`AlreadyArchived`; later phases add variants (spec D7).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(
    rename_all = "SCREAMING_SNAKE_CASE",
    export_to = "domain/LifecycleBlockerCode.ts"
)]
pub enum LifecycleBlockerCode {
    NotArchived,
    AlreadyArchived,
}

#[derive(Serialize, Clone, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/LifecycleBlocker.ts")]
pub struct LifecycleBlocker {
    pub action: LifecycleAction,
    pub code: LifecycleBlockerCode,
    pub message: String,
}

#[derive(Serialize, Clone, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/LifecycleEligibility.ts")]
pub struct LifecycleEligibility {
    pub can_archive: bool,
    pub can_unarchive: bool,
    pub can_delete: bool,
    pub blockers: Vec<LifecycleBlocker>,
}

/// One source of `LifecycleBlocker`s, consulted by `evaluate` inside the
/// same transaction a lifecycle write runs in.
pub trait LifecycleBlockerSource: Send + Sync {
    fn blockers(
        &self,
        printer: &StoredPrinter,
        tx: &rusqlite::Transaction<'_>,
    ) -> Result<Vec<LifecycleBlocker>, StorageError>;
}

/// Blocks `delete`/`unarchive` on a Printer that isn't archived yet — D6
/// requires archiving before deletion — and blocks `archive` on a Printer
/// that already is.
struct ArchiveStateBlockers;

impl LifecycleBlockerSource for ArchiveStateBlockers {
    fn blockers(
        &self,
        printer: &StoredPrinter,
        _tx: &rusqlite::Transaction<'_>,
    ) -> Result<Vec<LifecycleBlocker>, StorageError> {
        Ok(if printer.archived_at.is_none() {
            vec![
                LifecycleBlocker {
                    action: LifecycleAction::Delete,
                    code: LifecycleBlockerCode::NotArchived,
                    message: "Archive this Printer before deleting it.".to_string(),
                },
                LifecycleBlocker {
                    action: LifecycleAction::Unarchive,
                    code: LifecycleBlockerCode::NotArchived,
                    message: "This Printer is not archived.".to_string(),
                },
            ]
        } else {
            vec![LifecycleBlocker {
                action: LifecycleAction::Archive,
                code: LifecycleBlockerCode::AlreadyArchived,
                message: "This Printer is already archived.".to_string(),
            }]
        })
    }
}

/// P2's blocker sources, in the order their blockers should be reported.
/// Later phases append here rather than changing anything above.
pub fn blocker_sources() -> &'static [&'static dyn LifecycleBlockerSource] {
    &[&ArchiveStateBlockers]
}

/// The single derivation of a Printer's lifecycle eligibility. Always run
/// inside the same transaction as any write it gates.
pub fn evaluate(
    printer: &StoredPrinter,
    tx: &rusqlite::Transaction<'_>,
) -> Result<LifecycleEligibility, StorageError> {
    let mut blockers = Vec::new();
    for source in blocker_sources() {
        blockers.extend(source.blockers(printer, tx)?);
    }
    let blocks =
        |action: LifecycleAction| blockers.iter().any(|blocker| blocker.action == action);
    Ok(LifecycleEligibility {
        can_archive: !blocks(LifecycleAction::Archive),
        can_unarchive: !blocks(LifecycleAction::Unarchive),
        can_delete: !blocks(LifecycleAction::Delete),
        blockers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printers::CatalogRef;

    fn a_printer() -> StoredPrinter {
        StoredPrinter {
            id: "prn-1".to_string(),
            name: "Test Printer".to_string(),
            catalog_ref: CatalogRef::default(),
            ..Default::default()
        }
    }

    fn evaluate_in_a_fresh_transaction(printer: &StoredPrinter) -> LifecycleEligibility {
        let (_root, _lease, storage) = crate::test_storage();
        storage
            .read_transaction(|tx| Ok(evaluate(printer, tx)))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn an_active_printer_can_only_be_archived() {
        let eligibility = evaluate_in_a_fresh_transaction(&a_printer());

        assert!(eligibility.can_archive);
        assert!(!eligibility.can_unarchive);
        assert!(!eligibility.can_delete);
        assert_eq!(
            eligibility
                .blockers
                .iter()
                .map(|blocker| (blocker.action, blocker.code))
                .collect::<Vec<_>>(),
            vec![
                (LifecycleAction::Delete, LifecycleBlockerCode::NotArchived),
                (
                    LifecycleAction::Unarchive,
                    LifecycleBlockerCode::NotArchived
                ),
            ]
        );
    }

    #[test]
    fn an_archived_printer_can_only_be_unarchived_or_deleted() {
        let mut printer = a_printer();
        printer.archived_at = Some("2026-09-20T00:00:00Z".to_string());
        let eligibility = evaluate_in_a_fresh_transaction(&printer);

        assert!(!eligibility.can_archive);
        assert!(eligibility.can_unarchive);
        assert!(eligibility.can_delete);
        assert_eq!(
            eligibility
                .blockers
                .iter()
                .map(|blocker| (blocker.action, blocker.code))
                .collect::<Vec<_>>(),
            vec![(
                LifecycleAction::Archive,
                LifecycleBlockerCode::AlreadyArchived
            )]
        );
    }
}
