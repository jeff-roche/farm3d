//! `create_printers_batch` / `cancel_printer_batch` (spec D1, D3, D9, D13,
//! D14).
//!
//! A batch runs in three phases:
//!
//! 1. **Plan** (no network I/O): correlation ids, the shared catalog ref,
//!    each row's identity, its Connection schema and credential source, and
//!    duplicate hosts within the batch and against the DB. Invalid identity
//!    rejects the row; an invalid Connection only drops the Connection (the
//!    row still becomes a setup-incomplete Printer, D1).
//! 2. **Probe** (when `probe` is set) with at most [`PROBE_CONCURRENCY`]
//!    rows in flight.
//! 3. **Commit** each row through [`create_printer_with`] — the single-create
//!    path, including its provisional-credential ordering and
//!    supervise-after-commit — as soon as its own probe finishes.
//!
//! Cancellation (via [`BatchRegistry`]) drops in-flight probes and reports
//! every not-yet-committed row as `cancelled`; committed rows stay.
//!
//! Secrets arrive as [`SecretInput`], which moves them into `Zeroizing` as
//! they are deserialized and never prints them. The result reports only
//! `credentialStored`.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Deserializer, Serialize};
use tauri::AppHandle;
use tokio::sync::watch;
use ts_rs::TS;
use zeroize::Zeroizing;

use crate::catalog::resolve::{resolve_catalog_ref, resolve_printer, ResolvedPrinter};
use crate::catalog::{BedShape, PrinterProfile};
use crate::connections::{ConnectionConfig, ProbeResult, ReportedCapabilities, MOONRAKER_KIND};
use crate::contracts::command::{
    CommandError, CommandSuccess, ErrorCode, IncomingContractVersion, JsonValue,
};
use crate::printers::commands::{OperationWarning, OperationWarningCode};
use crate::printers::create::{
    create_printer_with, probe_submission, validate_location, validate_name, CreatePrinterOptions,
};
use crate::printers::host_identity::canonical_host_identity;
use crate::printers::repository::PrinterRepository;
use crate::printers::{CatalogRef, StartSafety};
use crate::RuntimeServices;

/// At most this many rows are probing or committing at once (D14).
pub const PROBE_CONCURRENCY: usize = 4;

/// The frontend's `buildMismatches` tolerance: differences up to 1 mm are
/// rounding, not a wrong catalog variant.
const TOLERANCE_MM: f64 = 1.0;

// --- Request ----------------------------------------------------------------

/// A secret carried by the batch request. It is moved into `Zeroizing` as it
/// is deserialized, so no plain `String` copy outlives the request, and its
/// `Debug` output is redacted.
pub struct SecretInput(Zeroizing<String>);

impl<'de> Deserialize<'de> for SecretInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|value| Self(Zeroizing::new(value)))
    }
}

impl fmt::Debug for SecretInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

impl SecretInput {
    /// The trimmed secret, or `None` when it is blank — the same "blank
    /// means no credential" rule `create_printer` applies.
    fn non_blank(&self) -> Option<Zeroizing<String>> {
        let trimmed = self.0.trim();
        (!trimmed.is_empty()).then(|| Zeroizing::new(trimmed.to_string()))
    }
}

#[derive(Deserialize, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "command/CreatePrintersBatchInput.ts"
)]
pub struct CreatePrintersBatchInput {
    /// Client-generated; must be unique among running batches.
    pub batch_id: String,
    pub shared: BatchShared,
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub shared_credential: Option<SecretInput>,
    pub probe: bool,
    pub rows: Vec<BatchRowInput>,
}

#[derive(Deserialize, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/BatchShared.ts")]
pub struct BatchShared {
    pub catalog_ref: CatalogRef,
    #[serde(default)]
    #[ts(optional)]
    pub default_bed_type: Option<String>,
    pub start_safety: StartSafety,
}

#[derive(Deserialize, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/BatchRowInput.ts")]
pub struct BatchRowInput {
    pub row_id: String,
    pub name: String,
    #[serde(default)]
    #[ts(optional)]
    pub location: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub connection: Option<BatchRowConnection>,
}

#[derive(Deserialize, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/BatchRowConnection.ts")]
pub struct BatchRowConnection {
    pub kind: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub use_tls: bool,
    pub credential: BatchCredentialSource,
}

#[derive(Deserialize, Debug, TS)]
#[serde(tag = "source", rename_all = "camelCase")]
#[ts(
    tag = "source",
    rename_all = "camelCase",
    export_to = "command/BatchCredentialSource.ts"
)]
pub enum BatchCredentialSource {
    None,
    Shared,
    Row {
        #[ts(as = "String")]
        value: SecretInput,
    },
}

// --- Result -----------------------------------------------------------------

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "command/CreatePrintersBatchOutput.ts"
)]
pub struct CreatePrintersBatchOutput {
    pub batch_id: String,
    /// In request order; every request `rowId` appears exactly once.
    pub rows: Vec<BatchRowResult>,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/BatchRowResult.ts")]
pub struct BatchRowResult {
    pub row_id: String,
    pub outcome: BatchRowOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub printer: Option<ResolvedPrinter>,
    pub credential_stored: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub probe: Option<ProbeResult>,
    pub errors: Vec<BatchRowError>,
    pub warnings: Vec<BatchRowWarning>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/BatchRowOutcome.ts")]
pub enum BatchRowOutcome {
    Created,
    CreatedSetupIncomplete,
    Rejected,
    Cancelled,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/BatchRowError.ts")]
pub struct BatchRowError {
    pub code: BatchRowErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub field_path: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub conflicting_printer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub conflicting_row_id: Option<String>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(
    rename_all = "SCREAMING_SNAKE_CASE",
    export_to = "command/BatchRowErrorCode.ts"
)]
pub enum BatchRowErrorCode {
    Validation,
    DuplicateHost,
    UnsupportedAdapter,
    AuthenticationFailed,
    PrinterUnreachable,
    Timeout,
    ProtocolError,
    CredentialUnavailable,
    PersistenceUnavailable,
    Cancelled,
}

impl From<ErrorCode> for BatchRowErrorCode {
    /// Codes with a row-level meaning map one-to-one; anything else a row
    /// commit can fail with is a storage-side failure.
    fn from(code: ErrorCode) -> Self {
        match code {
            ErrorCode::Validation => Self::Validation,
            ErrorCode::DuplicateHost => Self::DuplicateHost,
            ErrorCode::UnsupportedAdapter => Self::UnsupportedAdapter,
            ErrorCode::AuthenticationFailed => Self::AuthenticationFailed,
            ErrorCode::PrinterUnreachable => Self::PrinterUnreachable,
            ErrorCode::Timeout => Self::Timeout,
            ErrorCode::ProtocolError => Self::ProtocolError,
            ErrorCode::CredentialUnavailable => Self::CredentialUnavailable,
            _ => Self::PersistenceUnavailable,
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/BatchRowWarning.ts")]
pub struct BatchRowWarning {
    pub code: BatchRowWarningCode,
    pub message: String,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(
    rename_all = "SCREAMING_SNAKE_CASE",
    export_to = "command/BatchRowWarningCode.ts"
)]
pub enum BatchRowWarningCode {
    CapabilityMismatch,
    DuplicateName,
    CredentialCleanupPending,
    SupervisorReconciliationFailed,
}

#[derive(Serialize, Debug, Default, TS)]
#[ts(export_to = "command/CancelPrinterBatchData.ts")]
pub struct CancelPrinterBatchData {}

// --- Registry ---------------------------------------------------------------

/// Process-wide map of running batches to their cancellation channel.
pub struct BatchRegistry {
    running: Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl BatchRegistry {
    pub fn global() -> &'static BatchRegistry {
        static REGISTRY: OnceLock<BatchRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| BatchRegistry {
            running: Mutex::new(HashMap::new()),
        })
    }

    fn running(&self) -> std::sync::MutexGuard<'_, HashMap<String, watch::Sender<bool>>> {
        // The map holds no invariant a panicking holder could break.
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers `batch_id` as running; `CONFLICT` if it already is.
    pub fn register(&self, batch_id: &str) -> Result<watch::Receiver<bool>, CommandError> {
        let mut running = self.running();
        if running.contains_key(batch_id) {
            return Err(CommandError::conflict(
                "A batch with this id is already running.",
            ));
        }
        let (sender, receiver) = watch::channel(false);
        running.insert(batch_id.to_string(), sender);
        Ok(receiver)
    }

    /// Signals cancellation. A no-op for unknown or finished ids.
    pub fn cancel(&self, batch_id: &str) {
        if let Some(sender) = self.running().get(batch_id) {
            sender.send_replace(true);
        }
    }

    pub fn finish(&self, batch_id: &str) {
        self.running().remove(batch_id);
    }
}

/// Calls [`BatchRegistry::finish`] however the batch ends.
struct Registration {
    registry: &'static BatchRegistry,
    batch_id: String,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.registry.finish(&self.batch_id);
    }
}

async fn wait_cancelled(mut receiver: watch::Receiver<bool>) {
    if receiver.wait_for(|cancelled| *cancelled).await.is_err() {
        // The sender lives until the batch finishes, so this cannot happen
        // while a row is running; never report a cancellation that wasn't.
        std::future::pending::<()>().await;
    }
}

// --- Capability mismatch ----------------------------------------------------

/// Mirrors the frontend's `buildMismatches`: compares what the host reports
/// against the Profile with a 1 mm tolerance. A value the host did not
/// report is never a mismatch, and only a rectangular bed has a width/depth
/// to compare.
pub fn capability_mismatches(
    profile: &PrinterProfile,
    reported: &ReportedCapabilities,
) -> Vec<String> {
    let mut mismatches = Vec::new();
    let mut check = |label: &str, catalog: f64, host: Option<f64>| {
        if let Some(host) = host {
            if (catalog - host).abs() > TOLERANCE_MM {
                mismatches.push(format!(
                    "{label}: catalog says {catalog} mm, the printer reports {host} mm"
                ));
            }
        }
    };
    if let BedShape::Rectangular {
        width_mm, depth_mm, ..
    } = profile.bed_shape
    {
        check("Bed width", width_mm, reported.bed_width_mm);
        check("Bed depth", depth_mm, reported.bed_depth_mm);
    }
    check(
        "Printable height",
        profile.printable_height_mm,
        reported.printable_height_mm,
    );
    mismatches
}

// --- Row errors and warnings ------------------------------------------------

fn detail(error: &CommandError, key: &str) -> Option<String> {
    match error.details.as_ref()?.get(key)? {
        JsonValue::String(value) => Some(value.clone()),
        _ => None,
    }
}

/// A `CommandError` as a row error. `field_path` (the row-scoped path)
/// replaces the error's own unscoped `fieldPath` detail when given.
fn row_error(error: &CommandError, field_path: Option<String>) -> BatchRowError {
    BatchRowError {
        code: error.code.into(),
        field_path: field_path.or_else(|| detail(error, "fieldPath")),
        message: error.message.clone(),
        conflicting_printer_id: detail(error, "conflictingPrinterId"),
        conflicting_row_id: None,
    }
}

fn validation_error(field_path: String, message: &str) -> BatchRowError {
    BatchRowError {
        code: BatchRowErrorCode::Validation,
        field_path: Some(field_path),
        message: message.to_string(),
        conflicting_printer_id: None,
        conflicting_row_id: None,
    }
}

fn warning(code: BatchRowWarningCode, message: impl Into<String>) -> BatchRowWarning {
    BatchRowWarning {
        code,
        message: message.into(),
    }
}

fn operation_warning(warning_in: &OperationWarning) -> Option<BatchRowWarning> {
    match warning_in.code {
        OperationWarningCode::CredentialCleanupPending => Some(warning(
            BatchRowWarningCode::CredentialCleanupPending,
            "A credential could not be removed yet; cleanup will be retried.",
        )),
        OperationWarningCode::SupervisorReconciliationFailed => Some(warning(
            BatchRowWarningCode::SupervisorReconciliationFailed,
            "Monitoring could not be started for this Printer.",
        )),
        // Only possible if the secret written moments earlier vanished
        // before supervision read it; the Printer's status already shows
        // that it needs a credential, and the batch contract has no code
        // for it.
        OperationWarningCode::CredentialRequired => None,
    }
}

// --- Phase 1: plan ----------------------------------------------------------

struct PlannedConnection {
    config: ConnectionConfig,
    secret: Option<Zeroizing<String>>,
}

/// A row that will be committed (with or without its Connection).
struct PlannedRow {
    index: usize,
    row_id: String,
    name: String,
    location: Option<String>,
    connection: Option<PlannedConnection>,
    errors: Vec<BatchRowError>,
    warnings: Vec<BatchRowWarning>,
}

impl PlannedRow {
    fn finish(
        self,
        outcome: BatchRowOutcome,
        printer: Option<ResolvedPrinter>,
        credential_stored: bool,
        probe: Option<ProbeResult>,
    ) -> (usize, BatchRowResult) {
        (
            self.index,
            BatchRowResult {
                row_id: self.row_id,
                outcome,
                printer,
                credential_stored,
                probe,
                errors: self.errors,
                warnings: self.warnings,
            },
        )
    }

    fn cancelled(mut self) -> (usize, BatchRowResult) {
        self.errors.push(BatchRowError {
            code: BatchRowErrorCode::Cancelled,
            field_path: None,
            message: "The batch was cancelled before this row was saved.".to_string(),
            conflicting_printer_id: None,
            conflicting_row_id: None,
        });
        self.finish(BatchRowOutcome::Cancelled, None, false, None)
    }
}

fn rejected(row_id: String, errors: Vec<BatchRowError>) -> BatchRowResult {
    BatchRowResult {
        row_id,
        outcome: BatchRowOutcome::Rejected,
        printer: None,
        credential_stored: false,
        probe: None,
        errors,
        warnings: Vec::new(),
    }
}

/// `rowId`s must be non-empty and unique, or results can't be correlated
/// and the whole request fails.
fn validate_correlation(input: &CreatePrintersBatchInput) -> Result<(), CommandError> {
    if input.batch_id.trim().is_empty() {
        return Err(CommandError::validation_at(
            "batchId",
            "A batch id is required.",
        ));
    }
    let mut seen = HashSet::new();
    for (index, row) in input.rows.iter().enumerate() {
        if row.row_id.trim().is_empty() {
            return Err(CommandError::validation_at(
                format!("rows[{index}].rowId"),
                "Every row needs a row id.",
            ));
        }
        if !seen.insert(row.row_id.as_str()) {
            return Err(CommandError::validation_at(
                format!("rows[{index}].rowId"),
                "Row ids must be unique within a batch.",
            ));
        }
    }
    Ok(())
}

/// Tracks what earlier rows in the batch have claimed.
struct PlanState<'a> {
    repository: PrinterRepository,
    shared_secret: Option<&'a Zeroizing<String>>,
    existing_names: HashSet<String>,
    batch_names: HashSet<String>,
    /// Host identity -> the `rowId` that claimed it.
    batch_hosts: HashMap<String, String>,
}

impl PlanState<'_> {
    /// `Ok(Err(_))` drops the Connection with that row error; `Err(_)` fails
    /// the whole request (the DB could not be read).
    fn plan_connection(
        &mut self,
        index: usize,
        row_id: &str,
        connection: BatchRowConnection,
    ) -> Result<Result<PlannedConnection, BatchRowError>, CommandError> {
        let path = |field: &str| format!("rows[{index}].connection.{field}");
        if connection.kind != MOONRAKER_KIND {
            return Ok(Err(row_error(
                &CommandError::unsupported_adapter(&connection.kind),
                Some(path("kind")),
            )));
        }
        let host = connection.host.trim();
        let Some(identity) = canonical_host_identity(host, connection.port) else {
            return Ok(Err(validation_error(path("host"), "A host is required.")));
        };
        if connection.port == 0 {
            return Ok(Err(validation_error(
                path("port"),
                "The port must be positive.",
            )));
        }
        let secret = match &connection.credential {
            BatchCredentialSource::None => None,
            BatchCredentialSource::Shared => match self.shared_secret {
                Some(secret) => Some(secret.clone()),
                None => {
                    return Ok(Err(validation_error(
                        path("credential"),
                        "This row uses the shared credential, but none was provided.",
                    )))
                }
            },
            BatchCredentialSource::Row { value } => value.non_blank(),
        };
        if let Some(earlier) = self.batch_hosts.get(&identity) {
            return Ok(Err(BatchRowError {
                code: BatchRowErrorCode::DuplicateHost,
                field_path: Some(path("host")),
                message: "An earlier row in this batch already uses this host and port."
                    .to_string(),
                conflicting_printer_id: None,
                conflicting_row_id: Some(earlier.clone()),
            }));
        }
        if let Some(conflicting) = self
            .repository
            .find_active_by_host_identity(&identity, None)
            .map_err(|error| CommandError::from_repository(error.into()))?
        {
            return Ok(Err(row_error(
                &CommandError::duplicate_host(&conflicting),
                Some(path("host")),
            )));
        }
        self.batch_hosts.insert(identity, row_id.to_string());
        Ok(Ok(PlannedConnection {
            config: ConnectionConfig {
                kind: connection.kind,
                host: host.to_string(),
                port: connection.port,
                use_tls: connection.use_tls,
                credential_ref: None,
            },
            secret,
        }))
    }

    fn plan_row(&mut self, index: usize, row: BatchRowInput) -> Result<Plan, CommandError> {
        let mut errors = Vec::new();
        let name = validate_name(&row.name)
            .map_err(|error| errors.push(row_error(&error, Some(format!("rows[{index}].name")))))
            .ok();
        let location = validate_location(row.location.as_deref())
            .map_err(|error| {
                errors.push(row_error(&error, Some(format!("rows[{index}].location"))))
            })
            .ok();
        let (Some(name), Some(location)) = (name, location) else {
            return Ok(Plan::Rejected(index, rejected(row.row_id, errors)));
        };

        let mut warnings = Vec::new();
        let folded = name.to_lowercase();
        let duplicates_existing = self.existing_names.contains(&folded);
        let duplicates_earlier_row = !self.batch_names.insert(folded);
        if duplicates_existing || duplicates_earlier_row {
            warnings.push(warning(
                BatchRowWarningCode::DuplicateName,
                "Another Printer already uses this name.",
            ));
        }

        let connection = match row.connection {
            None => None,
            Some(connection) => match self.plan_connection(index, &row.row_id, connection)? {
                Ok(planned) => Some(planned),
                Err(error) => {
                    errors.push(error);
                    None
                }
            },
        };
        Ok(Plan::Commit(PlannedRow {
            index,
            row_id: row.row_id,
            name,
            location,
            connection,
            errors,
            warnings,
        }))
    }
}

enum Plan {
    Rejected(usize, BatchRowResult),
    Commit(PlannedRow),
}

// --- Phases 2 and 3: probe and commit ---------------------------------------

async fn commit_row<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    shared: &BatchShared,
    profile: &PrinterProfile,
    mut row: PlannedRow,
    probe: Option<Result<ProbeResult, CommandError>>,
) -> (usize, BatchRowResult) {
    let mut probe_result = None;
    match probe {
        Some(Err(error)) => {
            row.errors.push(row_error(&error, None));
            row.connection = None;
        }
        Some(Ok(result)) => {
            row.warnings.extend(
                capability_mismatches(profile, &result.reported)
                    .into_iter()
                    .map(|message| warning(BatchRowWarningCode::CapabilityMismatch, message)),
            );
            probe_result = Some(result);
        }
        None => {}
    }

    let options = |connection| CreatePrinterOptions {
        name: row.name.clone(),
        catalog_ref: shared.catalog_ref.clone(),
        location: row.location.clone(),
        start_safety: shared.start_safety,
        default_bed_type: shared.default_bed_type.clone(),
        connection,
    };
    let connection = row
        .connection
        .take()
        .map(|planned| (planned.config, planned.secret));
    let had_connection = connection.is_some();
    let mut created = create_printer_with(services, options(connection)).await;
    if let Err(error) = &created {
        // A host claimed since planning, or an unwritable credential store,
        // costs the row its Connection, not its Printer (D1).
        if had_connection
            && matches!(
                error.code,
                ErrorCode::DuplicateHost | ErrorCode::CredentialUnavailable
            )
        {
            row.errors.push(row_error(error, None));
            created = create_printer_with(services, options(None)).await;
        }
    }

    match created {
        Ok(created) => {
            row.warnings
                .extend(created.warnings.iter().filter_map(operation_warning));
            let outcome = if created.printer.connection.is_some() {
                BatchRowOutcome::Created
            } else {
                BatchRowOutcome::CreatedSetupIncomplete
            };
            let printer = resolve_printer(&services.catalog, &created.printer);
            row.finish(
                outcome,
                Some(printer),
                created.credential_stored,
                probe_result,
            )
        }
        Err(error) => {
            row.errors.push(row_error(&error, None));
            row.finish(BatchRowOutcome::Rejected, None, false, probe_result)
        }
    }
}

/// The whole batch: validate, register, plan, then probe and commit with
/// bounded concurrency. Results come back in request order.
pub async fn create_printers_batch_with<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    input: CreatePrintersBatchInput,
) -> Result<CreatePrintersBatchOutput, CommandError> {
    validate_correlation(&input)?;
    let CreatePrintersBatchInput {
        batch_id,
        shared,
        shared_credential,
        probe,
        rows,
    } = input;
    let shared_secret = shared_credential.as_ref().and_then(SecretInput::non_blank);
    drop(shared_credential);

    let registry = BatchRegistry::global();
    let cancel = registry.register(&batch_id)?;
    let _registration = Registration {
        registry,
        batch_id: batch_id.clone(),
    };

    let row_count = rows.len();
    let mut results: Vec<Option<BatchRowResult>> = (0..row_count).map(|_| None).collect();
    let (variant, _) = resolve_catalog_ref(&services.catalog, &shared.catalog_ref);
    let Some(profile) = variant.map(PrinterProfile::from) else {
        for (index, row) in rows.into_iter().enumerate() {
            results[index] = Some(rejected(
                row.row_id,
                vec![validation_error(
                    "shared.catalogRef".to_string(),
                    "The Printer Profile could not be resolved.",
                )],
            ));
        }
        return Ok(CreatePrintersBatchOutput {
            batch_id,
            rows: results.into_iter().flatten().collect(),
        });
    };

    let repository = PrinterRepository::new(Arc::clone(&services.storage));
    let existing_names = repository
        .list()
        .map_err(|error| CommandError::from_repository(error.into()))?
        .into_iter()
        .map(|printer| printer.name.to_lowercase())
        .collect();
    let mut state = PlanState {
        repository,
        shared_secret: shared_secret.as_ref(),
        existing_names,
        batch_names: HashSet::new(),
        batch_hosts: HashMap::new(),
    };
    let mut planned = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        match state.plan_row(index, row)? {
            Plan::Rejected(index, result) => results[index] = Some(result),
            Plan::Commit(row) => planned.push(row),
        }
    }
    drop(state);

    let shared = &shared;
    let profile = &profile;
    let committed: Vec<(usize, BatchRowResult)> = stream::iter(planned)
        .map(|row| {
            let cancel = cancel.clone();
            async move {
                if *cancel.borrow() {
                    return row.cancelled();
                }
                let probed = match (row.connection.as_ref(), probe) {
                    (Some(connection), true) => {
                        let outcome = tokio::select! {
                            result = probe_submission(
                                &services.manager,
                                &connection.config,
                                connection.secret.clone(),
                                None,
                            ) => Some(result),
                            () = wait_cancelled(cancel.clone()) => None,
                        };
                        match outcome {
                            Some(result) => Some(result),
                            None => return row.cancelled(),
                        }
                    }
                    _ => None,
                };
                // A finished probe does not save a row the user has since
                // cancelled (D14).
                if *cancel.borrow() {
                    return row.cancelled();
                }
                commit_row(services, shared, profile, row, probed).await
            }
        })
        .buffer_unordered(PROBE_CONCURRENCY)
        .collect()
        .await;
    for (index, result) in committed {
        results[index] = Some(result);
    }

    let rows: Vec<BatchRowResult> = results.into_iter().flatten().collect();
    debug_assert_eq!(rows.len(), row_count, "every row yields exactly one result");
    Ok(CreatePrintersBatchOutput { batch_id, rows })
}

// --- Commands ---------------------------------------------------------------

#[tauri::command]
pub async fn create_printers_batch<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    input: CreatePrintersBatchInput,
) -> Result<CommandSuccess<CreatePrintersBatchOutput>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    create_printers_batch_with(&services, input)
        .await
        .map(CommandSuccess::new)
}

#[tauri::command]
pub fn cancel_printer_batch<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: tauri::State<crate::bootstrap::BootstrapState<RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    batch_id: String,
) -> Result<CommandSuccess<CancelPrinterBatchData>, CommandError> {
    contract_version.validate()?;
    bootstrap.ready()?;
    BatchRegistry::global().cancel(&batch_id);
    Ok(CommandSuccess::new(CancelPrinterBatchData {}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(bed_shape: BedShape) -> PrinterProfile {
        PrinterProfile {
            bed_shape,
            printable_height_mm: 256.0,
            bed_exclude_areas: vec![],
            default_bed_type: "4".to_string(),
            nozzle_diameter_mm: vec![0.4],
            nozzle_type: "hardened_steel".to_string(),
            gcode_flavor: "klipper".to_string(),
            has_auxiliary_fan: true,
            supports_air_filtration: true,
            supports_multi_filament: false,
            suggested_host_type: None,
        }
    }

    fn rectangular() -> PrinterProfile {
        profile(BedShape::Rectangular {
            width_mm: 256.0,
            depth_mm: 256.0,
            origin_x_mm: 0.0,
            origin_y_mm: 0.0,
        })
    }

    fn reported(
        width: Option<f64>,
        depth: Option<f64>,
        height: Option<f64>,
    ) -> ReportedCapabilities {
        ReportedCapabilities {
            bed_width_mm: width,
            bed_depth_mm: depth,
            printable_height_mm: height,
        }
    }

    #[test]
    fn matching_capabilities_are_not_a_mismatch() {
        assert!(capability_mismatches(
            &rectangular(),
            &reported(Some(256.0), Some(256.0), Some(256.0))
        )
        .is_empty());
    }

    #[test]
    fn a_differing_bed_width_is_described_like_the_frontend_does() {
        assert_eq!(
            capability_mismatches(&rectangular(), &reported(Some(220.0), None, None)),
            ["Bed width: catalog says 256 mm, the printer reports 220 mm"]
        );
    }

    #[test]
    fn unreported_values_are_never_a_mismatch() {
        assert!(capability_mismatches(&rectangular(), &reported(None, None, None)).is_empty());
    }

    #[test]
    fn differences_within_one_millimetre_are_tolerated() {
        assert!(capability_mismatches(
            &rectangular(),
            &reported(Some(256.4), Some(255.0), Some(257.0))
        )
        .is_empty());
        assert_eq!(
            capability_mismatches(&rectangular(), &reported(None, None, Some(257.5))).len(),
            1
        );
    }

    #[test]
    fn a_polygon_bed_skips_width_and_depth_but_still_checks_height() {
        let polygon = profile(BedShape::Polygon { points: vec![] });
        assert!(capability_mismatches(&polygon, &reported(Some(1.0), Some(1.0), None)).is_empty());
        assert_eq!(
            capability_mismatches(&polygon, &reported(None, None, Some(100.0))),
            ["Printable height: catalog says 256 mm, the printer reports 100 mm"]
        );
    }

    #[test]
    fn row_error_codes_map_from_command_error_codes() {
        assert_eq!(
            BatchRowErrorCode::from(ErrorCode::AuthenticationFailed),
            BatchRowErrorCode::AuthenticationFailed
        );
        assert_eq!(
            BatchRowErrorCode::from(ErrorCode::DuplicateHost),
            BatchRowErrorCode::DuplicateHost
        );
        for storage_side in [
            ErrorCode::PersistenceUnavailable,
            ErrorCode::CorruptData,
            ErrorCode::Internal,
        ] {
            assert_eq!(
                BatchRowErrorCode::from(storage_side),
                BatchRowErrorCode::PersistenceUnavailable
            );
        }
    }

    #[test]
    fn request_debug_output_redacts_secrets() {
        let input: CreatePrintersBatchInput = serde_json::from_value(serde_json::json!({
            "batchId": "b",
            "shared": {
                "catalogRef": {"vendor": "", "model": "", "variant": "", "modelId": "", "printerVariant": ""},
                "startSafety": "confirmBedClear",
            },
            "sharedCredential": "SHARED-s3cret",
            "probe": false,
            "rows": [{
                "rowId": "r1",
                "name": "A",
                "connection": {
                    "kind": "moonraker", "host": "h", "port": 1,
                    "credential": {"source": "row", "value": "ROW-s3cret"},
                },
            }],
        }))
        .unwrap();
        let debug = format!("{input:?}");
        assert!(!debug.contains("s3cret"), "{debug}");
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn the_registry_refuses_a_running_id_and_frees_it_on_finish() {
        let registry = BatchRegistry {
            running: Mutex::new(HashMap::new()),
        };
        let receiver = registry.register("b").unwrap();
        assert_eq!(
            registry.register("b").unwrap_err().code,
            ErrorCode::Conflict
        );
        registry.cancel("unknown");
        assert!(!*receiver.borrow());
        registry.cancel("b");
        assert!(*receiver.borrow());
        registry.finish("b");
        assert!(registry.register("b").is_ok());
    }
}
