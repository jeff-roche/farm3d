use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use ts_rs::TS;

use crate::catalog::resolve::ResolvedPrinter;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::document_io::DocumentKind;
use crate::persistence::SnapshotKind;

use super::repository::PrinterRepository;
use super::{CatalogRef, PrinterPatch};

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "command/PrinterMutationResult.ts"
)]
pub struct PrinterMutationResult {
    pub printer: ResolvedPrinter,
    pub warnings: Vec<OperationWarning>,
}

#[derive(Clone, Copy, Debug, Serialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(
    rename_all = "SCREAMING_SNAKE_CASE",
    export_to = "command/OperationWarningCode.ts"
)]
pub enum OperationWarningCode {
    CredentialCleanupPending,
    CredentialRequired,
    SupervisorReconciliationFailed,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/OperationWarning.ts")]
pub struct OperationWarning {
    pub code: OperationWarningCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub entity_id: Option<String>,
}

impl OperationWarning {
    pub fn cleanup() -> Self {
        Self {
            code: OperationWarningCode::CredentialCleanupPending,
            entity_id: None,
        }
    }
    pub fn credential_required(id: &str) -> Self {
        Self {
            code: OperationWarningCode::CredentialRequired,
            entity_id: Some(id.to_string()),
        }
    }
    pub fn supervisor(id: &str) -> Self {
        Self {
            code: OperationWarningCode::SupervisorReconciliationFailed,
            entity_id: Some(id.to_string()),
        }
    }
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename = "DeletePrinterData",
    rename_all = "camelCase",
    export_to = "command/DeletePrinterData.ts"
)]
pub struct DeletePrinterResult {
    pub deleted_id: String,
    #[ts(type = "number")]
    pub deleted_revision: i64,
    pub credential_cleanup_pending: bool,
    pub warnings: Vec<OperationWarning>,
}

fn mutation(printer: ResolvedPrinter) -> CommandSuccess<PrinterMutationResult> {
    CommandSuccess::new(PrinterMutationResult {
        printer,
        warnings: Vec::new(),
    })
}

#[tauri::command]
pub fn list_printers<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<Vec<ResolvedPrinter>>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let records = PrinterRepository::new(Arc::clone(&services.storage))
        .list()
        .map_err(|error| {
            CommandError::from_repository(crate::persistence::RepositoryError::Storage(error))
        })?;
    Ok(CommandSuccess::new(
        records
            .iter()
            .map(|printer| crate::catalog::resolve::resolve_printer(&services.catalog, printer))
            .collect(),
    ))
}

#[tauri::command]
pub fn create_printer<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    name: String,
    catalog_ref: CatalogRef,
) -> Result<CommandSuccess<PrinterMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let catalog = &services.catalog;
    let (variant, _) = crate::catalog::resolve::resolve_catalog_ref(catalog, &catalog_ref);
    let variant = variant.ok_or_else(|| {
        CommandError::validation_at("catalogRef", "The Printer Profile could not be resolved.")
    })?;
    let stored = super::StoredPrinter {
        id: PrinterRepository::generate_id(),
        name,
        catalog_ref,
        notes: String::new(),
        overrides: super::PrinterProfileOverrides::default(),
        last_known_good: Some(super::LastKnownGood {
            profile: crate::catalog::PrinterProfile::from(variant),
            catalog_version: catalog.source_tag.clone(),
            resolved_at: crate::printers::now_rfc3339(),
        }),
        connection: None,
        ..Default::default()
    };
    let stored = PrinterRepository::new(Arc::clone(&services.storage))
        .create(stored)
        .map_err(CommandError::from_repository)?;
    services.manager.reconcile_printer(
        &stored.id,
        crate::connections::supervisor::PrinterSetupFacts {
            has_usable_connection: false,
            profile_resolved: true,
        },
    );
    Ok(mutation(crate::catalog::resolve::resolve_printer(
        catalog, &stored,
    )))
}

#[tauri::command]
pub fn update_printer<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    expected_revision: i64,
    id: String,
    patch: PrinterPatch,
) -> Result<CommandSuccess<PrinterMutationResult>, CommandError> {
    contract_version.validate()?;
    if patch.name.is_none() && patch.notes.is_none() {
        return Err(CommandError::validation_at(
            "patch",
            "At least one field is required.",
        ));
    }
    let services = bootstrap.ready()?;
    let updated = PrinterRepository::new(Arc::clone(&services.storage))
        .update(&id, expected_revision, |printer| {
            if let Some(name) = patch.name {
                printer.name = name;
            }
            if let Some(notes) = patch.notes {
                printer.notes = notes;
            }
        })
        .map_err(CommandError::from_repository)?;
    Ok(mutation(crate::catalog::resolve::resolve_printer(
        &services.catalog,
        &updated,
    )))
}

#[tauri::command]
pub async fn delete_printer<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    expected_revision: i64,
    id: String,
) -> Result<CommandSuccess<DeletePrinterResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let deleted = crate::connections::commands::with_credential_coordination(|| {
        PrinterRepository::new(Arc::clone(&services.storage)).delete(&id, expected_revision)
    })
    .map_err(CommandError::from_repository)?;
    let mut warnings = Vec::new();
    let _reconciliation = services.manager.reconciliation_guard().await;
    if !services.manager.stop_and_wait(&id).await {
        warnings.push(OperationWarning::supervisor(&id));
    }
    let mut credential_cleanup_pending = false;
    if deleted
        .connection
        .as_ref()
        .and_then(|connection| connection.credential_ref.as_ref())
        .is_some()
    {
        if crate::connections::commands::retry_pending_credential_cleanup(
            &services.storage,
            services.credentials.as_ref(),
        )
        .is_err()
        {
            credential_cleanup_pending = true;
            warnings.push(OperationWarning::cleanup());
        }
    }
    Ok(CommandSuccess::new(DeletePrinterResult {
        deleted_id: id,
        deleted_revision: expected_revision,
        credential_cleanup_pending,
        warnings,
    }))
}

#[tauri::command]
pub fn set_printer_override<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    expected_revision: i64,
    id: String,
    field: String,
    value: Option<crate::contracts::command::JsonValue>,
) -> Result<CommandSuccess<PrinterMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let repository = PrinterRepository::new(Arc::clone(&services.storage));
    let existing = repository
        .get(&id)
        .map_err(|error| CommandError::from_repository(error.into()))?
        .ok_or_else(|| CommandError::not_found(&id))?;
    let overrides = super::apply_override(
        &existing.overrides,
        &field,
        value.map(crate::contracts::command::JsonValue::into_serde_value),
    )
    .map_err(|message| CommandError::validation_at(&field, message))?;
    let updated = repository
        .update(&id, expected_revision, |printer| {
            printer.overrides = overrides;
        })
        .map_err(CommandError::from_repository)?;
    Ok(mutation(crate::catalog::resolve::resolve_printer(
        &services.catalog,
        &updated,
    )))
}

#[tauri::command]
pub fn rebind_printer<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    expected_revision: i64,
    id: String,
    catalog_ref: CatalogRef,
) -> Result<CommandSuccess<PrinterMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let catalog = &services.catalog;
    let (variant, _) = crate::catalog::resolve::resolve_catalog_ref(catalog, &catalog_ref);
    let variant = variant.ok_or_else(|| {
        CommandError::validation_at("catalogRef", "The Printer Profile could not be resolved.")
    })?;
    let baseline = super::LastKnownGood {
        profile: crate::catalog::PrinterProfile::from(variant),
        catalog_version: catalog.source_tag.clone(),
        resolved_at: crate::printers::now_rfc3339(),
    };
    let updated = PrinterRepository::new(Arc::clone(&services.storage))
        .update(&id, expected_revision, |printer| {
            printer.catalog_ref = catalog_ref;
            printer.last_known_good = Some(baseline);
        })
        .map_err(CommandError::from_repository)?;
    Ok(mutation(crate::catalog::resolve::resolve_printer(
        catalog, &updated,
    )))
}

#[tauri::command]
pub fn resolve_profile_drift<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    expected_revision: i64,
    id: String,
    action: String,
) -> Result<CommandSuccess<PrinterMutationResult>, CommandError> {
    contract_version.validate()?;
    if action != "accept" && action != "pin" {
        return Err(CommandError::validation_at(
            "action",
            "action must be accept or pin.",
        ));
    }
    let services = bootstrap.ready()?;
    let catalog = &services.catalog;
    let existing = PrinterRepository::new(Arc::clone(&services.storage))
        .get(&id)
        .map_err(|error| CommandError::from_repository(error.into()))?
        .ok_or_else(|| CommandError::not_found(&id))?;
    if existing.revision != expected_revision {
        return Err(CommandError::revision_conflict(
            &id,
            expected_revision,
            existing.revision,
        ));
    }
    let resolved = crate::catalog::resolve::resolve_printer(catalog, &existing);
    if resolved.profile_resolution.profile_drift.is_empty() {
        return Err(CommandError::revision_conflict(
            &id,
            expected_revision,
            existing.revision,
        ));
    }
    let pinned_overrides = (action == "pin")
        .then(|| {
            super::pin_drift(
                &existing.overrides,
                &resolved.profile_resolution.profile_drift,
            )
        })
        .transpose()
        .map_err(|message| CommandError::validation_at("profileDrift", message))?;
    let updated = PrinterRepository::new(Arc::clone(&services.storage))
        .update(&id, expected_revision, |printer| {
            if action == "accept" {
                if let (Some(variant), _) =
                    crate::catalog::resolve::resolve_catalog_ref(catalog, &printer.catalog_ref)
                {
                    printer.last_known_good = Some(super::LastKnownGood {
                        profile: crate::catalog::PrinterProfile::from(variant),
                        catalog_version: catalog.source_tag.clone(),
                        resolved_at: crate::printers::now_rfc3339(),
                    });
                }
            } else if let Some(overrides) = pinned_overrides {
                printer.overrides = overrides;
            }
        })
        .map_err(CommandError::from_repository)?;
    Ok(mutation(crate::catalog::resolve::resolve_printer(
        catalog, &updated,
    )))
}

#[derive(Serialize, TS)]
#[serde(tag = "status", rename_all = "camelCase")]
#[ts(
    tag = "status",
    rename_all = "camelCase",
    rename = "PrintersExportOutcome",
    export_to = "command/PrintersExportOutcome.ts"
)]
pub enum ExportResult {
    Cancelled,
    Exported {
        #[serde(rename = "exportedAt")]
        #[ts(rename = "exportedAt")]
        exported_at: String,
        #[serde(rename = "recordCount")]
        #[ts(rename = "recordCount")]
        record_count: usize,
    },
    Unsupported {
        #[ts(type = "\"desktopRequired\"")]
        reason: crate::contracts::command::DesktopRequiredReason,
    },
}

#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "command/PrinterRevisionPrecondition.ts"
)]
pub struct PrinterRevisionPrecondition {
    id: String,
    #[ts(type = "number")]
    revision: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrintersDocument {
    #[allow(dead_code)]
    schema_version: i64,
    #[allow(dead_code)]
    exported_at: String,
    printers: Vec<super::StoredPrinter>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportPrintersDocument<'a> {
    schema_version: u8,
    exported_at: &'a str,
    printers: &'a [serde_json::Value],
}

#[derive(Serialize, TS)]
#[serde(tag = "status", rename_all = "camelCase")]
#[ts(
    tag = "status",
    rename_all = "camelCase",
    rename = "PrintersImportOutcome",
    export_to = "command/PrintersImportOutcome.ts"
)]
pub enum PrintersImportResult {
    Cancelled,
    Applied {
        printers: Vec<ResolvedPrinter>,
        #[serde(rename = "createdCount")]
        #[ts(rename = "createdCount")]
        created_count: usize,
        #[serde(rename = "updatedCount")]
        #[ts(rename = "updatedCount")]
        updated_count: usize,
        #[serde(rename = "deletedCount")]
        #[ts(rename = "deletedCount")]
        deleted_count: usize,
        warnings: Vec<OperationWarning>,
    },
    Unsupported {
        #[ts(type = "\"desktopRequired\"")]
        reason: crate::contracts::command::DesktopRequiredReason,
    },
}

#[tauri::command]
pub async fn export_printers<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<ExportResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let Some(path) = services.documents.save_json(DocumentKind::Printers)? else {
        return Ok(CommandSuccess::new(ExportResult::Cancelled));
    };
    let printers = PrinterRepository::new(Arc::clone(&services.storage))
        .list()
        .map_err(|error| CommandError::from_repository(error.into()))?;
    let exported_at = crate::printers::now_rfc3339();
    let exported = printers
        .iter()
        .map(export_printer)
        .collect::<Result<Vec<_>, _>>()?;
    let bytes = serde_json::to_vec_pretty(&ExportPrintersDocument {
        schema_version: 1,
        exported_at: &exported_at,
        printers: &exported,
    })
    .map_err(|_| CommandError::internal())?;
    services.documents.atomic_write(&path, &bytes)?;
    Ok(CommandSuccess::new(ExportResult::Exported {
        exported_at,
        record_count: printers.len(),
    }))
}

#[tauri::command]
pub async fn import_printers<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    expected_revisions: Vec<PrinterRevisionPrecondition>,
) -> Result<CommandSuccess<PrintersImportResult>, CommandError> {
    contract_version.validate()?;
    let expected: Vec<_> = expected_revisions
        .into_iter()
        .map(|value| (value.id, value.revision))
        .collect();
    if expected.iter().any(|(_, revision)| *revision <= 0)
        || expected
            .windows(2)
            .any(|pair| pair[0].0.as_bytes() >= pair[1].0.as_bytes())
    {
        return Err(CommandError::validation_at(
            "expectedRevisions",
            "Printer revision preconditions must be sorted and unique.",
        ));
    }
    let services = bootstrap.ready()?;
    let Some(path) = services.documents.open_json(DocumentKind::Printers)? else {
        return Ok(CommandSuccess::new(PrintersImportResult::Cancelled));
    };
    let bytes = services.documents.read(&path)?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(CommandError::validation(
            "The Printers document is too large.",
        ));
    }
    let document = parse_printers_document(&bytes)?;
    let mut ids = std::collections::HashSet::new();
    if document
        .printers
        .iter()
        .any(|printer| !ids.insert(printer.id.clone()))
    {
        return Err(CommandError::validation("Printer IDs must be unique."));
    }
    let repository = PrinterRepository::new(Arc::clone(&services.storage));
    let current = repository
        .list()
        .map_err(|error| CommandError::from_repository(error.into()))?
        .into_iter()
        .map(|printer| (printer.id, printer.revision))
        .collect::<Vec<_>>();
    if current != expected {
        let revision_differences = expected
            .iter()
            .zip(&current)
            .filter(|(expected, current)| expected.0 == current.0 && expected.1 != current.1)
            .collect::<Vec<_>>();
        if expected.len() == current.len()
            && expected
                .iter()
                .zip(&current)
                .all(|(left, right)| left.0 == right.0)
            && revision_differences.len() == 1
        {
            let (expected_row, current_row) = revision_differences[0];
            return Err(CommandError::revision_conflict(
                &expected_row.0,
                expected_row.1,
                current_row.1,
            ));
        }
        return Err(CommandError::set_conflict(expected.len(), current.len()));
    }
    services
        .storage
        .create_snapshot(SnapshotKind::Printers)
        .map_err(|_| CommandError::persistence_unavailable())?;
    services.documents.after_snapshot(DocumentKind::Printers)?;
    let old: std::collections::HashSet<_> = expected.iter().map(|value| value.0.clone()).collect();
    let imported_ids: std::collections::HashSet<_> = document
        .printers
        .iter()
        .map(|value| value.id.clone())
        .collect();
    let old_records = PrinterRepository::new(Arc::clone(&services.storage))
        .list()
        .map_err(|error| CommandError::from_repository(error.into()))?;
    let stored = crate::connections::commands::with_credential_coordination(|| {
        PrinterRepository::new(Arc::clone(&services.storage))
            .replace_all(&expected, document.printers)
    })
    .map_err(CommandError::from_repository)?;
    services.documents.after_printers_commit();
    let _reconciliation = services.manager.reconciliation_guard().await;
    let mut warnings = Vec::new();
    for printer in old_records {
        if !services.manager.stop_and_wait(&printer.id).await {
            warnings.push(OperationWarning::supervisor(&printer.id));
        }
    }
    let needs_store = stored.iter().any(|printer| {
        printer
            .connection
            .as_ref()
            .and_then(|connection| connection.credential_ref.as_ref())
            .is_some()
    });
    let credential_store = needs_store.then(|| Arc::clone(&services.credentials));
    for printer in &stored {
        let profile_resolved =
            crate::catalog::resolve::resolve_catalog_ref(&services.catalog, &printer.catalog_ref)
                .0
                .is_some();
        let Some(config) = printer.connection.clone() else {
            services.manager.reconcile_printer(
                &printer.id,
                crate::connections::supervisor::PrinterSetupFacts {
                    has_usable_connection: false,
                    profile_resolved,
                },
            );
            continue;
        };
        if config.kind != crate::connections::MOONRAKER_KIND {
            services.manager.reconcile_printer(
                &printer.id,
                crate::connections::supervisor::PrinterSetupFacts {
                    has_usable_connection: false,
                    profile_resolved,
                },
            );
            warnings.push(OperationWarning::supervisor(&printer.id));
            continue;
        }
        let credential = match config.credential_ref.as_deref() {
            None => None,
            Some(reference) => credential_store
                .as_ref()
                .and_then(|store| store.get(reference).ok())
                .flatten(),
        };
        if config.credential_ref.is_some() && credential.is_none() {
            services.manager.reconcile_printer(
                &printer.id,
                crate::connections::supervisor::PrinterSetupFacts {
                    has_usable_connection: false,
                    profile_resolved,
                },
            );
            warnings.push(OperationWarning::credential_required(&printer.id));
        } else {
            services
                .manager
                .start(
                    printer.id.clone(),
                    config,
                    credential.map(zeroize::Zeroizing::new),
                    crate::connections::supervisor::PrinterSetupFacts {
                        has_usable_connection: true,
                        profile_resolved,
                    },
                )
                .await;
        }
    }
    let resolved = stored
        .iter()
        .map(|printer| crate::catalog::resolve::resolve_printer(&services.catalog, printer))
        .collect();
    Ok(CommandSuccess::new(PrintersImportResult::Applied {
        printers: resolved,
        created_count: imported_ids.difference(&old).count(),
        updated_count: imported_ids.intersection(&old).count(),
        deleted_count: old.difference(&imported_ids).count(),
        warnings,
    }))
}

fn export_printer(printer: &super::StoredPrinter) -> Result<serde_json::Value, CommandError> {
    let mut value = serde_json::to_value(printer).map_err(|_| CommandError::internal())?;
    let object = value.as_object_mut().ok_or_else(CommandError::internal)?;
    object.remove("group");
    object.remove("createdAt");
    object.remove("updatedAt");
    // P2's lifecycle columns (location/startSafety/archivedAt) aren't part
    // of this schemaVersion-1 document shape yet — a later task's command
    // surface change decides how/when to add them to export/import.
    object.remove("location");
    object.remove("startSafety");
    object.remove("archivedAt");
    object
        .entry("overrides")
        .or_insert_with(|| serde_json::json!({}));
    object
        .entry("lastKnownGood")
        .or_insert(serde_json::Value::Null);
    object
        .entry("connection")
        .or_insert(serde_json::Value::Null);
    Ok(value)
}

fn parse_printers_document(bytes: &[u8]) -> Result<PrintersDocument, CommandError> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(CommandError::validation(
            "The Printers document is too large.",
        ));
    }
    crate::persistence::validation::validate_import_json(bytes).map_err(|error| {
        if error.kind == crate::persistence::validation::ImportValidationErrorKind::InvalidJson {
            return CommandError::corrupt_import("printersImport");
        }
        CommandError::validation_at(
            error.field_path,
            match error.kind {
                crate::persistence::validation::ImportValidationErrorKind::CredentialField => {
                    "The document contains a credential value field."
                }
                crate::persistence::validation::ImportValidationErrorKind::UnsupportedNumber => {
                    "The document contains a number that cannot be represented safely."
                }
                crate::persistence::validation::ImportValidationErrorKind::InvalidJson => {
                    unreachable!()
                }
            },
        )
    })?;
    let raw: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| CommandError::corrupt_import("printersImport"))?;
    let root = raw
        .as_object()
        .ok_or_else(|| CommandError::validation("The Printers document must be an object."))?;
    if root
        .keys()
        .any(|key| !matches!(key.as_str(), "schemaVersion" | "exportedAt" | "printers"))
    {
        return Err(CommandError::validation(
            "The Printers document contains an unknown field.",
        ));
    }
    let version = root
        .get("schemaVersion")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| CommandError::validation("schemaVersion must be a positive integer."))?;
    if version > 1 {
        return Err(CommandError::unsupported_schema(version));
    }
    if version != 1 {
        return Err(CommandError::validation("schemaVersion must be 1."));
    }
    let rows = root
        .get("printers")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| CommandError::validation("printers must be an array."))?;
    if rows.len() > 10_000 {
        return Err(CommandError::validation(
            "The Printers document has too many rows.",
        ));
    }
    const FIELDS: &[&str] = &[
        "id",
        "revision",
        "name",
        "catalogRef",
        "notes",
        "overrides",
        "lastKnownGood",
        "connection",
    ];
    for row in rows {
        let object = row
            .as_object()
            .ok_or_else(|| CommandError::validation("Every Printer must be an object."))?;
        if object.keys().any(|key| !FIELDS.contains(&key.as_str()))
            || FIELDS[..6].iter().any(|key| !object.contains_key(*key))
        {
            return Err(CommandError::validation(
                "A Printer contains missing or unknown fields.",
            ));
        }
        if !object
            .get("overrides")
            .is_some_and(serde_json::Value::is_object)
        {
            return Err(CommandError::validation(
                "Printer overrides must be an object.",
            ));
        }
    }
    let document: PrintersDocument = serde_json::from_value(raw).map_err(|_| {
        CommandError::validation("The selected Printers document has an invalid shape.")
    })?;
    for printer in &document.printers {
        if printer.id.is_empty()
            || printer.id.len() > 512
            || printer.id.chars().any(char::is_control)
            || printer.revision <= 0
            || printer.name.is_empty()
        {
            return Err(CommandError::validation(
                "A Printer identity, revision, or name is invalid.",
            ));
        }
        if printer.catalog_ref.vendor.is_empty()
            || printer.catalog_ref.model.is_empty()
            || printer.catalog_ref.variant.is_empty()
            || printer.catalog_ref.model_id.is_empty()
            || printer.catalog_ref.printer_variant.is_empty()
        {
            return Err(CommandError::validation("A Printer catalogRef is invalid."));
        }
        if let Some(connection) = &printer.connection {
            if connection.kind.is_empty() || connection.host.is_empty() {
                return Err(CommandError::validation("A Printer Connection is invalid."));
            }
            if let Some(reference) = &connection.credential_ref {
                validate_credential_reference(reference)?;
            }
        }
    }
    let mut ids = std::collections::HashSet::new();
    if document
        .printers
        .iter()
        .any(|printer| !ids.insert(&printer.id))
    {
        return Err(CommandError::validation("Printer IDs must be unique."));
    }
    Ok(document)
}

pub(crate) fn validate_credential_reference(reference: &str) -> Result<(), CommandError> {
    let valid_legacy = reference
        .strip_prefix("farm3d/printer/")
        .and_then(|value| value.strip_suffix("/apikey"))
        .is_some_and(|owner| !owner.is_empty());
    let valid_current = reference
        .strip_prefix("farm3d/credential/")
        .and_then(|value| uuid::Uuid::parse_str(value).ok().map(|uuid| (value, uuid)))
        .is_some_and(|(text, uuid)| {
            uuid.get_version_num() == 4 && uuid.hyphenated().to_string() == text
        });
    if reference.len() > 512
        || reference.chars().any(char::is_control)
        || (!valid_legacy && !valid_current)
    {
        return Err(CommandError::validation("A credentialRef is invalid."));
    }
    Ok(())
}

#[cfg(test)]
mod import_export_tests {
    use super::*;
    use crate::contracts::command::ErrorCode;

    fn document(connection: &str, overrides: &str) -> Vec<u8> {
        format!(r#"{{"schemaVersion":1,"exportedAt":"x","printers":[{{"id":"legacy/Bay α","revision":1,"name":"Bay","catalogRef":{{"vendor":"Example","model":"Printer","variant":"0.4","modelId":"Example-1","printerVariant":"0.4"}},"notes":"","overrides":{overrides},"lastKnownGood":null,"connection":{connection}}}]}}"#).into_bytes()
    }

    #[test]
    fn printer_import_preserves_unknown_overrides_and_unresolved_catalog_identity() {
        let parsed =
            parse_printers_document(&document("null", r#"{"future":{"enabled":true}}"#)).unwrap();
        assert_eq!(
            parsed.printers[0].overrides.extra["future"]["enabled"],
            true
        );
        assert_eq!(parsed.printers[0].catalog_ref.model_id, "Example-1");
    }

    #[test]
    fn printer_import_rejects_future_schema_duplicate_ids_and_secret_fields() {
        let future = br#"{"schemaVersion":2,"exportedAt":"x","printers":[]}"#;
        assert_eq!(
            parse_printers_document(future).unwrap_err().code,
            ErrorCode::UnsupportedSchemaVersion
        );
        let secret = document(
            r#"{"kind":"moonraker","host":"x","port":7125,"useTls":false,"apiKey":"sentinel"}"#,
            "{}",
        );
        assert_eq!(
            parse_printers_document(&secret).unwrap_err().code,
            ErrorCode::Validation
        );
        let one = String::from_utf8(document("null", "{}")).unwrap();
        let row = serde_json::from_str::<serde_json::Value>(&one).unwrap()["printers"][0].clone();
        let duplicate =
            serde_json::json!({"schemaVersion":1,"exportedAt":"x","printers":[row.clone(), row]});
        assert_eq!(
            parse_printers_document(&serde_json::to_vec(&duplicate).unwrap())
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }

    #[test]
    fn printer_import_validates_credential_reference_forms() {
        let valid = document(
            r#"{"kind":"moonraker","host":"x","port":7125,"useTls":false,"credentialRef":"farm3d/credential/6ba7b810-9dad-4f83-a131-2a6f44cbbf89"}"#,
            "{}",
        );
        assert!(parse_printers_document(&valid).is_ok());
        let malformed = document(
            r#"{"kind":"moonraker","host":"x","port":7125,"useTls":false,"credentialRef":"farm3d/credential/not-a-uuid"}"#,
            "{}",
        );
        assert_eq!(
            parse_printers_document(&malformed).unwrap_err().code,
            ErrorCode::Validation
        );
    }
}
