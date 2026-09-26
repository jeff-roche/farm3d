//! P6 Task 8: the Moonraker capability adapter (`moonraker::control`)
//! against `FakeMoonraker`: every trait method's success path, and each
//! scripted fault's D5 classification (`Definitive` or `Indeterminate` for a
//! write, an `Err` rather than a guess for a read).

mod common;

use std::time::Duration;

use common::fake_moonraker::{FakeJob, FakeMoonraker, Fault, Route, StartTrace, TRACEBACK_MARKER};
use farm3d_lib::connections::capabilities::{
    ArtifactStaging, CameraDiscovery, CommandFailure, DiffersReason, HistoryQuery, HostJobState,
    HostOperationFailureCode, HostStateQuery, InconclusiveReason, KlippyState, LocateOutcome,
    PrintControl, PrintStatsState, StagedArtifact,
};
use farm3d_lib::connections::moonraker::control::{MoonrakerCapabilities, MoonrakerTimings};
use farm3d_lib::connections::{ConnectionConfig, ConnectionError};
use serde_json::json;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const SEEDED_SECRET: &str = "seeded-secret-XYZ123";
const HOST_PATH: &str = "farm3d/slr-0001.gcode";

/// Short enough that a timeout test takes well under a second.
fn short_timings() -> MoonrakerTimings {
    MoonrakerTimings {
        connect: Duration::from_millis(500),
        query: Duration::from_millis(300),
        control: Duration::from_millis(300),
        transfer_base: Duration::from_millis(300),
        transfer_per_started_mib: Duration::from_millis(10),
    }
}

fn adapter(fake: &FakeMoonraker) -> MoonrakerCapabilities {
    MoonrakerCapabilities::new(&fake.config(), None, short_timings())
}

fn adapter_with_key(config: &ConnectionConfig, key: &str) -> MoonrakerCapabilities {
    MoonrakerCapabilities::new(
        config,
        Some(Zeroizing::new(key.to_string())),
        short_timings(),
    )
}

fn gcode() -> Vec<u8> {
    let mut bytes = b"; farm3d test artifact\n".to_vec();
    for line in 0..200 {
        bytes.extend_from_slice(format!("M117 line {line}\n").as_bytes());
    }
    bytes
}

fn artifact_for(bytes: &[u8]) -> StagedArtifact {
    StagedArtifact {
        host_path: HOST_PATH.to_string(),
        sha256: format!("{:x}", Sha256::digest(bytes)),
        size: bytes.len() as u64,
    }
}

fn body(bytes: &[u8]) -> Box<dyn tokio::io::AsyncRead + Send + Unpin> {
    Box::new(std::io::Cursor::new(bytes.to_vec()))
}

/// A loopback port with nothing listening on it.
fn closed_port_config() -> ConnectionConfig {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    ConnectionConfig {
        kind: "moonraker".to_string(),
        host: "127.0.0.1".to_string(),
        port,
        use_tls: false,
        credential_ref: None,
    }
}

fn definitive(code: HostOperationFailureCode) -> Result<(), CommandFailure> {
    Err(CommandFailure::Definitive(code))
}

fn indeterminate(reason: InconclusiveReason) -> Result<(), CommandFailure> {
    Err(CommandFailure::Indeterminate {
        reason,
        no_longer_pending: false,
    })
}

async fn upload(fake: &FakeMoonraker, bytes: &[u8]) -> Result<(), CommandFailure> {
    adapter(fake)
        .upload(&artifact_for(bytes), body(bytes))
        .await
}

// --- ArtifactStaging: success paths --------------------------------------------

#[tokio::test]
async fn upload_sends_root_path_checksum_and_file_in_order_and_stores_the_file() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();

    assert_eq!(upload(&fake, &bytes).await, Ok(()));

    assert_eq!(fake.file(HOST_PATH), Some(bytes.clone()));
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/server/files/upload");
    assert_eq!(request.form_parts, ["root", "path", "checksum", "file"]);
    assert_eq!(
        request.form_values,
        [
            ("root".to_string(), "gcodes".to_string()),
            ("path".to_string(), "farm3d".to_string()),
            ("checksum".to_string(), artifact_for(&bytes).sha256),
        ]
    );
    // The fake refuses (and fails the test on) any `print` field; this one
    // had none, and nothing started.
    assert_eq!(fake.print_state().0, "standby");
}

#[tokio::test]
async fn locate_matches_the_uploaded_file() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();
    upload(&fake, &bytes).await.unwrap();

    let outcome = adapter(&fake).locate(&artifact_for(&bytes)).await;

    assert_eq!(outcome, Ok(LocateOutcome::Matches));
    let requests = fake.requests();
    let download = requests.last().unwrap();
    assert_eq!(download.method, "GET");
    assert_eq!(
        download.target,
        "/server/files/gcodes/farm3d/slr-0001.gcode"
    );
}

#[tokio::test]
async fn locate_is_absent_when_nothing_is_at_the_path() {
    let fake = FakeMoonraker::start();
    let outcome = adapter(&fake).locate(&artifact_for(&gcode())).await;
    assert_eq!(outcome, Ok(LocateOutcome::Absent));
}

#[tokio::test]
async fn locate_differs_by_size_for_a_shorter_file_and_by_hash_for_same_size_bytes() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();

    fake.put_file(HOST_PATH, &bytes[..bytes.len() - 10]);
    assert_eq!(
        adapter(&fake).locate(&artifact_for(&bytes)).await,
        Ok(LocateOutcome::Differs {
            reason: DiffersReason::Size {
                actual: bytes.len() as u64 - 10
            }
        })
    );

    let mut same_size = bytes.clone();
    same_size[0] = b'#';
    fake.put_file(HOST_PATH, &same_size);
    assert_eq!(
        adapter(&fake).locate(&artifact_for(&bytes)).await,
        Ok(LocateOutcome::Differs {
            reason: DiffersReason::Hash
        })
    );
}

// --- ArtifactStaging: scripted faults --------------------------------------------

#[tokio::test]
async fn upload_stored_then_response_dropped_is_indeterminate_and_the_file_is_there() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();
    fake.fault(Route::Upload, Fault::StoreThenDropResponse);

    assert_eq!(
        upload(&fake, &bytes).await,
        indeterminate(InconclusiveReason::ResponseLost)
    );
    assert_eq!(
        adapter(&fake).locate(&artifact_for(&bytes)).await,
        Ok(LocateOutcome::Matches)
    );
}

#[tokio::test]
async fn upload_dropped_mid_body_is_indeterminate_and_nothing_is_stored() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();
    fake.fault(Route::Upload, Fault::DropMidBody);

    assert_eq!(
        upload(&fake, &bytes).await,
        indeterminate(InconclusiveReason::ResponseLost)
    );
    assert_eq!(
        adapter(&fake).locate(&artifact_for(&bytes)).await,
        Ok(LocateOutcome::Absent)
    );
}

#[tokio::test]
async fn upload_delivered_late_is_absent_at_first_and_matches_after_the_delay() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();
    fake.fault(
        Route::Upload,
        Fault::DeliverLate(Duration::from_millis(400)),
    );

    assert_eq!(
        upload(&fake, &bytes).await,
        indeterminate(InconclusiveReason::ResponseLost)
    );
    assert_eq!(
        adapter(&fake).locate(&artifact_for(&bytes)).await,
        Ok(LocateOutcome::Absent)
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        adapter(&fake).locate(&artifact_for(&bytes)).await,
        Ok(LocateOutcome::Matches)
    );
}

#[tokio::test]
async fn upload_that_leaves_a_different_file_answers_201_and_locate_says_differs() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();
    let partial = bytes[..bytes.len() / 2].to_vec();
    fake.fault(Route::Upload, Fault::StoreDifferentBytes(partial.clone()));

    assert_eq!(upload(&fake, &bytes).await, Ok(()));
    assert_eq!(
        adapter(&fake).locate(&artifact_for(&bytes)).await,
        Ok(LocateOutcome::Differs {
            reason: DiffersReason::Size {
                actual: partial.len() as u64
            }
        })
    );
}

#[tokio::test]
async fn upload_answered_after_the_timeout_is_indeterminate() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();
    fake.fault(
        Route::Upload,
        Fault::DelayResponse(Duration::from_millis(1500)),
    );

    assert_eq!(
        upload(&fake, &bytes).await,
        indeterminate(InconclusiveReason::ResponseLost)
    );
    // The host applied it; only the answer was late (spike Gate D row 1).
    assert_eq!(fake.file(HOST_PATH), Some(bytes));
}

#[tokio::test]
async fn upload_definitive_rejections() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();

    fake.fault(Route::Upload, Fault::unauthorized());
    assert_eq!(
        upload(&fake, &bytes).await,
        definitive(HostOperationFailureCode::AuthRejected)
    );
    fake.fault(Route::Upload, Fault::file_loaded());
    assert_eq!(
        upload(&fake, &bytes).await,
        definitive(HostOperationFailureCode::FileLoaded)
    );
    fake.fault(Route::Upload, Fault::checksum_mismatch());
    assert_eq!(
        upload(&fake, &bytes).await,
        definitive(HostOperationFailureCode::ChecksumRejected)
    );
    fake.fault(Route::Upload, Fault::respond(400, "Bad Request"));
    assert_eq!(
        upload(&fake, &bytes).await,
        definitive(HostOperationFailureCode::HostRejected)
    );
    fake.fault(Route::Upload, Fault::respond(500, "Internal Server Error"));
    assert_eq!(
        upload(&fake, &bytes).await,
        indeterminate(InconclusiveReason::UnexpectedResponse)
    );
    assert_eq!(fake.file(HOST_PATH), None);
}

#[tokio::test]
async fn upload_with_a_wrong_checksum_is_rejected_by_the_fake_like_moonraker() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();
    let mut wrong = artifact_for(&bytes);
    wrong.sha256 = "0".repeat(64);

    assert_eq!(
        adapter(&fake).upload(&wrong, body(&bytes)).await,
        definitive(HostOperationFailureCode::ChecksumRejected)
    );
    assert_eq!(fake.file(HOST_PATH), None);
}

#[tokio::test]
async fn upload_over_the_loaded_file_is_file_loaded() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();
    upload(&fake, &bytes).await.unwrap();
    adapter(&fake).start(HOST_PATH).await.unwrap();

    assert_eq!(
        upload(&fake, &bytes).await,
        definitive(HostOperationFailureCode::FileLoaded)
    );
}

#[tokio::test]
async fn upload_to_a_closed_port_is_definitively_unreachable() {
    let bytes = gcode();
    let capabilities = MoonrakerCapabilities::new(&closed_port_config(), None, short_timings());
    assert_eq!(
        capabilities
            .upload(&artifact_for(&bytes), body(&bytes))
            .await,
        definitive(HostOperationFailureCode::HostUnreachable)
    );
}

#[tokio::test]
async fn locate_errors_are_never_absent() {
    let fake = FakeMoonraker::start();
    let artifact = artifact_for(&gcode());

    fake.fault(Route::Download, Fault::unauthorized());
    assert!(matches!(
        adapter(&fake).locate(&artifact).await,
        Err(ConnectionError::Auth(_))
    ));
    fake.fault(
        Route::Download,
        Fault::respond(500, "Internal Server Error"),
    );
    assert!(matches!(
        adapter(&fake).locate(&artifact).await,
        Err(ConnectionError::Protocol(_))
    ));
    fake.fault(
        Route::Download,
        Fault::DelayResponse(Duration::from_millis(1500)),
    );
    assert_eq!(
        adapter(&fake).locate(&artifact).await,
        Err(ConnectionError::Timeout)
    );
    let unreachable = MoonrakerCapabilities::new(&closed_port_config(), None, short_timings());
    assert!(matches!(
        unreachable.locate(&artifact).await,
        Err(ConnectionError::Unreachable(_))
    ));
}

#[tokio::test]
async fn locate_stops_reading_a_chunked_body_once_it_passes_the_size() {
    // No `Content-Length`, a body far larger than the artifact, and a body
    // that does not end for 5 s: reading to the end would time out.
    let fake = FakeMoonraker::start();
    let bytes = gcode();
    let mut oversized = bytes.clone();
    oversized.extend(std::iter::repeat_n(b';', 64 * 1024));
    fake.put_file(HOST_PATH, &oversized);
    fake.fault(
        Route::Download,
        Fault::ChunkedDownloadThenStall(Duration::from_secs(5)),
    );

    let started = std::time::Instant::now();
    let outcome = adapter(&fake).locate(&artifact_for(&bytes)).await;

    match outcome {
        Ok(LocateOutcome::Differs {
            reason: DiffersReason::Size { actual },
        }) => assert!(actual > bytes.len() as u64, "actual {actual}"),
        other => panic!("expected Differs by size, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn locate_hashes_a_complete_chunked_body() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();
    fake.put_file(HOST_PATH, &bytes);
    fake.fault(
        Route::Download,
        Fault::ChunkedDownloadThenStall(Duration::ZERO),
    );
    assert_eq!(
        adapter(&fake).locate(&artifact_for(&bytes)).await,
        Ok(LocateOutcome::Matches)
    );
}

// --- PrintControl ------------------------------------------------------------------

async fn staged(fake: &FakeMoonraker) {
    upload(fake, &gcode()).await.unwrap();
}

#[tokio::test]
async fn start_pause_resume_cancel_succeed_and_change_the_host() {
    let fake = FakeMoonraker::start();
    staged(&fake).await;
    let control = adapter(&fake);

    assert_eq!(control.start(HOST_PATH).await, Ok(()));
    assert_eq!(
        fake.print_state(),
        ("printing".into(), HOST_PATH.into(), false)
    );
    let history = fake.history();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, "in_progress");

    assert_eq!(control.pause().await, Ok(()));
    assert_eq!(
        fake.print_state(),
        ("paused".into(), HOST_PATH.into(), true)
    );
    assert_eq!(control.resume().await, Ok(()));
    assert_eq!(
        fake.print_state(),
        ("printing".into(), HOST_PATH.into(), false)
    );
    assert_eq!(control.cancel().await, Ok(()));
    assert_eq!(fake.print_state().0, "cancelled");

    let targets: Vec<String> = fake
        .requests()
        .iter()
        .filter(|request| request.method == "POST")
        .map(|request| request.target.clone())
        .collect();
    assert_eq!(
        targets[1..],
        [
            "/printer/print/start?filename=farm3d%2Fslr-0001.gcode",
            "/printer/print/pause",
            "/printer/print/resume",
            "/printer/print/cancel",
        ]
    );
}

#[tokio::test]
async fn start_applied_then_response_dropped_is_indeterminate_for_each_trace() {
    for trace in [
        StartTrace::PrintingWithHistoryJob,
        StartTrace::PrintingOnly,
        StartTrace::CompleteOnly,
    ] {
        let fake = FakeMoonraker::start();
        staged(&fake).await;
        fake.fault(Route::Start, Fault::ApplyStartThenDrop(trace));

        assert_eq!(
            adapter(&fake).start(HOST_PATH).await,
            indeterminate(InconclusiveReason::ResponseLost),
            "{trace:?}"
        );
        let (state, filename, _) = fake.print_state();
        assert_eq!(filename, HOST_PATH, "{trace:?}");
        match trace {
            StartTrace::PrintingWithHistoryJob => {
                assert_eq!(state, "printing");
                assert_eq!(fake.history().len(), 1);
            }
            StartTrace::PrintingOnly => {
                assert_eq!(state, "printing");
                assert!(fake.history().is_empty());
            }
            StartTrace::CompleteOnly => {
                assert_eq!(state, "complete");
                assert!(fake.history().is_empty());
            }
        }
    }
}

#[tokio::test]
async fn start_definitive_rejections_from_the_host_state() {
    let fake = FakeMoonraker::start();
    let control = adapter(&fake);

    // Nothing staged: 400 "Unable to open file".
    assert_eq!(
        control.start(HOST_PATH).await,
        definitive(HostOperationFailureCode::FileMissing)
    );
    staged(&fake).await;
    control.start(HOST_PATH).await.unwrap();
    // Already printing: 400 "SD busy".
    assert_eq!(
        control.start(HOST_PATH).await,
        definitive(HostOperationFailureCode::HostBusy)
    );
    // Klipper stopped: 503 "Klippy Host not connected".
    fake.with_state(|state| state.klippy_state = "disconnected".to_string());
    assert_eq!(
        control.start(HOST_PATH).await,
        definitive(HostOperationFailureCode::HostNotReady)
    );
    // Klipper shut down: HTTP says only "Unknown" (spike Gate E).
    fake.with_state(|state| state.klippy_state = "shutdown".to_string());
    assert_eq!(
        control.start(HOST_PATH).await,
        definitive(HostOperationFailureCode::HostRejected)
    );
}

#[tokio::test]
async fn scripted_start_rejections_are_definitive() {
    let fake = FakeMoonraker::start();
    staged(&fake).await;
    let control = adapter(&fake);
    for (fault, code) in [
        (Fault::sd_busy(), HostOperationFailureCode::HostBusy),
        (
            Fault::unable_to_open_file(),
            HostOperationFailureCode::FileMissing,
        ),
        (
            Fault::klippy_host_not_connected(),
            HostOperationFailureCode::HostNotReady,
        ),
        (
            Fault::unauthorized(),
            HostOperationFailureCode::AuthRejected,
        ),
    ] {
        fake.fault(Route::Start, fault.clone());
        assert_eq!(
            control.start(HOST_PATH).await,
            definitive(code),
            "{fault:?}"
        );
    }
    assert_eq!(fake.print_state().0, "standby");
}

#[tokio::test]
async fn a_start_dropped_by_a_klipper_restart_is_indeterminate_and_no_longer_pending() {
    let fake = FakeMoonraker::start();
    staged(&fake).await;
    fake.fault(Route::Start, Fault::klippy_disconnected());

    assert_eq!(
        adapter(&fake).start(HOST_PATH).await,
        Err(CommandFailure::Indeterminate {
            reason: InconclusiveReason::KlipperRestarted,
            no_longer_pending: true,
        })
    );
}

#[tokio::test]
async fn control_timeouts_unexpected_answers_and_connect_failures() {
    let fake = FakeMoonraker::start();
    staged(&fake).await;
    let control = adapter(&fake);

    fake.fault(
        Route::Start,
        Fault::DelayResponse(Duration::from_millis(1500)),
    );
    assert_eq!(
        control.start(HOST_PATH).await,
        indeterminate(InconclusiveReason::ResponseLost)
    );
    fake.fault(Route::Pause, Fault::respond(500, "Internal Server Error"));
    assert_eq!(
        control.pause().await,
        indeterminate(InconclusiveReason::UnexpectedResponse)
    );

    let unreachable = MoonrakerCapabilities::new(&closed_port_config(), None, short_timings());
    for result in [
        unreachable.start(HOST_PATH).await,
        unreachable.pause().await,
        unreachable.resume().await,
        unreachable.cancel().await,
    ] {
        assert_eq!(
            result,
            definitive(HostOperationFailureCode::HostUnreachable)
        );
    }
}

#[tokio::test]
async fn control_ok_without_an_effect_is_still_ok_at_dispatch() {
    // D5: the executor's verification window, not the response, proves a
    // pause, resume, or cancel. The adapter only reports the answer.
    let fake = FakeMoonraker::start();
    staged(&fake).await;
    let control = adapter(&fake);
    control.start(HOST_PATH).await.unwrap();

    for route in [Route::Pause, Route::Resume, Route::Cancel] {
        fake.fault(route, Fault::OkWithoutEffect);
    }
    assert_eq!(control.pause().await, Ok(()));
    assert_eq!(control.resume().await, Ok(()));
    assert_eq!(control.cancel().await, Ok(()));
    assert_eq!(
        fake.print_state(),
        ("printing".into(), HOST_PATH.into(), false)
    );
}

#[tokio::test]
async fn control_503s_and_400s_classify_like_start() {
    let fake = FakeMoonraker::start();
    let control = adapter(&fake);
    fake.fault(Route::Cancel, Fault::klippy_disconnected());
    assert_eq!(
        control.cancel().await,
        Err(CommandFailure::Indeterminate {
            reason: InconclusiveReason::KlipperRestarted,
            no_longer_pending: true,
        })
    );
    fake.with_state(|state| state.klippy_state = "disconnected".to_string());
    assert_eq!(
        control.resume().await,
        definitive(HostOperationFailureCode::HostNotReady)
    );
}

#[tokio::test]
async fn a_restart_clears_live_state_but_keeps_files_and_history() {
    let fake = FakeMoonraker::start();
    staged(&fake).await;
    adapter(&fake).start(HOST_PATH).await.unwrap();

    fake.restart();

    assert_eq!(fake.print_state(), ("standby".into(), String::new(), false));
    assert!(fake.file(HOST_PATH).is_some());
    let history = fake.history();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, "klippy_disconnect");
    let state = adapter(&fake).host_job_state().await.unwrap();
    assert_eq!(state.print.unwrap().state, PrintStatsState::Standby);
}

// --- HostStateQuery ------------------------------------------------------------------

#[tokio::test]
async fn host_facts_for_a_single_tool_printer() {
    let fake = FakeMoonraker::start();
    let facts = adapter(&fake).host_facts().await.unwrap();

    assert_eq!(facts.host_software, "Moonraker v0.11.0-1-g1cfb0c4-prind");
    assert_eq!(facts.api_version, "1.5.0");
    assert!(facts.has_virtual_sdcard && facts.has_pause_resume && facts.has_history);
    assert!(facts.has_heater_bed);
    assert_eq!(facts.tool_count, 1);
    assert_eq!(facts.camera_count, 0);
}

#[tokio::test]
async fn host_facts_count_four_tools_and_cameras_and_ignore_look_alikes() {
    let fake = FakeMoonraker::start();
    fake.with_state(|state| {
        state.tools = vec![(200.0, 210.0); 4];
        state.bed = None;
        state.extra_objects = vec!["extruder_offset_calibration".to_string()];
        state.webcams = vec![
            json!({"name": "front", "service": "webrtc-camerastreamer",
                   "stream_url": "http://192.0.2.10/webcam/webrtc"}),
            json!({"name": "top", "service": "mjpegstreamer-adaptive",
                   "stream_url": "/webcam2/?action=stream"}),
        ];
    });

    let facts = adapter(&fake).host_facts().await.unwrap();

    assert_eq!(facts.tool_count, 4);
    assert!(!facts.has_heater_bed);
    assert_eq!(facts.camera_count, 2);
}

#[tokio::test]
async fn host_job_state_reads_every_tool_in_order_and_the_bed() {
    let fake = FakeMoonraker::start();
    fake.with_state(|state| {
        state.tools = vec![(200.0, 210.0), (201.0, 211.0), (202.0, 0.0), (23.5, 0.0)];
    });

    let state = adapter(&fake).host_job_state().await.unwrap();

    assert_eq!(state.klippy_state, KlippyState::Ready);
    assert_eq!(
        state
            .tools
            .iter()
            .map(|tool| (tool.index, tool.temp_c, tool.target_c))
            .collect::<Vec<_>>(),
        [
            (0, Some(200.0), Some(210.0)),
            (1, Some(201.0), Some(211.0)),
            (2, Some(202.0), Some(0.0)),
            (3, Some(23.5), Some(0.0)),
        ]
    );
    let bed = state.bed.unwrap();
    assert_eq!((bed.temp_c, bed.target_c), (60.0, 65.0));
    let print = state.print.unwrap();
    assert_eq!(print.state, PrintStatsState::Standby);
    assert_eq!(print.filename, None);

    let targets: Vec<String> = fake.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(
        targets,
        [
            "/server/info",
            "/printer/objects/list",
            "/printer/objects/query?webhooks&print_stats&pause_resume&heater_bed\
             &extruder&extruder1&extruder2&extruder3",
        ]
    );
}

#[tokio::test]
async fn host_job_state_on_a_no_bed_printer_has_no_bed() {
    let fake = FakeMoonraker::start();
    fake.with_state(|state| state.bed = None);
    let state = adapter(&fake).host_job_state().await.unwrap();
    assert_eq!(state.bed, None);
}

#[tokio::test]
async fn host_job_state_queries_objects_only_when_klipper_is_ready() {
    let fake = FakeMoonraker::start();
    fake.with_state(|state| state.klippy_state = "disconnected".to_string());

    let state = adapter(&fake).host_job_state().await.unwrap();

    assert_eq!(
        state,
        HostJobState {
            klippy_state: KlippyState::Disconnected,
            print: None,
            tools: vec![],
            bed: None,
        }
    );
    let targets: Vec<String> = fake.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(targets, ["/server/info"]);
}

#[tokio::test]
async fn a_klippy_503_on_a_read_is_host_not_ready() {
    let fake = FakeMoonraker::start();
    // server.info can still say `ready` while objects already fail (a
    // restart between the two reads).
    fake.fault(Route::ObjectsList, Fault::klippy_host_not_connected());
    assert_eq!(
        adapter(&fake).host_job_state().await,
        Err(ConnectionError::HostNotReady)
    );
    fake.fault(Route::ObjectsQuery, Fault::klippy_disconnected());
    assert_eq!(
        adapter(&fake).host_job_state().await,
        Err(ConnectionError::HostNotReady)
    );
    fake.fault(Route::History, Fault::klippy_host_not_connected());
    assert_eq!(
        adapter(&fake)
            .job_history(HistoryQuery {
                since_epoch_s: None,
                limit: 50
            })
            .await,
        Err(ConnectionError::HostNotReady)
    );
    // A 503 that is not Klippy's is not evidence about Klipper.
    fake.fault(
        Route::ObjectsList,
        Fault::respond(503, "Service Unavailable"),
    );
    assert!(matches!(
        adapter(&fake).host_job_state().await,
        Err(ConnectionError::Protocol(_))
    ));
    // And no raw body leaks through the new variant.
    let text = format!(
        "{} {:?}",
        ConnectionError::HostNotReady,
        ConnectionError::HostNotReady
    );
    assert!(!text.contains(TRACEBACK_MARKER));
}

#[tokio::test]
async fn host_state_reads_fail_rather_than_guess() {
    let fake = FakeMoonraker::start();
    fake.fault(Route::ServerInfo, Fault::unauthorized());
    assert!(matches!(
        adapter(&fake).host_job_state().await,
        Err(ConnectionError::Auth(_))
    ));
    fake.fault(
        Route::ObjectsQuery,
        Fault::DelayResponse(Duration::from_millis(1500)),
    );
    assert_eq!(
        adapter(&fake).host_job_state().await,
        Err(ConnectionError::Timeout)
    );
    fake.fault(Route::History, Fault::respond(500, "Internal Server Error"));
    assert!(matches!(
        adapter(&fake)
            .job_history(HistoryQuery {
                since_epoch_s: None,
                limit: 50
            })
            .await,
        Err(ConnectionError::Protocol(_))
    ));
}

#[tokio::test]
async fn job_history_sends_limit_order_and_since_and_parses_hex_ids() {
    let fake = FakeMoonraker::start();
    fake.with_state(|state| {
        state.history = vec![
            FakeJob {
                job_id: "000009".into(),
                filename: "farm3d/old.gcode".into(),
                status: "completed".into(),
                start_time: 100.0,
            },
            FakeJob {
                job_id: "not-hex".into(),
                filename: "farm3d/bad.gcode".into(),
                status: "completed".into(),
                start_time: 150.0,
            },
            FakeJob {
                job_id: "0000A1".into(),
                filename: HOST_PATH.into(),
                status: "klippy_disconnect".into(),
                start_time: 200.5,
            },
        ];
    });
    let history = adapter(&fake);

    let all = history
        .job_history(HistoryQuery {
            since_epoch_s: None,
            limit: 50,
        })
        .await
        .unwrap();
    assert_eq!(
        all.iter().map(|job| job.job_id).collect::<Vec<_>>(),
        [0xA1, 9]
    );
    assert_eq!(all[0].filename, HOST_PATH);
    assert_eq!(all[0].status, "klippy_disconnect");
    assert_eq!(all[0].start_time_epoch_s, 200.5);

    let recent = history
        .job_history(HistoryQuery {
            since_epoch_s: Some(170.0),
            limit: 50,
        })
        .await
        .unwrap();
    assert_eq!(
        recent.iter().map(|job| job.job_id).collect::<Vec<_>>(),
        [0xA1]
    );

    let targets: Vec<String> = fake.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(
        targets,
        [
            "/server/history/list?limit=50&order=desc",
            "/server/history/list?limit=50&order=desc&since=170",
        ]
    );
}

// --- CameraDiscovery ------------------------------------------------------------------

#[tokio::test]
async fn cameras_keep_name_and_service_only() {
    let fake = FakeMoonraker::start();
    fake.with_state(|state| {
        state.webcams = vec![json!({"name": "front", "service": "webrtc-camerastreamer",
            "stream_url": "http://192.0.2.10/webcam/webrtc",
            "snapshot_url": "http://192.0.2.10/webcam/snapshot"})];
    });

    let cameras = adapter(&fake).cameras().await.unwrap();

    assert_eq!(cameras.len(), 1);
    assert_eq!(cameras[0].name, "front");
    assert_eq!(cameras[0].service, "webrtc-camerastreamer");
    assert!(!format!("{cameras:?}").contains("192.0.2.10"));
}

// --- credentials ------------------------------------------------------------------------

#[tokio::test]
async fn the_api_key_goes_on_every_request() {
    let fake = FakeMoonraker::start();
    fake.with_state(|state| state.api_key = Some(SEEDED_SECRET.to_string()));
    let capabilities = adapter_with_key(&fake.config(), SEEDED_SECRET);
    let bytes = gcode();
    let artifact = artifact_for(&bytes);

    capabilities.upload(&artifact, body(&bytes)).await.unwrap();
    assert_eq!(
        capabilities.locate(&artifact).await,
        Ok(LocateOutcome::Matches)
    );
    capabilities.start(HOST_PATH).await.unwrap();
    capabilities.pause().await.unwrap();
    capabilities.resume().await.unwrap();
    capabilities.cancel().await.unwrap();
    capabilities.host_facts().await.unwrap();
    capabilities.host_job_state().await.unwrap();
    capabilities
        .job_history(HistoryQuery {
            since_epoch_s: None,
            limit: 50,
        })
        .await
        .unwrap();
    capabilities.cameras().await.unwrap();

    let requests = fake.requests();
    assert!(requests.len() >= 12);
    for request in requests {
        assert_eq!(
            request.headers.get("x-api-key").map(String::as_str),
            Some(SEEDED_SECRET),
            "{}",
            request.target
        );
    }
}

#[tokio::test]
async fn the_seeded_secret_never_appears_in_an_error_or_debug_output() {
    let fake = FakeMoonraker::start();
    fake.with_state(|state| state.api_key = Some("a-different-key".to_string()));
    let rejected = adapter_with_key(&fake.config(), SEEDED_SECRET);
    let unreachable = adapter_with_key(&closed_port_config(), SEEDED_SECRET);
    let bytes = gcode();
    let artifact = artifact_for(&bytes);

    let mut seen = vec![format!("{rejected:?}"), format!("{unreachable:?}")];
    for capabilities in [&rejected, &unreachable] {
        seen.push(format!(
            "{:?}",
            capabilities.upload(&artifact, body(&bytes)).await
        ));
        seen.push(format!("{:?}", capabilities.start(HOST_PATH).await));
        seen.push(format!("{:?}", capabilities.cancel().await));
        for error in [
            capabilities.locate(&artifact).await.unwrap_err(),
            capabilities.host_facts().await.unwrap_err(),
            capabilities.host_job_state().await.unwrap_err(),
            capabilities
                .job_history(HistoryQuery {
                    since_epoch_s: None,
                    limit: 50,
                })
                .await
                .unwrap_err(),
            capabilities.cameras().await.unwrap_err(),
        ] {
            seen.push(format!("{error} {error:?}"));
        }
    }
    // A timeout's error, too.
    fake.with_state(|state| state.api_key = None);
    fake.fault(
        Route::ServerInfo,
        Fault::DelayResponse(Duration::from_millis(1500)),
    );
    let error = adapter_with_key(&fake.config(), SEEDED_SECRET)
        .host_facts()
        .await
        .unwrap_err();
    seen.push(format!("{error} {error:?}"));

    for text in &seen {
        assert!(!text.contains(SEEDED_SECRET), "leaked: {text}");
        assert!(!text.contains(TRACEBACK_MARKER), "raw body leaked: {text}");
    }
    assert!(seen.iter().any(|text| text.contains("AuthRejected")));
}

#[tokio::test]
async fn a_tls_connection_never_reaches_the_network() {
    let fake = FakeMoonraker::start();
    let mut config = fake.config();
    config.use_tls = true;
    let capabilities = MoonrakerCapabilities::new(&config, None, short_timings());
    let bytes = gcode();

    assert_eq!(
        capabilities
            .upload(&artifact_for(&bytes), body(&bytes))
            .await,
        definitive(HostOperationFailureCode::HostUnreachable)
    );
    assert!(capabilities.host_facts().await.is_err());
    assert!(fake.requests().is_empty());
}

// --- the fake itself ------------------------------------------------------------------------

#[tokio::test]
async fn the_fake_refuses_and_counts_an_upload_with_a_print_field() {
    let fake = FakeMoonraker::start();
    let bytes = gcode();
    // Built by hand: the adapter has no way to send `print`.
    let form = reqwest::multipart::Form::new()
        .text("root", "gcodes")
        .text("path", "farm3d")
        .text("print", "true")
        .part(
            "file",
            reqwest::multipart::Part::bytes(bytes).file_name("slr-0001.gcode"),
        );
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!(
            "http://127.0.0.1:{}/server/files/upload",
            fake.port
        ))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status().as_u16(), 400);
    assert_eq!(fake.take_print_field_uploads(), 1);
    assert_eq!(fake.file(HOST_PATH), None);
    assert_eq!(fake.print_state().0, "standby");
}

#[tokio::test]
async fn timings_default_to_the_spec_values() {
    let timings = MoonrakerTimings::default();
    assert_eq!(timings.connect, Duration::from_secs(5));
    assert_eq!(timings.query, Duration::from_secs(10));
    assert_eq!(timings.control, Duration::from_secs(60));
    assert_eq!(timings.transfer_base, Duration::from_secs(60));
    assert_eq!(timings.transfer_per_started_mib, Duration::from_secs(1));
    // 60 s plus 1 s per *started* MiB.
    assert_eq!(timings.transfer(0), Duration::from_secs(60));
    assert_eq!(timings.transfer(1), Duration::from_secs(61));
    assert_eq!(timings.transfer(1024 * 1024), Duration::from_secs(61));
    assert_eq!(timings.transfer(1024 * 1024 + 1), Duration::from_secs(62));
}
