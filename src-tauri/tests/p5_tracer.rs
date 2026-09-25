//! P5 Task 16: the end-to-end tracer through the Tauri IPC path (spec
//! acceptance criterion 16, plus a restart mid-run).
//!
//! 1. Import `orca-two-plates.3mf` (managed) and `orca-cube.gcode`.
//! 2. Create a Preparation for the two-plate Model, seeded with two
//!    plates, on the test catalog's profile, and slice both plates: two
//!    revisions with distinct plate keys and one source revision.
//! 3. Create an external revision from the G-code with the nozzle
//!    confirmed and the Printer Profile, material, and filament absent: it
//!    requires manual Printer selection.
//! 4. Record every revision's record JSON, its stored rows, and the
//!    SHA-256 of every blob read back through `open_verified`.
//! 5. Restart over the same roots: everything recorded is byte-identical,
//!    and no operation is `queued` or `running`.
//! 6. Start a slice and restart while it runs. The old app's worker is
//!    held once its engine exits, as a dead farm3d's would be, so only
//!    recovery cleans up: the engine is stopped, the operation is
//!    `interrupted`, its work directory is gone, nothing new is stored
//!    when the old worker is let go, and the step-4 record is unchanged.
//!
//! `tracer_runs_against_fake_orca` runs in CI; step 6 uses fake-orca's
//! `hang` scenario. `real_orca_tracer_runs_against_a_real_orcaslicer` is
//! the same tracer against the OrcaSlicer in `FARM3D_ORCA`, run by
//! `just test-orca`; its step 6 slices a dense generated sphere, so the
//! slice is still running when the restart comes.
//!
//! Needs `--features test-support`, which builds `fake-orca`.

mod common;
mod p5_harness;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use farm3d_lib::slicing::invocation::WORK_ROOT_DIR;
use farm3d_lib::slicing::operations::SchedulerPoint;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use p5_harness::*;

/// Which engine a tracer run drives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Engine {
    Fake,
    Real,
}

/// Everything step 4 records, compared byte for byte after each restart.
#[derive(PartialEq, Eq, Debug)]
struct Recorded {
    /// `get_slice_revision`'s JSON, by revision id.
    records: BTreeMap<String, String>,
    /// `list_slice_revisions`' JSON, by Model id.
    lists: BTreeMap<String, String>,
    /// The `slice_revisions` and `slice_revision_blobs` rows, every column.
    rows: Vec<String>,
    /// By revision id: the SHA-256 of each blob's bytes as `open_verified`
    /// returns them, by role (`gcode` plus the `slice_revision_blobs`
    /// roles).
    blob_hashes: BTreeMap<String, BTreeMap<String, String>>,
}

/// Every column of every row `sql` returns, one string per row.
fn rows(running: &Running, sql: &str) -> Vec<String> {
    running
        .services
        .storage
        .read(|connection| {
            let mut statement = connection.prepare(sql)?;
            let columns = statement.column_count();
            let rows = statement.query_map([], |row| {
                (0..columns)
                    .map(|index| row.get::<_, rusqlite::types::Value>(index))
                    .collect::<Result<Vec<_>, _>>()
                    .map(|values| format!("{values:?}"))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Step 4: the record of every revision of `model_ids`.
fn record(running: &Running, model_ids: &[&str]) -> Recorded {
    let mut records = BTreeMap::new();
    let mut lists = BTreeMap::new();
    let mut blob_hashes = BTreeMap::new();
    for model_id in model_ids {
        let list = running.ok("list_slice_revisions", json!({ "modelId": model_id }));
        for summary in list.as_array().unwrap() {
            let id = summary["id"].as_str().unwrap().to_string();
            let revision = running.ok("get_slice_revision", json!({ "sliceRevisionId": id }));
            records.insert(id.clone(), serde_json::to_string(&revision).unwrap());

            let mut hashes = BTreeMap::new();
            let gcode = running.gcode_sha256(&id);
            hashes.insert("gcode".to_string(), sha256_hex(&running.blob(&gcode)));
            let blobs = running
                .services
                .storage
                .read(|connection| {
                    let mut statement = connection.prepare(
                        "SELECT role, sha256 FROM slice_revision_blobs \
                         WHERE revision_id = ?1 ORDER BY role",
                    )?;
                    let rows = statement.query_map([&id], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?;
                    rows.collect::<Result<Vec<_>, _>>()
                })
                .unwrap();
            for (role, sha256) in blobs {
                let bytes = running.blob(&sha256);
                assert_eq!(sha256_hex(&bytes), sha256, "{id} {role} verifies");
                hashes.insert(role, sha256_hex(&bytes));
            }
            blob_hashes.insert(id, hashes);
        }
        lists.insert(model_id.to_string(), serde_json::to_string(&list).unwrap());
    }
    Recorded {
        records,
        lists,
        rows: [
            rows(running, "SELECT * FROM slice_revisions ORDER BY id"),
            rows(
                running,
                "SELECT * FROM slice_revision_blobs ORDER BY revision_id, role",
            ),
        ]
        .concat(),
        blob_hashes,
    }
}

/// How many operations are `queued` or `running`, straight from the table.
fn active_operations(running: &Running) -> i64 {
    running
        .services
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM slice_operations WHERE state IN ('queued', 'running')",
                [],
                |row| row.get(0),
            )
        })
        .unwrap()
}

/// The committed D8 argument vector for this platform
/// (`just gen-slicing-fixtures`).
fn committed_arguments() -> Value {
    let vectors: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/slicing/argument-vectors.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let platform = if cfg!(target_os = "linux") {
        "linux"
    } else {
        "otherPlatforms"
    };
    vectors[platform].clone()
}

/// A closed UV sphere of `rings × segments × 2` triangles and the given
/// radius, as a binary STL: enough geometry that a real OrcaSlicer spends
/// seconds loading it (spike Gate E: a 1 M-triangle sphere sat in the
/// loading phase for 6.6 s).
fn write_dense_sphere(path: &Path, rings: u32, segments: u32, radius: f32) {
    let point = |ring: u32, segment: u32| -> [f32; 3] {
        let polar = std::f32::consts::PI * ring as f32 / rings as f32;
        let azimuth = std::f32::consts::TAU * segment as f32 / segments as f32;
        [
            radius * polar.sin() * azimuth.cos(),
            radius * polar.sin() * azimuth.sin(),
            radius * polar.cos(),
        ]
    };
    let mut triangles: Vec<[[f32; 3]; 3]> = Vec::new();
    for ring in 0..rings {
        for segment in 0..segments {
            let next = (segment + 1) % segments;
            let (a, b) = (point(ring, segment), point(ring, next));
            let (c, d) = (point(ring + 1, segment), point(ring + 1, next));
            if ring > 0 {
                triangles.push([a, c, b]);
            }
            if ring + 1 < rings {
                triangles.push([b, c, d]);
            }
        }
    }
    let mut bytes = vec![0u8; 80];
    bytes.extend_from_slice(&(triangles.len() as u32).to_le_bytes());
    for triangle in &triangles {
        bytes.extend_from_slice(&[0u8; 12]);
        for vertex in triangle {
            for coordinate in vertex {
                bytes.extend_from_slice(&coordinate.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&[0u8; 2]);
    }
    fs::write(path, bytes).unwrap();
}

/// The six tracer steps against `farm`'s engine.
fn run_tracer(farm: &Farm, engine: Engine) {
    let first = farm.start();
    if engine == Engine::Real {
        let status = first.ok("check_slicer_runtime", json!({}));
        assert_eq!(status["canSlice"], true, "{status}");
    }

    // 1. Import the two-plate 3MF (managed) and the G-code.
    let two_plates = first.import(
        &farm.source("orca-two-plates.3mf", "two-plates.3mf"),
        "managed",
    );
    let gcode = first.import(&farm.source("orca-cube.gcode", "cube.gcode"), "managed");
    let two_plates_id = two_plates["id"].as_str().unwrap().to_string();
    let gcode_id = gcode["id"].as_str().unwrap().to_string();
    let source_revision = two_plates["currentRevision"]["id"].clone();

    // 2. A Preparation seeded with two plates, on the catalog profile
    //    target; slice both plates.
    let preparation = first.prepare(&two_plates_id);
    assert_eq!(
        preparation["document"]["target"],
        json!({ "kind": "profile", "catalogRef": common::a_ref_json() })
    );
    let seeded = plates(&preparation);
    assert_eq!(seeded.len(), 2, "one plate per Orca plate: {preparation}");
    let operations = first.start("tracer-slice-all", &preparation, &[&seeded[0], &seeded[1]]);
    assert_eq!(operations.len(), 2);
    let succeeded: Vec<Value> = ids(&operations)
        .iter()
        .map(|id| {
            let operation = wait_for_terminal(&first, id, engine);
            assert_eq!(operation["state"], "succeeded", "{operation}");
            operation
        })
        .collect();
    let revisions = first.ok("list_slice_revisions", json!({ "modelId": two_plates_id }));
    let revisions = revisions.as_array().unwrap();
    assert_eq!(revisions.len(), 2, "{revisions:?}");
    assert_ne!(
        revisions[0]["plate"]["plateKey"], revisions[1]["plate"]["plateKey"],
        "distinct plate keys"
    );
    assert!(revisions
        .iter()
        .all(|revision| revision["sourceRevisionId"] == source_revision
            && revision["kind"] == "farm3d"));
    let mut plate_keys: Vec<&Value> = seeded.iter().map(|plate| &plate["plateKey"]).collect();
    let mut sliced_keys: Vec<&Value> = succeeded
        .iter()
        .map(|operation| &operation["plateKey"])
        .collect();
    plate_keys.sort_by_key(|key| key.to_string());
    sliced_keys.sort_by_key(|key| key.to_string());
    assert_eq!(plate_keys, sliced_keys, "each plate sliced once");
    // The manifest records the committed D8 argument vector.
    for revision in revisions {
        let manifest_sha256: String = first
            .services
            .storage
            .read(|connection| {
                connection.query_row(
                    "SELECT sha256 FROM slice_revision_blobs \
                     WHERE revision_id = ?1 AND role = 'manifest'",
                    [revision["id"].as_str().unwrap()],
                    |row| row.get(0),
                )
            })
            .unwrap();
        let manifest: Value = serde_json::from_slice(&first.blob(&manifest_sha256)).unwrap();
        assert_eq!(manifest["arguments"], committed_arguments());
    }

    // 3. An external revision with the nozzle confirmed, the rest absent.
    let external = first.create_external(
        "tracer-external",
        gcode["currentRevision"]["id"].as_str().unwrap(),
        json!({
            "printerProfile": { "kind": "absent" },
            "nozzleDiameterMm": { "kind": "confirmed", "value": 0.4 },
            "materialFamily": { "kind": "absent" },
            "filamentDiameterMm": { "kind": "absent" },
        }),
    );
    assert_eq!(external["kind"], "external");
    assert_eq!(external["requiresManualPrinterSelection"], true);
    assert_eq!(
        external["facts"]["nozzleDiameterMm"],
        json!({ "value": 0.4, "provenance": "operatorConfirmed" })
    );
    for absent in ["printerProfile", "materialFamily", "filamentDiameterMm"] {
        assert_eq!(
            external["facts"][absent]["provenance"], "absent",
            "{absent}"
        );
    }

    // 4. Record every revision.
    let models = [two_plates_id.as_str(), gcode_id.as_str()];
    let recorded = record(&first, &models);
    assert_eq!(recorded.records.len(), 3);

    // 5. Restart over the same roots.
    wait_for("the scheduler to go idle", || {
        first.services.slicing.scheduler_idle().then_some(())
    });
    drop(first);
    let (recovery, second) = farm.restart();
    assert!(recovery.interrupted.is_empty(), "{recovery:?}");
    assert_eq!(recovery.work_dirs_removed, 0);
    assert_eq!(record(&second, &models), recorded, "unchanged by a restart");
    assert_eq!(active_operations(&second), 0);
    assert!(second.slicing()["activeAndRecentOperations"]
        .as_array()
        .unwrap()
        .iter()
        .all(|operation| operation["state"] != "running"));

    // 6. Start a slice and restart while it runs.
    let (preparation, plate) = match engine {
        Engine::Fake => {
            second.scenario(&[("FAKE_ORCA_SCENARIO", "hang")]);
            let preparation = second.prepare(&two_plates_id);
            let plate = plates(&preparation).remove(0);
            (preparation, plate)
        }
        Engine::Real => {
            let sphere = farm.sources.path().join("dense-sphere.stl");
            write_dense_sphere(&sphere, 500, 1000, 40.0);
            let model = second.import(&sphere, "managed");
            let preparation = second.prepare(model["id"].as_str().unwrap());
            let plate = plates(&preparation).remove(0);
            (preparation, plate)
        }
    };
    // The old app's worker is held once its engine exits, as if that
    // farm3d had died mid-run: only recovery may clean up after it.
    let (exited, on_exit) = mpsc::channel::<String>();
    let (release, released) = mpsc::channel::<()>();
    let (exited, released) = (Mutex::new(exited), Mutex::new(released));
    second
        .services
        .slicing
        .set_scheduler_hook(Some(Arc::new(move |point| {
            if let SchedulerPoint::Exited(id) = point {
                exited.lock().unwrap().send(id.to_string()).unwrap();
                released.lock().unwrap().recv().unwrap();
            }
        })));
    let stored_before = second.stored_counts();
    let mid_run = ids(&second.start("tracer-mid-run", &preparation, &[&plate])).remove(0);
    second.wait_state(&mid_run, "running");
    let work = second.work_dir(&mid_run);
    assert!(work.exists());
    let engine_pid = second.pid(&mid_run);
    // A real AppImage starts as a `/bin/sh libexec/orca-slicer-env` wrapper
    // that then execs `orca-slicer`; recovery recognises only the latter
    // (still_running_engine), so the restart waits for it.
    wait_for("the engine to be OrcaSlicer itself", || {
        fs::read_link(format!("/proc/{engine_pid}/exe"))
            .ok()
            .filter(|exe| exe.file_name().is_some_and(|name| name == "orca-slicer"))
    });

    let (recovery, third) = farm.restart();
    assert_eq!(recovery.interrupted.len(), 1, "{recovery:?}");
    assert_eq!(recovery.interrupted[0].id, mid_run);
    assert!(recovery.interrupted[0].was_running);
    assert_eq!(
        recovery.stopped,
        vec![mid_run.clone()],
        "the engine was ours"
    );
    assert!(!process_alive(engine_pid), "recovery stopped the engine");
    assert_eq!(
        on_exit.recv_timeout(Duration::from_secs(30)).unwrap(),
        mid_run,
        "the old worker saw its engine end"
    );
    assert_eq!(recovery.work_dirs_removed, 1, "{recovery:?}");
    assert!(!work.exists(), "recovery removed the work directory");
    let work_root_is_empty = |running: &Running| {
        running
            .services
            .storage
            .paths()
            .content_root()
            .join(WORK_ROOT_DIR)
            .read_dir()
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true)
    };
    assert!(work_root_is_empty(&third));
    let interrupted = third.operation(&mid_run);
    assert_eq!(interrupted["state"], "interrupted", "{interrupted}");
    assert!(
        interrupted.get("sliceRevisionId").is_none(),
        "{interrupted}"
    );
    assert_eq!(active_operations(&third), 0);
    assert_eq!(
        record(&third, &models),
        recorded,
        "unchanged by a restart mid-run"
    );

    // Let the old worker go: it finds its operation interrupted and
    // stores nothing.
    release.send(()).unwrap();
    wait_for("the old worker to go idle", || {
        second.services.slicing.scheduler_idle().then_some(())
    });
    second.services.slicing.set_scheduler_hook(None);
    assert_eq!(third.stored_counts(), stored_before, "nothing new stored");
    assert_eq!(third.operation(&mid_run), interrupted);
    assert!(work_root_is_empty(&third));
    assert_eq!(record(&third, &models), recorded);
    drop(second);
}

/// Waits for operation `id` to leave `queued` and `running`, allowing a
/// real OrcaSlicer two minutes per plate.
fn wait_for_terminal(running: &Running, id: &str, engine: Engine) -> Value {
    match engine {
        Engine::Fake => {
            running.wait_state(id, "succeeded");
            running.operation(id)
        }
        Engine::Real => {
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                let operation = running.operation(id);
                if operation["state"] != "queued" && operation["state"] != "running" {
                    if operation["state"] != "succeeded" {
                        let log = running
                            .ok("get_slice_operation_log", json!({ "sliceOperationId": id }));
                        panic!("{operation}\n{}", log["text"]);
                    }
                    return operation;
                }
                assert!(Instant::now() < deadline, "the slice took over 2 minutes");
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

#[test]
fn tracer_runs_against_fake_orca() {
    run_tracer(&Farm::new(), Engine::Fake);
}

/// The tracer against the real OrcaSlicer in `FARM3D_ORCA` (spec D23).
#[test]
#[ignore = "needs a real OrcaSlicer: set FARM3D_ORCA (just test-orca)"]
fn real_orca_tracer_runs_against_a_real_orcaslicer() {
    let farm = Farm::with_real_orca();
    run_tracer(&farm, Engine::Real);
}
