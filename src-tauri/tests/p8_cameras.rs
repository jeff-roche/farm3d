//! Task 7 (P8 D4): camera sources, host-webcam resolution, the bounded
//! `FrameFetcher`, `CameraServices` health, and the camera commands.
//!
//! - Pure: manual-URL validation and host-webcam URL resolution.
//! - `FrameFetcher` against `FakeCamera` (a loopback HTTP server with
//!   switchable answers): JPEG and PNG pass; HTML sent as `image/jpeg`,
//!   more than 10 MiB, a slow answer, and a redirect each fail typed; no
//!   error ever carries a URL, host, or port.
//! - The commands over `FakeMoonraker` and `FakeCamera`: set/get/clear,
//!   host-webcam listing, `test_camera` (writes nothing; a draft never
//!   touches the saved source's health), `camera_preview_frame` (health
//!   and `camera.health.changed` only on a change; one fetch per Printer
//!   at a time), OctoPrint's `unsupportedAdapter`, and the seeded-secret
//!   scans (global constraint 3).
//! - Task 8, the optionality tests (global constraint 5): with a capture
//!   hanging on `FakeCamera`, `printer_statuses`, a slice start,
//!   `assign_queue_entry`, and `start_job` each complete within their
//!   normal bounds, and the Attention projector keeps passing.

mod common;
mod p5_harness;
mod p7_dispatch_rig;

use farm3d_lib::cameras::config::{validate_snapshot_url, validate_source};
use farm3d_lib::cameras::resolve::{resolve_listed, resolve_webcam_url};
use farm3d_lib::cameras::{CameraErrorKind, CameraSource};
use zeroize::Zeroizing;

// --- Step 1: pure validation and resolution --------------------------------------

fn field_of(error: &farm3d_lib::contracts::command::CommandError) -> String {
    let value = serde_json::to_value(error).unwrap();
    assert_eq!(value["code"], "VALIDATION", "{value}");
    value["details"]["fieldPath"].as_str().unwrap().to_string()
}

#[test]
fn a_manual_url_must_be_plain_http_with_a_host_and_no_userinfo_or_fragment() {
    for accepted in [
        "http://192.0.2.10/webcam/?action=snapshot",
        "http://192.0.2.10:8080/snapshot.jpg",
        "http://camera.example/snap?token=abc&size=hd",
        "http://192.0.2.10:1/x",
        "http://192.0.2.10:65535/x",
        "http://[2001:db8::1]:8080/snap",
        "  http://192.0.2.10/snap  ",
        "http://a",
    ] {
        let stored = validate_snapshot_url(accepted, "source.snapshotUrl")
            .unwrap_or_else(|error| panic!("{accepted:?} was refused: {error:?}"));
        assert_eq!(stored, accepted.trim());
    }
    let long_path = "a".repeat(2048);
    for (rejected, why) in [
        ("https://192.0.2.10/snap", "https"),
        ("HTTP://192.0.2.10/snap", "an upper-case scheme"),
        ("ftp://192.0.2.10/snap", "ftp"),
        ("file:///etc/passwd", "file"),
        ("rtsp://192.0.2.10/stream", "rtsp"),
        ("192.0.2.10/snap", "no scheme"),
        ("http://", "no host"),
        ("http:///snapshot", "an empty authority"),
        ("http://:8080/snap", "an empty host"),
        ("http://user@192.0.2.10/snap", "a user name"),
        ("http://user:SEEDPASS@192.0.2.10/snap", "a user name and password"),
        ("http://@192.0.2.10/snap", "an empty userinfo"),
        ("http://192.0.2.10/snap#frag", "a fragment"),
        ("http://192.0.2.10/snap#", "an empty fragment"),
        ("http://192.0.2.10:0/snap", "port 0"),
        ("http://192.0.2.10:65536/snap", "port 65536"),
        ("http://192.0.2.10/sn ap", "a space"),
        ("http://192.0.2.10/sn\tap", "a tab"),
        ("http://192.0.2.10/sn\u{1}ap", "an embedded control"),
        ("http://192.0.2.10/sn\nap", "an embedded newline"),
        ("", "empty"),
    ] {
        let error = validate_snapshot_url(rejected, "source.snapshotUrl")
            .expect_err(&format!("{why} must be refused"));
        assert_eq!(field_of(&error), "source.snapshotUrl", "{why}");
        let text = format!("{error:?} {}", serde_json::to_string(&error).unwrap());
        assert!(!text.contains("SEEDPASS") && !text.contains("192.0.2.10"), "{why}: {text}");
    }
    let too_long = format!("http://192.0.2.10/{long_path}");
    assert_eq!(field_of(&validate_snapshot_url(&too_long, "camera.snapshotUrl").unwrap_err()), "camera.snapshotUrl");
}

#[test]
fn a_source_is_validated_field_by_field() {
    let webcam = |name: &str, service: Option<&str>, port: Option<u16>| CameraSource::HostWebcam {
        webcam_name: name.to_string(),
        webcam_service: service.map(str::to_string),
        web_port: port,
    };
    assert_eq!(
        validate_source(&webcam("  front ", Some(" mjpegstreamer "), Some(8080)), "source").unwrap(),
        webcam("front", Some("mjpegstreamer"), Some(8080))
    );
    assert_eq!(
        validate_source(&webcam("front", Some("  "), None), "source").unwrap(),
        webcam("front", None, None),
        "a blank service is no service"
    );
    for (source, field) in [
        (webcam("", None, None), "source.webcamName"),
        (webcam("   ", None, None), "source.webcamName"),
        (webcam(&"n".repeat(129), None, None), "source.webcamName"),
        (webcam("bad\u{7}name", None, None), "source.webcamName"),
        (webcam("front", Some(&"s".repeat(65)), None), "source.webcamService"),
        (webcam("front", None, Some(0)), "source.webPort"),
        (
            CameraSource::SnapshotUrl {
                snapshot_url: "https://192.0.2.10/x".to_string(),
            },
            "source.snapshotUrl",
        ),
    ] {
        assert_eq!(field_of(&validate_source(&source, "source").unwrap_err()), field);
    }
    assert_eq!(
        field_of(&validate_source(&webcam("", None, None), "camera").unwrap_err()),
        "camera.webcamName"
    );
    assert_eq!(validate_source(&webcam(&"n".repeat(128), None, Some(65535)), "source").unwrap(), webcam(&"n".repeat(128), None, Some(65535)));
}

fn resolved(host: &str, port: Option<u16>, url: &str) -> Result<String, CameraErrorKind> {
    resolve_webcam_url(host, port, url).map(|url| url.to_string())
}

#[test]
fn a_relative_webcam_url_resolves_against_the_connection_host_and_web_port() {
    assert_eq!(
        resolved("192.0.2.10", Some(8080), "/webcam/?action=snapshot").unwrap(),
        "http://192.0.2.10:8080/webcam/?action=snapshot"
    );
    assert_eq!(
        resolved("192.0.2.10", None, "/webcam/?action=snapshot").unwrap(),
        "http://192.0.2.10/webcam/?action=snapshot",
        "no webPort is port 80"
    );
    assert_eq!(
        resolved("192.0.2.10", Some(8080), "webcam/snapshot.jpg").unwrap(),
        "http://192.0.2.10:8080/webcam/snapshot.jpg",
        "a path-relative value joins the base path"
    );
    assert_eq!(
        resolved("printer.example", Some(81), "/snap#part").unwrap(),
        "http://printer.example:81/snap",
        "the fragment is dropped"
    );
    assert_eq!(
        resolved("2001:db8::1", Some(8080), "/snap").unwrap(),
        "http://[2001:db8::1]:8080/snap",
        "an IPv6 Connection host is bracketed"
    );
}

#[test]
fn an_absolute_webcam_url_must_stay_on_the_connection_host() {
    assert_eq!(
        resolved("192.0.2.10", Some(8080), "http://192.0.2.10:8081/snap?token=abc").unwrap(),
        "http://192.0.2.10:8081/snap?token=abc",
        "an absolute URL on the Connection host is used as it is"
    );
    assert_eq!(
        resolved("Printer.EXAMPLE", None, "http://printer.example/snap").unwrap(),
        "http://printer.example/snap",
        "the host compares ASCII case-insensitively"
    );
    assert_eq!(
        resolved("192.0.2.10", Some(8080), "//192.0.2.10:9000/snap").unwrap(),
        "http://192.0.2.10:9000/snap",
        "a scheme-relative value on the same host is accepted"
    );
    for (value, why) in [
        ("http://192.0.2.99/snap", "another host"),
        ("//192.0.2.99/snap", "a scheme-relative value on another host"),
        ("https://192.0.2.10/snap", "https"),
        ("ftp://192.0.2.10/snap", "another scheme"),
        ("http://user:pass@192.0.2.10/snap", "userinfo"),
        ("http://user@192.0.2.10/snap", "a user name"),
    ] {
        assert_eq!(
            resolved("192.0.2.10", Some(8080), value).unwrap_err(),
            CameraErrorKind::HostMismatch,
            "{why}"
        );
    }
}

#[test]
fn a_missing_webcam_or_snapshot_url_is_typed() {
    assert_eq!(
        resolve_listed("192.0.2.10", None, None).unwrap_err(),
        CameraErrorKind::NoSuchWebcam
    );
    for empty in ["", "   "] {
        assert_eq!(
            resolve_listed("192.0.2.10", None, Some(Zeroizing::new(empty.to_string()))).unwrap_err(),
            CameraErrorKind::NoSnapshotUrl
        );
    }
    assert_eq!(
        resolve_listed("192.0.2.10", None, Some(Zeroizing::new("/snap".to_string())))
            .unwrap()
            .as_str(),
        "http://192.0.2.10/snap"
    );
    assert_eq!(
        resolve_listed("192.0.2.10", None, Some(Zeroizing::new("http://192.0.2.99/snap".to_string())))
            .unwrap_err(),
        CameraErrorKind::HostMismatch
    );
}

// --- Step 2: the FrameFetcher against FakeCamera -----------------------------------

use std::time::{Duration, Instant};

use common::fake_camera::{Answer, FakeCamera, JPEG, PNG};
use farm3d_lib::cameras::fetch::{encode_frame, sniff, CameraError, FrameFetcher, MAX_FRAME_BYTES};
use farm3d_lib::cameras::services::CameraTimings;
use farm3d_lib::cameras::CameraContentType;
use farm3d_lib::contracts::command::CommandError;

/// The seeded-secret corpus (global constraint 3): a password in a
/// userinfo attempt, a token in a manual URL's query, and a host webcam
/// whose `snapshot_url` names an RFC 5737 host and carries a token.
const SEED_PASS: &str = "SEEDED-P8-CAMERA-PASS-3f9a";
const SEED_QUERY_TOKEN: &str = "SEEDED-P8-CAMERA-TOKEN-b21c";
const SEED_WEBCAM_TOKEN: &str = "SEEDED-P8-WEBCAM-TOKEN-77d0";
const SEED_HOST: &str = "192.0.2.77";

/// Fails when `text` names any part of the corpus or any of `endpoints`
/// (loopback hosts and ports in play), or any URL at all.
fn assert_clean(text: &str, endpoints: &[String]) {
    for needle in [SEED_PASS, SEED_QUERY_TOKEN, SEED_WEBCAM_TOKEN, SEED_HOST, "http://", "https://", "127.0.0.1", "snapshot?"]
        .iter()
        .map(|needle| needle.to_string())
        .chain(endpoints.iter().cloned())
    {
        assert!(!text.contains(&needle), "{needle:?} leaked into: {text}");
    }
}

fn camera_error_text(error: &CameraError) -> String {
    format!("{error} | {error:?} | {}", serde_json::to_string(error).unwrap())
}

fn production_fetcher() -> FrameFetcher {
    FrameFetcher::new(CameraTimings::default().fetch)
}

async fn fetch_error(fetcher: &FrameFetcher, camera: &FakeCamera, path: &str) -> CameraError {
    let error = fetcher.fetch(&camera.url(path)).await.expect_err("the fetch must fail");
    assert_clean(&camera_error_text(&error), &[camera.port.to_string()]);
    error
}

#[test]
fn magic_bytes_decide_the_type_and_the_frame_encoding_is_length_prefixed() {
    assert_eq!(sniff(JPEG), Some(CameraContentType::Jpeg));
    assert_eq!(sniff(PNG), Some(CameraContentType::Png));
    for other in [&b""[..], b"\xFF\xD8", b"<html>", b"--boundarydonotcross", b"\x89PNG\r\n\x1a"] {
        assert_eq!(sniff(other), None, "{other:?}");
    }
    let header = farm3d_lib::cameras::FrameHeader {
        content_type: CameraContentType::Png,
        captured_at: "2026-09-28T09:00:00.000Z".to_string(),
        byte_len: PNG.len() as i64,
        snapshot_id: None,
    };
    let encoded = encode_frame(&header, PNG);
    let length = u32::from_be_bytes(encoded[..4].try_into().unwrap()) as usize;
    assert!(length <= 4096);
    let decoded: serde_json::Value = serde_json::from_slice(&encoded[4..4 + length]).unwrap();
    assert_eq!(
        decoded,
        serde_json::json!({
            "contentType": "image/png",
            "capturedAt": "2026-09-28T09:00:00.000Z",
            "byteLen": PNG.len(),
            "snapshotId": null,
        })
    );
    assert_eq!(&encoded[4 + length..], PNG);
}

#[test]
fn the_production_budget_is_five_seconds_and_ten_mib() {
    assert_eq!(CameraTimings::default().fetch, Duration::from_secs(5));
    assert_eq!(CameraTimings::default().preview_min_interval, Duration::from_secs(1));
    assert_eq!(MAX_FRAME_BYTES, 10_485_760);
}

#[tokio::test]
async fn a_jpeg_and_a_png_pass_with_no_credential_sent() {
    let camera = FakeCamera::start(Answer::Jpeg);
    let fetcher = production_fetcher();
    let before = chrono::Utc::now();
    let frame = fetcher.fetch(&camera.url("/snapshot?token=abc")).await.unwrap();
    assert_eq!(frame.content_type, CameraContentType::Jpeg);
    assert_eq!(frame.bytes, JPEG);
    assert!(frame.captured_at >= before);
    camera.answer(Answer::Png);
    let frame = fetcher.fetch(&camera.url("/snapshot.png")).await.unwrap();
    assert_eq!(frame.content_type, CameraContentType::Png);
    assert_eq!(frame.bytes, PNG);
    let requests = camera.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/snapshot?token=abc");
    for request in &requests {
        for credential in ["x-api-key", "authorization", "cookie"] {
            assert!(request.header(credential).is_none(), "{credential} was sent");
        }
    }
}

#[tokio::test]
async fn html_sent_as_a_jpeg_or_an_mjpeg_stream_is_not_an_image() {
    let camera = FakeCamera::start(Answer::HtmlAsJpeg);
    let fetcher = production_fetcher();
    assert_eq!(fetch_error(&fetcher, &camera, "/snap").await.kind(), CameraErrorKind::NotAnImage);
    camera.answer(Answer::Mjpeg);
    assert_eq!(fetch_error(&fetcher, &camera, "/stream").await.kind(), CameraErrorKind::NotAnImage);
}

#[tokio::test]
async fn more_than_ten_mib_is_too_large_and_the_stream_is_cut_off() {
    let camera = FakeCamera::start(Answer::HugeStreamed);
    let fetcher = production_fetcher();
    let error = fetch_error(&fetcher, &camera, "/huge").await;
    assert_eq!(error.kind(), CameraErrorKind::TooLarge);
    let deadline = Instant::now() + Duration::from_secs(15);
    let written = loop {
        if let Some(written) = camera.huge_written() {
            break written;
        }
        assert!(Instant::now() < deadline, "the camera never saw the client hang up");
        // Yield to the runtime: the client's connection task is what closes
        // the dropped response's socket.
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(
        written < common::fake_camera::HUGE_BYTES,
        "the client read the whole {written}-byte body instead of cutting it off"
    );
    camera.answer(Answer::HugeDeclared);
    assert_eq!(fetch_error(&fetcher, &camera, "/declared").await.kind(), CameraErrorKind::TooLarge);
}

#[tokio::test]
async fn a_six_second_answer_times_out_at_five() {
    // The one deliberate slow case: the production 5 s budget against a
    // 6 s camera.
    let camera = FakeCamera::start(Answer::Slow(Duration::from_secs(6)));
    let fetcher = production_fetcher();
    let started = Instant::now();
    let error = fetch_error(&fetcher, &camera, "/slow").await;
    assert_eq!(error.kind(), CameraErrorKind::Timeout);
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(4900) && took < Duration::from_millis(5900), "{took:?}");
}

#[tokio::test]
async fn a_redirect_is_an_http_status_and_is_never_followed() {
    let camera = FakeCamera::start(Answer::Redirect);
    let fetcher = production_fetcher();
    let error = fetch_error(&fetcher, &camera, "/snap").await;
    assert_eq!(error.kind(), CameraErrorKind::HttpStatus);
    assert_eq!(error.status(), Some(302));
    assert_eq!(camera.requests().len(), 1, "the redirect was followed");
    camera.answer(Answer::Status(404));
    let error = fetch_error(&fetcher, &camera, "/missing").await;
    assert_eq!((error.kind(), error.status()), (CameraErrorKind::HttpStatus, Some(404)));
}

#[tokio::test]
async fn a_closed_port_is_unreachable() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let error = production_fetcher()
        .fetch(&format!("http://127.0.0.1:{port}/snap?token={SEED_QUERY_TOKEN}"))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), CameraErrorKind::Unreachable);
    assert_clean(&camera_error_text(&error), &[port.to_string()]);
}

/// Every kind's `Display`, `Debug`, and serialization, and the command
/// error each maps to, are endpoint-free.
#[test]
fn no_camera_error_or_its_command_error_names_an_endpoint() {
    for kind in CameraErrorKind::ALL {
        let error = if kind == CameraErrorKind::HttpStatus {
            CameraError::http_status(503)
        } else {
            CameraError::new(kind)
        };
        assert_clean(&camera_error_text(&error), &[]);
        for printer in [Some("prn-cam"), None] {
            let command: CommandError = farm3d_lib::cameras::commands::camera_command_error(printer, &error);
            assert_clean(&serde_json::to_string(&command).unwrap(), &[]);
        }
    }
}

// --- Step 3: the camera commands ------------------------------------------------------

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::fake_moonraker::FakeMoonraker;
use farm3d_lib::cameras::services::CameraServices;
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::supervisor::STATUS_EVENT;
use farm3d_lib::connections::{ConnectionConfig, OCTOPRINT_KIND};
use farm3d_lib::persistence::{MetadataRootLease, Storage};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::StoredPrinter;
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeResponseBody};
use tauri::test::{MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::Listener;

/// A Moonraker Printer on `FakeMoonraker` (API key [`SECRET`]).
const MOON: &str = "prn-cam";
/// An OctoPrint Printer (never contacted: nothing here needs its host).
const OCTO: &str = "prn-octo";
/// A Printer with no Connection.
const BARE: &str = "prn-bare";
const SECRET: &str = "SEEDED-P8-CAMERA-API-KEY-5e1d";
const CREDENTIAL_REF: &str = "farm3d/printer/prn-cam/apikey";

struct Rig {
    _temp: tempfile::TempDir,
    _lease: MetadataRootLease,
    root: PathBuf,
    storage: Arc<Storage>,
    moonraker: FakeMoonraker,
    camera: FakeCamera,
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
    events: Arc<Mutex<Vec<String>>>,
}

impl Rig {
    /// Three Printers (Moonraker with a credential, OctoPrint, and one with
    /// no Connection) and a camera answering JPEGs.
    fn new(timings: CameraTimings) -> Self {
        let (temp, lease, storage, _database) = common::storage();
        let moonraker = FakeMoonraker::start();
        moonraker.with_state(|state| state.api_key = Some(SECRET.to_string()));
        let credentials = temp.path().join("credentials");
        CredentialStore::file_backed(credentials.clone())
            .set(CREDENTIAL_REF, SECRET)
            .unwrap();
        let printers = PrinterRepository::new(Arc::clone(&storage));
        printers
            .create(StoredPrinter {
                connection: Some(ConnectionConfig {
                    credential_ref: Some(CREDENTIAL_REF.to_string()),
                    ..moonraker.config()
                }),
                ..common::a_stored_printer(MOON)
            })
            .unwrap();
        printers
            .create(StoredPrinter {
                connection: Some(ConnectionConfig {
                    kind: OCTOPRINT_KIND.to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 9,
                    use_tls: false,
                    credential_ref: None,
                }),
                ..common::a_stored_printer(OCTO)
            })
            .unwrap();
        printers.create(common::a_stored_printer(BARE)).unwrap();
        let (app, webview, _manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::cameras::commands::get_printer_camera,
                farm3d_lib::cameras::commands::set_printer_camera,
                farm3d_lib::cameras::commands::clear_printer_camera,
                farm3d_lib::cameras::commands::list_host_webcams,
                farm3d_lib::cameras::commands::test_camera,
                farm3d_lib::cameras::commands::camera_preview_frame,
                farm3d_lib::attention::commands::list_attention,
                farm3d_lib::printers::commands::archive_printer,
                farm3d_lib::printers::commands::delete_printer,
            ],
            Arc::clone(&storage),
            Arc::new(common::a_catalog()),
            credentials,
            |_config, _key| None,
            move |services| services.cameras = Arc::new(CameraServices::new(timings)),
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        app.listen(STATUS_EVENT, move |event| {
            sink.lock().unwrap().push(event.payload().to_string());
        });
        Self {
            root: temp.path().to_path_buf(),
            _temp: temp,
            _lease: lease,
            storage,
            moonraker,
            camera: FakeCamera::start(Answer::Jpeg),
            _app: app,
            webview,
            services,
            events,
        }
    }

    /// Previews always fetch (no reuse window), so each call is one fetch.
    fn eager() -> Self {
        Self::new(CameraTimings {
            preview_min_interval: Duration::ZERO,
            ..CameraTimings::default()
        })
    }

    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|success| success["data"].clone())
    }

    fn ok(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    fn err(&self, command: &str, body: Value) -> Value {
        match self.call(command, body) {
            Ok(value) => panic!("{command} succeeded: {value}"),
            Err(error) => error,
        }
    }

    /// A binary command's frame: its header and image bytes.
    fn frame(&self, command: &str, body: Value) -> Result<(Value, Vec<u8>), Value> {
        binary(&self.webview, command, body)
    }

    fn preview(&self, printer: &str) -> Result<(Value, Vec<u8>), Value> {
        self.frame("camera_preview_frame", json!({"printerId": printer}))
    }

    fn set(&self, operation: &str, printer: &str, source: Value) -> Result<Value, Value> {
        self.call(
            "set_printer_camera",
            json!({"operationId": operation, "printerId": printer, "source": source}),
        )
    }

    fn snapshot_source(&self, path: &str) -> Value {
        json!({"kind": "snapshotUrl", "snapshotUrl": self.camera.url(path)})
    }

    fn webcam_source(&self) -> Value {
        json!({"kind": "hostWebcam", "webcamName": "front", "webcamService": "mjpegstreamer-adaptive", "webPort": self.camera.port})
    }

    /// The Moonraker fake's webcam list: one webcam, `front`, with this
    /// `snapshot_url` (and a stream URL that must never surface).
    fn webcams(&self, snapshot_url: Option<&str>) {
        let mut entry = json!({
            "name": "front", "service": "mjpegstreamer-adaptive", "enabled": true,
            "stream_url": format!("http://{SEED_HOST}/webcam/?action=stream&token={SEED_WEBCAM_TOKEN}"),
        });
        if let Some(url) = snapshot_url {
            entry["snapshot_url"] = json!(url);
        }
        self.moonraker.with_state(|state| state.webcams = vec![entry]);
    }

    fn connection(&self, credential: &str) -> Value {
        json!({"kind": "moonraker", "host": "127.0.0.1", "port": self.moonraker.port, "credential": credential})
    }

    /// Every `camera.health.changed` payload for `printer`, in order.
    fn health_events(&self, printer: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|text| serde_json::from_str::<Value>(text).unwrap())
            .filter(|event| event["type"] == "camera.health.changed" && event["subject"]["id"] == printer)
            .map(|event| {
                assert_eq!(event["subject"]["kind"], "printer");
                event["payload"]["health"].clone()
            })
            .collect()
    }

    fn all_events(&self) -> String {
        self.events.lock().unwrap().join("\n")
    }

    fn count(&self, sql: &str) -> i64 {
        self.storage.read(|connection| connection.query_row(sql, [], |row| row.get(0))).unwrap()
    }

    /// The endpoints in play (loopback), for [`assert_clean`]. The bare
    /// camera port is not one: a host webcam's `webPort` is configuration
    /// the summary returns, as the Connection editor gets its port.
    fn endpoints(&self) -> Vec<String> {
        vec![
            format!("127.0.0.1:{}", self.camera.port),
            format!("127.0.0.1:{}", self.moonraker.port),
            format!(":{}/", self.camera.port),
            SECRET.to_string(),
        ]
    }

    fn assert_clean(&self, text: &str) {
        assert_clean(text, &self.endpoints());
    }

    /// Waits (condition, not time) until `done`.
    fn wait_until(&self, what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn binary(
    webview: &tauri::WebviewWindow<MockRuntime>,
    command: &str,
    mut body: Value,
) -> Result<(Value, Vec<u8>), Value> {
    body["contractVersion"] = json!(1);
    let response = tauri::test::get_ipc_response(
        webview,
        InvokeRequest {
            cmd: command.to_string(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: body.into(),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )?;
    let bytes = match response {
        InvokeResponseBody::Raw(bytes) => bytes,
        InvokeResponseBody::Json(json) => panic!("{command} answered JSON, not a frame: {json}"),
    };
    let length = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
    assert!(length <= 4096, "header length {length}");
    let header: Value = serde_json::from_slice(&bytes[4..4 + length]).unwrap();
    let image = bytes[4 + length..].to_vec();
    assert_eq!(header["byteLen"], json!(image.len()));
    Ok((header, image))
}

/// Every file under `root` except the SQLite journal files, relative.
fn files_under(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let name = path.strip_prefix(root).unwrap().display().to_string();
                if !name.ends_with("-wal") && !name.ends_with("-shm") {
                    found.push(name);
                }
            }
        }
    }
    found.sort();
    found
}

#[test]
fn set_get_and_clear_round_trip_and_only_get_returns_the_url() {
    let rig = Rig::eager();
    assert_eq!(rig.ok("get_printer_camera", json!({"printerId": MOON})), Value::Null);
    let token_url = rig.camera.url(&format!("/snapshot?token={SEED_QUERY_TOKEN}"));
    let source = json!({"kind": "snapshotUrl", "snapshotUrl": token_url});

    let summary = rig.set("op-set", MOON, source.clone()).unwrap();
    assert_eq!(summary["printerId"], MOON);
    assert_eq!(summary["revision"], 1);
    assert_eq!(summary["sourceKind"], "snapshotUrl");
    assert_eq!(summary["hasSnapshotUrl"], true);
    assert_eq!(summary["webcamName"], Value::Null);
    assert!(summary["updatedAt"].is_string());
    rig.assert_clean(&summary.to_string());
    let health = rig.health_events(MOON);
    assert_eq!(health.len(), 1);
    assert_eq!(
        health[0],
        json!({"printerId": MOON, "state": "unknown", "sourceKind": "snapshotUrl",
               "lastSuccessAt": null, "lastFailureAt": null, "lastFailureKind": null})
    );

    // The one exception: get_printer_camera returns the manual URL.
    let camera = rig.ok("get_printer_camera", json!({"printerId": MOON}));
    assert_eq!(camera["source"], source);
    assert_eq!(camera["revision"], 1);

    // A replay returns the current summary and publishes nothing; a reused
    // id with another request is VALIDATION on operationId.
    assert_eq!(rig.set("op-set", MOON, source.clone()).unwrap(), summary);
    let reused = rig.set("op-set", MOON, rig.snapshot_source("/other.jpg")).unwrap_err();
    assert_eq!(reused["details"]["fieldPath"], "operationId");
    // The same source under a new id changes nothing.
    assert_eq!(rig.set("op-set-same", MOON, source.clone()).unwrap()["revision"], 1);
    assert_eq!(rig.health_events(MOON).len(), 1, "a replay or no-op publishes nothing");

    // A rejected request never burns its id.
    let userinfo = json!({"kind": "snapshotUrl", "snapshotUrl": format!("http://user:{SEED_PASS}@{SEED_HOST}/snap")});
    let refused = rig.set("op-webcam", MOON, userinfo).unwrap_err();
    assert_eq!(refused["code"], "VALIDATION");
    assert_eq!(refused["details"]["fieldPath"], "source.snapshotUrl");
    rig.assert_clean(&refused.to_string());
    let summary = rig.set("op-webcam", MOON, rig.webcam_source()).unwrap();
    assert_eq!(summary["revision"], 2);
    assert_eq!(summary["sourceKind"], "hostWebcam");
    assert_eq!(summary["webcamName"], "front");
    assert_eq!(summary["webcamService"], "mjpegstreamer-adaptive");
    assert_eq!(summary["webPort"], rig.camera.port);
    assert_eq!(summary["hasSnapshotUrl"], false);
    assert_eq!(rig.health_events(MOON).last().unwrap()["sourceKind"], "hostWebcam");

    // hostWebcam needs a Connection; a missing Printer is NOT_FOUND.
    let bare = rig.set("op-bare", BARE, rig.webcam_source()).unwrap_err();
    assert_eq!((bare["code"].as_str(), bare["details"]["fieldPath"].as_str()), (Some("VALIDATION"), Some("source.kind")));
    assert_eq!(rig.set("op-missing", "prn-missing", source.clone()).unwrap_err()["code"], "NOT_FOUND");
    assert_eq!(rig.err("get_printer_camera", json!({"printerId": "prn-missing"}))["code"], "NOT_FOUND");
    assert_eq!(rig.err("clear_printer_camera", json!({"operationId": "op-c-missing", "printerId": "prn-missing"}))["code"], "NOT_FOUND");

    // clear: once for real, then a replay and a no-op publish nothing.
    let events = rig.health_events(MOON).len();
    let cleared = rig.ok("clear_printer_camera", json!({"operationId": "op-clear", "printerId": MOON}));
    assert_eq!(cleared, json!({"printerId": MOON, "cleared": true}));
    let health = rig.health_events(MOON);
    assert_eq!(health.len(), events + 1);
    assert_eq!(
        health.last().unwrap(),
        &json!({"printerId": MOON, "state": "notConfigured", "sourceKind": null,
                "lastSuccessAt": null, "lastFailureAt": null, "lastFailureKind": null})
    );
    assert_eq!(rig.ok("get_printer_camera", json!({"printerId": MOON})), Value::Null);
    assert_eq!(
        rig.ok("clear_printer_camera", json!({"operationId": "op-clear", "printerId": MOON})),
        json!({"printerId": MOON, "cleared": true})
    );
    assert_eq!(
        rig.ok("clear_printer_camera", json!({"operationId": "op-clear-again", "printerId": MOON})),
        json!({"printerId": MOON, "cleared": false})
    );
    assert_eq!(rig.health_events(MOON).len(), events + 1);

    // The operation digests are hashes: no URL reaches `operations`.
    let digests: String = rig
        .storage
        .read(|connection| {
            connection.query_row("SELECT group_concat(id || kind || request_digest) FROM operations", [], |row| row.get(0))
        })
        .unwrap();
    rig.assert_clean(&digests);
    rig.assert_clean(&rig.all_events());
}

#[test]
fn list_host_webcams_returns_names_and_services_only() {
    let rig = Rig::eager();
    rig.webcams(Some(&format!("http://{SEED_HOST}:8080/snap?token={SEED_WEBCAM_TOKEN}")));
    let expected = json!([{"name": "front", "service": "mjpegstreamer-adaptive"}]);
    let listed = rig.ok("list_host_webcams", json!({"printerId": MOON}));
    assert_eq!(listed, expected);
    rig.assert_clean(&listed.to_string());
    // The Setup wizard: an unsaved Connection, used and never stored.
    let listed = rig.ok("list_host_webcams", json!({"connection": rig.connection(SECRET)}));
    assert_eq!(listed, expected);
    // The list is read with the API key, like every Moonraker read.
    let reads: Vec<_> = rig
        .moonraker
        .requests()
        .into_iter()
        .filter(|request| request.path() == "/server/webcams/list")
        .collect();
    assert_eq!(reads.len(), 2);
    for (body, field) in [
        (json!({}), "printerId"),
        (json!({"printerId": MOON, "connection": rig.connection(SECRET)}), "printerId"),
        (json!({"printerId": BARE}), "printerId"),
    ] {
        let error = rig.err("list_host_webcams", body);
        assert_eq!((error["code"].as_str(), error["details"]["fieldPath"].as_str()), (Some("VALIDATION"), Some(field)));
    }
    assert_eq!(rig.err("list_host_webcams", json!({"printerId": "prn-missing"}))["code"], "NOT_FOUND");
    let wrong_key = rig.err("list_host_webcams", json!({"connection": rig.connection("not-the-key")}));
    assert_eq!(wrong_key["code"], "CAMERA_FAILED");
    assert_eq!(wrong_key["details"]["kind"], "webcamListFailed");
    rig.assert_clean(&wrong_key.to_string());
    let octoprint = rig.err("list_host_webcams", json!({"printerId": OCTO}));
    assert_eq!(octoprint["code"], "CAPABILITY_UNSUPPORTED");
    assert_eq!(octoprint["details"]["capability"], "camera");
    assert_eq!(octoprint["details"]["reason"], "adapter");
    assert_eq!(octoprint["details"]["printerId"], OCTO);
}

#[test]
fn test_camera_returns_a_frame_and_writes_nothing() {
    let rig = Rig::eager();
    rig.webcams(Some(&format!("/webcam/?action=snapshot&token={SEED_WEBCAM_TOKEN}")));
    let tables = |rig: &Rig| {
        [
            "SELECT COUNT(*) FROM camera_snapshots",
            "SELECT COUNT(*) FROM printer_cameras",
            "SELECT COUNT(*) FROM operations",
            "SELECT COUNT(*) FROM attention_events",
            "SELECT COUNT(*) FROM printers",
        ]
        .map(|sql| rig.count(sql))
    };
    let before = (tables(&rig), files_under(&rig.root));

    let (header, image) = rig
        .frame("test_camera", json!({"source": rig.snapshot_source("/snapshot.jpg")}))
        .unwrap();
    assert_eq!(image, JPEG);
    assert_eq!(header["contentType"], "image/jpeg");
    assert_eq!(header["snapshotId"], Value::Null);
    assert!(header["capturedAt"].is_string());
    rig.assert_clean(&header.to_string());

    // The Setup wizard: a host webcam through an unsaved Connection.
    rig.camera.answer(Answer::Png);
    let (header, image) = rig
        .frame("test_camera", json!({"connection": rig.connection(SECRET), "source": rig.webcam_source()}))
        .unwrap();
    assert_eq!((header["contentType"].as_str(), image.as_slice()), (Some("image/png"), PNG));
    let fetched = rig.camera.requests().last().unwrap().clone();
    assert_eq!(fetched.target, format!("/webcam/?action=snapshot&token={SEED_WEBCAM_TOKEN}"));
    assert!(fetched.header("x-api-key").is_none(), "the camera never gets the API key");

    // A host webcam needs exactly one of printerId and connection.
    for body in [
        json!({"source": rig.webcam_source()}),
        json!({"printerId": MOON, "connection": rig.connection(SECRET), "source": rig.webcam_source()}),
    ] {
        let error = rig.frame("test_camera", body).unwrap_err();
        assert_eq!(error["details"]["fieldPath"], "printerId");
    }
    // A Printer without a saved camera can test one; nothing is stored.
    rig.frame("test_camera", json!({"printerId": BARE, "source": rig.snapshot_source("/x.png")})).unwrap();
    assert_eq!(rig.frame("test_camera", json!({"printerId": "prn-missing", "source": rig.snapshot_source("/x.png")})).unwrap_err()["code"], "NOT_FOUND");

    assert_eq!((tables(&rig), files_under(&rig.root)), before, "test_camera wrote something");
    assert!(rig.health_events(BARE).is_empty() && rig.health_events(MOON).is_empty());
    assert!(rig.services.cameras.health(BARE).is_none());
}

#[test]
fn a_draft_test_never_touches_the_saved_sources_health() {
    let rig = Rig::eager();
    rig.set("op-set", MOON, rig.snapshot_source("/saved.jpg")).unwrap();
    rig.preview(MOON).unwrap();
    assert_eq!(rig.health_events(MOON).last().unwrap()["state"], "ok");
    let published = rig.health_events(MOON).len();

    rig.camera.answer(Answer::Status(404));
    let error = rig
        .frame("test_camera", json!({"printerId": MOON, "source": rig.snapshot_source("/draft.jpg")}))
        .unwrap_err();
    assert_eq!((error["code"].as_str(), error["details"]["httpStatus"].as_u64()), (Some("CAMERA_FAILED"), Some(404)));
    assert_eq!(rig.health_events(MOON).len(), published, "a draft test changed the saved health");
    assert_eq!(rig.services.cameras.health(MOON).unwrap().state, farm3d_lib::cameras::CameraHealthState::Ok);
    let listed = rig.ok("list_attention", json!({}));
    assert_eq!(listed["cameraHealth"][0]["state"], "ok");

    // Testing the saved source itself does update the health.
    rig.frame("test_camera", json!({"printerId": MOON, "source": rig.snapshot_source("/saved.jpg")}))
        .unwrap_err();
    let health = rig.health_events(MOON);
    assert_eq!(health.len(), published + 1);
    assert_eq!(health.last().unwrap()["state"], "failing");
    assert_eq!(health.last().unwrap()["lastFailureKind"], "httpStatus");
}

#[test]
fn a_preview_updates_health_and_publishes_only_on_a_change() {
    let rig = Rig::eager();
    let not_configured = rig.preview(MOON).unwrap_err();
    assert_eq!(not_configured["code"], "CAMERA_NOT_CONFIGURED");
    assert_eq!(not_configured["recovery"], json!(["OPEN_PRINTER_SETUP"]));
    assert_eq!(not_configured["details"]["printerId"], MOON);
    assert_eq!(rig.preview("prn-missing").unwrap_err()["code"], "NOT_FOUND");

    rig.set("op-set", MOON, rig.snapshot_source("/snap.jpg")).unwrap();
    let (header, image) = rig.preview(MOON).unwrap();
    assert_eq!((header["contentType"].as_str(), image.as_slice()), (Some("image/jpeg"), JPEG));
    let states = |rig: &Rig| -> Vec<(String, Value)> {
        rig.health_events(MOON)
            .into_iter()
            .map(|health| (health["state"].as_str().unwrap().to_string(), health["lastFailureKind"].clone()))
            .collect()
    };
    assert_eq!(states(&rig), [("unknown".to_string(), Value::Null), ("ok".to_string(), Value::Null)]);
    rig.preview(MOON).unwrap();
    assert_eq!(states(&rig).len(), 2, "an unchanged health publishes nothing");

    rig.camera.answer(Answer::Status(404));
    let failed = rig.preview(MOON).unwrap_err();
    assert_eq!(failed["code"], "CAMERA_FAILED");
    assert_eq!(failed["details"], json!({"printerId": MOON, "kind": "httpStatus", "httpStatus": 404}));
    assert_eq!(failed["recovery"], json!(["RETRY"]));
    assert_eq!(failed["retryable"], true);
    rig.preview(MOON).unwrap_err();
    rig.camera.answer(Answer::HtmlAsJpeg);
    assert_eq!(rig.preview(MOON).unwrap_err()["details"]["kind"], "notAnImage");
    rig.camera.answer(Answer::Jpeg);
    rig.preview(MOON).unwrap();
    assert_eq!(
        states(&rig),
        [
            ("unknown".to_string(), Value::Null),
            ("ok".to_string(), Value::Null),
            ("failing".to_string(), json!("httpStatus")),
            ("failing".to_string(), json!("notAnImage")),
            ("ok".to_string(), json!("notAnImage")),
        ]
    );
    let health = rig.health_events(MOON).pop().unwrap();
    assert!(health["lastSuccessAt"].is_string() && health["lastFailureAt"].is_string());

    let listed = rig.ok("list_attention", json!({}));
    assert_eq!(listed["cameraHealth"].as_array().unwrap().len(), 1);
    assert_eq!(listed["cameraHealth"][0]["printerId"], MOON);
    assert_eq!(listed["cameraHealth"][0]["state"], "ok");
    assert_eq!(listed["cameraHealth"][0]["sourceKind"], "snapshotUrl");
    rig.assert_clean(&rig.all_events());
}

#[test]
fn a_preview_within_the_interval_reuses_the_last_frame_until_the_source_changes() {
    let rig = Rig::new(CameraTimings {
        preview_min_interval: Duration::from_secs(3600),
        ..CameraTimings::default()
    });
    rig.set("op-set", MOON, rig.snapshot_source("/a.jpg")).unwrap();
    let (first, _) = rig.preview(MOON).unwrap();
    let (again, _) = rig.preview(MOON).unwrap();
    assert_eq!(first["capturedAt"], again["capturedAt"], "the same frame, with its own time");
    assert_eq!(rig.camera.requests().len(), 1);
    assert!(rig.services.cameras.has_preview_frame(MOON));

    rig.set("op-set-b", MOON, rig.snapshot_source("/b.png")).unwrap();
    assert!(!rig.services.cameras.has_preview_frame(MOON), "a new source drops the last frame");
    rig.camera.answer(Answer::Png);
    let (header, _) = rig.preview(MOON).unwrap();
    assert_eq!(header["contentType"], "image/png");
    assert_eq!(rig.camera.requests().len(), 2);
    assert_eq!(rig.camera.requests()[1].target, "/b.png");
}

#[test]
fn concurrent_previews_wait_for_one_fetch_and_share_it() {
    let rig = Rig::eager();
    rig.set("op-set", MOON, rig.snapshot_source("/held.jpg")).unwrap();
    rig.camera.answer(Answer::Held);
    let callers: Vec<_> = (0..5)
        .map(|_| {
            let webview = rig.webview.clone();
            std::thread::spawn(move || binary(&webview, "camera_preview_frame", json!({"printerId": MOON})))
        })
        .collect();
    rig.wait_until("all five previews are in and one fetch reached the camera", || {
        rig.services.cameras.in_flight(MOON) == 5 && !rig.camera.requests().is_empty()
    });
    assert_eq!(rig.camera.requests().len(), 1, "one fetch per Printer at a time");
    rig.camera.release();
    let frames: Vec<_> = callers.into_iter().map(|caller| caller.join().unwrap().unwrap()).collect();
    assert_eq!(rig.camera.requests().len(), 1, "the waiting previews shared the one fetch");
    assert!(frames.iter().all(|(header, image)| image == JPEG && header["capturedAt"] == frames[0].0["capturedAt"]));
    assert_eq!(rig.services.cameras.in_flight(MOON), 0);
}

#[test]
fn a_host_webcam_resolves_through_the_connection_and_fails_typed() {
    let rig = Rig::eager();
    rig.webcams(Some(&format!("/webcam/?action=snapshot&token={SEED_WEBCAM_TOKEN}")));
    rig.set("op-set", MOON, rig.webcam_source()).unwrap();
    let (_, image) = rig.preview(MOON).unwrap();
    assert_eq!(image, JPEG);
    let fetched = rig.camera.requests().pop().unwrap();
    assert_eq!(fetched.target, format!("/webcam/?action=snapshot&token={SEED_WEBCAM_TOKEN}"));
    assert!(fetched.header("x-api-key").is_none());

    let failure = |rig: &Rig| {
        let error = rig.preview(MOON).unwrap_err();
        rig.assert_clean(&error.to_string());
        error
    };
    rig.webcams(Some(&format!("http://{SEED_HOST}:8080/snap?token={SEED_WEBCAM_TOKEN}")));
    let mismatch = failure(&rig);
    assert_eq!(mismatch["code"], "CAMERA_HOST_MISMATCH");
    assert_eq!(mismatch["details"], json!({"printerId": MOON}));
    assert_eq!(mismatch["recovery"], json!(["OPEN_PRINTER_SETUP"]));
    assert_eq!(rig.health_events(MOON).last().unwrap()["lastFailureKind"], "hostMismatch");

    rig.moonraker.with_state(|state| state.webcams = Vec::new());
    let missing = failure(&rig);
    assert_eq!(missing["details"]["kind"], "noSuchWebcam");
    assert_eq!(missing["recovery"], json!(["RETRY", "OPEN_PRINTER_SETUP"]));
    rig.webcams(None);
    assert_eq!(failure(&rig)["details"]["kind"], "noSnapshotUrl");
    rig.webcams(Some(""));
    assert_eq!(failure(&rig)["details"]["kind"], "noSnapshotUrl");
    rig.moonraker.set_reachable(false);
    assert_eq!(failure(&rig)["details"]["kind"], "webcamListFailed");
    rig.moonraker.set_reachable(true);
    rig.webcams(Some("/webcam/?action=snapshot"));
    rig.camera.answer(Answer::Status(503));
    let unavailable = failure(&rig);
    assert_eq!(unavailable["details"], json!({"printerId": MOON, "kind": "httpStatus", "httpStatus": 503}));
    rig.assert_clean(&rig.all_events());
}

#[test]
fn octoprint_has_no_host_webcams_but_a_manual_url_works() {
    let rig = Rig::eager();
    let refused = rig.set("op-octo-webcam", OCTO, rig.webcam_source()).unwrap_err();
    assert_eq!(refused["code"], "CAPABILITY_UNSUPPORTED");
    assert_eq!(
        refused["details"],
        json!({"printerId": OCTO, "capability": "camera", "reason": "adapter", "detail": refused["message"]})
    );
    let tested = rig.frame("test_camera", json!({"printerId": OCTO, "source": rig.webcam_source()})).unwrap_err();
    assert_eq!(tested["code"], "CAPABILITY_UNSUPPORTED");
    let draft = rig
        .frame(
            "test_camera",
            json!({"connection": {"kind": "octoprint", "host": "127.0.0.1", "port": 9}, "source": rig.webcam_source()}),
        )
        .unwrap_err();
    assert_eq!((draft["code"].as_str(), &draft["details"]["printerId"]), (Some("CAPABILITY_UNSUPPORTED"), &Value::Null));

    rig.set("op-octo-url", OCTO, rig.snapshot_source("/octo.jpg")).unwrap();
    assert_eq!(rig.preview(OCTO).unwrap().1, JPEG);

    // A saved host webcam on an adapter without the lookup (its Connection
    // changed kind after the camera was saved) is `unsupported`.
    rig.storage
        .write(|tx| {
            tx.execute(
                "UPDATE printer_cameras SET source_kind = 'hostWebcam', snapshot_url = NULL, \
                 webcam_name = 'front' WHERE printer_id = ?1",
                [OCTO],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(rig.preview(OCTO).unwrap_err()["code"], "CAPABILITY_UNSUPPORTED");
    assert_eq!(rig.health_events(OCTO).last().unwrap()["state"], "unsupported");
}

#[test]
fn deleting_a_printer_drops_its_camera_health_and_last_frame() {
    let rig = Rig::new(CameraTimings::default());
    rig.set("op-set", BARE, rig.snapshot_source("/bare.jpg")).unwrap();
    rig.preview(BARE).unwrap();
    assert!(rig.services.cameras.health(BARE).is_some());
    assert!(rig.services.cameras.has_preview_frame(BARE));
    rig.ok(
        "archive_printer",
        json!({"id": BARE, "expectedRevision": 1, "operationId": "op-archive", "spoolDispositions": []}),
    );
    rig.ok("delete_printer", json!({"id": BARE, "expectedRevision": 2}));
    assert!(rig.services.cameras.health(BARE).is_none());
    assert!(!rig.services.cameras.has_preview_frame(BARE));
    assert_eq!(rig.count("SELECT COUNT(*) FROM printer_cameras"), 0);
    assert_eq!(rig.ok("list_attention", json!({}))["cameraHealth"], json!([]));
}

/// Global constraint 3, fed through every input that builds a payload:
/// a userinfo attempt, a manual URL with a query token, and host-webcam
/// lists naming an RFC 5737 host and a token. Scans every error, every
/// emitted event, every command response but `get_printer_camera`'s, and
/// every persisted row but `printer_cameras.snapshot_url`.
#[test]
fn the_seeded_corpus_never_leaves_the_camera_module() {
    let rig = Rig::eager();
    let mut responses = Vec::new();
    let mut record = |result: Result<Value, Value>| {
        let text = match result {
            Ok(value) | Err(value) => value.to_string(),
        };
        responses.push(text);
    };
    let userinfo = json!({"kind": "snapshotUrl", "snapshotUrl": format!("http://user:{SEED_PASS}@{SEED_HOST}:8080/snap?token={SEED_QUERY_TOKEN}")});
    let token = json!({"kind": "snapshotUrl", "snapshotUrl": rig.camera.url(&format!("/snapshot?token={SEED_QUERY_TOKEN}"))});
    record(rig.set("op-userinfo", MOON, userinfo.clone()));
    record(rig.frame("test_camera", json!({"source": userinfo})).map(|(header, _)| header));
    record(rig.set("op-token", MOON, token.clone()));
    record(rig.preview(MOON).map(|(header, _)| header));
    rig.camera.answer(Answer::Status(404));
    record(rig.preview(MOON).map(|(header, _)| header));
    record(rig.frame("test_camera", json!({"printerId": MOON, "source": token})).map(|(header, _)| header));
    // The single exception, checked and deliberately left out of the scan.
    assert!(rig.ok("get_printer_camera", json!({"printerId": MOON})).to_string().contains(SEED_QUERY_TOKEN));

    rig.webcams(Some(&format!("http://{SEED_HOST}:8080/snap?token={SEED_WEBCAM_TOKEN}")));
    record(rig.call("list_host_webcams", json!({"printerId": MOON})));
    record(rig.set("op-webcam", MOON, rig.webcam_source()));
    record(rig.preview(MOON).map(|(header, _)| header));
    record(rig.frame("test_camera", json!({"connection": rig.connection(SECRET), "source": rig.webcam_source()})).map(|(header, _)| header));
    rig.webcams(Some(&format!("/webcam/snap?token={SEED_WEBCAM_TOKEN}")));
    record(rig.preview(MOON).map(|(header, _)| header));
    rig.camera.answer(Answer::Jpeg);
    record(rig.preview(MOON).map(|(header, _)| header));
    record(rig.call("clear_printer_camera", json!({"operationId": "op-clear", "printerId": MOON})));
    record(rig.call("list_attention", json!({})));
    record(rig.set("op-token-again", MOON, json!({"kind": "snapshotUrl", "snapshotUrl": rig.camera.url(&format!("/snapshot?token={SEED_QUERY_TOKEN}"))})));
    record(rig.call("list_attention", json!({})));

    assert!(responses.len() >= 14);
    for response in &responses {
        rig.assert_clean(response);
    }
    let events = rig.all_events();
    assert!(events.contains("camera.health.changed"));
    rig.assert_clean(&events);

    // Every persisted row except the manual URL's own column.
    let rows = rig
        .storage
        .read(|connection| {
            let tables: Vec<String> = connection
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut text = String::new();
            for table in tables {
                let columns: Vec<String> = connection
                    .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                for column in columns {
                    if table == "printer_cameras" && column == "snapshot_url" {
                        continue;
                    }
                    let mut statement =
                        connection.prepare(&format!("SELECT CAST(\"{column}\" AS TEXT) FROM \"{table}\""))?;
                    for value in statement.query_map([], |row| row.get::<_, Option<String>>(0))? {
                        text.push_str(&value?.unwrap_or_default());
                        text.push('\n');
                    }
                }
            }
            Ok(text)
        })
        .unwrap();
    assert!(rows.contains(MOON), "the scan read the rows");
    for needle in [
        SEED_PASS.to_string(),
        SEED_QUERY_TOKEN.to_string(),
        SEED_WEBCAM_TOKEN.to_string(),
        SEED_HOST.to_string(),
        format!("127.0.0.1:{}", rig.camera.port),
        format!(":{}/", rig.camera.port),
        SECRET.to_string(),
    ] {
        assert!(!rows.contains(&needle), "{needle:?} was persisted");
    }
    let stored: String = rig
        .storage
        .read(|connection| connection.query_row("SELECT snapshot_url FROM printer_cameras", [], |row| row.get(0)))
        .unwrap();
    assert!(stored.contains(SEED_QUERY_TOKEN), "the manual URL lives in its own column");
}

// --- Task 8 carries: saved-source host webcams without a Connection, and the
// host comparison -------------------------------------------------------------------

/// A saved host webcam whose Printer later lost its Connection: a preview
/// (a saved-source fetch) fails `webcamListFailed` and marks the health
/// failing. Saving a new host webcam there, or testing a draft one, stays
/// `VALIDATION` on `source.kind`.
#[test]
fn a_saved_host_webcam_whose_printer_lost_its_connection_fails_webcam_list_failed() {
    let rig = Rig::eager();
    rig.webcams(Some("/webcam/?action=snapshot"));
    rig.set("op-set", MOON, rig.webcam_source()).unwrap();
    rig.storage
        .write(|tx| {
            tx.execute("UPDATE printers SET connection_json = NULL WHERE id = ?1", [MOON])?;
            Ok(())
        })
        .unwrap();
    let error = rig.preview(MOON).unwrap_err();
    assert_eq!((error["code"].as_str(), error["details"]["kind"].as_str()), (Some("CAMERA_FAILED"), Some("webcamListFailed")));
    let health = rig.services.cameras.health(MOON).unwrap();
    assert_eq!(health.state, farm3d_lib::cameras::CameraHealthState::Failing);
    assert_eq!(health.last_failure_kind, Some(CameraErrorKind::WebcamListFailed));
    assert_eq!(rig.health_events(MOON).last().unwrap()["lastFailureKind"], "webcamListFailed");

    // A test of the saved source is a saved-source fetch too.
    let tested = rig.frame("test_camera", json!({"printerId": MOON, "source": rig.webcam_source()})).unwrap_err();
    assert_eq!(tested["details"]["kind"], "webcamListFailed");
    // A draft host webcam on the Printer, and a new save, stay VALIDATION.
    let draft = json!({"kind": "hostWebcam", "webcamName": "other", "webcamService": null, "webPort": null});
    let tested = rig.frame("test_camera", json!({"printerId": MOON, "source": draft})).unwrap_err();
    assert_eq!((tested["code"].as_str(), tested["details"]["fieldPath"].as_str()), (Some("VALIDATION"), Some("source.kind")));
    let saved = rig.set("op-set-again", MOON, draft).unwrap_err();
    assert_eq!((saved["code"].as_str(), saved["details"]["fieldPath"].as_str()), (Some("VALIDATION"), Some("source.kind")));
}

/// The resolved URL's host is compared with the parsed base URL's host, so
/// an uncompressed or upper-case IPv6 Connection host, or an odd IPv4
/// spelling, is still the same host.
#[test]
fn the_host_comparison_uses_the_parsed_connection_host() {
    assert_eq!(
        resolved("2001:DB8:0:0:0:0:0:1", Some(8080), "/snap").unwrap(),
        "http://[2001:db8::1]:8080/snap",
        "an uncompressed upper-case IPv6 host and a relative URL"
    );
    assert_eq!(
        resolved("[2001:DB8::1]", None, "http://[2001:db8::1]/snap").unwrap(),
        "http://[2001:db8::1]/snap",
        "a bracketed Connection host and an absolute URL on it"
    );
    assert_eq!(
        resolved("0xC0.0.2.10", None, "http://192.0.2.10/snap").unwrap(),
        "http://192.0.2.10/snap",
        "a hex IPv4 spelling"
    );
    assert_eq!(
        resolved("2001:DB8:0:0:0:0:0:1", None, "http://[2001:db8::2]/snap").unwrap_err(),
        CameraErrorKind::HostMismatch,
        "another IPv6 host still mismatches"
    );
}

// --- Task 8: a camera can never block the four core flows (global constraint 5) --------

use farm3d_lib::jobs::JobTimings;
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::StartSafety;

/// Well past every core flow's normal bound, and far below the capture's
/// own budget: a flow that waited for the hanging camera would blow it.
const FLOW_BOUND: Duration = Duration::from_secs(5);
/// `printer_statuses` reads memory only.
const STATUS_BOUND: Duration = Duration::from_secs(1);

/// A capture budget longer than any test: the capture hangs for as long
/// as the camera holds its answer (released at each test's end).
fn hanging_timings() -> CameraTimings {
    CameraTimings {
        fetch: Duration::from_secs(60),
        ..CameraTimings::default()
    }
}

fn store_camera(storage: &Storage, printer_id: &str, camera: &FakeCamera) {
    storage
        .write(|tx| {
            tx.execute(
                "INSERT INTO printer_cameras(printer_id, source_kind, snapshot_url, updated_at) \
                 VALUES (?1, 'snapshotUrl', ?2, '2026-09-28T09:00:00.000Z')",
                rusqlite::params![printer_id, camera.url("/hang.jpg")],
            )?;
            Ok(())
        })
        .unwrap();
}

/// Starts a `capture_snapshot` of `printer_id` on its own thread and waits
/// until the camera has the request (which it holds): a capture is hanging.
fn hang_a_capture(
    webview: &tauri::WebviewWindow<MockRuntime>,
    printer_id: &str,
    camera: &FakeCamera,
) -> std::thread::JoinHandle<Result<Value, Value>> {
    let webview = webview.clone();
    let body = json!({"contractVersion": 1, "operationId": "op-hang", "printerId": printer_id});
    let capture = std::thread::spawn(move || common::invoke(&webview, "capture_snapshot", body));
    let deadline = Instant::now() + Duration::from_secs(15);
    while camera.requests().is_empty() {
        assert!(Instant::now() < deadline, "the capture never reached the camera");
        std::thread::sleep(Duration::from_millis(5));
    }
    capture
}

/// The dispatch rig with the capture runtime on and a camera that holds
/// every answer.
fn hanging_dispatch_rig() -> (p7_dispatch_rig::Roots, p7_dispatch_rig::Running, FakeCamera) {
    let roots = p7_dispatch_rig::Roots::new(StartSafety::ConfirmBedClear);
    let (app, _) = p7_dispatch_rig::boot_with_attention(
        &roots,
        p7_dispatch_rig::status_of(OperationalState::Ready),
        JobTimings::default(),
        p7_dispatch_rig::AttentionBoot {
            timings: farm3d_lib::attention::services::AttentionTimings {
                pass_min_interval: Duration::ZERO,
                safety_tick: Duration::from_secs(3600),
            },
            clock: None,
            cameras: Some(hanging_timings()),
            notifications: None,
        },
    );
    let camera = FakeCamera::start(Answer::Held);
    store_camera(&app.storage, p7_dispatch_rig::PRINTER, &camera);
    (roots, app, camera)
}

fn timed<T>(flow: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let result = flow();
    (result, started.elapsed())
}

/// Releases the camera and checks the capture it held then finished.
fn finish(capture: std::thread::JoinHandle<Result<Value, Value>>, camera: &FakeCamera) {
    camera.release();
    let snapshot = capture.join().unwrap().expect("the held capture finishes once released");
    assert_eq!(snapshot["data"]["trigger"], "manual");
}

#[test]
fn printer_statuses_and_the_projector_never_wait_for_a_hanging_camera() {
    let (_roots, app, camera) = hanging_dispatch_rig();
    let capture = hang_a_capture(&app.webview, p7_dispatch_rig::PRINTER, &camera);

    let (statuses, took) = timed(|| app.ok("printer_statuses", json!({})));
    assert!(took < STATUS_BOUND, "printer_statuses took {took:?}");
    assert!(statuses.to_string().contains(p7_dispatch_rig::PRINTER), "{statuses}");
    // Monitoring goes on: a whole projector pass completes.
    let (_, took) = timed(|| app.attention_pass());
    assert!(took < FLOW_BOUND, "an attention pass took {took:?}");
    assert_eq!(app.services.cameras.in_flight(p7_dispatch_rig::PRINTER), 1, "the capture still hangs");
    finish(capture, &camera);
}

#[test]
fn assign_queue_entry_succeeds_while_a_camera_hangs() {
    let (_roots, app, camera) = hanging_dispatch_rig();
    let spool = app.spool();
    app.load(&spool);
    let capture = hang_a_capture(&app.webview, p7_dispatch_rig::PRINTER, &camera);

    let (job, took) = timed(|| app.assign(&spool));
    assert!(took < FLOW_BOUND, "assign_queue_entry took {took:?}");
    assert!(job.starts_with("job-"), "{job}");
    app.wait_job(&job, "awaitingStart");
    assert_eq!(app.services.cameras.in_flight(p7_dispatch_rig::PRINTER), 1, "the capture still hangs");
    finish(capture, &camera);
}

#[test]
fn start_job_succeeds_while_a_camera_hangs() {
    let (_roots, app, camera) = hanging_dispatch_rig();
    let job = app.awaiting_start();
    let capture = hang_a_capture(&app.webview, p7_dispatch_rig::PRINTER, &camera);

    let (started, took) = timed(|| app.start("op-start", &job, "ready"));
    assert!(took < FLOW_BOUND, "start_job took {took:?}");
    started.expect("start_job");
    app.wait_job(&job, "printing");
    assert_eq!(app.services.cameras.in_flight(p7_dispatch_rig::PRINTER), 1, "the capture still hangs");
    finish(capture, &camera);
}

#[test]
fn a_slice_starts_while_a_camera_hangs() {
    let farm = p5_harness::Farm::new();
    let running = p5_harness::Running::boot_with_cameras(
        &farm.paths,
        &farm.lease,
        farm.probe_timeout,
        Some(hanging_timings()),
    );
    PrinterRepository::new(Arc::clone(&running.services.storage))
        .create(common::a_stored_printer("prn-slice-cam"))
        .unwrap();
    let camera = FakeCamera::start(Answer::Held);
    store_camera(&running.services.storage, "prn-slice-cam", &camera);
    let model = running.import(&farm.source("orca-two-plates.3mf", "two.3mf"), "managed");
    let preparation = running.prepare(model["id"].as_str().unwrap());
    let plates = p5_harness::plates(&preparation);
    let capture = hang_a_capture(&running.webview, "prn-slice-cam", &camera);

    let (operations, took) = timed(|| running.start("op-slice", &preparation, &[&plates[0]]));
    assert!(took < FLOW_BOUND, "start_slice took {took:?}");
    let id = p5_harness::ids(&operations).remove(0);
    running.wait_state(&id, "succeeded");
    assert_eq!(running.services.cameras.in_flight("prn-slice-cam"), 1, "the capture still hangs");
    finish(capture, &camera);
}
