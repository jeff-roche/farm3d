//! The `#[tauri::command]` surface for connections. Thin by design — every
//! decision worth testing lives in a sibling module.

use super::capabilities::{self, AdapterCapabilityRow, PrinterCapabilities};
use super::credentials::{
    credential_ref_for, CredentialBackend, CredentialStore, CredentialStoreKind,
};
use super::discovery::{discover, DiscoveredPrinter};
use super::{ConnectionConfig, ProbeResult};
use crate::catalog::resolve::resolve_printer;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::persistence::Storage;
use crate::printers::commands::{OperationWarning, PrinterMutationResult};
use crate::printers::create::probe_submission;
use crate::printers::host_identity::canonical_host_identity;
use crate::printers::repository::PrinterRepository;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::AppHandle;
use ts_rs::TS;
use zeroize::Zeroize;

const DISCOVERY_WINDOW: Duration = Duration::from_secs(3);
const DISCOVERY_HARD_TIMEOUT: Duration = Duration::from_secs(5);

/// `pub(crate)` so `printers::create::create_printer_with` can coordinate
/// its own provisional-credential write under the same process-wide lock
/// `coordinate_connection_change` uses — the two must never interleave, or
/// a provisional row's precedence bookkeeping (`enqueue_credential_cleanup`)
/// could race.
pub(crate) fn credential_coordinator() -> &'static Mutex<()> {
    static COORDINATOR: OnceLock<Mutex<()>> = OnceLock::new();
    COORDINATOR.get_or_init(|| Mutex::new(()))
}

pub fn with_credential_coordination<T>(
    operation: impl FnOnce() -> Result<T, crate::persistence::RepositoryError>,
) -> Result<T, crate::persistence::RepositoryError> {
    let _guard = credential_coordinator().lock().map_err(|_| {
        crate::persistence::RepositoryError::Storage(
            crate::persistence::StorageError::PersistenceUnavailable,
        )
    })?;
    operation()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSubmission<'a> {
    Preserve,
    Clear,
    Replace(&'a str),
}

#[derive(Debug)]
pub struct ConnectionCommit {
    pub printer: crate::printers::StoredPrinter,
    pub cleanup_pending: bool,
}

fn storage_command_error(error: crate::persistence::StorageError) -> CommandError {
    match error {
        crate::persistence::StorageError::UnsupportedSchemaVersion => {
            CommandError::unsupported_schema(crate::persistence::CURRENT_SCHEMA_VERSION)
        }
        crate::persistence::StorageError::MigrationFailed => CommandError::migration_failed(),
        crate::persistence::StorageError::CorruptData { .. } => CommandError::database_corrupt(),
        crate::persistence::StorageError::InvalidSnapshot
        | crate::persistence::StorageError::Database => CommandError::persistence_unavailable(),
        crate::persistence::StorageError::PathCollision
        | crate::persistence::StorageError::PersistenceUnavailable
        | crate::persistence::StorageError::UnsupportedLocking
        | crate::persistence::StorageError::Filesystem
        | crate::persistence::StorageError::OperationFailed => {
            CommandError::persistence_unavailable()
        }
        // Reached only on a write path (insert/replace); every call site
        // here reads through `PrinterRepository::get`, which never produces
        // it. Writes go through `RepositoryError` -> `CommandError::from_repository`
        // instead, which maps this to `ErrorCode::DuplicateHost` properly.
        crate::persistence::StorageError::DuplicateHost(_) => CommandError::internal(),
    }
}

/// Coordinates the SQLite/credential-store replacement protocol under the
/// process-wide credential mutex. The new secret is durable before a Printer
/// can reference it; every uncommitted value remains queued for startup cleanup.
pub fn coordinate_connection_change(
    storage: &Arc<Storage>,
    store: &dyn CredentialBackend,
    id: &str,
    expected_revision: i64,
    mut config: ConnectionConfig,
    submission: CredentialSubmission<'_>,
) -> Result<ConnectionCommit, CommandError> {
    if expected_revision <= 0 {
        return Err(CommandError::validation_at(
            "expectedRevision",
            "The expected revision must be positive.",
        ));
    }
    let _guard = credential_coordinator()
        .lock()
        .map_err(|_| CommandError::internal())?;
    let repository = PrinterRepository::new(Arc::clone(storage));
    let existing = repository
        .get(id)
        .map_err(storage_command_error)?
        .ok_or_else(|| CommandError::not_found(id))?;
    let previous_reference = existing
        .connection
        .as_ref()
        .and_then(|connection| connection.credential_ref.clone());

    let provisional = match submission {
        CredentialSubmission::Preserve => {
            config.credential_ref = previous_reference.clone();
            None
        }
        CredentialSubmission::Clear => {
            config.credential_ref = None;
            None
        }
        CredentialSubmission::Replace(secret) => {
            let reference = format!("farm3d/credential/{}", uuid::Uuid::new_v4());
            repository
                .enqueue_credential_cleanup(&reference, Some(id), "provisional")
                .map_err(storage_command_error)?;
            if store.set(&reference, secret).is_err() {
                return Err(CommandError::credential_unavailable("configured"));
            }
            config.credential_ref = Some(reference.clone());
            Some(reference)
        }
    };

    let updated = repository
        .set_connection(
            id,
            expected_revision,
            Some(config),
            provisional.as_deref(),
            if matches!(submission, CredentialSubmission::Clear) {
                "cleared"
            } else {
                "replaced"
            },
        )
        .map_err(CommandError::from_repository)?;
    let cleanup_pending = previous_reference
        != updated
            .connection
            .as_ref()
            .and_then(|connection| connection.credential_ref.clone())
        && retry_pending_credential_cleanup_locked(storage, store).is_err();
    Ok(ConnectionCommit {
        printer: updated,
        cleanup_pending,
    })
}

#[derive(Deserialize, Clone, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ConnectionSubmission.ts")]
pub struct ConnectionSubmission {
    pub kind: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub use_tls: bool,
    /// `None` = leave the stored credential untouched (the UI never echoes it
    /// back). `Some("")` = clear it. These must stay distinct.
    #[serde(default, rename = "credential")]
    #[ts(optional, rename = "credential")]
    pub api_key: Option<String>,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CredentialStoreInfo.ts")]
pub struct CredentialStoreInfo {
    pub kind: CredentialStoreKind,
    /// Why the keychain was unavailable, so the Connection tab can explain
    /// the fallback rather than silently downgrading.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason_code: Option<String>,
}

/// Splits a submission into what gets persisted and what goes to the
/// credential store. The whole point is that the secret exits here and is
/// never part of the returned config.
pub fn split_submission(
    submission: ConnectionSubmission,
    printer_id: &str,
) -> (ConnectionConfig, Option<String>) {
    let key_ref = credential_ref_for(printer_id);
    let (credential_ref, secret) = match submission.api_key.as_deref() {
        None => (Some(key_ref), None),
        Some(key) if key.trim().is_empty() => (None, None),
        Some(key) => (Some(key_ref), Some(key.trim().to_string())),
    };
    (
        ConnectionConfig {
            kind: submission.kind,
            host: submission.host,
            port: submission.port,
            use_tls: submission.use_tls,
            credential_ref,
        },
        secret,
    )
}

#[cfg(test)]
fn credential_store_if_needed(
    config_dir: &Path,
    needed: bool,
    detect: impl FnOnce(PathBuf) -> CredentialStore,
) -> Option<CredentialStore> {
    needed.then(|| detect(config_dir.to_path_buf()))
}

#[cfg(test)]
fn credential_store_needed(
    secret: &Option<String>,
    credential_to_clear: Option<&str>,
    config: &ConnectionConfig,
) -> bool {
    secret.is_some() || credential_to_clear.is_some() || config.credential_ref.is_some()
}

/// The OLD credential (if any) that must be deleted from the store because
/// the newly split config no longer references one — e.g. the user
/// explicitly cleared their API key (`split_submission` turns `Some("")`
/// into `credential_ref: None`). If the new config still carries a
/// `credential_ref`, nothing needs clearing, even if it happens to name the
/// same key: the "omitted" case round-trips the same ref on purpose.
///
/// Pure over the two refs so this is testable without a Tauri `AppHandle`.
#[cfg(test)]
fn credential_to_clear(
    previous_ref: Option<&str>,
    new_config: &ConnectionConfig,
) -> Option<String> {
    if new_config.credential_ref.is_some() {
        return None;
    }
    previous_ref.map(str::to_string)
}

/// `split_submission` returns the DERIVED credential ref for the "api_key was
/// omitted" case, which means "leave the stored secret alone". That is right
/// when editing an existing connection, but on a first-ever save nothing was
/// ever written: persisting the derived ref would make `printers.json` claim a
/// credential the store has never seen, and the Connection tab would then
/// offer to "keep" a secret that does not exist. Carrying the PREVIOUS ref
/// forward instead is a no-op for the edit case and the truth for a first save.
///
/// Pure so it is testable without a Tauri `AppHandle`.
#[cfg(test)]
fn settle_credential_ref(
    config: &mut ConnectionConfig,
    api_key_omitted: bool,
    previous_ref: Option<&str>,
) {
    if api_key_omitted {
        config.credential_ref = previous_ref.map(str::to_string);
    }
}

#[tauri::command]
pub async fn set_printer_connection<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    expected_revision: i64,
    id: String,
    mut submission: ConnectionSubmission,
    accept_unverified: Option<bool>,
) -> Result<CommandSuccess<PrinterMutationResult>, CommandError> {
    contract_version.validate()?;
    if expected_revision <= 0 {
        return Err(CommandError::validation_at(
            "expectedRevision",
            "The expected revision must be positive.",
        ));
    }
    if submission.host.is_empty() {
        return Err(CommandError::validation_at("host", "A host is required."));
    }
    if submission.port == 0 {
        return Err(CommandError::validation_at(
            "port",
            "The port must be positive.",
        ));
    }
    crate::connections::reject_tls(submission.use_tls)?;
    if !super::is_supported_kind(&submission.kind) {
        return Err(CommandError::unsupported_adapter(submission.kind));
    }
    let services = bootstrap.ready()?;
    let repository = PrinterRepository::new(Arc::clone(&services.storage));
    let existing = repository
        .get(&id)
        .map_err(storage_command_error)?
        .ok_or_else(|| CommandError::not_found(&id))?;
    if existing.revision != expected_revision {
        return Err(CommandError::revision_conflict(
            &id,
            expected_revision,
            existing.revision,
        ));
    }
    // D3: reject a host another active Printer owns BEFORE probing or
    // writing any secret (same precheck as `create_printer_with`). An
    // archived Printer holds no host (D6), so it is not checked here; the
    // repository write still enforces the invariant on commit.
    if existing.archived_at.is_none() {
        if let Some(identity) = canonical_host_identity(&submission.host, submission.port) {
            if let Some(conflicting) = repository
                .find_active_by_host_identity(&identity, Some(&id))
                .map_err(storage_command_error)?
            {
                return Err(CommandError::duplicate_host(&conflicting));
            }
        }
    }
    let previous_credential_ref = existing
        .connection
        .as_ref()
        .and_then(|c| c.credential_ref.clone());
    let submitted_secret = submission.api_key.take().map(zeroize::Zeroizing::new);
    let credential_store = (submitted_secret
        .as_ref()
        .map(|value| value.as_str())
        .is_some_and(|value| !value.is_empty())
        || previous_credential_ref.is_some())
    .then(|| Arc::clone(&services.credentials));
    let config = ConnectionConfig {
        kind: submission.kind,
        host: submission.host,
        port: submission.port,
        use_tls: submission.use_tls,
        credential_ref: None,
    };
    let change = match submitted_secret.as_ref().map(|value| value.as_str()) {
        None => CredentialSubmission::Preserve,
        Some("") => CredentialSubmission::Clear,
        Some(secret) => CredentialSubmission::Replace(secret),
    };
    // D8: replacing an already-present Connection is probed BEFORE it is
    // persisted, unless the caller opts out with `acceptUnverified`. First-
    // time sets and clears never reach here because `differs` is only
    // evaluated against an existing Connection.
    if let Some(existing_connection) = existing.connection.clone() {
        let new_secret_submitted = matches!(change, CredentialSubmission::Replace(_));
        let differs = existing_connection.kind != config.kind
            || existing_connection.host != config.host
            || existing_connection.port != config.port
            || existing_connection.use_tls != config.use_tls
            || new_secret_submitted;
        if differs && accept_unverified != Some(true) {
            let probe_secret = match change {
                CredentialSubmission::Replace(secret) => {
                    Some(zeroize::Zeroizing::new(secret.to_string()))
                }
                CredentialSubmission::Clear => None,
                CredentialSubmission::Preserve => match previous_credential_ref.as_deref() {
                    Some(reference) => Some(zeroize::Zeroizing::new(
                        services
                            .credentials
                            .get(reference)
                            .map_err(|_| {
                                CommandError::credential_unavailable(
                                    match services.credentials.kind() {
                                        CredentialStoreKind::Keychain => "keychain",
                                        CredentialStoreKind::File => "fallbackFile",
                                    },
                                )
                            })?
                            .ok_or_else(|| CommandError::credential_required(&id))?,
                    )),
                    None => None,
                },
            };
            probe_submission(&services.manager, &config, probe_secret, Some(&id)).await?;
        }
    }
    let no_store =
        CredentialStore::file_backed(std::env::temp_dir().join("farm3d-unused-credential-store"));
    let committed = coordinate_connection_change(
        &services.storage,
        credential_store
            .as_ref()
            .map(|store| store.as_ref() as &dyn CredentialBackend)
            .unwrap_or(&no_store),
        &id,
        expected_revision,
        config,
        change,
    )?;
    let updated = committed.printer;
    let _reconciliation = services.manager.reconciliation_guard().await;
    let mut warnings = Vec::new();
    if crate::printers::setup::supervise_persisted(
        &services.manager,
        &services.storage,
        services.credentials.as_ref(),
        &services.catalog,
        &updated,
    )
    .await
        == crate::printers::setup::SupervisionOutcome::CredentialRequired
    {
        warnings.push(OperationWarning::credential_required(&id));
    }
    if committed.cleanup_pending {
        warnings.push(OperationWarning::cleanup());
    }
    Ok(CommandSuccess::new(PrinterMutationResult {
        printer: resolve_printer(&services.catalog, &updated),
        warnings,
    }))
}

#[tauri::command]
pub async fn clear_printer_connection<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    expected_revision: i64,
    id: String,
) -> Result<CommandSuccess<PrinterMutationResult>, CommandError> {
    contract_version.validate()?;
    if expected_revision <= 0 {
        return Err(CommandError::validation_at(
            "expectedRevision",
            "The expected revision must be positive.",
        ));
    }
    let services = bootstrap.ready()?;
    let repository = PrinterRepository::new(Arc::clone(&services.storage));
    let existing = repository
        .get(&id)
        .map_err(storage_command_error)?
        .ok_or_else(|| CommandError::not_found(&id))?;
    if existing.revision != expected_revision {
        return Err(CommandError::revision_conflict(
            &id,
            expected_revision,
            existing.revision,
        ));
    }
    let old = existing
        .connection
        .as_ref()
        .and_then(|connection| connection.credential_ref.clone());
    if existing.connection.is_none() {
        return Err(CommandError::revision_conflict(
            &id,
            expected_revision,
            existing.revision,
        ));
    }
    let updated = with_credential_coordination(|| {
        repository.set_connection(&id, expected_revision, None, None, "cleared")
    })
    .map_err(CommandError::from_repository)?;
    let mut warnings = Vec::new();
    let _reconciliation = services.manager.reconciliation_guard().await;
    // Reconcile against the PERSISTED row (a concurrent archive/delete/set
    // may have committed since ours). D6: an archived Printer never gets a
    // live status — drop its snapshot and publish removal instead.
    let current = repository
        .get(&id)
        .ok()
        .unwrap_or_else(|| Some(updated.clone()));
    let graceful = match current {
        None => services.manager.stop(&id).await,
        Some(current) if current.archived_at.is_some() => {
            services.manager.discard_connection(&id).await
        }
        Some(current) if current.connection.is_some() => !matches!(
            crate::printers::setup::supervise_printer(
                &services.manager,
                services.credentials.as_ref(),
                &services.catalog,
                &current,
            )
            .await,
            crate::printers::setup::SupervisionOutcome::Archived(false)
                | crate::printers::setup::SupervisionOutcome::Deleted(false)
        ),
        Some(current) => {
            let (facts, _) =
                crate::printers::setup::derive_setup_facts(&current, &services.catalog);
            services
                .manager
                .clear_connection(&id, facts.profile_resolved)
                .await
        }
    };
    if !graceful {
        warnings.push(OperationWarning::supervisor(&id));
    }
    if let Some(reference) = old.as_deref() {
        let credential_store = Arc::clone(&services.credentials);
        let _ = reference;
        if retry_pending_credential_cleanup(&services.storage, credential_store.as_ref()).is_err() {
            warnings.push(OperationWarning::cleanup());
        }
    }
    Ok(CommandSuccess::new(PrinterMutationResult {
        printer: resolve_printer(&services.catalog, &updated),
        warnings,
    }))
}

/// Probes the SUBMITTED config, not the stored one, so "Test connection"
/// validates what the user typed before they commit it.
#[tauri::command]
pub async fn test_printer_connection<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    id: String,
    mut submission: ConnectionSubmission,
) -> Result<CommandSuccess<ProbeResult>, CommandError> {
    contract_version.validate()?;
    if submission.host.is_empty() {
        return Err(CommandError::validation_at("host", "A host is required."));
    }
    if submission.port == 0 {
        return Err(CommandError::validation_at(
            "port",
            "The port must be positive.",
        ));
    }
    crate::connections::reject_tls(submission.use_tls)?;
    let services = bootstrap.ready()?;
    let existing = PrinterRepository::new(Arc::clone(&services.storage))
        .get(&id)
        .map_err(storage_command_error)?
        .ok_or_else(|| CommandError::not_found(&id))?;
    let omitted = submission.api_key.is_none();
    let explicit = submission
        .api_key
        .take()
        .map(zeroize::Zeroizing::new)
        .map(|mut value| {
            let trimmed = value.trim().to_string();
            value.zeroize();
            zeroize::Zeroizing::new(trimmed)
        });
    let committed_ref = if omitted {
        existing
            .connection
            .and_then(|connection| connection.credential_ref)
    } else {
        None
    };
    let config = ConnectionConfig {
        kind: submission.kind,
        host: submission.host,
        port: submission.port,
        use_tls: submission.use_tls,
        credential_ref: committed_ref.clone(),
    };
    let api_key = match explicit {
        Some(value) if value.is_empty() => None,
        Some(value) => Some(value),
        None => match committed_ref {
            Some(reference) => Some(zeroize::Zeroizing::new(
                services
                    .credentials
                    .get(&reference)
                    .map_err(|_| {
                        CommandError::credential_unavailable(match services.credentials.kind() {
                            CredentialStoreKind::Keychain => "keychain",
                            CredentialStoreKind::File => "fallbackFile",
                        })
                    })?
                    .ok_or_else(|| CommandError::credential_required(&id))?,
            )),
            None => None,
        },
    };
    probe_submission(&services.manager, &config, api_key, Some(&id))
        .await
        .map(CommandSuccess::new)
}

#[tauri::command]
pub async fn credential_store_info<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<CredentialStoreInfo>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let store = &services.credentials;
    Ok(CommandSuccess::new(CredentialStoreInfo {
        kind: store.kind(),
        reason_code: store.unavailable_reason_code().map(str::to_string),
    }))
}

#[tauri::command]
pub async fn discover_printers<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<Vec<DiscoveredPrinter>>, CommandError> {
    contract_version.validate()?;
    let _services = bootstrap.ready()?;
    // mdns-sd's receiver is blocking, so this belongs on the blocking pool
    // rather than parked on an async worker for three seconds.
    let worker = async {
        tauri::async_runtime::spawn_blocking(|| discover(DISCOVERY_WINDOW))
            .await
            .map_err(|_| CommandError::internal())?
            .map_err(|_| CommandError::internal())
    };
    await_discovery(worker, DISCOVERY_HARD_TIMEOUT)
        .await
        .map(CommandSuccess::new)
}

async fn await_discovery<F>(
    worker: F,
    hard_timeout: Duration,
) -> Result<Vec<DiscoveredPrinter>, CommandError>
where
    F: std::future::Future<Output = Result<Vec<DiscoveredPrinter>, CommandError>>,
{
    tokio::time::timeout(hard_timeout, worker)
        .await
        .map_err(|_| CommandError::discovery_timeout())?
}

/// Stays synchronous deliberately: this reads only the in-memory `StatusMap`
/// (a mutex lock and a clone), so there is nothing here that can block the
/// main thread the way the credential-store commands can.
#[tauri::command]
pub fn printer_statuses<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<crate::connections::supervisor::PrinterStatusBackfill>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    Ok(CommandSuccess::new(services.manager.status_backfill()))
}

/// D6. `hostFacts` is always `None` for now: no adapter has a `host_state`
/// builder yet (Task 8 adds Moonraker's), so no host rule can apply and
/// every capability comes back `notVerified` at best.
#[tauri::command]
pub fn printer_capabilities<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    printer_id: String,
) -> Result<CommandSuccess<PrinterCapabilities>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let printer = PrinterRepository::new(Arc::clone(&services.storage))
        .get(&printer_id)
        .map_err(storage_command_error)?
        .ok_or_else(|| CommandError::not_found(&printer_id))?;
    Ok(CommandSuccess::new(capabilities::capabilities_for(
        &printer, None,
    )))
}

/// D6 "Rows at the end of P6": the registry's own capabilities, in
/// registry order, with no Printer or host behind them.
#[tauri::command]
pub fn adapter_capability_matrix<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<Vec<AdapterCapabilityRow>>, CommandError> {
    contract_version.validate()?;
    let _services = bootstrap.ready()?;
    Ok(CommandSuccess::new(
        capabilities::adapter_capability_matrix(),
    ))
}

/// Retries cleanup work that is safe to perform automatically. Imported
/// orphan references are deliberately retained until a user resolves them.
pub fn retry_pending_credential_cleanup(
    storage: &Storage,
    store: &dyn CredentialBackend,
) -> Result<(), String> {
    let _guard = credential_coordinator()
        .lock()
        .map_err(|_| "credential coordinator unavailable")?;
    retry_pending_credential_cleanup_locked(storage, store)
}

fn retry_pending_credential_cleanup_locked(
    storage: &Storage,
    store: &dyn CredentialBackend,
) -> Result<(), String> {
    let pending = storage
        .read(|connection| {
            let mut statement = connection.prepare(
                "SELECT credential_ref FROM pending_credential_cleanup WHERE reason != 'import_orphan' ORDER BY credential_ref",
            )?;
            let references = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(references)
        })
        .map_err(|error| error.to_string())?;
    for reference in pending {
        let reachable = storage.read(|connection| connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM printers WHERE json_extract(connection_json, '$.credentialRef') = ?1)",
            [&reference], |row| row.get::<_, bool>(0)
        )).map_err(|error| error.to_string())?;
        if reachable {
            continue;
        }
        if store.delete(&reference).is_err() {
            storage.write(|transaction| {
                transaction.execute("UPDATE pending_credential_cleanup SET attempt_count=attempt_count+1, last_error_code='CREDENTIAL_UNAVAILABLE', last_attempt_at=?2 WHERE credential_ref=?1", rusqlite::params![reference, crate::printers::now_rfc3339()])?;
                Ok(())
            }).map_err(|storage_error| storage_error.to_string())?;
            return Err("credential cleanup unavailable".to_string());
        }
        storage
            .write(|transaction| {
                transaction.execute(
                    "DELETE FROM pending_credential_cleanup WHERE credential_ref = ?1",
                    [&reference],
                )?;
                Ok(())
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::MOONRAKER_KIND;
    use crate::contracts::command::ErrorCode;
    use std::cell::Cell;
    use std::path::Path;

    #[tokio::test]
    async fn discovery_worker_failure_is_internal() {
        let error = await_discovery(
            async { Err(CommandError::internal()) },
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::Internal);
    }

    #[tokio::test]
    async fn discovery_hard_guard_is_timeout() {
        let error = await_discovery(
            std::future::pending::<Result<Vec<DiscoveredPrinter>, CommandError>>(),
            Duration::from_millis(1),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::Timeout);
        assert!(error.retryable);
    }

    #[test]
    fn startup_cleanup_deletes_eligible_orphans_and_retains_import_orphans() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::persistence::StoragePaths::new(
            temp.path().join("metadata"),
            temp.path().join("data"),
        )
        .unwrap();
        let lease = crate::persistence::MetadataRootLease::acquire(&paths).unwrap();
        let storage = crate::persistence::Storage::open(paths, &lease).unwrap();
        let store = CredentialStore::file_backed(temp.path().join("credentials"));
        store.set("eligible", "DELETE_ME").unwrap();
        store.set("imported", "KEEP_ME").unwrap();
        storage.write(|transaction| {
            transaction.execute("INSERT INTO pending_credential_cleanup(credential_ref, reason, created_at) VALUES ('eligible','provisional','now'), ('imported','import_orphan','now')", [])?;
            Ok(())
        }).unwrap();

        retry_pending_credential_cleanup(&storage, &store).unwrap();

        assert_eq!(store.get("eligible").unwrap(), None);
        assert_eq!(store.get("imported").unwrap().as_deref(), Some("KEEP_ME"));
        let rows = storage
            .read(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM pending_credential_cleanup",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn credential_free_first_save_does_not_detect_a_store() {
        let submission = ConnectionSubmission {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: 7125,
            use_tls: false,
            api_key: Some(String::new()),
        };
        let api_key_omitted = submission.api_key.is_none();
        let (mut config, secret) = split_submission(submission, "prn-1");
        settle_credential_ref(&mut config, api_key_omitted, None);
        let credential_to_clear = credential_to_clear(None, &config);
        let detections = Cell::new(0);

        let store = credential_store_if_needed(
            Path::new("unused"),
            credential_store_needed(&secret, credential_to_clear.as_deref(), &config),
            |dir| {
                detections.set(detections.get() + 1);
                CredentialStore::file_backed(dir)
            },
        );

        assert_eq!((store.is_none(), detections.get()), (true, 0));
    }

    #[test]
    fn a_submitted_api_key_never_lands_in_the_persisted_config() {
        let (config, secret) = split_submission(
            ConnectionSubmission {
                kind: MOONRAKER_KIND.to_string(),
                host: "voron.local".to_string(),
                port: 7125,
                use_tls: false,
                api_key: Some("s3cret".to_string()),
            },
            "prn-1",
        );

        assert_eq!(secret.as_deref(), Some("s3cret"));
        assert_eq!(
            config.credential_ref.as_deref(),
            Some("farm3d/printer/prn-1/apikey")
        );
        let json = serde_json::to_string(&config).unwrap();
        assert!(
            !json.contains("s3cret"),
            "the secret leaked into the persisted config"
        );
    }

    #[test]
    fn an_empty_api_key_stores_no_reference_at_all() {
        // Trusted-LAN Moonraker instances need no key; a dangling
        // credentialRef pointing at nothing would make the Connection tab
        // claim a credential exists when none does.
        let (config, secret) = split_submission(
            ConnectionSubmission {
                kind: MOONRAKER_KIND.to_string(),
                host: "voron.local".to_string(),
                port: 7125,
                use_tls: false,
                api_key: Some("   ".to_string()),
            },
            "prn-1",
        );
        assert_eq!(secret, None);
        assert_eq!(config.credential_ref, None);
    }

    #[test]
    fn omitting_the_api_key_field_preserves_the_existing_reference() {
        // The UI never echoes a stored secret back, so "no api_key in this
        // submission" must mean "leave the stored one alone", not "clear it".
        let (config, secret) = split_submission(
            ConnectionSubmission {
                kind: MOONRAKER_KIND.to_string(),
                host: "voron.local".to_string(),
                port: 7125,
                use_tls: false,
                api_key: None,
            },
            "prn-1",
        );
        assert_eq!(secret, None);
        assert_eq!(
            config.credential_ref.as_deref(),
            Some("farm3d/printer/prn-1/apikey")
        );
    }

    #[test]
    fn a_submitted_api_key_is_trimmed_before_it_is_stored() {
        // A pasted key with accidental leading/trailing whitespace must not
        // be stored verbatim — it would fail as an `X-Api-Key` header value
        // against the real printer with no indication why.
        let (config, secret) = split_submission(
            ConnectionSubmission {
                kind: MOONRAKER_KIND.to_string(),
                host: "voron.local".to_string(),
                port: 7125,
                use_tls: false,
                api_key: Some("  s3cret  \n".to_string()),
            },
            "prn-1",
        );
        assert_eq!(secret.as_deref(), Some("s3cret"));
        assert_eq!(
            config.credential_ref.as_deref(),
            Some("farm3d/printer/prn-1/apikey")
        );
    }

    fn moonraker_config(credential_ref: Option<&str>) -> ConnectionConfig {
        ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: 7125,
            use_tls: false,
            credential_ref: credential_ref.map(str::to_string),
        }
    }

    #[test]
    fn clearing_the_credential_ref_forgets_the_old_one() {
        // The scenario from the orphaned-secret bug: the new config has no
        // credential_ref (an explicit clear), so the OLD ref must be
        // returned for deletion — otherwise the secret it named survives in
        // the store forever with nothing left pointing at it.
        let new_config = moonraker_config(None);
        assert_eq!(
            credential_to_clear(Some("farm3d/printer/prn-1/apikey"), &new_config),
            Some("farm3d/printer/prn-1/apikey".to_string())
        );
    }

    #[test]
    fn keeping_a_credential_ref_forgets_nothing() {
        // Whether the ref is unchanged (the "omitted api_key" case) or is a
        // fresh key for the same printer, the new config still names a
        // credential — there is nothing to delete.
        let new_config = moonraker_config(Some("farm3d/printer/prn-1/apikey"));
        assert_eq!(
            credential_to_clear(Some("farm3d/printer/prn-1/apikey"), &new_config),
            None
        );
    }

    #[test]
    fn there_was_never_a_credential_to_forget() {
        let new_config = moonraker_config(None);
        assert_eq!(credential_to_clear(None, &new_config), None);
    }

    #[test]
    fn a_first_save_with_no_api_key_persists_no_credential_ref() {
        // The UI reads `credentialRef` as "a credential is stored". On a
        // first-ever save with the key field left blank nothing was written,
        // so the ref split_submission derived must NOT be persisted —
        // otherwise the Connection tab offers to "keep" a secret that the
        // credential store has never seen.
        let mut config = moonraker_config(Some("farm3d/printer/prn-1/apikey"));
        settle_credential_ref(&mut config, true, None);
        assert_eq!(config.credential_ref, None);
    }

    #[test]
    fn editing_with_no_api_key_keeps_the_stored_credential_ref() {
        // The other half of the same rule: an existing connection whose
        // secret is untouched must keep pointing at it.
        let mut config = moonraker_config(Some("farm3d/printer/prn-1/apikey"));
        settle_credential_ref(&mut config, true, Some("farm3d/printer/prn-1/apikey"));
        assert_eq!(
            config.credential_ref.as_deref(),
            Some("farm3d/printer/prn-1/apikey")
        );
    }

    #[test]
    fn a_submitted_api_key_settles_on_its_own_ref_regardless_of_history() {
        // A key WAS submitted, so a credential is genuinely being written:
        // the derived ref stands whether or not one was stored before.
        let mut config = moonraker_config(Some("farm3d/printer/prn-1/apikey"));
        settle_credential_ref(&mut config, false, None);
        assert_eq!(
            config.credential_ref.as_deref(),
            Some("farm3d/printer/prn-1/apikey")
        );
    }
}
