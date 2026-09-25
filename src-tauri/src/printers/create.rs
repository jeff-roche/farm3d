//! `create_printer` with full options (D8, D9, D13), `probe_connection`
//! (probing without ever touching storage or the credential store), and the
//! `probe_error`/`probe_submission` helpers shared with
//! `test_printer_connection` and `set_printer_connection`'s
//! probe-before-replace check.

use std::sync::Arc;

use tauri::AppHandle;
use zeroize::Zeroizing;

use crate::catalog::resolve::resolve_catalog_ref;
use crate::connections::commands::{credential_coordinator, retry_pending_credential_cleanup};
use crate::connections::credentials::CredentialStoreKind;
use crate::connections::supervisor::ConnectionManager;
use crate::connections::{ConnectionConfig, ConnectionError, ProbeResult, MOONRAKER_KIND};
use crate::contracts::command::{
    CommandError, CommandSuccess, ErrorCode, IncomingContractVersion, JsonValue, RecoveryCode,
};
use crate::contracts::ContractVersion;
use crate::printers::commands::OperationWarning;
use crate::printers::repository::PrinterRepository;
use crate::printers::setup::{supervise_persisted, SupervisionOutcome};
use crate::printers::{
    CatalogRef, LastKnownGood, PrinterProfileOverrides, StartSafety, StoredPrinter,
};
use crate::spools::slots::{InitialLoad, SlotSpec};
use crate::RuntimeServices;

use super::host_identity::canonical_host_identity;

/// What `create_printer` and the batch-create path both build before
/// handing off to [`create_printer_with`].
pub struct CreatePrinterOptions {
    pub name: String,
    pub catalog_ref: CatalogRef,
    pub location: Option<String>,
    pub start_safety: StartSafety,
    pub default_bed_type: Option<String>,
    /// `None` = Profile-only. `Some((config, secret))` — `config` never
    /// carries a `credential_ref`; one is minted only once the secret (if
    /// any) is durably written.
    pub connection: Option<(ConnectionConfig, Option<Zeroizing<String>>)>,
    /// D4/D12: the new Printer's Material Slot layout. Copied at creation —
    /// there is no shared/batch layout entity (D12, user decision 3).
    pub slot_layout: Vec<SlotSpec>,
    /// D12: Spools to load into `slot_layout` positions in the same create
    /// transaction as the layout insert, sharing one generated
    /// `operationId`. Always empty for batch create (D12: "batch never
    /// loads Spools").
    pub initial_loads: Vec<InitialLoad>,
}

pub struct CreateOutcome {
    pub printer: StoredPrinter,
    pub credential_stored: bool,
    pub warnings: Vec<OperationWarning>,
}

/// Trimmed, 1..=128 characters, no control characters. Shared by
/// `create_printer` and `update_printer`.
pub fn validate_name(name: &str) -> Result<String, CommandError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 128 || trimmed.chars().any(char::is_control)
    {
        return Err(CommandError::validation_at(
            "name",
            "The name must be 1-128 characters with no control characters.",
        ));
    }
    Ok(trimmed.to_string())
}

/// Trimmed; an absent or blank value becomes `None`; otherwise at most 128
/// characters with no control characters. Shared by `create_printer` and
/// `update_printer`.
pub fn validate_location(value: Option<&str>) -> Result<Option<String>, CommandError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > 128 || trimmed.chars().any(char::is_control) {
        return Err(CommandError::validation_at(
            "location",
            "The location must be at most 128 characters with no control characters.",
        ));
    }
    Ok(Some(trimmed.to_string()))
}

/// The `ConnectionError -> CommandError` mapping, extracted verbatim from
/// `test_printer_connection` so `probe_connection`, `test_printer_connection`,
/// and the probe-before-replace check in `set_printer_connection` share it.
///
/// `entity` is `Some(id)` for the two call sites that probe an EXISTING
/// Printer (`test_printer_connection`, `set_printer_connection`'s replace
/// check) — matching `test_printer_connection`'s pre-extraction behaviour of
/// attaching an `entityId` detail — and `None` only for `probe_connection`,
/// which has no Printer id to attach. Every other detail
/// (code/message/recovery/retryable, and `adapterKind` on a protocol error)
/// is unchanged either way.
pub fn probe_error(
    error: ConnectionError,
    adapter_kind: &str,
    entity: Option<&str>,
) -> CommandError {
    let (code, message, recovery, retryable) = match error {
        ConnectionError::Unreachable(_) => (
            ErrorCode::PrinterUnreachable,
            "The Printer could not be reached.",
            vec![RecoveryCode::CheckConnection, RecoveryCode::Retry],
            true,
        ),
        ConnectionError::Auth(_) => (
            ErrorCode::AuthenticationFailed,
            "The Printer rejected the credential.",
            vec![
                RecoveryCode::CheckCredentials,
                RecoveryCode::ReenterCredential,
            ],
            false,
        ),
        ConnectionError::Protocol(_) => (
            ErrorCode::ProtocolError,
            "The Printer returned an unexpected response.",
            vec![RecoveryCode::CheckConnection, RecoveryCode::Retry],
            true,
        ),
        ConnectionError::Timeout => (
            ErrorCode::Timeout,
            "The Printer did not respond in time.",
            vec![RecoveryCode::CheckConnection, RecoveryCode::Retry],
            true,
        ),
    };
    match entity {
        Some(id) => {
            CommandError::safe_network(code, message, recovery, retryable, id, adapter_kind)
        }
        None => {
            let mut details = std::collections::BTreeMap::new();
            if code == ErrorCode::ProtocolError {
                details.insert(
                    "adapterKind".to_string(),
                    JsonValue::String(adapter_kind.to_string()),
                );
            }
            CommandError {
                contract_version: ContractVersion::V1,
                code,
                message: message.to_string(),
                recovery,
                retryable,
                correlation_id: None,
                field_errors: None,
                details: if details.is_empty() {
                    None
                } else {
                    Some(details)
                },
            }
        }
    }
}

/// Builds a Connection through the manager's (possibly test-injected)
/// factory and probes it — never storage, never the credential store, never
/// supervision. Shared by `probe_connection`, `test_printer_connection`, and
/// `set_printer_connection`'s probe-before-replace check.
///
/// `entity` is threaded straight through to `probe_error` — `Some(id)` when
/// the caller has a Printer id to attach to a network-error detail (both
/// existing-Printer call sites), `None` for `probe_connection`, which never
/// has one.
pub async fn probe_submission<R: tauri::Runtime>(
    manager: &ConnectionManager<R>,
    config: &ConnectionConfig,
    secret: Option<Zeroizing<String>>,
    entity: Option<&str>,
) -> Result<ProbeResult, CommandError> {
    let adapter_kind = config.kind.clone();
    let connection = manager
        .connection_for(config, secret)
        .ok_or_else(|| CommandError::unsupported_adapter(&adapter_kind))?;
    connection
        .probe()
        .await
        .map_err(|error| probe_error(error, &adapter_kind, entity))
}

/// `probe_connection`: probes a submitted Connection with no Printer id,
/// no storage read/write, no credential-store access, and no supervision
/// side effect (spec D8).
#[tauri::command]
pub async fn probe_connection<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    mut submission: crate::connections::commands::ConnectionSubmission,
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
    let secret = submission
        .api_key
        .take()
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| Zeroizing::new(value.to_string()));
    let config = ConnectionConfig {
        kind: submission.kind,
        host: submission.host,
        port: submission.port,
        use_tls: submission.use_tls,
        credential_ref: None,
    };
    probe_submission(&services.manager, &config, secret, None)
        .await
        .map(CommandSuccess::new)
}

/// Validates, writes the credential (provisional -> secret -> row commit),
/// then supervises. Never starts supervision before commit (spec D8/D9/D13).
pub async fn create_printer_with<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    options: CreatePrinterOptions,
) -> Result<CreateOutcome, CommandError> {
    let name = validate_name(&options.name)?;
    let location = validate_location(options.location.as_deref())?;

    // D12: each initial load must be an active Spool currently in storage
    // (see `spools::repository::is_loadable_from_storage`'s doc comment).
    // Checked here, above the credential-coordinator lock and before any
    // credential-store write, so a rejection never leaves a provisional
    // credential row to clean up — unlike a check placed after the lock is
    // taken, whose `Err` return would skip straight past
    // `retry_pending_credential_cleanup` (see the `drop(_guard)` /
    // `create_result` handling below).
    for (index, load) in options.initial_loads.iter().enumerate() {
        if load.slot_index >= options.slot_layout.len() {
            return Err(CommandError::validation_at(
                format!("initialLoads[{index}].slotIndex"),
                "slotIndex must reference an entry of slotLayout.",
            ));
        }
        let eligible = services
            .storage
            .read_transaction(|tx| {
                Ok(crate::spools::repository::is_loadable_from_storage(
                    tx,
                    &load.spool_id,
                ))
            })
            .and_then(|inner| inner)
            .map_err(|error| CommandError::from_repository(error.into()))?;
        if !eligible {
            return Err(CommandError::validation_at(
                format!("initialLoads[{index}].spoolId"),
                "The Spool must be active and currently in storage.",
            ));
        }
    }

    let (variant, _) = resolve_catalog_ref(&services.catalog, &options.catalog_ref);
    let variant = variant.ok_or_else(|| {
        CommandError::validation_at("catalogRef", "The Printer Profile could not be resolved.")
    })?;
    let last_known_good = Some(LastKnownGood {
        profile: crate::catalog::PrinterProfile::from(variant),
        catalog_version: services.catalog.source_tag.clone(),
        resolved_at: crate::printers::now_rfc3339(),
    });

    let mut overrides = PrinterProfileOverrides::default();
    if let Some(bed_type) = options.default_bed_type.as_deref() {
        if bed_type != variant.default_bed_type.as_str() {
            overrides.default_bed_type = Some(bed_type.to_string());
        }
    }

    let repository = PrinterRepository::new(Arc::clone(&services.storage));
    let mut connection_config: Option<ConnectionConfig> = None;
    let mut submitted_secret: Option<Zeroizing<String>> = None;
    if let Some((config, secret)) = options.connection {
        if config.kind != MOONRAKER_KIND {
            return Err(CommandError::unsupported_adapter(config.kind));
        }
        if config.host.is_empty() {
            return Err(CommandError::validation_at("host", "A host is required."));
        }
        if config.port == 0 {
            return Err(CommandError::validation_at(
                "port",
                "The port must be positive.",
            ));
        }
        crate::connections::reject_tls(config.use_tls)?;
        if let Some(identity) = canonical_host_identity(&config.host, config.port) {
            if let Some(conflicting) = repository
                .find_active_by_host_identity(&identity, None)
                .map_err(|error| CommandError::from_repository(error.into()))?
            {
                return Err(CommandError::duplicate_host(&conflicting));
            }
        }
        connection_config = Some(config);
        submitted_secret = secret;
    }

    // Everything from the provisional-credential row through the Printer
    // row commit happens under the same process-wide lock
    // `coordinate_connection_change` uses, so the two paths never race each
    // other's `pending_credential_cleanup` bookkeeping.
    let _guard = credential_coordinator()
        .lock()
        .map_err(|_| CommandError::internal())?;

    let mut provisional_reference: Option<String> = None;
    let mut credential_stored = false;
    if let (Some(config), Some(secret)) = (connection_config.as_mut(), submitted_secret.as_ref()) {
        let reference = format!("farm3d/credential/{}", uuid::Uuid::new_v4());
        repository
            .enqueue_credential_cleanup(&reference, None, "provisional")
            .map_err(|error| CommandError::from_repository(error.into()))?;
        if services
            .credentials
            .set(&reference, secret.as_str())
            .is_err()
        {
            return Err(CommandError::credential_unavailable(
                match services.credentials.kind() {
                    CredentialStoreKind::Keychain => "keychain",
                    CredentialStoreKind::File => "fallbackFile",
                },
            ));
        }
        config.credential_ref = Some(reference.clone());
        provisional_reference = Some(reference);
        credential_stored = true;
    }

    let printer = StoredPrinter {
        id: PrinterRepository::generate_id(),
        revision: 0,
        name,
        catalog_ref: options.catalog_ref,
        notes: String::new(),
        overrides,
        last_known_good,
        connection: connection_config,
        location,
        start_safety: options.start_safety,
        archived_at: None,
        // Filled by `create_with_layout` before it returns — see that
        // method's doc comment.
        material_slots: Vec::new(),
        created_at: String::new(),
        updated_at: String::new(),
    };

    let create_result = repository.create_with_layout(
        printer,
        provisional_reference.as_deref(),
        &options.slot_layout,
        &options.initial_loads,
    );
    // Drop the coordinator lock before any further credential-store call —
    // `retry_pending_credential_cleanup` (below, on the error path) takes
    // the SAME lock itself, and it is not reentrant.
    drop(_guard);
    let stored = match create_result {
        Ok(stored) => stored,
        Err(error) => {
            // The insert rolled back, so the provisional row (if any) is
            // still there, still unreferenced by any Printer. Try to clean
            // it — and the secret it names — up right away; if that also
            // fails, the row is left for startup cleanup to retry.
            let _ =
                retry_pending_credential_cleanup(&services.storage, services.credentials.as_ref());
            return Err(CommandError::from_repository(error));
        }
    };

    let mut warnings = Vec::new();
    let _reconciliation = services.manager.reconciliation_guard().await;
    if supervise_persisted(
        &services.manager,
        &services.storage,
        services.credentials.as_ref(),
        &services.catalog,
        &stored,
    )
    .await
        == SupervisionOutcome::CredentialRequired
    {
        warnings.push(OperationWarning::credential_required(&stored.id));
    }

    Ok(CreateOutcome {
        printer: stored,
        credential_stored,
        warnings,
    })
}
