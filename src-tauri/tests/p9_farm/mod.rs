//! The every-domain Farm (P9 Task 5's first cut; Task 11 extends it
//! without renaming). A real migrated `Storage` with a row in every domain
//! the backup, restore, and reference-integrity tests read, real content
//! blobs and camera media files whose bytes hash to their names, stored
//! credentials, and the P9 secret corpus seeded where a backup must not
//! carry it. Every id and timestamp is fixed (Task 17 relies on the ids).
//!
//! Wraps `tests/common/farm_seed.rs::seed_every_domain` (raw SQL) and adds
//! what a backup needs on top.
//!
//! Include with `mod p9_farm;` next to `#[path = "common/secrets.rs"] mod
//! secrets;` (this module uses `crate::secrets`).
#![allow(dead_code)]

#[path = "../common/farm_seed.rs"]
pub mod farm_seed;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use farm3d_lib::catalog::{BedShape, PrinterProfile};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::jobs::PrinterSnapshot;
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};
use farm3d_lib::printers::CatalogRef;
use farm3d_lib::settings::repository::SettingsRepository;
use farm3d_lib::slicing::facts::{Farm3dFacts, ProfileSnapshot};
use farm3d_lib::slicing::repository::{insert_farm3d_revision, NewFarm3dRevision};
use farm3d_lib::slicing::{
    RuntimeChannel, SliceControls, SliceEstimates, SlicePlateRef, SliceRevisionTarget,
    SliceRuntimeInfo, SliceTarget,
};
use farm3d_lib::spools::MaterialFamily;

use crate::secrets;
use farm_seed::{
    exec, seed_every_domain, seed_printer, GCODE_BYTES, GCODE_HASH, NOW, THREE_MF_INSPECTION,
};

/// The fixed ids and timestamps (Task 17 depends on them).
pub mod ids {
    /// `farm_seed`'s Printer (renamed to the corpus Printer name), with a
    /// Connection whose `credentialRef` is [`CREDENTIAL_REF_A`], a stored
    /// camera URL, and a status snapshot.
    pub const PRINTER_A: &str = "prn-a";
    /// A second Printer with its own `credentialRef`.
    pub const PRINTER_B: &str = "prn-b";
    pub const CREDENTIAL_REF_A: &str = "farm3d/printer/prn-a/apikey";
    pub const CREDENTIAL_REF_B: &str = "farm3d/printer/prn-b/apikey";
    /// `farm_seed`'s G-code Model (held by Slice Revisions and a Job).
    pub const MODEL_GCODE: &str = "mdl-a";
    /// A managed 3MF Model with a thumbnail, referenced by nothing else, so
    /// it can be deleted.
    pub const MODEL_3MF: &str = "mdl-3mf";
    pub const REVISION_3MF: &str = "msr-3mf";
    pub const PROJECT: &str = "prj-a";
    pub const SLICE_REVISION: &str = "slr-a";
    pub const JOB: &str = "job-a";
    /// An incident snapshot (unpinned, unpruned).
    pub const SNAPSHOT_INCIDENT: &str = "snp-a";
    /// A manual snapshot, the completion evidence (unpinned, unpruned).
    pub const SNAPSHOT_MANUAL: &str = "snp-b";
    /// A pinned manual snapshot (unpruned).
    pub const SNAPSHOT_PINNED: &str = "snp-c";
    /// A manual snapshot already pruned (`age`); it has no file.
    pub const SNAPSHOT_PRUNED: &str = "snp-d";
    /// A `pending_blob_cleanup` hash with no row and no file.
    pub const PENDING_BLOB: &str =
        "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    pub const NOW: &str = super::farm_seed::NOW;

    // --- Task 11's extension (every id below is fixed too) ---------------

    /// `farm_seed`'s Spool (`#1`): Job A's, with Job A's active
    /// reservation and the failed Job's unresolved one.
    pub const SPOOL_A: &str = "spl-a";
    /// An archived Printer (profile-only) with a camera, alert defaults, a
    /// slot, and a Spool's movement history through that slot.
    pub const PRINTER_ARCHIVED: &str = "prn-archived";
    /// Printer B's two slots; [`SPOOL_LOADED`] sits in the first.
    pub const SLOT_B1: &str = "slt-b1";
    pub const SLOT_B2: &str = "slt-b2";
    /// The archived Printer's slot.
    pub const SLOT_ARCHIVED: &str = "slt-x1";
    /// Loaded in [`SLOT_B1`] (`#2`).
    pub const SPOOL_LOADED: &str = "spl-loaded";
    /// Archived from `active`, in storage, weighed with [`TARE`] (`#3`).
    pub const SPOOL_ARCHIVED: &str = "spl-archived";
    pub const TARE: &str = "tar-a";
    /// [`SPOOL_LOADED`]'s load into [`SLOT_B1`].
    pub const MOVEMENT_LOAD_B: &str = "mov-load-b";
    /// [`SPOOL_ARCHIVED`]'s load into, and relocation out of,
    /// [`SLOT_ARCHIVED`] when its Printer was archived.
    pub const MOVEMENT_LOAD_ARCHIVED: &str = "mov-load-x";
    pub const MOVEMENT_ARCHIVED: &str = "mov-archive-x";
    /// A second revision of [`MODEL_3MF`] (same content, its own thumbnail
    /// row).
    pub const REVISION_3MF_2: &str = "msr-3mf-2";
    /// A linked 3MF Model with two revisions, each with a thumbnail; the
    /// farm3d Slice Revision is sliced from its first.
    pub const MODEL_LINKED: &str = "mdl-link";
    pub const REVISION_LINKED_1: &str = "msr-link-1";
    pub const REVISION_LINKED_2: &str = "msr-link-2";
    /// Its path outside the Farm (an absolute path that isn't a home).
    pub const LINKED_PATH: &str = "/srv/farm3d-models/linked.3mf";
    /// `farm_seed`'s G-code Model's source revision.
    pub const REVISION_GCODE: &str = "msr-a";
    /// `farm_seed`'s Preparation, targeting Printer A.
    pub const PREPARATION: &str = "prp-a";
    /// `farm_seed`'s second external Slice Revision, targeting Printer A,
    /// referenced by nothing.
    pub const SLICE_REVISION_TARGETED: &str = "slr-t";
    /// A farm3d Slice Revision of [`REVISION_LINKED_1`], targeting the
    /// archived Printer, referenced by nothing.
    pub const SLICE_REVISION_FARM3D: &str = "slr-farm";
    /// Job A's Queue Entry (`farm_seed` leaves it `queued`).
    pub const QUEUE_ENTRY_A: &str = "qen-a";
    /// An open (`queued`) entry on [`SLICE_REVISION`], pinned to Printer B.
    pub const QUEUE_OPEN: &str = "qen-open";
    /// The failed Job's closed entry, with a successor.
    pub const QUEUE_FAILED: &str = "qen-failed";
    /// [`QUEUE_FAILED`]'s `retry` successor (`queued`).
    pub const QUEUE_RETRY: &str = "qen-retry";
    /// The cancelled Job's closed entry.
    pub const QUEUE_CANCELLED: &str = "qen-cancelled";
    /// A failed Job on Printer A whose material settlement is `pending`.
    pub const JOB_FAILED: &str = "job-failed";
    /// A Job cancelled before it started.
    pub const JOB_CANCELLED: &str = "job-cancelled";
    pub const RESERVATION_A: &str = "rsv-a";
    pub const RESERVATION_FAILED: &str = "rsv-failed";
    pub const RESERVATION_CANCELLED: &str = "rsv-cancelled";
    /// Job A's correction amount event.
    pub const CORRECTION_EVENT: &str = "sev-a";
    /// Job A's `materialCorrected` Job event (its detail names
    /// [`CORRECTION_EVENT`]).
    pub const JOB_EVENT_CORRECTED: &str = "jev-a-1";
    /// The failed Job's pending material reconciliation.
    pub const REQUIREMENT_PENDING: &str = "rrq-failed";
    /// Job A's resolved material reconciliation.
    pub const REQUIREMENT_RESOLVED: &str = "rrq-a";
    pub const INCIDENT: &str = "inc-a";
    /// Incident A's note.
    pub const INCIDENT_NOTE: &str = "iev-a-note";
    /// `farm_seed`'s open `printer.offline` Event on Printer A.
    pub const ATTENTION_OPEN: &str = "att-off";
    /// `farm_seed`'s `job.completed` Event with captured evidence.
    pub const ATTENTION_COMPLETED: &str = "att-done";
    /// A resolved `printer.connectionError` Event on Printer B, and its
    /// open recurrence.
    pub const ATTENTION_RESOLVED: &str = "att-conn-old";
    pub const ATTENTION_RECURRENCE: &str = "att-conn";
    /// The pending requirement's open Event (its detail names Spool A).
    pub const ATTENTION_REQUIREMENT: &str = "att-rrq";
    /// `farm_seed`'s unresolved (`dispatching`) upload on Printer A.
    pub const HOST_OPERATION_UNRESOLVED: &str = "hop-a";
    /// A succeeded upload on Printer A (Job A's).
    pub const HOST_OPERATION_SUCCEEDED: &str = "hop-done";
    /// A failed upload on Printer B of [`SLICE_REVISION_TARGETED`].
    pub const HOST_OPERATION_FAILED: &str = "hop-failed";
}

pub struct Farm {
    pub temp: tempfile::TempDir,
    pub metadata_lease: MetadataRootLease,
    pub storage: Arc<Storage>,
    /// Where the credential store keeps `credentials.json` (the metadata
    /// root, as in production).
    pub credentials_dir: PathBuf,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// A small but real 3MF-shaped zip, so nested-archive scans recurse into
/// it. Deterministic (the zip crate's default timestamp).
pub fn three_mf_bytes(label: &str) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("3D/3dmodel.model", options).unwrap();
    writer
        .write_all(format!("<model label=\"{label}\"/>").as_bytes())
        .unwrap();
    writer.finish().unwrap().into_inner()
}

pub fn thumbnail_bytes() -> Vec<u8> {
    b"\x89PNG\r\n\x1a\n p9 thumbnail".to_vec()
}

/// The linked Model's embedded thumbnail (its own blob).
pub fn linked_thumbnail_bytes() -> Vec<u8> {
    b"\x89PNG\r\n\x1a\n p9 linked thumbnail".to_vec()
}

pub fn snapshot_bytes(id: &str) -> Vec<u8> {
    format!("\u{ff}\u{d8} p9 snapshot {id}").into_bytes()
}

pub fn snapshot_rel_path(id: &str) -> String {
    format!("snapshots/2026/01/{id}.jpg")
}

impl Farm {
    /// The Farm: `seed_every_domain` plus the backup-relevant rows, files,
    /// and credentials. See [`ids`].
    pub fn with_every_domain() -> Farm {
        let temp = tempfile::tempdir().expect("temp");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("paths");
        let metadata_lease = MetadataRootLease::acquire(&paths).expect("lease");
        let storage = Arc::new(Storage::open(paths, &metadata_lease).expect("storage"));
        SettingsRepository::new(Arc::clone(&storage))
            .ensure_default()
            .expect("settings");
        let credentials_dir = storage.paths().metadata_root().to_path_buf();
        let farm = Farm {
            temp,
            metadata_lease,
            storage,
            credentials_dir,
        };
        farm.seed();
        farm
    }

    pub fn paths(&self) -> &StoragePaths {
        self.storage.paths()
    }

    pub fn blob_path(&self, sha256: &str) -> PathBuf {
        self.paths()
            .content_root()
            .join("blobs/sha256")
            .join(&sha256[..2])
            .join(sha256)
    }

    pub fn media_file(&self, rel_path: &str) -> PathBuf {
        self.paths().media_root().join(rel_path)
    }

    pub fn write_blob(&self, bytes: &[u8]) -> String {
        let sha256 = sha256_hex(bytes);
        let path = self.blob_path(&sha256);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        sha256
    }

    /// `(3MF source hash, thumbnail hash, plate 3MF hash)`.
    pub fn blob_hashes(&self) -> (String, String, String) {
        (
            sha256_hex(&three_mf_bytes("source")),
            sha256_hex(&thumbnail_bytes()),
            sha256_hex(&three_mf_bytes("plate")),
        )
    }

    /// `(linked 3MF source hash, linked thumbnail hash)`: the linked
    /// Model's own blobs, shared by its two revisions.
    pub fn linked_blob_hashes(&self) -> (String, String) {
        (
            sha256_hex(&three_mf_bytes("linked")),
            sha256_hex(&linked_thumbnail_bytes()),
        )
    }

    /// Every `content_blobs` hash, sorted.
    pub fn content_hashes(&self) -> Vec<String> {
        let mut hashes = vec![GCODE_HASH.to_string()];
        let (source, thumbnail, plate) = self.blob_hashes();
        hashes.extend([source, thumbnail, plate]);
        let (linked_source, linked_thumbnail) = self.linked_blob_hashes();
        hashes.extend([linked_source, linked_thumbnail]);
        hashes.sort();
        hashes
    }

    /// Adds an unpinned `manual` snapshot `id` of Printer A, linked to no
    /// Incident or Event, with its image file (for retention tests, which
    /// would otherwise prune the linked seeds).
    pub fn add_unlinked_snapshot(&self, id: &str) {
        let rel_path = snapshot_rel_path(id);
        let bytes = snapshot_bytes(id);
        self.write(|tx| {
            farm_seed::seed_manual_snapshot(tx, id, ids::PRINTER_A, &rel_path);
            tx.execute(
                "UPDATE camera_snapshots SET byte_len = ?1, sha256 = ?2 WHERE id = ?3",
                params![bytes.len() as i64, sha256_hex(&bytes), id],
            )
            .unwrap();
        });
        let path = self.media_file(&rel_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// Pins every snapshot the seed links to an Incident or an Event, so
    /// retention leaves them alone.
    pub fn pin_linked_snapshots(&self) {
        self.write(|tx| {
            tx.execute(
                "UPDATE camera_snapshots SET pinned_at = ?1 WHERE id IN (?2, ?3)",
                params![NOW, ids::SNAPSHOT_INCIDENT, ids::SNAPSHOT_MANUAL],
            )
            .unwrap();
        });
    }

    /// Exact `count(*)` of every table, by name.
    pub fn counts(&self) -> BTreeMap<String, i64> {
        table_counts(&Connection::open(self.paths().database()).unwrap())
    }

    fn write(&self, seed: impl FnOnce(&Connection)) {
        self.storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed(tx);
                Ok(())
            })
            .expect("seed");
    }

    fn seed(&self) {
        let (source_hash, thumbnail_hash, plate_hash) = self.blob_hashes();
        self.write(|tx| {
            seed_every_domain(tx);
            seed_printer(tx, ids::PRINTER_B);
            let connection = |host: &str, reference: &str| {
                serde_json::json!({
                    "kind": "moonraker",
                    "host": host,
                    "port": 7125,
                    "useTls": false,
                    "credentialRef": reference,
                })
                .to_string()
            };
            tx.execute(
                "UPDATE printers SET name = ?1, connection_json = ?2, location = 'Bay A'
                  WHERE id = ?3",
                params![
                    secrets::PRINTER_NAME,
                    connection(secrets::HOST, ids::CREDENTIAL_REF_A),
                    ids::PRINTER_A
                ],
            )
            .unwrap();
            tx.execute(
                "UPDATE printers SET connection_json = ?1 WHERE id = ?2",
                params![
                    connection(secrets::HOST_NAME, ids::CREDENTIAL_REF_B),
                    ids::PRINTER_B
                ],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO printer_cameras(printer_id, source_kind, snapshot_url, updated_at)
                 VALUES (?1, 'snapshotUrl', ?2, ?3)",
                params![ids::PRINTER_A, secrets::STORED_CAMERA_URL, NOW],
            )
            .unwrap();
            // The telemetry cache: excluded from a backup (D5), so the
            // corpus header seeded here must not survive into the copy.
            tx.execute(
                "INSERT INTO printer_status_snapshots(printer_id, telemetry_json,
                   last_observed_at, persisted_at)
                 VALUES (?1, ?2, ?3, ?3)",
                params![
                    ids::PRINTER_A,
                    serde_json::json!({ "note": secrets::HEADER_LINE }).to_string(),
                    NOW
                ],
            )
            .unwrap();
            // This machine's Slicer runtime paths (nulled in a backup).
            tx.execute(
                "INSERT OR REPLACE INTO slicer_runtime_config(singleton_id, revision,
                   engine_path, preset_source_path, updated_at)
                 VALUES (1, 1, ?1, ?2, ?3)",
                params![
                    format!("{}/orca/orca-slicer", secrets::HOME_PATH),
                    format!("{}/orca/presets", secrets::HOME_PATH),
                    NOW
                ],
            )
            .unwrap();
            exec(
                tx,
                &format!(
                    "INSERT INTO pending_blob_cleanup(sha256, created_at)
                       VALUES ('{pending}', '{NOW}');
                     INSERT INTO library_projects(id, revision, name, created_at, updated_at)
                       VALUES ('{project}', 1, 'Project A', '{NOW}', '{NOW}');
                     INSERT INTO project_models(project_id, model_id, added_at)
                       VALUES ('{project}', '{model}', '{NOW}');",
                    pending = ids::PENDING_BLOB,
                    project = ids::PROJECT,
                    model = ids::MODEL_GCODE,
                ),
            );
            // A managed 3MF Model (stored uncompressed in a backup) with a
            // thumbnail, and a plate 3MF on the Slice Revision.
            exec(
                tx,
                &format!(
                    "INSERT INTO content_blobs(sha256, size_bytes, created_at) VALUES
                       ('{source_hash}', {source_len}, '{NOW}'),
                       ('{thumbnail_hash}', {thumbnail_len}, '{NOW}'),
                       ('{plate_hash}', {plate_len}, '{NOW}');
                     INSERT INTO library_models(id, revision, name, format, storage_mode,
                       created_at, updated_at)
                       VALUES ('{model}', 1, 'Bracket', '3mf', 'managed', '{NOW}', '{NOW}');
                     INSERT INTO model_source_revisions(id, model_id, sequence, content_sha256,
                       size_bytes, format, origin, source_file_name, source_path, captured_at,
                       inspector_version, inspection_json)
                       VALUES ('{revision}', '{model}', 1, '{source_hash}', {source_len}, '3mf',
                               'import', 'bracket.3mf', '{home}/models/bracket.3mf', '{NOW}', 1,
                               '{inspection}');
                     INSERT INTO model_revision_thumbnails(revision_id, source, origin_part,
                       media_type, width, height, content_sha256)
                       VALUES ('{revision}', 'embedded', 'Metadata/thumbnail.png', 'image/png',
                               1, 1, '{thumbnail_hash}');
                     INSERT INTO slice_revision_blobs(revision_id, role, sha256)
                       VALUES ('{slice}', 'plate3mf', '{plate_hash}');",
                    source_len = three_mf_bytes("source").len(),
                    thumbnail_len = thumbnail_bytes().len(),
                    plate_len = three_mf_bytes("plate").len(),
                    model = ids::MODEL_3MF,
                    revision = ids::REVISION_3MF,
                    slice = ids::SLICE_REVISION,
                    home = secrets::HOME_PATH,
                    inspection = farm_seed::THREE_MF_INSPECTION,
                ),
            );
            // Snapshots: two unpinned (seeded), one pinned, one pruned.
            farm_seed::seed_manual_snapshot(
                tx,
                ids::SNAPSHOT_PINNED,
                ids::PRINTER_A,
                &snapshot_rel_path(ids::SNAPSHOT_PINNED),
            );
            farm_seed::seed_manual_snapshot(
                tx,
                ids::SNAPSHOT_PRUNED,
                ids::PRINTER_A,
                &snapshot_rel_path(ids::SNAPSHOT_PRUNED),
            );
            tx.execute(
                "UPDATE camera_snapshots SET pinned_at = ?1 WHERE id = ?2",
                params![NOW, ids::SNAPSHOT_PINNED],
            )
            .unwrap();
            tx.execute(
                "UPDATE camera_snapshots SET pruned_at = ?1, prune_reason = 'age' WHERE id = ?2",
                params![NOW, ids::SNAPSHOT_PRUNED],
            )
            .unwrap();
            for id in [
                ids::SNAPSHOT_INCIDENT,
                ids::SNAPSHOT_MANUAL,
                ids::SNAPSHOT_PINNED,
                ids::SNAPSHOT_PRUNED,
            ] {
                let bytes = snapshot_bytes(id);
                tx.execute(
                    "UPDATE camera_snapshots SET byte_len = ?1, sha256 = ?2 WHERE id = ?3",
                    params![bytes.len() as i64, sha256_hex(&bytes), id],
                )
                .unwrap();
            }
        });

        self.seed_extension();

        // A secret written and deleted: it stays in the live database's
        // free pages, which the online backup copies verbatim.
        self.write(|tx| {
            tx.execute(
                "INSERT INTO library_projects(id, revision, name, created_at, updated_at)
                 VALUES ('prj-deleted', 1, ?1, ?2, ?2)",
                params![secrets::USERINFO_URL, NOW],
            )
            .unwrap();
        });
        self.write(|tx| {
            tx.execute("DELETE FROM library_projects WHERE id = 'prj-deleted'", [])
                .unwrap();
        });
        Connection::open(self.paths().database())
            .unwrap()
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();

        // Files: every blob, and every unpruned snapshot's image.
        assert_eq!(self.write_blob(&GCODE_BYTES), GCODE_HASH);
        self.write_blob(&three_mf_bytes("source"));
        self.write_blob(&thumbnail_bytes());
        self.write_blob(&three_mf_bytes("plate"));
        for id in [
            ids::SNAPSHOT_INCIDENT,
            ids::SNAPSHOT_MANUAL,
            ids::SNAPSHOT_PINNED,
        ] {
            let path = self.media_file(&snapshot_rel_path(id));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, snapshot_bytes(id)).unwrap();
        }

        // Credential values live in the store, never in the database.
        let store = CredentialStore::file_backed(self.credentials_dir.clone());
        store
            .set(ids::CREDENTIAL_REF_A, secrets::CREDENTIAL_VALUE)
            .unwrap();
        store
            .set(ids::CREDENTIAL_REF_B, secrets::CREDENTIAL_VALUE_2)
            .unwrap();
    }
}

/// A Job's or an Incident's `printer_snapshot_json` the repositories can
/// decode.
pub fn printer_snapshot_json(name: &str) -> String {
    serde_json::to_string(&PrinterSnapshot {
        name: name.to_string(),
        location: Some("Bay A".to_string()),
        catalog_ref: None,
        adapter_kind: Some("moonraker".to_string()),
        profile: printer_profile(),
    })
    .unwrap()
}

fn printer_profile() -> PrinterProfile {
    PrinterProfile {
        bed_shape: BedShape::Rectangular {
            width_mm: 250.0,
            depth_mm: 250.0,
            origin_x_mm: 0.0,
            origin_y_mm: 0.0,
        },
        printable_height_mm: 250.0,
        bed_exclude_areas: Vec::new(),
        default_bed_type: "4".to_string(),
        nozzle_diameter_mm: vec![0.4],
        nozzle_type: "brass".to_string(),
        gcode_flavor: "klipper".to_string(),
        has_auxiliary_fan: false,
        supports_air_filtration: false,
        supports_multi_filament: false,
        suggested_host_type: None,
    }
}

impl Farm {
    /// Task 11's extension: every domain the reference matrix acts on (see
    /// the second half of [`ids`]). Raw SQL, like `farm_seed`, except the
    /// farm3d Slice Revision, which goes through the repository so its
    /// JSON columns are the real shapes.
    fn seed_extension(&self) {
        use ids::*;
        let (source_hash, thumbnail_hash, _) = self.blob_hashes();
        let source_len = three_mf_bytes("source").len();
        let (linked_hash, linked_thumbnail_hash) = self.linked_blob_hashes();
        let linked_len = three_mf_bytes("linked").len();
        let linked_thumbnail_len = linked_thumbnail_bytes().len();
        let ended = "2026-01-02T00:00:00.000Z";
        self.storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, PRINTER_ARCHIVED);
                exec(
                    tx,
                    &format!(
                        "UPDATE printers SET name = 'Archived Printer', archived_at = '{NOW}',
                                revision = 2 WHERE id = '{PRINTER_ARCHIVED}';
                         INSERT INTO material_slots(id, printer_id, position, name, created_at)
                           VALUES ('{SLOT_B1}', '{PRINTER_B}', 0, 'A1', '{NOW}'),
                                  ('{SLOT_B2}', '{PRINTER_B}', 1, 'A2', '{NOW}'),
                                  ('{SLOT_ARCHIVED}', '{PRINTER_ARCHIVED}', 0, 'A1', '{NOW}');
                         INSERT INTO printer_cameras(printer_id, source_kind, webcam_name,
                           updated_at)
                           VALUES ('{PRINTER_B}', 'hostWebcam', 'bay-b', '{NOW}'),
                                  ('{PRINTER_ARCHIVED}', 'hostWebcam', 'bay-x', '{NOW}');
                         INSERT INTO printer_alert_defaults(printer_id, offline_after_minutes,
                           notifications, snapshot_on_incident, snapshot_on_completion,
                           updated_at)
                           VALUES ('{PRINTER_B}', 5, 'follow', 1, 1, '{NOW}'),
                                  ('{PRINTER_ARCHIVED}', NULL, 'muted', 0, 0, '{NOW}');
                         INSERT INTO spool_tares(id, revision, name, weight_mg, created_at,
                           updated_at)
                           VALUES ('{TARE}', 1, 'Cardboard core', 250000, '{NOW}', '{NOW}');
                         INSERT INTO spools(id, revision, spool_number, manufacturer,
                           material_family, color_name, diameter, nominal_mg, current_mg,
                           confidence, lifecycle, archived_from, slot_id, storage_label, tare_id,
                           created_at, updated_at)
                           VALUES ('{SPOOL_LOADED}', 1, 2, 'Acme', 'PETG', 'Blue', '1.75',
                                   1000000, 800000, 'estimated', 'active', NULL, '{SLOT_B1}',
                                   NULL, NULL, '{NOW}', '{NOW}'),
                                  ('{SPOOL_ARCHIVED}', 1, 3, 'Acme', 'PLA', 'Red', '1.75',
                                   1000000, 300000, 'measured', 'archived', 'active', NULL,
                                   'Shelf 1', '{TARE}', '{NOW}', '{NOW}');
                         INSERT INTO spool_movements(id, operation_id, spool_id, reason,
                           from_slot_id, from_storage_label, to_slot_id, to_storage_label,
                           occurred_at)
                           VALUES ('{MOVEMENT_LOAD_B}', 'op-load-b', '{SPOOL_LOADED}', 'load',
                                   NULL, 'Shelf 1', '{SLOT_B1}', NULL, '{NOW}'),
                                  ('{MOVEMENT_LOAD_ARCHIVED}', 'op-load-x', '{SPOOL_ARCHIVED}',
                                   'load', NULL, 'Shelf 1', '{SLOT_ARCHIVED}', NULL, '{NOW}'),
                                  ('{MOVEMENT_ARCHIVED}', 'op-archive-x', '{SPOOL_ARCHIVED}',
                                   'printerArchived', '{SLOT_ARCHIVED}', NULL, NULL, 'Shelf 1',
                                   '{NOW}');"
                    ),
                );
                // Models: a second managed revision (the same content and
                // thumbnail as the first), and a linked Model with two
                // revisions over its own source and thumbnail blobs.
                exec(
                    tx,
                    &format!(
                        "INSERT INTO content_blobs(sha256, size_bytes, created_at) VALUES
                           ('{linked_hash}', {linked_len}, '{NOW}'),
                           ('{linked_thumbnail_hash}', {linked_thumbnail_len}, '{NOW}');
                         INSERT INTO library_models(id, revision, name, format, storage_mode,
                           linked_path, link_state, link_checked_at, created_at, updated_at)
                           VALUES ('{MODEL_LINKED}', 1, 'Linked bracket', '3mf', 'linked',
                                   '{LINKED_PATH}', 'ok', '{NOW}', '{NOW}', '{NOW}');
                         INSERT INTO model_source_revisions(id, model_id, sequence,
                           content_sha256, size_bytes, format, origin, source_file_name,
                           source_path, captured_at, inspector_version, inspection_json)
                           VALUES ('{REVISION_3MF_2}', '{MODEL_3MF}', 2, '{source_hash}',
                                   {source_len}, '3mf', 'addedRevision', 'bracket-v2.3mf',
                                   '/srv/farm3d-models/bracket-v2.3mf', '{NOW}', 1, '{THREE_MF_INSPECTION}'),
                                  ('{REVISION_LINKED_1}', '{MODEL_LINKED}', 1, '{linked_hash}',
                                   {linked_len}, '3mf', 'import', 'linked.3mf', '{LINKED_PATH}',
                                   '{NOW}', 1, '{THREE_MF_INSPECTION}'),
                                  ('{REVISION_LINKED_2}', '{MODEL_LINKED}', 2, '{linked_hash}',
                                   {linked_len}, '3mf', 'linkedChange', 'linked.3mf',
                                   '{LINKED_PATH}', '{NOW}', 1, '{THREE_MF_INSPECTION}');
                         INSERT INTO model_revision_thumbnails(revision_id, source, origin_part,
                           media_type, width, height, content_sha256)
                           VALUES ('{REVISION_3MF_2}', 'embedded', 'Metadata/thumbnail.png',
                                   'image/png', 1, 1, '{thumbnail_hash}'),
                                  ('{REVISION_LINKED_1}', 'embedded', 'Metadata/thumbnail.png',
                                   'image/png', 1, 1, '{linked_thumbnail_hash}'),
                                  ('{REVISION_LINKED_2}', 'embedded', 'Metadata/thumbnail.png',
                                   'image/png', 1, 1, '{linked_thumbnail_hash}');"
                    ),
                );
                let profile = ProfileSnapshot::new(CatalogRef::default(), &printer_profile());
                insert_farm3d_revision(
                    tx,
                    &NewFarm3dRevision {
                        id: SLICE_REVISION_FARM3D.to_string(),
                        source_revision_id: REVISION_LINKED_1.to_string(),
                        plate: SlicePlateRef {
                            plate_key: "plate-1".to_string(),
                            plate_index: 1,
                            plate_name: Some("Plate 1".to_string()),
                        },
                        gcode_sha256: GCODE_HASH.to_string(),
                        gcode_size: GCODE_BYTES.len() as i64,
                        target: SliceRevisionTarget {
                            target: SliceTarget::Printer {
                                printer_id: PRINTER_ARCHIVED.to_string(),
                            },
                            profile: profile.clone(),
                            machine_preset: "Printer 0.4 nozzle".to_string(),
                            process_preset: "0.20mm Standard".to_string(),
                            filament_preset: "Generic PLA".to_string(),
                            controls: SliceControls::default(),
                        },
                        facts: Farm3dFacts::new(profile, 0.4, MaterialFamily::Pla, None, 1.75),
                        estimates: SliceEstimates {
                            print_seconds: Some(3600),
                            filament_grams: Some(10.0),
                            ..SliceEstimates::none()
                        },
                        runtime: SliceRuntimeInfo {
                            engine_version: "2.4.2".to_string(),
                            engine_channel: RuntimeChannel::Release,
                            preset_source_version: "2.4.2".to_string(),
                            preset_source_channel: RuntimeChannel::Release,
                        },
                        blobs: Vec::new(),
                    },
                )?;
                // The repository stamps `created_at` with the clock; pin it
                // (Slice Revisions are immutable, so re-insert the row).
                exec(
                    tx,
                    &format!(
                        "CREATE TEMP TABLE farm_slr AS
                           SELECT * FROM slice_revisions WHERE id = '{SLICE_REVISION_FARM3D}';
                         DELETE FROM slice_revisions WHERE id = '{SLICE_REVISION_FARM3D}';
                         UPDATE farm_slr SET created_at = '{NOW}';
                         INSERT INTO slice_revisions SELECT * FROM farm_slr;
                         DROP TABLE farm_slr;"
                    ),
                );
                // Queue Entries and Jobs: an open entry, a failed Job whose
                // closed entry has a retry successor, and a Job cancelled
                // before it started.
                exec(
                    tx,
                    &format!(
                        "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id,
                           copy_index, origin_entry_id, origin_kind, state, close_reason,
                           position, policy, preference, estimate_mg, estimate_source,
                           manual_printer_id, created_at, updated_at, closed_at)
                           VALUES ('{QUEUE_OPEN}', 1, '{SLICE_REVISION}', 'qln-open', 1, NULL,
                                   NULL, 'queued', NULL, 2, 'manual', 'loadedFirst', 500000,
                                   'operatorEntered', '{PRINTER_B}', '{NOW}', '{NOW}', NULL),
                                  ('{QUEUE_FAILED}', 2, '{SLICE_REVISION}', 'qln-failed', 1, NULL,
                                   NULL, 'closed', 'failed', NULL, 'recommended', 'loadedFirst',
                                   500000, 'operatorEntered', NULL, '{NOW}', '{ended}',
                                   '{ended}'),
                                  ('{QUEUE_RETRY}', 1, '{SLICE_REVISION}', 'qln-failed', 1,
                                   '{QUEUE_FAILED}', 'retry', 'queued', NULL, 3, 'recommended',
                                   'loadedFirst', 500000, 'operatorEntered', NULL, '{ended}',
                                   '{ended}', NULL),
                                  ('{QUEUE_CANCELLED}', 2, '{SLICE_REVISION}', 'qln-cancelled', 1,
                                   NULL, NULL, 'closed', 'cancelled', NULL, 'recommended',
                                   'loadedFirst', 500000, 'operatorEntered', NULL, '{NOW}',
                                   '{ended}', '{ended}');
                         INSERT INTO spool_reservations(id, spool_id, holder_kind, holder_id,
                           amount_mg, state, operation_id, created_at, settled_at)
                           VALUES ('{RESERVATION_FAILED}', '{SPOOL_A}', 'job', '{JOB_FAILED}',
                                   500000, 'unresolved', 'op-rsv-failed', '{NOW}', NULL),
                                  ('{RESERVATION_CANCELLED}', '{SPOOL_A}', 'job',
                                   '{JOB_CANCELLED}', 500000, 'released', 'op-rsv-cancelled',
                                   '{NOW}', '{ended}');
                         INSERT INTO jobs(id, revision, queue_entry_id, slice_revision_id,
                           printer_id, printer_snapshot_json, spool_id, reservation_id,
                           estimate_mg, state, cancel_reason, settlement, assigned_by,
                           created_at, updated_at, ended_at)
                           VALUES ('{JOB_FAILED}', 3, '{QUEUE_FAILED}', '{SLICE_REVISION}',
                                   '{PRINTER_A}', '{{}}', '{SPOOL_A}', '{RESERVATION_FAILED}',
                                   500000, 'failed', NULL, 'pending', 'operator', '{NOW}',
                                   '{ended}', '{ended}'),
                                  ('{JOB_CANCELLED}', 2, '{QUEUE_CANCELLED}', '{SLICE_REVISION}',
                                   '{PRINTER_A}', '{{}}', '{SPOOL_A}', '{RESERVATION_CANCELLED}',
                                   500000, 'cancelled', 'cancelledBeforeStart', 'notRequired',
                                   'operator', '{NOW}', '{ended}', '{ended}');
                         UPDATE queue_entries SET job_id = '{JOB_FAILED}'
                           WHERE id = '{QUEUE_FAILED}';
                         UPDATE queue_entries SET job_id = '{JOB_CANCELLED}'
                           WHERE id = '{QUEUE_CANCELLED}';
                         INSERT INTO reconciliation_requirements(id, job_id, kind, status,
                           spool_id, reservation_id, opened_at, resolved_at, resolution_json)
                           VALUES ('{REQUIREMENT_PENDING}', '{JOB_FAILED}',
                                   'materialReconciliation', 'pending', '{SPOOL_A}',
                                   '{RESERVATION_FAILED}', '{ended}', NULL, NULL),
                                  ('{REQUIREMENT_RESOLVED}', '{JOB}', 'materialReconciliation',
                                   'resolved', '{SPOOL_A}', '{RESERVATION_A}', '{ended}',
                                   '{ended}',
                                   '{{\"kind\":\"settled\",\"method\":\"estimated\",\"usedMg\":100000}}');
                         INSERT INTO job_events(id, job_id, sequence, kind, from_state, to_state,
                           operation_id, detail_json, at)
                           VALUES ('{JOB_EVENT_CORRECTED}', '{JOB}', 1, 'materialCorrected',
                                   'completed', 'completed', 'op-correct-a',
                                   '{{\"correctionEventId\":\"{CORRECTION_EVENT}\"}}', '{ended}');
                         INSERT INTO incident_events(id, incident_id, sequence, kind,
                           detail_json, operation_id, at)
                           VALUES ('{INCIDENT_NOTE}', '{INCIDENT}', 2, 'noteAdded',
                                   '{{\"kind\":\"noteAdded\",\"text\":\"Checked the nozzle.\"}}',
                                   'op-note-a', '{ended}');"
                    ),
                );
                // Attention: a resolved Event and its open recurrence, and
                // the pending requirement's Event.
                exec(
                    tx,
                    &format!(
                        "INSERT INTO attention_events(id, dedup_key, condition, severity,
                           requires_action, resolution_mode, notification_class, source_kind,
                           source_id, printer_id, job_id, spool_id, requirement_id,
                           subject_snapshot_json, detail_json, summary, origin,
                           first_observed_at, last_observed_at, recurrence_of, read_at,
                           resolved_at, resolution)
                           VALUES ('{ATTENTION_RESOLVED}',
                                   'printer.connectionError:printer:{PRINTER_B}',
                                   'printer.connectionError', 'warning', 1, 'auto',
                                   'connectivity', 'printer', '{PRINTER_B}', '{PRINTER_B}', NULL,
                                   NULL, NULL, '{{}}',
                                   '{{\"kind\":\"printerConnectionError\",\"cause\":\"auth\"}}',
                                   'Connection error.', 'live', '{NOW}', '{NOW}', NULL, '{NOW}',
                                   '{NOW}', 'conditionCleared'),
                                  ('{ATTENTION_RECURRENCE}',
                                   'printer.connectionError:printer:{PRINTER_B}',
                                   'printer.connectionError', 'warning', 1, 'auto',
                                   'connectivity', 'printer', '{PRINTER_B}', '{PRINTER_B}', NULL,
                                   NULL, NULL, '{{}}',
                                   '{{\"kind\":\"printerConnectionError\",\"cause\":\"auth\"}}',
                                   'Connection error.', 'live', '{ended}', '{ended}',
                                   '{ATTENTION_RESOLVED}', NULL, NULL, NULL),
                                  ('{ATTENTION_REQUIREMENT}',
                                   'requirement.materialReconciliation:reconciliationRequirement:{REQUIREMENT_PENDING}',
                                   'requirement.materialReconciliation', 'warning', 1, 'action',
                                   'reconciliation', 'reconciliationRequirement',
                                   '{REQUIREMENT_PENDING}', '{PRINTER_A}', '{JOB_FAILED}',
                                   '{SPOOL_A}', '{REQUIREMENT_PENDING}', '{{}}',
                                   '{{\"kind\":\"requirementMaterialReconciliation\",\"requirementStatus\":\"pending\",\"spoolId\":\"{SPOOL_A}\"}}',
                                   'Reconcile material.', 'live', '{ended}', '{ended}', NULL, NULL,
                                   NULL, NULL);"
                    ),
                );
                // Terminal Host Operations: Job A's succeeded upload, and a
                // failed one on Printer B.
                exec(
                    tx,
                    &format!(
                        "INSERT INTO host_operations(id, operation_id, printer_id, kind,
                           slice_revision_id, gcode_sha256, gcode_size, host_path, endpoint_json,
                           state, failure_json, resolution_json, created_at, dispatched_at,
                           resolved_at, job_id)
                           VALUES ('{HOST_OPERATION_SUCCEEDED}', 'hop-done-op', '{PRINTER_A}',
                                   'upload', '{SLICE_REVISION}', '{GCODE_HASH}', 200,
                                   'part.gcode', '{{}}', 'succeeded', NULL,
                                   '{{\"kind\":\"artifactVerified\",\"reconciled\":false}}',
                                   '{NOW}', '{NOW}', '{NOW}', '{JOB}'),
                                  ('{HOST_OPERATION_FAILED}', 'hop-failed-op', '{PRINTER_B}',
                                   'upload', '{SLICE_REVISION_TARGETED}', '{GCODE_HASH}', 200,
                                   'part.gcode', '{{}}', 'failed',
                                   '{{\"code\":\"hostUnreachable\",\"message\":\"farm3d couldn''t connect to the printer. Nothing was sent.\"}}',
                                   NULL, '{NOW}', '{NOW}', '{NOW}', NULL);"
                    ),
                );
                // Every Job's and Incident's Printer snapshot is decodable.
                tx.execute(
                    "UPDATE jobs SET printer_snapshot_json = ?1",
                    [printer_snapshot_json("Printer A")],
                )?;
                tx.execute(
                    "UPDATE incidents SET printer_snapshot_json = ?1",
                    [printer_snapshot_json("Printer A")],
                )?;
                Ok(())
            })
            .expect("seed extension");
        self.write_blob(&three_mf_bytes("linked"));
        self.write_blob(&linked_thumbnail_bytes());
    }
}

pub fn table_counts(connection: &Connection) -> BTreeMap<String, i64> {
    let names: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    names
        .into_iter()
        .map(|name| {
            let rows = connection
                .query_row(&format!("SELECT count(*) FROM \"{name}\""), [], |row| {
                    row.get(0)
                })
                .unwrap();
            (name, rows)
        })
        .collect()
}

/// `true` when `path`'s bytes contain `needle`.
pub fn file_contains(path: &Path, needle: &str) -> bool {
    let bytes = fs::read(path).unwrap();
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}
