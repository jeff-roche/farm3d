//! Task 8 (P8 D5): the snapshot media store and its retention.
//!
//! - Step 1, pure: `plan_prune` (age, the disk cap, pinned rows, `fits`).
//! - Step 2, the store against a real migrated `Storage` and its media
//!   root: the capture write order (tmp, rename, then the row), the
//!   startup sweep after a crash on either side of a commit, a missing
//!   file, and 20 concurrent captures against a pruning pass.
//!
//! - Step 3, the triggers over `FakeCamera` and the P7 dispatch rig: an
//!   Incident's capture (stored, or `evidenceSkipped`), the toggle off, a
//!   Job completion's evidence, the lagged capture consumer, and the
//!   snapshot commands (`capture_snapshot`, `set_snapshot_pinned`,
//!   `snapshot_image`, `list_snapshots`, `media_usage`).
//!
//! Every frame here is a few synthetic bytes (global constraint 2).

mod common;
mod p7_dispatch_rig;

use chrono::{DateTime, Duration as ChronoDuration, Utc};

use farm3d_lib::cameras::retention::{plan_prune, PruneAction, RetainedSnapshot, RetentionPolicy};
use farm3d_lib::cameras::PruneReason;

const NOW_TEXT: &str = "2026-09-28T09:00:00Z";

fn now() -> DateTime<Utc> {
    NOW_TEXT.parse().unwrap()
}

fn row(id: &str, days_old: i64, byte_len: i64, pinned: bool) -> RetainedSnapshot {
    RetainedSnapshot {
        id: id.to_string(),
        captured_at: now() - ChronoDuration::days(days_old),
        byte_len,
        pinned,
    }
}

fn policy(retention_days: i64, cap_bytes: i64) -> RetentionPolicy {
    RetentionPolicy {
        retention_days,
        cap_bytes,
    }
}

fn action(id: &str, reason: PruneReason) -> PruneAction {
    PruneAction {
        id: id.to_string(),
        reason,
    }
}

// --- Step 1: plan_prune -------------------------------------------------------------

#[test]
fn rows_older_than_the_retention_period_are_pruned_for_age() {
    let rows = [
        row("snp-old", 31, 10, false),
        row("snp-edge", 30, 10, false),
        row("snp-new", 1, 10, false),
    ];
    let plan = plan_prune(&rows, policy(30, 1_000), now(), 0);
    assert_eq!(plan.prune, vec![action("snp-old", PruneReason::Age)]);
    assert!(plan.fits);
}

#[test]
fn over_the_cap_the_oldest_unpinned_rows_go_for_the_disk_cap_until_usage_fits() {
    let rows = [
        row("snp-c", 3, 40, false),
        row("snp-a", 5, 40, false),
        row("snp-b", 5, 40, false),
        row("snp-d", 1, 40, false),
    ];
    // 160 used + 30 incoming against 120: two rows must go, oldest first
    // (captured_at, then id).
    let plan = plan_prune(&rows, policy(30, 120), now(), 30);
    assert_eq!(
        plan.prune,
        vec![
            action("snp-a", PruneReason::DiskCap),
            action("snp-b", PruneReason::DiskCap)
        ]
    );
    assert!(plan.fits);
    // Exactly at the cap fits.
    let plan = plan_prune(&rows, policy(30, 160), now(), 0);
    assert!(plan.prune.is_empty() && plan.fits);
}

#[test]
fn age_goes_first_and_counts_toward_the_cap() {
    let rows = [
        row("snp-ancient", 40, 50, false),
        row("snp-mid", 10, 50, false),
        row("snp-new", 1, 50, false),
    ];
    let plan = plan_prune(&rows, policy(30, 100), now(), 50);
    assert_eq!(
        plan.prune,
        vec![
            action("snp-ancient", PruneReason::Age),
            action("snp-mid", PruneReason::DiskCap)
        ]
    );
    assert!(plan.fits);
}

#[test]
fn pinned_rows_are_never_pruned() {
    let rows = [
        row("snp-pinned-old", 400, 50, true),
        row("snp-pinned", 2, 50, true),
        row("snp-free", 1, 50, false),
    ];
    let plan = plan_prune(&rows, policy(30, 120), now(), 10);
    assert_eq!(plan.prune, vec![action("snp-free", PruneReason::DiskCap)]);
    assert!(plan.fits, "100 pinned + 10 incoming fits 120");
}

#[test]
fn when_only_pinned_rows_remain_the_disk_cap_actions_are_dropped_and_age_stays() {
    let rows = [
        row("snp-old", 40, 10, false),
        row("snp-free", 1, 30, false),
        row("snp-pinned", 2, 100, true),
    ];
    let plan = plan_prune(&rows, policy(30, 120), now(), 50);
    assert!(!plan.fits, "100 pinned + 50 incoming can't fit 120");
    assert_eq!(
        plan.prune,
        vec![action("snp-old", PruneReason::Age)],
        "evidence is never removed for nothing"
    );
    // The janitor's own pass (nothing incoming) over pinned-only usage.
    let plan = plan_prune(
        &[
            row("snp-pinned", 2, 200, true),
            row("snp-free", 1, 5, false),
        ],
        policy(30, 120),
        now(),
        0,
    );
    assert!(!plan.fits);
    assert!(plan.prune.is_empty());
    // Nothing at all fits trivially.
    let plan = plan_prune(&[], policy(30, 120), now(), 0);
    assert!(plan.fits && plan.prune.is_empty());
}

// --- Step 2: the store ----------------------------------------------------------------

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use farm3d_lib::attention::{
    repository as attention_repo, AttentionDetail, AttentionOrigin, AttentionSubject, Condition,
    ConditionKind,
};
use farm3d_lib::cameras::fetch::Frame;
use farm3d_lib::cameras::media::{self, CaptureLink, MediaStore, NewSnapshot, StoreOutcome};
use farm3d_lib::cameras::retention::{CaptureFault, MediaJanitor};
use farm3d_lib::cameras::{CameraContentType, CameraSnapshot};
use farm3d_lib::catalog::{BedShape, PrinterProfile};
use farm3d_lib::incidents::{repository as incidents_repo, IncidentEntryDetail, IncidentKind};
use farm3d_lib::jobs::PrinterSnapshot;
use farm3d_lib::persistence::{RepositoryError, Storage};

const PRINTER: &str = "prn-media";

/// A synthetic JPEG of `len` bytes (the magic, then filler).
fn jpeg(len: usize, at: DateTime<Utc>) -> Frame {
    let mut bytes = vec![0x5A; len.max(4)];
    bytes[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
    Frame {
        content_type: CameraContentType::Jpeg,
        bytes,
        captured_at: at,
    }
}

struct Store {
    _temp: tempfile::TempDir,
    _lease: farm3d_lib::persistence::MetadataRootLease,
    storage: Arc<Storage>,
    janitor: MediaJanitor,
}

impl Store {
    fn new() -> Self {
        let (temp, lease, storage, _) = common::storage();
        storage
            .write(|tx| {
                tx.execute_batch(&format!(
                    "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
                       catalog_variant, catalog_model_id, catalog_printer_variant, notes,
                       overrides_json, created_at, updated_at)
                     VALUES ('{PRINTER}', 1, 'Voron', '', '', '', '', '', '', '{{}}',
                             '{NOW_TEXT}', '{NOW_TEXT}');"
                ))?;
                Ok(())
            })
            .unwrap();
        Self {
            _temp: temp,
            _lease: lease,
            storage,
            janitor: MediaJanitor::default(),
        }
    }

    fn root(&self) -> &Path {
        self.storage.paths().media_root()
    }

    fn media(&self) -> MediaStore {
        MediaStore::for_storage(&self.storage)
    }

    async fn manual(
        &self,
        operation_id: &str,
        frame: &Frame,
        policy: RetentionPolicy,
    ) -> Result<StoreOutcome, RepositoryError> {
        let digest = format!("digest-{operation_id}");
        media::store_frame(
            &self.storage,
            &self.janitor,
            policy,
            now(),
            NewSnapshot {
                printer_id: PRINTER,
                link: CaptureLink::Manual {
                    operation_id,
                    digest: &digest,
                },
                frame,
            },
        )
        .await
    }

    async fn stored(
        &self,
        operation_id: &str,
        frame: &Frame,
        policy: RetentionPolicy,
    ) -> CameraSnapshot {
        match self.manual(operation_id, frame, policy).await.unwrap() {
            StoreOutcome::Stored { snapshot, .. } => snapshot,
            other => panic!("not stored: {other:?}"),
        }
    }

    /// A `printer.hostFailed` Incident on the Printer.
    fn incident(&self) -> String {
        self.storage
            .write_repo(|tx| {
                let event = attention_repo::insert(
                    tx,
                    &host_failed(),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;
                let incident = incidents_repo::open(
                    tx,
                    IncidentKind::PrinterHostFailed,
                    PRINTER,
                    None,
                    &printer_snapshot(),
                    &event.id,
                    now(),
                )?;
                Ok(incident.id)
            })
            .unwrap()
    }

    fn rel_path(&self, id: &str) -> String {
        self.storage
            .read(|conn| {
                conn.query_row(
                    "SELECT rel_path FROM camera_snapshots WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
            })
            .unwrap()
    }

    fn snapshot(&self, id: &str) -> CameraSnapshot {
        self.storage
            .read(|conn| Ok(media::load_snapshot(conn, id)))
            .unwrap()
            .unwrap()
            .expect("the row exists")
    }

    fn scalar(&self, sql: &str) -> i64 {
        self.storage
            .read(|conn| conn.query_row(sql, [], |row| row.get(0)))
            .unwrap()
    }

    /// Every file under the media root, relative, `/`-separated.
    fn files(&self) -> BTreeSet<String> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
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
        let mut out = BTreeSet::new();
        walk(self.root(), self.root(), &mut out);
        out
    }

    /// The unpruned rows' paths: exactly what should be on disk.
    fn unpruned_paths(&self) -> BTreeSet<String> {
        self.storage
            .read(|conn| {
                let mut statement =
                    conn.prepare("SELECT rel_path FROM camera_snapshots WHERE pruned_at IS NULL")?;
                let paths = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect();
                paths
            })
            .unwrap()
    }

    fn entry_kinds(&self, incident_id: &str) -> Vec<IncidentEntryDetail> {
        self.storage
            .read(|conn| Ok(incidents_repo::entries(conn, incident_id)))
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|entry| entry.detail)
            .collect()
    }
}

fn printer_snapshot() -> PrinterSnapshot {
    PrinterSnapshot {
        name: "Voron".to_string(),
        location: None,
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
            suggested_port: None,
        },
    }
}

fn host_failed() -> Condition {
    Condition {
        kind: ConditionKind::PrinterHostFailed,
        source_id: PRINTER.to_string(),
        printer_id: Some(PRINTER.to_string()),
        job_id: None,
        spool_id: None,
        requirement_id: None,
        subject: AttentionSubject {
            printer_name: Some("Voron".to_string()),
            printer_location: None,
            job_label: None,
            spool_number: None,
            spool_label: None,
        },
        detail: AttentionDetail::PrinterHostFailed,
        acknowledge: false,
    }
}

const ROOMY: RetentionPolicy = RetentionPolicy {
    retention_days: 30,
    cap_bytes: 1_000_000,
};

#[tokio::test]
async fn a_capture_writes_the_image_under_the_media_root_then_inserts_its_row() {
    let store = Store::new();
    assert!(store.root().ends_with("farm3d-media/v1"));
    let frame = jpeg(100, "2026-09-28T08:59:58.250Z".parse().unwrap());
    let snapshot = store.stored("op-1", &frame, ROOMY).await;
    assert!(snapshot.id.starts_with("snp-"));
    assert_eq!(
        snapshot.trigger,
        farm3d_lib::cameras::SnapshotTrigger::Manual
    );
    assert_eq!(snapshot.byte_len, 100);
    assert_eq!(snapshot.captured_at, "2026-09-28T08:59:58.250Z");
    assert_eq!(snapshot.sha256.len(), 64);
    let rel_path = store.rel_path(&snapshot.id);
    assert_eq!(rel_path, format!("snapshots/2026/09/{}.jpg", snapshot.id));
    assert_eq!(
        store.files(),
        BTreeSet::from([rel_path.clone()]),
        "tmp/ is empty"
    );
    assert_eq!(
        std::fs::read(store.root().join(&rel_path)).unwrap(),
        frame.bytes
    );
    let wire = serde_json::to_string(&snapshot).unwrap();
    assert!(
        !wire.contains("snapshots/") && !wire.contains("relPath"),
        "{wire}"
    );

    // A failure before commit (a reused operation id) unlinks the renamed
    // file and writes no row.
    store
        .storage
        .write_repo(|tx| {
            farm3d_lib::spools::operations::claim(
                tx,
                "op-taken",
                farm3d_lib::spools::operations::OperationKind::SetSnapshotPinned,
                "other",
            )?;
            Ok(())
        })
        .unwrap();
    let error = store.manual("op-taken", &frame, ROOMY).await.unwrap_err();
    assert!(
        matches!(error, RepositoryError::OperationIdReused),
        "{error:?}"
    );
    assert_eq!(store.scalar("SELECT COUNT(*) FROM camera_snapshots"), 1);
    assert_eq!(store.files(), BTreeSet::from([rel_path]));
}

#[tokio::test]
async fn the_sweep_deletes_an_orphan_from_a_crash_after_the_rename() {
    let store = Store::new();
    let kept = store.stored("op-kept", &jpeg(50, now()), ROOMY).await;
    // The crash: the image was renamed into place, the row never written.
    let orphan_id = media::new_snapshot_id();
    let orphan = MediaStore::rel_path(&orphan_id, now(), CameraContentType::Jpeg);
    store
        .media()
        .write_image(&orphan_id, &orphan, &jpeg(50, now()).bytes)
        .unwrap();
    // And a write interrupted before its rename.
    std::fs::write(store.root().join("tmp/snp-interrupted.part"), b"partial").unwrap();
    assert_eq!(store.files().len(), 3);

    let changes = media::startup_sweep(&store.storage, now()).unwrap();
    assert!(changes.is_empty(), "no row changed: {changes:?}");
    assert_eq!(store.files(), BTreeSet::from([store.rel_path(&kept.id)]));
    assert_eq!(store.snapshot(&kept.id).pruned_at, None);
}

#[tokio::test]
async fn the_sweep_deletes_a_pruned_file_whose_unlink_never_ran_and_the_row_stays_pruned() {
    let store = Store::new();
    let old = store
        .stored("op-old", &jpeg(60, now() - ChronoDuration::days(1)), ROOMY)
        .await;
    let new = store.stored("op-new", &jpeg(60, now()), ROOMY).await;
    let tight = RetentionPolicy {
        retention_days: 30,
        cap_bytes: 100,
    };
    // The crash: the prune committed, the unlink never ran.
    let commit = media::mark_prunable(&store.storage, tight, now()).unwrap();
    assert_eq!(commit.rel_paths, vec![store.rel_path(&old.id)]);
    assert_eq!(commit.changes.snapshots.len(), 1);
    let pruned = store.snapshot(&old.id);
    assert_eq!(pruned.prune_reason, Some(PruneReason::DiskCap));
    assert_eq!(pruned.revision, old.revision + 1);
    assert!(store.files().contains(&store.rel_path(&old.id)));

    media::startup_sweep(&store.storage, now()).unwrap();
    assert_eq!(store.files(), BTreeSet::from([store.rel_path(&new.id)]));
    let after = store.snapshot(&old.id);
    assert_eq!(after, pruned, "the row stays pruned, reason unchanged");
    assert_eq!(
        store.scalar("SELECT COUNT(*) FROM camera_snapshots"),
        2,
        "a pruned row stays"
    );
}

#[tokio::test]
async fn a_missing_file_becomes_pruned_missing_file_even_when_pinned() {
    let store = Store::new();
    let incident = store.incident();
    let linked = match media::store_frame(
        &store.storage,
        &store.janitor,
        ROOMY,
        now(),
        NewSnapshot {
            printer_id: PRINTER,
            link: CaptureLink::Incident {
                incident_id: &incident,
                job_id: None,
            },
            frame: &jpeg(40, now()),
        },
    )
    .await
    .unwrap()
    {
        StoreOutcome::Stored { snapshot, changes } => {
            assert_eq!(changes.incidents.len(), 1);
            assert_eq!(changes.incidents[0].snapshot_count, 1);
            snapshot
        }
        other => panic!("{other:?}"),
    };
    let pinned = store.stored("op-pinned", &jpeg(40, now()), ROOMY).await;
    media::set_pinned(&store.storage, "op-pin", &pinned.id, true, now()).unwrap();
    std::fs::remove_file(store.root().join(store.rel_path(&linked.id))).unwrap();
    std::fs::remove_file(store.root().join(store.rel_path(&pinned.id))).unwrap();

    let changes = media::startup_sweep(&store.storage, now()).unwrap();
    assert_eq!(changes.snapshots.len(), 2);
    assert_eq!(changes.incidents.len(), 1, "the linked Incident, once");
    let linked_after = store.snapshot(&linked.id);
    assert_eq!(linked_after.prune_reason, Some(PruneReason::MissingFile));
    let pinned_after = store.snapshot(&pinned.id);
    assert_eq!(pinned_after.prune_reason, Some(PruneReason::MissingFile));
    assert!(
        pinned_after.pinned_at.is_some(),
        "the pin stays; the image is gone either way"
    );
    let entries = store.entry_kinds(&incident);
    assert_eq!(
        entries.last().unwrap(),
        &IncidentEntryDetail::EvidencePruned {
            snapshot_id: linked.id.clone(),
            reason: PruneReason::MissingFile
        }
    );
    // A second sweep changes nothing.
    assert!(media::startup_sweep(&store.storage, now())
        .unwrap()
        .is_empty());
}

/// D5 "Capture order", pinned by crashes injected inside `store_frame`
/// itself: before the rename only a part in `tmp/` exists; after the
/// rename the image exists with no row. Either way the startup sweep
/// leaves no file without an unpruned row and no unpruned row without its
/// file.
#[tokio::test]
async fn a_crash_on_either_side_of_the_rename_leaves_what_the_sweep_repairs() {
    let store = Store::new();
    let kept = store.stored("op-kept", &jpeg(50, now()), ROOMY).await;
    let kept_path = store.rel_path(&kept.id);

    store
        .janitor
        .inject_capture_fault_once(CaptureFault::BeforeRename);
    assert!(store
        .manual("op-crash-before", &jpeg(60, now()), ROOMY)
        .await
        .is_err());
    let files = store.files();
    let parts: Vec<&String> = files
        .iter()
        .filter(|path| path.starts_with("tmp/"))
        .collect();
    assert_eq!(parts.len(), 1, "the fsynced part is left: {files:?}");
    assert!(parts[0].ends_with(".part"));
    assert_eq!(
        files
            .iter()
            .filter(|path| path.starts_with("snapshots/"))
            .collect::<Vec<_>>(),
        vec![&kept_path],
        "nothing was renamed"
    );
    assert_eq!(
        store.scalar("SELECT COUNT(*) FROM camera_snapshots"),
        1,
        "no row"
    );
    assert_eq!(
        store.scalar("SELECT COUNT(*) FROM operations WHERE id = 'op-crash-before'"),
        0
    );

    store
        .janitor
        .inject_capture_fault_once(CaptureFault::AfterRename);
    assert!(store
        .manual("op-crash-after", &jpeg(70, now()), ROOMY)
        .await
        .is_err());
    let files = store.files();
    let renamed: Vec<&String> = files
        .iter()
        .filter(|path| path.starts_with("snapshots/") && **path != kept_path)
        .collect();
    assert_eq!(renamed.len(), 1, "the image is in place: {files:?}");
    assert_eq!(
        store.scalar("SELECT COUNT(*) FROM camera_snapshots"),
        1,
        "but it has no row"
    );
    assert_eq!(
        store.scalar("SELECT COUNT(*) FROM operations WHERE id = 'op-crash-after'"),
        0
    );

    let changes = media::startup_sweep(&store.storage, now()).unwrap();
    assert!(changes.is_empty(), "no row changed: {changes:?}");
    assert_eq!(
        store.files(),
        BTreeSet::from([kept_path]),
        "the part and the orphan are gone"
    );
    assert_eq!(store.files(), store.unpruned_paths());
    // The ids were never claimed: a retry stores normally.
    store
        .stored("op-crash-after", &jpeg(70, now()), ROOMY)
        .await;
    assert_eq!(store.files(), store.unpruned_paths());
}

/// D5 "`MediaJanitor`": 20 captures and a pruning pass released together
/// by a barrier, five rounds over. The janitor lock is what keeps usage
/// under the cap: without it, captures plan against the same stale usage
/// and overshoot it (checked by mutation; see the Task 8 report).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn twenty_concurrent_captures_against_a_pruning_pass_keep_usage_under_the_cap() {
    let store = Arc::new(Store::new());
    let cap = RetentionPolicy {
        retention_days: 30,
        cap_bytes: 5 * 100,
    };
    // Something already over the retention age, for the pass to find.
    store
        .stored(
            "op-ancient",
            &jpeg(100, now() - ChronoDuration::days(40)),
            ROOMY,
        )
        .await;
    for round in 0..5_i64 {
        let barrier = Arc::new(tokio::sync::Barrier::new(21));
        let mut tasks = Vec::new();
        for index in 0..20_i64 {
            let capturing = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                let frame = jpeg(
                    100,
                    now() - ChronoDuration::seconds(200 - round * 20 - index),
                );
                barrier.wait().await;
                capturing
                    .stored(&format!("op-{round}-{index}"), &frame, cap)
                    .await;
            }));
        }
        let pruning = Arc::clone(&store);
        let prune_barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            prune_barrier.wait().await;
            media::prune_pass(&pruning.storage, &pruning.janitor, cap, now())
                .await
                .unwrap();
        }));
        for task in tasks {
            task.await.unwrap();
        }
        let used = store.scalar(
            "SELECT COALESCE(SUM(byte_len), 0) FROM camera_snapshots WHERE pruned_at IS NULL",
        );
        assert!(
            used <= cap.cap_bytes,
            "round {round}: {used} > {}",
            cap.cap_bytes
        );
        assert_eq!(used, 500, "round {round}: the cap is used, not starved");
        assert_eq!(
            store.files(),
            store.unpruned_paths(),
            "round {round}: no orphan and no dangling row"
        );
    }
    assert_eq!(
        store.scalar("SELECT COUNT(*) FROM camera_snapshots"),
        101,
        "every row stays"
    );
}

// --- Step 3: the capture triggers and the snapshot commands --------------------------

use std::time::Duration;

use common::fake_camera::{Answer, FakeCamera, JPEG};
use farm3d_lib::attention::services::{run_pass, AttentionTimings};
use farm3d_lib::attention::{AttentionEvent, EvidenceOutcome};
use farm3d_lib::cameras::services::CameraTimings;
use farm3d_lib::cameras::{CameraErrorKind, EvidenceSkipReason, SnapshotTrigger};
use farm3d_lib::connections::supervisor::PrinterSetupFacts;
use farm3d_lib::jobs::JobTimings;
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::StartSafety;
use p7_dispatch_rig::{
    boot_with_attention, status_of, AttentionBoot, Roots, Running, PRINTER as RIG_PRINTER,
};
use serde_json::{json, Value};

/// A query token in the camera's URL: it must never leave
/// `printer_cameras.snapshot_url` (global constraint 3).
const CAMERA_TOKEN: &str = "SEEDED-P8-MEDIA-CAMERA-TOKEN-0d3f";

struct CaptureRig {
    roots: Roots,
    app: Running,
    camera: FakeCamera,
}

impl CaptureRig {
    /// The dispatch rig with the capture runtime on, the Printer's camera
    /// on `FakeCamera` (answering `answer`), and the default alert
    /// defaults (both snapshot toggles on).
    fn new(answer: Answer) -> Self {
        Self::with_timings(answer, CameraTimings::default())
    }

    fn with_timings(answer: Answer, cameras: CameraTimings) -> Self {
        Self::prepared(answer, cameras, |_| {})
    }

    /// [`CaptureRig::with_timings`], with `prepare` run over the roots
    /// before the app (and its startup sweep) boots.
    fn prepared(answer: Answer, cameras: CameraTimings, prepare: impl FnOnce(&Roots)) -> Self {
        let roots = Roots::new(StartSafety::ConfirmBedClear);
        prepare(&roots);
        let camera = FakeCamera::start(answer);
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
                clock: None,
                cameras: Some(cameras),
                notifications: None,
                real_adapters: false,
            },
        );
        app.attention_pass();
        let url = camera.url(&format!("/snapshot?token={CAMERA_TOKEN}"));
        app.storage
            .write(|tx| {
                tx.execute(
                    "INSERT INTO printer_cameras(printer_id, source_kind, snapshot_url, updated_at) \
                     VALUES (?1, 'snapshotUrl', ?2, '2026-09-28T09:00:00.000Z')",
                    rusqlite::params![RIG_PRINTER, url],
                )?;
                Ok(())
            })
            .unwrap();
        Self { roots, app, camera }
    }

    /// Waits until the capture consumer has taken every committed change
    /// and no capture is running: every capture asked for so far is done.
    fn settle(&self) {
        let services = &self.app.services;
        self.app.wait_until("the captures settle", || {
            services.cameras.capture_rounds() >= services.attention.applied_sent()
                && services.cameras.captures_in_flight() == 0
        });
    }

    fn set_toggles(&self, on_incident: bool, on_completion: bool) {
        self.app
            .storage
            .write(|tx| {
                tx.execute(
                    "INSERT INTO printer_alert_defaults(printer_id, offline_after_minutes, notifications,
                       snapshot_on_incident, snapshot_on_completion, updated_at)
                     VALUES (?1, 5, 'follow', ?2, ?3, '2026-09-28T09:00:00.000Z')",
                    rusqlite::params![RIG_PRINTER, on_incident, on_completion],
                )?;
                Ok(())
            })
            .unwrap();
    }

    /// A Job fails on the fake; returns it and its Incident, once projected.
    fn fail_job(&self) -> (String, String) {
        let job = self.app.printing();
        self.roots.fake.finish_print("klippy_shutdown");
        self.app.wait_job(&job, "failed");
        let incident = || {
            self.app
                .attention_rows()
                .into_iter()
                .find(|event| {
                    event.condition == ConditionKind::JobFailed
                        && event.job_id.as_deref() == Some(job.as_str())
                })
                .and_then(|event| event.incident_id)
        };
        self.app
            .wait_until("the failure's Incident opens", || incident().is_some());
        let incident = incident().unwrap();
        (job, incident)
    }

    /// A Job completes on the fake; returns it and its `job.completed`
    /// Event, once projected.
    fn complete_job(&self) -> (String, AttentionEvent) {
        let job = self.app.printing();
        self.roots.fake.finish_print("completed");
        self.app.mirror(&self.roots.fake);
        self.app.wait_job(&job, "completed");
        self.app.wait_until("job.completed is projected", || {
            self.app.services.attention.poke();
            self.completed_event(&job).is_some()
        });
        let completed = self.completed_event(&job).unwrap();
        (job, completed)
    }

    fn completed_event(&self, job: &str) -> Option<AttentionEvent> {
        self.app.attention_rows().into_iter().find(|event| {
            event.condition == ConditionKind::JobCompleted && event.job_id.as_deref() == Some(job)
        })
    }

    fn entries(&self, incident_id: &str) -> Vec<IncidentEntryDetail> {
        self.app
            .storage
            .read(|conn| Ok(incidents_repo::entries(conn, incident_id)))
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|entry| entry.detail)
            .collect()
    }

    fn snapshots(&self) -> Vec<CameraSnapshot> {
        let ids: Vec<String> = self
            .app
            .storage
            .read(|conn| {
                let mut statement =
                    conn.prepare("SELECT id FROM camera_snapshots ORDER BY captured_at, id")?;
                let ids = statement.query_map([], |row| row.get(0))?.collect();
                ids
            })
            .unwrap();
        ids.iter()
            .map(|id| {
                self.app
                    .storage
                    .read(|conn| Ok(media::load_snapshot(conn, id)))
                    .unwrap()
                    .unwrap()
                    .unwrap()
            })
            .collect()
    }

    fn incident_row(&self, incident_id: &str) -> farm3d_lib::incidents::Incident {
        self.app
            .storage
            .read(|conn| Ok(incidents_repo::get(conn, incident_id)))
            .unwrap()
            .unwrap()
            .unwrap()
    }

    fn call(&self, command: &str, body: Value) -> Result<Value, Value> {
        self.app.call(command, body)
    }

    fn binary(&self, command: &str, mut body: Value) -> Result<(Value, Vec<u8>), Value> {
        body["contractVersion"] = json!(1);
        let response = tauri::test::get_ipc_response(
            &self.app.webview,
            tauri::webview::InvokeRequest {
                cmd: command.to_string(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        );
        match response {
            Ok(tauri::ipc::InvokeResponseBody::Raw(bytes)) => {
                let length = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
                let header: Value = serde_json::from_slice(&bytes[4..4 + length]).unwrap();
                Ok((header, bytes[4 + length..].to_vec()))
            }
            Ok(other) => panic!("{command} answered JSON: {other:?}"),
            Err(error) => Err(error),
        }
    }

    fn stream(&self, event_type: &str) -> Vec<Value> {
        self.app
            .events
            .lock()
            .unwrap()
            .iter()
            .map(|text| serde_json::from_str::<Value>(text).unwrap())
            .filter(|event| event["type"] == event_type)
            .collect()
    }

    /// Global constraint 3: the camera URL's token, the camera's endpoint,
    /// and the media store's paths never reach a row other than
    /// `printer_cameras.snapshot_url`, or any event.
    fn assert_clean(&self) {
        let endpoint = format!("127.0.0.1:{}", self.camera.port);
        let events = self.app.events.lock().unwrap().join("\n");
        for needle in [
            CAMERA_TOKEN,
            endpoint.as_str(),
            "snapshots/",
            "farm3d-media",
        ] {
            assert!(!events.contains(needle), "{needle:?} reached an event");
        }
        let rows: String = self
            .app
            .storage
            .read(|conn| {
                let mut text = String::new();
                for (table, columns) in [
                    ("camera_snapshots", "id || trigger || COALESCE(incident_id,'') || COALESCE(job_id,'') || sha256"),
                    ("incident_events", "detail_json"),
                    ("attention_events", "detail_json || COALESCE(evidence_json,'') || summary"),
                ] {
                    let mut statement = conn.prepare(&format!("SELECT {columns} FROM {table}"))?;
                    for value in statement.query_map([], |row| row.get::<_, String>(0))? {
                        text.push_str(&value?);
                        text.push('\n');
                    }
                }
                Ok(text)
            })
            .unwrap();
        for needle in [CAMERA_TOKEN, endpoint.as_str(), "http://"] {
            assert!(!rows.contains(needle), "{needle:?} was persisted");
        }
    }
}

fn timeline_kinds(entries: &[IncidentEntryDetail]) -> Vec<String> {
    entries
        .iter()
        .map(|detail| {
            serde_json::to_value(detail).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn an_incident_opening_captures_one_incident_snapshot_and_records_it() {
    let rig = CaptureRig::new(Answer::Jpeg);
    let (job, incident_id) = rig.fail_job();
    rig.settle();

    let snapshots = rig.snapshots();
    assert_eq!(snapshots.len(), 1, "{snapshots:?}");
    let snapshot = &snapshots[0];
    assert_eq!(snapshot.trigger, SnapshotTrigger::Incident);
    assert_eq!(snapshot.incident_id.as_deref(), Some(incident_id.as_str()));
    assert_eq!(snapshot.job_id.as_deref(), Some(job.as_str()));
    assert_eq!(snapshot.byte_len, JPEG.len() as i64);
    let entries = rig.entries(&incident_id);
    assert_eq!(
        entries.last().unwrap(),
        &IncidentEntryDetail::EvidenceCaptured {
            snapshot_id: snapshot.id.clone(),
            trigger: SnapshotTrigger::Incident
        }
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, IncidentEntryDetail::EvidenceCaptured { .. }))
            .count(),
        1
    );
    let incident = rig.incident_row(&incident_id);
    assert_eq!(incident.snapshot_count, 1);
    // Published after commit: the snapshot, and the Incident's final row.
    let published = rig.stream("attention.snapshot.changed");
    assert!(published
        .iter()
        .any(|event| event["payload"]["snapshot"]["id"] == json!(snapshot.id)));
    let last_incident = rig
        .stream("attention.incident.changed")
        .into_iter()
        .rfind(|event| event["subject"]["id"] == json!(incident_id))
        .unwrap();
    assert_eq!(
        last_incident["payload"]["incident"]["revision"],
        json!(incident.revision)
    );
    assert_eq!(last_incident["payload"]["incident"]["snapshotCount"], 1);
    // The capture was a saved-source fetch: the health is ok.
    assert_eq!(
        rig.app.services.cameras.health(RIG_PRINTER).unwrap().state,
        farm3d_lib::cameras::CameraHealthState::Ok
    );
    // The image is served back through snapshot_image.
    let (header, image) = rig
        .binary("snapshot_image", json!({"snapshotId": snapshot.id}))
        .unwrap();
    assert_eq!(image, JPEG);
    assert_eq!(header["snapshotId"], json!(snapshot.id));
    assert_eq!(header["contentType"], "image/jpeg");
    assert_eq!(
        timeline_kinds(&entries[..entries.len() - 1]),
        ["opened", "eventLinked"]
    );
    rig.assert_clean();
}

#[test]
fn a_failing_camera_records_evidence_skipped_and_the_incident_is_otherwise_identical() {
    let rig = CaptureRig::new(Answer::Status(503));
    let (_, incident_id) = rig.fail_job();
    rig.settle();
    assert!(rig.snapshots().is_empty());
    let entries = rig.entries(&incident_id);
    assert_eq!(
        entries.last().unwrap(),
        &IncidentEntryDetail::EvidenceSkipped {
            reason: EvidenceSkipReason::CameraError,
            error_kind: Some(CameraErrorKind::HttpStatus)
        }
    );
    // The same timeline as a captured one, but for the outcome.
    assert_eq!(
        timeline_kinds(&entries[..entries.len() - 1]),
        ["opened", "eventLinked"]
    );
    let incident = rig.incident_row(&incident_id);
    assert_eq!(incident.snapshot_count, 0);
    assert_eq!(incident.linked_event_ids.len(), 2);
    assert_eq!(incident.state, farm3d_lib::incidents::IncidentState::Open);
    assert_eq!(
        rig.app
            .services
            .cameras
            .health(RIG_PRINTER)
            .unwrap()
            .last_failure_kind,
        Some(CameraErrorKind::HttpStatus)
    );
    rig.assert_clean();
}

#[test]
fn with_snapshot_on_incident_off_nothing_is_captured_or_recorded() {
    let rig = CaptureRig::new(Answer::Jpeg);
    rig.set_toggles(false, true);
    let (_, incident_id) = rig.fail_job();
    rig.app.attention_pass();
    rig.settle();
    assert!(rig.snapshots().is_empty());
    assert_eq!(
        timeline_kinds(&rig.entries(&incident_id)),
        ["opened", "eventLinked"]
    );
    assert!(
        rig.camera.requests().is_empty(),
        "the camera was never asked"
    );
}

#[test]
fn a_job_completion_captures_a_completion_snapshot_recorded_on_its_event() {
    let rig = CaptureRig::new(Answer::Jpeg);
    let (job, _) = rig.complete_job();
    rig.settle();
    let completed = rig.completed_event(&job).unwrap();
    let snapshots = rig.snapshots();
    assert_eq!(snapshots.len(), 1, "{snapshots:?}");
    let snapshot = &snapshots[0];
    assert_eq!(snapshot.trigger, SnapshotTrigger::Completion);
    assert_eq!(snapshot.job_id.as_deref(), Some(job.as_str()));
    assert_eq!(snapshot.incident_id, None);
    assert_eq!(
        completed.evidence,
        Some(EvidenceOutcome::Captured {
            snapshot_id: snapshot.id.clone()
        })
    );
    // Evidence is its own field, never the planner-owned detail.
    assert_eq!(
        serde_json::to_value(&completed.detail).unwrap()["kind"],
        "jobCompleted"
    );
    // No new job_events kinds.
    let kinds: Vec<String> = rig.app.event_kinds(&job);
    assert!(
        kinds
            .iter()
            .all(|kind| !kind.to_lowercase().contains("snapshot")
                && !kind.to_lowercase().contains("evidence")),
        "{kinds:?}"
    );
    // The Event was published with its evidence.
    let published = rig
        .stream("attention.event.changed")
        .into_iter()
        .rfind(|event| event["subject"]["id"] == json!(completed.id))
        .unwrap();
    assert_eq!(
        published["payload"]["event"]["evidence"]["status"],
        "captured"
    );
    // A later pass leaves the evidence in place.
    rig.app.attention_pass();
    rig.app.attention_pass();
    let after = rig.completed_event(&job).unwrap();
    assert_eq!(after.evidence, completed.evidence);
    assert_eq!(after.revision, completed.revision);
    rig.assert_clean();
}

/// Carry 1: the capture consumer's receiver has room for 64 passes. When
/// it lags, it re-derives what it missed from storage and captures it.
#[test]
fn a_lagged_capture_consumer_rederives_the_captures_it_missed() {
    let rig = CaptureRig::new(Answer::Jpeg);
    let services = &rig.app.services;
    services.cameras.hold_captures();
    let (_, incident_id) = rig.fail_job();
    // Far past the hand-off's capacity while the consumer is held: each
    // pass opens or resolves a connection error.
    for _ in 0..40 {
        tauri::async_runtime::block_on(rig.app.manager.report_error(
            RIG_PRINTER,
            "The Printer answered in a way farm3d couldn't read.",
            PrinterSetupFacts::complete(),
        ));
        run_pass(services).unwrap();
        rig.app.status(OperationalState::Ready);
        run_pass(services).unwrap();
    }
    let lags = services.cameras.capture_lags();
    services.cameras.release_captures();
    rig.settle();
    assert!(
        services.cameras.capture_lags() > lags,
        "the consumer lagged"
    );
    let entries = rig.entries(&incident_id);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, IncidentEntryDetail::EvidenceCaptured { .. }))
            .count(),
        1,
        "{entries:?}"
    );
    assert_eq!(rig.snapshots().len(), 1);
}

#[test]
fn the_snapshot_commands_capture_pin_serve_and_count() {
    let rig = CaptureRig::new(Answer::Jpeg);
    let (_, incident_id) = rig.fail_job();
    rig.settle();
    let incident_snapshot = rig.snapshots().remove(0);

    // capture_snapshot: a manual snapshot; a replay never touches the camera.
    let manual = rig
        .call(
            "capture_snapshot",
            json!({"operationId": "op-cap", "printerId": RIG_PRINTER}),
        )
        .unwrap();
    assert_eq!(manual["trigger"], "manual");
    assert_eq!(manual["incidentId"], Value::Null);
    assert!(manual.get("relPath").is_none());
    let requests = rig.camera.requests().len();
    let replay = rig
        .call(
            "capture_snapshot",
            json!({"operationId": "op-cap", "printerId": RIG_PRINTER}),
        )
        .unwrap();
    assert_eq!(replay, manual);
    assert_eq!(
        rig.camera.requests().len(),
        requests,
        "a replay fetches nothing"
    );
    let reused = rig
        .call(
            "capture_snapshot",
            json!({"operationId": "op-cap", "printerId": "prn-other"}),
        )
        .unwrap_err();
    assert_eq!(
        (
            reused["code"].as_str(),
            reused["details"]["fieldPath"].as_str()
        ),
        (Some("VALIDATION"), Some("operationId"))
    );
    assert_eq!(
        rig.call(
            "capture_snapshot",
            json!({"operationId": "op-missing", "printerId": "prn-missing"})
        )
        .unwrap_err()["code"],
        "NOT_FOUND"
    );

    // set_snapshot_pinned: idempotent, and a timeline row when linked.
    let pin = |operation: &str, id: &str, pinned: bool| {
        rig.call(
            "set_snapshot_pinned",
            json!({"operationId": operation, "snapshotId": id, "pinned": pinned}),
        )
    };
    let before = rig.entries(&incident_id).len();
    let pinned = pin("op-pin", &incident_snapshot.id, true).unwrap();
    assert!(pinned["pinnedAt"].is_string());
    assert_eq!(pinned["revision"], json!(incident_snapshot.revision + 1));
    assert_eq!(
        rig.entries(&incident_id).last().unwrap(),
        &IncidentEntryDetail::EvidencePinned {
            snapshot_id: incident_snapshot.id.clone()
        }
    );
    let published = rig.stream("attention.snapshot.changed").len();
    assert_eq!(
        pin("op-pin", &incident_snapshot.id, true).unwrap(),
        pinned,
        "a replay"
    );
    assert_eq!(
        pin("op-pin-again", &incident_snapshot.id, true).unwrap(),
        pinned,
        "the same state is a no-op"
    );
    assert_eq!(rig.entries(&incident_id).len(), before + 1);
    assert_eq!(
        rig.stream("attention.snapshot.changed").len(),
        published,
        "nothing published"
    );
    // An unlinked (manual) snapshot pins with no timeline row anywhere.
    let manual_id = manual["id"].as_str().unwrap();
    assert!(pin("op-pin-manual", manual_id, true).unwrap()["pinnedAt"].is_string());
    assert_eq!(rig.entries(&incident_id).len(), before + 1);

    // list_snapshots and media_usage.
    let listed = rig
        .call("list_snapshots", json!({"printerId": RIG_PRINTER}))
        .unwrap();
    assert_eq!(listed["snapshots"].as_array().unwrap().len(), 2);
    assert_eq!(listed["nextCursor"], Value::Null);
    let page = rig.call("list_snapshots", json!({"limit": 1})).unwrap();
    assert_eq!(page["snapshots"].as_array().unwrap().len(), 1);
    let next = rig
        .call(
            "list_snapshots",
            json!({"limit": 1, "before": page["nextCursor"]}),
        )
        .unwrap();
    assert_ne!(next["snapshots"][0]["id"], page["snapshots"][0]["id"]);
    let by_incident = rig
        .call("list_snapshots", json!({"incidentId": incident_id}))
        .unwrap();
    assert_eq!(
        by_incident["snapshots"][0]["id"],
        json!(incident_snapshot.id)
    );
    assert_eq!(
        rig.call("list_snapshots", json!({"incidentId": "inc-missing"}))
            .unwrap_err()["code"],
        "NOT_FOUND"
    );
    assert_eq!(
        rig.call("list_snapshots", json!({"before": "nonsense"}))
            .unwrap_err()["code"],
        "VALIDATION"
    );
    let usage = rig.call("media_usage", json!({})).unwrap();
    assert_eq!(usage["usedBytes"], json!(2 * JPEG.len()));
    assert_eq!(usage["pinnedBytes"], json!(2 * JPEG.len()));
    assert_eq!(usage["snapshotCount"], 2);
    assert_eq!(usage["pinnedCount"], 2);
    assert_eq!(usage["prunedCount"], 0);
    assert_eq!(usage["retentionDays"], 30);
    assert_eq!(usage["capBytes"], json!(2048_i64 * 1024 * 1024));

    // A missing image: snapshot_image marks it pruned (missingFile).
    let rel_path: String = rig
        .app
        .storage
        .read(|conn| {
            conn.query_row(
                "SELECT rel_path FROM camera_snapshots WHERE id = ?1",
                [&incident_snapshot.id],
                |row| row.get(0),
            )
        })
        .unwrap();
    std::fs::remove_file(rig.app.storage.paths().media_root().join(rel_path)).unwrap();
    let missing = rig
        .binary(
            "snapshot_image",
            json!({"snapshotId": incident_snapshot.id}),
        )
        .unwrap_err();
    assert_eq!(missing["code"], "EVIDENCE_PRUNED");
    assert_eq!(
        missing["details"],
        json!({"snapshotId": incident_snapshot.id, "reason": "missingFile"})
    );
    assert_eq!(
        rig.entries(&incident_id).last().unwrap(),
        &IncidentEntryDetail::EvidencePruned {
            snapshot_id: incident_snapshot.id.clone(),
            reason: PruneReason::MissingFile
        }
    );
    let again = rig
        .binary(
            "snapshot_image",
            json!({"snapshotId": incident_snapshot.id}),
        )
        .unwrap_err();
    assert_eq!(again["code"], "EVIDENCE_PRUNED");
    // Pinning a pruned row is refused (and burns no id); unpinning it works.
    let refused = pin("op-pin-pruned", &incident_snapshot.id, true).unwrap_err();
    assert_eq!(refused["code"], "EVIDENCE_PRUNED");
    let unpinned = pin("op-unpin", &incident_snapshot.id, false).unwrap();
    assert_eq!(unpinned["pinnedAt"], Value::Null);
    assert_eq!(unpinned["pruneReason"], "missingFile");
    assert_eq!(
        rig.entries(&incident_id).last().unwrap(),
        &IncidentEntryDetail::EvidenceUnpinned {
            snapshot_id: incident_snapshot.id.clone()
        }
    );
    assert_eq!(
        pin("op-unpin-again", &incident_snapshot.id, false).unwrap(),
        unpinned,
        "unpinning twice is a no-op"
    );
    assert_eq!(
        rig.binary("snapshot_image", json!({"snapshotId": "snp-missing"}))
            .unwrap_err()["code"],
        "NOT_FOUND"
    );
    let usage = rig.call("media_usage", json!({})).unwrap();
    assert_eq!(usage["usedBytes"], json!(JPEG.len()));
    assert_eq!(usage["prunedCount"], 1);
    rig.assert_clean();
}

/// D5: when only pinned snapshots are left to prune, `capture_snapshot` is
/// `SNAPSHOT_DISK_CAP` and an Incident's capture records
/// `evidenceSkipped { reason: diskCap }`.
#[test]
fn a_full_cap_of_pinned_evidence_refuses_a_capture() {
    let rig = CaptureRig::new(Answer::Jpeg);
    farm3d_lib::settings::repository::SettingsRepository::new(Arc::clone(&rig.app.storage))
        .ensure_default()
        .unwrap();
    let ten_mib: i64 = 10 * 1024 * 1024;
    rig.app
        .storage
        .write(|tx| {
            tx.execute("UPDATE settings SET snapshot_disk_cap_mb = 100", [])?;
            for index in 0..10 {
                tx.execute(
                    "INSERT INTO camera_snapshots(id, printer_id, trigger, operation_id, captured_at,
                       content_type, byte_len, sha256, rel_path, pinned_at)
                     VALUES (?1, ?2, 'manual', ?3, '2026-09-28T08:00:00.000Z', 'image/jpeg', ?4, ?5, ?6,
                             '2026-09-28T08:00:00.000Z')",
                    rusqlite::params![
                        format!("snp-pinned-{index}"),
                        RIG_PRINTER,
                        format!("op-seed-{index}"),
                        ten_mib,
                        "e".repeat(64),
                        format!("snapshots/2026/09/snp-pinned-{index}.jpg"),
                    ],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let refused = rig
        .call(
            "capture_snapshot",
            json!({"operationId": "op-full", "printerId": RIG_PRINTER}),
        )
        .unwrap_err();
    assert_eq!(refused["code"], "SNAPSHOT_DISK_CAP");
    assert_eq!(
        refused["details"],
        json!({"usedBytes": 10 * ten_mib, "capBytes": 100 * 1024 * 1024, "pinnedBytes": 10 * ten_mib})
    );
    // The rejected command burned no id and stored nothing.
    let unclaimed: i64 = rig
        .app
        .storage
        .read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM operations WHERE id = 'op-full'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(unclaimed, 0);
    assert_eq!(rig.snapshots().len(), 10);
    assert!(rig
        .app
        .storage
        .paths()
        .media_root()
        .join("snapshots")
        .read_dir()
        .map_or(true, |mut dir| dir.next().is_none()));

    let (_, incident_id) = rig.fail_job();
    rig.settle();
    assert_eq!(
        rig.entries(&incident_id).last().unwrap(),
        &IncidentEntryDetail::EvidenceSkipped {
            reason: EvidenceSkipReason::DiskCap,
            error_kind: None
        }
    );
    assert_eq!(rig.snapshots().len(), 10);
}

/// D5 "`MediaJanitor`": a poke (what `save_settings` / `import_settings`
/// call when the retention changes) runs a prune pass under the stored
/// settings; the pruned row stays, its file goes, and it is published.
#[test]
fn a_janitor_poke_prunes_under_the_stored_retention() {
    let rig = CaptureRig::new(Answer::Jpeg);
    rig.app.wait_until("the janitor's first pass", || {
        rig.app.services.cameras.janitor().passes() >= 1
    });
    let manual = rig
        .call(
            "capture_snapshot",
            json!({"operationId": "op-old", "printerId": RIG_PRINTER}),
        )
        .unwrap();
    let id = manual["id"].as_str().unwrap().to_string();
    // Three days old, then a one-day retention.
    farm3d_lib::settings::repository::SettingsRepository::new(Arc::clone(&rig.app.storage))
        .ensure_default()
        .unwrap();
    rig.app
        .storage
        .write(|tx| {
            tx.execute(
                "UPDATE camera_snapshots SET captured_at = ?2 WHERE id = ?1",
                rusqlite::params![
                    id,
                    (Utc::now() - ChronoDuration::days(3))
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                ],
            )?;
            tx.execute("UPDATE settings SET snapshot_retention_days = 1", [])?;
            Ok(())
        })
        .unwrap();
    let rel_path: String = rig
        .app
        .storage
        .read(|conn| {
            conn.query_row(
                "SELECT rel_path FROM camera_snapshots WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
        })
        .unwrap();
    let file = rig.app.storage.paths().media_root().join(rel_path);
    assert!(file.exists());

    let passes = rig.app.services.cameras.janitor().passes();
    rig.app.services.cameras.janitor().poke();
    rig.app.wait_until("the poked pass", || {
        rig.app.services.cameras.janitor().passes() > passes
    });
    let pruned = rig
        .snapshots()
        .into_iter()
        .find(|snapshot| snapshot.id == id)
        .unwrap();
    assert_eq!(pruned.prune_reason, Some(PruneReason::Age));
    assert!(!file.exists(), "the file was unlinked after commit");
    assert!(rig
        .stream("attention.snapshot.changed")
        .iter()
        .any(|event| event["payload"]["snapshot"]["id"] == json!(id)
            && event["payload"]["snapshot"]["pruneReason"] == "age"));
    assert_eq!(
        rig.binary("snapshot_image", json!({"snapshotId": id}))
            .unwrap_err()["details"]["reason"],
        "age"
    );
}

/// Global constraint 5: a media root the startup sweep can't clean (here
/// `tmp` is a regular file where the sweep needs a directory) never blocks
/// startup. Until a later sweep succeeds, an Incident's capture records
/// `evidenceSkipped { reason: storage }` without touching the camera, and
/// `capture_snapshot` is `PERSISTENCE_UNAVAILABLE`. Once the entry is
/// repaired, the janitor's next pass sweeps again and captures resume.
#[test]
fn a_failed_media_sweep_degrades_capture_instead_of_blocking_startup() {
    let rig = CaptureRig::prepared(Answer::Jpeg, CameraTimings::default(), |roots| {
        std::fs::write(roots.paths.media_root().join("tmp"), b"not a directory").unwrap();
    });
    let services = &rig.app.services;
    assert!(!services.cameras.media_available(), "the sweep failed");
    // The app is up and serving: the core flows work.
    rig.app.ok("printer_statuses", json!({}));

    let refused = rig
        .call(
            "capture_snapshot",
            json!({"operationId": "op-down", "printerId": RIG_PRINTER}),
        )
        .unwrap_err();
    assert_eq!(refused["code"], "PERSISTENCE_UNAVAILABLE");
    let refused_text = refused.to_string();
    assert!(
        !refused_text.contains("farm3d-media") && !refused_text.contains("tmp"),
        "{refused_text}"
    );

    let (_, incident_id) = rig.fail_job();
    rig.settle();
    assert_eq!(
        rig.entries(&incident_id).last().unwrap(),
        &IncidentEntryDetail::EvidenceSkipped {
            reason: EvidenceSkipReason::Storage,
            error_kind: None
        }
    );
    assert!(rig.camera.requests().is_empty(), "nothing was fetched");
    assert!(rig.snapshots().is_empty());

    // Repaired: the janitor's next pass sweeps again and capture resumes.
    std::fs::remove_file(rig.app.storage.paths().media_root().join("tmp")).unwrap();
    let passes = services.cameras.janitor().passes();
    services.cameras.janitor().poke();
    rig.app.wait_until("the janitor retries the sweep", || {
        services.cameras.janitor().passes() > passes
    });
    assert!(services.cameras.media_available());
    let stored = rig
        .call(
            "capture_snapshot",
            json!({"operationId": "op-up", "printerId": RIG_PRINTER}),
        )
        .unwrap();
    assert_eq!(stored["trigger"], "manual");
    rig.assert_clean();
}

/// A startup sweep error reaches `apply_startup_sweep` as an error value,
/// never a panic or a startup failure.
#[test]
fn the_startup_sweep_reports_an_unusable_media_root_as_an_error() {
    let store = Store::new();
    std::fs::write(store.root().join("tmp"), b"not a directory").unwrap();
    assert!(media::startup_sweep(&store.storage, now()).is_err());
}
