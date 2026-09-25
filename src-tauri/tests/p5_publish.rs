//! P5 Task 7: publishing a slice run (spec D11–D13) end to end against
//! `fake-orca` (D23): run the supervisor, validate the output, and publish
//! the Slice Revision, or fail the operation with only its log.
//!
//! Needs `--features test-support`, which builds `fake-orca`.

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use farm3d_lib::catalog::BedShape;
use farm3d_lib::library::content::{CancelFlag, ContentStore};
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::printers::CatalogRef;
use farm3d_lib::slicing::facts::{Farm3dFacts, ProfileSnapshot};
use farm3d_lib::slicing::invocation::{EngineIdentity, PresetSourceIdentity, WorkDir};
use farm3d_lib::slicing::process::{
    output_missing, run_slice, SliceCommand, SliceObserver, SliceProgress,
};
use farm3d_lib::slicing::publish::{self, finish_run, FinishedRun, PublishInputs};
use farm3d_lib::slicing::repository::{
    insert_operation, insert_preparation, transition_operation, NewSliceOperation,
    OperationTransition,
};
use farm3d_lib::slicing::runtime::{FileHashCache, OrcaVersion, PresetSourceOrigin};
use farm3d_lib::slicing::{
    InstanceDoc, InstanceTransform, PlateDoc, PlateSnapshot, PreparationDocument, SliceControls,
    SliceFailureCode, SliceOperationState, SliceRevisionBlobRole, SliceRevisionTarget, SliceTarget,
};
use farm3d_lib::spools::MaterialFamily;

const FAKE_ORCA: &str = env!("CARGO_BIN_EXE_fake-orca");
const OPERATION: &str = "sop-e2e";
const MACHINE: &str = "Test Printer 0.4 nozzle";
const PROCESS: &str = "0.20mm Standard @Test";
const FILAMENT: &str = "Test PLA @Test";
const STL_HASH: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const NOW: &str = "2026-01-01T00:00:00.000Z";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

struct NoObserver;

impl SliceObserver for NoObserver {
    fn spawned(&mut self, _pid: u32) {}
    fn progress(&mut self, _update: SliceProgress) {}
}

fn a_profile() -> ProfileSnapshot {
    ProfileSnapshot {
        catalog_ref: CatalogRef {
            vendor: "Test".to_string(),
            model: "Printer".to_string(),
            variant: MACHINE.to_string(),
            model_id: "TP".to_string(),
            printer_variant: "0.4".to_string(),
        },
        bed_shape: BedShape::Rectangular {
            width_mm: 256.0,
            depth_mm: 256.0,
            origin_x_mm: 0.0,
            origin_y_mm: 0.0,
        },
        printable_height_mm: 256.0,
        bed_exclude_areas: Vec::new(),
        nozzle_type: "hardened_steel".to_string(),
        gcode_flavor: "klipper".to_string(),
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    _lease: MetadataRootLease,
    storage: Storage,
    store: ContentStore,
    work: WorkDir,
    inputs: PublishInputs,
}

impl Fixture {
    /// A `running` operation on plate 1 of an STL Model, with its work
    /// directory prepared as the operation layer would.
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths =
            StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
        let lease = MetadataRootLease::acquire(&paths).unwrap();
        let storage = Storage::open(paths, &lease).unwrap();
        let content_root = storage.paths().content_root().to_path_buf();
        let store = ContentStore::open(&content_root).unwrap();

        let plate = PlateDoc {
            plate_key: "plate-a".to_string(),
            name: Some("Left".to_string()),
            instances: vec![InstanceDoc {
                instance_key: "instance-a".to_string(),
                object_key: 1,
                transform: InstanceTransform {
                    translate_mm: [128.0, 128.0],
                    rotate_deg: [0.0, 0.0, 0.0],
                    scale: [1.0, 1.0, 1.0],
                },
            }],
        };
        let document = PreparationDocument {
            plates: vec![plate.clone()],
            target: SliceTarget::Profile {
                catalog_ref: a_profile().catalog_ref,
            },
            process_preset: Some(PROCESS.to_string()),
            filament_preset: Some(FILAMENT.to_string()),
            controls: SliceControls::default(),
        };
        storage
            .write_repo(|tx| {
                tx.execute(
                    "INSERT INTO content_blobs(sha256, size_bytes, created_at) VALUES (?1, 100, ?2)",
                    [STL_HASH, NOW],
                )?;
                tx.execute(
                    "INSERT INTO library_models(id, revision, name, format, storage_mode,
                                                created_at, updated_at)
                     VALUES ('mdl-cube', 1, 'Cube', 'stl', 'managed', ?1, ?1)",
                    [NOW],
                )?;
                tx.execute(
                    "INSERT INTO model_source_revisions(id, model_id, sequence, content_sha256,
                       size_bytes, format, origin, source_file_name, source_path, captured_at,
                       inspector_version, inspection_json)
                     VALUES ('msr-cube-1', 'mdl-cube', 1, ?1, 100, 'stl', 'import', 'cube.stl',
                             '/src/cube.stl', ?2, 1, '{}')",
                    [STL_HASH, NOW],
                )?;
                insert_preparation(tx, "prp-cube", "mdl-cube", "msr-cube-1", &document)?;
                insert_operation(
                    tx,
                    &NewSliceOperation {
                        id: OPERATION.to_string(),
                        preparation_id: "prp-cube".to_string(),
                        source_revision_id: "msr-cube-1".to_string(),
                        plate: PlateSnapshot {
                            plate_index: 1,
                            plate,
                        },
                    },
                )?;
                transition_operation(
                    tx,
                    OPERATION,
                    OperationTransition::Start {
                        pid: 1,
                        pid_started_at: 1,
                    },
                )
            })
            .unwrap();

        let work = WorkDir::for_operation(&content_root, OPERATION);
        work.create().unwrap();
        for (path, name) in [
            (work.machine_json(), MACHINE),
            (work.process_json(), PROCESS),
            (work.filament_json(), FILAMENT),
        ] {
            let preset = serde_json::json!({ "name": name, "from": "system" });
            fs::write(path, serde_json::to_vec(&preset).unwrap()).unwrap();
        }
        fs::copy(
            fixtures().join("slicing/two-cube-plate.3mf"),
            work.plate_3mf(),
        )
        .unwrap();

        let version = OrcaVersion {
            major: 2,
            minor: 4,
            patch: 2,
            prerelease: None,
        };
        let inputs = PublishInputs {
            operation_id: OPERATION.to_string(),
            target: SliceRevisionTarget {
                target: document.target.clone(),
                profile: a_profile(),
                machine_preset: MACHINE.to_string(),
                process_preset: PROCESS.to_string(),
                filament_preset: FILAMENT.to_string(),
                controls: SliceControls::default(),
            },
            facts: Farm3dFacts::new(a_profile(), 0.4, MaterialFamily::Pla, None, 1.75),
            engine: EngineIdentity::of(Path::new(FAKE_ORCA), &version, &FileHashCache::new())
                .unwrap(),
            preset_source: PresetSourceIdentity::new(&version, PresetSourceOrigin::Engine),
            profile_overrides: Vec::new(),
        };
        Self {
            temp,
            _lease: lease,
            storage,
            store,
            work,
            inputs,
        }
    }

    /// Runs fake-orca with `scenario` over `orca-cube.gcode`, then
    /// finishes the run.
    fn slice(&self, scenario: &str) -> FinishedRun {
        let mut command = SliceCommand::new(PathBuf::from(FAKE_ORCA), self.work.clone(), None);
        command.environment.extend([
            (
                OsString::from("FAKE_ORCA_SCENARIO"),
                OsString::from(scenario),
            ),
            (
                OsString::from("FAKE_ORCA_GCODE"),
                fixtures().join("library/orca-cube.gcode").into_os_string(),
            ),
        ]);
        let run = run_slice(&command, &CancelFlag::never(), &mut NoObserver);
        finish_run(
            &self.store,
            &self.storage,
            &self.inputs,
            &self.work,
            &run,
            &CancelFlag::never(),
        )
        .unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.storage
            .read(|connection| connection.query_row(sql, [], |row| row.get(0)))
            .unwrap()
    }

    fn blob(&self, sha256: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.store
            .open_verified(sha256)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    }

    fn text(&self, sql: &str) -> String {
        self.storage
            .read(|connection| connection.query_row(sql, [], |row| row.get(0)))
            .unwrap()
    }

    /// The absolute paths a stored log or manifest must never name.
    fn assert_no_known_paths(&self, text: &str) {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        for path in [
            self.temp.path(),
            self.work.root(),
            Path::new(FAKE_ORCA),
            &home,
        ] {
            let path = path.to_string_lossy();
            assert!(!text.contains(path.as_ref()), "names {path}:\n{text}");
        }
    }
}

#[test]
fn a_fake_orca_success_publishes_a_revision_with_its_six_blobs_and_the_exact_gcode() {
    let fixture = Fixture::new();

    let FinishedRun::Published(revision) = fixture.slice("success") else {
        panic!("expected a published revision");
    };

    assert_eq!(fixture.count("SELECT COUNT(*) FROM slice_revisions"), 1);
    let roles: Vec<SliceRevisionBlobRole> = revision.blobs.iter().map(|blob| blob.role).collect();
    assert_eq!(
        roles,
        [
            SliceRevisionBlobRole::Plate3mf,
            SliceRevisionBlobRole::MachinePreset,
            SliceRevisionBlobRole::ProcessPreset,
            SliceRevisionBlobRole::FilamentPreset,
            SliceRevisionBlobRole::Manifest,
            SliceRevisionBlobRole::Log,
        ]
    );
    let estimates = revision.summary.estimates.as_ref().unwrap();
    assert_eq!(estimates.print_seconds, Some(222));
    assert_eq!(estimates.layer_count, Some(50));
    assert_eq!(
        fixture.text("SELECT state FROM slice_operations"),
        "succeeded"
    );
    assert_eq!(
        fixture.text("SELECT slice_revision_id FROM slice_operations"),
        revision.summary.id
    );

    // The G-code blob is byte for byte what fake-orca wrote.
    let written = fs::read(fixture.work.gcode()).unwrap();
    let gcode_sha256 = fixture.text("SELECT gcode_sha256 FROM slice_revisions");
    assert_eq!(fixture.blob(&gcode_sha256), written);
    // Every input blob is byte for byte the file the engine loaded.
    for (role, path) in [
        ("plate3mf", fixture.work.plate_3mf()),
        ("machinePreset", fixture.work.machine_json()),
        ("processPreset", fixture.work.process_json()),
        ("filamentPreset", fixture.work.filament_json()),
    ] {
        let sha256 = fixture.text(&format!(
            "SELECT sha256 FROM slice_revision_blobs WHERE role = '{role}'"
        ));
        assert_eq!(fixture.blob(&sha256), fs::read(path).unwrap(), "{role}");
    }

    let log = String::from_utf8(
        fixture.blob(&fixture.text("SELECT sha256 FROM slice_revision_blobs WHERE role = 'log'")),
    )
    .unwrap();
    assert!(log.contains("fake-orca scenario success"), "{log}");
    fixture.assert_no_known_paths(&log);
    let manifest = String::from_utf8(
        fixture
            .blob(&fixture.text("SELECT sha256 FROM slice_revision_blobs WHERE role = 'manifest'")),
    )
    .unwrap();
    fixture.assert_no_known_paths(&manifest);
    let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    assert_eq!(manifest["engine"]["sha256"], fixture.inputs.engine.sha256);
    assert_eq!(manifest["arguments"][0], "<engine>");
    assert_eq!(
        manifest["arguments"].as_array().unwrap().last().unwrap(),
        "<work>/input/plate.3mf"
    );
}

fn invalid(reason: &str) -> SliceFailureCode {
    SliceFailureCode::OutputInvalid {
        reason: reason.to_string(),
    }
}

#[test]
fn fake_orca_output_failures_fail_the_operation_with_only_its_log() {
    let cases = [
        ("wrongPresetNames", invalid(publish::WRONG_PRINTER_PRESET)),
        ("malformedOutput", invalid(publish::NOT_GCODE)),
        ("oversizedOutput", invalid(publish::TOO_LARGE)),
        ("successNoOutput", output_missing().code),
    ];
    for (scenario, expected) in cases {
        let fixture = Fixture::new();

        let FinishedRun::Unpublished(operation) = fixture.slice(scenario) else {
            panic!("{scenario}: expected a failed operation");
        };

        assert_eq!(operation.state, SliceOperationState::Failed, "{scenario}");
        let failure = operation.failure.unwrap();
        assert_eq!(failure.code, expected, "{scenario}");
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM slice_revisions"),
            0,
            "{scenario}"
        );
        // The seeded STL blob and the log; nothing else, in rows or files.
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM content_blobs"),
            2,
            "{scenario}"
        );
        let log_sha256 = fixture.text("SELECT log_sha256 FROM slice_operations");
        let log = String::from_utf8(fixture.blob(&log_sha256)).unwrap();
        assert!(log.contains(&format!("scenario {scenario}")), "{log}");
        fixture.assert_no_known_paths(&log);
        let blobs = fixture.storage.paths().content_root().join("blobs/sha256");
        let files: usize = fs::read_dir(blobs)
            .unwrap()
            .map(|prefix| fs::read_dir(prefix.unwrap().path()).unwrap().count())
            .sum();
        assert_eq!(files, 1, "{scenario}");
    }
}

// ---------------------------------------------------------------------------
// Real OrcaSlicer (spec D23): `FARM3D_ORCA=<engine>`, optional
// `FARM3D_ORCA_PRESETS=<preset source>`; run with `just test-orca`.
// ---------------------------------------------------------------------------

mod real {
    use std::time::Duration;

    use super::*;
    use farm3d_lib::catalog::PointMm;
    use farm3d_lib::slicing::presets::{
        default_filament, default_process, PresetIndex, PresetKind,
    };
    use farm3d_lib::slicing::repository::SlicerRuntimeConfig;
    use farm3d_lib::slicing::runtime::{resolve_runtime, DiscoveryEnv};

    /// A stock profile whose start G-code purges off the bed (`G1 Y-3`),
    /// outside the 2 mm XY tolerance.
    const OFF_BED_PURGE_MACHINE: &str = "Prusa MK3S 0.4 nozzle";

    /// The engine, its version, and the preset source's `profiles`
    /// directory, or a panic: an ignored real-Orca test must never pass
    /// without running.
    fn real_orca() -> (PathBuf, OrcaVersion, PathBuf, tempfile::TempDir) {
        let engine = std::env::var_os("FARM3D_ORCA")
            .map(PathBuf::from)
            .expect("FARM3D_ORCA must name an OrcaSlicer engine; run through `just test-orca`");
        let presets = std::env::var_os("FARM3D_ORCA_PRESETS").map(PathBuf::from);
        let cache = tempfile::tempdir().unwrap();
        let runtime = resolve_runtime(
            &SlicerRuntimeConfig {
                revision: 1,
                engine_path: Some(engine.to_str().unwrap().to_string()),
                preset_source_path: presets.map(|path| path.to_str().unwrap().to_string()),
                updated_at: None,
            },
            &DiscoveryEnv {
                home: None,
                path_var: None,
                probe_timeout: Duration::from_secs(20),
            },
            cache.path(),
        );
        let engine = runtime.engine.expect("an accepted engine");
        let source = runtime.preset_source.expect("a readable preset source");
        (engine.path, engine.version, source.profiles_dir, cache)
    }

    /// The machine preset's rectangular `printable_area` and height.
    fn profile_of(machine: &serde_json::Map<String, serde_json::Value>) -> ProfileSnapshot {
        let points: Vec<PointMm> = machine["printable_area"]
            .as_array()
            .unwrap()
            .iter()
            .map(|point| {
                let (x, y) = point.as_str().unwrap().split_once('x').unwrap();
                PointMm {
                    x_mm: x.parse().unwrap(),
                    y_mm: y.parse().unwrap(),
                }
            })
            .collect();
        let height = &machine["printable_height"];
        let height = height
            .as_str()
            .map(|text| text.parse().unwrap())
            .or_else(|| height.as_f64())
            .unwrap();
        ProfileSnapshot {
            bed_shape: BedShape::Polygon { points },
            printable_height_mm: height,
            ..a_profile()
        }
    }

    #[test]
    #[ignore = "needs a real OrcaSlicer: set FARM3D_ORCA (just test-orca)"]
    fn real_orca_an_off_bed_purge_in_the_start_gcode_still_publishes() {
        let (engine, version, profiles, _cache) = real_orca();
        let mut fixture = Fixture::new();
        let index = PresetIndex::build(&profiles, "real", &CancelFlag::never()).unwrap();
        let machine = OFF_BED_PURGE_MACHINE;
        let process = default_process(&index.offered(PresetKind::Process, machine)).unwrap();
        let filament = default_filament(
            &index.offered(PresetKind::Filament, machine),
            &[MaterialFamily::Pla],
        )
        .unwrap();
        let flat_machine = index.flatten(PresetKind::Machine, machine).unwrap();
        for (kind, name, path) in [
            (PresetKind::Machine, machine, fixture.work.machine_json()),
            (
                PresetKind::Process,
                process.as_str(),
                fixture.work.process_json(),
            ),
            (
                PresetKind::Filament,
                filament.as_str(),
                fixture.work.filament_json(),
            ),
        ] {
            let flat = index.flatten(kind, name).unwrap();
            fs::write(path, serde_json::to_vec_pretty(&flat).unwrap()).unwrap();
        }
        let target = &mut fixture.inputs.target;
        target.machine_preset = machine.to_string();
        target.process_preset = process;
        target.filament_preset = filament;
        target.profile = profile_of(&flat_machine);
        fixture.inputs.engine =
            EngineIdentity::of(&engine, &version, &FileHashCache::new()).unwrap();

        let command = SliceCommand::new(engine, fixture.work.clone(), Some(profiles));
        let run = run_slice(&command, &CancelFlag::never(), &mut NoObserver);
        let finished = finish_run(
            &fixture.store,
            &fixture.storage,
            &fixture.inputs,
            &fixture.work,
            &run,
            &CancelFlag::never(),
        )
        .unwrap();

        let FinishedRun::Published(revision) = finished else {
            panic!(
                "expected a published revision, got {finished:?}\nlog:\n{}",
                run.log.text
            );
        };
        // The start G-code did purge off the bed, before the first layer.
        let gcode = fs::read_to_string(fixture.work.gcode()).unwrap();
        let purge = gcode.find("G1 Y-3").expect("an off-bed purge move");
        assert!(purge < gcode.find(";LAYER_CHANGE").unwrap());
        let manifest =
            fixture.text("SELECT sha256 FROM slice_revision_blobs WHERE role = 'manifest'");
        let manifest: serde_json::Value = serde_json::from_slice(&fixture.blob(&manifest)).unwrap();
        assert_eq!(
            manifest["boundsCheck"],
            serde_json::json!({ "status": "checked", "scope": "printBody" })
        );
        assert_eq!(revision.summary.plate.as_ref().unwrap().plate_index, 1);
    }
}
