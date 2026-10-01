//! D18's process-local `operationId` ledger for the commands whose effect
//! is a file or a swapped database rather than one transaction. A replay
//! in the same process returns the cached result; the same id with another
//! kind or request digest is `VALIDATION` on `operationId`. The ledger is
//! lost at restart, so a replay after one runs again (a new file).
//!
//! Only a completed result is recorded. A failure, or a `cancelled`
//! outcome (the dialog closed, nothing happened), records nothing, so a
//! retry under the same id runs again.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::contracts::command::CommandError;
use crate::persistence::RepositoryError;

/// The process-local operation kinds and their digests (D18).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcessOperationKind {
    /// `create_backup`, digest `{ media }`.
    CreateBackup,
    /// `delete_backup`, digest `{ backupId }`.
    DeleteBackup,
    /// `apply_restore`, digest `{ stagingId }`.
    ApplyRestore,
    /// `clear_storage`, digest `{ target }`.
    ClearStorage,
    /// `export_diagnostics`, digest `{ sections }` sorted.
    ExportDiagnostics,
    /// `reset_farm` tier `farm`, digest `{ request }`.
    ResetFarm,
}

struct Recorded {
    kind: ProcessOperationKind,
    digest: String,
    result: serde_json::Value,
}

#[derive(Default)]
pub struct ProcessOperations {
    recorded: Mutex<HashMap<String, Recorded>>,
}

impl ProcessOperations {
    /// The cached result of `operation_id`, if this process completed it
    /// for the same `kind` and `request`. A blank id, or one recorded for
    /// another kind or request, is `VALIDATION` on `operationId`.
    pub fn replay<T: DeserializeOwned>(
        &self,
        operation_id: &str,
        kind: ProcessOperationKind,
        request: &impl Serialize,
    ) -> Result<Option<T>, CommandError> {
        if operation_id.trim().is_empty() {
            return Err(CommandError::from_repository(RepositoryError::Validation {
                field_path: "operationId",
            }));
        }
        let digest = crate::spools::operations::digest(request);
        let recorded = self
            .recorded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match recorded.get(operation_id) {
            None => Ok(None),
            Some(entry) if entry.kind == kind && entry.digest == digest => {
                serde_json::from_value(entry.result.clone())
                    .map(Some)
                    .map_err(|_| CommandError::internal())
            }
            Some(_) => Err(CommandError::from_repository(
                RepositoryError::OperationIdReused,
            )),
        }
    }

    /// Records `result` as `operation_id`'s outcome.
    pub fn record(
        &self,
        operation_id: &str,
        kind: ProcessOperationKind,
        request: &impl Serialize,
        result: &impl Serialize,
    ) {
        let Ok(result) = serde_json::to_value(result) else {
            return;
        };
        self.recorded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                operation_id.to_string(),
                Recorded {
                    kind,
                    digest: crate::spools::operations::digest(request),
                    result,
                },
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_replay_returns_the_cached_result_and_a_reuse_is_validation() {
        let ledger = ProcessOperations::default();
        let request = json!({ "media": "all" });
        assert_eq!(
            ledger
                .replay::<serde_json::Value>("op", ProcessOperationKind::CreateBackup, &request)
                .unwrap(),
            None
        );
        ledger.record(
            "op",
            ProcessOperationKind::CreateBackup,
            &request,
            &json!({ "a": 1 }),
        );
        assert_eq!(
            ledger
                .replay::<serde_json::Value>("op", ProcessOperationKind::CreateBackup, &request)
                .unwrap(),
            Some(json!({ "a": 1 }))
        );
        for (kind, request) in [
            (
                ProcessOperationKind::CreateBackup,
                json!({ "media": "none" }),
            ),
            (
                ProcessOperationKind::DeleteBackup,
                json!({ "media": "all" }),
            ),
        ] {
            let error = ledger
                .replay::<serde_json::Value>("op", kind, &request)
                .unwrap_err();
            let error = serde_json::to_value(error).unwrap();
            assert_eq!(error["code"], "VALIDATION");
            assert_eq!(error["details"]["fieldPath"], "operationId");
        }
        let blank = ledger
            .replay::<serde_json::Value>(" ", ProcessOperationKind::CreateBackup, &request)
            .unwrap_err();
        assert_eq!(serde_json::to_value(blank).unwrap()["code"], "VALIDATION");
    }
}
