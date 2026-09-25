//! The ElegooLink fake SDCP server against the behavior #8 recorded from a
//! real Centauri Carbon (ADR-0012, `tests/sim/elegoolink.rs`).
//!
//! The fake runs in-process, so these tests are hermetic and are NOT
//! `#[ignore]`d. They need #8's redacted captures, which live at
//! `docs/superpowers/baselines/evidence/a0-3-elegoolink/` once #8 merges
//! (or wherever `FARM3D_SIM_ELEGOOLINK_CAPTURES` points). Without them each
//! test prints PENDING and returns.
//!
//! These tests check the fake itself: that it reproduces the recorded idle
//! behavior and stays inside what the captures cover. Adapter tests will
//! drive the production ElegooLink adapter against it once that exists.
//! Command behavior (upload, start, pause, resume, cancel, camera) is
//! pending #8's passive command captures (runbook S7, S7b, S10p).

mod sim;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sim::elegoolink::{FakeSdcp, Pending};
use tokio_tungstenite::tungstenite::Message;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn pending(pending: Pending) {
    eprintln!("PENDING: {}", pending.0);
}

async fn connect(fake: &FakeSdcp) -> Socket {
    let (socket, _) = tokio_tungstenite::connect_async(fake.ws_url())
        .await
        .expect("connect to the fake");
    socket
}

/// A request in the shape the captures show a client sending.
fn request(cmd: i64, data: Value, request_id: &str) -> String {
    json!({
        "Id": "farm3d-test",
        "Data": {
            "Cmd": cmd,
            "Data": data,
            "RequestID": request_id,
            "MainboardID": "REDACTED-MAINBOARDID-1",
            "TimeStamp": 0,
            "From": 0,
        },
        "Topic": "sdcp/request/REDACTED-MAINBOARDID-1",
    })
    .to_string()
}

/// Reads frames until `quiet` passes with none arriving.
async fn drain(socket: &mut Socket, quiet: Duration) -> Vec<Value> {
    let mut frames = Vec::new();
    while let Ok(Some(Ok(message))) = tokio::time::timeout(quiet, socket.next()).await {
        if let Message::Text(text) = message {
            frames.push(serde_json::from_str(&text).unwrap_or(Value::Null));
        }
    }
    frames
}

#[tokio::test(flavor = "multi_thread")]
async fn the_fake_answers_cmd_1_and_cmd_0_the_way_the_printer_did() {
    let fake = match FakeSdcp::start_with_idle(None).await {
        Ok(fake) => fake,
        Err(p) => return pending(p),
    };
    let mut socket = connect(&fake).await;
    socket
        .send(Message::Text(
            request(1, json!({}), "req-attributes").into(),
        ))
        .await
        .unwrap();
    socket
        .send(Message::Text(request(0, json!({}), "req-status").into()))
        .await
        .unwrap();
    let frames = drain(&mut socket, Duration::from_millis(500)).await;

    let ack_1 = frames
        .iter()
        .find(|f| f["Data"]["Cmd"] == 1)
        .expect("an ack for Cmd 1");
    assert_eq!(ack_1["Data"]["RequestID"], "req-attributes");
    assert_eq!(ack_1["Data"]["Data"]["Ack"], 0);
    assert!(
        frames.iter().any(|f| f.get("Attributes").is_some()),
        "{frames:?}"
    );

    let ack_0 = frames
        .iter()
        .find(|f| f["Data"]["Cmd"] == 0)
        .expect("an ack for Cmd 0");
    assert_eq!(ack_0["Data"]["RequestID"], "req-status");
    let status = frames
        .iter()
        .find(|f| f.get("Status").is_some())
        .expect("a Status frame after Cmd 0");
    // A recorded quirk: some PrintInfo keys arrive hex-encoded, with a NUL.
    let print_info = status["Status"]["PrintInfo"]
        .as_object()
        .expect("PrintInfo");
    assert!(
        print_info
            .keys()
            .any(|key| key == "54 6F 74 61 6C 45 78 74 72 75 73 69 6F 6E 00"),
        "expected the recorded hex-encoded TotalExtrusion key: {:?}",
        print_info.keys().collect::<Vec<_>>()
    );
    assert!(fake.unmodelled().is_empty(), "{:?}", fake.unmodelled());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_fake_never_pushes_status_and_never_answers_ping() {
    let fake = match FakeSdcp::start_with_idle(None).await {
        Ok(fake) => fake,
        Err(p) => return pending(p),
    };
    assert!(
        fake.model.ping_observed,
        "the captures must show a ping for this to be tested"
    );
    assert!(
        fake.model.ping_replies.is_empty(),
        "captures show a pong: {:?}",
        fake.model.ping_replies
    );

    let mut socket = connect(&fake).await;
    socket
        .send(Message::Text(request(0, json!({}), "req-status").into()))
        .await
        .unwrap();
    let _ = drain(&mut socket, Duration::from_millis(500)).await;
    socket.send(Message::Text("ping".into())).await.unwrap();
    let after_ping = drain(&mut socket, Duration::from_millis(1_500)).await;
    assert!(
        after_ping.is_empty(),
        "the fake sent frames unprompted: {after_ping:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_fake_drops_a_silent_client_without_a_close_frame() {
    // The recorded silence before the drop is about a minute; check the
    // measured value, then exercise the behavior with a short one.
    let recorded = match FakeSdcp::start().await {
        Ok(fake) => fake.model.idle_drop_after,
        Err(p) => return pending(p),
    };
    let recorded = recorded.expect("the captures should include a silent drop (1006)");
    assert!(
        (Duration::from_secs(55)..=Duration::from_secs(70)).contains(&recorded),
        "recorded idle drop {recorded:?}"
    );

    let fake = FakeSdcp::start_with_idle(Some(Duration::from_millis(800)))
        .await
        .unwrap();
    let mut socket = connect(&fake).await;
    socket
        .send(Message::Text(request(0, json!({}), "req-status").into()))
        .await
        .unwrap();
    let mut saw_close_frame = false;
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(message) = socket.next().await {
            match message {
                Ok(Message::Close(_)) => saw_close_frame = true,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the fake did not drop a silent client");
    assert!(
        !saw_close_frame,
        "the recorded drop has no close frame (1006)"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn client_traffic_keeps_the_connection_open() {
    let fake = match FakeSdcp::start_with_idle(Some(Duration::from_millis(800))).await {
        Ok(fake) => fake,
        Err(p) => return pending(p),
    };
    let mut socket = connect(&fake).await;
    // Recorded: a client sending `ping` every 25 s stayed connected for
    // 300 s. Scaled to the shortened idle time.
    for _ in 0..6 {
        socket
            .send(Message::Text("ping".into()))
            .await
            .expect("still connected");
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    socket
        .send(Message::Text(request(0, json!({}), "req-late").into()))
        .await
        .unwrap();
    let frames = drain(&mut socket, Duration::from_millis(500)).await;
    assert!(
        frames.iter().any(|f| f["Data"]["RequestID"] == "req-late"),
        "{frames:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unrecorded_commands_get_no_answer_and_are_reported() {
    let fake = match FakeSdcp::start_with_idle(None).await {
        Ok(fake) => fake,
        Err(p) => return pending(p),
    };
    let mut socket = connect(&fake).await;
    // Cmd 129 appears in no capture yet (command captures are pending #8).
    socket
        .send(Message::Text(
            request(129, json!({}), "req-unrecorded").into(),
        ))
        .await
        .unwrap();
    let frames = drain(&mut socket, Duration::from_millis(800)).await;
    assert!(frames.is_empty(), "the fake invented a reply: {frames:?}");
    assert_eq!(fake.unmodelled().len(), 1);
    eprintln!("modelled commands: {:?}", fake.model.modelled_commands());
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_gets_the_recorded_reply() {
    let fake = match FakeSdcp::start_with_idle(None).await {
        Ok(fake) => fake,
        Err(p) => return pending(p),
    };
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket
        .send_to(b"M99999", fake.discovery_addr)
        .await
        .unwrap();
    let mut buffer = [0u8; 2048];
    let (length, _) = tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buffer))
        .await
        .expect("a discovery reply")
        .unwrap();
    let reply: Value = serde_json::from_slice(&buffer[..length]).unwrap();
    assert_eq!(reply["Data"]["MachineName"], "Centauri Carbon");
}
