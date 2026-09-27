//! D6/D13: the operations ledger — one row per client-supplied
//! `operationId`. Each idempotent command ([`OperationKind`]) calls
//! [`claim`] first, inside its own transaction, with a digest of the
//! request fields that define it. A first use records the id and the
//! command applies; a retry with the same kind and digest is a
//! [`Claim::Replay`] and writes nothing; any other reuse is
//! [`RepositoryError::OperationIdReused`].
//!
//! The claim rolls back with the rest of a failed transaction, so a
//! rejected request never burns its `operationId`.

use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::persistence::RepositoryError;
use crate::printers::now_rfc3339;

use super::encode_enum;

/// The commands that claim an `operationId`. Stored as the camelCase
/// string the migration's `kind` CHECK lists. Printer create's initial
/// loads aren't here: their id is server-generated, so no client can
/// retry it.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    MoveSpool,
    ArchivePrinter,
    SpoolLifecycle,
    /// P5 D10: `start_slice`.
    StartSlice,
    /// P5 D16: `create_external_slice_revision`.
    CreateExternalSliceRevision,
    /// P6 D2: `stage_slice_revision`.
    StageSliceRevision,
    /// P6 D2: `start_staged_artifact`.
    StartStagedArtifact,
    /// P6 D2: `pause_host_print`.
    PauseHostPrint,
    /// P6 D2: `resume_host_print`.
    ResumeHostPrint,
    /// P6 D2: `cancel_host_print`.
    CancelHostPrint,
    /// P6 D2: `abandon_host_operation`.
    AbandonHostOperation,
}

/// What [`claim`] found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Claim {
    /// The id was new and is now recorded; apply the operation.
    Fresh,
    /// The same request already ran under this id; write nothing.
    Replay,
}

/// SHA-256 hex of `request`'s JSON. Pass a struct (fields serialize in
/// declaration order), never a map, so the digest is deterministic.
pub fn digest(request: &impl Serialize) -> String {
    let json = serde_json::to_vec(request).expect("operation requests always serialize");
    format!("{:x}", Sha256::digest(json))
}

/// Records `operation_id` for `kind`/`request_digest`, or reports it as a
/// replay of the same request. A blank id is `VALIDATION` on
/// `operationId`; an id recorded for another kind or digest is
/// [`RepositoryError::OperationIdReused`].
pub fn claim(
    tx: &Transaction<'_>,
    operation_id: &str,
    kind: OperationKind,
    request_digest: &str,
) -> Result<Claim, RepositoryError> {
    if operation_id.trim().is_empty() {
        return Err(RepositoryError::Validation {
            field_path: "operationId",
        });
    }
    let kind = encode_enum(kind);
    let recorded: Option<(String, String)> = tx
        .query_row(
            "SELECT kind, request_digest FROM operations WHERE id = ?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match recorded {
        None => {
            tx.execute(
                "INSERT INTO operations(id, kind, request_digest, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![operation_id, kind, request_digest, now_rfc3339()],
            )?;
            Ok(Claim::Fresh)
        }
        Some((recorded_kind, recorded_digest))
            if recorded_kind == kind && recorded_digest == request_digest =>
        {
            Ok(Claim::Replay)
        }
        Some(_) => Err(RepositoryError::OperationIdReused),
    }
}
