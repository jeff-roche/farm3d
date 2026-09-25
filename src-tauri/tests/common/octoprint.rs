//! A canned OctoPrint REST server for integration tests: a plain HTTP/1.1
//! responder on a loopback port, answering the five endpoints the
//! status-only adapter reads with the shapes from OctoPrint's REST API docs.
//!
//! It is a stand-in for wiring tests, not evidence that farm3d works against
//! a real OctoPrint — that is what the ignored `a0_octoprint_live` test and
//! the live-validation record are for.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use serde_json::json;

/// What the stub reports on its next answer. Tests mutate it to simulate
/// a printer being disconnected from OctoPrint.
#[derive(Clone, Debug)]
pub struct StubState {
    pub job_state: String,
    /// `progress.completion`, 0-100 as OctoPrint reports it.
    pub completion: Option<f64>,
    /// `false` makes `GET /api/printer` answer 409, as OctoPrint does when
    /// no printer is connected.
    pub printer_attached: bool,
}

#[derive(Clone, Debug)]
pub struct SeenRequest {
    pub path: String,
    pub headers: HashMap<String, String>,
}

pub struct OctoPrintStub {
    pub port: u16,
    pub state: Arc<Mutex<StubState>>,
    pub seen: Arc<Mutex<Vec<SeenRequest>>>,
}

impl OctoPrintStub {
    /// `api_key`: when `Some`, every request without exactly this
    /// `X-Api-Key` answers 403, as an access-controlled OctoPrint does.
    pub fn start(api_key: Option<&str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind OctoPrint stub");
        let port = listener.local_addr().expect("stub address").port();
        let state = Arc::new(Mutex::new(StubState {
            job_state: "Printing".to_string(),
            completion: Some(42.5),
            printer_attached: true,
        }));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let required_key = api_key.map(str::to_string);
        {
            let state = Arc::clone(&state);
            let seen = Arc::clone(&seen);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { return };
                    let Ok(clone) = stream.try_clone() else { continue };
                    let mut reader = BufReader::new(clone);
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).is_err() {
                        continue;
                    }
                    let path = request_line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    let mut headers = HashMap::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':') {
                            headers
                                .insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                        }
                    }
                    let authorized = match &required_key {
                        None => true,
                        Some(key) => headers.get("x-api-key") == Some(key),
                    };
                    seen.lock().unwrap().push(SeenRequest {
                        path: path.clone(),
                        headers,
                    });
                    let snapshot = state.lock().unwrap().clone();
                    let (status, body) = if authorized {
                        respond(&path, &snapshot)
                    } else {
                        (403, String::new())
                    };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }
            });
        }
        Self { port, state, seen }
    }

    pub fn requests(&self) -> Vec<SeenRequest> {
        self.seen.lock().unwrap().clone()
    }
}

fn respond(path: &str, state: &StubState) -> (u16, String) {
    let body = match path {
        "/api/version" => json!({"api": "0.1", "server": "1.10.3", "text": "OctoPrint 1.10.3"}),
        "/api/connection" => json!({
            "current": {
                "state": if state.printer_attached { "Operational" } else { "Closed" },
                "port": "VIRTUAL", "baudrate": 115200, "printerProfile": "_default"
            },
            "options": {"ports": ["VIRTUAL"], "baudrates": [115200]}
        }),
        "/api/printerprofiles" => json!({"profiles": {"_default": {
            "id": "_default", "name": "Stub Printer",
            "volume": {"formFactor": "rectangular", "origin": "lowerleft",
                       "width": 256.0, "depth": 256.0, "height": 256.0}
        }}}),
        "/api/job" => json!({
            "job": {"file": {"name": state.completion.map(|_| "benchy.gcode")}},
            "progress": {"completion": state.completion, "printTime": state.completion.map(|_| 120)},
            "state": if state.printer_attached { state.job_state.as_str() } else { "Offline" }
        }),
        "/api/printer?exclude=sd,state" if !state.printer_attached => {
            return (409, "Printer is not operational".to_string())
        }
        "/api/printer?exclude=sd,state" => json!({"temperature": {
            "tool0": {"actual": 214.5, "target": 215.0, "offset": 0},
            "bed": {"actual": 59.8, "target": 60.0, "offset": 0}
        }}),
        _ => return (404, String::new()),
    };
    (200, body.to_string())
}
