//! P9 Task 8 (spec D15, the reset parts of D8, D18): tiered reset.
//!
//! - Tier (a), `settings`: the defaults are applied after F1's pre-reset
//!   snapshot; the Slicer runtime and the per-Printer alert defaults stay;
//!   a stale revision is `CONFLICT`.
//! - Tier (b), `cameraMedia`: unpinned (or all) media pruned with reason
//!   `reset`; the rows and Incident timeline entries stay; the files are
//!   gone once the lease drops; `integrity::check` is clean.
//! - Tier (c), `farm`: `RESTART_PENDING` while a journal waits, the lease
//!   held (activity `reset`) by every tier, the exact phrase, the preview's
//!   classes and warnings, the safety backup first, and the installer's
//!   roll-forward from every reset row of the spec's "Installer fault
//!   points" table (below, copied verbatim). Each row injects a fault
//!   (`installer::Fault`: a crash drops every handle and returns, an error
//!   is handled in-run), then runs the installer again with no fault.
//!
//! **Reset (tier c, roll forward only)** (spec "Installer fault points",
//! verbatim)
//!
//! | # | Fault point | Journal at the fault | Next start | Outcome |
//! |---|---|---|---|---|
//! | r1 | after `reset_farm` writes the journal, before restart | `pending` | runs from `markInstalling` | reset done |
//! | r2 | `moveDatabaseAside` after the main file only | `installing`/`moveDatabaseAside` | moves the rest | reset done |
//! | r3 | `moveRootsAside` after `content_root` only | `installing`/`moveRootsAside` | skips the moved root, moves the rest | reset done |
//! | r4 | `createFreshDatabase` before its first commit | `installing`/`createFreshDatabase` | deletes the partial set, recreates | reset done |
//! | r5 | `deleteCredentials` after half the refs | `installing`/`deleteCredentials` | deletes the rest (absent counts as deleted) | reset done; every ref gone from the fake store |
//! | r6 | the credential store is unavailable | — (in-run) | — | reset done; every ref queued with reason `reset` |
//! | r7 | `deleteSafetyBackups` half done (option on) | `installing`/`deleteSafetyBackups` | deletes the rest | reset done; `safety/` holds only this reset's safety backup |
//! | r8 | `removePrevious` half done | `installing`/`removePrevious` | deletes the rest | reset done |
//! | r9 | after `markDone` | `done` | nothing | reset done; `restore_status` is `done` until acknowledged |
//! | r10 | an I/O error at `moveRootsAside`, twice | — | startup `RESTORE_FAILED` (`installFailed`) each time; the third start succeeds | reset done |
//!
//! "Reset done" means: the database is fresh at the current schema with no
//! domain rows, `content_root`, `media_root`, and `log_root` exist and are
//! empty, `snapshot_root` and `legacy_root` are empty, no `previous/` or
//! `.aside-*` directory remains, every journaled ref is deleted or queued
//! with reason `reset`, and `safety/` holds this reset's safety backup (if
//! one was written) and, unless the option was ticked, the older ones.

mod common;
mod p9_farm;
#[path = "common/secrets.rs"]
mod secrets;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};
use tauri::test::MockRuntime;

use farm3d_lib::backup::installer::{
    self, Fault, FaultEffect, FaultPoint, Faults, InstallOutcome, InstallReport, InstallerError,
};
use farm3d_lib::backup::journal::{self, JournalPhase, RestoreJournal};
use farm3d_lib::backup::lease::LeaseActivity;
use farm3d_lib::backup::restart::RecordingRestarter;
use farm3d_lib::backup::writer::WriterHooks;
use farm3d_lib::backup::{archive, InstallerStep, RestoreJournalKind};
use farm3d_lib::cameras::media::{self as camera_media};
use farm3d_lib::cameras::retention::MediaJanitor;
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::diagnostics::reset::{self, ResetMediaScope};
use farm3d_lib::persistence::integrity::{self, IntegrityRoots};
use farm3d_lib::persistence::{
    MetadataRootLease, RepositoryError, SnapshotKind, Storage, StoragePaths, CURRENT_SCHEMA_VERSION,
};
use farm3d_lib::RuntimeServices;

use p9_farm::{ids, Farm};

const CREATED_AT: &str = "2026-09-29T12:00:00.000Z";
const NOW: &str = ids::NOW;
const THIS_SAFETY: &str = "sfb-this-reset";
const OLDER_SAFETY: [&str; 2] = ["sfb-older-1", "sfb-older-2"];

fn created_at() -> DateTime<Utc> {
    CREATED_AT.parse().unwrap()
}

fn sql(farm: &Farm, statements: &str) {
    farm.storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            tx.execute_batch(statements)
                .unwrap_or_else(|error| panic!("fixture SQL failed: {error}\n{statements}"));
            Ok(())
        })
        .expect("fixture write");
}

fn query_i64(farm: &Farm, statement: &str) -> i64 {
    farm.storage
        .read(|connection| connection.query_row(statement, [], |row| row.get(0)))
        .unwrap()
}

fn query_text(farm: &Farm, statement: &str) -> Option<String> {
    farm.storage
        .read(|connection| connection.query_row(statement, [], |row| row.get(0)))
        .unwrap()
}

fn names_in(directory: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn safety_dir(paths: &StoragePaths) -> PathBuf {
    paths.backup_root().join("safety")
}

fn safety_file(id: &str) -> String {
    format!("{id}.farm3d-backup")
}

/// The pre-import settings snapshot databases (not their `-wal`/`-shm`).
fn pre_import_snapshots(paths: &StoragePaths) -> Vec<String> {
    names_in(paths.snapshot_root())
        .into_iter()
        .filter(|name| {
            name.starts_with(".farm3d-pre-import-settings-") && name.ends_with(".sqlite3")
        })
        .collect()
}

/// Every journaled ref's value, as stored: the Farm's two Printer refs
/// (`cred-a`, the seeded cleanup row, has no value).
fn stored_secrets(farm: &Farm) -> Vec<String> {
    let store = CredentialStore::file_backed(farm.credentials_dir.clone());
    [ids::CREDENTIAL_REF_A, ids::CREDENTIAL_REF_B]
        .iter()
        .map(|reference| store.get(reference).unwrap().expect("a stored value"))
        .collect()
}

// --- the command rig --------------------------------------------------------------------

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
    restarter: Arc<RecordingRestarter>,
}

impl Rig {
    fn new(farm: &Farm) -> Rig {
        Rig::with_hooks(farm, WriterHooks::default())
    }

    fn with_hooks(farm: &Farm, hooks: WriterHooks) -> Rig {
        let restarter = Arc::new(RecordingRestarter::default());
        let injected = Arc::clone(&restarter);
        let (app, webview, _manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::diagnostics::commands::reset_preview,
                farm3d_lib::diagnostics::commands::reset_farm,
                farm3d_lib::backup::commands::restore_status,
            ],
            Arc::clone(&farm.storage),
            Arc::new(common::a_catalog()),
            farm.credentials_dir.clone(),
            no_connection,
            move |services| {
                let mut backup = services
                    .backup
                    .with_dialogs(Arc::new(
                        farm3d_lib::backup::dialogs::CancelledPortabilityDialogs,
                    ))
                    .with_restarter(injected);
                backup.writer_hooks = hooks;
                services.backup = Arc::new(backup);
            },
        );
        Rig {
            _app: app,
            webview,
            services,
            restarter,
        }
    }

    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|envelope| envelope["data"].clone())
    }

    fn reset(
        &self,
        operation_id: &str,
        request: Value,
        confirmation: &str,
    ) -> Result<Value, Value> {
        self.call(
            "reset_farm",
            json!({
                "operationId": operation_id,
                "request": request,
                "confirmation": confirmation,
            }),
        )
    }

    fn preview(&self, tier: &str) -> Value {
        self.call("reset_preview", json!({ "tier": tier }))
            .unwrap_or_else(|error| panic!("reset_preview: {error}"))
    }

    /// Counts the lease's releases from now on.
    fn releases(&self) -> Arc<AtomicUsize> {
        let counter = Arc::new(AtomicUsize::new(0));
        let hook = Arc::clone(&counter);
        self.services.backup.lease.on_release(move || {
            hook.fetch_add(1, Ordering::SeqCst);
        });
        counter
    }
}

fn settings_request(expected_revision: i64) -> Value {
    json!({ "tier": "settings", "expectedRevision": expected_revision })
}

fn media_request(scope: &str) -> Value {
    json!({ "tier": "cameraMedia", "scope": scope })
}

fn farm_request(safety_backup: bool, delete_safety_backups: bool) -> Value {
    json!({
        "tier": "farm",
        "safetyBackup": safety_backup,
        "deleteSafetyBackups": delete_safety_backups,
    })
}

fn settings_revision(farm: &Farm) -> i64 {
    query_i64(farm, "SELECT revision FROM settings")
}

// --- tier (a) -----------------------------------------------------------------------------

/// Changes every settings field away from its default, and sets the Slicer
/// runtime and a Printer's alert defaults (which tier (a) keeps).
fn customize_settings(farm: &Farm) {
    sql(
        farm,
        &format!(
            "UPDATE settings SET revision = revision + 1, theme_mode = 'farm3d-dark',
               monitor_section = 'location', monitor_density = 'compact',
               notify_fatal = 0, notify_confirmation = 0, notify_completion = 0,
               notify_reconciliation = 1, notify_connectivity = 1, notify_inventory = 1,
               snapshot_retention_days = 7, snapshot_disk_cap_mb = 512;
             INSERT INTO printer_alert_defaults(printer_id, revision, offline_after_minutes,
               notifications, snapshot_on_incident, snapshot_on_completion, updated_at)
               VALUES ('{printer}', 3, 5, 'muted', 0, 1, '{NOW}');",
            printer = ids::PRINTER_A,
        ),
    );
}

fn slicer_runtime(farm: &Farm) -> (i64, Option<String>, Option<String>, String) {
    farm.storage
        .read(|connection| {
            connection.query_row(
                "SELECT revision, engine_path, preset_source_path, updated_at
                   FROM slicer_runtime_config",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
        })
        .unwrap()
}

fn alert_defaults(farm: &Farm) -> Vec<(String, i64, Option<i64>, String, i64, i64, String)> {
    farm.storage
        .read(|connection| {
            connection
                .prepare(
                    "SELECT printer_id, revision, offline_after_minutes, notifications,
                            snapshot_on_incident, snapshot_on_completion, updated_at
                       FROM printer_alert_defaults ORDER BY printer_id",
                )?
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                })?
                .collect()
        })
        .unwrap()
}

#[test]
fn settings_reset_applies_the_defaults_after_a_pre_reset_snapshot() {
    let farm = Farm::with_every_domain();
    customize_settings(&farm);
    let slicer = slicer_runtime(&farm);
    let alerts = alert_defaults(&farm);
    let rig = Rig::new(&farm);
    let releases = rig.releases();
    let pokes = rig.services.cameras.janitor().pokes();
    let revision = settings_revision(&farm);
    assert!(pre_import_snapshots(farm.paths()).is_empty());

    let result = rig
        .reset("op-settings", settings_request(revision), "reset settings")
        .unwrap();
    assert_eq!(result["tier"], "settings");
    let settings = &result["settings"];
    assert_eq!(settings["revision"], json!(revision + 1));
    assert_eq!(settings["themeMode"], "system");
    assert_eq!(settings["monitorSection"], "printerModel");
    assert_eq!(settings["monitorDensity"], "comfortable");
    assert_eq!(
        settings["notifications"],
        json!({
            "fatal": true, "confirmation": true, "completion": true,
            "reconciliation": false, "connectivity": false, "inventory": false
        })
    );
    assert_eq!(
        settings["snapshotRetention"],
        json!({ "retentionDays": 30, "diskCapMb": 2048 })
    );

    // F1's pre-import snapshot of the settings as they were.
    let snapshots = pre_import_snapshots(farm.paths());
    assert_eq!(snapshots.len(), 1, "{snapshots:?}");
    let before = Connection::open(farm.paths().snapshot_root().join(&snapshots[0])).unwrap();
    let theme: String = before
        .query_row("SELECT theme_mode FROM settings", [], |row| row.get(0))
        .unwrap();
    assert_eq!(theme, "farm3d-dark");

    // Exactly the settings row: the Slicer runtime and alert defaults stay.
    assert_eq!(slicer_runtime(&farm), slicer);
    assert_eq!(alert_defaults(&farm), alerts);
    assert_eq!(
        query_text(
            &farm,
            "SELECT kind FROM operations WHERE id = 'op-settings'"
        )
        .as_deref(),
        Some("resetSettings")
    );
    // The retention changed, so the janitor was poked; the lease was held.
    assert!(rig.services.cameras.janitor().pokes() > pokes);
    assert_eq!(
        releases.load(Ordering::SeqCst),
        1,
        "the lease was taken and released"
    );
    assert_eq!(rig.services.backup.lease.holder(), None);

    // A replay returns the current record with no side effect.
    let replay = rig
        .reset("op-settings", settings_request(revision), "reset settings")
        .unwrap();
    assert_eq!(replay, result);
    assert_eq!(pre_import_snapshots(farm.paths()).len(), 1);
    assert_eq!(settings_revision(&farm), revision + 1);
    // The same id for another request is VALIDATION.
    let reused = rig
        .reset(
            "op-settings",
            settings_request(revision + 1),
            "reset settings",
        )
        .unwrap_err();
    assert_eq!(reused["code"], "VALIDATION");
    assert_eq!(reused["details"]["fieldPath"], "operationId");
}

#[test]
fn settings_reset_with_a_stale_revision_is_conflict() {
    let farm = Farm::with_every_domain();
    customize_settings(&farm);
    let rig = Rig::new(&farm);
    let revision = settings_revision(&farm);
    let error = rig
        .reset("op-stale", settings_request(revision - 1), "reset settings")
        .unwrap_err();
    assert_eq!(error["code"], "CONFLICT", "{error}");
    assert_eq!(settings_revision(&farm), revision);
    assert_eq!(
        query_text(&farm, "SELECT theme_mode FROM settings").as_deref(),
        Some("farm3d-dark")
    );
    assert!(
        pre_import_snapshots(farm.paths()).is_empty(),
        "no snapshot for a refusal"
    );
    assert_eq!(
        query_i64(
            &farm,
            "SELECT count(*) FROM operations WHERE id = 'op-stale'"
        ),
        0
    );
}

#[test]
fn every_tier_needs_its_exact_phrase() {
    let farm = Farm::with_every_domain();
    let rig = Rig::new(&farm);
    let revision = settings_revision(&farm);
    let cases = [
        (settings_request(revision), "reset settings"),
        (media_request("unpinned"), "reset media"),
        (farm_request(false, false), "reset farm"),
    ];
    for (request, phrase) in &cases {
        let mut wrong: Vec<String> = vec![
            phrase.to_uppercase(),
            format!(" {phrase}"),
            format!("{phrase} "),
            String::new(),
        ];
        wrong.extend(
            cases
                .iter()
                .map(|(_, other)| other.to_string())
                .filter(|other| other != phrase),
        );
        for confirmation in wrong {
            let error = rig
                .reset("op-phrase", request.clone(), &confirmation)
                .unwrap_err();
            assert_eq!(error["code"], "CONFIRMATION_MISMATCH", "{confirmation:?}");
            assert_eq!(error["details"], json!({ "expected": phrase }));
            assert_eq!(error["recovery"], json!(["EDIT_FIELDS"]));
        }
    }
    assert!(pre_import_snapshots(farm.paths()).is_empty());
    assert!(!journal::journal_path(farm.paths()).exists());
    assert_eq!(settings_revision(&farm), revision);
    assert_eq!(rig.restarter.requests(), 0);
}

// --- tier (b) -----------------------------------------------------------------------------

/// The every-domain Farm's raw-SQL Incident carries `printer_snapshot_json
/// '{}'`; a prune publishes the Incident it touched, so give it a Printer
/// snapshot the repository can read.
fn decodable_incident(farm: &Farm) {
    use farm3d_lib::catalog::{BedShape, PrinterProfile};
    let snapshot = farm3d_lib::jobs::PrinterSnapshot {
        name: "Printer A".to_string(),
        location: Some("Bay A".to_string()),
        catalog_ref: None,
        adapter_kind: None,
        profile: PrinterProfile {
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
            gcode_flavor: "marlin".to_string(),
            has_auxiliary_fan: false,
            supports_air_filtration: false,
            supports_multi_filament: false,
            suggested_host_type: None,
        },
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    farm.storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            tx.execute("UPDATE incidents SET printer_snapshot_json = ?1", [&json])
                .unwrap();
            Ok(())
        })
        .unwrap();
}

type SnapshotRow = (String, Option<String>, Option<String>, Option<String>, i64);

/// `(id, pinned_at, pruned_at, prune_reason, revision)` of every snapshot.
fn snapshot_rows(farm: &Farm) -> Vec<SnapshotRow> {
    farm.storage
        .read(|connection| {
            connection
                .prepare(
                    "SELECT id, pinned_at, pruned_at, prune_reason, revision
                       FROM camera_snapshots ORDER BY id",
                )?
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?
                .collect()
        })
        .unwrap()
}

/// Every `incident_events` row, as `(id, kind, snapshot_id, detail_json)`.
fn incident_timeline(farm: &Farm) -> Vec<(String, String, Option<String>, String)> {
    farm.storage
        .read(|connection| {
            connection
                .prepare(
                    "SELECT id, kind, snapshot_id, detail_json FROM incident_events
                      ORDER BY incident_id, sequence",
                )?
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect()
        })
        .unwrap()
}

fn snapshot_file(farm: &Farm, id: &str) -> PathBuf {
    farm.media_file(&p9_farm::snapshot_rel_path(id))
}

fn assert_integrity_clean(farm: &Farm) {
    let report = farm
        .storage
        .read(|connection| {
            Ok(
                integrity::check(connection, Some(&IntegrityRoots::from_paths(farm.paths())))
                    .unwrap(),
            )
        })
        .unwrap();
    assert!(report.violations().is_empty(), "{report:?}");
}

fn byte_len(farm: &Farm, id: &str) -> i64 {
    query_i64(
        farm,
        &format!("SELECT byte_len FROM camera_snapshots WHERE id = '{id}'"),
    )
}

#[test]
fn media_reset_prunes_the_unpinned_images_and_keeps_every_row_and_timeline_entry() {
    let farm = Farm::with_every_domain();
    decodable_incident(&farm);
    let unpinned = [ids::SNAPSHOT_INCIDENT, ids::SNAPSHOT_MANUAL];
    for id in unpinned.iter().chain([&ids::SNAPSHOT_PINNED]) {
        assert!(snapshot_file(&farm, id).is_file(), "{id}");
    }
    let rows_before = snapshot_rows(&farm);
    let timeline_before = incident_timeline(&farm);
    let counts_before = farm.counts();
    let rig = Rig::new(&farm);
    let releases = rig.releases();

    let result = rig
        .reset("op-media", media_request("unpinned"), "reset media")
        .unwrap();
    assert_eq!(result["tier"], "cameraMedia");
    assert_eq!(result["prunedCount"], json!(2));
    assert_eq!(
        result["freedBytes"],
        json!(unpinned.iter().map(|id| byte_len(&farm, id)).sum::<i64>())
    );
    assert_eq!(
        releases.load(Ordering::SeqCst),
        1,
        "the lease was taken and released"
    );

    // The rows stay; only the selected ones are pruned `reset`.
    let rows = snapshot_rows(&farm);
    assert_eq!(rows.len(), rows_before.len());
    for (row, before) in rows.iter().zip(&rows_before) {
        if unpinned.contains(&row.0.as_str()) {
            assert!(row.2.is_some(), "{row:?}");
            assert_eq!(row.3.as_deref(), Some("reset"));
            assert_eq!(row.4, before.4 + 1);
            assert!(!snapshot_file(&farm, &row.0).exists(), "{} unlinked", row.0);
        } else {
            assert_eq!(row, before, "untouched");
        }
    }
    assert!(snapshot_file(&farm, ids::SNAPSHOT_PINNED).is_file());

    // Every earlier timeline entry stays; the Incident-linked one gains
    // `evidencePruned { reason: "reset" }`.
    let timeline = incident_timeline(&farm);
    assert_eq!(&timeline[..timeline_before.len()], &timeline_before[..]);
    let added = &timeline[timeline_before.len()..];
    assert_eq!(added.len(), 1, "{added:?}");
    assert_eq!(added[0].1, "evidencePruned");
    assert_eq!(added[0].2.as_deref(), Some(ids::SNAPSHOT_INCIDENT));
    let detail: Value = serde_json::from_str(&added[0].3).unwrap();
    assert_eq!(detail["reason"], "reset");

    // Only the media changed: every other table's count is the same.
    // (Dropping the lease retries P4's deferred blob cleanup, D5, which
    // clears the seeded `pending_blob_cleanup` row whose file is gone.)
    let counts = farm.counts();
    for (table, rows) in &counts {
        if table == "pending_blob_cleanup" {
            continue;
        }
        let expected = match table.as_str() {
            "incident_events" => counts_before[table] + 1,
            "operations" => counts_before[table] + 1,
            _ => counts_before[table],
        };
        assert_eq!(*rows, expected, "{table}");
    }
    assert_integrity_clean(&farm);

    // A replay prunes nothing.
    let replay = rig
        .reset("op-media", media_request("unpinned"), "reset media")
        .unwrap();
    assert_eq!(
        replay,
        json!({ "tier": "cameraMedia", "prunedCount": 0, "freedBytes": 0 })
    );
    assert_eq!(snapshot_rows(&farm), rows);
}

#[test]
fn media_reset_all_prunes_pinned_images_too_and_keeps_pinned_at() {
    let farm = Farm::with_every_domain();
    decodable_incident(&farm);
    let pinned_before = snapshot_rows(&farm)
        .into_iter()
        .find(|row| row.0 == ids::SNAPSHOT_PINNED)
        .unwrap();
    assert!(pinned_before.1.is_some());
    let rig = Rig::new(&farm);
    let result = rig
        .reset("op-all", media_request("all"), "reset media")
        .unwrap();
    assert_eq!(result["prunedCount"], json!(3));
    let pinned = snapshot_rows(&farm)
        .into_iter()
        .find(|row| row.0 == ids::SNAPSHOT_PINNED)
        .unwrap();
    assert_eq!(pinned.1, pinned_before.1, "pinned_at is kept");
    assert_eq!(pinned.3.as_deref(), Some("reset"));
    for id in [
        ids::SNAPSHOT_INCIDENT,
        ids::SNAPSHOT_MANUAL,
        ids::SNAPSHOT_PINNED,
    ] {
        assert!(!snapshot_file(&farm, id).exists(), "{id}");
    }
    // The already-pruned row keeps its own reason.
    let pruned = snapshot_rows(&farm)
        .into_iter()
        .find(|row| row.0 == ids::SNAPSHOT_PRUNED)
        .unwrap();
    assert_eq!(pruned.3.as_deref(), Some("age"));
    assert_integrity_clean(&farm);
}

/// Tier (b) holds the lease: its files wait in the lease's queue and are
/// unlinked only when it drops.
#[test]
fn media_reset_unlinks_its_files_when_the_lease_drops() {
    let farm = Farm::with_every_domain();
    decodable_incident(&farm);
    let janitor = MediaJanitor::default();
    let guard = janitor
        .backup_lease()
        .try_acquire(LeaseActivity::Reset)
        .unwrap();
    let pruned = reset::reset_camera_media(
        &farm.storage,
        "op-queued",
        ResetMediaScope::Unpinned,
        Utc::now(),
    )
    .unwrap();
    assert_eq!(pruned.pruned_count, 2);
    camera_media::queue_unlinks(&farm.storage, &janitor, &pruned.rel_paths);
    for id in [ids::SNAPSHOT_INCIDENT, ids::SNAPSHOT_MANUAL] {
        assert!(
            snapshot_file(&farm, id).is_file(),
            "{id} waits for the lease"
        );
    }
    drop(guard);
    for id in [ids::SNAPSHOT_INCIDENT, ids::SNAPSHOT_MANUAL] {
        assert!(!snapshot_file(&farm, id).exists(), "{id} unlinked");
    }
}

// --- tier (c): the command ----------------------------------------------------------------

#[test]
fn every_tier_refuses_restart_pending_before_the_lease_and_any_snapshot() {
    let farm = Farm::with_every_domain();
    let rig = Rig::new(&farm);
    let first = rig
        .reset("op-farm", farm_request(false, false), "reset farm")
        .unwrap();
    assert_eq!(
        first,
        json!({ "tier": "farm", "status": "restarting", "safetyBackupId": null })
    );
    // The lease is kept until the restart.
    assert_eq!(
        rig.services.backup.lease.holder(),
        Some(LeaseActivity::Reset)
    );
    let path = journal::journal_path(farm.paths());
    let bytes = fs::read(&path).unwrap();
    let journal_id = journal::read(farm.paths()).unwrap().unwrap().id;
    let revision = settings_revision(&farm);
    for (operation_id, request, phrase) in [
        ("op-a", settings_request(revision), "reset settings"),
        ("op-b", media_request("all"), "reset media"),
        ("op-c", farm_request(true, true), "reset farm"),
    ] {
        let error = rig.reset(operation_id, request, phrase).unwrap_err();
        assert_eq!(error["code"], "RESTART_PENDING", "{operation_id}: {error}");
        assert_eq!(
            error["details"],
            json!({ "journalId": journal_id, "kind": "reset" })
        );
        assert_eq!(error["recovery"], json!(["RESTART_APPLICATION"]));
    }
    assert_eq!(fs::read(&path).unwrap(), bytes, "the journal is unchanged");
    assert!(pre_import_snapshots(farm.paths()).is_empty());
    assert!(snapshot_file(&farm, ids::SNAPSHOT_INCIDENT).is_file());
    assert_eq!(settings_revision(&farm), revision);
    assert!(names_in(&safety_dir(farm.paths())).is_empty());
}

#[test]
fn every_tier_is_refused_while_another_operation_holds_the_lease() {
    let farm = Farm::with_every_domain();
    let rig = Rig::new(&farm);
    let guard = rig
        .services
        .backup
        .lease
        .try_acquire(LeaseActivity::Backup)
        .unwrap();
    let revision = settings_revision(&farm);
    for (operation_id, request, phrase) in [
        ("op-a", settings_request(revision), "reset settings"),
        ("op-b", media_request("all"), "reset media"),
        ("op-c", farm_request(true, false), "reset farm"),
    ] {
        let error = rig.reset(operation_id, request, phrase).unwrap_err();
        assert_eq!(error["code"], "BACKUP_IN_PROGRESS", "{operation_id}");
        assert_eq!(error["details"], json!({ "activity": "backup" }));
    }
    drop(guard);
    assert!(pre_import_snapshots(farm.paths()).is_empty());
    assert!(!journal::journal_path(farm.paths()).exists());
    assert!(snapshot_file(&farm, ids::SNAPSHOT_INCIDENT).is_file());
}

fn class_of<'a>(preview: &'a Value, class: &str) -> &'a Value {
    preview["classes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["class"] == class)
        .unwrap_or_else(|| panic!("no class {class} in {preview}"))
}

fn classes(preview: &Value) -> Vec<(String, String)> {
    preview["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["class"].as_str().unwrap().to_string(),
                entry["effect"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn tree_bytes(root: &Path) -> (i64, i64) {
    let mut total = (0, 0);
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let metadata = fs::symlink_metadata(entry.path()).unwrap();
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                total.0 += 1;
                total.1 += metadata.len() as i64;
            }
        }
    }
    total
}

#[test]
fn the_preview_names_every_class_with_counts_and_bytes_and_warns_of_active_work() {
    let farm = Farm::with_every_domain();
    seed_farm_extras(&farm, None);
    let rig = Rig::new(&farm);

    let settings = rig.preview("settings");
    assert_eq!(settings["tier"], "settings");
    assert_eq!(settings["phrase"], "reset settings");
    assert_eq!(
        classes(&settings),
        [
            ("settings", "reset"),
            ("slicerRuntime", "kept"),
            ("printerAlertDefaults", "kept")
        ]
        .map(|(class, effect)| (class.to_string(), effect.to_string()))
    );
    assert_eq!(settings["warnings"], json!([]));

    let media = rig.preview("cameraMedia");
    assert_eq!(media["phrase"], "reset media");
    let unpinned = class_of(&media, "unpinnedSnapshots");
    assert_eq!(unpinned["effect"], "pruned");
    assert_eq!(unpinned["count"], json!(2));
    assert_eq!(
        unpinned["bytes"],
        json!(byte_len(&farm, ids::SNAPSHOT_INCIDENT) + byte_len(&farm, ids::SNAPSHOT_MANUAL))
    );
    assert_eq!(class_of(&media, "pinnedSnapshots")["count"], json!(1));
    assert_eq!(class_of(&media, "snapshotRecords")["effect"], "kept");
    assert_eq!(class_of(&media, "snapshotRecords")["count"], json!(4));

    let preview = rig.preview("farm");
    assert_eq!(preview["tier"], "farm");
    assert_eq!(preview["phrase"], "reset farm");
    let expected: Vec<(String, String)> = [
        ("settings", "reset"),
        ("slicerRuntime", "deleted"),
        ("printerAlertDefaults", "deleted"),
        ("printers", "deleted"),
        ("spools", "deleted"),
        ("library", "deleted"),
        ("sliceRevisions", "deleted"),
        ("queueAndJobs", "deleted"),
        ("incidentsAndAttention", "deleted"),
        ("snapshotRecords", "deleted"),
        ("content", "deleted"),
        ("cameraMedia", "deleted"),
        ("logs", "deleted"),
        ("credentials", "deleted"),
        ("preImportSnapshots", "deleted"),
        ("restoreStaging", "deleted"),
        ("legacyArchives", "deleted"),
        ("safetyBackups", "kept"),
        ("slicerProfileCache", "kept"),
    ]
    .iter()
    .map(|(class, effect)| (class.to_string(), effect.to_string()))
    .collect();
    assert_eq!(classes(&preview), expected);
    for entry in preview["classes"].as_array().unwrap() {
        assert!(entry["count"].is_number(), "{entry}");
    }
    let paths = farm.paths();
    let files = |class: &str, root: &Path| {
        let (count, bytes) = tree_bytes(root);
        let entry = class_of(&preview, class);
        assert_eq!(entry["count"], json!(count), "{class}");
        assert_eq!(entry["bytes"], json!(bytes), "{class}");
        assert!(count > 0, "{class} is seeded");
    };
    files("content", &paths.content_root().join("blobs"));
    files("cameraMedia", &paths.media_root().join("snapshots"));
    files("logs", paths.log_root());
    files("legacyArchives", paths.legacy_root());
    files("safetyBackups", &safety_dir(paths));
    assert_eq!(class_of(&preview, "printers")["count"], json!(3));
    assert_eq!(class_of(&preview, "credentials")["count"], json!(3));
    assert!(class_of(&preview, "printers")["bytes"].is_null());
    let (count, bytes) = paths
        .snapshot_root()
        .read_dir()
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_name() != ".restore-staging")
        .fold((0_i64, 0_i64), |(count, bytes), entry| {
            (count + 1, bytes + entry.metadata().unwrap().len() as i64)
        });
    assert!(count >= 1, "the pre-import snapshot and its sidecars");
    assert_eq!(
        class_of(&preview, "preImportSnapshots")["count"],
        json!(count)
    );
    assert_eq!(
        class_of(&preview, "preImportSnapshots")["bytes"],
        json!(bytes)
    );
    assert_eq!(class_of(&preview, "restoreStaging")["count"], json!(3));
    // D7's blockers are a warning, not a refusal (`hop-a` is dispatching).
    assert_eq!(
        preview["warnings"],
        json!([{ "kind": "activeWork", "activeJobs": 0, "hostOperations": 1, "sliceOperations": 0 }])
    );
    let result = rig
        .reset("op-farm", farm_request(false, false), "reset farm")
        .unwrap();
    assert_eq!(result["status"], "restarting");
}

#[test]
fn reset_farm_writes_the_safety_backup_first_then_the_journal_and_restarts() {
    let farm = Farm::with_every_domain();
    let journal_path = journal::journal_path(farm.paths());
    let seen = Arc::new(Mutex::new(None));
    let hooks = WriterHooks {
        before_verify: Some(Box::new({
            let seen = Arc::clone(&seen);
            let checked = journal_path.clone();
            move |_| {
                *seen.lock().unwrap() = Some(checked.exists());
            }
        })),
        ..WriterHooks::default()
    };
    let rig = Rig::with_hooks(&farm, hooks);
    let result = rig
        .reset("op-farm", farm_request(true, false), "reset farm")
        .unwrap();
    assert_eq!(result["tier"], "farm");
    assert_eq!(result["status"], "restarting");
    let safety_backup_id = result["safetyBackupId"].as_str().unwrap().to_string();
    assert_eq!(
        *seen.lock().unwrap(),
        Some(false),
        "the safety backup was written before the journal"
    );
    let safety = safety_dir(farm.paths()).join(safety_file(&safety_backup_id));
    let manifest = archive::open(&safety).unwrap().manifest().clone();
    assert_eq!(manifest.origin.as_str(), "beforeReset");
    assert_eq!(rig.restarter.requests(), 0, "not before the response");
    assert!(rig.restarter.wait_for(1, Duration::from_secs(5)));

    let journal = journal::read(farm.paths()).unwrap().unwrap();
    assert_eq!(journal.kind, RestoreJournalKind::Reset);
    assert!(journal.id.starts_with("rsf-"));
    assert_eq!(journal.phase, JournalPhase::Pending);
    assert_eq!(
        journal.safety_backup_id.as_deref(),
        Some(safety_backup_id.as_str())
    );
    assert_eq!(
        journal.orphan_credential_refs,
        vec!["cred-a", ids::CREDENTIAL_REF_A, ids::CREDENTIAL_REF_B]
    );
    assert_eq!(
        journal.reset.map(|options| options.delete_safety_backups),
        Some(false)
    );
    assert_eq!(journal.staging_id, None);
    assert_eq!(journal.carry, None);
    // Refs only, never a value.
    let bytes = fs::read_to_string(&journal_path).unwrap();
    for secret in stored_secrets(&farm) {
        assert!(!bytes.contains(&secret));
    }

    // A replay returns the cached result without a second safety backup;
    // the same id for another request is VALIDATION.
    assert_eq!(
        rig.reset("op-farm", farm_request(true, false), "reset farm")
            .unwrap(),
        result
    );
    assert_eq!(names_in(&safety_dir(farm.paths())).len(), 1);
    let reused = rig
        .reset("op-farm", farm_request(true, true), "reset farm")
        .unwrap_err();
    assert_eq!(reused["code"], "VALIDATION");
    assert_eq!(reused["details"]["fieldPath"], "operationId");
}

// --- tier (c): the installer ------------------------------------------------------------

/// What tier (c) clears and keeps beyond the Farm's own rows and files: a
/// pre-import snapshot, the three restore staging directories, an F0
/// legacy archive, a log file, two older safety backups, and (when named)
/// this reset's safety backup.
fn seed_farm_extras(farm: &Farm, this_safety: Option<&str>) {
    let paths = farm.paths();
    farm.storage
        .create_snapshot(SnapshotKind::Settings)
        .unwrap();
    let staging = "stg-00000000-0000-4000-8000-000000000001";
    let write = |path: PathBuf, bytes: &[u8]| {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    };
    write(
        paths
            .snapshot_root()
            .join(".restore-staging")
            .join(staging)
            .join("candidate.sqlite3"),
        b"staged candidate",
    );
    write(
        paths
            .content_root()
            .join("staging")
            .join(format!("restore-{staging}"))
            .join("aa/aa00"),
        b"staged blob",
    );
    write(
        paths
            .media_root()
            .join(format!("restore-{staging}"))
            .join("snapshots/2026/01/snp-x.jpg"),
        b"staged image",
    );
    write(
        paths.legacy_root().join("printers-migrated-1.json"),
        b"{\"schemaVersion\":1,\"printers\":[]}",
    );
    write(paths.log_root().join("farm3d.log"), b"{\"code\":\"x\"}\n");
    for id in OLDER_SAFETY.iter().copied().chain(this_safety) {
        write(safety_dir(paths).join(safety_file(id)), id.as_bytes());
    }
}

/// A Farm with a `pending` reset journal, closed as the process that wrote
/// it would be before the restart.
struct Site {
    temp: tempfile::TempDir,
    paths: StoragePaths,
    lease: MetadataRootLease,
    credentials_dir: PathBuf,
    journal_id: String,
    refs: Vec<String>,
    safety_backup_id: Option<String>,
    fresh_counts: BTreeMap<String, i64>,
}

fn site(delete_safety_backups: bool) -> Site {
    let farm = Farm::with_every_domain();
    seed_farm_extras(&farm, Some(THIS_SAFETY));
    // Uncheckpointed frames, so the database set is three files.
    let journal = reset::write_farm_journal(
        &farm.storage,
        Some(THIS_SAFETY.to_string()),
        delete_safety_backups,
        created_at(),
    )
    .unwrap();
    let Farm {
        temp,
        metadata_lease,
        storage,
        credentials_dir,
    } = farm;
    let paths = storage.paths().clone();
    drop(
        Arc::try_unwrap(storage)
            .ok()
            .expect("the only Storage handle"),
    );
    add_wal_frames(&paths);
    Site {
        fresh_counts: fresh_counts(),
        temp,
        paths,
        lease: metadata_lease,
        credentials_dir,
        journal_id: journal.id,
        refs: journal.orphan_credential_refs,
        safety_backup_id: Some(THIS_SAFETY.to_string()),
    }
}

/// Every table's count in a newly migrated database.
fn fresh_counts() -> BTreeMap<String, i64> {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("m"), temp.path().join("d")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Storage::open(paths.clone(), &lease).unwrap();
    drop(storage);
    p9_farm::table_counts(&Connection::open(paths.database()).unwrap())
}

fn add_wal_frames(paths: &StoragePaths) {
    let connection = Connection::open(paths.database()).unwrap();
    connection
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .unwrap();
    connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .unwrap();
    connection
        .execute_batch(&format!(
            "INSERT INTO library_projects(id, revision, name, created_at, updated_at)
               VALUES ('prj-wal', 1, 'Only in the WAL', '{NOW}', '{NOW}');"
        ))
        .unwrap();
    drop(connection);
    assert!(
        fs::metadata(paths.metadata_root().join("farm3d.sqlite3-wal"))
            .unwrap()
            .len()
            > 0
    );
}

fn store(site: &Site) -> CredentialStore {
    CredentialStore::file_backed(site.credentials_dir.clone())
}

/// A store every operation of which fails: its file doesn't parse.
fn unavailable_store(site: &Site) -> CredentialStore {
    let directory = site.temp.path().join("broken-store");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("credentials.json"), b"not json").unwrap();
    CredentialStore::file_backed(directory)
}

fn crash_at(step: InstallerStep, point: FaultPoint) -> Fault {
    Fault {
        step,
        point,
        effect: FaultEffect::Crash,
        times: 1,
    }
}

impl Site {
    fn journal(&self) -> RestoreJournal {
        journal::read(&self.paths).unwrap().expect("a journal")
    }

    fn run_with(
        &self,
        faults: Vec<Fault>,
        open: impl FnOnce() -> CredentialStore,
    ) -> Result<InstallReport, InstallerError> {
        installer::run_with_faults(&self.paths, &self.lease, open, &Faults::new(faults))
    }

    fn run(&self, faults: Vec<Fault>) -> Result<InstallReport, InstallerError> {
        self.run_with(faults, || store(self))
    }

    fn crash(&self, faults: Vec<Fault>) {
        match self.run(faults) {
            Err(InstallerError::Crashed) => {}
            other => panic!("expected the injected crash, got {other:?}"),
        }
    }

    fn run_clean(&self) -> InstallReport {
        let report = self.run(vec![]).expect("the installer ran");
        assert_eq!(report.outcome, InstallOutcome::Installed, "{report:?}");
        assert_eq!(report.kind, Some(RestoreJournalKind::Reset));
        report
    }

    fn at_fault(&self, step: InstallerStep) {
        let journal = self.journal();
        assert_eq!(journal.phase, JournalPhase::Installing);
        assert_eq!(journal.step, Some(step));
    }

    fn roots(&self) -> [&Path; 3] {
        [
            self.paths.content_root(),
            self.paths.media_root(),
            self.paths.log_root(),
        ]
    }

    fn aside(&self, root: &Path) -> PathBuf {
        root.parent().unwrap().join(format!(
            ".aside-{}-{}",
            self.journal_id,
            root.file_name().unwrap().to_string_lossy()
        ))
    }

    fn pending_cleanup(&self) -> Vec<(String, Option<String>, String)> {
        Connection::open(self.paths.database())
            .unwrap()
            .prepare(
                "SELECT credential_ref, printer_id, reason FROM pending_credential_cleanup
                  ORDER BY credential_ref",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    /// "Reset done" (spec), then the fresh Farm opens at the current schema,
    /// empty and clean.
    fn assert_reset_done(&self, safety: &[&str]) {
        let journal = self.journal();
        assert_eq!(journal.phase, JournalPhase::Done, "{journal:?}");
        assert_eq!(journal.kind, RestoreJournalKind::Reset);
        assert!(journal.outcome.is_some());
        assert_eq!(journal.step, Some(InstallerStep::MarkDone));

        // The roots exist and are empty; nothing is left aside.
        for root in self.roots() {
            assert!(root.is_dir(), "{} exists", root.display());
            assert!(names_in(root).is_empty(), "{} is empty", root.display());
            let parent = root.parent().unwrap();
            assert!(
                !names_in(parent)
                    .iter()
                    .any(|name| name.starts_with(".aside-")),
                "no aside beside {}",
                root.display()
            );
        }
        for root in [self.paths.snapshot_root(), self.paths.legacy_root()] {
            assert!(names_in(root).is_empty(), "{} is empty", root.display());
        }
        assert!(!journal::previous_dir(&self.paths, &self.journal_id).exists());

        // Every journaled ref is gone from the store or queued `reset`.
        let queued: BTreeMap<String, (Option<String>, String)> = self
            .pending_cleanup()
            .into_iter()
            .map(|(reference, printer, reason)| (reference, (printer, reason)))
            .collect();
        let store = store(self);
        for reference in &self.refs {
            match queued.get(reference) {
                Some((printer, reason)) => {
                    assert_eq!(reason, "reset", "{reference}");
                    assert_eq!(printer, &None);
                }
                None => assert!(
                    !store.contains(reference).unwrap(),
                    "{reference} was deleted"
                ),
            }
        }
        let refs: BTreeSet<&String> = self.refs.iter().collect();
        assert!(queued.keys().all(|reference| refs.contains(reference)));

        // `safety/`.
        let expected: Vec<String> = safety.iter().map(|id| safety_file(id)).collect();
        let mut expected = expected;
        expected.sort();
        assert_eq!(names_in(&safety_dir(&self.paths)), expected);

        // The fresh database: current schema, no domain rows.
        let connection = Connection::open(self.paths.database()).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        let counts = p9_farm::table_counts(&connection);
        for (table, rows) in &counts {
            if table == "pending_credential_cleanup" {
                continue;
            }
            assert_eq!(Some(rows), self.fresh_counts.get(table), "{table}");
        }
        assert_eq!(
            counts.keys().collect::<Vec<_>>(),
            self.fresh_counts.keys().collect::<Vec<_>>()
        );
        let report =
            integrity::check(&connection, Some(&IntegrityRoots::from_paths(&self.paths))).unwrap();
        assert!(report.violations().is_empty(), "{report:?}");
        drop(connection);

        // It opens as the next start would, empty and clean.
        let storage = Arc::new(Storage::open(self.paths.clone(), &self.lease).unwrap());
        farm3d_lib::settings::repository::SettingsRepository::new(Arc::clone(&storage))
            .ensure_default()
            .unwrap();
        farm3d_lib::library::content::ContentStore::open(self.paths.content_root())
            .unwrap()
            .startup_sweep(&storage)
            .unwrap();
        camera_media::startup_sweep(&storage, Utc::now()).unwrap();
        let printers: i64 = storage
            .read(|connection| {
                connection.query_row("SELECT count(*) FROM printers", [], |row| row.get(0))
            })
            .unwrap();
        assert_eq!(printers, 0);
        let report = storage
            .read(|connection| {
                Ok(
                    integrity::check(connection, Some(&IntegrityRoots::from_paths(&self.paths)))
                        .unwrap(),
                )
            })
            .unwrap();
        assert!(report.violations().is_empty(), "{report:?}");
    }

    fn older_and_this(&self) -> Vec<&'static str> {
        let mut all: Vec<&str> = OLDER_SAFETY.to_vec();
        all.push(THIS_SAFETY);
        all
    }
}

#[test]
fn r1_a_pending_reset_journal_runs_from_mark_installing() {
    let farm = Farm::with_every_domain();
    seed_farm_extras(&farm, None);
    let rig = Rig::new(&farm);
    let result = rig
        .reset("op-r1", farm_request(true, false), "reset farm")
        .unwrap();
    let safety_backup_id = result["safetyBackupId"].as_str().unwrap().to_string();
    assert!(rig.restarter.wait_for(1, Duration::from_secs(5)));
    drop(rig);
    let journal = journal::read(farm.paths()).unwrap().unwrap();
    assert_eq!(journal.phase, JournalPhase::Pending);
    assert_eq!(journal.step, None);

    // The restart.
    let Farm {
        temp,
        metadata_lease,
        storage,
        credentials_dir,
    } = farm;
    let paths = storage.paths().clone();
    // The rig's runtime may still hold a handle; the restart is simulated
    // by the installer running under the same metadata-root lease.
    drop(storage);
    let site = Site {
        fresh_counts: fresh_counts(),
        temp,
        paths,
        lease: metadata_lease,
        credentials_dir,
        journal_id: journal.id.clone(),
        refs: journal.orphan_credential_refs.clone(),
        safety_backup_id: Some(safety_backup_id.clone()),
    };
    site.run_clean();
    let mut kept: Vec<&str> = OLDER_SAFETY.to_vec();
    kept.push(&safety_backup_id);
    site.assert_reset_done(&kept);
    assert!(site.pending_cleanup().is_empty(), "every ref was deleted");
}

#[test]
fn r2_a_partial_database_move_moves_the_rest() {
    let site = site(false);
    site.crash(vec![crash_at(
        InstallerStep::MoveDatabaseAside,
        FaultPoint::Within(0),
    )]);
    site.at_fault(InstallerStep::MoveDatabaseAside);
    let previous = journal::previous_dir(&site.paths, &site.journal_id);
    assert!(previous.join("farm3d.sqlite3").is_file());
    assert!(site
        .paths
        .metadata_root()
        .join("farm3d.sqlite3-wal")
        .is_file());
    site.run_clean();
    site.assert_reset_done(&site.older_and_this());
}

#[test]
fn r3_a_partial_roots_move_skips_the_moved_root_and_moves_the_rest() {
    let site = site(false);
    site.crash(vec![crash_at(
        InstallerStep::MoveRootsAside,
        FaultPoint::Within(0),
    )]);
    site.at_fault(InstallerStep::MoveRootsAside);
    let [content, media, log] = site.roots();
    assert!(site.aside(content).is_dir(), "content_root moved");
    assert!(names_in(content).is_empty(), "and recreated empty");
    assert!(!site.aside(media).exists() && !names_in(media).is_empty());
    assert!(!site.aside(log).exists() && !names_in(log).is_empty());
    // The next start's `StoragePaths` recreates what is missing; the
    // installer must leave the recreated root alone.
    let reopened =
        StoragePaths::new(site.paths.metadata_root(), site.temp.path().join("data")).unwrap();
    assert_eq!(reopened.content_root(), content);
    site.run_clean();
    site.assert_reset_done(&site.older_and_this());
}

/// Two moved roots under one parent (a `log_root` beside `content_root`):
/// each gets its own aside, so both move, including after a crash between
/// them.
#[test]
fn r3b_two_roots_under_one_parent_both_move() {
    let mut site = site(false);
    let parent = site.paths.content_root().parent().unwrap().to_path_buf();
    site.paths = site
        .paths
        .clone()
        .with_log_root(parent.join("logs"))
        .unwrap();
    fs::write(site.paths.log_root().join("farm3d.log"), b"old log").unwrap();
    site.crash(vec![crash_at(
        InstallerStep::MoveRootsAside,
        FaultPoint::Within(0),
    )]);
    site.at_fault(InstallerStep::MoveRootsAside);
    let [content, _, log] = site.roots();
    assert!(site.aside(content).is_dir(), "content_root moved");
    assert!(!names_in(log).is_empty(), "log_root not moved yet");
    site.run_clean();
    site.assert_reset_done(&site.older_and_this());
    assert!(!parent.join("logs").join("farm3d.log").exists());
}

#[test]
fn r4_a_partial_fresh_database_is_deleted_and_recreated() {
    let site = site(false);
    site.crash(vec![crash_at(
        InstallerStep::CreateFreshDatabase,
        FaultPoint::Within(0),
    )]);
    site.at_fault(InstallerStep::CreateFreshDatabase);
    assert!(site.paths.database().is_file(), "the partial set");
    site.run_clean();
    site.assert_reset_done(&site.older_and_this());
}

#[test]
fn r5_a_crash_after_half_the_refs_deletes_the_rest() {
    let site = site(false);
    assert_eq!(site.refs.len(), 3);
    let store_before = store(&site);
    assert!(store_before.contains(ids::CREDENTIAL_REF_A).unwrap());
    site.crash(vec![crash_at(
        InstallerStep::DeleteCredentials,
        FaultPoint::Within(0),
    )]);
    site.at_fault(InstallerStep::DeleteCredentials);
    // The first ref (`cred-a`, absent from the store) is done; the Printer
    // refs are still there.
    assert!(store(&site).contains(ids::CREDENTIAL_REF_A).unwrap());
    site.run_clean();
    site.assert_reset_done(&site.older_and_this());
    let store = store(&site);
    for reference in &site.refs {
        assert!(!store.contains(reference).unwrap(), "{reference} gone");
    }
    assert!(site.pending_cleanup().is_empty());
}

#[test]
fn r6_an_unavailable_store_queues_every_ref_with_reason_reset() {
    let site = site(false);
    let report = site
        .run_with(vec![], || unavailable_store(&site))
        .expect("the step never fails for a credential");
    assert_eq!(report.outcome, InstallOutcome::Installed);
    let queued: Vec<String> = site
        .pending_cleanup()
        .into_iter()
        .map(|(reference, printer, reason)| {
            assert_eq!(reason, "reset");
            assert_eq!(printer, None);
            reference
        })
        .collect();
    assert_eq!(queued, site.refs);
    site.assert_reset_done(&site.older_and_this());
    // The values are still in the real store, for F1's startup retry.
    assert!(store(&site).contains(ids::CREDENTIAL_REF_A).unwrap());
}

#[test]
fn r7_a_half_done_safety_backup_delete_finishes_and_keeps_this_resets_backup() {
    let site = site(true);
    site.crash(vec![crash_at(
        InstallerStep::DeleteSafetyBackups,
        FaultPoint::Within(0),
    )]);
    site.at_fault(InstallerStep::DeleteSafetyBackups);
    assert_eq!(names_in(&safety_dir(&site.paths)).len(), 2, "one deleted");
    site.run_clean();
    site.assert_reset_done(&[THIS_SAFETY]);
}

#[test]
fn r8_a_half_done_remove_previous_finishes() {
    let site = site(false);
    site.crash(vec![crash_at(
        InstallerStep::RemovePrevious,
        FaultPoint::Within(0),
    )]);
    site.at_fault(InstallerStep::RemovePrevious);
    assert!(!journal::previous_dir(&site.paths, &site.journal_id).exists());
    assert!(site.aside(site.paths.content_root()).is_dir());
    site.run_clean();
    site.assert_reset_done(&site.older_and_this());
}

#[test]
fn r9_after_mark_done_nothing_runs_and_the_status_is_done_until_acknowledged() {
    let site = site(false);
    site.crash(vec![crash_at(InstallerStep::MarkDone, FaultPoint::End)]);
    assert_eq!(site.journal().phase, JournalPhase::Done);
    let report = site
        .run_with(vec![], || panic!("a finished reset opens no store"))
        .unwrap();
    assert_eq!(report.outcome, InstallOutcome::AlreadyFinished);
    site.assert_reset_done(&site.older_and_this());
    let status = serde_json::to_value(site.journal().status()).unwrap();
    assert_eq!(status["state"], "done");
    assert_eq!(status["kind"], "reset");
    assert_eq!(status["journalId"], json!(site.journal_id));
    assert_eq!(status["safetyBackupId"], json!(site.safety_backup_id));
    assert!(status["finishedAt"].is_string());
    // Acknowledged: the journal and its directory go.
    journal::remove(&site.paths, &site.journal_id).unwrap();
    assert!(journal::read(&site.paths).unwrap().is_none());
    assert!(!journal::journal_dir(&site.paths, &site.journal_id).exists());
}

#[test]
fn r10_an_io_error_at_move_roots_aside_twice_fails_startup_then_the_third_start_succeeds() {
    let site = site(false);
    let faults = Faults::new([Fault {
        step: InstallerStep::MoveRootsAside,
        point: FaultPoint::Within(0),
        effect: FaultEffect::Error(std::io::ErrorKind::Other),
        times: 2,
    }]);
    for _ in 0..2 {
        match installer::run_with_faults(&site.paths, &site.lease, || store(&site), &faults) {
            Err(error @ InstallerError::InstallFailed { .. }) => {
                let error = serde_json::to_value(error.to_command_error()).unwrap();
                assert_eq!(error["code"], "RESTORE_FAILED");
                assert_eq!(
                    error["details"],
                    json!({ "reason": "installFailed", "step": "moveRootsAside" })
                );
                assert_eq!(error["retryable"], true);
            }
            other => panic!("expected RESTORE_FAILED, got {other:?}"),
        }
        site.at_fault(InstallerStep::MoveRootsAside);
    }
    let report =
        installer::run_with_faults(&site.paths, &site.lease, || store(&site), &faults).unwrap();
    assert_eq!(report.outcome, InstallOutcome::Installed);
    site.assert_reset_done(&site.older_and_this());
}

/// Startup order: the installer runs the reset before the log starts and
/// before `Storage::open`, and opens the credential store once, after the
/// metadata-root lease.
#[test]
fn a_reset_at_startup_opens_the_credential_store_once_then_the_fresh_farm() {
    let site = site(true);
    let Site {
        temp: _temp,
        paths,
        lease,
        credentials_dir,
        ..
    } = site;
    drop(lease);
    let opened = AtomicUsize::new(0);
    let slot = Mutex::new(None);
    let (storage, report) = farm3d_lib::open_storage(paths.clone(), &slot, || {
        opened.fetch_add(1, Ordering::SeqCst);
        assert!(
            farm3d_lib::persistence::MetadataRootLease::acquire(&paths).is_err(),
            "the lease is held"
        );
        CredentialStore::file_backed(credentials_dir.clone())
    })
    .map_err(|_| "startup failed")
    .unwrap();
    assert_eq!(report.outcome, InstallOutcome::Installed);
    assert_eq!(report.kind, Some(RestoreJournalKind::Reset));
    assert_eq!(opened.load(Ordering::SeqCst), 1);
    let printers: i64 = storage
        .read(|connection| {
            connection.query_row("SELECT count(*) FROM printers", [], |row| row.get(0))
        })
        .unwrap();
    assert_eq!(printers, 0);
    assert_eq!(
        names_in(&safety_dir(&paths)),
        vec![safety_file(THIS_SAFETY)]
    );
}
