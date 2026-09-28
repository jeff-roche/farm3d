//! P8 decision 8 "Deep-link targets": where opening an Attention Event
//! goes, in F1's `NavigationTarget` format.
//!
//! | Source | Target |
//! |---|---|
//! | Printer | `monitor/printer/<id>` |
//! | Job, or a Reconciliation Requirement's Job | `queue/job/<id>` |
//! | Spool | `spools/spool/<id>` |
//! | (source gone) | `monitor/attention/<eventId>` |
//!
//! A notification opens the Event's source; when that source no longer
//! exists (evaluated at click time), it opens the Event itself. An
//! archived Printer stays a valid `monitor/printer` target.
//! `src/attention/deep-link.ts` mirrors this table.

use rusqlite::{Connection, OptionalExtension};

use crate::contracts::navigation::{
    NavigationDestination, NavigationSelection, NavigationSelectionKind, NavigationTarget,
};
use crate::contracts::ContractVersion;

use super::{AttentionEvent, AttentionSourceKind};

fn target(
    destination: NavigationDestination,
    kind: NavigationSelectionKind,
    id: &str,
) -> NavigationTarget {
    NavigationTarget {
        version: ContractVersion::V1,
        destination,
        selection: Some(NavigationSelection {
            kind,
            id: id.to_string(),
        }),
    }
}

/// `monitor/attention/<eventId>`: the Event itself.
pub fn event_target(event: &AttentionEvent) -> NavigationTarget {
    target(
        NavigationDestination::Monitor,
        NavigationSelectionKind::Attention,
        &event.id,
    )
}

/// The Event's source's target, assuming it still exists. A requirement
/// opens its Job; one without a Job (never in practice) opens the Event.
pub fn source_target(event: &AttentionEvent) -> NavigationTarget {
    match event.source.kind {
        AttentionSourceKind::Printer => target(
            NavigationDestination::Monitor,
            NavigationSelectionKind::Printer,
            &event.source.id,
        ),
        AttentionSourceKind::Job => target(
            NavigationDestination::Queue,
            NavigationSelectionKind::Job,
            &event.source.id,
        ),
        AttentionSourceKind::ReconciliationRequirement => match &event.job_id {
            Some(job_id) => target(
                NavigationDestination::Queue,
                NavigationSelectionKind::Job,
                job_id,
            ),
            None => event_target(event),
        },
        AttentionSourceKind::Spool => target(
            NavigationDestination::Spools,
            NavigationSelectionKind::Spool,
            &event.source.id,
        ),
    }
}

/// Decision 8, at click time: the source's target when the source row
/// still exists, otherwise the Event itself.
pub fn target_for(event: &AttentionEvent, source_exists: bool) -> NavigationTarget {
    if source_exists {
        source_target(event)
    } else {
        event_target(event)
    }
}

/// Whether the row [`source_target`] names still exists (archived rows
/// count).
pub fn source_exists(conn: &Connection, event: &AttentionEvent) -> rusqlite::Result<bool> {
    let (table, id) = match event.source.kind {
        AttentionSourceKind::Printer => ("printers", event.source.id.as_str()),
        AttentionSourceKind::Job => ("jobs", event.source.id.as_str()),
        AttentionSourceKind::ReconciliationRequirement => match &event.job_id {
            Some(job_id) => ("jobs", job_id.as_str()),
            None => return Ok(false),
        },
        AttentionSourceKind::Spool => ("spools", event.source.id.as_str()),
    };
    Ok(conn
        .query_row(
            &format!("SELECT 1 FROM {table} WHERE id = ?1"),
            [id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}
