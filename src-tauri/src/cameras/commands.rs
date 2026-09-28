//! P8 D7 "Commands": the camera commands — `get_printer_camera`,
//! `set_printer_camera`, `clear_printer_camera`, `list_host_webcams`,
//! `test_camera`, and `camera_preview_frame`. (`capture_snapshot` and the
//! snapshot commands arrive with Task 8's media store.)
//!
//! Global constraint 3: `get_printer_camera` is the only command that
//! returns a manual URL. `set_printer_camera` answers with the redacted
//! `PrinterCameraSummary`, `list_host_webcams` with names and services,
//! and the two frame commands with a binary frame whose header names no
//! endpoint. Every error is typed and URL-free.
//!
//! The two mutating commands claim their `operationId` in their write
//! transaction (global constraint 10): a replay returns the current state
//! and publishes nothing; a rejected request never burns its id. After a
//! real change they reset the Printer's camera health and publish
//! `camera.health.changed` on the `attention` stream.

use std::sync::Arc;

use serde::Serialize;
use tauri::ipc::Response;
use tauri::AppHandle;
use ts_rs::TS;
use zeroize::Zeroizing;

use crate::bootstrap::BootstrapState;
use crate::connections::commands::ConnectionSubmission;
use crate::connections::ConnectionConfig;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::repository::PrinterRepository;
use crate::printers::StoredPrinter;
use crate::spools::operations::{self, Claim, OperationKind};
use crate::RuntimeServices;

use super::fetch::{encode_frame, CameraError, Frame};
use super::resolve::{self, WebcamHost};
use super::services::{self as camera_services, CameraFetchError};
use super::{
    config, CameraErrorKind, CameraSource, CameraSourceInput, HostWebcam, PrinterCamera,
    PrinterCameraSummary,
};

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

fn ready<R: tauri::Runtime>(
    bootstrap: &Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<Arc<RuntimeServices<R>>, CommandError> {
    contract_version.validate()?;
    bootstrap.ready()
}

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

/// The command error for a failed fetch: `CAMERA_HOST_MISMATCH`,
/// `CAPABILITY_UNSUPPORTED` (`unsupportedAdapter`), or `CAMERA_FAILED`.
pub fn camera_command_error(printer_id: Option<&str>, error: &CameraError) -> CommandError {
    match error.kind() {
        CameraErrorKind::HostMismatch => CommandError::camera_host_mismatch(printer_id),
        CameraErrorKind::UnsupportedAdapter => CommandError::camera_unsupported_adapter(printer_id),
        kind => CommandError::camera_failed(
            printer_id,
            &crate::spools::encode_enum(kind),
            error.status(),
            error.message(),
            matches!(
                kind,
                CameraErrorKind::NoSuchWebcam | CameraErrorKind::NoSnapshotUrl
            ),
        ),
    }
}

fn no_connection() -> CommandError {
    CommandError::validation_at(
        "source.kind",
        "A host webcam needs the Printer's Connection. Use a manual snapshot URL.",
    )
}

fn fetch_error(printer_id: &str, error: CameraFetchError) -> CommandError {
    match error {
        CameraFetchError::NotFound => CommandError::not_found(printer_id),
        CameraFetchError::NotConfigured => CommandError::camera_not_configured(printer_id),
        CameraFetchError::NoConnection => no_connection(),
        CameraFetchError::Camera(error) => camera_command_error(Some(printer_id), &error),
        CameraFetchError::Storage(error) => storage_error(error),
    }
}

/// D7 "Binary frame".
fn frame_response(frame: &Frame) -> Response {
    Response::new(encode_frame(&frame.header(None), &frame.bytes))
}

fn load_printer<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    printer_id: &str,
) -> Result<StoredPrinter, CommandError> {
    PrinterRepository::new(Arc::clone(&services.storage))
        .get(printer_id)
        .map_err(storage_error)?
        .ok_or_else(|| CommandError::not_found(printer_id))
}

fn printer_exists(tx: &rusqlite::Transaction<'_>, printer_id: &str) -> rusqlite::Result<bool> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM printers WHERE id = ?1)",
        [printer_id],
        |row| row.get(0),
    )
}

fn now_text() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A submitted (unsaved) Connection, checked as `probe_connection` checks
/// one, with its credential. Never stored.
fn submitted_host(mut submission: ConnectionSubmission) -> Result<WebcamHost, CommandError> {
    if submission.host.trim().is_empty() {
        return Err(CommandError::validation_at(
            "connection.host",
            "A host is required.",
        ));
    }
    if submission.port == 0 {
        return Err(CommandError::validation_at(
            "connection.port",
            "The port must be positive.",
        ));
    }
    crate::connections::reject_tls(submission.use_tls)?;
    if crate::connections::adapters::descriptor(&submission.kind).is_none() {
        return Err(CommandError::unsupported_adapter(&submission.kind));
    }
    let api_key = submission
        .api_key
        .take()
        .map(Zeroizing::new)
        .map(|key| Zeroizing::new(key.trim().to_string()))
        .filter(|key| !key.is_empty());
    Ok(WebcamHost {
        config: ConnectionConfig {
            kind: submission.kind,
            host: submission.host,
            port: submission.port,
            use_tls: submission.use_tls,
            credential_ref: None,
        },
        api_key,
    })
}

/// `get_printer_camera`: the Printer's source, manual URL included — the
/// single command that returns one (for the Setup editor). `null` when the
/// Printer has no camera.
#[tauri::command]
pub async fn get_printer_camera<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    printer_id: String,
) -> Result<CommandSuccess<Option<PrinterCamera>>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let (exists, camera) = services
        .storage
        .read_transaction(|tx| {
            Ok((
                printer_exists(tx, &printer_id)?,
                config::get(tx, &printer_id)?,
            ))
        })
        .map_err(storage_error)?;
    if !exists {
        return Err(CommandError::not_found(&printer_id));
    }
    Ok(CommandSuccess::new(camera))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetDigest<'a> {
    printer_id: &'a str,
    source: &'a CameraSource,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PrinterDigest<'a> {
    printer_id: &'a str,
}

/// `set_printer_camera`: validates and stores the source (last writer
/// wins; an identical source is a no-op), resets the Printer's camera
/// health, and returns the redacted summary. A `hostWebcam` source needs a
/// Connection whose adapter can list webcams.
#[tauri::command]
pub async fn set_printer_camera<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    printer_id: String,
    source: CameraSourceInput,
) -> Result<CommandSuccess<PrinterCameraSummary>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let source = config::validate_source(&source.0, "source")?;
    let printer = load_printer(&services, &printer_id)?;
    if let CameraSource::HostWebcam { .. } = source {
        let connection = printer.connection.as_ref().ok_or_else(no_connection)?;
        if !resolve::supports_host_webcams(&connection.kind) {
            return Err(CommandError::camera_unsupported_adapter(Some(&printer_id)));
        }
    }
    let digest = operations::digest(&SetDigest {
        printer_id: &printer_id,
        source: &source,
    });
    let now = now_text();
    let (camera, changed) = services
        .storage
        .write_repo(|tx| {
            if operations::claim(tx, &operation_id, OperationKind::SetPrinterCamera, &digest)?
                == Claim::Replay
            {
                return Ok((config::get(tx, &printer_id)?, false));
            }
            if !printer_exists(tx, &printer_id)? {
                return Err(RepositoryError::NotFound {
                    entity_id: printer_id.clone(),
                });
            }
            let (camera, changed) = config::put(tx, &printer_id, &source, &now)?;
            Ok((Some(camera), changed))
        })
        .map_err(CommandError::from_repository)?;
    // A replay after the camera was cleared has no current source.
    let camera = camera.ok_or_else(|| CommandError::camera_not_configured(&printer_id))?;
    if changed {
        services.cameras.source_changed(
            &app,
            &services.attention.stream,
            &printer_id,
            Some(camera.source.kind()),
        );
    }
    Ok(CommandSuccess::new(camera.summary()))
}

/// `clear_printer_camera`'s result.
#[derive(Serialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterCameraCleared.ts")]
pub struct PrinterCameraCleared {
    pub printer_id: String,
    /// Whether a source was removed. A replay reports whether the Printer
    /// has no source now.
    pub cleared: bool,
}

/// `clear_printer_camera`: removes the Printer's source and resets its
/// health to `notConfigured`. Clearing a Printer with no camera is a
/// no-op (`cleared: false`).
#[tauri::command]
pub async fn clear_printer_camera<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    printer_id: String,
) -> Result<CommandSuccess<PrinterCameraCleared>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let digest = operations::digest(&PrinterDigest {
        printer_id: &printer_id,
    });
    let (cleared, fresh) = services
        .storage
        .write_repo(|tx| {
            if operations::claim(
                tx,
                &operation_id,
                OperationKind::ClearPrinterCamera,
                &digest,
            )? == Claim::Replay
            {
                return Ok((config::get(tx, &printer_id)?.is_none(), false));
            }
            if !printer_exists(tx, &printer_id)? {
                return Err(RepositoryError::NotFound {
                    entity_id: printer_id.clone(),
                });
            }
            Ok((config::delete(tx, &printer_id)?, true))
        })
        .map_err(CommandError::from_repository)?;
    if fresh && cleared {
        services
            .cameras
            .source_changed(&app, &services.attention.stream, &printer_id, None);
    }
    Ok(CommandSuccess::new(PrinterCameraCleared {
        printer_id,
        cleared,
    }))
}

/// `list_host_webcams`: the webcams a Moonraker host lists, by name and
/// service only, through exactly one of a saved Printer's Connection or an
/// unsaved one (the Setup wizard; used exactly as `probe_connection` uses
/// it, and never stored).
#[tauri::command]
pub async fn list_host_webcams<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    printer_id: Option<String>,
    connection: Option<ConnectionSubmission>,
) -> Result<CommandSuccess<Vec<HostWebcam>>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let host = match (&printer_id, connection) {
        (Some(printer_id), None) => {
            let printer = load_printer(&services, printer_id)?;
            camera_services::printer_webcam_host(&services, &printer).ok_or_else(|| {
                CommandError::validation_at("printerId", "This Printer has no Connection.")
            })?
        }
        (None, Some(submission)) => submitted_host(submission)?,
        _ => {
            return Err(CommandError::validation_at(
                "printerId",
                "Give exactly one of printerId and connection.",
            ))
        }
    };
    let printer_ref = printer_id.as_deref();
    let build = crate::connections::adapters::descriptor(&host.config.kind)
        .ok_or_else(|| CommandError::unsupported_adapter(&host.config.kind))?
        .camera
        .ok_or_else(|| CommandError::camera_unsupported_adapter(printer_ref))?;
    let discovery = build(&host.config, host.api_key.clone());
    let budget = services.cameras.timings().fetch;
    match tokio::time::timeout(budget, discovery.cameras()).await {
        Ok(Ok(webcams)) => Ok(CommandSuccess::new(
            webcams
                .into_iter()
                .map(|webcam| HostWebcam {
                    name: webcam.name,
                    service: webcam.service,
                })
                .collect(),
        )),
        Ok(Err(_)) | Err(_) => Err(camera_command_error(
            printer_ref,
            &CameraError::new(CameraErrorKind::WebcamListFailed),
        )),
    }
}

/// `test_camera`: fetches one frame from `source` and returns it; nothing
/// is stored. A `hostWebcam` source is looked up through exactly one of a
/// Printer's Connection (`printerId`) or an unsaved one (`connection`). A
/// `snapshotUrl` needs neither; a `printerId`, if given, only selects
/// whose health a test of the saved source updates.
#[tauri::command]
pub async fn test_camera<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    printer_id: Option<String>,
    connection: Option<ConnectionSubmission>,
    source: CameraSourceInput,
) -> Result<Response, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let source = config::validate_source(&source.0, "source")?;
    let host_webcam = matches!(source, CameraSource::HostWebcam { .. });
    if host_webcam && printer_id.is_some() == connection.is_some() {
        return Err(CommandError::validation_at(
            "printerId",
            "A host webcam test needs exactly one of printerId and connection.",
        ));
    }
    let frame = match &printer_id {
        Some(printer_id) => {
            camera_services::test_printer_source(&services, &app, printer_id, &source)
                .await
                .map_err(|error| fetch_error(printer_id, error))?
        }
        None => {
            let host = match (host_webcam, connection) {
                (true, Some(submission)) => Some(submitted_host(submission)?),
                _ => None,
            };
            Arc::new(
                camera_services::fetch_source(&services, &source, host.as_ref())
                    .await
                    .map_err(|error| camera_command_error(None, &error))?,
            )
        }
    };
    Ok(frame_response(&frame))
}

/// `camera_preview_frame`: the Printer's current frame for the Camera
/// tab, never stored. Coalesced: one fetch per Printer at a time, and a
/// frame younger than the preview interval is served again.
#[tauri::command]
pub async fn camera_preview_frame<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    printer_id: String,
) -> Result<Response, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let frame = camera_services::preview_frame(&services, &app, &printer_id)
        .await
        .map_err(|error| fetch_error(&printer_id, error))?;
    Ok(frame_response(&frame))
}
