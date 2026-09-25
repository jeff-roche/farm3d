//! P5 Task 6: the OrcaSlicer process supervisor (spec D9, D11) against
//! `fake-orca` (D23), plus `#[ignore]` twins against a real OrcaSlicer.
//!
//! Needs `--features test-support`, which builds `fake-orca`. The real
//! twins read `FARM3D_ORCA` (the engine) and, optionally,
//! `FARM3D_ORCA_PRESETS`; run them with `just test-orca`.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use farm3d_lib::library::content::CancelFlag;
use farm3d_lib::slicing::invocation::WorkDir;
use farm3d_lib::slicing::process::{
    outcome, run_slice, RunExit, SliceCommand, SliceObserver, SliceOutcome, SliceProgress,
    SliceRun, LOG_MAX_BYTES,
};
use farm3d_lib::slicing::SliceFailureCode;

const FAKE_ORCA: &str = env!("CARGO_BIN_EXE_fake-orca");
const MACHINE: &str = "Test Printer 0.4 nozzle";
const PROCESS: &str = "0.20mm Standard @Test";
const FILAMENT: &str = "Test PLA @Test";
/// A cancel test that fails must not wait out the 30-minute default.
const CANCEL_TEST_TIMEOUT: Duration = Duration::from_secs(20);

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// A work directory with presets named like real flat presets and the
/// golden two-cube plate.
fn prepared_work(temp: &Path) -> WorkDir {
    let work = WorkDir::at(temp.join("slicing-work").join("sop-test"));
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
    work
}

fn fake_command(work: &WorkDir, scenario: &str) -> SliceCommand {
    let mut command = SliceCommand::new(PathBuf::from(FAKE_ORCA), work.clone(), None);
    let gcode = fixtures().join("library/orca-cube.gcode");
    command.environment.extend([
        (
            OsString::from("FAKE_ORCA_SCENARIO"),
            OsString::from(scenario),
        ),
        (OsString::from("FAKE_ORCA_GCODE"), gcode.into_os_string()),
    ]);
    command
}

#[derive(Default)]
struct Recorder {
    pid: Option<u32>,
    updates: Vec<SliceProgress>,
}

impl SliceObserver for Recorder {
    fn spawned(&mut self, pid: u32) {
        self.pid = Some(pid);
    }

    fn progress(&mut self, update: SliceProgress) {
        self.updates.push(update);
    }
}

fn slice(command: &SliceCommand) -> (SliceRun, Recorder) {
    let mut recorder = Recorder::default();
    let run = run_slice(command, &CancelFlag::never(), &mut recorder);
    (run, recorder)
}

fn failure_code(run: &SliceRun, work: &WorkDir) -> SliceFailureCode {
    match outcome(run, work) {
        SliceOutcome::Failed(failure) => failure.code,
        other => panic!("expected a failure, got {other:?}\nlog:\n{}", run.log.text),
    }
}

/// No word of the log is an absolute path.
fn assert_no_absolute_paths(text: &str, known: &[&Path]) {
    for path in known {
        let path = path.to_string_lossy();
        assert!(!text.contains(path.as_ref()), "the log names {path}");
    }
    // An absolute path starts with a `/` that doesn't continue a relative
    // path or a placeholder (`<work>/x`, `~/x`, `a/b`): at the start, or
    // after a space, `:`, `=`, a quote, and so on.
    let continues_path =
        |byte: u8| byte.is_ascii_alphanumeric() || b"_-.~+>/".contains(&byte) || byte >= 0x80;
    let bytes = text.as_bytes();
    if let Some(at) = (0..bytes.len())
        .find(|&at| bytes[at] == b'/' && (at == 0 || !continues_path(bytes[at - 1])))
    {
        let start = at.saturating_sub(40);
        let end = (at + 60).min(bytes.len());
        panic!(
            "the log holds an absolute path: {:?}",
            String::from_utf8_lossy(&bytes[start..end])
        );
    }
}

#[test]
fn the_absolute_path_check_catches_paths_after_punctuation() {
    for text in [
        "x=/etc/a",
        "key:/etc/a",
        "\"/etc/a\"",
        "/etc/a",
        "see /etc/a",
    ] {
        let caught = std::panic::catch_unwind(|| assert_no_absolute_paths(text, &[])).is_err();
        assert!(caught, "{text}");
    }
    assert_no_absolute_paths("<work>/input ~/.config a/b <engine>/orca 1/2", &[]);
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap()
}

fn alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
        && fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|stat| {
                // A zombie has already exited.
                let state = stat.rsplit(')').next().unwrap_or("").trim_start();
                !state.starts_with('Z')
            })
            .unwrap_or(false)
}

fn assert_monotonic(updates: &[SliceProgress]) {
    let totals: Vec<u8> = updates
        .iter()
        .filter_map(|update| update.total_percent)
        .collect();
    assert!(
        totals.windows(2).all(|pair| pair[0] <= pair[1]),
        "{totals:?}"
    );
    assert!(totals.iter().all(|total| *total <= 100));
}

#[test]
fn a_successful_slice_writes_gcode_that_names_the_loaded_presets() {
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    let (run, recorder) = slice(&fake_command(&work, "success"));

    assert_eq!(run.exit, RunExit::Exited { code: 0 });
    assert_eq!(
        run.result.as_ref().map(|result| result.return_code),
        Some(0)
    );
    assert_eq!(outcome(&run, &work), SliceOutcome::OutputWritten);
    assert!(recorder.pid.is_some());
    let gcode = fs::read_to_string(work.gcode()).unwrap();
    assert!(gcode.contains(&format!("; printer_settings_id = {MACHINE}\n")));
    assert!(gcode.contains(&format!("; filament_settings_id = \"{FILAMENT}\"\n")));

    // D24: progress exists on Linux only.
    if cfg!(target_os = "linux") {
        let messages: Vec<&str> = recorder
            .updates
            .iter()
            .map(|u| u.message.as_str())
            .collect();
        assert_eq!(messages.first(), Some(&"Loading file"), "{messages:?}");
        assert_eq!(recorder.updates.last().unwrap().total_percent, Some(100));
        assert_monotonic(&recorder.updates);
    } else {
        assert_eq!(recorder.updates.len(), 1);
        assert_eq!(recorder.updates[0].total_percent, None);
    }
    assert!(
        run.log.text.contains("OrcaSlicer-2.4.2"),
        "{}",
        run.log.text
    );
    assert!(!run.log.truncated);
    assert_no_absolute_paths(&run.log.text, &[work.root(), &home()]);
}

#[test]
fn every_gate_f_return_code_maps_to_its_d11_failure() {
    let cases = [
        (-50, SliceFailureCode::ObjectsOutsidePlate),
        (-5, SliceFailureCode::PresetInvalid),
        (-3, SliceFailureCode::InputMissing),
        (-6, SliceFailureCode::InputInvalid),
        (-17, SliceFailureCode::PresetIncompatible),
        (-24, SliceFailureCode::EngineError { return_code: -24 }),
    ];
    for (code, expected) in cases {
        let temp = tempfile::tempdir().unwrap();
        let work = prepared_work(temp.path());
        let (run, _) = slice(&fake_command(&work, &format!("fail:{code}")));
        assert_eq!(run.exit, RunExit::Exited { code }, "exit & 0xff read as i8");
        assert_eq!(run.result.as_ref().unwrap().return_code, code);
        assert_eq!(failure_code(&run, &work), expected, "code {code}");

        // Without result.json, the exit status alone gives the same code.
        let mut command = fake_command(&work, &format!("fail:{code}"));
        command
            .environment
            .push(("FAKE_ORCA_NO_RESULT".into(), "1".into()));
        fs::remove_file(work.result_json()).unwrap();
        let (run, _) = slice(&command);
        assert_eq!(run.result, None);
        assert_eq!(
            failure_code(&run, &work),
            expected,
            "code {code}, exit only"
        );
    }
    // An unmapped code keeps OrcaSlicer's error string.
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    let (run, _) = slice(&fake_command(&work, "fail:-24"));
    let SliceOutcome::Failed(failure) = outcome(&run, &work) else {
        panic!("not a failure");
    };
    assert_eq!(failure.message, "The input 3mf is from a newer version.");
}

#[test]
fn missing_and_unreadable_inputs_fail_like_orca() {
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    fs::remove_file(work.filament_json()).unwrap();
    let (run, _) = slice(&fake_command(&work, "success"));
    assert_eq!(failure_code(&run, &work), SliceFailureCode::InputMissing);
    // fake-orca names the missing file; the log keeps only `<work>`.
    assert!(
        run.log
            .text
            .contains("No such file: <work>/input/filament.json"),
        "{}",
        run.log.text
    );
    assert_no_absolute_paths(&run.log.text, &[work.root()]);

    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    fs::write(work.process_json(), "{ not json").unwrap();
    let (run, _) = slice(&fake_command(&work, "success"));
    assert_eq!(failure_code(&run, &work), SliceFailureCode::PresetInvalid);
}

#[test]
fn success_without_gcode_is_output_missing() {
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    let (run, _) = slice(&fake_command(&work, "successNoOutput"));
    assert_eq!(run.exit, RunExit::Exited { code: 0 });
    assert_eq!(failure_code(&run, &work), SliceFailureCode::OutputMissing);
}

#[test]
fn bad_output_still_reaches_the_publish_step() {
    // D11 checks 3–6 belong to publishing; the supervisor only needs the file.
    for scenario in ["malformedOutput", "oversizedOutput", "wrongPresetNames"] {
        let temp = tempfile::tempdir().unwrap();
        let work = prepared_work(temp.path());
        let (run, _) = slice(&fake_command(&work, scenario));
        assert_eq!(
            outcome(&run, &work),
            SliceOutcome::OutputWritten,
            "{scenario}"
        );
    }
}

#[test]
fn an_unexpected_signal_is_an_engine_crash() {
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    let (run, _) = slice(&fake_command(&work, "crash"));
    assert!(
        matches!(run.exit, RunExit::Signalled { .. }),
        "{:?}",
        run.exit
    );
    assert!(matches!(
        failure_code(&run, &work),
        SliceFailureCode::EngineCrashed { signal: 6 }
    ));
}

#[test]
fn an_engine_that_cannot_start_is_spawn_failed_without_its_path() {
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    let engine = temp.path().join("engines").join("missing-orca");
    let command = SliceCommand::new(engine.clone(), work.clone(), None);
    let (run, recorder) = slice(&command);
    assert!(matches!(run.exit, RunExit::SpawnFailed { .. }));
    assert_eq!(recorder.pid, None);
    assert_eq!(failure_code(&run, &work), SliceFailureCode::SpawnFailed);
    assert!(
        run.log.text.contains("<engine>/missing-orca"),
        "{}",
        run.log.text
    );
    assert_no_absolute_paths(&run.log.text, &[&engine]);
}

#[test]
fn a_hung_slice_times_out_through_the_stop_escalation() {
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    let mut command = fake_command(&work, "hang");
    command.timeout = Duration::from_millis(500);
    let started = Instant::now();
    let (run, recorder) = slice(&command);
    assert_eq!(run.exit, RunExit::TimedOut);
    assert_eq!(failure_code(&run, &work), SliceFailureCode::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(!alive(recorder.pid.unwrap() as i32));
}

#[cfg(unix)]
#[test]
fn cancel_mid_run_stops_the_grandchild_within_six_seconds() {
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    let mut command = fake_command(&work, "hangWithGrandchild");
    command.timeout = CANCEL_TEST_TIMEOUT;
    let (sender, receiver) = tokio::sync::watch::channel(false);
    let pid_file = work.root().join("grandchild.pid");
    let (cancelled_at, when) = mpsc::channel();
    let canceller = {
        let pid_file = pid_file.clone();
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while fs::read_to_string(&pid_file).map_or(true, |text| text.trim().is_empty()) {
                assert!(Instant::now() < deadline, "no grandchild started");
                thread::sleep(Duration::from_millis(10));
            }
            thread::sleep(Duration::from_millis(100));
            let _ = cancelled_at.send(Instant::now());
            sender.send(true).unwrap();
        })
    };
    let mut recorder = Recorder::default();
    let run = run_slice(&command, &CancelFlag::new(receiver), &mut recorder);
    canceller.join().unwrap();
    let cancelled = when.recv().unwrap();

    let grandchild: i32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    while alive(grandchild) && cancelled.elapsed() < Duration::from_secs(6) {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !alive(grandchild),
        "the grandchild outlived the cancel by 6 s"
    );
    assert!(!alive(recorder.pid.unwrap() as i32));
    assert_eq!(run.exit, RunExit::Cancelled);
    assert_eq!(outcome(&run, &work), SliceOutcome::Cancelled);
    // SIGTERM is enough for fake-orca, so the whole stop is quick.
    assert!(
        cancelled.elapsed() < Duration::from_secs(2),
        "{:?}",
        cancelled.elapsed()
    );
    assert!(!work.gcode().exists());
}

#[test]
fn garbage_progress_lines_are_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    let (run, recorder) = slice(&fake_command(&work, "garbageProgress"));
    assert_eq!(outcome(&run, &work), SliceOutcome::OutputWritten);
    if !cfg!(target_os = "linux") {
        return;
    }
    let messages: Vec<&str> = recorder
        .updates
        .iter()
        .map(|u| u.message.as_str())
        .collect();
    assert_eq!(
        messages,
        [
            "Loading file",
            "Too far",
            "Backwards",
            "Slicing mesh",
            "Generating perimeters",
            "Exporting G-code",
            "Slicing finished"
        ]
    );
    let totals: Vec<Option<u8>> = recorder.updates.iter().map(|u| u.total_percent).collect();
    assert_eq!(
        totals,
        [
            Some(1),
            Some(100),
            Some(100),
            Some(100),
            Some(100),
            Some(100),
            Some(100)
        ]
    );
    assert_eq!(recorder.updates[1].plate_percent, Some(100));
    assert_eq!(recorder.updates[2].plate_percent, Some(5));
}

#[test]
fn an_oversized_log_keeps_its_head_and_tail_within_the_cap_and_no_paths() {
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    let (run, _) = slice(&fake_command(&work, "oversizedLog"));
    assert_eq!(outcome(&run, &work), SliceOutcome::OutputWritten);
    let log = &run.log;
    assert!(log.truncated);
    assert!(log.text.len() <= LOG_MAX_BYTES, "{} bytes", log.text.len());
    assert!(
        log.text.len() > LOG_MAX_BYTES - 64 * 1024,
        "{} bytes",
        log.text.len()
    );
    assert!(log.text[..64 * 1024].contains("FAKE-ORCA FIRST LINE"));
    assert!(log.text.contains("bytes of log omitted"));
    assert!(log.text.contains("FAKE-ORCA LAST LINE"));
    assert!(log
        .text
        .contains("reading <work>/input/plate.3mf from ~/.config via <engine>/fake-orca"));
    let engine_dir = Path::new(FAKE_ORCA).parent().unwrap();
    assert_no_absolute_paths(&log.text, &[work.root(), &home(), engine_dir]);
}

#[test]
fn a_panicking_observer_still_stops_the_group() {
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    let mut command = fake_command(&work, "hang");
    command.timeout = CANCEL_TEST_TIMEOUT;
    let pid = std::sync::Arc::new(std::sync::Mutex::new(None));

    struct Panicker(std::sync::Arc<std::sync::Mutex<Option<u32>>>);
    impl SliceObserver for Panicker {
        fn spawned(&mut self, pid: u32) {
            *self.0.lock().unwrap() = Some(pid);
            panic!("observer failure");
        }
        fn progress(&mut self, _update: SliceProgress) {}
    }
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_slice(&command, &CancelFlag::never(), &mut Panicker(pid.clone()))
    }));
    assert!(unwound.is_err());
    let pid = pid.lock().unwrap().expect("spawned") as i32;
    assert!(!alive(pid), "the unwind left the engine running");
}

#[test]
fn the_child_gets_only_the_allowlisted_environment() {
    // A variable farm3d has but OrcaSlicer must not see. Cargo also sets
    // CARGO_PKG_NAME for every test binary.
    std::env::set_var("FARM3D_TEST_SENTINEL", "leak");
    assert!(std::env::var_os("CARGO_PKG_NAME").is_some());
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    let mut command = fake_command(&work, "success");
    command
        .environment
        .push(("FAKE_ORCA_PRINT_ENV".into(), "1".into()));
    let (run, _) = slice(&command);
    assert_eq!(outcome(&run, &work), SliceOutcome::OutputWritten);
    let names: Vec<&str> = run
        .log
        .text
        .lines()
        .filter_map(|line| line.strip_prefix("env: "))
        .collect();
    assert!(names.contains(&"PATH"), "{names:?}");
    assert!(!names.contains(&"FARM3D_TEST_SENTINEL"), "{names:?}");
    assert!(!names.contains(&"CARGO_PKG_NAME"), "{names:?}");
    let allowed = [
        "HOME",
        "USER",
        "LANG",
        "LC_ALL",
        "TMPDIR",
        "XDG_RUNTIME_DIR",
        "PATH",
    ];
    for name in names {
        assert!(
            allowed.contains(&name) || name.starts_with("FAKE_ORCA_"),
            "{name} leaked into the child"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_process_that_ignores_sigterm_is_killed_after_the_grace() {
    let temp = tempfile::tempdir().unwrap();
    let work = prepared_work(temp.path());
    let mut command = fake_command(&work, "hangIgnoringTerm");
    command.timeout = CANCEL_TEST_TIMEOUT;
    command.grace = Duration::from_millis(300);
    let (sender, receiver) = tokio::sync::watch::channel(false);

    /// Cancels 300 ms after the spawn, once the shell has taken over.
    struct CancelSoon {
        pid: Option<u32>,
        sender: tokio::sync::watch::Sender<bool>,
    }
    impl SliceObserver for CancelSoon {
        fn spawned(&mut self, pid: u32) {
            self.pid = Some(pid);
            let sender = self.sender.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(300));
                let _ = sender.send(true);
            });
        }
        fn progress(&mut self, _update: SliceProgress) {}
    }
    let mut observer = CancelSoon { pid: None, sender };
    let started = Instant::now();
    let run = run_slice(&command, &CancelFlag::new(receiver), &mut observer);
    assert_eq!(run.exit, RunExit::Cancelled);
    assert!(run.killed, "SIGTERM alone should not have stopped it");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    let group = observer.pid.unwrap() as i32;
    assert!(!alive(group));
    let survivors: Vec<i32> = fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                // Field 5 (after the command in parentheses) is the group.
                let fields: Vec<&str> = stat
                    .rsplit(')')
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .collect();
                fields.first() != Some(&"Z") && fields.get(2) == Some(&group.to_string().as_str())
            })
        })
        .collect();
    assert!(
        survivors.is_empty(),
        "group members survived: {survivors:?}"
    );
}

// ---------------------------------------------------------------------------
// Real OrcaSlicer (spec D23): `FARM3D_ORCA=<engine>`, optional
// `FARM3D_ORCA_PRESETS=<preset source>`; run with `just test-orca`.
// ---------------------------------------------------------------------------

mod real {
    use super::*;
    use farm3d_lib::slicing::presets::{
        default_filament, default_process, PresetIndex, PresetKind,
    };
    use farm3d_lib::slicing::repository::SlicerRuntimeConfig;
    use farm3d_lib::slicing::runtime::{resolve_runtime, DiscoveryEnv};
    use farm3d_lib::spools::MaterialFamily;

    /// The engine and preset source, or a panic: an ignored real-Orca test
    /// must never pass without running.
    fn real_orca() -> (PathBuf, PathBuf, tempfile::TempDir) {
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
        (engine.path, source.profiles_dir, cache)
    }

    /// A work directory with Gate B's Elegoo Centauri Carbon presets and
    /// the golden two-cube plate.
    fn real_work(temp: &Path, profiles: &Path) -> (WorkDir, String, String) {
        let index = PresetIndex::build(profiles, "real", &CancelFlag::never()).unwrap();
        let machine = "Elegoo Centauri Carbon 0.4 nozzle";
        let process = default_process(&index.offered(PresetKind::Process, machine)).unwrap();
        let filament = default_filament(
            &index.offered(PresetKind::Filament, machine),
            &[MaterialFamily::Pla],
        )
        .unwrap();
        let work = prepared_work(temp);
        for (kind, name, path) in [
            (PresetKind::Machine, machine, work.machine_json()),
            (PresetKind::Process, process.as_str(), work.process_json()),
            (
                PresetKind::Filament,
                filament.as_str(),
                work.filament_json(),
            ),
        ] {
            let flat = index.flatten(kind, name).unwrap();
            fs::write(path, serde_json::to_vec_pretty(&flat).unwrap()).unwrap();
        }
        (work, machine.to_string(), filament)
    }

    /// A core 3MF holding one UV sphere of about 1 M triangles (the size
    /// Gate E cancelled), centred on a 256 mm bed, so a slice runs for
    /// seconds.
    fn heavy_sphere_plate() -> Vec<u8> {
        use std::fmt::Write as _;
        use std::io::Write as _;
        let (stacks, slices, radius) = (500usize, 1000usize, 30.0f64);
        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" \
             xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n\
             <resources><object id=\"1\" type=\"model\"><mesh><vertices>\n",
        );
        let vertex = |xml: &mut String, x: f64, y: f64, z: f64| {
            let _ = writeln!(xml, "<vertex x=\"{x:.4}\" y=\"{y:.4}\" z=\"{z:.4}\"/>");
        };
        vertex(&mut xml, 128.0, 128.0, 2.0 * radius);
        for stack in 1..stacks {
            let polar = std::f64::consts::PI * stack as f64 / stacks as f64;
            for slice in 0..slices {
                let azimuth = std::f64::consts::TAU * slice as f64 / slices as f64;
                vertex(
                    &mut xml,
                    128.0 + radius * polar.sin() * azimuth.cos(),
                    128.0 + radius * polar.sin() * azimuth.sin(),
                    radius + radius * polar.cos(),
                );
            }
        }
        vertex(&mut xml, 128.0, 128.0, 0.0);
        xml.push_str("</vertices><triangles>\n");
        let bottom = 1 + (stacks - 1) * slices;
        let ring = |stack: usize, slice: usize| 1 + (stack - 1) * slices + slice % slices;
        let mut triangle = |a: usize, b: usize, c: usize| {
            let _ = writeln!(xml, "<triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>");
        };
        for slice in 0..slices {
            triangle(0, ring(1, slice), ring(1, slice + 1));
            triangle(bottom, ring(stacks - 1, slice + 1), ring(stacks - 1, slice));
        }
        for stack in 1..stacks - 1 {
            for slice in 0..slices {
                let (a, b) = (ring(stack, slice), ring(stack, slice + 1));
                let (c, d) = (ring(stack + 1, slice), ring(stack + 1, slice + 1));
                triangle(a, c, d);
                triangle(a, d, b);
            }
        }
        xml.push_str("</triangles></mesh></object></resources>\n<build><item objectid=\"1\"/></build>\n</model>\n");

        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("[Content_Types].xml", options).unwrap();
        zip.write_all(
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/></Types>\n",
        )
        .unwrap();
        zip.start_file("_rels/.rels", options).unwrap();
        zip.write_all(
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/></Relationships>\n",
        )
        .unwrap();
        zip.start_file("3D/3dmodel.model", options).unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    fn engine_mounts() -> usize {
        fs::read_to_string("/proc/self/mountinfo")
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains("/.mount_"))
            .count()
    }

    #[test]
    #[ignore = "needs a real OrcaSlicer: set FARM3D_ORCA (just test-orca)"]
    fn real_orca_slices_under_supervision() {
        let (engine, profiles, _cache) = real_orca();
        let temp = tempfile::tempdir().unwrap();
        let (work, machine, filament) = real_work(temp.path(), &profiles);
        let command = SliceCommand::new(engine.clone(), work.clone(), Some(profiles.clone()));
        let started = Instant::now();
        let (run, recorder) = slice(&command);
        eprintln!(
            "sliced in {:?}; exit {:?}; result {:?}",
            started.elapsed(),
            run.exit,
            run.result
        );
        for update in &recorder.updates {
            eprintln!("progress: {update:?}");
        }
        eprintln!("log ({} bytes):\n{}", run.log.text.len(), run.log.text);
        assert_eq!(
            outcome(&run, &work),
            SliceOutcome::OutputWritten,
            "{}",
            run.log.text
        );
        let gcode = fs::read_to_string(work.gcode()).unwrap();
        assert!(gcode.contains(&format!("; printer_settings_id = {machine}")));
        assert!(gcode.contains(&format!("; filament_settings_id = \"{filament}\"")));
        if cfg!(target_os = "linux") {
            assert!(!recorder.updates.is_empty(), "no FIFO progress");
            assert_monotonic(&recorder.updates);
        }
        assert_no_absolute_paths(&run.log.text, &[work.root(), &home(), &engine, &profiles]);
    }

    #[test]
    #[ignore = "needs a real OrcaSlicer: set FARM3D_ORCA (just test-orca)"]
    fn real_orca_cancel_stops_the_group_quickly() {
        let (engine, profiles, _cache) = real_orca();
        let temp = tempfile::tempdir().unwrap();
        let (work, _, _) = real_work(temp.path(), &profiles);
        fs::write(work.plate_3mf(), heavy_sphere_plate()).unwrap();
        let mut command = SliceCommand::new(engine, work.clone(), Some(profiles.clone()));
        command.timeout = CANCEL_TEST_TIMEOUT;
        let mounts_before = engine_mounts();

        /// Cancels on the first progress update, as a user would mid-run.
        struct CancelOnProgress {
            recorder: Recorder,
            sender: Option<tokio::sync::watch::Sender<bool>>,
            cancelled_at: Option<Instant>,
        }
        impl SliceObserver for CancelOnProgress {
            fn spawned(&mut self, pid: u32) {
                self.recorder.spawned(pid);
            }
            fn progress(&mut self, update: SliceProgress) {
                self.recorder.progress(update);
                if let Some(sender) = self.sender.take() {
                    eprintln!("cancelling at {:?}", self.recorder.updates.last());
                    self.cancelled_at = Some(Instant::now());
                    sender.send(true).unwrap();
                }
            }
        }
        let (sender, receiver) = tokio::sync::watch::channel(false);
        let mut observer = CancelOnProgress {
            recorder: Recorder::default(),
            sender: Some(sender),
            cancelled_at: None,
        };
        let run = run_slice(&command, &CancelFlag::new(receiver), &mut observer);
        let stopped_after = observer.cancelled_at.expect("a progress update").elapsed();
        eprintln!(
            "stopped {stopped_after:?} after cancel; log:\n{}",
            run.log.text
        );
        assert_eq!(run.exit, RunExit::Cancelled);
        assert_eq!(outcome(&run, &work), SliceOutcome::Cancelled);
        assert!(stopped_after < Duration::from_secs(6), "{stopped_after:?}");
        let pid = observer.recorder.pid.unwrap() as i32;
        assert!(!alive(pid));
        assert!(!work.gcode().exists());
        assert_eq!(engine_mounts(), mounts_before, "a mount was left behind");
    }
}
