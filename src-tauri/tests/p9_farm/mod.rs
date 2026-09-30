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

use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};
use farm3d_lib::settings::repository::SettingsRepository;

use crate::secrets;
use farm_seed::{exec, seed_every_domain, seed_printer, GCODE_BYTES, GCODE_HASH, NOW};

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

    /// Every `content_blobs` hash, sorted.
    pub fn content_hashes(&self) -> Vec<String> {
        let mut hashes = vec![GCODE_HASH.to_string()];
        let (source, thumbnail, plate) = self.blob_hashes();
        hashes.extend([source, thumbnail, plate]);
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
                               '{{}}');
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
