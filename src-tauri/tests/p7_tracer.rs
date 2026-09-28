//! P7 Task 16: the end-to-end tracer, from the Queue to material
//! settlement (spec acceptance, the restart matrix, settlement, D7, D8).
//!
//! One core function, [`run_queue_tracer`], runs from two entry points:
//!
//! - `queue_tracer_runs_against_fake_moonraker` (CI): against the
//!   in-process `FakeMoonraker` that `p7_dispatch_rig` starts. The test
//!   ends each print by editing the fake's state (`finish_print`).
//! - `p7_queue_tracer_runs_against_the_simulator` (`#[ignore]`, the
//!   evidence run `just test-sim` drives): against the Moonraker simulator,
//!   which runs the G-code it is sent. The fixture is no-motion (comments
//!   and `M117` only) and about 120 MB, so a print runs for about 10 s:
//!   long enough to cancel or stop mid-print, and it completes by itself.
//!   All three copies come from one Slice Revision, so they share that one
//!   file.
//!
//! Both runs, with a restart (a rebuilt `RuntimeServices` over the same
//! roots, as `build_runtime_services` does) where marked:
//!
//! 1. Seed one Moonraker Printer (sim-proven capabilities) and one matching
//!    Spool loaded in its Material Slot. The rig seeds a farm3d Slice
//!    Revision.
//! 2. Add to Queue with quantity 3. Assert 3 linked entries.
//! 3. `explain_queue_entry` on copy 1 names the Printer as a candidate.
//!    Copy 2 is blocked with `JOB_ACTIVE` once copy 1 is assigned.
//! 4. Assign copy 1 (operator). One Job, one reservation. **Restart.** The
//!    first app ran without the Job runtime, so the restart comes before
//!    the driver's stage (restart-matrix row R2).
//! 5. Staging completes, and the Job reaches `awaitingStart`. **Restart.**
//! 6. Start without the acknowledgement is rejected. Start with `bedClear`
//!    succeeds. **Restart** during `printing`.
//! 7. The print completes (sim: the file runs out; fake:
//!    `finish_print("completed")`). The Job is completed and the estimate
//!    consumed exactly once. The entry is closed. **Restart**, then check
//!    nothing changed.
//! 8. Record the optional measured correction. It appears as one
//!    correction.
//! 9. Copy 2: assign, start, and cancel mid-print. The Job is cancelled
//!    with a pending requirement. Defer. **Restart.** The requirement is
//!    still deferred, and the amount still unavailable. Settle with a
//!    measured amount; it settles exactly once.
//! 10. Copy 3: assign and start. The sim's `emergency_stop` gives a
//!     `klippy_shutdown` (the fake: `finish_print("klippy_shutdown")`),
//!     which fails the Job. Settle with the estimate.
//! 11. Retry copy 3. The new entry joins the same lineage, and the old
//!     history is unchanged.
//! 12. Archive the Printer: blocked while a Job is active, allowed after.
//!     Delete stays blocked (`JOB_HISTORY_EXISTS`).
//! 13. At every step, assert the uploads and starts farm3d sent (the
//!     rig's adapter counts) and what the host saw (the fake's request log;
//!     the simulator's history): no duplicate upload and no unconfirmed
//!     start.

mod common;
mod p7_dispatch_rig;
mod sim;

use std::cell::RefCell;
use std::time::{Duration, Instant};

use farm3d_lib::connections::ConnectionConfig;
use farm3d_lib::host_ops::HostOpsTimings;
use farm3d_lib::jobs::JobTimings;
use farm3d_lib::connections::moonraker::control::MoonrakerTimings;
use farm3d_lib::printers::StartSafety;
use farm3d_lib::spools::repository as spools_repository;
use p7_dispatch_rig::{
    boot_tuned, fast, id, status_from_host, Driver, RigTimings, Roots, Running, ESTIMATE_MG,
    HOST_PATH, PRINTER, SLR,
};
use serde_json::{json, Value};
use sim::moonraker::{no_motion_gcode, MoonrakerSim};

/// How often a mirrored wait re-reads the host and the Job.
const POLL: Duration = Duration::from_millis(50);

// ---------------------------------------------------------------------------
// Backend: what differs between the fake and the simulator.
// ---------------------------------------------------------------------------

/// What a Moonraker host reports about its print right now:
/// `print_stats.state`, the file, and the progress (0.0..=1.0).
struct HostView {
    print_state: String,
    filename: String,
    progress: f64,
}

/// Everything the tracer's core needs from whichever host it runs
/// against, so [`run_queue_tracer`] is written once. The fake lives in the
/// rig's [`Roots`], hence the `roots` argument.
trait Backend {
    /// The Printer's Connection; `None` is the rig's own fake.
    fn target(&self) -> Option<ConnectionConfig>;
    /// The Slice Revision's G-code.
    fn gcode(&self) -> Vec<u8>;
    fn timings(&self) -> RigTimings;
    /// The Job runtime's timings.
    fn job_timings(&self) -> JobTimings;
    /// The host's print, or `None` while it answers no status query.
    fn host_view(&self, roots: &Roots) -> Option<HostView>;
    /// Uploads the host itself saw, when it keeps a request log.
    fn host_uploads(&self, roots: &Roots) -> Option<usize>;
    /// Prints the host itself started for [`HOST_PATH`] during this run.
    fn host_starts(&self, roots: &Roots) -> usize;
    /// Returns once the running print has reached `progress`.
    fn progress_to(&self, roots: &Roots, progress: f64);
    /// Makes the running print complete.
    fn complete_print(&self, roots: &Roots);
    /// Makes the running print fail with `klippy_shutdown`.
    fn fail_print(&self, roots: &Roots);
    /// Brings the host back to standby after [`Backend::fail_print`].
    fn recover(&self, roots: &Roots);
    /// A safety check before any start (the simulator's heaters).
    fn before_start(&self) {}
}

// --- the fake (CI) ---------------------------------------------------------------

struct FakeBackend;

impl Backend for FakeBackend {
    fn target(&self) -> Option<ConnectionConfig> {
        None
    }

    fn gcode(&self) -> Vec<u8> {
        p7_dispatch_rig::gcode()
    }

    fn timings(&self) -> RigTimings {
        RigTimings::default()
    }

    /// The rig's 200 ms history poll.
    fn job_timings(&self) -> JobTimings {
        fast()
    }

    fn host_view(&self, roots: &Roots) -> Option<HostView> {
        let (print_state, filename, _) = roots.fake.print_state();
        let progress = roots.fake.with_state(|state| state.progress);
        Some(HostView {
            print_state,
            filename,
            progress,
        })
    }

    fn host_uploads(&self, roots: &Roots) -> Option<usize> {
        Some(roots.uploads())
    }

    fn host_starts(&self, roots: &Roots) -> usize {
        roots.starts()
    }

    fn progress_to(&self, roots: &Roots, progress: f64) {
        roots.fake.set_progress(progress);
    }

    fn complete_print(&self, roots: &Roots) {
        roots.fake.finish_print("completed");
    }

    fn fail_print(&self, roots: &Roots) {
        roots.fake.finish_print("klippy_shutdown");
    }

    fn recover(&self, roots: &Roots) {
        roots.fake.restart();
    }
}

// --- the simulator -----------------------------------------------------------------

struct SimBackend<'s> {
    sim: &'s MoonrakerSim,
    /// The run's start, as the host's epoch seconds, for the history query.
    since_epoch_s: f64,
}

impl<'s> SimBackend<'s> {
    fn new(sim: &'s MoonrakerSim) -> Self {
        Self {
            sim,
            since_epoch_s: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64(),
        }
    }
}

impl Backend for SimBackend<'_> {
    fn target(&self) -> Option<ConnectionConfig> {
        Some(self.sim.config())
    }

    /// About 120 MB of comments and `M117`: roughly 10 s on the simulator.
    fn gcode(&self) -> Vec<u8> {
        no_motion_gcode("p7-tracer", 120)
    }

    /// The simulator's slower answers: `sim_moonraker.rs`'s adapter and
    /// host-operation timings, and a wait long enough for a 120 MB upload
    /// plus a whole print.
    fn timings(&self) -> RigTimings {
        RigTimings {
            moonraker: MoonrakerTimings {
                connect: Duration::from_secs(2),
                query: Duration::from_secs(10),
                control: Duration::from_secs(20),
                transfer_base: Duration::from_secs(30),
                transfer_per_started_mib: Duration::from_millis(250),
            },
            host_ops: HostOpsTimings {
                settle_period: Duration::from_secs(3),
                verify_window: Duration::from_secs(15),
                verify_poll_interval: Duration::from_millis(250),
                backoff: |_| Duration::from_secs(3600),
                ..HostOpsTimings::default()
            },
            wait: Duration::from_secs(120),
        }
    }

    /// A 2 s history poll. The simulator can take over a second after
    /// accepting a start to begin the print, and so to list its history
    /// job; until then every poll is inconclusive, and three of them make
    /// the Job `outcomeUnknown` (D7). With the rig's 200 ms poll that is
    /// 0.6 s; production's 10 s poll allows 20 s.
    fn job_timings(&self) -> JobTimings {
        JobTimings {
            history_poll: Duration::from_secs(2),
            ..JobTimings::default()
        }
    }

    fn host_view(&self, _roots: &Roots) -> Option<HostView> {
        let status = self.sim.try_query("print_stats&virtual_sdcard").ok()?;
        let status = &status["result"]["status"];
        Some(HostView {
            print_state: status["print_stats"]["state"].as_str()?.to_string(),
            filename: status["print_stats"]["filename"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            progress: status["virtual_sdcard"]["progress"].as_f64().unwrap_or(0.0),
        })
    }

    /// Moonraker keeps no upload log; the rig's adapter count stands alone.
    fn host_uploads(&self, _roots: &Roots) -> Option<usize> {
        None
    }

    fn host_starts(&self, _roots: &Roots) -> usize {
        self.sim.history(&format!("limit=200&since={}", self.since_epoch_s))["result"]["jobs"]
            .as_array()
            .map(|jobs| {
                jobs.iter()
                    .filter(|job| job["filename"] == HOST_PATH)
                    .count()
            })
            .unwrap_or(0)
    }

    fn progress_to(&self, roots: &Roots, progress: f64) {
        sim::wait_until(
            &format!("the print reaching {progress}"),
            Duration::from_secs(60),
            || {
                self.host_view(roots)
                    .filter(|view| view.print_state == "printing" && view.progress >= progress)
                    .map(|_| ())
            },
        )
        .unwrap_or_else(|error| panic!("{error}"));
    }

    /// The file runs out by itself.
    fn complete_print(&self, _roots: &Roots) {}

    fn fail_print(&self, _roots: &Roots) {
        self.sim.emergency_stop();
    }

    fn recover(&self, _roots: &Roots) {
        self.sim.reset();
    }

    /// Never starts a print with a heater target set.
    fn before_start(&self) {
        for (heater, target) in self.sim.heater_targets() {
            assert!(target == 0.0, "refusing to start: {heater} has target {target}");
        }
    }
}

// ---------------------------------------------------------------------------
// The shared harness: identical regardless of which `Backend` is behind it.
// ---------------------------------------------------------------------------

/// The P6 `priorState` a Moonraker `print_stats.state` calls for.
fn prior_state(print_state: &str) -> &'static str {
    match print_state {
        "standby" => "ready",
        "complete" => "finished",
        "cancelled" => "cancelled",
        other => panic!("refusing to start: the host is {other}"),
    }
}

struct Tracer<'b, B: Backend> {
    backend: &'b B,
    roots: Roots,
    /// The host view last seeded as the Printer's live status.
    seeded: RefCell<Option<(String, String, u64)>>,
}

impl<'b, B: Backend> Tracer<'b, B> {
    fn host(&self) -> HostView {
        self.backend
            .host_view(&self.roots)
            .expect("the host answers a status query")
    }

    /// Seeds the host's print as the Printer's live status, when it has
    /// changed since the last seed.
    fn mirror(&self, app: &Running) {
        let Some(view) = self.backend.host_view(&self.roots) else {
            return;
        };
        let key = (
            view.print_state.clone(),
            view.filename.clone(),
            (view.progress * 1000.0) as u64,
        );
        if self.seeded.borrow().as_ref() == Some(&key) {
            return;
        }
        app.seed(status_from_host(
            &view.print_state,
            view.filename,
            view.progress,
        ));
        *self.seeded.borrow_mut() = Some(key);
    }

    /// Boots the app over the roots with the host's live status, the Job
    /// runtime `driver`, and the backend's Job timings.
    fn boot(&self, driver: Driver) -> Running {
        let view = self.host();
        let app = boot_tuned(
            &self.roots,
            driver,
            status_from_host(&view.print_state, view.filename, view.progress),
            self.backend.job_timings(),
            None,
        );
        *self.seeded.borrow_mut() = None;
        self.mirror(&app);
        app
    }

    /// A restart: everything in motion settles, the Job runtime stops as a
    /// crash would, and a new app boots over the same roots.
    fn restart(&self, app: Running) -> Running {
        app.quiesce();
        app.stop_runtime();
        drop(app);
        let app = self.boot(Driver::Started);
        app.wait_first_pass();
        app.quiesce();
        app
    }

    /// Waits until `done` holds, seeding the host's live status each round.
    /// A timeout reports `job_id`'s Job and Host Operations.
    fn wait(&self, app: &Running, job_id: &str, what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + app.wait;
        loop {
            self.mirror(app);
            if done() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting until {what}: {}\nits events: {:?}\nits Host Operations: {:?}",
                app.job(job_id),
                app.event_kinds(job_id),
                app.ops(job_id)
            );
            std::thread::sleep(POLL);
        }
    }

    /// Waits until the Job's stored state is `state`, with the host's
    /// status mirrored meanwhile: the Job as `get_job_history` has it.
    fn wait_job(&self, app: &Running, job_id: &str, state: &str) -> Value {
        let stored = || app.text(&format!("SELECT state FROM jobs WHERE id = '{job_id}'"));
        let deadline = Instant::now() + app.wait;
        loop {
            self.mirror(app);
            if stored().as_deref() == Some(state) {
                return app.job(job_id);
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {state}: {}\nits Host Operations: {:?}",
                app.job(job_id),
                app.ops(job_id)
            );
            std::thread::sleep(POLL);
        }
    }

    /// Step 13: exactly `uploads` uploads and `starts` starts so far, as
    /// farm3d sent them and as the host saw them.
    fn assert_writes(&self, app: &Running, uploads: usize, starts: usize, at: &str) {
        app.quiesce();
        assert_eq!(self.roots.writes.uploads(), uploads, "{at}: uploads farm3d sent");
        assert_eq!(self.roots.writes.starts(), starts, "{at}: starts farm3d sent");
        if let Some(seen) = self.backend.host_uploads(&self.roots) {
            assert_eq!(seen, uploads, "{at}: uploads the host saw");
        }
        assert_eq!(
            self.backend.host_starts(&self.roots),
            starts,
            "{at}: prints the host started"
        );
    }

    /// Assigns `entry_id` to the Printer with `spool_id` (operator) and
    /// waits until the driver has staged it: the Job id.
    fn assign_and_stage(
        &self,
        app: &Running,
        operation_id: &str,
        entry_id: &str,
        spool_id: &str,
    ) -> String {
        let change = app.ok(
            "assign_queue_entry",
            json!({
                "operationId": operation_id, "entryId": entry_id,
                "printerId": PRINTER, "spoolId": spool_id,
            }),
        );
        let job_id = id(&change["jobs"][0]);
        assert_eq!(change["jobs"][0]["assignedBy"], "operator");
        let job = self.wait_job(app, &job_id, "awaitingStart");
        assert_eq!(job["hostPath"], HOST_PATH, "{job}");
        job_id
    }

    /// Starts an `awaitingStart` Job with the bed-clear acknowledgement
    /// and the `priorState` the host calls for, and waits until the tracker
    /// has pinned its history job.
    fn start(&self, app: &Running, operation_id: &str, job_id: &str) {
        self.backend.before_start();
        let prior = prior_state(&self.host().print_state);
        app.start(operation_id, job_id, prior)
            .unwrap_or_else(|error| panic!("start_job from {prior}: {error}"));
        // The status stream lags the start's reply: until the Job is
        // `printing`, the Printer's live status is still whatever the host
        // last reported (for copies 2 and 3, the previous run of this same
        // file, ended).
        app.wait_job(job_id, "printing");
        self.wait(app, job_id, "the Job pinned", || {
            app.count_events(job_id, "hostJobPinned") == 1
        });
        let job = app.job(job_id);
        self.assert_printing_unless_host_complete(&job);
        assert_eq!(job["startConfirmation"], "bedClear", "{job}");
        // The Job's progress comes from its own print only: never from the
        // previous run of the same file, which the host still reports as
        // finished or cancelled until this print begins (owner decision 5).
        let host = self.host();
        if host.print_state == "printing" {
            assert!(
                job["maxProgressPct"].as_i64().unwrap()
                    <= (host.progress * 100.0).floor() as i64,
                "the Job took another run's progress: {job}"
            );
        }
    }

    /// `job` (read before this call) is `printing`, or `completed` only when
    /// the host itself reports the print `complete` (on the simulator, a
    /// 10 s print can end during a restart; on the fake it never does).
    /// The host is read after the Job, and a host never leaves `complete`
    /// by itself, so a completed Job always finds it there.
    fn assert_printing_unless_host_complete(&self, job: &Value) {
        let host = self.host().print_state;
        match job["state"].as_str() {
            Some("printing") => {}
            Some("completed") => assert_eq!(host, "complete", "{job}"),
            _ => panic!("expected printing (host {host}): {job}"),
        }
    }

    /// Moves the print to `progress` and waits until the Job has recorded
    /// at least that much.
    fn progress_to(&self, app: &Running, job_id: &str, progress: f64) {
        self.backend.progress_to(&self.roots, progress);
        let pct = (progress * 100.0).floor() as i64;
        self.wait(app, job_id, "the Job recording its progress", || {
            app.job_column(job_id, "max_progress_pct").unwrap_or(0) >= pct
        });
    }

    fn requirement(&self, app: &Running, job_id: &str) -> Value {
        app.history(job_id)["requirements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|requirement| requirement["kind"] == "materialReconciliation")
            .cloned()
            .expect("a materialReconciliation requirement")
    }

    fn available_mg(&self, app: &Running, spool_id: &str) -> i64 {
        app.storage
            .read(|connection| {
                Ok(spools_repository::load_record(connection, spool_id)
                    .unwrap()
                    .unwrap()
                    .availability
                    .available_mg)
            })
            .unwrap()
    }

    /// The Spool's ledger rows of `kind` against `reservation_id`.
    fn ledger(
        &self,
        app: &Running,
        spool_id: &str,
        kind: &str,
        reservation_id: &str,
    ) -> Vec<Value> {
        app.amount_events(spool_id)
            .into_iter()
            .filter(|event| event["kind"] == kind && event["reservationId"] == reservation_id)
            .collect()
    }

    /// The Queue Entry of `job_id`, as its history has it (closed entries
    /// included).
    fn entry(&self, app: &Running, job_id: &str) -> Value {
        app.history(job_id)["entry"].clone()
    }

    fn printer_revision(&self, app: &Running) -> i64 {
        app.scalar(&format!("SELECT revision FROM printers WHERE id = '{PRINTER}'"))
    }

    fn archive(
        &self,
        app: &Running,
        operation_id: &str,
        dispositions: Value,
    ) -> Result<Value, Value> {
        app.call(
            "archive_printer",
            json!({
                "id": PRINTER, "expectedRevision": self.printer_revision(app),
                "operationId": operation_id, "spoolDispositions": dispositions,
            }),
        )
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

/// A Job that ended mid-print recorded its own progress: at least the 30 %
/// the tracer waited for, and short of the whole print.
fn assert_mid_print(job: &Value) {
    let pct = job["maxProgressPct"].as_i64().unwrap();
    assert!((30..100).contains(&pct), "{job}");
}

/// A net measured amount, as the settle and correct dialogs send it.
fn net(net_mg: i64) -> Value {
    json!({"kind": "net", "netMg": net_mg, "confidence": "measured"})
}

// ---------------------------------------------------------------------------
// The tracer's core.
// ---------------------------------------------------------------------------

fn run_queue_tracer<B: Backend>(backend: &B) {
    let tracer = Tracer {
        backend,
        roots: Roots::on_host(
            StartSafety::ConfirmBedClear,
            backend.target(),
            &backend.gcode(),
            backend.timings(),
        ),
        seeded: RefCell::new(None),
    };
    let t = &tracer;

    // 1. One Printer (the rig's, sim-proven) and one matching Spool loaded
    //    in its Material Slot. The first app runs without the Job runtime:
    //    step 4's restart then lands before the driver's stage (R2).
    let app = t.boot(Driver::Off);
    let spool = app.spool();
    app.load(&spool);
    let spool_mg = 1_000_000;

    // 2. Add to Queue with quantity 3: three linked entries.
    let added = app.ok(
        "add_to_queue",
        json!({
            "operationId": "trc-add", "sliceRevisionId": SLR, "quantity": 3,
            "policy": "recommended", "preference": "loadedFirst",
        }),
    );
    let entries = added["entries"].as_array().unwrap().clone();
    assert_eq!(entries.len(), 3, "{added}");
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(entry["lineageId"], entries[0]["lineageId"], "{entry}");
        assert_eq!(entry["copyIndex"], index as i64 + 1, "{entry}");
        assert_eq!(entry["copyCount"], 3, "{entry}");
        assert_eq!(entry["state"], "queued", "{entry}");
        assert_eq!(entry["position"], index as i64 + 1, "{entry}");
        assert_eq!(entry["estimate"]["amountMg"], ESTIMATE_MG, "{entry}");
    }
    let [copy1, copy2, copy3] = [id(&entries[0]), id(&entries[1]), id(&entries[2])];

    // 3. Copy 1's explanation names the Printer, with the loaded Spool.
    let explained = app.ok("explain_queue_entry", json!({ "entryId": copy1 }));
    assert_eq!(explained["verdict"], "awaitingOperator", "{explained}");
    assert_eq!(explained["candidates"][0]["printerId"], PRINTER, "{explained}");
    assert_eq!(explained["candidates"][0]["spool"]["spoolId"], json!(spool), "{explained}");
    assert_eq!(explained["candidates"][0]["loadedMatch"], true, "{explained}");

    // 4. Assign copy 1: one Job, one active reservation, nothing sent yet.
    let change = app.ok(
        "assign_queue_entry",
        json!({
            "operationId": "trc-assign-1", "entryId": copy1,
            "printerId": PRINTER, "spoolId": spool,
        }),
    );
    let job1 = id(&change["jobs"][0]);
    assert_eq!(change["jobs"][0]["state"], "assigned", "{change}");
    assert_eq!(change["jobs"][0]["assignedBy"], "operator", "{change}");
    assert_eq!(change["entries"][0]["state"], "assigned", "{change}");
    let reservation1 = change["jobs"][0]["reservationId"].as_str().unwrap().to_string();
    assert_eq!(app.scalar("SELECT COUNT(*) FROM jobs"), 1);
    assert_eq!(
        app.scalar(&format!(
            "SELECT COUNT(*) FROM spool_reservations \
             WHERE spool_id = '{spool}' AND state = 'active'"
        )),
        1
    );
    assert_eq!(app.reservation_state(&reservation1), "active");
    assert_eq!(t.available_mg(&app, &spool), spool_mg - ESTIMATE_MG);

    // 3 (continued). Copy 2 is now blocked by copy 1's Job.
    let explained = app.ok("explain_queue_entry", json!({ "entryId": copy2 }));
    assert_eq!(explained["verdict"], "blocked", "{explained}");
    let job_active = explained["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|blocker| blocker["code"] == "JOB_ACTIVE")
        .unwrap_or_else(|| panic!("no JOB_ACTIVE blocker: {explained}"));
    assert_eq!(job_active["printerIds"], json!([PRINTER]), "{explained}");
    t.assert_writes(&app, 0, 0, "after assigning copy 1");

    // Restart before the driver ever ran.
    let app = t.restart(app);

    // 5. The driver stages once, and the Job awaits its start.
    let job = t.wait_job(&app, &job1, "awaitingStart");
    assert!(job["uploadHostOperationId"].is_string(), "{job}");
    assert_eq!(job["hostPath"], HOST_PATH, "{job}");
    t.assert_writes(&app, 1, 0, "after staging copy 1");

    let app = t.restart(app);
    assert_eq!(app.job(&job1)["state"], "awaitingStart");
    t.assert_writes(&app, 1, 0, "after the restart in awaitingStart");

    // 6. Start without the acknowledgement is refused, and sends nothing.
    let prior = prior_state(&t.host().print_state);
    let missing = app.call(
        "start_job",
        json!({ "operationId": "trc-start-1-none", "jobId": job1, "priorState": prior }),
    );
    // A missing key never reaches the command: Tauri refuses the call while
    // deserializing its arguments, so there is no farm3d error code.
    assert_eq!(
        missing.expect_err("a start with no acknowledgement"),
        json!(
            "invalid args `acknowledgement` for command `start_job`: \
             command start_job missing required key acknowledgement"
        )
    );
    let refused = app
        .call(
            "start_job",
            json!({
                "operationId": "trc-start-1-empty", "jobId": job1,
                "priorState": prior, "acknowledgement": "",
            }),
        )
        .unwrap_err();
    assert_eq!(refused["code"], "VALIDATION", "{refused}");
    assert_eq!(refused["details"]["fieldPath"], "acknowledgement", "{refused}");
    assert_eq!(app.job(&job1)["state"], "awaitingStart");
    t.assert_writes(&app, 1, 0, "after the refused starts");

    // Start with `bedClear`.
    t.start(&app, "trc-start-1", &job1);
    t.assert_writes(&app, 1, 1, "after starting copy 1");

    // Restart during `printing`: the tracker keeps its pin, and nothing is
    // sent again.
    let app = t.restart(app);
    t.assert_printing_unless_host_complete(&app.job(&job1));
    assert_eq!(app.count_events(&job1, "hostJobPinned"), 1);
    t.assert_writes(&app, 1, 1, "after the restart while printing");

    // 7. The print completes: the estimate is consumed once, and the entry
    //    closes.
    backend.complete_print(&t.roots);
    let job = t.wait_job(&app, &job1, "completed");
    assert_eq!(job["settlement"], "settled", "{job}");
    assert_eq!(job["settlementMethod"], "estimated", "{job}");
    assert_eq!(app.reservation_state(&reservation1), "consumed");
    let consumed = t.ledger(&app, &spool, "consumption", &reservation1);
    assert_eq!(consumed.len(), 1, "consumed exactly once: {consumed:?}");
    assert_eq!(app.spool_current_mg(&spool), spool_mg - ESTIMATE_MG);
    let entry = t.entry(&app, &job1);
    assert_eq!(entry["state"], "closed", "{entry}");
    assert_eq!(entry["closeReason"], "completed", "{entry}");
    assert_eq!(entry["position"], Value::Null, "{entry}");
    t.assert_writes(&app, 1, 1, "after copy 1 completed");

    let before = (app.job(&job1), entry, app.amount_events(&spool));
    let app = t.restart(app);
    let after = (app.job(&job1), t.entry(&app, &job1), app.amount_events(&spool));
    assert_eq!(after, before, "a restart after completion changes nothing");
    t.assert_writes(&app, 1, 1, "after the restart past completion");

    // 8. The optional measured correction: one correction.
    let measured_mg = app.spool_current_mg(&spool) - 1_500;
    let corrected = app
        .correct("trc-correct-1", &job1, net(measured_mg))
        .unwrap_or_else(|error| panic!("correct_job_material: {error}"));
    assert_eq!(corrected["jobs"][0]["corrected"], true, "{corrected}");
    let corrections: Vec<Value> = app
        .amount_events(&spool)
        .into_iter()
        .filter(|event| event["isCorrection"] == true)
        .collect();
    assert_eq!(corrections.len(), 1, "{corrections:?}");
    assert_eq!(corrections[0]["reservationId"], json!(reservation1));
    assert_eq!(app.count_events(&job1, "materialCorrected"), 1);
    assert_eq!(app.spool_current_mg(&spool), measured_mg);

    // 9. Copy 2: assign, start, and cancel mid-print.
    let job2 = t.assign_and_stage(&app, "trc-assign-2", &copy2, &spool);
    t.assert_writes(&app, 2, 1, "after staging copy 2");
    t.start(&app, "trc-start-2", &job2);
    t.assert_writes(&app, 2, 2, "after starting copy 2");
    t.progress_to(&app, &job2, 0.3);
    app.job_command("cancel_job", "trc-cancel-2", &job2)
        .unwrap_or_else(|error| panic!("cancel_job: {error}"));
    let job = t.wait_job(&app, &job2, "cancelled");
    assert_eq!(job["cancelReason"], "cancelledByOperator", "{job}");
    assert_mid_print(&job);
    assert_eq!(job["settlement"], "pending", "{job}");
    let reservation2 = job["reservationId"].as_str().unwrap().to_string();
    assert_eq!(app.reservation_state(&reservation2), "unresolved");
    assert_eq!(t.requirement(&app, &job2)["status"], "pending");
    assert_eq!(t.entry(&app, &job2)["closeReason"], "cancelled");

    // Defer, then restart: still deferred, the amount still unavailable.
    let deferred = app
        .settle("trc-defer-2", &job2, json!({"kind": "defer"}))
        .unwrap_or_else(|error| panic!("defer: {error}"));
    assert_eq!(deferred["jobs"][0]["settlement"], "deferred", "{deferred}");
    let available = t.available_mg(&app, &spool);
    let current = app.spool_current_mg(&spool);
    assert_eq!(available, current - ESTIMATE_MG, "the deferred amount is held");
    let app = t.restart(app);
    assert_eq!(app.job(&job2)["settlement"], "deferred");
    assert_eq!(t.requirement(&app, &job2)["status"], "deferred");
    assert_eq!(app.reservation_state(&reservation2), "unresolved");
    assert_eq!(t.available_mg(&app, &spool), available, "still unavailable");
    assert_eq!(app.spool_current_mg(&spool), current);

    // Settle with a measured amount, exactly once (a replay writes nothing).
    let measured_mg = current - 4_000;
    let settle = json!({"kind": "measured", "entry": net(measured_mg)});
    let settled = app
        .settle("trc-settle-2", &job2, settle.clone())
        .unwrap_or_else(|error| panic!("settle measured: {error}"));
    assert_eq!(settled["jobs"][0]["settlement"], "settled", "{settled}");
    assert_eq!(settled["jobs"][0]["settlementMethod"], "measured", "{settled}");
    let replayed = app
        .settle("trc-settle-2", &job2, settle)
        .unwrap_or_else(|error| panic!("settle replay: {error}"));
    assert_eq!(replayed["jobs"][0]["settlement"], "settled");
    assert_eq!(app.count_events(&job2, "materialSettled"), 1);
    assert_eq!(t.ledger(&app, &spool, "measurement", &reservation2).len(), 1);
    assert_eq!(app.reservation_state(&reservation2), "consumed");
    assert_eq!(t.requirement(&app, &job2)["status"], "resolved");
    assert_eq!(app.spool_current_mg(&spool), measured_mg);
    t.assert_writes(&app, 2, 2, "after settling copy 2");

    // 10. Copy 3: assign, start, and fail with `klippy_shutdown`.
    let job3 = t.assign_and_stage(&app, "trc-assign-3", &copy3, &spool);
    t.assert_writes(&app, 3, 2, "after staging copy 3");
    t.start(&app, "trc-start-3", &job3);
    t.assert_writes(&app, 3, 3, "after starting copy 3");
    t.progress_to(&app, &job3, 0.3);
    backend.fail_print(&t.roots);
    let job = t.wait_job(&app, &job3, "failed");
    assert_mid_print(&job);
    assert_eq!(job["settlement"], "pending", "{job}");
    let reservation3 = job["reservationId"].as_str().unwrap().to_string();
    assert_eq!(app.reservation_state(&reservation3), "unresolved");
    assert_eq!(t.entry(&app, &job3)["closeReason"], "failed");
    let estimated_use = job["settlementPreview"]["estimatedUseMg"].as_i64().unwrap();
    assert_eq!(
        estimated_use,
        farm3d_lib::jobs::estimated_use_mg(ESTIMATE_MG, job["maxProgressPct"].as_i64().unwrap()),
        "{job}"
    );
    assert!(estimated_use > 0, "the print got under way: {job}");

    // Settle with the estimate.
    let current = app.spool_current_mg(&spool);
    let settled = app
        .settle("trc-settle-3", &job3, json!({"kind": "estimated"}))
        .unwrap_or_else(|error| panic!("settle estimated: {error}"));
    assert_eq!(settled["jobs"][0]["settlementMethod"], "estimated", "{settled}");
    let consumed = t.ledger(&app, &spool, "consumption", &reservation3);
    assert_eq!(consumed.len(), 1, "{consumed:?}");
    assert_eq!(app.spool_current_mg(&spool), current - estimated_use);
    assert_eq!(app.count_events(&job3, "materialSettled"), 1);
    t.assert_writes(&app, 3, 3, "after settling copy 3");

    backend.recover(&t.roots);
    t.mirror(&app);

    // 11. Retry copy 3: a new entry in the same lineage; the old history is
    //     unchanged (only `retry` leaves the read-time `allowedActions`).
    let without_actions = |mut history: Value| {
        history["job"]["allowedActions"] = Value::Null;
        history["entry"]["allowedActions"] = Value::Null;
        history["lineage"] = Value::Null;
        history
    };
    let history_before = without_actions(app.history(&job3));
    let retried = app
        .job_command("retry_job", "trc-retry-3", &job3)
        .unwrap_or_else(|error| panic!("retry_job: {error}"));
    let retry = retried["entries"][0].clone();
    assert_eq!(retry["state"], "queued", "{retry}");
    assert_eq!(retry["lineageId"], entries[0]["lineageId"], "{retry}");
    assert_eq!(retry["originEntryId"], json!(copy3), "{retry}");
    assert_eq!(retry["originKind"], "retry", "{retry}");
    assert_eq!(without_actions(app.history(&job3)), history_before);
    let lineage = app.history(&job3)["lineage"].as_array().unwrap().len();
    assert_eq!(lineage, 4, "three copies and the retry");

    // 12. Archive: blocked while the retry's Job is active, allowed after.
    let job4 = t.assign_and_stage(&app, "trc-assign-4", &id(&retry), &spool);
    t.assert_writes(&app, 4, 3, "after staging the retry");
    let spool_revision = app.scalar(&format!("SELECT revision FROM spools WHERE id = '{spool}'"));
    let to_storage = json!([{
        "spoolId": spool, "expectedSpoolRevision": spool_revision,
        "disposition": {"kind": "storage"},
    }]);
    let blocked = t
        .archive(&app, "trc-archive-blocked", to_storage.clone())
        .unwrap_err();
    assert_eq!(blocked["code"], "LIFECYCLE_BLOCKED", "{blocked}");
    assert_eq!(blocker_codes(&blocked), vec!["JOB_ACTIVE"], "{blocked}");

    let cancelled = app
        .job_command("cancel_job", "trc-cancel-4", &job4)
        .unwrap_or_else(|error| panic!("cancel before start: {error}"));
    assert_eq!(cancelled["jobs"][0]["cancelReason"], "cancelledBeforeStart");
    assert_eq!(cancelled["jobs"][0]["settlement"], "notRequired");
    t.archive(&app, "trc-archive", to_storage)
        .unwrap_or_else(|error| panic!("archive_printer: {error}"));
    assert!(
        app.text(&format!("SELECT archived_at FROM printers WHERE id = '{PRINTER}'"))
            .is_some(),
        "archived"
    );

    let revision = t.printer_revision(&app);
    let refused = app
        .call("delete_printer", json!({ "id": PRINTER, "expectedRevision": revision }))
        .unwrap_err();
    assert_eq!(refused["code"], "LIFECYCLE_BLOCKED", "{refused}");
    assert_eq!(blocker_codes(&refused), vec!["JOB_HISTORY_EXISTS"], "{refused}");

    // 13. Four stages and three starts in all, each start confirmed.
    t.assert_writes(&app, 4, 3, "at the end");
    for job_id in [&job1, &job2, &job3] {
        assert_eq!(app.job(job_id)["startConfirmation"], "bedClear");
    }
}

// ---------------------------------------------------------------------------
// The two entry points.
// ---------------------------------------------------------------------------

#[test]
fn queue_tracer_runs_against_fake_moonraker() {
    run_queue_tracer(&FakeBackend);
}

#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn p7_queue_tracer_runs_against_the_simulator() {
    let discovered = MoonrakerSim::discover();
    let sim = require_sim!(discovered);
    let _guard = sim::exclusive();
    sim.reset();

    run_queue_tracer(&SimBackend::new(&sim));

    sim.reset();
}
