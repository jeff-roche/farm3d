//! P8 D9: the camera domain's pure wire types — a Printer's configured
//! camera source, its health, and the `camera_snapshots` evidence record.
//! See the P8 design spec's "Backend model" module layout table.
//!
//! This module (Task 3) is wire types only: source resolution
//! (`resolve.rs`), the frame fetcher (`fetch.rs`), the media store
//! (`media.rs`), retention (`retention.rs`), capture (`capture.rs`),
//! services (`services.rs`), and commands (`commands.rs`) are later tasks
//! (see the module layout table in the design spec).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// D9: how a Printer's camera is reached.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CameraSourceKind.ts")]
pub enum CameraSourceKind {
    HostWebcam,
    SnapshotUrl,
}

impl CameraSourceKind {
    pub const ALL: [CameraSourceKind; 2] =
        [CameraSourceKind::HostWebcam, CameraSourceKind::SnapshotUrl];
}

/// D9/D4: a Printer's configured camera source. `HostWebcam` names a
/// Moonraker webcam by name, resolved against the Printer's Connection at
/// fetch time; `SnapshotUrl` is a manual, fully-formed URL (global
/// constraint 3: it never enters an event, error, log, or persisted
/// payload other than this column and the Printers export file).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/CameraSource.ts"
)]
pub enum CameraSource {
    HostWebcam {
        webcam_name: String,
        webcam_service: Option<String>,
        web_port: Option<u16>,
    },
    SnapshotUrl {
        snapshot_url: String,
    },
}

/// `set_printer_camera`'s argument shape — structurally identical to
/// [`CameraSource`] (D9 decision: "the wire types" table gives it its own
/// name for the input side, but it is the same type). `#[serde(transparent)]`
/// makes this newtype serialize and deserialize exactly as `CameraSource`
/// does, so it is a real alias on the wire, not a wrapped object; ts-rs's
/// own newtype handling (no `#[ts(type = ...)]` override — that would drop
/// the dependency and leave `CameraSource` unimported) renders it as one on
/// the TypeScript side too: `export type CameraSourceInput = CameraSource;`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(transparent)]
#[ts(export_to = "domain/CameraSourceInput.ts")]
pub struct CameraSourceInput(pub CameraSource);

/// `get_printer_camera`'s result: the only command that returns a manual
/// URL (global constraint 3's one exception).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterCamera.ts")]
pub struct PrinterCamera {
    pub printer_id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub source: CameraSource,
    pub updated_at: String,
}

/// A Moonraker `webcams.list` entry, as `list_host_webcams` returns it.
/// Never a URL (D4/global constraint 3): just enough to let the operator
/// pick a name.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/HostWebcam.ts")]
pub struct HostWebcam {
    pub name: String,
    pub service: String,
}

/// D9: why a frame fetch failed. `hostMismatch` and `unsupportedAdapter`
/// map to their own error codes (`CAMERA_HOST_MISMATCH`,
/// `CAPABILITY_UNSUPPORTED`); every other kind is `CAMERA_FAILED`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CameraErrorKind.ts")]
pub enum CameraErrorKind {
    Unreachable,
    Timeout,
    HttpStatus,
    TooLarge,
    NotAnImage,
    NoSuchWebcam,
    NoSnapshotUrl,
    HostMismatch,
    UnsupportedAdapter,
    WebcamListFailed,
}

impl CameraErrorKind {
    pub const ALL: [CameraErrorKind; 10] = [
        CameraErrorKind::Unreachable,
        CameraErrorKind::Timeout,
        CameraErrorKind::HttpStatus,
        CameraErrorKind::TooLarge,
        CameraErrorKind::NotAnImage,
        CameraErrorKind::NoSuchWebcam,
        CameraErrorKind::NoSnapshotUrl,
        CameraErrorKind::HostMismatch,
        CameraErrorKind::UnsupportedAdapter,
        CameraErrorKind::WebcamListFailed,
    ];
}

/// Why a capture that was attempted didn't produce a snapshot (D10/D11:
/// "evidenceSkipped means tried and couldn't" — nothing is recorded when
/// the toggle is off, there is no source, or the Incident came from the
/// backfill). Shared by `AttentionEvent.evidence` (`EvidenceOutcome`) and
/// `IncidentEntryDetail::EvidenceSkipped`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/EvidenceSkipReason.ts")]
pub enum EvidenceSkipReason {
    CameraError,
    DiskCap,
}

impl EvidenceSkipReason {
    pub const ALL: [EvidenceSkipReason; 2] =
        [EvidenceSkipReason::CameraError, EvidenceSkipReason::DiskCap];
}

/// D9: a Printer's camera preview health, one row per Printer with a
/// camera source.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CameraHealthState.ts")]
pub enum CameraHealthState {
    NotConfigured,
    Unsupported,
    Unknown,
    Ok,
    Failing,
}

impl CameraHealthState {
    pub const ALL: [CameraHealthState; 5] = [
        CameraHealthState::NotConfigured,
        CameraHealthState::Unsupported,
        CameraHealthState::Unknown,
        CameraHealthState::Ok,
        CameraHealthState::Failing,
    ];
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CameraHealth.ts")]
pub struct CameraHealth {
    pub printer_id: String,
    pub state: CameraHealthState,
    pub source_kind: Option<CameraSourceKind>,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub last_failure_kind: Option<CameraErrorKind>,
}

/// D10: what captured a `camera_snapshots` row.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SnapshotTrigger.ts")]
pub enum SnapshotTrigger {
    Incident,
    Completion,
    Manual,
}

impl SnapshotTrigger {
    pub const ALL: [SnapshotTrigger; 3] = [
        SnapshotTrigger::Incident,
        SnapshotTrigger::Completion,
        SnapshotTrigger::Manual,
    ];
}

/// D11: why a snapshot's image was pruned. The row survives; only the
/// file is removed.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PruneReason.ts")]
pub enum PruneReason {
    Age,
    DiskCap,
    MissingFile,
}

impl PruneReason {
    pub const ALL: [PruneReason; 3] =
        [PruneReason::Age, PruneReason::DiskCap, PruneReason::MissingFile];
}

/// A captured frame's image format — `camera_snapshots.content_type`
/// and `FrameHeader.contentType` (magic-byte checked, never the request
/// header).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[ts(export_to = "domain/CameraContentType.ts")]
pub enum CameraContentType {
    #[serde(rename = "image/jpeg")]
    #[ts(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/png")]
    #[ts(rename = "image/png")]
    Png,
}

impl CameraContentType {
    pub const ALL: [CameraContentType; 2] = [CameraContentType::Jpeg, CameraContentType::Png];
}

/// `camera_snapshots` in full (spec "Backend model" wire types). Never
/// `rel_path` (the media root is Rust-only).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CameraSnapshot.ts")]
pub struct CameraSnapshot {
    pub id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub printer_id: String,
    pub incident_id: Option<String>,
    pub job_id: Option<String>,
    pub trigger: SnapshotTrigger,
    pub captured_at: String,
    pub content_type: CameraContentType,
    #[ts(type = "number")]
    pub byte_len: i64,
    pub sha256: String,
    pub pinned_at: Option<String>,
    pub pruned_at: Option<String>,
    pub prune_reason: Option<PruneReason>,
}

/// `media_usage`'s result (D11): usage is the sum of stored `byteLen`
/// over unpruned rows, never a filesystem walk.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/MediaUsage.ts")]
pub struct MediaUsage {
    #[ts(type = "number")]
    pub used_bytes: i64,
    #[ts(type = "number")]
    pub pinned_bytes: i64,
    #[ts(type = "number")]
    pub cap_bytes: i64,
    #[ts(type = "number")]
    pub retention_days: i64,
    #[ts(type = "number")]
    pub snapshot_count: i64,
    #[ts(type = "number")]
    pub pinned_count: i64,
    #[ts(type = "number")]
    pub pruned_count: i64,
}

/// The binary-frame header (`test_camera`, `camera_preview_frame`,
/// `snapshot_image`): bytes 0-3 are this header's length, big-endian;
/// this JSON follows; the image bytes follow that. `src/cameras/frame.ts`
/// parses it. `snapshotId` is set by `snapshot_image` only.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/FrameHeader.ts")]
pub struct FrameHeader {
    pub content_type: CameraContentType,
    pub captured_at: String,
    #[ts(type = "number")]
    pub byte_len: i64,
    pub snapshot_id: Option<String>,
}
