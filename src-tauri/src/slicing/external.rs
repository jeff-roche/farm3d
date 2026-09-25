//! D16: `create_external_slice_revision` — an external Slice Revision over
//! a G-code Model Source Revision the user already imported, from the
//! operator's confirmed facts. farm3d never infers a Printer, nozzle, or
//! material from the file's own claims: [`ExternalFacts`] can only be
//! built from [`ConfirmedFacts`], so the file's claims can reach only
//! [`ClaimedEstimates`] (shown as "What the file says (not verified)"),
//! never [`super::SliceFacts`].
//!
//! The revision reuses the source revision's content hash as its
//! `gcode_sha256` ([`repository::insert_external_revision`]): there is no
//! copy, and no `slice_revision_blobs` rows. Idempotency is by
//! `operationId`, through the P3 ledger's [`OperationKind::CreateExternalSliceRevision`],
//! following [`super::operations::start_slice`]'s pattern: a replay is
//! detected before any resolution work runs (so a retry after the printer
//! or catalog entry it named has since changed still returns the original
//! result), and `external_lock` serializes the check with the claim so the
//! two can't interleave.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::contracts::command::CommandError;
use crate::library::formats::{Inspection, Producer};
use crate::library::repository as library_repository;
use crate::persistence::{RepositoryError, Storage, StorageError};
use crate::spools::encode_enum;
use crate::spools::operations::{self as ledger, Claim, OperationKind};
use crate::spools::MaterialFamily;

use super::presets::resolve_target;
use super::publish::claimed_estimates_from_claims;
use super::repository::{self, NewExternalRevision};
use super::{
    events, operations, ClaimedEstimates, ConfirmedFact, ConfirmedFacts, ExternalFacts,
    SliceRevisionRecord, SliceTarget, SlicingServices,
};

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// D16: one fact of `create_external_slice_revision`'s request — the
/// operator either confirms a value or leaves it absent. There is no way
/// to say `farm3dInput` here, matching [`super::facts::ConfirmedFact`],
/// which this converts to.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    export_to = "command/ConfirmedFactRequest.ts"
)]
pub enum ConfirmedFactRequest<T> {
    Confirmed { value: T },
    Absent,
}

impl<T> ConfirmedFactRequest<T> {
    fn into_confirmed(self) -> ConfirmedFact<T> {
        match self {
            Self::Confirmed { value } => ConfirmedFact::Confirmed(value),
            Self::Absent => ConfirmedFact::Absent,
        }
    }
}

/// D16 `create_external_slice_revision`'s `facts`. The Printer Profile is
/// confirmed as a [`SliceTarget`] (a Printer or a catalog profile),
/// resolved to a [`super::ProfileSnapshot`] at creation; the other three
/// facts are confirmed directly.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "command/CreateExternalSliceRevisionFacts.ts"
)]
pub struct CreateExternalSliceRevisionFacts {
    pub printer_profile: ConfirmedFactRequest<SliceTarget>,
    pub nozzle_diameter_mm: ConfirmedFactRequest<f64>,
    pub material_family: ConfirmedFactRequest<MaterialFamily>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub material_other: Option<String>,
    pub filament_diameter_mm: ConfirmedFactRequest<f64>,
}

/// The request fields that define a `create_external_slice_revision`
/// call, in a fixed order for [`ledger::digest`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateExternalDigest<'a> {
    source_revision_id: &'a str,
    facts: &'a CreateExternalSliceRevisionFacts,
}

// ---------------------------------------------------------------------------
// Replay and id derivation
// ---------------------------------------------------------------------------

/// The Slice Revision id for `create_external_slice_revision`
/// `operation_id`: `slr-` and a UUID-shaped SHA-256 prefix of it, so a
/// replay (even after a restart) names the same row (mirrors
/// [`operations::derived_operation_id`]).
fn derived_revision_id(operation_id: &str) -> String {
    let hash = format!("{:x}", Sha256::digest(operation_id.as_bytes()));
    format!(
        "slr-{}-{}-{}-{}-{}",
        &hash[0..8],
        &hash[8..12],
        &hash[12..16],
        &hash[16..20],
        &hash[20..32]
    )
}

/// The revision of an earlier, identical `create_external_slice_revision`,
/// or `None` for a new `operationId`. The same id for another request is
/// the P3 ledger's `operationId` reuse error.
fn replay(
    storage: &Storage,
    operation_id: &str,
    digest: &str,
) -> Result<Option<SliceRevisionRecord>, CommandError> {
    let recorded = storage
        .read(|connection| operations::recorded_claim(connection, operation_id))
        .map_err(storage_error)?;
    let Some((kind, recorded_digest)) = recorded else {
        return Ok(None);
    };
    if kind != encode_enum(OperationKind::CreateExternalSliceRevision) || recorded_digest != digest
    {
        return Err(CommandError::from_repository(
            RepositoryError::OperationIdReused,
        ));
    }
    let revision_id = derived_revision_id(operation_id);
    let record = storage
        .read(|connection| Ok(repository::load_revision(connection, &revision_id)))
        .map_err(storage_error)?
        .map_err(storage_error)?
        .ok_or_else(|| CommandError::not_found(revision_id))?;
    Ok(Some(record))
}

/// D16: the source revision's G-code claims, read only for display —
/// never for facts. A source revision that isn't G-code is `VALIDATION` on
/// `sourceRevisionId`.
fn source_claims(
    storage: &Storage,
    source_revision_id: &str,
) -> Result<(ClaimedEstimates, Option<Producer>), CommandError> {
    let inspection = storage
        .read(|connection| {
            Ok(library_repository::load_source_revision_inspection(
                connection,
                source_revision_id,
            ))
        })
        .map_err(storage_error)?
        .map_err(storage_error)?
        .ok_or_else(|| CommandError::not_found(source_revision_id.to_string()))?;
    match inspection {
        Inspection::Gcode(gcode) => {
            Ok((claimed_estimates_from_claims(&gcode.claims), gcode.producer))
        }
        _ => Err(CommandError::from_repository(RepositoryError::Validation {
            field_path: "sourceRevisionId",
        })),
    }
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// D16 `create_external_slice_revision`: creates an external Slice
/// Revision over `source_revision_id` (a G-code Model Source Revision),
/// from `facts`. Idempotent by `operation_id`.
pub fn create_external_slice_revision<R: tauri::Runtime>(
    services: &Arc<SlicingServices<R>>,
    operation_id: &str,
    source_revision_id: &str,
    facts: &CreateExternalSliceRevisionFacts,
) -> Result<SliceRevisionRecord, CommandError> {
    if operation_id.trim().is_empty() {
        return Err(CommandError::from_repository(RepositoryError::Validation {
            field_path: "operationId",
        }));
    }
    let digest = ledger::digest(&CreateExternalDigest {
        source_revision_id,
        facts,
    });
    let _one_at_a_time = lock(&services.external_lock);
    if let Some(record) = replay(&services.storage, operation_id, &digest)? {
        return Ok(record);
    }

    let printer_profile = match &facts.printer_profile {
        ConfirmedFactRequest::Confirmed { value } => {
            let resolved = resolve_target(&services.storage, &services.catalog, value)?;
            ConfirmedFact::Confirmed(resolved.profile_snapshot())
        }
        ConfirmedFactRequest::Absent => ConfirmedFact::Absent,
    };
    let (claimed_estimates, producer) = source_claims(&services.storage, source_revision_id)?;

    let external_facts = ExternalFacts::new(ConfirmedFacts {
        printer_profile,
        nozzle_diameter_mm: facts.nozzle_diameter_mm.clone().into_confirmed(),
        material_family: facts.material_family.clone().into_confirmed(),
        material_other: facts.material_other.clone(),
        filament_diameter_mm: facts.filament_diameter_mm.clone().into_confirmed(),
    });

    let new_revision = NewExternalRevision {
        id: derived_revision_id(operation_id),
        source_revision_id: source_revision_id.to_string(),
        facts: external_facts,
        claimed_estimates,
        producer,
    };

    let record = services
        .storage
        .write_repo(|tx| {
            if ledger::claim(
                tx,
                operation_id,
                OperationKind::CreateExternalSliceRevision,
                &digest,
            )? == Claim::Replay
            {
                // Unreachable: `external_lock` serializes the replay check
                // above with the claim here.
                return Err(RepositoryError::OperationIdReused);
            }
            repository::insert_external_revision(tx, &new_revision)
        })
        .map_err(CommandError::from_repository)?;

    services.publish(vec![events::revision_created(&record.summary)]);
    Ok(record)
}
