//! P9 Task 6 (spec D3, D4, D6, D7, D8 "Staging", D10): restore staging,
//! verification, the preview, and refusal.
//!
//! - Staging: a backup is extracted into the three staging roots with every
//!   entry verified; an older schema is migrated on the staged copy only;
//!   every D3 and D4 rejection fails and leaves nothing staged; a staging
//!   expires after 24 hours; startup removes every staging except the one a
//!   `pending` or `installing` journal names.
//! - Preview: the spec's "Conflict fixtures" table (one test per row), the
//!   per-table counts, the notices, and the D7 blockers. The preview is
//!   read-only for the live Farm.
//! - Commands: `preview_restore` (file and safety-backup sources, a closed
//!   dialog, the lease) and `discard_restore_preview`.
//!
//! The "Conflict fixtures" table, copied verbatim from the spec. Each
//! fixture starts from a baseline where local and backup are equal (one row
//! per domain, same ids, same content) and changes only what the row says.
//! "L" is a local row, "B" a backup row. Expected items are exactly those
//! listed; every other row is unchanged.
//!
//! | # | Domain | Local | Backup | Content equal | Natural key situation | Expected |
//! |---|---|---|---|---|---|---|
//! | c1 | settings | singleton | singleton | yes | — | none |
//! | c2 | settings | theme `farm3d-dark` | theme `system` | no | — | `changed` settings |
//! | c3 | printer | L1 | L1 | yes | active, same identity | none |
//! | c4 | printer | L1 named "Left" | L1 named "Left Carbon" | no | — | `changed` L1 |
//! | c5 | printer | L1 revision 3 | L1 revision 2, otherwise equal | no | — | `changed` L1 |
//! | c6 | printer | L2 (not in backup), active, identity `192.0.2.20:7125` | B2 (not local), active, same identity | — | clash | `uniqueClash` L2/B2 |
//! | c7 | printer | L2 active, identity H | B2 archived, identity H | — | backup side archived | `onlyLocal` L2 |
//! | c8 | printer | L2 archived, identity H | B2 active, identity H | — | local side archived | `onlyLocal` L2 |
//! | c9 | printer | L2 with no Connection | B2 with no Connection | — | no identity | `onlyLocal` L2 |
//! | c10 | printer | — | B3 (not local) | — | no local match | none (added) |
//! | c11 | printer | L2 identity H; L1 identity K | B has L1 with identity H | no (L1) | clash through a shared id | `changed` L1, `uniqueClash` L2/L1 |
//! | c12 | printer | L1 active identity H | L1 archived identity H | no | same id | `changed` L1 only |
//! | c13 | spool | S2 #12 (not in backup) | T2 #12 (not local) | — | clash | `uniqueClash` S2/T2 |
//! | c14 | spool | S2 #12 | — | — | no backup #12 | `onlyLocal` S2 |
//! | c15 | spool | S1 `current_mg` 800000 | S1 `current_mg` 750000 | no | — | `changed` S1 |
//! | c16 | spool | S1 #12; S2 #14 (not in backup) | S1 #14 | no (S1) | clash through a shared id | `changed` S1, `uniqueClash` S2/S1 |
//! | c17 | tare | R2 "Cardboard 1kg" | R3 "cardboard 1KG" | — | ASCII case fold | `uniqueClash` R2/R3 |
//! | c18 | tare | R2 "Ölspule" | R3 "ölspule" | — | differs outside ASCII | `onlyLocal` R2 |
//! | c19 | project | P2 "Brackets" | P3 "BRACKETS" | — | clash | `uniqueClash` P2/P3 |
//! | c20 | project | P1 | P1 | yes | — | none |
//! | c21 | project | — | P3 (not local) | — | — | none (added) |
//! | c22 | model | M2 (not in backup) | — | — | no natural key | `onlyLocal` M2 |
//! | c23 | model | M1 named "Hook v2" | M1 named "Hook" | no | — | `changed` M1 |
//! | c24 | sliceRevision | SR2 (not in backup) | — | — | — | `onlyLocal` SR2 |
//! | c25 | queueEntry | Q1 `closed` | Q1 `queued` | no | — | `changed` Q1 |
//! | c26 | job | J2 (finished after the backup) | — | — | — | `onlyLocal` J2 |
//! | c27 | job | J1 | J1 | yes | — | none |
//! | c28 | incident | I1 closed | I1 open | no | — | `changed` I1 |
//! | c29 | attentionEvent | A1 with `read_at` set | A1 unread | no | — | `changed` A1 |
//! | c30 | snapshot | N2 (not in backup) | — | — | — | `onlyLocal` N2 |
//! | c31 | snapshot | N1 pruned `age` | N1 unpruned | no | — | `changed` N1 |
//! | c32 | (child) | Printer L1's slot renamed | original slot name | root equal | — | none; `material_slots` counts equal |
//! | c33 | (excluded) | a `printer_status_snapshots` row | none (sanitized) | — | — | none; local 1, backup 0 in counts |
//! | c34 | (kept local) | Slicer runtime paths set | paths NULL (sanitized) | — | — | none; `slicerRuntimeKeptLocal` notice |
//! | c35 | job | 250 local-only finished Jobs | — | — | — | one `onlyLocal`/`job` group, `total` 250, 200 items, ordered by id |
//! | c36 | printer | L2 identity H, L3 identity K (neither in backup) | B2 identity H, B3 identity K | — | two clashes | two `uniqueClash` items, ordered L2, L3 |
//!
//! The fixtures use the every-domain Farm's rows as the baseline: L1 is
//! `prn-a`, S1 `spl-a`, P1 `prj-a`, M1 `mdl-a`, Q1 `qen-a`, J1 `job-a`, I1
//! `inc-a`, A1 `att-off`, and N1 `snp-b`.

mod common;
mod p9_farm;
#[path = "common/secrets.rs"]
mod secrets;

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::test::MockRuntime;

use farm3d_lib::backup::archive::{self, ArchiveWriter, EntryCompression};
use farm3d_lib::backup::dialogs::PortabilityDialogs;
use farm3d_lib::backup::lease::{BackupLease, LeaseActivity};
use farm3d_lib::backup::manifest::{Manifest, ManifestMigration};
use farm3d_lib::backup::preview;
use farm3d_lib::backup::safety;
use farm3d_lib::backup::staging::{self, RestoreError, StagedCandidate, StagingOptions, Stagings};
use farm3d_lib::backup::writer::{write_backup, BackupRequest, WriterHooks};
use farm3d_lib::backup::{
    BackupMediaChoice, BackupOrigin, RestoreBlocker, RestoreBlockerKind, RestoreConflictClass,
    RestoreDomain, RestoreNotice, RestorePreview, RestorePreviewSource,
};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::persistence::test_support::apply_through;
use farm3d_lib::persistence::{
    MetadataRootLease, RepositoryError, Storage, StoragePaths, CURRENT_SCHEMA_VERSION,
};
use farm3d_lib::RuntimeServices;

use p9_farm::{ids, Farm};

const CREATED_AT: &str = "2026-09-29T12:00:00.000Z";
const NOW: &str = ids::NOW;
/// H and K of the fixture table.
const IDENTITY_H: &str = "192.0.2.20:7125";
const IDENTITY_K: &str = "192.0.2.21:7125";

// --- helpers ------------------------------------------------------------------------

fn created_at() -> DateTime<Utc> {
    CREATED_AT.parse().unwrap()
}

/// Runs `statements` in one write transaction on `farm`.
fn sql(farm: &Farm, statements: &str) {
    if statements.trim().is_empty() {
        return;
    }
    farm.storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            tx.execute_batch(statements)
                .unwrap_or_else(|error| panic!("fixture SQL failed: {error}\n{statements}"));
            Ok(())
        })
        .expect("fixture write");
}

/// The every-domain Farm with its one clock-stamped row pinned, so two
/// Farms built alike are equal row for row.
fn farm() -> Farm {
    let farm = Farm::with_every_domain();
    sql(&farm, &format!("UPDATE settings SET updated_at = '{NOW}';"));
    farm
}

/// A backup of `farm` with `media`, written beside it.
fn write_archive(farm: &Farm, media: BackupMediaChoice) -> PathBuf {
    let lease = BackupLease::new();
    let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
    let directory = farm.temp.path().join("exports");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("backup-{}.farm3d-backup", uuid::Uuid::new_v4()));
    write_backup(
        &farm.storage,
        &guard,
        &path,
        &BackupRequest {
            media,
            origin: BackupOrigin::Operator,
            created_at: Some(created_at()),
            app_version: "0.1.0".to_string(),
        },
        &WriterHooks::default(),
    )
    .expect("backup");
    path
}

fn stage_with(
    paths: &StoragePaths,
    archive: &Path,
    options: &StagingOptions,
) -> Result<StagedCandidate, RestoreError> {
    let lease = BackupLease::new();
    let guard = lease.try_acquire(LeaseActivity::RestorePreview).unwrap();
    staging::stage(paths, &guard, archive, options)
}

fn stage(local: &Farm, archive: &Path) -> StagedCandidate {
    stage_with(local.paths(), archive, &StagingOptions::default()).expect("staged")
}

fn file_source() -> RestorePreviewSource {
    RestorePreviewSource::File {
        file_name: "backup.farm3d-backup".to_string(),
    }
}

fn preview_of(local: &Farm, candidate: &StagedCandidate) -> RestorePreview {
    let store = CredentialStore::file_backed(local.credentials_dir.clone());
    preview::compute(&local.storage, candidate, file_source(), &store).expect("preview")
}

/// Builds a local and a backup Farm from the same baseline, changes each
/// with its SQL, and previews restoring the backup over the local Farm.
fn fixture(local_sql: &str, backup_sql: &str) -> RestorePreview {
    fixture_with(local_sql, backup_sql, BackupMediaChoice::All, |_, _| {})
}

fn fixture_with(
    local_sql: &str,
    backup_sql: &str,
    media: BackupMediaChoice,
    before_preview: impl FnOnce(&Farm, &Farm),
) -> RestorePreview {
    let local = farm();
    let remote = farm();
    sql(&local, local_sql);
    sql(&remote, backup_sql);
    let archive = write_archive(&remote, media);
    let candidate = stage(&local, &archive);
    before_preview(&local, &remote);
    preview_of(&local, &candidate)
}

type Item = (Option<String>, Option<String>, String);
type Group = (RestoreConflictClass, RestoreDomain, i64, Vec<Item>);

fn conflicts(preview: &RestorePreview) -> Vec<Group> {
    preview
        .conflicts
        .iter()
        .map(|group| {
            (
                group.class,
                group.domain,
                group.total,
                group
                    .items
                    .iter()
                    .map(|item| {
                        (
                            item.local_id.clone(),
                            item.backup_id.clone(),
                            item.label.clone(),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

fn only_local(domain: RestoreDomain, id: &str, label: &str) -> Group {
    (
        RestoreConflictClass::OnlyLocal,
        domain,
        1,
        vec![(Some(id.to_string()), None, label.to_string())],
    )
}

fn changed(domain: RestoreDomain, id: &str, label: &str) -> Group {
    (
        RestoreConflictClass::Changed,
        domain,
        1,
        vec![(
            Some(id.to_string()),
            Some(id.to_string()),
            label.to_string(),
        )],
    )
}

fn unique_clash(domain: RestoreDomain, local: &str, backup: &str, label: &str) -> Group {
    (
        RestoreConflictClass::UniqueClash,
        domain,
        1,
        vec![(
            Some(local.to_string()),
            Some(backup.to_string()),
            label.to_string(),
        )],
    )
}

fn count(preview: &RestorePreview, table: &str) -> (i64, i64) {
    let row = preview
        .counts
        .iter()
        .find(|count| count.table == table)
        .unwrap_or_else(|| panic!("no count for {table}"));
    (row.local, row.backup)
}

/// A Printer row. `identity` sets `host_identity`; `archived` sets
/// `archived_at`.
fn printer(id: &str, name: &str, identity: Option<&str>, archived: bool) -> String {
    let identity = identity.map_or("NULL".to_string(), |identity| format!("'{identity}'"));
    let archived = if archived {
        format!("'{NOW}'")
    } else {
        "NULL".to_string()
    };
    format!(
        "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model, catalog_variant,
           catalog_model_id, catalog_printer_variant, notes, overrides_json, created_at,
           updated_at, host_identity, archived_at)
         VALUES ('{id}', 1, '{name}', '', '', '', '', '', '', '{{}}', '{NOW}', '{NOW}',
                 {identity}, {archived});"
    )
}

fn spool(id: &str, number: i64) -> String {
    format!(
        "INSERT INTO spools(id, revision, spool_number, manufacturer, material_family,
           color_name, diameter, nominal_mg, current_mg, confidence, lifecycle,
           created_at, updated_at)
         VALUES ('{id}', 1, {number}, 'Acme', 'PLA', 'Black', '1.75', 1000000, 1000000,
                 'measured', 'active', '{NOW}', '{NOW}');"
    )
}

fn tare(id: &str, name: &str) -> String {
    format!(
        "INSERT INTO spool_tares(id, revision, name, weight_mg, created_at, updated_at)
         VALUES ('{id}', 1, '{name}', 200000, '{NOW}', '{NOW}');"
    )
}

fn project(id: &str, name: &str) -> String {
    format!(
        "INSERT INTO library_projects(id, revision, name, created_at, updated_at)
         VALUES ('{id}', 1, '{name}', '{NOW}', '{NOW}');"
    )
}

/// A finished Job with its own closed Queue Entry and reservation.
fn finished_job(suffix: &str) -> String {
    format!(
        "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
           state, close_reason, position, policy, preference, estimate_mg, estimate_source,
           created_at, updated_at, closed_at)
         VALUES ('qen-{suffix}', 1, 'slr-a', 'qln-{suffix}', 1, 'closed', 'completed', NULL,
                 'manual', 'loadedFirst', 500000, 'operatorEntered', '{NOW}', '{NOW}', '{NOW}');
         INSERT INTO spool_reservations(id, spool_id, holder_kind, holder_id, amount_mg, state,
           operation_id, created_at)
         VALUES ('rsv-{suffix}', 'spl-a', 'job', 'job-{suffix}', 500000, 'active',
                 'rsv-{suffix}-op', '{NOW}');
         INSERT INTO jobs(id, revision, queue_entry_id, slice_revision_id, printer_id,
           printer_snapshot_json, spool_id, reservation_id, estimate_mg, state, settlement,
           settlement_method, assigned_by, created_at, updated_at, ended_at)
         VALUES ('job-{suffix}', 1, 'qen-{suffix}', 'slr-a', 'prn-a', '{{}}', 'spl-a',
                 'rsv-{suffix}', 500000, 'completed', 'settled', 'estimated', 'operator',
                 '{NOW}', '{NOW}', '{NOW}');"
    )
}

/// An assigned (active) Job on `printer_id`, with its Queue Entry and
/// reservation.
fn active_job(suffix: &str, printer_id: &str) -> String {
    format!(
        "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
           state, position, policy, preference, estimate_mg, estimate_source,
           created_at, updated_at)
         VALUES ('qen-{suffix}', 1, 'slr-a', 'qln-{suffix}', 1, 'queued', 90, 'manual',
                 'loadedFirst', 500000, 'operatorEntered', '{NOW}', '{NOW}');
         INSERT INTO spool_reservations(id, spool_id, holder_kind, holder_id, amount_mg, state,
           operation_id, created_at)
         VALUES ('rsv-{suffix}', 'spl-a', 'job', 'job-{suffix}', 500000, 'active',
                 'rsv-{suffix}-op', '{NOW}');
         INSERT INTO jobs(id, revision, queue_entry_id, slice_revision_id, printer_id,
           printer_snapshot_json, spool_id, reservation_id, estimate_mg, state, settlement,
           assigned_by, created_at, updated_at)
         VALUES ('job-{suffix}', 1, 'qen-{suffix}', 'slr-a', '{printer_id}', '{{}}', 'spl-a',
                 'rsv-{suffix}', 500000, 'assigned', 'open', 'operator', '{NOW}', '{NOW}');
         UPDATE queue_entries SET state = 'assigned', job_id = 'job-{suffix}'
          WHERE id = 'qen-{suffix}';"
    )
}

/// The staging directories under the three roots.
fn staged_directories(paths: &StoragePaths) -> Vec<String> {
    let mut found = Vec::new();
    let list = |directory: PathBuf, keep: &dyn Fn(&str) -> bool, found: &mut Vec<String>| {
        if let Ok(entries) = fs::read_dir(&directory) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if keep(&name) {
                    found.push(entry.path().to_string_lossy().into_owned());
                }
            }
        }
    };
    list(
        paths.snapshot_root().join(".restore-staging"),
        &|_| true,
        &mut found,
    );
    list(
        paths.content_root().join("staging"),
        &|name| name.starts_with("restore-"),
        &mut found,
    );
    list(
        paths.media_root().to_path_buf(),
        &|name| name.starts_with("restore-"),
        &mut found,
    );
    found.sort();
    found
}

fn sha256_file(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn is_staging_id(id: &str) -> bool {
    id.strip_prefix("stg-")
        .and_then(|rest| uuid::Uuid::parse_str(rest).ok())
        .is_some_and(|uuid| uuid.get_version_num() == 4)
}

fn error_json(error: RestoreError) -> Value {
    serde_json::to_value(CommandError::from(error)).unwrap()
}

// --- Step 1: staging ------------------------------------------------------------------

#[test]
fn a_backup_is_staged_under_the_three_staging_roots_with_verified_checksums() {
    let remote = farm();
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let local = farm();
    let candidate = stage(&local, &archive);
    let paths = local.paths();
    let id = candidate.staging_id.clone();
    assert!(is_staging_id(&id), "{id}");

    // The database and a copy of the manifest.
    let database_dir = paths.snapshot_root().join(".restore-staging").join(&id);
    assert_eq!(candidate.layout.database_dir, database_dir);
    let candidate_path = database_dir.join("candidate.sqlite3");
    assert!(candidate_path.is_file());
    let manifest = Manifest::parse(&fs::read(database_dir.join("manifest.json")).unwrap())
        .expect("staged manifest parses");
    assert_eq!(manifest, archive::verify(&archive).unwrap());
    assert_eq!(candidate.manifest, manifest);
    // Not migrated (already current), so the staged bytes are the archive's.
    assert_eq!(candidate.migrated_from, None);
    assert_eq!(
        sha256_file(&candidate_path),
        manifest.entries[0].sha256,
        "an unmigrated candidate is byte-for-byte the archived database"
    );
    for sidecar in ["-wal", "-shm", "-journal"] {
        assert!(
            !database_dir
                .join(format!("candidate.sqlite3{sidecar}"))
                .exists(),
            "{sidecar} left beside the candidate"
        );
    }

    // Content under `<content_root>/staging/restore-<id>/<hh>/<hex>`.
    let content_dir = paths
        .content_root()
        .join("staging")
        .join(format!("restore-{id}"));
    assert_eq!(candidate.layout.content_dir, content_dir);
    let mut content = 0;
    let mut media = 0;
    for entry in &manifest.entries {
        if let Some(rest) = entry.path.strip_prefix("content/sha256/") {
            let staged = content_dir.join(rest);
            assert_eq!(sha256_file(&staged), entry.sha256, "{}", entry.path);
            assert!(rest.ends_with(&entry.sha256));
            content += 1;
        } else if let Some(rel_path) = entry.path.strip_prefix("media/") {
            let staged = paths
                .media_root()
                .join(format!("restore-{id}"))
                .join(rel_path);
            assert_eq!(sha256_file(&staged), entry.sha256, "{}", entry.path);
            media += 1;
        }
    }
    assert_eq!(content, remote.content_hashes().len());
    assert_eq!(media, 3);
    assert_eq!(
        candidate.layout.media_dir,
        paths.media_root().join(format!("restore-{id}"))
    );

    // No `.part` file is left anywhere.
    let mut stack = vec![
        database_dir.clone(),
        content_dir.clone(),
        candidate.layout.media_dir.clone(),
    ];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                assert!(
                    !path.to_string_lossy().ends_with(".part"),
                    "{} left",
                    path.display()
                );
            }
        }
    }

    // The staging expires 24 hours after it was created.
    assert_eq!(
        candidate.expires_at(),
        candidate.created_at + Duration::hours(24)
    );
}

#[test]
fn media_none_still_stages_an_empty_snapshots_directory() {
    let remote = farm();
    let archive = write_archive(&remote, BackupMediaChoice::None);
    let local = farm();
    let candidate = stage(&local, &archive);
    let snapshots = candidate.layout.media_dir.join("snapshots");
    assert!(snapshots.is_dir());
    assert_eq!(fs::read_dir(&snapshots).unwrap().count(), 0);
}

/// Hashes of the live database set, to prove a preview never writes it.
fn live_database_bytes(paths: &StoragePaths) -> BTreeMap<String, Option<String>> {
    ["", "-wal"]
        .iter()
        .map(|suffix| {
            let path = PathBuf::from(format!("{}{suffix}", paths.database().display()));
            (
                suffix.to_string(),
                path.exists().then(|| sha256_file(&path)),
            )
        })
        .collect()
}

/// A `schemaVersion` 9 backup: a v9 database with its manifest.
fn v9_archive(directory: &Path) -> PathBuf {
    let database = directory.join("v9.sqlite3");
    let mut connection = Connection::open(&database).unwrap();
    apply_through(&mut connection, 9).unwrap();
    connection
        .execute(
            "INSERT INTO settings(singleton_id, revision, theme_mode, updated_at)
             VALUES (1, 4, 'farm3d-dark', ?1)",
            [NOW],
        )
        .unwrap();
    let migrations: Vec<ManifestMigration> = connection
        .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |row| {
            Ok(ManifestMigration {
                version: row.get(0)?,
                name: row.get(1)?,
                checksum: row.get(2)?,
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let counts = p9_farm::table_counts(&connection);
    connection.close().unwrap();
    let bytes = fs::read(&database).unwrap();

    // Borrow a real manifest's shape and replace what differs.
    let remote = farm();
    let template = archive::verify(&write_archive(&remote, BackupMediaChoice::None)).unwrap();
    let mut manifest = template;
    manifest.schema_version = 9;
    manifest.migrations = migrations;
    manifest.counts = counts;
    manifest.credential_ref_count = 0;
    manifest.media = Default::default();
    let entries = vec![(archive::DATABASE_PATH.to_string(), bytes)];
    write_rebuilt(directory, &mut manifest, &entries, |_| {})
}

#[test]
fn an_older_schema_candidate_is_migrated_on_the_staged_copy_and_the_live_database_is_untouched() {
    let local = farm();
    let directory = local.temp.path().join("v9");
    fs::create_dir_all(&directory).unwrap();
    let archive = v9_archive(&directory);
    let before = live_database_bytes(local.paths());

    // The compatibility window starts at 10, so this binary refuses it...
    let refused = stage_with(local.paths(), &archive, &StagingOptions::default())
        .expect_err("schema 9 is outside the window");
    assert_eq!(
        error_json(refused)["details"],
        json!({ "reason": "manifestInvalid", "fieldPath": "manifest.schemaVersion" })
    );
    assert!(staged_directories(local.paths()).is_empty());

    // ...and a window opened to 9 (the seam a later schema's binary would
    // have) migrates it on the staged copy.
    let options = StagingOptions {
        min_schema_version: 9,
        ..StagingOptions::default()
    };
    let candidate = stage_with(local.paths(), &archive, &options).expect("staged");
    assert_eq!(candidate.migrated_from, Some(9));
    let staged = Connection::open(candidate.layout.candidate_path()).unwrap();
    let version: i64 = staged
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    let theme: String = staged
        .query_row("SELECT theme_mode FROM settings", [], |row| row.get(0))
        .unwrap();
    assert_eq!(theme, "farm3d-dark", "rows survive the migration");
    drop(staged);

    let preview = preview_of(&local, &candidate);
    assert!(preview.notices.contains(&RestoreNotice::Migrated {
        from_schema_version: 9
    }));
    assert_eq!(preview.backup.schema_version, 9);
    assert_eq!(live_database_bytes(local.paths()), before);
}

// --- D3 and D4 rejections ------------------------------------------------------------

/// Every entry of `path`, in manifest order.
fn read_entries(path: &Path) -> (Manifest, Vec<(String, Vec<u8>)>) {
    let manifest = archive::verify(path).unwrap();
    let mut zip = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let entries = manifest
        .entries
        .iter()
        .map(|entry| {
            let mut bytes = Vec::new();
            zip.by_name(&entry.path)
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            (entry.path.clone(), bytes)
        })
        .collect();
    (manifest, entries)
}

/// Writes `entries` with `manifest` (its `entries` recomputed from the
/// bytes, then `edit` applied).
fn write_rebuilt(
    directory: &Path,
    manifest: &mut Manifest,
    entries: &[(String, Vec<u8>)],
    edit: impl FnOnce(&mut Manifest),
) -> PathBuf {
    manifest.entries = entries
        .iter()
        .map(
            |(path, bytes)| farm3d_lib::backup::manifest::ManifestEntry {
                path: path.clone(),
                bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
            },
        )
        .collect();
    edit(manifest);
    let path = directory.join(format!("rebuilt-{}.farm3d-backup", uuid::Uuid::new_v4()));
    let mut writer = ArchiveWriter::new(fs::File::create(&path).unwrap(), created_at()).unwrap();
    writer.write_manifest(manifest).unwrap();
    for (name, bytes) in entries {
        writer
            .write_entry(
                name,
                &bytes[..],
                EntryCompression::Deflated,
                bytes.len() as u64,
            )
            .unwrap();
    }
    writer.finish().unwrap();
    path
}

/// `original` rebuilt with its database changed by `tamper` (the manifest
/// hashes follow the new bytes, so only D4's checks can catch it).
fn with_database(
    directory: &Path,
    original: &Path,
    tamper: impl FnOnce(&Connection),
    edit: impl FnOnce(&mut Manifest),
) -> PathBuf {
    let (mut manifest, mut entries) = read_entries(original);
    let file = directory.join(format!("tamper-{}.sqlite3", uuid::Uuid::new_v4()));
    fs::write(&file, &entries[0].1).unwrap();
    let connection = Connection::open(&file).unwrap();
    tamper(&connection);
    connection.close().unwrap();
    entries[0].1 = fs::read(&file).unwrap();
    write_rebuilt(directory, &mut manifest, &entries, edit)
}

fn rebuilt(
    directory: &Path,
    original: &Path,
    entries_edit: impl FnOnce(&mut Vec<(String, Vec<u8>)>),
    edit: impl FnOnce(&mut Manifest),
) -> PathBuf {
    let (mut manifest, mut entries) = read_entries(original);
    entries_edit(&mut entries);
    write_rebuilt(directory, &mut manifest, &entries, edit)
}

fn invalid(reason: &str, field_path: &str) -> Value {
    json!({
        "code": "BACKUP_INVALID",
        "message": "This file is damaged or isn't a farm3d backup.",
        "recovery": [],
        "retryable": false,
        "details": { "reason": reason, "fieldPath": field_path },
    })
}

/// The error without its contract version, for comparing.
fn shape(error: Value) -> Value {
    let mut error = error;
    error.as_object_mut().unwrap().remove("contractVersion");
    error
}

#[test]
fn every_d3_and_d4_rejection_fails_and_leaves_nothing_staged() {
    let remote = farm();
    let original = write_archive(&remote, BackupMediaChoice::All);
    let local = farm();
    let directory = local.temp.path().join("tampered");
    fs::create_dir_all(&directory).unwrap();
    let before = live_database_bytes(local.paths());
    let content_index = read_entries(&original)
        .1
        .iter()
        .position(|(path, _)| path.starts_with("content/"))
        .unwrap();

    let truncated = directory.join("truncated.farm3d-backup");
    let bytes = fs::read(&original).unwrap();
    fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();

    let cases: Vec<(&str, PathBuf, StagingOptions, Value)> = vec![
        (
            "a truncated file",
            truncated,
            StagingOptions::default(),
            invalid("notAZip", "archive"),
        ),
        (
            "an archive entry the manifest doesn't list",
            rebuilt(
                &directory,
                &original,
                |_| {},
                |manifest| {
                    manifest.entries.pop();
                },
            ),
            StagingOptions::default(),
            invalid(
                "entryUnlisted",
                &format!("entries[{}]", {
                    // The manifest is entry 0; the last archive entry is unlisted.
                    read_entries(&original).1.len()
                }),
            ),
        ),
        (
            "a content entry whose bytes changed",
            rebuilt(
                &directory,
                &original,
                |_| {},
                |manifest| manifest.entries[content_index].sha256 = "e".repeat(64),
            ),
            StagingOptions::default(),
            invalid(
                "checksumMismatch",
                &format!("manifest.entries[{content_index}]"),
            ),
        ),
        (
            "a content entry longer than declared",
            rebuilt(
                &directory,
                &original,
                |_| {},
                |manifest| manifest.entries[content_index].bytes -= 1,
            ),
            StagingOptions::default(),
            invalid(
                "sizeMismatch",
                &format!("manifest.entries[{content_index}]"),
            ),
        ),
        (
            "a content entry named for another hash",
            rebuilt(
                &directory,
                &original,
                |entries| entries[content_index].1.push(b'!'),
                |_| {},
            ),
            StagingOptions::default(),
            invalid(
                "contentNameMismatch",
                &format!("manifest.entries[{content_index}]"),
            ),
        ),
        (
            "a newer format",
            rebuilt(
                &directory,
                &original,
                |_| {},
                |manifest| manifest.format_version = 2,
            ),
            StagingOptions::default(),
            json!({
                "code": "UNSUPPORTED_BACKUP_FORMAT",
                "message": "This backup was made by a newer version of farm3d.",
                "recovery": ["UPGRADE_FARM3D"],
                "retryable": false,
                "details": { "supportedFormatVersion": 1, "receivedFormatVersion": 2 },
            }),
        ),
        (
            "a newer schema",
            rebuilt(
                &directory,
                &original,
                |_| {},
                |manifest| manifest.schema_version = CURRENT_SCHEMA_VERSION + 1,
            ),
            StagingOptions::default(),
            json!({
                "code": "UNSUPPORTED_SCHEMA_VERSION",
                "message": "This backup was made by a newer version of farm3d.",
                "recovery": ["UPGRADE_FARM3D"],
                "retryable": false,
                "details": {
                    "supportedVersion": CURRENT_SCHEMA_VERSION,
                    "receivedVersion": CURRENT_SCHEMA_VERSION + 1,
                },
            }),
        ),
        (
            "a schema older than the window",
            rebuilt(
                &directory,
                &original,
                |_| {},
                |manifest| manifest.schema_version = 9,
            ),
            StagingOptions::default(),
            invalid("manifestInvalid", "manifest.schemaVersion"),
        ),
        (
            "a manifest migration that isn't the binary's",
            rebuilt(
                &directory,
                &original,
                |_| {},
                |manifest| manifest.migrations[2].checksum = "0".repeat(64),
            ),
            StagingOptions::default(),
            invalid("migrationMismatch", "manifest.migrations[2]"),
        ),
        (
            "a manifest missing a migration",
            rebuilt(
                &directory,
                &original,
                |_| {},
                |manifest| {
                    manifest.migrations.pop();
                },
            ),
            StagingOptions::default(),
            invalid("migrationMismatch", "manifest.migrations"),
        ),
        (
            "a database whose migrations differ from the manifest's",
            with_database(
                &directory,
                &original,
                |db| {
                    db.execute(
                        "UPDATE schema_migrations SET name = 'renamed' WHERE version = 3",
                        [],
                    )
                    .unwrap();
                },
                |_| {},
            ),
            StagingOptions::default(),
            invalid("migrationMismatch", "manifest.migrations"),
        ),
        (
            "a database whose user_version isn't the manifest's",
            with_database(
                &directory,
                &original,
                |db| db.execute_batch("PRAGMA user_version = 9").unwrap(),
                |_| {},
            ),
            StagingOptions::default(),
            invalid("migrationMismatch", "manifest.schemaVersion"),
        ),
        (
            "a manifest count that differs from the database",
            rebuilt(
                &directory,
                &original,
                |_| {},
                |manifest| *manifest.counts.get_mut("spools").unwrap() += 1,
            ),
            StagingOptions::default(),
            invalid("countMismatch", "manifest.counts.spools"),
        ),
        (
            "a manifest table the database lacks",
            rebuilt(
                &directory,
                &original,
                |_| {},
                |manifest| {
                    manifest.counts.insert("widgets".to_string(), 0);
                },
            ),
            StagingOptions::default(),
            invalid("countMismatch", "manifest.counts.widgets"),
        ),
        (
            "a database with a foreign-key violation",
            with_database(
                &directory,
                &original,
                |db| {
                    db.execute_batch(
                        "PRAGMA foreign_keys = OFF;
                         INSERT INTO material_slots(id, printer_id, position, name, created_at)
                         VALUES ('slt-orphan', 'prn-missing', 0, 'Main', '2026-01-01');",
                    )
                    .unwrap();
                },
                |manifest| *manifest.counts.get_mut("material_slots").unwrap() += 1,
            ),
            StagingOptions::default(),
            invalid("databaseInvalid", "database"),
        ),
        (
            "a database that breaks the integrity catalogue",
            with_database(
                &directory,
                &original,
                |db| {
                    db.execute(
                        "UPDATE jobs SET correction_event_id = 'sev-missing' WHERE id = 'job-a'",
                        [],
                    )
                    .unwrap();
                },
                |_| {},
            ),
            StagingOptions::default(),
            invalid("databaseInvalid", "database"),
        ),
        (
            "a database with a schema object farm3d never creates",
            with_database(
                &directory,
                &original,
                |db| {
                    db.execute_batch(
                        "CREATE TRIGGER planted AFTER INSERT ON settings BEGIN
                           DELETE FROM printers;
                         END;",
                    )
                    .unwrap();
                },
                |_| {},
            ),
            StagingOptions::default(),
            invalid("databaseInvalid", "database"),
        ),
        (
            "a database entry that isn't a database",
            rebuilt(
                &directory,
                &original,
                |entries| entries[0].1 = b"not a database at all".to_vec(),
                |_| {},
            ),
            StagingOptions::default(),
            invalid("databaseInvalid", "database"),
        ),
        (
            "too little free space to stage",
            original.clone(),
            StagingOptions {
                available_bytes: Some(10),
                ..StagingOptions::default()
            },
            json!({
                "code": "INSUFFICIENT_SPACE",
                "message": "There isn't enough free disk space.",
                "recovery": ["RETRY"],
                "retryable": false,
                "details": { "target": "restoreStaging" },
            }),
        ),
    ];

    for (case, path, options, expected) in cases {
        let error = stage_with(local.paths(), &path, &options)
            .err()
            .unwrap_or_else(|| panic!("{case}: staged"));
        let mut actual = shape(error_json(error));
        if expected["code"] == "INSUFFICIENT_SPACE" {
            let details = actual["details"].as_object_mut().unwrap();
            assert_eq!(details.remove("availableBytes"), Some(json!(10)), "{case}");
            assert!(details.remove("requiredBytes").unwrap().as_u64().unwrap() > 10);
        }
        assert_eq!(actual, expected, "{case}");
        assert!(
            staged_directories(local.paths()).is_empty(),
            "{case}: left {:?}",
            staged_directories(local.paths())
        );
    }
    assert_eq!(live_database_bytes(local.paths()), before);

    // The untampered original still stages.
    stage(&local, &original);
}

// --- expiry and startup cleanup ------------------------------------------------------

#[test]
fn a_staging_expires_after_24_hours_and_an_unknown_id_is_expired() {
    let remote = farm();
    let archive = write_archive(&remote, BackupMediaChoice::None);
    let local = farm();
    let candidate = stage(&local, &archive);
    let id = candidate.staging_id.clone();
    let created = candidate.created_at;
    let stagings = Stagings::default();
    stagings.replace(candidate);

    let resolved = stagings
        .resolve(
            &id,
            created + Duration::hours(24) - Duration::milliseconds(1),
        )
        .expect("still fresh");
    assert_eq!(resolved.staging_id, id);

    for (label, staging_id, now) in [
        ("expired", id.as_str(), created + Duration::hours(24)),
        (
            "unknown",
            "stg-00000000-0000-4000-8000-000000000000",
            created,
        ),
    ] {
        let error = stagings
            .resolve(staging_id, now)
            .err()
            .unwrap_or_else(|| panic!("{label} resolved"));
        assert_eq!(
            shape(error_json(error)),
            json!({
                "code": "RESTORE_STAGING_EXPIRED",
                "message": "This restore preview expired. Preview the backup again.",
                "recovery": [],
                "retryable": false,
                "details": { "stagingId": staging_id },
            }),
            "{label}"
        );
    }
    // An id that isn't a staging id is never echoed.
    let error = stagings.resolve("../../etc", created).err().unwrap();
    assert_eq!(
        error_json(error)["details"],
        json!({ "stagingId": "stagingId" })
    );
}

/// A Farm directory tree (no seed) whose Storage can be dropped and
/// reopened, as a restart does.
struct Reopenable {
    _temp: tempfile::TempDir,
    paths: StoragePaths,
    lease: MetadataRootLease,
}

impl Reopenable {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths =
            StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
        let lease = MetadataRootLease::acquire(&paths).unwrap();
        drop(Storage::open(paths.clone(), &lease).unwrap());
        Self {
            _temp: temp,
            paths,
            lease,
        }
    }

    fn restart(&self) {
        drop(Storage::open(self.paths.clone(), &self.lease).expect("reopen"));
    }

    fn write_journal(&self, body: &str) {
        let directory = self.paths.metadata_root().join("restore");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("journal.json"), body).unwrap();
    }
}

#[test]
fn startup_removes_every_staging_except_the_one_a_pending_or_installing_journal_names() {
    let remote = farm();
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let farm_dirs = Reopenable::new();
    let paths = &farm_dirs.paths;
    let kept = stage_with(paths, &archive, &StagingOptions::default()).unwrap();
    let other = stage_with(paths, &archive, &StagingOptions::default()).unwrap();
    // Strays of each kind, and a half-written staging.
    fs::create_dir_all(paths.snapshot_root().join(".restore-staging/incomplete")).unwrap();
    fs::create_dir_all(paths.content_root().join("staging/restore-stg-stray/ab")).unwrap();
    fs::create_dir_all(paths.media_root().join("restore-stg-stray/snapshots")).unwrap();
    let dirs = |candidate: &StagedCandidate| {
        [
            candidate.layout.database_dir.clone(),
            candidate.layout.content_dir.clone(),
            candidate.layout.media_dir.clone(),
        ]
    };

    for phase in ["pending", "installing"] {
        farm_dirs.write_journal(
            &json!({
                "journalVersion": 1,
                "id": "rst-00000000-0000-4000-8000-000000000001",
                "kind": "restore",
                "phase": phase,
                "stagingId": kept.staging_id,
            })
            .to_string(),
        );
        farm_dirs.restart();
        for directory in dirs(&kept) {
            assert!(
                directory.is_dir(),
                "{phase}: {} removed",
                directory.display()
            );
        }
        let mut expected: Vec<String> = dirs(&kept)
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        expected.sort();
        assert_eq!(staged_directories(paths), expected, "{phase}");
        // The kept staging is intact.
        assert!(kept.layout.candidate_path().is_file());
        let _ = &other;
    }

    // An unreadable journal touches nothing (the installer refuses to
    // start on it).
    farm_dirs.write_journal("{ not json");
    farm_dirs.restart();
    assert!(kept.layout.candidate_path().is_file());

    // A finished journal protects nothing.
    for phase in ["installed", "done", "failed"] {
        let before = staged_directories(paths);
        farm_dirs.write_journal(
            &json!({
                "journalVersion": 1,
                "id": "rst-00000000-0000-4000-8000-000000000001",
                "kind": "restore",
                "phase": phase,
                "stagingId": kept.staging_id,
            })
            .to_string(),
        );
        farm_dirs.restart();
        if phase == "installed" {
            assert!(before.len() == 3);
        }
        assert!(staged_directories(paths).is_empty(), "{phase}");
    }

    // No journal at all: everything goes.
    fs::remove_file(paths.metadata_root().join("restore/journal.json")).unwrap();
    stage_with(paths, &archive, &StagingOptions::default()).unwrap();
    farm_dirs.restart();
    assert!(staged_directories(paths).is_empty());
}

#[test]
fn discarding_a_staging_removes_all_three_directories() {
    let remote = farm();
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let local = farm();
    let candidate = stage(&local, &archive);
    let id = candidate.staging_id.clone();
    let stagings = Stagings::default();
    stagings.replace(candidate);
    assert_eq!(staged_directories(local.paths()).len(), 3);
    assert!(!stagings.discard("stg-00000000-0000-4000-8000-000000000000"));
    assert_eq!(staged_directories(local.paths()).len(), 3);
    assert!(stagings.discard(&id));
    assert!(staged_directories(local.paths()).is_empty());
    assert!(!stagings.discard(&id), "nothing staged under that id now");

    // Replacing a staging removes the one before it.
    let first = stage(&local, &archive);
    let first_dirs = first.layout.database_dir.clone();
    stagings.replace(first);
    let second = stage(&local, &archive);
    stagings.replace(second);
    assert!(!first_dirs.exists());
    assert_eq!(staged_directories(local.paths()).len(), 3);
}

// --- Step 2: the conflict fixtures ---------------------------------------------------

#[test]
fn c01_equal_settings_are_not_a_conflict() {
    let preview = fixture("", "");
    assert_eq!(conflicts(&preview), vec![]);
}

#[test]
fn c02_a_changed_theme_is_changed_settings() {
    let preview = fixture(
        "UPDATE settings SET theme_mode = 'farm3d-dark';",
        "UPDATE settings SET theme_mode = 'system';",
    );
    assert_eq!(
        conflicts(&preview),
        vec![changed(RestoreDomain::Settings, "settings", "Settings")]
    );
}

#[test]
fn c03_an_equal_active_printer_with_the_same_identity_is_not_a_conflict() {
    let both = format!("UPDATE printers SET host_identity = '{IDENTITY_H}' WHERE id = 'prn-a';");
    assert_eq!(conflicts(&fixture(&both, &both)), vec![]);
}

#[test]
fn c04_a_renamed_printer_is_changed() {
    let preview = fixture(
        "UPDATE printers SET name = 'Left' WHERE id = 'prn-a';",
        "UPDATE printers SET name = 'Left Carbon' WHERE id = 'prn-a';",
    );
    assert_eq!(
        conflicts(&preview),
        vec![changed(RestoreDomain::Printer, "prn-a", "Left")]
    );
}

#[test]
fn c05_a_revision_difference_alone_is_changed() {
    let preview = fixture(
        "UPDATE printers SET revision = 3 WHERE id = 'prn-a';",
        "UPDATE printers SET revision = 2 WHERE id = 'prn-a';",
    );
    assert_eq!(
        conflicts(&preview),
        vec![changed(
            RestoreDomain::Printer,
            "prn-a",
            secrets::PRINTER_NAME
        )]
    );
}

#[test]
fn c06_two_active_printers_with_one_identity_clash() {
    let preview = fixture(
        &printer("prn-l2", "L2", Some("192.0.2.20:7125"), false),
        &printer("prn-b2", "B2", Some("192.0.2.20:7125"), false),
    );
    assert_eq!(
        conflicts(&preview),
        vec![unique_clash(
            RestoreDomain::Printer,
            "prn-l2",
            "prn-b2",
            "L2"
        )]
    );
}

#[test]
fn c07_an_archived_backup_printer_does_not_clash() {
    let preview = fixture(
        &printer("prn-l2", "L2", Some(IDENTITY_H), false),
        &printer("prn-b2", "B2", Some(IDENTITY_H), true),
    );
    assert_eq!(
        conflicts(&preview),
        vec![only_local(RestoreDomain::Printer, "prn-l2", "L2")]
    );
}

#[test]
fn c08_an_archived_local_printer_does_not_clash() {
    let preview = fixture(
        &printer("prn-l2", "L2", Some(IDENTITY_H), true),
        &printer("prn-b2", "B2", Some(IDENTITY_H), false),
    );
    assert_eq!(
        conflicts(&preview),
        vec![only_local(RestoreDomain::Printer, "prn-l2", "L2")]
    );
}

#[test]
fn c09_printers_without_a_connection_do_not_clash() {
    let preview = fixture(
        &printer("prn-l2", "L2", None, false),
        &printer("prn-b2", "B2", None, false),
    );
    assert_eq!(
        conflicts(&preview),
        vec![only_local(RestoreDomain::Printer, "prn-l2", "L2")]
    );
}

#[test]
fn c10_a_backup_only_printer_is_added_not_a_conflict() {
    let preview = fixture("", &printer("prn-b3", "B3", None, false));
    assert_eq!(conflicts(&preview), vec![]);
    assert_eq!(count(&preview, "printers"), (2, 3));
}

#[test]
fn c11_a_clash_through_a_shared_id() {
    let preview = fixture(
        &format!(
            "UPDATE printers SET host_identity = '{IDENTITY_K}' WHERE id = 'prn-a';
             {}",
            printer("prn-l2", "L2", Some(IDENTITY_H), false)
        ),
        &format!("UPDATE printers SET host_identity = '{IDENTITY_H}' WHERE id = 'prn-a';"),
    );
    assert_eq!(
        conflicts(&preview),
        vec![
            changed(RestoreDomain::Printer, "prn-a", secrets::PRINTER_NAME),
            unique_clash(RestoreDomain::Printer, "prn-l2", "prn-a", "L2"),
        ]
    );
}

#[test]
fn c12_the_same_printer_archived_in_the_backup_is_only_changed() {
    let both = format!("UPDATE printers SET host_identity = '{IDENTITY_H}' WHERE id = 'prn-a';");
    let preview = fixture(
        &both,
        &format!("{both} UPDATE printers SET archived_at = '{NOW}' WHERE id = 'prn-a';"),
    );
    assert_eq!(
        conflicts(&preview),
        vec![changed(
            RestoreDomain::Printer,
            "prn-a",
            secrets::PRINTER_NAME
        )]
    );
}

#[test]
fn c13_two_spools_with_one_number_clash() {
    let preview = fixture(&spool("spl-s2", 12), &spool("spl-t2", 12));
    assert_eq!(
        conflicts(&preview),
        vec![unique_clash(
            RestoreDomain::Spool,
            "spl-s2",
            "spl-t2",
            "#12"
        )]
    );
}

#[test]
fn c14_a_local_only_spool_is_only_local() {
    let preview = fixture(&spool("spl-s2", 12), "");
    assert_eq!(
        conflicts(&preview),
        vec![only_local(RestoreDomain::Spool, "spl-s2", "#12")]
    );
}

#[test]
fn c15_a_spool_amount_difference_is_changed() {
    let preview = fixture(
        "UPDATE spools SET current_mg = 800000 WHERE id = 'spl-a';",
        "UPDATE spools SET current_mg = 750000 WHERE id = 'spl-a';",
    );
    assert_eq!(
        conflicts(&preview),
        vec![changed(RestoreDomain::Spool, "spl-a", "#1")]
    );
}

#[test]
fn c16_a_spool_number_clash_through_a_shared_id() {
    let preview = fixture(
        &format!(
            "UPDATE spools SET spool_number = 12 WHERE id = 'spl-a'; {}",
            spool("spl-s2", 14)
        ),
        "UPDATE spools SET spool_number = 14 WHERE id = 'spl-a';",
    );
    assert_eq!(
        conflicts(&preview),
        vec![
            changed(RestoreDomain::Spool, "spl-a", "#12"),
            unique_clash(RestoreDomain::Spool, "spl-s2", "spl-a", "#14"),
        ]
    );
}

#[test]
fn c17_tare_names_clash_under_an_ascii_case_fold() {
    let preview = fixture(
        &tare("tar-r2", "Cardboard 1kg"),
        &tare("tar-r3", "cardboard 1KG"),
    );
    assert_eq!(
        conflicts(&preview),
        vec![unique_clash(
            RestoreDomain::Tare,
            "tar-r2",
            "tar-r3",
            "Cardboard 1kg"
        )]
    );
}

#[test]
fn c18_tare_names_that_differ_outside_ascii_do_not_clash() {
    let preview = fixture(&tare("tar-r2", "Ölspule"), &tare("tar-r3", "ölspule"));
    assert_eq!(
        conflicts(&preview),
        vec![only_local(RestoreDomain::Tare, "tar-r2", "Ölspule")]
    );
}

#[test]
fn c19_project_names_clash() {
    let preview = fixture(
        &project("prj-p2", "Brackets"),
        &project("prj-p3", "BRACKETS"),
    );
    assert_eq!(
        conflicts(&preview),
        vec![unique_clash(
            RestoreDomain::Project,
            "prj-p2",
            "prj-p3",
            "Brackets"
        )]
    );
}

#[test]
fn c20_an_equal_project_is_not_a_conflict() {
    let preview = fixture("", "");
    assert!(preview
        .conflicts
        .iter()
        .all(|group| group.domain != RestoreDomain::Project));
    assert_eq!(count(&preview, "library_projects"), (1, 1));
}

#[test]
fn c21_a_backup_only_project_is_added_not_a_conflict() {
    let preview = fixture("", &project("prj-p3", "Hinges"));
    assert_eq!(conflicts(&preview), vec![]);
    assert_eq!(count(&preview, "library_projects"), (1, 2));
}

#[test]
fn c22_a_local_only_model_is_only_local() {
    let preview = fixture(
        &format!(
            "INSERT INTO library_models(id, revision, name, format, storage_mode, created_at,
               updated_at)
             VALUES ('mdl-m2', 1, 'Clip', 'stl', 'managed', '{NOW}', '{NOW}');"
        ),
        "",
    );
    assert_eq!(
        conflicts(&preview),
        vec![only_local(RestoreDomain::Model, "mdl-m2", "Clip")]
    );
}

#[test]
fn c23_a_renamed_model_is_changed() {
    let preview = fixture(
        "UPDATE library_models SET name = 'Hook v2' WHERE id = 'mdl-a';",
        "UPDATE library_models SET name = 'Hook' WHERE id = 'mdl-a';",
    );
    assert_eq!(
        conflicts(&preview),
        vec![changed(RestoreDomain::Model, "mdl-a", "Hook v2")]
    );
}

#[test]
fn c24_a_local_only_slice_revision_is_only_local() {
    let preview = fixture(
        &format!(
            "INSERT INTO slice_revisions(id, kind, model_id, source_revision_id, gcode_sha256,
               gcode_size, target_json, facts_json, requires_manual_printer_selection,
               estimates_json, created_at)
             VALUES ('slr-sr2', 'external', 'mdl-a', 'msr-a', '{hash}', 200, '{{}}', '{{}}', 1,
                     '{{}}', '{NOW}');",
            hash = p9_farm::farm_seed::GCODE_HASH
        ),
        "",
    );
    assert_eq!(
        conflicts(&preview),
        vec![only_local(
            RestoreDomain::SliceRevision,
            "slr-sr2",
            "slr-sr2"
        )]
    );
}

#[test]
fn c25_a_closed_queue_entry_is_changed() {
    let preview = fixture(
        &format!(
            "UPDATE queue_entries SET state = 'closed', close_reason = 'removed', position = NULL,
               closed_at = '{NOW}' WHERE id = 'qen-a';"
        ),
        "",
    );
    assert_eq!(
        conflicts(&preview),
        vec![changed(RestoreDomain::QueueEntry, "qen-a", "qen-a")]
    );
}

#[test]
fn c26_a_job_finished_after_the_backup_is_only_local() {
    let preview = fixture(&finished_job("j2"), "");
    assert_eq!(
        conflicts(&preview)
            .into_iter()
            .filter(|group| group.1 == RestoreDomain::Job)
            .collect::<Vec<_>>(),
        vec![only_local(RestoreDomain::Job, "job-j2", "job-j2")]
    );
}

#[test]
fn c27_an_equal_job_is_not_a_conflict() {
    let preview = fixture("", "");
    assert!(preview
        .conflicts
        .iter()
        .all(|group| group.domain != RestoreDomain::Job));
    assert_eq!(count(&preview, "jobs"), (1, 1));
}

#[test]
fn c28_a_closed_incident_is_changed() {
    let preview = fixture(
        &format!("UPDATE incidents SET closed_at = '{NOW}' WHERE id = 'inc-a';"),
        "",
    );
    assert_eq!(
        conflicts(&preview),
        vec![changed(RestoreDomain::Incident, "inc-a", "inc-a")]
    );
}

#[test]
fn c29_a_read_attention_event_is_changed() {
    let preview = fixture(
        &format!("UPDATE attention_events SET read_at = '{NOW}' WHERE id = 'att-off';"),
        "",
    );
    assert_eq!(
        conflicts(&preview),
        vec![changed(RestoreDomain::AttentionEvent, "att-off", "att-off")]
    );
}

#[test]
fn c30_a_local_only_snapshot_is_only_local() {
    let preview = fixture(
        &format!(
            "INSERT INTO camera_snapshots(id, printer_id, trigger, operation_id, captured_at,
               content_type, byte_len, sha256, rel_path)
             VALUES ('snp-n2', 'prn-a', 'manual', 'snp-n2-op', '{NOW}', 'image/jpeg', 100,
                     '{sha}', 'snapshots/2026/01/snp-n2.jpg');",
            sha = "c".repeat(64)
        ),
        "",
    );
    assert_eq!(
        conflicts(&preview),
        vec![only_local(RestoreDomain::Snapshot, "snp-n2", "snp-n2")]
    );
}

#[test]
fn c31_a_snapshot_pruned_locally_is_changed() {
    let preview = fixture(
        &format!(
            "UPDATE camera_snapshots SET pruned_at = '{NOW}', prune_reason = 'age'
              WHERE id = 'snp-b';"
        ),
        "",
    );
    assert_eq!(
        conflicts(&preview),
        vec![changed(RestoreDomain::Snapshot, "snp-b", "snp-b")]
    );
}

#[test]
fn c32_a_renamed_child_row_is_not_a_conflict() {
    let slot = format!(
        "INSERT INTO material_slots(id, printer_id, position, name, created_at)
         VALUES ('slt-a', 'prn-a', 0, 'Main', '{NOW}');"
    );
    let preview = fixture(
        &format!("{slot} UPDATE material_slots SET name = 'Left spool' WHERE id = 'slt-a';"),
        &slot,
    );
    assert_eq!(conflicts(&preview), vec![]);
    assert_eq!(count(&preview, "material_slots"), (1, 1));
}

#[test]
fn c33_the_excluded_telemetry_cache_is_only_counted() {
    let preview = fixture("", "");
    assert_eq!(conflicts(&preview), vec![]);
    assert_eq!(count(&preview, "printer_status_snapshots"), (1, 0));
}

#[test]
fn c34_the_slicer_runtime_is_kept_local_with_a_notice() {
    let preview = fixture("", "");
    assert_eq!(conflicts(&preview), vec![]);
    assert!(preview
        .notices
        .contains(&RestoreNotice::SlicerRuntimeKeptLocal));
}

#[test]
fn c35_a_large_group_is_capped_at_200_items_in_id_order() {
    let jobs: String = (0..250)
        .map(|index| finished_job(&format!("c35-{index:03}")))
        .collect();
    let preview = fixture(&jobs, "");
    // Each Job brings its own (local-only) Queue Entry; the Job groups are
    // what this row is about.
    let groups: Vec<Group> = conflicts(&preview)
        .into_iter()
        .filter(|group| group.1 == RestoreDomain::Job)
        .collect();
    assert_eq!(groups.len(), 1, "one Job group");
    let (class, domain, total, items) = &groups[0];
    assert_eq!(
        (*class, *domain, *total),
        (RestoreConflictClass::OnlyLocal, RestoreDomain::Job, 250)
    );
    let expected: Vec<Item> = (0..200)
        .map(|index| {
            let id = format!("job-c35-{index:03}");
            (Some(id.clone()), None, id)
        })
        .collect();
    assert_eq!(items, &expected);
}

#[test]
fn c36_two_clashes_are_ordered_by_local_id() {
    let preview = fixture(
        &format!(
            "{}{}",
            printer("prn-l3", "L3", Some(IDENTITY_K), false),
            printer("prn-l2", "L2", Some(IDENTITY_H), false)
        ),
        &format!(
            "{}{}",
            printer("prn-b2", "B2", Some(IDENTITY_H), false),
            printer("prn-b3", "B3", Some(IDENTITY_K), false)
        ),
    );
    assert_eq!(
        conflicts(&preview),
        vec![(
            RestoreConflictClass::UniqueClash,
            RestoreDomain::Printer,
            2,
            vec![
                (
                    Some("prn-l2".to_string()),
                    Some("prn-b2".to_string()),
                    "L2".to_string()
                ),
                (
                    Some("prn-l3".to_string()),
                    Some("prn-b3".to_string()),
                    "L3".to_string()
                ),
            ]
        )]
    );
}

#[test]
fn conflict_groups_follow_class_then_domain_order() {
    let preview = fixture(
        &format!(
            "UPDATE settings SET theme_mode = 'farm3d-dark';
             UPDATE spools SET current_mg = 800000 WHERE id = 'spl-a';
             {}{}{}",
            spool("spl-s2", 12),
            printer("prn-l2", "L2", None, false),
            tare("tar-r2", "Cardboard")
        ),
        &tare("tar-r3", "CARDBOARD"),
    );
    let order: Vec<(RestoreConflictClass, RestoreDomain)> = conflicts(&preview)
        .into_iter()
        .map(|(class, domain, _, _)| (class, domain))
        .collect();
    assert_eq!(
        order,
        vec![
            (RestoreConflictClass::OnlyLocal, RestoreDomain::Printer),
            (RestoreConflictClass::OnlyLocal, RestoreDomain::Spool),
            (RestoreConflictClass::Changed, RestoreDomain::Settings),
            (RestoreConflictClass::Changed, RestoreDomain::Spool),
            (RestoreConflictClass::UniqueClash, RestoreDomain::Tare),
        ]
    );
}

// --- counts, notices, blockers -----------------------------------------------------

#[test]
fn counts_list_every_table_local_versus_backup_sorted() {
    let local = farm();
    let remote = farm();
    sql(&remote, &project("prj-p3", "Hinges"));
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let candidate = stage(&local, &archive);
    let preview = preview_of(&local, &candidate);

    let local_counts = local.counts();
    let tables: Vec<&str> = preview
        .counts
        .iter()
        .map(|count| count.table.as_str())
        .collect();
    let mut sorted = tables.clone();
    sorted.sort();
    assert_eq!(tables, sorted, "sorted by table");
    assert_eq!(
        tables,
        local_counts.keys().map(String::as_str).collect::<Vec<_>>(),
        "every table either side has"
    );
    for count in &preview.counts {
        assert_eq!(count.local, local_counts[&count.table], "{}", count.table);
        assert_eq!(
            count.backup, candidate.manifest.counts[&count.table],
            "{}",
            count.table
        );
    }
    assert_eq!(count(&preview, "library_projects"), (1, 2));
    // The sanitized classes are empty in the candidate.
    for table in [
        "printer_status_snapshots",
        "pending_credential_cleanup",
        "pending_blob_cleanup",
    ] {
        assert_eq!(count(&preview, table).1, 0, "{table}");
    }
}

#[test]
fn the_preview_describes_the_backup_and_its_expiry() {
    let local = farm();
    let remote = farm();
    let archive = write_archive(&remote, BackupMediaChoice::Pinned);
    let candidate = stage(&local, &archive);
    let preview = preview_of(&local, &candidate);
    assert_eq!(preview.staging_id, candidate.staging_id);
    assert_eq!(
        DateTime::parse_from_rfc3339(&preview.expires_at).unwrap(),
        DateTime::parse_from_rfc3339(&preview.created_at).unwrap() + Duration::hours(24)
    );
    let backup = serde_json::to_value(&preview.backup).unwrap();
    assert_eq!(
        backup,
        json!({
            "createdAt": CREATED_AT,
            "appVersion": "0.1.0",
            "schemaVersion": CURRENT_SCHEMA_VERSION,
            "formatVersion": 1,
            "origin": "operator",
            "media": "pinned",
            "platform": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH },
        })
    );
    assert_eq!(
        serde_json::to_value(&preview.source).unwrap(),
        json!({ "kind": "file", "fileName": "backup.farm3d-backup" })
    );
}

#[test]
fn notices_follow_the_type_order() {
    // Every notice at once: the local store lacks Printer B's credential,
    // a local Printer's ref isn't in the backup, one linked Model's path
    // is missing here, the backup has an active Job, and it left media out.
    let existing = tempfile::NamedTempFile::new().unwrap();
    let existing_path = existing.path().to_string_lossy().into_owned();
    let linked = |id: &str, path: &str| {
        format!(
            "INSERT INTO library_models(id, revision, name, format, storage_mode, linked_path,
               link_state, created_at, updated_at)
             VALUES ('{id}', 1, '{id}', 'stl', 'linked', '{path}', 'ok', '{NOW}', '{NOW}');"
        )
    };
    let preview = fixture_with(
        &format!(
            "{}UPDATE printers SET connection_json = json_object('kind', 'moonraker',
               'host', '192.0.2.30', 'port', 7125, 'useTls', 0,
               'credentialRef', 'farm3d/printer/prn-l2/apikey') WHERE id = 'prn-l2';",
            printer("prn-l2", "L2", None, false)
        ),
        &format!(
            "{}{}{}",
            linked("mdl-linked-gone", "/nonexistent/p9/restore/part.stl"),
            linked("mdl-linked-here", &existing_path),
            active_job("act", "prn-b"),
        ),
        BackupMediaChoice::Pinned,
        |local, _| {
            CredentialStore::file_backed(local.credentials_dir.clone())
                .delete(ids::CREDENTIAL_REF_B)
                .unwrap();
        },
    );
    assert_eq!(
        preview.notices,
        vec![
            RestoreNotice::CredentialsToReenter { printer_count: 1 },
            // The seeded `cred-a` cleanup row and Printer L2's ref.
            RestoreNotice::CredentialsOrphaned { ref_count: 2 },
            RestoreNotice::LinkedPathsMissing { model_count: 1 },
            RestoreNotice::ActiveJobsAtBackup { job_count: 1 },
            RestoreNotice::MediaNotInBackup {
                snapshot_count: 2,
                missing_file_count: 0
            },
            RestoreNotice::SlicerRuntimeKeptLocal,
        ]
    );
    // No credential value reaches the preview.
    let wire = serde_json::to_vec(&preview).unwrap();
    secrets::assert_no_backup_forbidden(&wire, "preview");
}

#[test]
fn a_baseline_preview_has_only_the_always_notices() {
    let preview = fixture("", "");
    assert_eq!(
        preview.notices,
        vec![
            // The seeded `cred-a` cleanup row names a ref no restored
            // Printer uses.
            RestoreNotice::CredentialsOrphaned { ref_count: 1 },
            RestoreNotice::SlicerRuntimeKeptLocal,
        ]
    );
}

#[test]
fn blockers_are_listed_by_kind_then_id_and_capped_at_50() {
    let local = farm();
    // The every-domain Farm's `hop-a` is `dispatching`. Add an active and
    // an `outcomeUnknown` Job, and 60 queued or running slice operations.
    let mut statements = active_job("act", "prn-b");
    statements.push_str(&finished_job("unk"));
    statements.push_str(
        "UPDATE jobs SET state = 'outcomeUnknown', settlement = 'open', settlement_method = NULL,
           ended_at = NULL WHERE id = 'job-unk';",
    );
    for index in 0..60 {
        let state = if index % 2 == 0 { "queued" } else { "running" };
        statements.push_str(&format!(
            "INSERT INTO slice_operations(id, preparation_id, source_revision_id, plate_key,
               plate_snapshot_json, state, queued_at)
             VALUES ('sop-{index:02}', 'prp-a', 'msr-a', 'plate-1', '{{}}', '{state}', '{NOW}');"
        ));
    }
    // Finished work never blocks.
    statements.push_str(&format!(
        "INSERT INTO slice_operations(id, preparation_id, source_revision_id, plate_key,
           plate_snapshot_json, state, queued_at, finished_at)
         VALUES ('sop-done', 'prp-a', 'msr-a', 'plate-1', '{{}}', 'cancelled', '{NOW}', '{NOW}');"
    ));
    sql(&local, &statements);
    let remote = farm();
    let archive = write_archive(&remote, BackupMediaChoice::None);
    let candidate = stage(&local, &archive);
    let preview = preview_of(&local, &candidate);

    assert_eq!(preview.blocker_total, 2 + 1 + 60);
    assert_eq!(preview.blockers.len(), 50);
    let mut expected = vec![
        RestoreBlocker {
            kind: RestoreBlockerKind::ActiveJob,
            id: "job-act".to_string(),
        },
        RestoreBlocker {
            kind: RestoreBlockerKind::ActiveJob,
            id: "job-unk".to_string(),
        },
        RestoreBlocker {
            kind: RestoreBlockerKind::HostOperation,
            id: "hop-a".to_string(),
        },
    ];
    expected.extend((0..47).map(|index| RestoreBlocker {
        kind: RestoreBlockerKind::SliceOperation,
        id: format!("sop-{index:02}"),
    }));
    assert_eq!(preview.blockers, expected);
    // The live blocker query is the one `apply_restore` reuses.
    let (blockers, total) = local
        .storage
        .read_transaction(|tx| preview::blockers(tx))
        .unwrap();
    assert_eq!((blockers, total), (expected, 63));
}

#[test]
fn a_preview_leaves_the_live_farm_untouched() {
    let local = farm();
    let remote = farm();
    sql(&remote, &project("prj-p3", "Hinges"));
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let before = live_database_bytes(local.paths());
    let counts = local.counts();
    let blobs: Vec<String> = local
        .content_hashes()
        .iter()
        .map(|hash| sha256_file(&local.blob_path(hash)))
        .collect();
    let candidate = stage(&local, &archive);
    let preview = preview_of(&local, &candidate);
    assert!(!preview.counts.is_empty());
    assert_eq!(live_database_bytes(local.paths()), before);
    assert_eq!(local.counts(), counts);
    let after: Vec<String> = local
        .content_hashes()
        .iter()
        .map(|hash| sha256_file(&local.blob_path(hash)))
        .collect();
    assert_eq!(after, blobs);
}

// --- the commands -------------------------------------------------------------------

struct FakeDialogs {
    open: Mutex<Option<PathBuf>>,
}

impl PortabilityDialogs for FakeDialogs {
    fn open_backup(&self) -> Result<Option<PathBuf>, CommandError> {
        Ok(self.open.lock().unwrap().clone())
    }
    fn save_backup(&self, _suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
    fn save_diagnostics(&self, _suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
}

fn no_connection(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

struct Rig {
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
    dialogs: Arc<FakeDialogs>,
}

impl Rig {
    fn new(farm: &Farm) -> Rig {
        let dialogs = Arc::new(FakeDialogs {
            open: Mutex::new(None),
        });
        let injected: Arc<dyn PortabilityDialogs> = dialogs.clone();
        let (app, webview, _manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::backup::commands::preview_restore,
                farm3d_lib::backup::commands::discard_restore_preview,
            ],
            Arc::clone(&farm.storage),
            Arc::new(common::a_catalog()),
            farm.credentials_dir.clone(),
            no_connection,
            move |services| services.backup = Arc::new(services.backup.with_dialogs(injected)),
        );
        Rig {
            _app: app,
            webview,
            services,
            dialogs,
        }
    }

    fn open_with(&self, path: Option<PathBuf>) {
        *self.dialogs.open.lock().unwrap() = path;
    }

    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|envelope| envelope["data"].clone())
    }

    fn ok(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .unwrap_or_else(|error| panic!("{command}: {error}"))
    }

    fn err(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .err()
            .unwrap_or_else(|| panic!("{command} succeeded"))
    }
}

#[test]
fn preview_restore_stages_a_chosen_file_and_discard_removes_it() {
    let remote = farm();
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let local = farm();
    let rig = Rig::new(&local);

    // A closed dialog is `cancelled`, with nothing staged.
    assert_eq!(
        rig.ok("preview_restore", json!({ "source": { "kind": "file" } })),
        json!({ "status": "cancelled" })
    );
    assert!(staged_directories(local.paths()).is_empty());

    rig.open_with(Some(archive.clone()));
    let first = rig.ok("preview_restore", json!({ "source": { "kind": "file" } }));
    assert_eq!(first["status"], "previewed");
    let preview = &first["preview"];
    let first_id = preview["stagingId"].as_str().unwrap().to_string();
    assert!(is_staging_id(&first_id));
    assert_eq!(
        preview["source"],
        json!({ "kind": "file", "fileName": archive.file_name().unwrap().to_str().unwrap() })
    );
    assert_eq!(preview["blockerTotal"], 1, "the seeded dispatching hop-a");
    assert_eq!(
        preview["blockers"],
        json!([{ "kind": "hostOperation", "id": "hop-a" }])
    );
    // No full path anywhere in the result.
    let text = first.to_string();
    assert!(!text.contains(&*local.temp.path().to_string_lossy()));
    assert!(!text.contains(&*remote.temp.path().to_string_lossy()));
    assert_eq!(staged_directories(local.paths()).len(), 3);

    // A second preview replaces the first staging.
    let second = rig.ok("preview_restore", json!({ "source": { "kind": "file" } }));
    let second_id = second["preview"]["stagingId"].as_str().unwrap().to_string();
    assert_ne!(first_id, second_id);
    let remaining = staged_directories(local.paths());
    assert_eq!(remaining.len(), 3);
    assert!(remaining.iter().all(|path| path.contains(&second_id)));

    // Discarding an id that isn't staged is `false`; the current one goes.
    assert_eq!(
        rig.ok("discard_restore_preview", json!({ "stagingId": first_id })),
        json!({ "discarded": false })
    );
    assert_eq!(
        rig.ok("discard_restore_preview", json!({ "stagingId": second_id })),
        json!({ "discarded": true })
    );
    assert!(staged_directories(local.paths()).is_empty());
    assert_eq!(
        rig.ok("discard_restore_preview", json!({ "stagingId": second_id })),
        json!({ "discarded": false })
    );
}

#[test]
fn preview_restore_reads_a_safety_backup_by_id() {
    let local = farm();
    let rig = Rig::new(&local);
    let lease = BackupLease::new();
    let guard = lease.try_acquire(LeaseActivity::RestoreApply).unwrap();
    let safety = safety::write_safety_backup(
        &local.storage,
        &guard,
        BackupOrigin::BeforeRestore,
        created_at(),
        "0.1.0",
        &WriterHooks::default(),
    )
    .unwrap();
    drop(guard);

    let result = rig.ok(
        "preview_restore",
        json!({ "source": { "kind": "safetyBackup", "backupId": safety.backup_id } }),
    );
    assert_eq!(result["status"], "previewed");
    assert_eq!(
        result["preview"]["source"],
        json!({ "kind": "safetyBackup", "backupId": safety.backup_id })
    );
    assert_eq!(result["preview"]["backup"]["origin"], "beforeRestore");
    // Restoring the Farm's own safety backup changes nothing.
    assert_eq!(result["preview"]["conflicts"], json!([]));

    for backup_id in ["sfb-00000000-0000-4000-8000-000000000000", "../escape"] {
        let error = rig.err(
            "preview_restore",
            json!({ "source": { "kind": "safetyBackup", "backupId": backup_id } }),
        );
        assert_eq!(error["code"], "NOT_FOUND", "{backup_id}");
        assert!(!error.to_string().contains("escape"));
    }
}

#[test]
fn preview_restore_refuses_while_the_lease_is_held_and_maps_archive_errors() {
    let local = farm();
    let rig = Rig::new(&local);
    let bad = local.temp.path().join("bad.farm3d-backup");
    fs::write(&bad, b"not a zip").unwrap();
    rig.open_with(Some(bad));

    let guard = rig
        .services
        .backup
        .lease
        .try_acquire(LeaseActivity::Backup)
        .unwrap();
    let error = rig.err("preview_restore", json!({ "source": { "kind": "file" } }));
    assert_eq!(error["code"], "BACKUP_IN_PROGRESS");
    assert_eq!(error["details"], json!({ "activity": "backup" }));
    drop(guard);

    let error = rig.err("preview_restore", json!({ "source": { "kind": "file" } }));
    assert_eq!(
        shape(error),
        invalid("notAZip", "archive"),
        "the typed refusal with a safe field path"
    );
    assert!(staged_directories(local.paths()).is_empty());
    // The lease was released.
    assert!(rig
        .services
        .backup
        .lease
        .try_acquire(LeaseActivity::Backup)
        .is_ok());
}
