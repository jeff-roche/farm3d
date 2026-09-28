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

use crate::cameras::{config as camera_config, CameraSource, CameraSourceInput};
use crate::catalog::resolve::{resolve_catalog_ref, resolve_printer, ResolvedPrinter};
use crate::catalog::{BedShape, PrinterProfile};
use crate::connections::{ConnectionConfig, ProbeResult, ReportedCapabilities};
use crate::contracts::command::{
    CommandError, CommandSuccess, ErrorCode, IncomingContractVersion, JsonValue,
};
use crate::printers::alerts::AlertDefaults;
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
    /// D12: copied into each row's own `CreatePrinterOptions` as its own,
    /// independent set of freshly-generated slot ids — there is no batch or
    /// shared layout entity (D12, user decision 3). `None` means every row
    /// gets `slots::default_layout()`.
    #[serde(default)]
    #[ts(optional)]
    pub slot_layout: Option<Vec<crate::spools::slots::SlotSpec>>,
    /// P8 D4: the camera shape every row's own camera is built from (see
    /// `CameraTemplate`). Validated once, up front. `None`: no row gets a
    /// camera.
    #[serde(default)]
    #[ts(optional)]
    pub camera_template: Option<CameraTemplate>,
    /// P8: written as each created Printer's own alert defaults. `None`:
    /// no row (the defaults apply).
    #[serde(default)]
    #[ts(optional)]
    pub alert_defaults: Option<AlertDefaults>,
}

/// P8 "Wire types": a batch's shared camera shape. It never holds an
/// endpoint: each row's camera is built on its own at commit.
///
/// - `hostWebcam` stores `webcamName` (and `webPort`) on every row with a
///   Connection that can list webcams; each resolves against that row's
///   own Connection at fetch time.
/// - `snapshotUrl` builds `http://<host>:<port><path>` per row, `host`
///   being the row's `cameraHostOverride`, else its Connection host, and
///   validates it like a manual URL. `path` starts with `/`, is at most
///   1024 characters, and has no `#`, backslash, or whitespace.
///
/// A row with neither a Connection nor an override gets no camera.
#[derive(Deserialize, Clone, PartialEq, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "command/CameraTemplate.ts"
)]
pub enum CameraTemplate {
    HostWebcam {
        webcam_name: String,
        web_port: Option<u16>,
    },
    SnapshotUrl {
        path: String,
        port: u16,
    },
}

/// Never prints a snapshot path: its query can carry a camera token.
impl fmt::Debug for CameraTemplate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CameraTemplate::HostWebcam {
                webcam_name,
                web_port,
            } => formatter
                .debug_struct("HostWebcam")
                .field("webcam_name", webcam_name)
                .field("web_port", web_port)
                .finish(),
            CameraTemplate::SnapshotUrl { port, .. } => formatter
                .debug_struct("SnapshotUrl")
                .field("path", &"<redacted>")
                .field("port", port)
                .finish(),
        }
    }
}

#[derive(Deserialize, TS)]
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
    /// P8 D4: the host this row's `snapshotUrl` camera uses instead of its
    /// Connection host (a camera on another box). A bare host: no user
    /// name, port, or path. Blank is none. Only meaningful with a
    /// `snapshotUrl` camera template.
    #[serde(default)]
    #[ts(optional)]
    pub camera_host_override: Option<String>,
}

/// Redacts the camera host override: it only ever reaches a camera URL.
impl fmt::Debug for BatchRowInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BatchRowInput")
            .field("row_id", &self.row_id)
            .field("name", &self.name)
            .field("location", &self.location)
            .field("connection", &self.connection)
            .field(
                "camera_host_override",
                &self.camera_host_override.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
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
        // Import-only (`import_printers` via `host_identity::archive_duplicates`)
        // — batch-created rows are never run through that pass, so this
        // never occurs here.
        OperationWarningCode::DuplicateHostArchived => None,
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
    /// Trimmed, checked against the `snapshotUrl` template at plan time.
    camera_host_override: Option<String>,
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

/// D4/D12: every row shares one Material Slot layout (`shared.slotLayout` —
/// see [`BatchShared`]'s own doc comment; `None` is `slots::default_layout`,
/// the same fallback [`commit_row`]'s `options` closure applies per row).
/// Validated once, up front, in the same `VALIDATION`-on-`slots` shape the
/// single-create path (`create_printer`) fails with at write time — so an
/// invalid layout fails the whole batch before any row's probe or commit
/// runs, rather than every row independently failing the identical check
/// inside its own create transaction.
fn validate_shared_layout(shared: &BatchShared) -> Result<(), CommandError> {
    let default = crate::spools::slots::default_layout();
    let layout = shared.slot_layout.as_deref().unwrap_or(&default);
    crate::spools::slots::validate_layout(layout).map_err(CommandError::from_repository)
}

/// P8 D4: the shared camera template, validated once, up front (like the
/// layout), with a batch-wide `VALIDATION` on
/// `shared.cameraTemplate.<field>`. Returns it normalized (the webcam name
/// trimmed).
fn normalize_camera_template(template: &CameraTemplate) -> Result<CameraTemplate, CommandError> {
    const PREFIX: &str = "shared.cameraTemplate";
    match template {
        CameraTemplate::HostWebcam {
            webcam_name,
            web_port,
        } => {
            let source = camera_config::validate_source(
                &CameraSource::HostWebcam {
                    webcam_name: webcam_name.clone(),
                    webcam_service: None,
                    web_port: *web_port,
                },
                PREFIX,
            )?;
            match source {
                CameraSource::HostWebcam {
                    webcam_name,
                    web_port,
                    ..
                } => Ok(CameraTemplate::HostWebcam {
                    webcam_name,
                    web_port,
                }),
                CameraSource::SnapshotUrl { .. } => Err(CommandError::internal()),
            }
        }
        CameraTemplate::SnapshotUrl { path, port } => {
            let path = camera_config::validate_template_path(path, &format!("{PREFIX}.path"))?;
            if *port == 0 {
                return Err(CommandError::validation_at(
                    format!("{PREFIX}.port"),
                    "The camera port must be between 1 and 65535.",
                ));
            }
            Ok(CameraTemplate::SnapshotUrl { path, port: *port })
        }
    }
}

/// P8 D4: one row's own camera, built from the template and the row's own
/// Connection (the one it is committed with) or its host override. Nothing
/// comes from another row. A row whose camera can't be built keeps its
/// Printer and gets a row error instead.
fn row_camera(
    template: Option<&CameraTemplate>,
    index: usize,
    host_override: Option<&str>,
    connection: Option<&ConnectionConfig>,
) -> (Option<CameraSourceInput>, Option<BatchRowError>) {
    match template {
        None => (None, None),
        Some(CameraTemplate::HostWebcam {
            webcam_name,
            web_port,
        }) => match connection {
            None => (None, None),
            Some(config) if !crate::cameras::resolve::supports_host_webcams(&config.kind) => (
                None,
                Some(validation_error(
                    "shared.cameraTemplate.kind".to_string(),
                    "This Printer's adapter can't list webcams, so it has no camera. Set a \
                     snapshot URL in its Setup.",
                )),
            ),
            Some(_) => (
                Some(CameraSourceInput(CameraSource::HostWebcam {
                    webcam_name: webcam_name.clone(),
                    webcam_service: None,
                    web_port: *web_port,
                })),
                None,
            ),
        },
        Some(CameraTemplate::SnapshotUrl { path, port }) => {
            let (host, field_path) = match (host_override, connection) {
                (Some(host), _) => (host, format!("rows[{index}].cameraHostOverride")),
                (None, Some(config)) => (
                    config.host.as_str(),
                    "shared.cameraTemplate.path".to_string(),
                ),
                (None, None) => return (None, None),
            };
            match camera_config::template_snapshot_url(host, *port, path) {
                Some(snapshot_url) => (
                    Some(CameraSourceInput(CameraSource::SnapshotUrl {
                        snapshot_url,
                    })),
                    None,
                ),
                None => (
                    None,
                    Some(validation_error(
                        field_path,
                        "This Printer's camera URL would not be valid, so it has no camera.",
                    )),
                ),
            }
        }
    }
}

/// Tracks what earlier rows in the batch have claimed.
struct PlanState<'a> {
    repository: PrinterRepository,
    shared_secret: Option<&'a Zeroizing<String>>,
    camera_template: Option<&'a CameraTemplate>,
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
        if !crate::connections::is_supported_kind(&connection.kind) {
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
        if connection.use_tls {
            return Ok(Err(validation_error(
                path("useTls"),
                crate::connections::TLS_UNSUPPORTED_MESSAGE,
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

    /// P8 D4: a row's camera host override, trimmed (blank is none). It
    /// rejects the row, like an invalid name, when it is not a bare host
    /// that makes a valid URL with the `snapshotUrl` template, or when the
    /// template is not a `snapshotUrl` one. The message never quotes it.
    fn plan_camera_host_override(
        &self,
        index: usize,
        value: Option<&str>,
        errors: &mut Vec<BatchRowError>,
    ) -> Option<String> {
        let host = value.map(str::trim).filter(|host| !host.is_empty())?;
        let field_path = format!("rows[{index}].cameraHostOverride");
        match self.camera_template {
            Some(CameraTemplate::SnapshotUrl { path, port }) => {
                if camera_config::template_snapshot_url(host, *port, path).is_some() {
                    return Some(host.to_string());
                }
                errors.push(validation_error(
                    field_path,
                    "Enter a camera host name or address with no user name, port, or path.",
                ));
            }
            _ => errors.push(validation_error(
                field_path,
                "A camera host override applies only to a snapshot URL camera template.",
            )),
        }
        None
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
        let camera_host_override =
            self.plan_camera_host_override(index, row.camera_host_override.as_deref(), &mut errors);
        let (Some(name), Some(location), true) = (name, location, errors.is_empty()) else {
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
            camera_host_override,
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
    app: &AppHandle<R>,
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

    let options = |connection, camera| CreatePrinterOptions {
        name: row.name.clone(),
        catalog_ref: shared.catalog_ref.clone(),
        location: row.location.clone(),
        start_safety: shared.start_safety,
        default_bed_type: shared.default_bed_type.clone(),
        connection,
        slot_layout: shared
            .slot_layout
            .clone()
            .unwrap_or_else(crate::spools::slots::default_layout),
        // D12: batch never loads Spools (user decision 3).
        initial_loads: Vec::new(),
        camera,
        // P8: each Printer gets its own row of the shared values.
        alert_defaults: shared.alert_defaults,
    };
    let connection = row
        .connection
        .take()
        .map(|planned| (planned.config, planned.secret));
    let had_connection = connection.is_some();
    let camera_for = |connection: Option<&ConnectionConfig>| {
        row_camera(
            shared.camera_template.as_ref(),
            row.index,
            row.camera_host_override.as_deref(),
            connection,
        )
    };
    let (camera, camera_error) = camera_for(connection.as_ref().map(|(config, _)| config));
    row.errors.extend(camera_error);
    let mut created = create_printer_with(services, options(connection, camera)).await;
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
            // Without its Connection the row's camera is rebuilt from its
            // override alone (a host webcam has nothing to resolve against).
            let (camera, _) = camera_for(None);
            created = create_printer_with(services, options(None, camera)).await;
        }
    }

    match created {
        Ok(created) => {
            row.warnings
                .extend(created.warnings.iter().filter_map(operation_warning));
            // P8 D4: the new source's own fresh health (`unknown`), never
            // another row's.
            if let Some(kind) = created.camera_kind {
                services.cameras.source_changed(
                    app,
                    &services.attention.stream,
                    &created.printer.id,
                    Some(kind),
                );
            }
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
    app: &AppHandle<R>,
    input: CreatePrintersBatchInput,
) -> Result<CreatePrintersBatchOutput, CommandError> {
    validate_correlation(&input)?;
    validate_shared_layout(&input.shared)?;
    let camera_template = input
        .shared
        .camera_template
        .as_ref()
        .map(normalize_camera_template)
        .transpose()?;
    let CreatePrintersBatchInput {
        batch_id,
        mut shared,
        shared_credential,
        probe,
        rows,
    } = input;
    shared.camera_template = camera_template;
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
        camera_template: shared.camera_template.as_ref(),
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
                commit_row(services, app, shared, profile, row, probed).await
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
    // P8 D2 "Wakes": the Attention projector re-reads Printers, and the new
    // ones' alert defaults.
    if rows.iter().any(|row| row.printer.is_some()) {
        services.attention.poke();
    }
    Ok(CreatePrintersBatchOutput { batch_id, rows })
}

// --- Commands ---------------------------------------------------------------

#[tauri::command]
pub async fn create_printers_batch<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    input: CreatePrintersBatchInput,
) -> Result<CommandSuccess<CreatePrintersBatchOutput>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    create_printers_batch_with(&services, &app, input)
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

    fn shared_with_layout(slot_layout: Option<Vec<crate::spools::slots::SlotSpec>>) -> BatchShared {
        BatchShared {
            catalog_ref: CatalogRef {
                vendor: String::new(),
                model: String::new(),
                variant: String::new(),
                model_id: String::new(),
                printer_variant: String::new(),
            },
            default_bed_type: None,
            start_safety: StartSafety::ConfirmBedClear,
            slot_layout,
            camera_template: None,
            alert_defaults: None,
        }
    }

    #[test]
    fn validate_shared_layout_rejects_an_invalid_layout_and_defaults_a_missing_one() {
        let error = validate_shared_layout(&shared_with_layout(Some(vec![]))).unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(
            error.details.unwrap().get("fieldPath"),
            Some(&crate::contracts::command::JsonValue::String(
                "slots".to_string()
            ))
        );

        assert!(validate_shared_layout(&shared_with_layout(None)).is_ok());
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
