//! A deliberately tiny blocking HTTP/1.0 client for the harness's control
//! calls (Moonraker's and OctoPrint's REST APIs, the Toxiproxy API).
//!
//! HTTP/1.0 with `Connection: close` keeps it honest: no keep-alive and no
//! chunked responses, so the body is simply everything until EOF. It exists
//! so the harness needs no HTTP client dependency; the adapters under test
//! bring their own transport.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct Url {
    pub host: String,
    pub port: u16,
    /// Everything after the authority, without a trailing slash.
    pub base_path: String,
}

impl Url {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let rest = raw
            .strip_prefix("http://")
            .ok_or_else(|| "only http:// URLs are supported".to_string())?;
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        // `[::1]:27125` keeps its brackets in `host`, which is what a socket
        // address string needs; `is_loopback` strips them.
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !authority.starts_with('[') || host.ends_with(']') => (
                host.to_string(),
                port.parse().map_err(|_| format!("bad port in {raw}"))?,
            ),
            _ => (authority.to_string(), 80),
        };
        if host.is_empty() {
            return Err(format!("no host in {raw}"));
        }
        Ok(Self {
            host,
            port,
            base_path: path.trim_end_matches('/').to_string(),
        })
    }

    /// `host:port`, as a socket address string.
    pub fn authority(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(&self) -> Result<Value, String> {
        serde_json::from_slice(&self.body).map_err(|error| {
            format!(
                "HTTP {} body is not JSON ({error}): {}",
                self.status,
                String::from_utf8_lossy(&self.body)
            )
        })
    }
}

pub fn request(
    method: &str,
    url: &Url,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&Value>,
) -> Result<Response, String> {
    let address = url
        .authority()
        .to_socket_addrs()
        .map_err(|error| format!("resolve {}: {error}", url.authority()))?
        .next()
        .ok_or_else(|| format!("no address for {}", url.authority()))?;
    let mut stream = TcpStream::connect_timeout(&address, TIMEOUT)
        .map_err(|error| format!("connect {}: {error}", url.authority()))?;
    stream.set_read_timeout(Some(TIMEOUT)).ok();
    stream.set_write_timeout(Some(TIMEOUT)).ok();

    let body = body.map(Value::to_string).unwrap_or_default();
    let mut head = format!(
        "{method} {}{path} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
        url.base_path,
        url.authority(),
        body.len()
    );
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(body.as_bytes()))
        .map_err(|error| format!("send to {}: {error}", url.authority()))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|error| format!("read from {}: {error}", url.authority()))?;
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| format!("no HTTP header terminator from {}", url.authority()))?;
    let status_line = String::from_utf8_lossy(&raw[..split]);
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| format!("bad status line from {}: {status_line}", url.authority()))?;
    Ok(Response {
        status,
        body: raw[split + 4..].to_vec(),
    })
}

pub fn get_json(url: &Url, path: &str, headers: &[(&str, &str)]) -> Result<Value, String> {
    let response = request("GET", url, path, headers, None)?;
    if response.status != 200 {
        return Err(format!(
            "GET {path} answered HTTP {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        ));
    }
    response.json()
}

pub fn post_json(
    url: &Url,
    path: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> Result<Response, String> {
    request("POST", url, path, headers, Some(body))
}
