//! P8 Task 16: the end-to-end tracer, from a Printer failure to a
//! notification, an Incident with camera evidence, and resolution (spec
//! D2–D8 and "Acceptance criteria" 14).
//!
//! One core function, [`run_attention_tracer`], runs from two entry points:
//!
//! - `attention_tracer_runs_against_the_fakes` (CI): `FakeMoonraker` (the
//!   dispatch rig's host), `FakeCamera` behind a host webcam the fake
//!   lists, and a `RecordingSink`. The fake speaks HTTP only, so the real
//!   supervisor never gets it Online: it classifies the fake as
//!   unreachable whether or not it is cut. So on the fakes the cut and the
//!   restore are synthetic at the connection layer (the fake refuses every
//!   connection; a restore stops the supervisor and seeds the live status
//!   again). They still drive the real supervisor's unreachable `error`
//!   into the projector; dropping a live connection is the simulator
//!   run's job.
//! - `p8_attention_tracer_runs_against_the_simulator` (`#[ignore]`, the
//!   evidence run `just test-sim` drives): the Moonraker simulator, its
//!   `[webcam farm3d-sim]`, and the sim camera behind the fault proxy. The
//!   real supervisor is Online over the simulator's WebSocket; a cut
//!   disables the Moonraker proxy, and a restore re-enables it and waits
//!   for the supervisor to come back Online by itself.
//!
//! Outside the cuts the Printer's live status is seeded from what the host
//! reports, as `p7_tracer.rs` does. The projector runs on a manual clock,
//! so the offline grace and the snapshot retention pass without waiting.
//! A **restart** rebuilds `RuntimeServices` over the same roots, as
//! `build_runtime_services` does: the startup backfill, then the runtimes.
//!
//! 1. Seed one Moonraker Printer with a host-webcam camera (`farm3d-sim`),
//!    `offlineAfterMinutes: 1`, and notifications `follow`. Enable the
//!    `connectivity` class (off by default; `printer.offline` is in it).
//!    Set focus to unfocused. A second, connection-less Printer (the
//!    decoy) has a manual camera URL on the backend's own camera with the
//!    corpus's userinfo and query token. Its capture is refused before any
//!    request (a URL with userinfo never reaches the network); then, with
//!    the userinfo dropped, one capture fetches a real frame through the
//!    token URL. On the fakes, the webcam list also carries the whole
//!    corpus URL (with its RFC 5737 host); on the simulator that host is
//!    scanned for but never seeded.
//! 2. Cut the Printer. The real supervisor reports `error` with the
//!    unreachable cause (asserted: the status, its message, and that it
//!    projects to `printer.offline`, never `printer.connectionError`).
//!    Before the grace passes: no Event.
//! 3. After the grace: exactly one open `printer.offline` Event and one
//!    notification whose target is `monitor/printer/<id>`. Twenty more
//!    offline observations: still one Event and one notification.
//! 5. Simulate the click: `farm3d-navigate-v1` carries the Printer target,
//!    and the Event is read. The click comes **before** step 4's restart:
//!    a restart forgets every outstanding notification (D6 "Click
//!    activation" step 1), so after it the same click is ignored, which
//!    step 4 checks.
//! 4. **Restart** (still cut). The backfill runs: still one open Event,
//!    and no new notification, even past the grace again.
//! 6. Acknowledge: the Event is acknowledged, still open, and still in the
//!    actionable count.
//! 7. Restore the Printer: the Event resolves `conditionCleared`, and its
//!    history (read, acknowledged, first observed) is intact.
//! 8. Cut and restore again: a new Event whose `recurrence_of` is step 3's.
//! 9. Run a print and fail it (sim `emergency_stop`; fake
//!    `finish_print("error")`): one `job.failed` Event, one Incident, and
//!    one `incident` snapshot from the camera (its bytes are the camera's),
//!    with an `evidenceCaptured` timeline row; the Job's material
//!    requirement projects into one `requirement.materialReconciliation`
//!    Event linked to the same Incident.
//! 10. Defer the material: that Event is acknowledged and still open.
//!     **Restart:** no duplicates. Settle it: that Event resolves
//!     `actionCompleted`, and the Incident stays open until the operator
//!     resolves `job.failed`. Then the Incident closes.
//! 11. Pin the snapshot, capture a manual one, age the clock past the
//!     retention, and run the janitor: the pinned snapshot survives; the
//!     unpinned manual one is pruned, and its row stays.
//! 12. Cut the Printer again until a `printer.offline` Event is open.
//!     Delete the Printer: `INCIDENT_HISTORY_EXISTS`. Archive it: the open
//!     Event resolves `conditionCleared` (D8), the `monitor/printer/<id>`
//!     target still resolves, and so does the Incident's.
//! 13. At every step: no dedup key has two open Events; nothing of the
//!     seeded-secret corpus or the camera's endpoint is in any emitted
//!     event, navigation, notification, notifier log line, or command
//!     response; and the camera has answered exactly the fetches farm3d was
//!     asked for (the decoy's capture, an Incident's capture, and one manual
//!     capture): no fetch without a trigger.

mod common;
mod p7_dispatch_rig;
mod sim;

use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use farm3d_lib::attention::deep_link;
use farm3d_lib::attention::projector::{AppliedChanges, EventChange};
use farm3d_lib::attention::services::AttentionTimings;
use farm3d_lib::attention::{AttentionEvent, AttentionResolution, ConditionKind};
use farm3d_lib::cameras::services::CameraTimings;
use farm3d_lib::connections::moonraker::control::MoonrakerTimings;
use farm3d_lib::connections::supervisor::PrinterSetupFacts;
use farm3d_lib::connections::{ConnectionConfig, ConnectionState, PrinterStatus};
use farm3d_lib::host_ops::{Clock, HostOpsTimings};
use farm3d_lib::jobs::JobTimings;
use farm3d_lib::notifications::recording::{RecordingSink, RecordingWindowControl};
use farm3d_lib::notifications::NAVIGATE_EVENT;
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::{StartSafety, StoredPrinter};
use common::fake_camera::{Answer, FakeCamera, JPEG};
use p7_dispatch_rig::{
    boot_with_attention, fast, id, status_from_host, AttentionBoot, ManualClock,
    NotificationBoot, RigTimings, Roots, Running, HOST_PATH, PRINTER, SECRET, SLR,
};
use serde_json::{json, Value};
use sim::camera::{CameraSim, TEST_PATTERN, WEBCAM_NAME, WEBCAM_SERVICE};
use sim::moonraker::{no_motion_gcode, MoonrakerSim};
use sim::toxiproxy::Proxy;
use tauri::Listener;
use zeroize::Zeroizing;

/// How often a wait re-reads what it waits on.
const POLL: Duration = Duration::from_millis(50);

/// Past `offlineAfterMinutes: 1`.
const PAST_GRACE: Duration = Duration::from_secs(61);

/// Past the default 30-day snapshot retention.
const PAST_RETENTION: Duration = Duration::from_secs(31 * 24 * 60 * 60);

/// The connection-less Printer whose manual camera URL carries the
/// corpus's userinfo and query token.
const DECOY: &str = "prn-decoy";

/// Global constraint 3's seeded-secret corpus (the one `p8_notifications.rs`
/// uses): a URL with userinfo, an RFC 5737 LAN-style host, and a query
/// token. The rig's stored API key (`SECRET`) is scanned beside it.
const SECRET_URL: &str = "http://operator:s3cr3t-P8N@192.0.2.10:8080/webcam?token=tok-P8N-77";
const CORPUS: [&str; 5] = [
    SECRET_URL,
    "s3cr3t-P8N",
    "operator:s3cr3t-P8N",
    "tok-P8N-77",
    "192.0.2.10",
];

/// The statuses the supervisor publishes for an unreachable host (P8 D2
/// "Reachability": `ConnectionError::Unreachable` or `Timeout`).
const UNREACHABLE_MESSAGES: [&str; 2] = [
    "The Printer could not be reached.",
    "The Printer did not respond in time.",
];

// ---------------------------------------------------------------------------
// Backend: what differs between the fakes and the simulator.
// ---------------------------------------------------------------------------

/// What a Moonraker host reports about its print right now.
struct HostView {
    print_state: String,
    filename: String,
    progress: f64,
}

trait Backend {
    /// The Printer's Connection; `None` is the rig's own fake.
    fn target(&self) -> Option<ConnectionConfig>;
    /// The Connection and credential the real supervisor uses for a cut.
    fn supervised(&self, roots: &Roots) -> (ConnectionConfig, Option<Zeroizing<String>>);
    /// Whether the real supervisor can get the host Online (the simulator
    /// speaks Moonraker's WebSocket; the fake speaks HTTP only).
    fn live_socket(&self) -> bool;
    fn gcode(&self) -> Vec<u8>;
    fn timings(&self) -> RigTimings;
    fn job_timings(&self) -> JobTimings;
    /// The host-webcam source's `webPort` (`None` is 80).
    fn web_port(&self) -> Option<u16>;
    /// Makes the host list the `farm3d-sim` webcam (the simulator's
    /// `moonraker.conf` already does).
    fn prepare_host(&self, roots: &Roots);
    /// The host's print, or `None` while it answers no status query.
    fn host_view(&self, roots: &Roots) -> Option<HostView>;
    /// The host goes away: every new connection fails.
    fn cut(&self, roots: &Roots);
    /// The host comes back.
    fn uncut(&self, roots: &Roots);
    /// Makes the running print fail.
    fn fail_print(&self, roots: &Roots);
    /// Brings the host back to standby after [`Backend::fail_print`].
    fn recover(&self, roots: &Roots);
    /// Snapshot `GET`s the camera has answered so far.
    fn camera_requests(&self) -> usize;
    /// The image the camera serves.
    fn camera_bytes(&self) -> Vec<u8>;
    /// The camera's `host:port`, which must never surface.
    fn camera_endpoint(&self) -> String;
    /// Checks the frame fetches sent no credential (where the camera
    /// records headers).
    fn assert_camera_saw_no_credential(&self) {}
    /// A safety check before any start (the simulator's heaters).
    fn before_start(&self) {}
}

// --- the fakes (CI) ------------------------------------------------------------

struct FakeBackend {
    camera: FakeCamera,
}

impl FakeBackend {
    fn new() -> Self {
        Self {
            camera: FakeCamera::start(Answer::Jpeg),
        }
    }
}

impl Backend for FakeBackend {
    fn target(&self) -> Option<ConnectionConfig> {
        None
    }

    fn supervised(&self, roots: &Roots) -> (ConnectionConfig, Option<Zeroizing<String>>) {
        (
            roots.fake.config(),
            Some(Zeroizing::new(SECRET.to_string())),
        )
    }

    fn live_socket(&self) -> bool {
        false
    }

    fn gcode(&self) -> Vec<u8> {
        p7_dispatch_rig::gcode()
    }

    fn timings(&self) -> RigTimings {
        RigTimings::default()
    }

    fn job_timings(&self) -> JobTimings {
        fast()
    }

    fn web_port(&self) -> Option<u16> {
        Some(self.camera.port)
    }

    /// The camera is `farm3d-sim`, with a relative URL carrying the
    /// corpus's query token; a decoy entry lists the whole corpus URL. The
    /// lookup must use neither beyond the one fetch.
    fn prepare_host(&self, roots: &Roots) {
        roots.fake.with_state(|state| {
            state.webcams = vec![
                json!({
                    "name": WEBCAM_NAME, "service": WEBCAM_SERVICE, "enabled": true,
                    "snapshot_url": "/snapshot.jpg?token=tok-P8N-77",
                    "stream_url": SECRET_URL,
                }),
                json!({
                    "name": "decoy", "service": "mjpegstreamer", "enabled": true,
                    "snapshot_url": SECRET_URL, "stream_url": SECRET_URL,
                }),
            ];
        });
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

    fn cut(&self, roots: &Roots) {
        roots.fake.set_reachable(false);
    }

    fn uncut(&self, roots: &Roots) {
        roots.fake.set_reachable(true);
    }

    fn fail_print(&self, roots: &Roots) {
        roots.fake.finish_print("error");
    }

    fn recover(&self, roots: &Roots) {
        roots.fake.restart();
    }

    fn camera_requests(&self) -> usize {
        self.camera.requests().len()
    }

    fn camera_bytes(&self) -> Vec<u8> {
        JPEG.to_vec()
    }

    fn camera_endpoint(&self) -> String {
        format!("127.0.0.1:{}", self.camera.port)
    }

    fn assert_camera_saw_no_credential(&self) {
        for request in self.camera.requests() {
            assert_eq!(request.method, "GET", "{request:?}");
            assert_eq!(request.header("x-api-key"), None, "{request:?}");
            assert_eq!(request.header("authorization"), None, "{request:?}");
            assert!(!format!("{request:?}").contains(SECRET), "{request:?}");
        }
    }
}

// --- the simulator -----------------------------------------------------------

struct SimBackend<'s> {
    sim: &'s MoonrakerSim,
    camera: &'s CameraSim,
}

impl Backend for SimBackend<'_> {
    fn target(&self) -> Option<ConnectionConfig> {
        Some(self.sim.config())
    }

    fn supervised(&self, _roots: &Roots) -> (ConnectionConfig, Option<Zeroizing<String>>) {
        (self.sim.config(), None)
    }

    fn live_socket(&self) -> bool {
        true
    }

    /// About 120 MB of comments and `M117`: roughly 10 s on the simulator,
    /// long enough to stop it mid-print (`p7_tracer.rs`).
    fn gcode(&self) -> Vec<u8> {
        no_motion_gcode("p8-tracer", 120)
    }

    /// `p7_tracer.rs`'s simulator timings.
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

    /// `p7_tracer.rs`'s 2 s history poll.
    fn job_timings(&self) -> JobTimings {
        JobTimings {
            history_poll: Duration::from_secs(2),
            ..JobTimings::default()
        }
    }

    /// The simulator lists an absolute URL on the Connection's own host.
    fn web_port(&self) -> Option<u16> {
        None
    }

    fn prepare_host(&self, _roots: &Roots) {}

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

    /// The Moonraker proxy refuses every connection and closes the open
    /// ones: the supervisor's WebSocket drops and every reconnect fails.
    fn cut(&self, _roots: &Roots) {
        self.sim.faults.set_enabled(Proxy::Moonraker, false);
    }

    fn uncut(&self, _roots: &Roots) {
        self.sim.faults.set_enabled(Proxy::Moonraker, true);
    }

    fn fail_print(&self, _roots: &Roots) {
        self.sim.emergency_stop();
    }

    fn recover(&self, _roots: &Roots) {
        self.sim.reset();
    }

    fn camera_requests(&self) -> usize {
        self.camera.requests()
    }

    fn camera_bytes(&self) -> Vec<u8> {
        TEST_PATTERN.to_vec()
    }

    fn camera_endpoint(&self) -> String {
        self.camera.target.authority()
    }

    fn before_start(&self) {
        for (heater, target) in self.sim.heater_targets() {
            assert!(target == 0.0, "refusing to start: {heater} has target {target}");
        }
    }
}

// ---------------------------------------------------------------------------
// The shared harness.
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

fn printer_target(printer_id: &str) -> Value {
    json!({"version": 1, "destination": "monitor", "selection": {"kind": "printer", "id": printer_id}})
}

fn blocker_codes(error: &Value) -> Vec<String> {
    error["details"]["blockers"]
        .as_array()
        .unwrap_or_else(|| panic!("no blockers: {error}"))
        .iter()
        .map(|blocker| blocker["code"].as_str().unwrap().to_string())
        .collect()
}

struct Tracer<'b, B: Backend> {
    backend: &'b B,
    roots: Roots,
    clock: Arc<ManualClock>,
    sink: Arc<RecordingSink>,
    control: Arc<RecordingWindowControl>,
    navigations: Arc<Mutex<Vec<Value>>>,
    /// Every command answer (success or error), for step 13's scan.
    responses: RefCell<Vec<String>>,
    /// The host view last seeded as the Printer's live status.
    seeded: RefCell<Option<(String, String, u64)>>,
    /// The camera fetches farm3d was asked for so far (step 13).
    expected_camera_requests: Cell<usize>,
    /// The camera's request count when the run began (the sim camera's
    /// count only grows while its container runs).
    camera_baseline: usize,
}

impl<'b, B: Backend> Tracer<'b, B> {
    fn new(backend: &'b B) -> Self {
        let roots = Roots::on_host(
            StartSafety::ConfirmBedClear,
            backend.target(),
            &backend.gcode(),
            backend.timings(),
        );
        backend.prepare_host(&roots);
        Self {
            backend,
            roots,
            clock: ManualClock::new(),
            sink: Arc::new(RecordingSink::available()),
            control: Arc::new(RecordingWindowControl::default()),
            navigations: Arc::new(Mutex::new(Vec::new())),
            responses: RefCell::new(Vec::new()),
            seeded: RefCell::new(None),
            expected_camera_requests: Cell::new(0),
            camera_baseline: backend.camera_requests(),
        }
    }

    // --- booting and restarting -------------------------------------------------

    /// Boots the app over the roots as `build_runtime_services` does, with
    /// the Printer's live status `initial`, captures on, and the
    /// notification runtime over the recording sink. The window is
    /// unfocused from the start.
    fn boot(&self, initial: PrinterStatus) -> (Running, AppliedChanges) {
        let (app, backfilled) = boot_with_attention(
            &self.roots,
            initial,
            self.backend.job_timings(),
            AttentionBoot {
                timings: AttentionTimings {
                    pass_min_interval: Duration::ZERO,
                    safety_tick: Duration::from_secs(3600),
                },
                clock: Some(Arc::clone(&self.clock) as Arc<dyn Clock>),
                cameras: Some(CameraTimings::default()),
                notifications: Some(NotificationBoot {
                    sink: Arc::clone(&self.sink) as _,
                    control: Arc::clone(&self.control) as _,
                }),
            },
        );
        app.services.notifications.focus().set(false);
        let navigations = Arc::clone(&self.navigations);
        app.app.listen(NAVIGATE_EVENT, move |event| {
            navigations
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });
        *self.seeded.borrow_mut() = None;
        app.wait_first_pass();
        app.attention_pass();
        (app, backfilled)
    }

    /// A restart: everything in motion settles, supervision and every
    /// runtime stop as a crash would, and a new app boots over the same
    /// roots with the live status `initial`.
    fn restart(&self, app: Running, initial: PrinterStatus) -> (Running, AppliedChanges) {
        app.quiesce();
        self.settle_captures(&app);
        tauri::async_runtime::block_on(app.manager.stop(PRINTER));
        app.stop_runtime();
        app.services.attention.stop();
        app.wait_until("the projector stopped", || {
            app.services.attention.running_tasks() == 0
        });
        drop(app);
        let (app, backfilled) = self.boot(initial);
        app.quiesce();
        (app, backfilled)
    }

    // --- commands ---------------------------------------------------------------

    /// A command, with its answer kept for step 13's scan.
    fn call(&self, app: &Running, command: &str, body: Value) -> Result<Value, Value> {
        let result = app.call(command, body);
        let text = match &result {
            Ok(value) | Err(value) => value.to_string(),
        };
        self.responses.borrow_mut().push(format!("{command}: {text}"));
        result
    }

    fn ok(&self, app: &Running, command: &str, body: Value) -> Value {
        self.call(app, command, body)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    /// A binary frame command's header and image.
    fn frame(&self, app: &Running, command: &str, mut body: Value) -> Result<(Value, Vec<u8>), Value> {
        body["contractVersion"] = json!(1);
        let response = tauri::test::get_ipc_response(
            &app.webview,
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
        let result = match response {
            Ok(tauri::ipc::InvokeResponseBody::Raw(bytes)) => {
                let length = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
                let header: Value = serde_json::from_slice(&bytes[4..4 + length]).unwrap();
                Ok((header, bytes[4 + length..].to_vec()))
            }
            Ok(other) => panic!("{command} answered JSON: {other:?}"),
            Err(error) => Err(error),
        };
        let text = match &result {
            Ok((header, _)) => header.to_string(),
            Err(error) => error.to_string(),
        };
        self.responses.borrow_mut().push(format!("{command}: {text}"));
        result
    }

    fn job(&self, app: &Running, job_id: &str) -> Value {
        self.ok(app, "get_job_history", json!({"jobId": job_id}))["job"].clone()
    }

    fn incident(&self, app: &Running, incident_id: &str) -> Value {
        self.ok(app, "get_incident", json!({"incidentId": incident_id}))
    }

    // --- the host's live status ---------------------------------------------------

    /// Seeds the host's print as the Printer's live status, when it has
    /// changed since the last seed. Only outside the cuts: then the real
    /// supervisor is not running.
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
        app.seed(status_from_host(&view.print_state, view.filename, view.progress));
        *self.seeded.borrow_mut() = Some(key);
    }

    fn host(&self) -> HostView {
        self.backend
            .host_view(&self.roots)
            .expect("the host answers a status query")
    }

    /// The host's live status as a fresh boot's initial status.
    fn host_status(&self) -> PrinterStatus {
        let view = self.host();
        status_from_host(&view.print_state, view.filename, view.progress)
    }

    /// Waits until `done`, mirroring the host's status meanwhile.
    fn wait_mirrored(&self, app: &Running, what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + app.wait;
        loop {
            self.mirror(app);
            if done() {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            std::thread::sleep(POLL);
        }
    }

    /// Waits until the Job's stored state is `state`, mirroring meanwhile.
    fn wait_job(&self, app: &Running, job_id: &str, state: &str) -> Value {
        let stored = || app.text(&format!("SELECT state FROM jobs WHERE id = '{job_id}'"));
        self.wait_mirrored(app, &format!("{job_id} is {state}"), || {
            stored().as_deref() == Some(state)
        });
        self.job(app, job_id)
    }

    fn status(&self, app: &Running) -> Option<PrinterStatus> {
        app.manager.statuses().remove(PRINTER)
    }

    // --- cuts and restores ------------------------------------------------------------

    fn start_supervision(&self, app: &Running) {
        let (config, key) = self.backend.supervised(&self.roots);
        tauri::async_runtime::block_on(app.manager.start(
            PRINTER.to_string(),
            config,
            key,
            PrinterSetupFacts::complete(),
        ));
    }

    /// Waits until the real supervisor reports the host unreachable, and
    /// checks that it is the unreachable path: `error`, with the message
    /// the unreachable and timeout causes carry (P8 D2 "Reachability").
    fn wait_unreachable(&self, app: &Running) {
        app.wait_until("the supervisor reports the host unreachable", || {
            self.status(app)
                .is_some_and(|status| status.connection_state == ConnectionState::Error)
        });
        let status = self.status(app).unwrap();
        assert!(
            status
                .error
                .as_deref()
                .is_some_and(|message| UNREACHABLE_MESSAGES.contains(&message)),
            "an unreachable host is an `error` with the unreachable cause: {status:?}"
        );
    }

    /// The host goes away under the real supervisor.
    fn cut(&self, app: &Running) {
        if self.backend.live_socket() {
            // The supervisor is Online over the host's WebSocket first, so
            // the cut drops a live connection.
            self.start_supervision(app);
            app.wait_until("the supervisor is Online", || {
                self.status(app)
                    .is_some_and(|status| status.connection_state == ConnectionState::Online)
            });
            self.backend.cut(&self.roots);
        } else {
            self.backend.cut(&self.roots);
            self.start_supervision(app);
        }
        self.wait_unreachable(app);
    }

    /// The host comes back; returns once the Printer is reachable again
    /// and supervision has stopped (the live status is seeded after).
    fn restore(&self, app: &Running) {
        self.backend.uncut(&self.roots);
        if self.backend.live_socket() {
            // The supervisor reconnects by itself.
            app.wait_until("the supervisor is Online again", || {
                self.status(app)
                    .is_some_and(|status| status.connection_state == ConnectionState::Online)
            });
            app.attention_pass();
        }
        tauri::async_runtime::block_on(app.manager.stop(PRINTER));
        *self.seeded.borrow_mut() = None;
        self.mirror(app);
        app.attention_pass();
    }

    /// Moves the projector's clock past the offline grace and runs a pass.
    fn pass_the_grace(&self, app: &Running) {
        self.clock.advance(PAST_GRACE);
        app.services.attention.poke();
        app.attention_pass();
    }

    // --- reading Attention ---------------------------------------------------------

    fn events(&self, app: &Running, condition: ConditionKind) -> Vec<AttentionEvent> {
        app.attention_rows()
            .into_iter()
            .filter(|event| event.condition == condition)
            .collect()
    }

    fn open(&self, app: &Running, condition: ConditionKind) -> Vec<AttentionEvent> {
        self.events(app, condition)
            .into_iter()
            .filter(|event| event.resolved_at.is_none())
            .collect()
    }

    fn event(&self, app: &Running, event_id: &str) -> AttentionEvent {
        app.attention_rows()
            .into_iter()
            .find(|event| event.id == event_id)
            .unwrap_or_else(|| panic!("no Event {event_id}"))
    }

    fn job_event(&self, app: &Running, condition: ConditionKind, job_id: &str) -> Option<AttentionEvent> {
        self.events(app, condition)
            .into_iter()
            .find(|event| event.job_id.as_deref() == Some(job_id))
    }

    /// `list_attention`'s open Events that are actionable (open and
    /// `requiresAction`, the Attention center's default filter).
    fn actionable(&self, app: &Running) -> Vec<String> {
        self.ok(app, "list_attention", json!({}))["open"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["resolvedAt"].is_null() && event["requiresAction"] == true)
            .map(|event| event["id"].as_str().unwrap().to_string())
            .collect()
    }

    /// The notifications shown so far that cover `dedup_key` (by
    /// themselves or folded into a summary).
    fn notifications_for(&self, dedup_key: &str) -> Vec<(u32, farm3d_lib::notifications::Notification)> {
        self.sink
            .shown()
            .into_iter()
            .filter(|(_, notification)| notification.dedup_keys.iter().any(|key| key == dedup_key))
            .collect()
    }

    /// Waits until the capture consumer has taken every committed change
    /// and no capture is running.
    fn settle_captures(&self, app: &Running) {
        let services = &app.services;
        app.wait_until("the captures settle", || {
            services.cameras.capture_rounds() >= services.attention.applied_sent()
                && services.cameras.captures_in_flight() == 0
        });
    }

    // --- step 13 -------------------------------------------------------------------

    /// Step 13, at the end of each step: one open Event per dedup key,
    /// nothing secret anywhere farm3d emitted or answered, and exactly the
    /// camera fetches farm3d was asked for.
    fn check(&self, app: &Running, at: &str) {
        let duplicated: Vec<(String, i64)> = app
            .storage
            .read(|connection| {
                let mut statement = connection.prepare(
                    "SELECT dedup_key, COUNT(*) FROM attention_events WHERE resolved_at IS NULL \
                     GROUP BY dedup_key HAVING COUNT(*) > 1",
                )?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .unwrap();
        assert!(duplicated.is_empty(), "{at}: a dedup key has two open Events: {duplicated:?}");

        let endpoint = self.backend.camera_endpoint();
        let needles: Vec<&str> = CORPUS
            .iter()
            .copied()
            .chain([SECRET, endpoint.as_str(), "/snapshot.jpg", "snapshots/", "farm3d-media"])
            .collect();
        let scan = |what: &str, text: &str| {
            for needle in &needles {
                assert!(!text.contains(needle), "{at}: {what} leaked {needle:?}: {text}");
            }
        };
        for event in app.events.lock().unwrap().iter() {
            scan("an emitted event", event);
        }
        for navigation in self.navigations.lock().unwrap().iter() {
            scan("a navigation", &navigation.to_string());
        }
        for (_, notification) in self.sink.shown() {
            scan("a notification's summary", &notification.summary);
            scan("a notification's body", &notification.body);
            scan(
                "a notification's target",
                &serde_json::to_string(&notification.target).unwrap(),
            );
        }
        for line in app.services.notifications.log_lines() {
            scan("a notifier log line", &line);
        }
        for response in self.responses.borrow().iter() {
            scan("a command response", response);
        }

        let answered = self.backend.camera_requests();
        let expected = self.expected_camera_requests.get();
        assert_eq!(
            answered.checked_sub(self.camera_baseline),
            Some(expected),
            "{at}: the camera answered {answered} requests since the run began at \
             {}; farm3d was asked for {expected} fetches",
            self.camera_baseline
        );
        self.backend.assert_camera_saw_no_credential();
    }

    fn expect_camera_fetch(&self) {
        self.expected_camera_requests
            .set(self.expected_camera_requests.get() + 1);
    }
}

/// Every `AppliedChanges` the projector has broadcast since `receiver` was
/// taken.
fn drain(
    receiver: &mut tokio::sync::broadcast::Receiver<Arc<AppliedChanges>>,
) -> Vec<Arc<AppliedChanges>> {
    let mut changes = Vec::new();
    loop {
        match receiver.try_recv() {
            Ok(applied) => changes.push(applied),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => return changes,
            Err(error) => panic!("the applied-changes receiver failed: {error}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The tracer's core.
// ---------------------------------------------------------------------------

fn run_attention_tracer<B: Backend>(backend: &B) {
    let t = &Tracer::new(backend);

    // 1. The Printer (the rig's), its host-webcam camera, its alert
    //    defaults, the connectivity class, and the decoy Printer.
    let (app, _) = t.boot(t.host_status());
    let summary = t.ok(
        &app,
        "set_printer_camera",
        json!({
            "operationId": "trc-camera", "printerId": PRINTER,
            "source": {
                "kind": "hostWebcam", "webcamName": WEBCAM_NAME,
                "webcamService": WEBCAM_SERVICE, "webPort": backend.web_port(),
            },
        }),
    );
    assert_eq!(summary["sourceKind"], "hostWebcam", "{summary}");
    let alerts = t.ok(
        &app,
        "set_printer_alert_defaults",
        json!({
            "operationId": "trc-alerts", "printerId": PRINTER,
            "alertDefaults": {
                "offlineAfterMinutes": 1, "notifications": "follow",
                "snapshotOnIncident": true, "snapshotOnCompletion": true,
            },
        }),
    );
    assert_eq!(alerts["alertDefaults"]["offlineAfterMinutes"], 1, "{alerts}");
    farm3d_lib::settings::repository::SettingsRepository::new(Arc::clone(&app.storage))
        .ensure_default()
        .unwrap();
    let settings = t.ok(&app, "load_settings", json!({}));
    assert_eq!(settings["notifications"]["connectivity"], false, "off by default");
    let saved = t.ok(
        &app,
        "save_settings",
        json!({
            "expectedRevision": settings["revision"],
            "themeMode": settings["themeMode"],
            "monitorSection": settings["monitorSection"],
            "monitorDensity": settings["monitorDensity"],
            "notifications": {
                "fatal": true, "confirmation": true, "completion": true,
                "reconciliation": false, "connectivity": true, "inventory": false,
            },
        }),
    );
    assert_eq!(saved["notifications"]["connectivity"], true, "{saved}");
    PrinterRepository::new(Arc::clone(&app.storage))
        .create(StoredPrinter {
            name: "Decoy".to_string(),
            ..common::a_stored_printer(DECOY)
        })
        .unwrap();
    // The decoy's camera: the backend's own camera, with the corpus's
    // userinfo and token. Written as-is (validation refuses userinfo; the
    // stored row is what's under test).
    let authority = backend.camera_endpoint();
    let decoy_url = |userinfo: &str| {
        format!("http://{userinfo}{authority}/snapshot.jpg?token=tok-P8N-77")
    };
    let set_decoy_url = |url: String| {
        app.storage
            .write(|tx| {
                tx.execute(
                    "INSERT INTO printer_cameras(printer_id, source_kind, snapshot_url, updated_at) \
                     VALUES (?1, 'snapshotUrl', ?2, '2026-09-28T09:00:00.000Z') \
                     ON CONFLICT(printer_id) DO UPDATE SET snapshot_url = excluded.snapshot_url",
                    rusqlite::params![DECOY, url],
                )?;
                Ok(())
            })
            .unwrap();
    };
    set_decoy_url(decoy_url("operator:s3cr3t-P8N@"));
    // With userinfo, the capture is refused before any request.
    let refused = t
        .call(
            &app,
            "capture_snapshot",
            json!({"operationId": "trc-decoy-userinfo", "printerId": DECOY}),
        )
        .unwrap_err();
    assert_eq!(refused["details"]["kind"], "noSnapshotUrl", "{refused}");
    t.check(&app, "after the decoy's refused capture");
    // With the token only, one real fetch through the corpus URL.
    set_decoy_url(decoy_url(""));
    let decoy = t.ok(
        &app,
        "capture_snapshot",
        json!({"operationId": "trc-decoy", "printerId": DECOY}),
    );
    t.expect_camera_fetch();
    assert_eq!(decoy["trigger"], "manual", "{decoy}");
    assert_eq!(decoy["printerId"], DECOY, "{decoy}");
    set_decoy_url(decoy_url("operator:s3cr3t-P8N@"));
    app.services.attention.poke();
    app.attention_pass();
    assert!(!app.services.notifications.focus().is_focused(), "unfocused");
    assert!(app.attention_rows().is_empty(), "nothing to attend to yet");
    t.check(&app, "after seeding");

    // 2. Cut: the real supervisor reports the unreachable cause. Within
    //    the grace, no Event.
    t.cut(&app);
    app.attention_pass();
    assert!(t.events(&app, ConditionKind::PrinterOffline).is_empty(), "inside the grace");
    assert!(
        t.events(&app, ConditionKind::PrinterConnectionError).is_empty(),
        "an unreachable host is never a connection error"
    );
    t.check(&app, "after the cut, inside the grace");

    // 3. Past the grace: one Event and one notification to the Printer.
    t.pass_the_grace(&app);
    let offline = t.open(&app, ConditionKind::PrinterOffline);
    assert_eq!(offline.len(), 1, "{offline:?}");
    let first = offline[0].clone();
    assert!(
        t.events(&app, ConditionKind::PrinterConnectionError).is_empty(),
        "the unreachable cause projects to printer.offline only"
    );
    assert_eq!(first.printer_id.as_deref(), Some(PRINTER));
    assert_eq!(first.recurrence_of, None);
    assert_eq!(first.read_at, None);
    app.wait_until("the offline notification", || {
        !t.notifications_for(&first.dedup_key).is_empty()
    });
    let shown = t.notifications_for(&first.dedup_key);
    assert_eq!(shown.len(), 1, "{shown:?}");
    let (notification_id, notification) = shown[0].clone();
    assert_eq!(
        serde_json::to_value(&notification.target).unwrap(),
        printer_target(PRINTER)
    );
    assert_eq!(notification.event_id.as_deref(), Some(first.id.as_str()));
    assert!(notification.body.starts_with("Warning: "), "{}", notification.body);

    // Twenty more offline observations amend quietly.
    let mut applied = app.services.attention.subscribe_applied();
    for _ in 0..20 {
        app.services.attention.poke();
        app.attention_pass();
    }
    let offline = t.open(&app, ConditionKind::PrinterOffline);
    assert_eq!(offline.len(), 1, "still one Event: {offline:?}");
    assert!(
        offline[0].observation_count >= first.observation_count + 20,
        "{} -> {}",
        first.observation_count,
        offline[0].observation_count
    );
    assert_eq!(offline[0].revision, first.revision, "an unchanged amendment bumps nothing");
    for changes in drain(&mut applied) {
        assert!(changes.notify.is_empty(), "an amendment never notifies: {changes:?}");
    }
    assert_eq!(t.notifications_for(&first.dedup_key).len(), 1, "still one notification");
    t.check(&app, "after twenty more offline observations");

    // 5. The click (before the restart; see the module doc): the window is
    //    raised, farm3d-navigate-v1 carries the Printer, and the Event is
    //    read.
    let before = app.services.notifications.signals_handled();
    t.sink.click(notification_id, "default");
    app.wait_until("the click is handled", || {
        app.services.notifications.signals_handled() > before
    });
    app.wait_until("the navigation", || !t.navigations.lock().unwrap().is_empty());
    assert_eq!(
        t.navigations.lock().unwrap().clone(),
        vec![json!({"contractVersion": 1, "target": printer_target(PRINTER), "openAttentionCenter": false})]
    );
    app.wait_until("the Event is read", || t.event(&app, &first.id).read_at.is_some());
    app.wait_until("the window is raised", || !t.control.steps().is_empty());
    t.check(&app, "after the click");

    // 4. Restart, still cut: the backfill inserts nothing and nothing
    //    notifies, even past the grace again.
    let shown_before = t.sink.shown().len();
    let before_restart = t.event(&app, &first.id);
    let (app, backfilled) = t.restart(app, PrinterStatus::new(ConnectionState::Offline));
    // Before supervision restarts: the backfill (and the first pass, with
    // the reach still inside the new grace) left the open Event as it was.
    let after_backfill = t.event(&app, &first.id);
    assert_eq!(after_backfill.revision, before_restart.revision, "{after_backfill:?}");
    assert_eq!(after_backfill.resolved_at, None, "{after_backfill:?}");
    assert_eq!(t.open(&app, ConditionKind::PrinterOffline).len(), 1);
    assert!(
        backfilled
            .events
            .iter()
            .all(|applied| !matches!(applied.change, EventChange::Inserted { .. })),
        "{:?}",
        backfilled.events
    );
    assert!(backfilled.notify.is_empty(), "a backfill never notifies");
    let mut applied = app.services.attention.subscribe_applied();
    t.start_supervision(&app);
    t.wait_unreachable(&app);
    app.attention_pass();
    t.pass_the_grace(&app);
    let offline = t.open(&app, ConditionKind::PrinterOffline);
    assert_eq!(offline.len(), 1, "still one open Event: {offline:?}");
    assert_eq!(offline[0].id, first.id);
    assert!(offline[0].read_at.is_some(), "still read");
    for changes in drain(&mut applied) {
        assert!(changes.notify.is_empty(), "nothing new to notify: {changes:?}");
    }
    assert_eq!(t.sink.shown().len(), shown_before, "no new notification");
    // A restart forgot the notification: clicking it now does nothing.
    let before = app.services.notifications.signals_handled();
    t.sink.click(notification_id, "default");
    app.wait_until("the stale click is handled", || {
        app.services.notifications.signals_handled() > before
    });
    assert_eq!(t.navigations.lock().unwrap().len(), 1, "a stale click navigates nowhere");
    t.check(&app, "after the restart");

    // 6. Acknowledge: still open, still actionable.
    let acknowledged = t.ok(
        &app,
        "acknowledge_attention_event",
        json!({"operationId": "trc-ack", "eventId": first.id}),
    );
    assert!(!acknowledged["events"][0]["acknowledgedAt"].is_null(), "{acknowledged}");
    assert!(acknowledged["events"][0]["resolvedAt"].is_null(), "acknowledge never resolves");
    assert_eq!(t.actionable(&app), vec![first.id.clone()]);
    let acknowledged = t.event(&app, &first.id);
    t.check(&app, "after acknowledging");

    // 7. Restore: the Event resolves conditionCleared, its history intact.
    t.restore(&app);
    let resolved = t.event(&app, &first.id);
    assert_eq!(resolved.resolution, Some(AttentionResolution::ConditionCleared));
    assert_eq!(resolved.read_at, acknowledged.read_at);
    assert_eq!(resolved.acknowledged_at, acknowledged.acknowledged_at);
    assert_eq!(resolved.first_observed_at, first.first_observed_at);
    assert!(resolved.observation_count >= acknowledged.observation_count);
    assert!(t.actionable(&app).is_empty(), "nothing actionable once resolved");
    t.check(&app, "after the restore");

    // 8. Cut and restore again: a recurrence of step 3's Event.
    t.cut(&app);
    app.attention_pass();
    assert!(t.open(&app, ConditionKind::PrinterOffline).is_empty(), "inside the grace");
    t.pass_the_grace(&app);
    let offline = t.open(&app, ConditionKind::PrinterOffline);
    assert_eq!(offline.len(), 1, "{offline:?}");
    let recurrence = offline[0].clone();
    assert_ne!(recurrence.id, first.id);
    assert_eq!(recurrence.recurrence_of.as_deref(), Some(first.id.as_str()));
    // The restarted notifier's limiter never saw the key: it notifies.
    app.wait_until("the recurrence's notification", || {
        t.notifications_for(&first.dedup_key).len() == 2
    });
    t.restore(&app);
    assert_eq!(
        t.event(&app, &recurrence.id).resolution,
        Some(AttentionResolution::ConditionCleared)
    );
    assert_eq!(t.events(&app, ConditionKind::PrinterOffline).len(), 2);
    t.check(&app, "after the second cut and restore");

    // 9. A print that fails.
    let spool = app.spool();
    app.load(&spool);
    let added = t.ok(
        &app,
        "add_to_queue",
        json!({
            "operationId": "trc-add", "sliceRevisionId": SLR, "quantity": 1,
            "policy": "recommended", "preference": "loadedFirst",
        }),
    );
    let entry = id(&added["entries"][0]);
    let assigned = t.ok(
        &app,
        "assign_queue_entry",
        json!({"operationId": "trc-assign", "entryId": entry, "printerId": PRINTER, "spoolId": spool}),
    );
    let job = id(&assigned["jobs"][0]);
    let staged = t.wait_job(&app, &job, "awaitingStart");
    assert_eq!(staged["hostPath"], HOST_PATH, "{staged}");
    t.wait_mirrored(&app, "job.startConfirmation opens", || {
        t.job_event(&app, ConditionKind::JobStartConfirmation, &job).is_some()
    });
    backend.before_start();
    let prior = prior_state(&t.host().print_state);
    t.ok(
        &app,
        "start_job",
        json!({"operationId": "trc-start", "jobId": job, "priorState": prior, "acknowledgement": "bedClear"}),
    );
    t.wait_job(&app, &job, "printing");
    t.wait_mirrored(&app, "the Job pinned", || {
        app.count_events(&job, "hostJobPinned") == 1
    });
    t.wait_mirrored(&app, "job.startConfirmation resolves", || {
        t.job_event(&app, ConditionKind::JobStartConfirmation, &job)
            .is_some_and(|event| event.resolution == Some(AttentionResolution::ActionCompleted))
    });
    t.check(&app, "while printing");

    backend.fail_print(&t.roots);
    t.wait_job(&app, &job, "failed");
    t.wait_mirrored(&app, "the failure's Events are projected into one Incident", || {
        [ConditionKind::JobFailed, ConditionKind::RequirementMaterialReconciliation]
            .iter()
            .all(|condition| {
                t.job_event(&app, *condition, &job)
                    .is_some_and(|event| event.incident_id.is_some())
            })
    });
    t.expect_camera_fetch();
    t.settle_captures(&app);
    let failed = t.job_event(&app, ConditionKind::JobFailed, &job).unwrap();
    let material = t
        .job_event(&app, ConditionKind::RequirementMaterialReconciliation, &job)
        .unwrap();
    let incident_id = failed.incident_id.clone().unwrap();
    assert_eq!(material.incident_id.as_deref(), Some(incident_id.as_str()), "the same Incident");
    assert_eq!(t.events(&app, ConditionKind::JobFailed).len(), 1);
    assert_eq!(t.events(&app, ConditionKind::RequirementMaterialReconciliation).len(), 1);
    assert!(
        t.events(&app, ConditionKind::PrinterHostFailed).is_empty(),
        "the Job carries the failure"
    );
    let incidents = t.ok(&app, "list_incidents", json!({"printerId": PRINTER}))["incidents"].clone();
    assert_eq!(incidents.as_array().unwrap().len(), 1, "{incidents}");
    assert_eq!(incidents[0]["id"], json!(incident_id));
    assert_eq!(incidents[0]["kind"], "job.failed");
    let snapshots = t.ok(&app, "list_snapshots", json!({"incidentId": incident_id}))["snapshots"].clone();
    assert_eq!(snapshots.as_array().unwrap().len(), 1, "{snapshots}");
    let evidence = snapshots[0].clone();
    assert_eq!(evidence["trigger"], "incident", "{evidence}");
    assert_eq!(evidence["jobId"], json!(job), "{evidence}");
    let evidence_id = evidence["id"].as_str().unwrap().to_string();
    let (header, image) = t
        .frame(&app, "snapshot_image", json!({"snapshotId": evidence_id}))
        .unwrap();
    assert_eq!(image, backend.camera_bytes(), "the camera's own frame");
    assert_eq!(header["contentType"], "image/jpeg");
    let detail = t.incident(&app, &incident_id);
    assert!(
        detail["timeline"].as_array().unwrap().iter().any(|item| {
            item["source"] == "incident"
                && item["entry"]["kind"] == "evidenceCaptured"
                && item["entry"]["detail"]["snapshotId"] == json!(evidence_id)
        }),
        "an evidenceCaptured row: {detail}"
    );
    // Fatal notifies; reconciliation is off.
    app.wait_until("the failure's notification", || {
        !t.notifications_for(&failed.dedup_key).is_empty()
    });
    assert_eq!(t.notifications_for(&failed.dedup_key).len(), 1);
    assert!(t.notifications_for(&material.dedup_key).is_empty(), "reconciliation is off");
    t.check(&app, "after the failure");

    backend.recover(&t.roots);
    t.mirror(&app);

    // 10. Defer the material: acknowledged, still open.
    t.ok(
        &app,
        "settle_job_material",
        json!({"operationId": "trc-defer", "jobId": job, "choice": {"kind": "defer"}}),
    );
    app.wait_until("the deferral acknowledges the material Event", || {
        t.event(&app, &material.id).acknowledged_at.is_some()
    });
    assert_eq!(t.event(&app, &material.id).resolved_at, None, "deferring never resolves");
    t.check(&app, "after deferring");

    // Restart: no duplicates.
    let rows_before: Vec<(String, String, Option<String>)> = app
        .attention_rows()
        .into_iter()
        .map(|event| (event.id, event.dedup_key, event.resolved_at))
        .collect();
    let host_status = t.host_status();
    let (app, backfilled) = t.restart(app, host_status);
    assert!(
        backfilled
            .events
            .iter()
            .all(|applied| !matches!(applied.change, EventChange::Inserted { .. })),
        "{:?}",
        backfilled.events
    );
    let rows_after: Vec<(String, String, Option<String>)> = app
        .attention_rows()
        .into_iter()
        .map(|event| (event.id, event.dedup_key, event.resolved_at))
        .collect();
    assert_eq!(rows_after, rows_before, "a restart changes no Event");
    t.check(&app, "after the restart while deferred");

    // Settle: the material Event resolves actionCompleted; the Incident
    // stays open while job.failed is open.
    t.ok(
        &app,
        "settle_job_material",
        json!({"operationId": "trc-settle", "jobId": job, "choice": {"kind": "estimated"}}),
    );
    app.wait_until("the material Event resolves", || {
        t.event(&app, &material.id).resolved_at.is_some()
    });
    assert_eq!(
        t.event(&app, &material.id).resolution,
        Some(AttentionResolution::ActionCompleted)
    );
    assert_eq!(t.incident(&app, &incident_id)["incident"]["state"], "open");
    let resolved = t.ok(
        &app,
        "resolve_attention_event",
        json!({"operationId": "trc-resolve", "eventId": failed.id}),
    );
    assert_eq!(resolved["events"][0]["resolution"], "operatorResolved", "{resolved}");
    assert_eq!(resolved["incidents"][0]["id"], json!(incident_id), "{resolved}");
    assert_eq!(resolved["incidents"][0]["state"], "closed", "{resolved}");
    assert_eq!(t.incident(&app, &incident_id)["incident"]["state"], "closed");
    t.check(&app, "after settling and resolving");

    // 11. Pin the evidence, capture a manual snapshot, age the clock past
    //     the retention, and run the janitor.
    let pinned = t.ok(
        &app,
        "set_snapshot_pinned",
        json!({"operationId": "trc-pin", "snapshotId": evidence_id, "pinned": true}),
    );
    assert!(pinned["pinnedAt"].is_string(), "{pinned}");
    let manual = t.ok(
        &app,
        "capture_snapshot",
        json!({"operationId": "trc-capture", "printerId": PRINTER}),
    );
    t.expect_camera_fetch();
    assert_eq!(manual["trigger"], "manual", "{manual}");
    assert_eq!(manual["incidentId"], Value::Null, "{manual}");
    let manual_id = manual["id"].as_str().unwrap().to_string();
    t.check(&app, "after the manual capture");
    t.clock.advance(PAST_RETENTION);
    let passes = app.services.cameras.janitor().passes();
    app.services.cameras.janitor().poke();
    app.wait_until("the janitor's pass", || {
        app.services.cameras.janitor().passes() > passes
    });
    let listed = t.ok(&app, "list_snapshots", json!({"printerId": PRINTER}))["snapshots"].clone();
    let row = |snapshot_id: &str| {
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|snapshot| snapshot["id"] == snapshot_id)
            .cloned()
            .unwrap_or_else(|| panic!("no row for {snapshot_id}: {listed}"))
    };
    let kept = row(&evidence_id);
    assert!(kept["pinnedAt"].is_string(), "{kept}");
    assert_eq!(kept["prunedAt"], Value::Null, "the pinned snapshot survives: {kept}");
    let (_, image) = t
        .frame(&app, "snapshot_image", json!({"snapshotId": evidence_id}))
        .unwrap();
    assert_eq!(image, backend.camera_bytes());
    let pruned = row(&manual_id);
    assert!(pruned["prunedAt"].is_string(), "the manual snapshot is pruned: {pruned}");
    assert_eq!(pruned["pruneReason"], "age", "{pruned}");
    let gone = t
        .frame(&app, "snapshot_image", json!({"snapshotId": manual_id}))
        .unwrap_err();
    assert_eq!(gone["code"], "EVIDENCE_PRUNED", "{gone}");
    assert_eq!(gone["details"]["reason"], "age", "{gone}");
    t.check(&app, "after the janitor");

    // 12. An open offline Event; delete is refused; archive resolves it
    //     and keeps every target resolvable.
    t.cut(&app);
    app.attention_pass();
    t.pass_the_grace(&app);
    let open_offline = t.open(&app, ConditionKind::PrinterOffline);
    assert_eq!(open_offline.len(), 1, "{open_offline:?}");
    let archived_away = open_offline[0].clone();
    let revision = app.scalar(&format!("SELECT revision FROM printers WHERE id = '{PRINTER}'"));
    let refused = t
        .call(&app, "delete_printer", json!({"id": PRINTER, "expectedRevision": revision}))
        .unwrap_err();
    assert_eq!(refused["code"], "LIFECYCLE_BLOCKED", "{refused}");
    assert!(
        blocker_codes(&refused).contains(&"INCIDENT_HISTORY_EXISTS".to_string()),
        "{refused}"
    );
    let spool_revision = app.scalar(&format!("SELECT revision FROM spools WHERE id = '{spool}'"));
    t.ok(
        &app,
        "archive_printer",
        json!({
            "id": PRINTER, "expectedRevision": revision, "operationId": "trc-archive",
            "spoolDispositions": [{
                "spoolId": spool, "expectedSpoolRevision": spool_revision,
                "disposition": {"kind": "storage"},
            }],
        }),
    );
    assert!(
        app.text(&format!("SELECT archived_at FROM printers WHERE id = '{PRINTER}'"))
            .is_some(),
        "archived"
    );
    // Archiving stopped the supervisor (`discard_connection`); the host
    // comes back only so the simulator is left as it was found.
    backend.uncut(&t.roots);
    app.services.attention.poke();
    app.attention_pass();
    let resolved = t.event(&app, &archived_away.id);
    assert_eq!(
        resolved.resolution,
        Some(AttentionResolution::ConditionCleared),
        "D8: archiving resolves the Printer's open Events: {resolved:?}"
    );
    assert_eq!(resolved.recurrence_of.as_deref(), Some(recurrence.id.as_str()));
    let offline_event = t.event(&app, &first.id);
    let exists = app
        .storage
        .read(|connection| Ok(deep_link::source_exists(connection, &offline_event)))
        .unwrap()
        .unwrap();
    assert!(exists, "an archived Printer is still a source");
    assert_eq!(
        serde_json::to_value(deep_link::target_for(&offline_event, exists)).unwrap(),
        printer_target(PRINTER)
    );
    let detail = t.incident(&app, &incident_id);
    assert_eq!(detail["incident"]["id"], json!(incident_id), "the Incident still opens");
    assert_eq!(
        t.ok(&app, "list_incidents", json!({"printerId": PRINTER}))["incidents"][0]["id"],
        json!(incident_id)
    );
    for condition in [
        ConditionKind::PrinterOffline,
        ConditionKind::PrinterConnectionError,
        ConditionKind::PrinterHostFailed,
    ] {
        assert!(t.open(&app, condition).is_empty(), "{condition:?} open on an archived Printer");
    }

    // 13. And at the end.
    t.check(&app, "at the end");
    assert_eq!(
        t.expected_camera_requests.get(),
        3,
        "the decoy's capture, one Incident capture, and one manual"
    );
}

// ---------------------------------------------------------------------------
// The two entry points.
// ---------------------------------------------------------------------------

#[test]
fn attention_tracer_runs_against_the_fakes() {
    run_attention_tracer(&FakeBackend::new());
}

#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn p8_attention_tracer_runs_against_the_simulator() {
    let discovered = MoonrakerSim::discover();
    let sim = require_sim!(discovered);
    let camera = require_sim!(CameraSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    run_attention_tracer(&SimBackend {
        sim: &sim,
        camera: &camera,
    });

    sim.reset();
}
