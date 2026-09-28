//! `FakeCamera`: an in-process HTTP snapshot camera on a loopback port, for
//! P8's `FrameFetcher`, `CameraServices`, and camera commands. Its answer
//! is switchable ([`Answer`]): a JPEG, a PNG, a status, a slow JPEG, an
//! oversize body (streamed or declared), HTML sent as `image/jpeg`, a
//! redirect, or a JPEG held until the test releases it. It records every
//! request (path, query, and headers), and how many body bytes an
//! oversize answer managed to write before the client hung up.
//!
//! Loopback only, with synthetic bytes: never a real camera or a captured
//! frame (global constraint 2).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// A synthetic JPEG: SOI, a JFIF APP0 marker, a few bytes, EOI.
pub const JPEG: &[u8] = &[
    0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00, 0x01,
    0x00, 0x01, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44, 0xFF, 0xD9,
];
/// A synthetic PNG: the signature and an IHDR-looking tail.
pub const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D', b'R',
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02,
];
/// What an oversize answer streams in total (well past the 10 MiB cap and
/// the loopback socket buffers).
pub const HUGE_BYTES: usize = 64 * 1024 * 1024;

/// How the camera answers the next requests.
#[derive(Clone, Debug)]
pub enum Answer {
    Jpeg,
    Png,
    /// This status with a short text body.
    Status(u16),
    /// A JPEG after this delay.
    Slow(Duration),
    /// A JPEG header followed by [`HUGE_BYTES`] with no `Content-Length`
    /// (the body ends when the connection closes).
    HugeStreamed,
    /// A small body that declares a `Content-Length` over the cap.
    HugeDeclared,
    /// HTML with `Content-Type: image/jpeg`.
    HtmlAsJpeg,
    /// A multipart MJPEG stream's opening boundary.
    Mjpeg,
    /// `302 Found` to another path on this camera.
    Redirect,
    /// A JPEG once [`FakeCamera::release`] is called.
    Held,
}

#[derive(Clone, Debug)]
pub struct CameraRequest {
    pub method: String,
    /// The request target: path and query.
    pub target: String,
    /// Lower-cased header names and their values.
    pub headers: Vec<(String, String)>,
}

impl CameraRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

struct Shared {
    answer: Mutex<Answer>,
    requests: Mutex<Vec<CameraRequest>>,
    held: Mutex<bool>,
    released: Condvar,
    /// Body bytes the last oversize answer wrote before its write failed.
    huge_written: AtomicUsize,
    huge_done: AtomicBool,
}

pub struct FakeCamera {
    pub port: u16,
    shared: Arc<Shared>,
}

impl FakeCamera {
    pub fn start(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind FakeCamera");
        let port = listener.local_addr().expect("camera address").port();
        let shared = Arc::new(Shared {
            answer: Mutex::new(answer),
            requests: Mutex::new(Vec::new()),
            held: Mutex::new(true),
            released: Condvar::new(),
            huge_written: AtomicUsize::new(0),
            huge_done: AtomicBool::new(false),
        });
        let accept = Arc::clone(&shared);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = Arc::clone(&accept);
                std::thread::spawn(move || serve(stream, &shared));
            }
        });
        Self { port, shared }
    }

    /// `http://127.0.0.1:<port><path>`.
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn answer(&self, answer: Answer) {
        *self.shared.answer.lock().unwrap() = answer;
    }

    pub fn requests(&self) -> Vec<CameraRequest> {
        self.shared.requests.lock().unwrap().clone()
    }

    /// Makes [`Answer::Held`] requests wait again (the camera starts held).
    pub fn hold(&self) {
        *self.shared.held.lock().unwrap() = true;
    }

    /// Lets every [`Answer::Held`] request answer.
    pub fn release(&self) {
        *self.shared.held.lock().unwrap() = false;
        self.shared.released.notify_all();
    }

    /// `Some(bytes)` once an oversize streamed answer has ended: how much
    /// of [`HUGE_BYTES`] it wrote before the client closed the connection.
    pub fn huge_written(&self) -> Option<usize> {
        self.shared
            .huge_done
            .load(Ordering::SeqCst)
            .then(|| self.shared.huge_written.load(Ordering::SeqCst))
    }
}

fn serve(stream: TcpStream, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    shared.requests.lock().unwrap().push(CameraRequest {
        method,
        target,
        headers,
    });
    let answer = shared.answer.lock().unwrap().clone();
    let mut stream = stream;
    match answer {
        Answer::Jpeg => respond(&mut stream, 200, "image/jpeg", JPEG, &[]),
        Answer::Png => respond(&mut stream, 200, "image/png", PNG, &[]),
        Answer::Status(status) => respond(&mut stream, status, "text/plain", b"no frame", &[]),
        Answer::Slow(delay) => {
            std::thread::sleep(delay);
            respond(&mut stream, 200, "image/jpeg", JPEG, &[]);
        }
        Answer::HtmlAsJpeg => respond(
            &mut stream,
            200,
            "image/jpeg",
            b"<!doctype html><html><body>login</body></html>",
            &[],
        ),
        Answer::Mjpeg => respond(
            &mut stream,
            200,
            "image/jpeg",
            b"--boundarydonotcross\r\nContent-Type: image/jpeg\r\n\r\n",
            &[],
        ),
        Answer::Redirect => respond(
            &mut stream,
            302,
            "text/plain",
            b"moved",
            &[("Location", "/moved/snapshot.jpg")],
        ),
        Answer::HugeDeclared => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                11 * 1024 * 1024
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(JPEG);
        }
        Answer::HugeStreamed => {
            shared.huge_done.store(false, Ordering::SeqCst);
            let head = "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes());
            let mut chunk = vec![0u8; 64 * 1024];
            chunk[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
            let mut written = 0;
            while written < HUGE_BYTES {
                if stream.write_all(&chunk).is_err() {
                    break;
                }
                written += chunk.len();
                chunk[..3].copy_from_slice(&[0, 0, 0]);
            }
            shared.huge_written.store(written, Ordering::SeqCst);
            shared.huge_done.store(true, Ordering::SeqCst);
        }
        Answer::Held => {
            let mut held = shared.held.lock().unwrap();
            while *held {
                held = shared.released.wait(held).unwrap();
            }
            drop(held);
            respond(&mut stream, 200, "image/jpeg", JPEG, &[]);
        }
    }
    let _ = stream.flush();
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra: &[(&str, &str)],
) {
    let mut head = format!(
        "HTTP/1.1 {status} Fake\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}
