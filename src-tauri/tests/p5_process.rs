//! P5 Task 6: the OrcaSlicer process supervisor (spec D9, D11) against
//! `fake-orca` (D23), plus `#[ignore]` twins against a real OrcaSlicer.
//!
//! Needs `--features test-support`, which builds `fake-orca`. The real
//! twins read `FARM3D_ORCA` (the engine) and, optionally,
//! `FARM3D_ORCA_PRESETS`; run them with `just test-orca`.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::mpsc;
#[cfg(unix)]
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

    /// Cancels once the shell has set its trap: it creates `term-ignored`
    /// in the work directory only after SIGTERM is ignored.
    struct CancelWhenTermIgnored {
        pid: Option<u32>,
        ready: PathBuf,
        sender: tokio::sync::watch::Sender<bool>,
    }
    impl SliceObserver for CancelWhenTermIgnored {
        fn spawned(&mut self, pid: u32) {
            self.pid = Some(pid);
            let sender = self.sender.clone();
            let ready = self.ready.clone();
            thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(10);
                while !ready.exists() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
                let _ = sender.send(true);
            });
        }
        fn progress(&mut self, _update: SliceProgress) {}
    }
    let ready = work.root().join("term-ignored");
    let mut observer = CancelWhenTermIgnored {
        pid: None,
        ready: ready.clone(),
        sender,
    };
    let started = Instant::now();
    let run = run_slice(&command, &CancelFlag::new(receiver), &mut observer);
    assert!(ready.exists(), "the shell never set its SIGTERM trap");
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
// PDEATHSIG (spec D9, AC7): an engine started through the production
// `spawn_group` dies with the thread that started it.
// ---------------------------------------------------------------------------

/// Set on the re-executed test binary that plays farm3d in
/// [`the_engine_exits_when_its_parent_is_killed`]: the file it writes the
/// engine's pid to.
#[cfg(target_os = "linux")]
const INTERMEDIATE_PID_FILE: &str = "FARM3D_TEST_PDEATHSIG_PID_FILE";

/// A `hang` fake-orca command: one progress line, then it sleeps until a
/// signal ends it. SIGTERM keeps its default action.
#[cfg(target_os = "linux")]
fn hanging_engine(temp: &Path) -> std::process::Command {
    let input = temp.join("input.3mf");
    fs::copy(fixtures().join("slicing/two-cube-plate.3mf"), &input).unwrap();
    let mut command = std::process::Command::new(FAKE_ORCA);
    command
        .arg("--slice")
        .arg("1")
        .arg("--outputdir")
        .arg(temp)
        .arg(&input)
        .current_dir(temp)
        .env("FAKE_ORCA_SCENARIO", "hang")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}

/// Polls until `pid` has exited (a zombie has), or `limit` passes.
#[cfg(target_os = "linux")]
fn gone_within(pid: i32, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if !alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// SIGKILLs `pid` when dropped, so a failed test leaves no engine behind.
/// [`Self::disarm`] it once the pid is confirmed gone: from then on the
/// pid may be reaped and reused, and the drop would kill a stranger.
#[cfg(target_os = "linux")]
struct KillOnDrop(Option<i32>);

#[cfg(target_os = "linux")]
impl KillOnDrop {
    fn new(pid: i32) -> Self {
        Self(Some(pid))
    }

    fn disarm(mut self) {
        self.0 = None;
    }
}

#[cfg(target_os = "linux")]
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = self.0.and_then(rustix::process::Pid::from_raw) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
    }
}

/// Kills and reaps a child process when dropped, so a failed assertion
/// never leaves it running. `std` never signals a child it has already
/// reaped, so a test may kill and wait on it first.
#[cfg(target_os = "linux")]
struct ReapOnDrop(std::process::Child);

#[cfg(target_os = "linux")]
impl Drop for ReapOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The parent's pid, field 4 of `/proc/<pid>/stat`.
#[cfg(target_os = "linux")]
fn parent_of(pid: i32) -> Option<i32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Not a test on its own: [`the_engine_exits_when_its_parent_is_killed`]
/// re-executes this test binary to run it as the engine's parent. It starts
/// a hanging fake-orca through the production `spawn_group`, publishes its
/// pid, and then blocks on this same thread until it is killed. Without
/// the variable it returns at once.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "helper process for the_engine_exits_when_its_parent_is_killed"]
fn pdeathsig_intermediate_parent() {
    let Some(pid_file) = std::env::var_os(INTERMEDIATE_PID_FILE).map(PathBuf::from) else {
        return;
    };
    let temp = pid_file.parent().unwrap().join("engine");
    fs::create_dir_all(&temp).unwrap();
    let child = farm3d_lib::slicing::process_group::spawn_group(&mut hanging_engine(&temp))
        .expect("spawn the engine");
    // Written whole, then renamed, so the test never reads half a pid.
    let partial = pid_file.with_extension("partial");
    fs::write(&partial, child.id().to_string()).unwrap();
    fs::rename(&partial, &pid_file).unwrap();
    // Park this thread, the engine's parent for PDEATHSIG, until SIGKILL.
    loop {
        thread::park();
    }
}

/// AC7: when farm3d dies without any chance to clean up (SIGKILL), its
/// engine exits too, through `PR_SET_PDEATHSIG(SIGTERM)`.
#[cfg(target_os = "linux")]
#[test]
fn the_engine_exits_when_its_parent_is_killed() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("engine.pid");
    let intermediate = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "pdeathsig_intermediate_parent",
            "--exact",
            "--ignored",
            "--test-threads=1",
            "--quiet",
        ])
        .env(INTERMEDIATE_PID_FILE, &pid_file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut intermediate = ReapOnDrop(intermediate);
    let intermediate_pid = intermediate.0.id() as i32;

    let deadline = Instant::now() + Duration::from_secs(20);
    let engine: i32 = loop {
        if let Ok(text) = fs::read_to_string(&pid_file) {
            break text.trim().parse().unwrap();
        }
        if let Some(status) = intermediate.0.try_wait().unwrap() {
            panic!("the intermediate exited before starting the engine: {status}");
        }
        if Instant::now() >= deadline {
            panic!("the intermediate never started the engine");
        }
        thread::sleep(Duration::from_millis(5));
    };
    let cleanup = KillOnDrop::new(engine);
    assert!(alive(engine), "the engine is not running");
    assert_eq!(parent_of(engine), Some(intermediate_pid));

    intermediate.0.kill().unwrap(); // SIGKILL
    intermediate.0.wait().unwrap();
    let gone = gone_within(engine, Duration::from_secs(1));
    if gone {
        cleanup.disarm();
    }
    assert!(gone, "the engine outlived its SIGKILLed parent by 1 s");
}

/// PDEATHSIG follows the *thread* that spawned the engine, not the
/// process: when that thread exits, the engine gets SIGTERM although
/// farm3d is still running. This is why the slicing scheduler spawns from
/// its own long-lived thread (`operations.rs`).
#[cfg(target_os = "linux")]
#[test]
fn the_engine_exits_when_the_thread_that_spawned_it_exits() {
    let temp = tempfile::tempdir().unwrap();
    let mut command = hanging_engine(temp.path());
    let mut child = thread::spawn(move || {
        farm3d_lib::slicing::process_group::spawn_group(&mut command).expect("spawn the engine")
    })
    .join()
    .unwrap();
    let engine = child.id() as i32;
    let cleanup = KillOnDrop::new(engine);
    assert_eq!(parent_of(engine), Some(std::process::id() as i32));
    let gone = gone_within(engine, Duration::from_secs(1));
    if gone {
        cleanup.disarm();
    }
    assert!(
        gone,
        "the engine outlived the thread that spawned it by 1 s"
    );
    // It was stopped by SIGTERM, the parent-death signal, and this process
    // is still its parent, so it can be reaped here.
    use std::os::unix::process::ExitStatusExt;
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(15), "{status:?}");
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

    /// The `; key = value` claims of a G-code's `CONFIG_BLOCK`.
    fn config_claims(gcode: &str) -> std::collections::BTreeMap<String, String> {
        gcode
            .lines()
            .skip_while(|line| line.trim() != "; CONFIG_BLOCK_START")
            .take_while(|line| line.trim() != "; CONFIG_BLOCK_END")
            .filter_map(|line| {
                let (key, value) = line.strip_prefix("; ")?.split_once(" = ")?;
                Some((key.to_string(), value.to_string()))
            })
            .collect()
    }

    /// Spec AC3 for D4's Printer Profile overrides: writing each mapped
    /// override key into the machine preset changes its G-code header
    /// claim. One baseline slice of the golden plate with the engine's own
    /// Elegoo Centauri Carbon presets, then one slice per key, each with a
    /// value that preset doesn't have.
    ///
    /// This drives D4's mapping (`apply_profile_overrides`) directly, not
    /// the commands: a Printer can override only `bedShape`,
    /// `printableHeightMm`, `bedExcludeAreas`, and `defaultBedType`
    /// (`PrinterProfileOverrides`), so the commands can never write
    /// `nozzle_diameter`, `nozzle_type`, or `gcode_flavor`, yet D4 maps
    /// them all.
    #[test]
    #[ignore = "needs a real OrcaSlicer: set FARM3D_ORCA (just test-orca)"]
    fn real_orca_each_profile_override_changes_its_gcode_header_claim() {
        use farm3d_lib::catalog::{BedShape, PointMm, PrinterProfile};
        use farm3d_lib::slicing::mapping::{
            apply_profile_overrides, ProfileFieldMapping, PROFILE_FIELD_MAPPINGS,
        };

        let point = |x_mm, y_mm| PointMm { x_mm, y_mm };
        // Only the overridden field of the profile is ever written, so the
        // rest of it is never read.
        let base = PrinterProfile {
            bed_shape: BedShape::Rectangular {
                width_mm: 256.0,
                depth_mm: 256.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            },
            printable_height_mm: 256.0,
            bed_exclude_areas: Vec::new(),
            default_bed_type: String::new(),
            nozzle_diameter_mm: vec![0.4],
            nozzle_type: String::new(),
            gcode_flavor: String::new(),
            has_auxiliary_fan: false,
            supports_air_filtration: false,
            supports_multi_filament: false,
            suggested_host_type: None,
        };
        /// A profile field, the profile with its override, the machine key
        /// it maps to, and the header claim expected.
        type Case = (&'static str, PrinterProfile, &'static str, &'static str);
        let with = |edit: &dyn Fn(&mut PrinterProfile)| {
            let mut profile = base.clone();
            edit(&mut profile);
            profile
        };
        let cases: Vec<Case> = vec![
            (
                "bedShape",
                with(&|p| {
                    p.bed_shape = BedShape::Rectangular {
                        width_mm: 300.0,
                        depth_mm: 280.0,
                        origin_x_mm: 0.0,
                        origin_y_mm: 0.0,
                    }
                }),
                "printable_area",
                "0x0,300x0,300x280,0x280",
            ),
            (
                "printableHeightMm",
                with(&|p| p.printable_height_mm = 200.0),
                "printable_height",
                "200",
            ),
            (
                "bedExcludeAreas",
                with(&|p| {
                    p.bed_exclude_areas = vec![
                        point(0.0, 0.0),
                        point(20.0, 0.0),
                        point(20.0, 20.0),
                        point(0.0, 20.0),
                    ]
                }),
                "bed_exclude_area",
                "0x0,20x0,20x20,0x20",
            ),
            (
                "nozzleDiameterMm",
                with(&|p| p.nozzle_diameter_mm = vec![0.6]),
                "nozzle_diameter",
                "0.6",
            ),
            (
                "nozzleType",
                with(&|p| p.nozzle_type = "stainless_steel".to_string()),
                "nozzle_type",
                "stainless_steel",
            ),
            (
                "gcodeFlavor",
                with(&|p| p.gcode_flavor = "marlin2".to_string()),
                "gcode_flavor",
                "marlin2",
            ),
            (
                "defaultBedType",
                with(&|p| p.default_bed_type = "Cool Plate".to_string()),
                "default_bed_type",
                "Cool Plate",
            ),
        ];
        // Every mapped field has a case, with the key it maps to.
        for (field, mapping) in PROFILE_FIELD_MAPPINGS {
            let ProfileFieldMapping::Mapped(key) = mapping else {
                continue;
            };
            let case = cases
                .iter()
                .find(|(name, ..)| *name == field)
                .unwrap_or_else(|| panic!("no case for {field}"));
            assert_eq!(case.2, key, "{field}");
        }
        assert_eq!(cases.len(), 7);

        let (engine, profiles, _cache) = real_orca();
        let slice_with = |label: &str, overridden: Option<(&str, &PrinterProfile)>| {
            let temp = tempfile::tempdir().unwrap();
            let (work, _, _) = real_work(temp.path(), &profiles);
            if let Some((field, profile)) = overridden {
                let mut machine: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_slice(&fs::read(work.machine_json()).unwrap()).unwrap();
                apply_profile_overrides(&mut machine, profile, &[field.to_string()], &[]).unwrap();
                fs::write(
                    work.machine_json(),
                    serde_json::to_vec_pretty(&machine).unwrap(),
                )
                .unwrap();
            }
            let command = SliceCommand::new(engine.clone(), work.clone(), Some(profiles.clone()));
            let (run, _) = slice(&command);
            assert_eq!(
                outcome(&run, &work),
                SliceOutcome::OutputWritten,
                "{label}: {}",
                run.log.text
            );
            config_claims(&fs::read_to_string(work.gcode()).unwrap())
        };

        let baseline = slice_with("baseline", None);
        assert!(!baseline.is_empty(), "the G-code has no CONFIG_BLOCK");
        for (field, profile, key, expected) in &cases {
            let header = slice_with(field, Some((field, profile)));
            let before = baseline.get(*key).map(String::as_str);
            let after = header.get(*key).map(String::as_str);
            eprintln!("{field}: {key} {before:?} -> {after:?}");
            assert_eq!(after, Some(*expected), "{field}: the header claim of {key}");
            assert_ne!(before, after, "{field}: {key} did not change");
        }
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
