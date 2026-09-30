//! P9 Task 18: the end-to-end tracer (issue #19), on the fakes.
//!
//! One flow over the every-domain Farm, through the real commands and the
//! real startup path:
//!
//! 1. The Farm, with the secret corpus in credentials, URLs, headers, a
//!    driven error, and the log.
//! 2. `create_backup` with `media: all`: no `BACKUP_FORBIDDEN` value in the
//!    archive; the stored camera URL (decision 14) only in the database copy.
//! 3. The local Farm drifts: a renamed Printer, a Spool with a clashing
//!    number, a deleted Project, a newly finished Job.
//! 4. `preview_restore`: every conflict class, the notices, the blockers.
//! 5. `apply_restore` and the restart, on a credential store with nothing
//!    in it (a new machine): counts equal the manifest, `integrity::check`
//!    is clean, and each Printer with a ref is `CREDENTIAL_REQUIRED`.
//! 6. `export_diagnostics`, every section: no corpus item at all.
//! 7. `reset_farm` tier (c): an empty, clean Farm; the safety backups kept.
//!
//! The Moonraker simulator's version of this flow is in `sim_moonraker.rs`.

mod common;
mod p9_farm;
#[path = "common/p9_flow.rs"]
mod p9_flow;
#[path = "common/secrets.rs"]
mod secrets;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::Connection;
use serde_json::{json, Value};

use farm3d_lib::backup::archive;
use farm3d_lib::backup::installer::InstallOutcome;
use farm3d_lib::backup::journal::{self, JournalPhase};
use farm3d_lib::backup::RestoreJournalKind;
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::supervisor::PrinterSetupFacts;
use farm3d_lib::diagnostics::log::LogId;
use farm3d_lib::persistence::{RepositoryError, Storage, StorageError, StoragePaths};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::setup::{supervise_printer, SupervisionOutcome};

use p9_farm::{ids, Farm};
use p9_flow::{assert_integrity_clean, finished_job, sql, Rig};

const NOW: &str = ids::NOW;
const PROJECT_EXTRA: &str = "prj-t18";
const RENAMED: &str = "Renamed after the backup";

fn spool(id: &str, number: i64) -> String {
    format!(
        "INSERT INTO spools(id, revision, spool_number, manufacturer, material_family,
           color_name, diameter, nominal_mg, current_mg, confidence, lifecycle,
           created_at, updated_at)
         VALUES ('{id}', 1, {number}, 'Acme', 'PLA', 'Black', '1.75', 1000000, 1000000,
                 'measured', 'active', '{NOW}', '{NOW}');"
    )
}

fn entry_bytes(archive_path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive_path).unwrap()).unwrap();
    let mut entries = BTreeMap::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        entries.insert(entry.name().to_string(), bytes);
    }
    entries
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// (class, domain, ids) of every conflict group.
fn conflict_groups(
    preview: &Value,
) -> Vec<(String, String, Vec<(Option<String>, Option<String>)>)> {
    preview["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| {
            (
                group["class"].as_str().unwrap().to_string(),
                group["domain"].as_str().unwrap().to_string(),
                group["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| {
                        (
                            item["localId"].as_str().map(str::to_string),
                            item["backupId"].as_str().map(str::to_string),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

fn notice_kinds(preview: &Value) -> Vec<String> {
    preview["notices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|notice| notice["kind"].as_str().unwrap().to_string())
        .collect()
}

/// Every table's count in a newly migrated database.
fn fresh_counts() -> BTreeMap<String, i64> {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("m"), temp.path().join("d")).unwrap();
    let lease = farm3d_lib::persistence::MetadataRootLease::acquire(&paths).unwrap();
    let storage = Storage::open(paths.clone(), &lease).unwrap();
    drop(storage);
    p9_farm::table_counts(&Connection::open(paths.database()).unwrap())
}

fn names_in(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Drives error and log paths whose errors carry the corpus, on the
/// process log in `log_root`.
fn seed_log(log_root: &Path) {
    farm3d_lib::diagnostics::log::init(log_root);
    farm3d_lib::f3d_log!(
        warn,
        "connections.cacheSaveFailed",
        printer_id = LogId::printer(ids::PRINTER_A),
        error = StorageError::DuplicateHost(secrets::HOST.to_string()),
    );
    farm3d_lib::f3d_log!(
        warn,
        "library.linkCheckFailed",
        model_id = LogId::model(ids::MODEL_3MF),
        error = RepositoryError::Storage(StorageError::CorruptData {
            source_name: "printers.json",
            source_sha256: Some(secrets::CREDENTIAL_VALUE.to_string()),
        }),
    );
    farm3d_lib::diagnostics::log::flush();
}

/// The Printer errored with the corpus in its message (a driven failure).
fn drive_error(rig: &Rig) {
    rig.services.manager.apply_connection_error(
        ids::PRINTER_A,
        format!("could not reach {}", secrets::USERINFO_URL),
        PrinterSetupFacts::complete(),
    );
}

#[test]
fn backup_conflicts_restore_diagnostics_and_reset_run_end_to_end_on_the_fakes() {
    // --- 1. The Farm, seeded with the corpus ---------------------------------
    let farm = Farm::with_every_domain();
    sql(
        &farm,
        &format!(
            "UPDATE settings SET updated_at = '{NOW}';
             INSERT INTO library_projects(id, revision, name, created_at, updated_at)
               VALUES ('{PROJECT_EXTRA}', 1, 'Brackets', '{NOW}', '{NOW}');"
        ),
    );
    let live = farm.paths().database();
    assert!(p9_farm::file_contains(live, secrets::HEADER_LINE));
    assert!(p9_farm::file_contains(live, secrets::USERINFO_URL));
    let credentials = fs::read(farm3d_lib::connections::credentials::credentials_file_path(
        &farm.credentials_dir,
    ))
    .unwrap();
    assert!(!secrets::find_any(secrets::BACKUP_FORBIDDEN, &credentials, "store").is_empty());
    seed_log(farm.paths().log_root());
    let exports = farm.temp.path().join("exports");
    fs::create_dir_all(&exports).unwrap();
    let rig = Rig::new(farm.storage.clone(), farm.credentials_dir.clone());
    drive_error(&rig);
    // A driven command failure: the confirmation is wrong.
    let refused = rig
        .call(
            "apply_restore",
            json!({
                "operationId": "op-refused",
                "stagingId": "stg-00000000-0000-4000-8000-000000000000",
                "confirmation": "restore",
            }),
        )
        .unwrap_err();
    secrets::assert_none_of(
        secrets::BACKUP_FORBIDDEN,
        refused.to_string().as_bytes(),
        "a command error",
    );

    // --- 2. Back up with media: all ------------------------------------------
    let archive_path = exports.join("tracer.farm3d-backup");
    *rig.dialogs.save_backup.lock().unwrap() = Some(archive_path.clone());
    let created = rig.ok(
        "create_backup",
        json!({ "operationId": "op-backup", "media": "all" }),
    );
    assert_eq!(created["status"], "exported", "{created}");
    let manifest = archive::open(&archive_path).unwrap().manifest().clone();
    let bytes = fs::read(&archive_path).unwrap();
    secrets::assert_no_backup_forbidden(&bytes, "backup");
    secrets::assert_none_of(
        secrets::BACKUP_FORBIDDEN,
        created.to_string().as_bytes(),
        "the create_backup result",
    );
    // Decision 14: the query-token camera URL is Farm data, in the database
    // copy and nowhere else.
    let entries = entry_bytes(&archive_path);
    for (name, entry) in &entries {
        assert_eq!(
            contains(entry, secrets::STORED_CAMERA_URL),
            name == archive::DATABASE_PATH,
            "the stored camera URL is in the database copy only, not {name}"
        );
    }
    assert!(contains(
        &entries[archive::DATABASE_PATH],
        secrets::PRINTER_NAME
    ));
    assert!(entries
        .keys()
        .any(|name| name.starts_with("media/snapshots/")));

    // --- 3. The local Farm drifts ---------------------------------------------
    sql(
        &farm,
        &format!(
            "UPDATE printers SET name = '{RENAMED}', revision = revision + 1 WHERE id = 'prn-a';
             UPDATE spools SET spool_number = 12 WHERE id = 'spl-a';
             {new_spool}
             DELETE FROM library_projects WHERE id = '{PROJECT_EXTRA}';
             {job}",
            new_spool = spool("spl-new", 1),
            job = finished_job("t18"),
        ),
    );

    // --- 4. Preview: every conflict class and the notices ---------------------
    // The new machine's credential store is empty: a rig over its own
    // directory previews and applies.
    let machine = tempfile::tempdir().unwrap();
    let empty_store = machine.path().join("credentials");
    fs::create_dir_all(&empty_store).unwrap();
    let rig = {
        drop(rig);
        Rig::new(farm.storage.clone(), empty_store.clone())
    };
    *rig.dialogs.open.lock().unwrap() = Some(archive_path.clone());
    let outcome = rig.ok("preview_restore", json!({ "source": { "kind": "file" } }));
    assert_eq!(outcome["status"], "previewed", "{outcome}");
    let preview = &outcome["preview"];
    let groups = conflict_groups(preview);
    let classes: BTreeSet<&str> = groups.iter().map(|group| group.0.as_str()).collect();
    assert_eq!(
        classes,
        BTreeSet::from(["onlyLocal", "changed", "uniqueClash"]),
        "{groups:?}"
    );
    let has = |class: &str, domain: &str, local: &str, backup: Option<&str>| {
        groups.iter().any(|group| {
            group.0 == class
                && group.1 == domain
                && group
                    .2
                    .contains(&(Some(local.to_string()), backup.map(str::to_string)))
        })
    };
    assert!(has("onlyLocal", "job", "job-t18", None), "{groups:?}");
    assert!(
        has("changed", "printer", "prn-a", Some("prn-a")),
        "{groups:?}"
    );
    assert!(
        has("changed", "spool", "spl-a", Some("spl-a")),
        "{groups:?}"
    );
    assert!(
        has("uniqueClash", "spool", "spl-new", Some("spl-a")),
        "{groups:?}"
    );
    // A deleted Project is not a conflict: the backup adds it back.
    assert!(
        !groups.iter().any(|group| group.1 == "project"),
        "{groups:?}"
    );
    let kinds = notice_kinds(preview);
    for expected in [
        "credentialsToReenter",
        "linkedPathsMissing",
        "slicerRuntimeKeptLocal",
    ] {
        assert!(kinds.iter().any(|kind| kind == expected), "{kinds:?}");
    }
    let to_reenter = preview["notices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|notice| notice["kind"] == "credentialsToReenter")
        .unwrap();
    assert_eq!(to_reenter["printerCount"], 2, "{to_reenter}");
    let counts: BTreeMap<String, (i64, i64)> = preview["counts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["table"].as_str().unwrap().to_string(),
                (
                    row["local"].as_i64().unwrap(),
                    row["backup"].as_i64().unwrap(),
                ),
            )
        })
        .collect();
    assert_eq!(counts["jobs"].0, counts["jobs"].1 + 1);
    assert_eq!(
        counts["library_projects"].1,
        counts["library_projects"].0 + 1
    );
    // Local work in flight refuses the restore until it settles.
    let blocker_kinds: Vec<&str> = preview["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|blocker| blocker["kind"].as_str().unwrap())
        .collect();
    assert!(!blocker_kinds.is_empty(), "the seeded upload is unresolved");
    let staging_id = preview["stagingId"].as_str().unwrap().to_string();
    let discarded = rig.ok(
        "discard_restore_preview",
        json!({ "stagingId": staging_id }),
    );
    assert_eq!(discarded["discarded"], true);
    sql(
        &farm,
        &format!(
            "UPDATE host_operations SET state = 'failed', resolved_at = '{NOW}',
                    failure_json = '{{\"code\":\"neverSent\"}}'
              WHERE state IN ('dispatching', 'uncertain', 'reconciling');
             UPDATE jobs SET state = 'failed', ended_at = '{NOW}'
              WHERE state NOT IN ('completed', 'failed', 'cancelled');"
        ),
    );
    let outcome = rig.ok("preview_restore", json!({ "source": { "kind": "file" } }));
    assert_eq!(outcome["preview"]["blockers"], json!([]), "{outcome}");
    let staging_id = outcome["preview"]["stagingId"]
        .as_str()
        .unwrap()
        .to_string();

    // --- 5. Apply, restart, and the new machine's start ------------------------
    let applied = rig
        .call(
            "apply_restore",
            json!({
                "operationId": "op-apply",
                "stagingId": staging_id,
                "confirmation": "restore",
            }),
        )
        .unwrap_or_else(|error| panic!("apply_restore: {error}"));
    assert_eq!(applied["status"], "restarting", "{applied}");
    let restore_safety = applied["safetyBackupId"].as_str().unwrap().to_string();
    assert!(rig.restarter.wait_for(1, Duration::from_secs(5)));
    let pending = fs::read_to_string(journal::journal_path(farm.paths())).unwrap();
    secrets::assert_no_backup_forbidden(pending.as_bytes(), "the restore journal");
    farm3d_lib::diagnostics::log::shutdown();
    let closed = p9_flow::relocate(farm.paths());
    let slot = Mutex::new(None);
    let (storage, report) = p9_flow::start(&closed.paths, &slot, &empty_store);
    assert_eq!(report.outcome, InstallOutcome::Installed, "{report:?}");
    assert_eq!(report.kind, Some(RestoreJournalKind::Restore));
    let journal = journal::read(&closed.paths).unwrap().unwrap();
    assert_eq!(journal.phase, JournalPhase::Done);
    let restored = assert_integrity_clean(&closed.paths);
    for (table, rows) in &restored {
        if table == "pending_credential_cleanup" || table == "slicer_runtime_config" {
            continue; // D8's two carried tables
        }
        assert_eq!(Some(rows), manifest.counts.get(table), "{table}");
    }
    assert_eq!(
        restored.keys().collect::<Vec<_>>(),
        manifest.counts.keys().collect::<Vec<_>>()
    );
    let database = Connection::open(closed.paths.database()).unwrap();
    let name_of = |id: &str| -> Option<String> {
        database
            .query_row("SELECT name FROM printers WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .ok()
    };
    assert_eq!(name_of("prn-a").as_deref(), Some(secrets::PRINTER_NAME));
    let ids_in = |table: &str, id: &str| -> i64 {
        database
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE id = ?1"),
                [id],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(ids_in("library_projects", PROJECT_EXTRA), 1);
    assert_eq!(ids_in("jobs", "job-t18"), 0);
    assert_eq!(ids_in("spools", "spl-new"), 0);
    drop(database);

    // Every Printer with a ref needs its credential entered again.
    let new_machine = CredentialStore::file_backed(empty_store.clone());
    let printers = PrinterRepository::new(storage.clone()).list().unwrap();
    let manager_rig = Rig::new(storage.clone(), empty_store.clone());
    let catalog = common::a_catalog();
    let mut required = Vec::new();
    for printer in &printers {
        let outcome = tauri::async_runtime::block_on(supervise_printer(
            &manager_rig.services.manager,
            &new_machine,
            &catalog,
            printer,
        ));
        let has_ref = printer
            .connection
            .as_ref()
            .and_then(|connection| connection.credential_ref.as_ref())
            .is_some();
        if printer.archived_at.is_none() && has_ref {
            assert_eq!(
                outcome,
                SupervisionOutcome::CredentialRequired,
                "{}",
                printer.id
            );
            required.push(printer.id.clone());
        } else {
            assert_ne!(
                outcome,
                SupervisionOutcome::CredentialRequired,
                "{}",
                printer.id
            );
        }
    }
    required.sort();
    assert_eq!(required, vec![ids::PRINTER_A, ids::PRINTER_B]);
    let restore_status = manager_rig.ok("restore_status", json!({}));
    assert_eq!(restore_status["state"], "done", "{restore_status}");

    // --- 6. Diagnostics, every section: no corpus item at all -------------------
    seed_log(closed.paths.log_root());
    drive_error(&manager_rig);
    let exports = closed.temp.path().join("exports");
    fs::create_dir_all(&exports).unwrap();
    let bundle_path = exports.join("bundle.zip");
    *manager_rig.dialogs.save_diagnostics.lock().unwrap() = Some(bundle_path.clone());
    let exported = manager_rig.ok(
        "export_diagnostics",
        json!({
            "operationId": "op-diagnostics",
            "sections": ["about", "health", "storage", "configuration", "logs", "recentProblems"],
        }),
    );
    assert_eq!(exported["status"], "exported", "{exported}");
    let bundle = fs::read(&bundle_path).unwrap();
    secrets::assert_no_corpus(&bundle, "diagnostics bundle");
    secrets::assert_no_corpus(exported.to_string().as_bytes(), "the export result");

    // --- 7. Reset tier (c) ---------------------------------------------------------
    let reset = manager_rig
        .call(
            "reset_farm",
            json!({
                "operationId": "op-reset",
                "request": { "tier": "farm", "safetyBackup": true, "deleteSafetyBackups": false },
                "confirmation": "reset farm",
            }),
        )
        .unwrap_or_else(|error| panic!("reset_farm: {error}"));
    assert_eq!(reset["status"], "restarting", "{reset}");
    let reset_safety = reset["safetyBackupId"].as_str().unwrap().to_string();
    assert!(manager_rig.restarter.wait_for(1, Duration::from_secs(5)));
    farm3d_lib::diagnostics::log::shutdown();
    let after_reset = p9_flow::relocate(storage.paths());
    let paths = after_reset.paths.clone();
    let slot = Mutex::new(None);
    let (storage, report) = p9_flow::start(&paths, &slot, &empty_store);
    assert_eq!(report.outcome, InstallOutcome::Installed, "{report:?}");
    assert_eq!(report.kind, Some(RestoreJournalKind::Reset));
    let fresh = assert_integrity_clean(&paths);
    assert_eq!(fresh, fresh_counts(), "an empty Farm");
    let safety = paths.backup_root().join("safety");
    let kept = names_in(&safety);
    for id in [&restore_safety, &reset_safety] {
        assert!(
            kept.contains(&format!("{id}.farm3d-backup")),
            "{id} is kept: {kept:?}"
        );
    }
    drop(storage);
}
