//! Task 11 (P8 D8): the lifecycle guards that keep a Printer's Incident
//! identity and camera evidence through archive and delete, and the
//! delete-time handling of its Attention Events and unattached snapshots.
//!
//! - A Printer with an Incident can be archived but not deleted
//!   (`INCIDENT_HISTORY_EXISTS`); archive resolves its `printer.*` Events
//!   `conditionCleared` on the next pass, and the Incident stays openable.
//! - A Printer with only Attention Events can be deleted: its open Events
//!   resolve `sourceRemoved`, `printer_id` becomes NULL, and the subject
//!   keeps its name and location.
//! - A pinned, unpruned, unattached manual snapshot blocks the delete
//!   (`PINNED_EVIDENCE_EXISTS`); unpinned or pruned ones are removed with
//!   the Printer, and their files after commit.
//! - `import_printers` (`replace_all`) while any Incident or snapshot row
//!   exists fails `EVIDENCE_EXISTS` and writes nothing.
//!
//! After every allowed delete, nothing is orphaned: every foreign key still
//! resolves, and no snapshot file is left without its row.
//!
//! Every frame is a few synthetic bytes from `FakeCamera` (global
//! constraint 2).

mod common;
mod p7_dispatch_rig;

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::fake_camera::{Answer, FakeCamera};
use farm3d_lib::attention::services::AttentionTimings;
use farm3d_lib::attention::{
    deep_link, repository as attention_repo, AttentionEvent, AttentionOrigin,
    AttentionResolution, ConditionKind,
};
use farm3d_lib::cameras::media;
use farm3d_lib::cameras::services::CameraTimings;
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::host_ops::Clock;
use farm3d_lib::jobs::JobTimings;
use farm3d_lib::persistence::Storage;
use farm3d_lib::printers::lifecycle::LifecycleAction;
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::StartSafety;
use p7_dispatch_rig::{
    boot_with_attention, status_of, AttentionBoot, ManualClock, Roots, Running, PRINTER,
};
use serde_json::{json, Value};

/// Past the default five-minute offline grace.
const PAST_GRACE: Duration = Duration::from_secs(5 * 60 + 1);

struct GuardRig {
    _roots: Roots,
    app: Running,
    _camera: FakeCamera,
    clock: Arc<ManualClock>,
}

impl GuardRig {
    /// The dispatch rig (one Printer, "Alpha" at "Bay A", no Job) with the
    /// capture runtime on, its camera on `FakeCamera`, and the projector's
    /// clock under the test's hand.
    fn new() -> Self {
        let roots = Roots::new(StartSafety::Unattended);
        let clock = ManualClock::new();
        let camera = FakeCamera::start(Answer::Jpeg);
        let (app, _) = boot_with_attention(
            &roots,
            status_of(OperationalState::Ready),
            JobTimings {
                history_poll: Duration::from_millis(200),
                ..JobTimings::default()
            },
            AttentionBoot {
                timings: AttentionTimings {
                    pass_min_interval: Duration::ZERO,
                    safety_tick: Duration::from_secs(3600),
                },
                clock: Some(clock.clone() as Arc<dyn Clock>),
                cameras: Some(CameraTimings::default()),
            },
        );
        let url = camera.url("/snapshot");
        app.storage
            .write(|tx| {
                tx.execute(
                    "UPDATE printers SET location = 'Bay A' WHERE id = ?1",
                    [PRINTER],
                )?;
                tx.execute(
                    "INSERT INTO printer_cameras(printer_id, source_kind, snapshot_url, updated_at) \
                     VALUES (?1, 'snapshotUrl', ?2, '2026-09-28T09:00:00.000Z')",
                    rusqlite::params![PRINTER, url],
                )?;
                Ok(())
            })
            .unwrap();
        app.attention_pass();
        Self {
            _roots: roots,
            app,
            _camera: camera,
            clock,
        }
    }

    fn storage(&self) -> &Arc<Storage> {
        &self.app.storage
    }

    fn call(&self, command: &str, body: Value) -> Result<Value, Value> {
        self.app.call(command, body)
    }

    fn revision(&self) -> i64 {
        self.app
            .scalar(&format!("SELECT revision FROM printers WHERE id = '{PRINTER}'"))
    }

    fn printer_exists(&self) -> bool {
        self.app
            .scalar(&format!("SELECT COUNT(*) FROM printers WHERE id = '{PRINTER}'"))
            == 1
    }

    fn archive(&self) {
        self.app.ok(
            "archive_printer",
            json!({"id": PRINTER, "expectedRevision": self.revision(),
                   "operationId": "op-archive", "spoolDispositions": []}),
        );
    }

    fn delete(&self) -> Result<Value, Value> {
        self.call(
            "delete_printer",
            json!({"id": PRINTER, "expectedRevision": self.revision()}),
        )
    }

    fn capture(&self, operation_id: &str) -> String {
        let snapshot = self
            .call(
                "capture_snapshot",
                json!({"operationId": operation_id, "printerId": PRINTER}),
            )
            .unwrap_or_else(|error| panic!("capture failed: {error}"));
        snapshot["id"].as_str().unwrap().to_string()
    }

    fn pin(&self, operation_id: &str, snapshot_id: &str, pinned: bool) {
        self.call(
            "set_snapshot_pinned",
            json!({"operationId": operation_id, "snapshotId": snapshot_id, "pinned": pinned}),
        )
        .unwrap_or_else(|error| panic!("pin failed: {error}"));
    }

    fn rel_path(&self, snapshot_id: &str) -> String {
        self.storage()
            .read(|conn| {
                conn.query_row(
                    "SELECT rel_path FROM camera_snapshots WHERE id = ?1",
                    [snapshot_id],
                    |row| row.get(0),
                )
            })
            .unwrap()
    }

    fn of(&self, condition: ConditionKind) -> Vec<AttentionEvent> {
        self.app
            .attention_rows()
            .into_iter()
            .filter(|event| event.condition == condition)
            .collect()
    }

    fn event(&self, id: &str) -> AttentionEvent {
        self.storage()
            .read(|conn| Ok(attention_repo::load_event(conn, id)))
            .unwrap()
            .unwrap()
            .expect("the Event row stays")
    }

    /// The Printer goes offline and stays so past its grace.
    fn go_offline_past_grace(&self) {
        self.app.seed(status_of(OperationalState::Offline));
        self.app.attention_pass();
        self.clock.advance(PAST_GRACE);
        self.app.services.attention.poke();
        self.app.attention_pass();
    }

    /// Waits until the capture consumer has taken every committed change
    /// and no capture is running.
    fn settle_captures(&self) {
        let services = &self.app.services;
        self.app.wait_until("the captures settle", || {
            services.cameras.capture_rounds() >= services.attention.applied_sent()
                && services.cameras.captures_in_flight() == 0
        });
    }

    fn eligibility_blockers(&self) -> Vec<(LifecycleAction, String)> {
        PrinterRepository::new(Arc::clone(self.storage()))
            .lifecycle_eligibility(PRINTER)
            .unwrap()
            .expect("the Printer exists")
            .blockers
            .into_iter()
            .map(|blocker| {
                (
                    blocker.action,
                    serde_json::to_value(blocker.code).unwrap().as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    /// Every file under the media root's `snapshots/`, relative.
    fn snapshot_files(&self) -> BTreeSet<String> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    out.insert(
                        path.strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
        let root = self.storage().paths().media_root().to_path_buf();
        let mut out = BTreeSet::new();
        walk(&root, &root.join("snapshots"), &mut out);
        out
    }

    fn unpruned_paths(&self) -> BTreeSet<String> {
        self.storage()
            .read(|conn| {
                let mut statement = conn
                    .prepare("SELECT rel_path FROM camera_snapshots WHERE pruned_at IS NULL")?;
                let paths = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect();
                paths
            })
            .unwrap()
    }

    /// The no-orphan invariant: every foreign key resolves (or is NULL
    /// where the schema says `SET NULL`), every snapshot's Printer,
    /// Incident, and Job exists, and every snapshot file has its row.
    fn assert_no_orphans(&self) {
        let violations: Vec<String> = self
            .storage()
            .read(|conn| {
                let mut statement = conn.prepare("PRAGMA foreign_key_check")?;
                let rows = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect();
                rows
            })
            .unwrap();
        assert!(violations.is_empty(), "foreign key violations in {violations:?}");
        for (what, sql) in [
            (
                "a snapshot's Printer",
                "SELECT COUNT(*) FROM camera_snapshots s
                 WHERE NOT EXISTS(SELECT 1 FROM printers p WHERE p.id = s.printer_id)",
            ),
            (
                "a snapshot's Incident",
                "SELECT COUNT(*) FROM camera_snapshots s WHERE s.incident_id IS NOT NULL
                 AND NOT EXISTS(SELECT 1 FROM incidents i WHERE i.id = s.incident_id)",
            ),
            (
                "a snapshot's Job",
                "SELECT COUNT(*) FROM camera_snapshots s WHERE s.job_id IS NOT NULL
                 AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.id = s.job_id)",
            ),
            (
                "an Incident's Printer",
                "SELECT COUNT(*) FROM incidents i
                 WHERE NOT EXISTS(SELECT 1 FROM printers p WHERE p.id = i.printer_id)",
            ),
            (
                "an Event's Printer",
                "SELECT COUNT(*) FROM attention_events e WHERE e.printer_id IS NOT NULL
                 AND NOT EXISTS(SELECT 1 FROM printers p WHERE p.id = e.printer_id)",
            ),
            (
                "an Event's Incident",
                "SELECT COUNT(*) FROM attention_events e WHERE e.incident_id IS NOT NULL
                 AND NOT EXISTS(SELECT 1 FROM incidents i WHERE i.id = e.incident_id)",
            ),
            (
                "a timeline row's snapshot",
                "SELECT COUNT(*) FROM incident_events t WHERE t.snapshot_id IS NOT NULL
                 AND NOT EXISTS(SELECT 1 FROM camera_snapshots s WHERE s.id = t.snapshot_id)",
            ),
        ] {
            assert_eq!(self.app.scalar(sql), 0, "{what} is dangling");
        }
        assert_eq!(
            self.snapshot_files(),
            self.unpruned_paths(),
            "a snapshot file without its row, or a row without its file"
        );
    }
}

impl Drop for GuardRig {
    fn drop(&mut self) {
        self.app.services.attention.release();
    }
}

fn blocker_codes(error: &Value) -> Vec<String> {
    error["details"]["blockers"]
        .as_array()
        .unwrap_or_else(|| panic!("no blockers: {error}"))
        .iter()
        .map(|blocker| blocker["code"].as_str().unwrap().to_string())
        .collect()
}

fn target_json(event: &AttentionEvent, source_exists: bool) -> Value {
    serde_json::to_value(deep_link::target_for(event, source_exists)).unwrap()
}

// --- Incidents: archive yes, delete no -------------------------------------------------

#[test]
fn a_printer_with_an_incident_can_be_archived_but_not_deleted() {
    let rig = GuardRig::new();
    // A host failure with no Job opens `printer.hostFailed` and its
    // Incident; the capture runtime stores the Incident's snapshot.
    rig.app.seed(status_of(OperationalState::Failed));
    rig.app.attention_pass();
    let failed = rig.of(ConditionKind::PrinterHostFailed);
    assert_eq!(failed.len(), 1, "{failed:?}");
    let incident_id = failed[0].incident_id.clone().expect("the Event opened an Incident");
    rig.settle_captures();
    assert_eq!(
        rig.app.scalar("SELECT COUNT(*) FROM camera_snapshots WHERE trigger = 'incident'"),
        1
    );

    // Archive proceeds; the next pass resolves the `printer.*` Event
    // `conditionCleared`, and the Incident stays openable.
    rig.archive();
    rig.app.attention_pass();
    let failed = rig.event(&failed[0].id);
    assert_eq!(failed.resolution, Some(AttentionResolution::ConditionCleared));
    assert_eq!(failed.printer_id.as_deref(), Some(PRINTER));
    let incident = rig.app.ok("get_incident", json!({"incidentId": incident_id}));
    assert_eq!(incident["incident"]["id"], json!(incident_id));
    assert_eq!(incident["incident"]["printerId"], PRINTER);
    // An archived Printer stays a valid deep-link target.
    let exists = rig
        .storage()
        .read(|conn| deep_link::source_exists(conn, &failed))
        .unwrap();
    assert!(exists);
    assert_eq!(
        target_json(&failed, exists),
        json!({"version": 1, "destination": "monitor",
               "selection": {"kind": "printer", "id": PRINTER}})
    );

    // Delete is blocked, and says why, both in the eligibility and the
    // command's error.
    assert!(rig
        .eligibility_blockers()
        .contains(&(LifecycleAction::Delete, "INCIDENT_HISTORY_EXISTS".to_string())));
    let files = rig.snapshot_files();
    let error = rig.delete().unwrap_err();
    assert_eq!(error["code"], "LIFECYCLE_BLOCKED", "{error}");
    assert_eq!(blocker_codes(&error), ["INCIDENT_HISTORY_EXISTS"]);
    assert_eq!(
        error["details"]["blockers"][0]["message"],
        "This Printer has Incident history. Archive it instead."
    );
    // Nothing was written.
    assert!(rig.printer_exists());
    assert_eq!(rig.app.scalar("SELECT COUNT(*) FROM incidents"), 1);
    assert_eq!(rig.app.scalar("SELECT COUNT(*) FROM camera_snapshots"), 1);
    assert_eq!(rig.snapshot_files(), files);
    assert_eq!(rig.event(&failed.id).printer_id.as_deref(), Some(PRINTER));
    rig.assert_no_orphans();
}

// --- Attention Events: delete keeps their identity -------------------------------------

#[test]
fn deleting_a_printer_with_only_attention_events_resolves_them_source_removed() {
    let rig = GuardRig::new();
    // One resolved `printer.offline` Event, then its recurrence, open.
    rig.go_offline_past_grace();
    rig.app.seed(status_of(OperationalState::Ready));
    rig.app.attention_pass();
    rig.go_offline_past_grace();
    let offline = rig.of(ConditionKind::PrinterOffline);
    assert_eq!(offline.len(), 2, "{offline:?}");
    let resolved = offline.iter().find(|event| event.resolved_at.is_some()).unwrap().clone();
    let open = offline.iter().find(|event| event.resolved_at.is_none()).unwrap().clone();
    assert_eq!(open.subject.printer_name.as_deref(), Some("Alpha"));
    assert_eq!(open.subject.printer_location.as_deref(), Some("Bay A"));

    // The projector is held so the archive's pass can't clear the Event
    // first: the delete itself must resolve it.
    rig.app.services.attention.hold();
    rig.archive();
    assert!(rig.eligibility_blockers().iter().all(|(action, _)| *action != LifecycleAction::Delete));
    let deleted = rig.delete().unwrap_or_else(|error| panic!("delete failed: {error}"));
    assert_eq!(deleted["deletedId"], PRINTER);
    assert!(!rig.printer_exists());

    let removed = rig.event(&open.id);
    assert_eq!(removed.resolution, Some(AttentionResolution::SourceRemoved));
    assert!(removed.resolved_at.is_some() && removed.read_at.is_some());
    assert_eq!(removed.printer_id, None, "ON DELETE SET NULL");
    assert_eq!(removed.subject, open.subject, "the subject keeps the Printer's identity");
    assert_eq!(removed.source.id, PRINTER, "the source still names the Printer");
    assert!(removed.revision > open.revision);
    let kept = rig.event(&resolved.id);
    assert_eq!(kept.resolution, Some(AttentionResolution::ConditionCleared));
    assert_eq!(kept.printer_id, None);
    assert_eq!(kept.subject, resolved.subject);
    let raw: String = rig
        .storage()
        .read(|conn| {
            conn.query_row(
                "SELECT subject_snapshot_json FROM attention_events WHERE id = ?1",
                [&open.id],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert!(raw.contains("Alpha") && raw.contains("Bay A"), "{raw}");

    // The resolution was published after commit.
    let published = rig.app.attention_stream("attention.event.changed", &open.id);
    assert_eq!(
        published.last().unwrap()["payload"]["event"]["resolution"],
        "sourceRemoved",
        "{published:?}"
    );
    // With its source gone, the Event deep-links to itself.
    let exists = rig
        .storage()
        .read(|conn| deep_link::source_exists(conn, &removed))
        .unwrap();
    assert!(!exists);
    assert_eq!(
        target_json(&removed, exists),
        json!({"version": 1, "destination": "monitor",
               "selection": {"kind": "attention", "id": open.id}})
    );

    // A later pass leaves them alone and opens nothing for the gone Printer.
    rig.app.services.attention.release();
    rig.app.services.attention.poke();
    rig.app.attention_pass();
    assert_eq!(rig.event(&open.id).resolution, Some(AttentionResolution::SourceRemoved));
    assert_eq!(
        rig.app.scalar("SELECT COUNT(*) FROM attention_events WHERE resolved_at IS NULL"),
        0
    );
    rig.assert_no_orphans();
}

// --- Unattached manual snapshots ------------------------------------------------------

#[test]
fn pinned_unattached_evidence_blocks_delete_and_the_rest_goes_with_the_printer() {
    let rig = GuardRig::new();
    let pinned = rig.capture("op-cap-pinned");
    rig.pin("op-pin", &pinned, true);
    let unpinned = rig.capture("op-cap-unpinned");
    // Pinned, but its file went missing: pruned `missingFile`, no image
    // left to protect.
    let missing = rig.capture("op-cap-missing");
    rig.pin("op-pin-missing", &missing, true);
    std::fs::remove_file(rig.storage().paths().media_root().join(rig.rel_path(&missing))).unwrap();
    media::mark_missing(rig.storage(), &missing, rig.app.services.attention.now()).unwrap();
    // Unpinned and pruned for age (its file already unlinked).
    let aged = rig.capture("op-cap-aged");
    std::fs::remove_file(rig.storage().paths().media_root().join(rig.rel_path(&aged))).unwrap();
    rig.storage()
        .write(|tx| {
            tx.execute(
                "UPDATE camera_snapshots SET pruned_at = '2026-09-28T09:00:00.000Z',
                   prune_reason = 'age', revision = revision + 1 WHERE id = ?1",
                [&aged],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        rig.app.scalar(
            "SELECT COUNT(*) FROM camera_snapshots WHERE pinned_at IS NOT NULL AND pruned_at IS NOT NULL"
        ),
        1
    );
    rig.archive();
    rig.assert_no_orphans();

    // Only the pinned, unpruned one blocks.
    assert!(rig
        .eligibility_blockers()
        .contains(&(LifecycleAction::Delete, "PINNED_EVIDENCE_EXISTS".to_string())));
    let files = rig.snapshot_files();
    assert_eq!(files.len(), 2, "{files:?}");
    let error = rig.delete().unwrap_err();
    assert_eq!(error["code"], "LIFECYCLE_BLOCKED", "{error}");
    assert_eq!(blocker_codes(&error), ["PINNED_EVIDENCE_EXISTS"]);
    assert_eq!(
        error["details"]["blockers"][0]["message"],
        "This Printer has pinned camera evidence. Unpin it or archive the Printer instead."
    );
    assert!(rig.printer_exists());
    assert_eq!(rig.app.scalar("SELECT COUNT(*) FROM camera_snapshots"), 4);
    assert_eq!(rig.snapshot_files(), files, "nothing unlinked");

    // Unpinned, the delete removes every unattached manual row, and the
    // unpruned ones' files after commit.
    rig.pin("op-unpin", &pinned, false);
    rig.delete().unwrap_or_else(|error| panic!("delete failed: {error}"));
    assert!(!rig.printer_exists());
    assert_eq!(rig.app.scalar("SELECT COUNT(*) FROM camera_snapshots"), 0);
    assert!(rig.snapshot_files().is_empty(), "{:?}", rig.snapshot_files());
    let _ = unpinned;
    rig.assert_no_orphans();

    // The startup sweep finds nothing left to clean.
    let swept = media::startup_sweep(rig.storage(), rig.app.services.attention.now()).unwrap();
    assert!(swept.is_empty(), "{swept:?}");
    assert!(rig.snapshot_files().is_empty());
}

// --- Import: EVIDENCE_EXISTS -------------------------------------------------------------

struct Plain {
    _temp: tempfile::TempDir,
    _lease: farm3d_lib::persistence::MetadataRootLease,
    storage: Arc<Storage>,
}

const P1: &str = "prn-guard-1";
const P2: &str = "prn-guard-2";
const AT: &str = "2026-09-28T09:00:00.000Z";

impl Plain {
    /// Two Printers, P1 and P2, with nothing else.
    fn new() -> Self {
        let (temp, lease, storage, _) = common::storage();
        let repository = PrinterRepository::new(Arc::clone(&storage));
        for id in [P1, P2] {
            repository.create(common::a_stored_printer(id)).unwrap();
        }
        Self {
            _temp: temp,
            _lease: lease,
            storage,
        }
    }

    fn expected(&self) -> Vec<(String, i64)> {
        let mut expected: Vec<(String, i64)> = PrinterRepository::new(Arc::clone(&self.storage))
            .list()
            .unwrap()
            .into_iter()
            .map(|printer| (printer.id, printer.revision))
            .collect();
        expected.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
        expected
    }

    fn scalar(&self, sql: &str) -> i64 {
        self.storage
            .read(|conn| conn.query_row(sql, [], |row| row.get(0)))
            .unwrap()
    }

    fn import(&self) -> Result<(), Value> {
        PrinterRepository::new(Arc::clone(&self.storage))
            .replace_all(
                &self.expected(),
                vec![common::a_stored_printer("prn-guard-new")],
                &HashMap::new(),
            )
            .map(|_| ())
            .map_err(|error| serde_json::to_value(CommandError::from_repository(error)).unwrap())
    }

    fn insert_snapshot(&self, id: &str, printer_id: &str) {
        self.storage
            .write(|tx| {
                tx.execute(
                    "INSERT INTO camera_snapshots(id, printer_id, trigger, operation_id, captured_at,
                       content_type, byte_len, sha256, rel_path)
                     VALUES (?1, ?2, 'manual', ?3, ?4, 'image/jpeg', 10, ?5, ?6)",
                    rusqlite::params![
                        id,
                        printer_id,
                        format!("op-{id}"),
                        AT,
                        "0".repeat(64),
                        format!("snapshots/2026/09/{id}.jpg"),
                    ],
                )?;
                Ok(())
            })
            .unwrap();
    }

    fn insert_incident(&self, id: &str, printer_id: &str) {
        self.storage
            .write(|tx| {
                tx.execute(
                    "INSERT INTO incidents(id, kind, printer_id, job_id, printer_snapshot_json, opened_at)
                     VALUES (?1, 'printer.hostFailed', ?2, NULL, '{}', ?3)",
                    rusqlite::params![id, printer_id, AT],
                )?;
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn an_import_over_camera_evidence_fails_evidence_exists_and_writes_nothing() {
    let plain = Plain::new();
    plain.insert_snapshot("snp-guard", P2);
    let error = plain.import().unwrap_err();
    assert_eq!(error["code"], "EVIDENCE_EXISTS", "{error}");
    assert_eq!(
        error["message"],
        "Printers with Incidents or camera evidence can't be replaced by an import."
    );
    assert_eq!(error["recovery"], json!([]));
    assert_eq!(
        error["details"],
        json!({"printerIds": [P2], "incidentIds": [], "snapshotIds": ["snp-guard"]})
    );
    assert_eq!(plain.scalar("SELECT COUNT(*) FROM printers"), 2);
    assert_eq!(
        plain.scalar("SELECT COUNT(*) FROM printers WHERE id = 'prn-guard-new'"),
        0
    );
    assert_eq!(plain.scalar("SELECT COUNT(*) FROM camera_snapshots"), 1);
}

#[test]
fn an_import_over_an_incident_fails_evidence_exists_and_writes_nothing() {
    let plain = Plain::new();
    plain.insert_incident("inc-guard", P1);
    let error = plain.import().unwrap_err();
    assert_eq!(error["code"], "EVIDENCE_EXISTS", "{error}");
    assert_eq!(
        error["details"],
        json!({"printerIds": [P1], "incidentIds": ["inc-guard"], "snapshotIds": []})
    );
    assert_eq!(plain.scalar("SELECT COUNT(*) FROM printers"), 2);
    assert_eq!(plain.scalar("SELECT COUNT(*) FROM incidents"), 1);
}

#[test]
fn an_import_resolves_the_replaced_printers_open_events_source_removed() {
    let plain = Plain::new();
    let now = chrono::Utc::now();
    let event = plain
        .storage
        .write_repo(|tx| {
            attention_repo::insert(
                tx,
                &offline_condition(P1),
                None,
                false,
                AttentionOrigin::Live,
                now,
            )
        })
        .unwrap();
    plain.import().unwrap_or_else(|error| panic!("import failed: {error}"));
    let after = plain
        .storage
        .read(|conn| Ok(attention_repo::load_event(conn, &event.id)))
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(after.resolution, Some(AttentionResolution::SourceRemoved));
    assert_eq!(after.printer_id, None);
    assert_eq!(after.subject, event.subject);
    assert_eq!(plain.scalar("SELECT COUNT(*) FROM printers"), 1);
}

fn offline_condition(printer_id: &str) -> farm3d_lib::attention::Condition {
    use farm3d_lib::attention::{AttentionDetail, AttentionSubject, Condition};
    Condition {
        kind: ConditionKind::PrinterOffline,
        source_id: printer_id.to_string(),
        printer_id: Some(printer_id.to_string()),
        job_id: None,
        spool_id: None,
        requirement_id: None,
        subject: AttentionSubject {
            printer_name: Some("Test Printer".to_string()),
            printer_location: Some("Bay B".to_string()),
            job_label: None,
            spool_number: None,
            spool_label: None,
        },
        detail: AttentionDetail::PrinterOffline {
            unreachable_since: AT.to_string(),
        },
        acknowledge: false,
    }
}

// --- Repository delete over the same rules ---------------------------------------------

#[test]
fn a_repository_delete_removes_unattached_rows_and_resolves_open_events() {
    let plain = Plain::new();
    let now = chrono::Utc::now();
    let event = plain
        .storage
        .write_repo(|tx| {
            attention_repo::insert(tx, &offline_condition(P1), None, false, AttentionOrigin::Live, now)
        })
        .unwrap();
    plain.insert_snapshot("snp-unattached", P1);
    plain.insert_snapshot("snp-other", P2);
    let repository = PrinterRepository::new(Arc::clone(&plain.storage));
    let printer = repository.get(P1).unwrap().unwrap();
    let archived = repository
        .archive(P1, printer.revision, "op-archive-p1", &[])
        .unwrap();
    repository.delete(P1, archived.revision).unwrap();
    assert_eq!(plain.scalar("SELECT COUNT(*) FROM printers WHERE id = 'prn-guard-1'"), 0);
    assert_eq!(
        plain.scalar("SELECT COUNT(*) FROM camera_snapshots WHERE id = 'snp-unattached'"),
        0
    );
    assert_eq!(
        plain.scalar("SELECT COUNT(*) FROM camera_snapshots WHERE id = 'snp-other'"),
        1,
        "another Printer's evidence stays"
    );
    let after = plain
        .storage
        .read(|conn| Ok(attention_repo::load_event(conn, &event.id)))
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(after.resolution, Some(AttentionResolution::SourceRemoved));
    assert_eq!(after.printer_id, None);
}
