//! P9 whole-Farm backups (spec D1–D5, D9, D18; ADR-0016). This task's part:
//!
//! - [`manifest`]: the closed `formatVersion` 1 manifest and the
//!   compatibility window (D2, D4);
//! - [`archive`]: the zip reader and writer, the entry path rules, the
//!   limits, and streaming SHA-256 (D2, D3);
//! - [`inventory`]: what goes in, the media pre-pass, and sanitization of
//!   the database copy (D5);
//! - [`lease`]: the process-wide [`lease::BackupLease`] that holds back
//!   every blob and media deletion while a backup runs (D5);
//! - [`writer`]: D5's writer, used by `create_backup` and every safety
//!   backup;
//! - [`safety`]: safety-backup location, verification, retention, and
//!   listing (D9);
//! - [`process_ops`]: the process-local `operationId` ledger (D18);
//! - [`dialogs`]: the injected native file dialogs (D18);
//! - [`commands`]: `backup_inventory`, `create_backup`, `list_backups`,
//!   and `delete_backup`.
//!
//! This file holds the wire types (ts-rs, camelCase).

pub mod archive;
pub mod commands;
pub mod dialogs;
pub mod inventory;
pub mod lease;
pub mod manifest;
pub mod process_ops;
pub mod safety;
pub mod writer;

use std::sync::{Arc, Weak};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::library::content::ContentStore;
use crate::persistence::Storage;

/// Which camera snapshots a backup carries (owner decision 2).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/BackupMediaChoice.ts")]
pub enum BackupMediaChoice {
    None,
    Pinned,
    All,
}

impl BackupMediaChoice {
    pub const ALL: [BackupMediaChoice; 3] = [
        BackupMediaChoice::None,
        BackupMediaChoice::Pinned,
        BackupMediaChoice::All,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            BackupMediaChoice::None => "none",
            BackupMediaChoice::Pinned => "pinned",
            BackupMediaChoice::All => "all",
        }
    }
}

/// Who wrote a backup: the operator, or farm3d before a restore or a reset.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/BackupOrigin.ts")]
pub enum BackupOrigin {
    Operator,
    BeforeRestore,
    BeforeReset,
}

impl BackupOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            BackupOrigin::Operator => "operator",
            BackupOrigin::BeforeRestore => "beforeRestore",
            BackupOrigin::BeforeReset => "beforeReset",
        }
    }
}

/// The machine-specific classes a backup never carries (D2 `excluded`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/BackupExcludedClass.ts")]
pub enum BackupExcludedClass {
    Credentials,
    SlicerRuntimePaths,
    PrinterStatusCache,
    PendingCredentialCleanup,
    PendingBlobCleanup,
    Logs,
}

impl BackupExcludedClass {
    /// Exactly the six classes, in the manifest's order.
    pub const ALL: [BackupExcludedClass; 6] = [
        BackupExcludedClass::Credentials,
        BackupExcludedClass::SlicerRuntimePaths,
        BackupExcludedClass::PrinterStatusCache,
        BackupExcludedClass::PendingCredentialCleanup,
        BackupExcludedClass::PendingBlobCleanup,
        BackupExcludedClass::Logs,
    ];
}

/// One table's exact `count(*)`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/TableCount.ts")]
pub struct TableCount {
    pub table: String,
    #[ts(type = "number")]
    pub rows: i64,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/BackupContentTotals.ts")]
pub struct BackupContentTotals {
    #[ts(type = "number")]
    pub count: i64,
    #[ts(type = "number")]
    pub bytes: i64,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/BackupMediaTotals.ts")]
pub struct BackupMediaTotals {
    pub choice: BackupMediaChoice,
    #[ts(type = "number")]
    pub count: i64,
    #[ts(type = "number")]
    pub bytes: i64,
}

/// `backup_inventory`: what a backup of the live Farm would hold.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/BackupInventory.ts")]
pub struct BackupInventory {
    /// The live database's `page_count × page_size`.
    #[ts(type = "number")]
    pub database_bytes: i64,
    pub content: BackupContentTotals,
    /// One per choice, in [`BackupMediaChoice::ALL`] order.
    pub media: Vec<BackupMediaTotals>,
    /// Sorted by table.
    pub counts: Vec<TableCount>,
    #[ts(type = "number")]
    pub credential_ref_count: i64,
    #[ts(type = "number")]
    pub active_job_count: i64,
    pub excluded: Vec<BackupExcludedClass>,
}

/// Why a desktop-only command didn't run (the frontend's web-mode
/// wrappers answer with it; Rust never does).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/DesktopRequiredReason.ts"
)]
pub enum DesktopRequiredReason {
    DesktopRequired,
}

/// `create_backup`'s result. A result never carries a full path.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/CreateBackupOutcome.ts"
)]
pub enum CreateBackupOutcome {
    Cancelled,
    Exported {
        exported_at: String,
        /// The destination's basename.
        file_name: String,
        #[ts(type = "number")]
        bytes: u64,
        media: BackupMediaChoice,
        #[ts(type = "number")]
        media_not_in_backup: i64,
        #[ts(type = "number")]
        media_missing_file: i64,
    },
    Unsupported {
        reason: DesktopRequiredReason,
    },
}

/// One file in `<backup_root>/safety/` (`list_backups`).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/BackupSummary.ts")]
pub struct BackupSummary {
    pub backup_id: String,
    pub origin: BackupOrigin,
    pub created_at: Option<String>,
    #[ts(type = "number")]
    pub bytes: u64,
    pub app_version: Option<String>,
    #[ts(type = "number | null")]
    pub schema_version: Option<i64>,
    pub media: Option<BackupMediaChoice>,
    /// `false`: the file didn't parse; only delete is offered.
    pub valid: bool,
}

/// `delete_backup`'s result.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/DeleteBackupOutcome.ts")]
pub struct DeleteBackupOutcome {
    pub backup_id: String,
    #[ts(type = "true")]
    pub deleted: bool,
}

/// D3: why an archive was refused (`BACKUP_INVALID`'s `details.reason`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/BackupInvalidReason.ts")]
pub enum BackupInvalidReason {
    NotAZip,
    TooManyEntries,
    UnsafePath,
    UnsupportedEntry,
    DuplicatePath,
    ManifestMissing,
    ManifestTooLarge,
    ManifestInvalid,
    EntryUnlisted,
    EntryMissing,
    SizeMismatch,
    ChecksumMismatch,
    ContentNameMismatch,
    MigrationMismatch,
    CountMismatch,
    DatabaseInvalid,
}

/// Everything the backup commands share (spec "Module layout":
/// `RuntimeServices.backup`): the lease, the process-local operation
/// ledger, and the dialogs.
pub struct BackupServices {
    pub lease: lease::BackupLease,
    pub operations: process_ops::ProcessOperations,
    pub dialogs: Arc<dyn dialogs::PortabilityDialogs>,
    /// Test hooks for the writer (free space, and a pause after the
    /// database copy). Production leaves them empty.
    pub writer_hooks: writer::WriterHooks,
}

impl BackupServices {
    /// Services on `lease` (the one shared with the content store and the
    /// media janitor). Registers the lease's release hook: dropping the
    /// lease retries the content store's deferred blob cleanup once.
    pub fn new(
        lease: lease::BackupLease,
        storage: &Arc<Storage>,
        content: &Arc<ContentStore>,
        dialogs: Arc<dyn dialogs::PortabilityDialogs>,
    ) -> Self {
        let storage = Arc::clone(storage);
        let content: Weak<ContentStore> = Arc::downgrade(content);
        lease.on_release(move || {
            if let Some(content) = content.upgrade() {
                // A blob that can't be unlinked keeps its pending row for
                // the next release or the startup sweep.
                let _ = content.release_unreferenced(&storage);
            }
        });
        Self {
            lease,
            operations: process_ops::ProcessOperations::default(),
            dialogs,
            writer_hooks: writer::WriterHooks::default(),
        }
    }

    /// The same services with other dialogs (tests inject a fake).
    pub fn with_dialogs(&self, dialogs: Arc<dyn dialogs::PortabilityDialogs>) -> Self {
        Self {
            lease: self.lease.clone(),
            operations: process_ops::ProcessOperations::default(),
            dialogs,
            writer_hooks: writer::WriterHooks::default(),
        }
    }
}
