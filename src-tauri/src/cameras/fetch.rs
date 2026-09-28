//! P8 D4 "The `FrameFetcher`": the one bounded way farm3d reads a camera
//! frame.
//!
//! One `reqwest` client built like the Moonraker client: no TLS backend,
//! no redirects (a 3xx is `httpStatus`), no retries, and no proxy. It
//! sends a plain `GET` with no credential (the camera is another service
//! than the printer host). The request has one budget,
//! `CameraTimings::fetch` (5 s), covering the connect and the whole body.
//! The body is streamed and cut off past [`MAX_FRAME_BYTES`]; nothing past
//! the limit is buffered. The image type comes from the magic bytes, never
//! from `Content-Type`.
//!
//! Every failure is a [`CameraError`]: a kind and, for `httpStatus`, the
//! status. It never carries a URL, host, or port, in `Display`, `Debug`,
//! or serialization. `reqwest`'s own errors quote the URL, so they are
//! classified and dropped here, never formatted.

use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

use super::{CameraContentType, CameraErrorKind, FrameHeader};

/// D4: the most a frame may be (10 MiB).
pub const MAX_FRAME_BYTES: usize = 10 * 1024 * 1024;
/// The binary frame header's largest length (D7 "Binary frame").
pub const MAX_FRAME_HEADER_BYTES: usize = 4096;

const JPEG_MAGIC: &[u8] = &[0xFF, 0xD8, 0xFF];
const PNG_MAGIC: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// Why a camera fetch failed: a kind, plus the status for `httpStatus`.
/// Carries nothing else, so it can't carry an endpoint.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraError {
    kind: CameraErrorKind,
    http_status: Option<u16>,
}

impl CameraError {
    pub fn new(kind: CameraErrorKind) -> Self {
        Self {
            kind,
            http_status: None,
        }
    }

    pub fn http_status(status: u16) -> Self {
        Self {
            kind: CameraErrorKind::HttpStatus,
            http_status: Some(status),
        }
    }

    pub fn kind(&self) -> CameraErrorKind {
        self.kind
    }

    pub fn status(&self) -> Option<u16> {
        self.http_status
    }

    /// The operator-facing sentence for this failure (`CAMERA_FAILED`'s
    /// message). Never names the camera's address.
    pub fn message(&self) -> String {
        match self.kind {
            CameraErrorKind::Unreachable => "The camera could not be reached.".to_string(),
            CameraErrorKind::Timeout => "The camera did not answer in time.".to_string(),
            CameraErrorKind::HttpStatus => match self.http_status {
                Some(status) => format!("The camera answered with HTTP status {status}."),
                None => "The camera answered with an error status.".to_string(),
            },
            CameraErrorKind::TooLarge => "The camera's image is larger than 10 MiB.".to_string(),
            CameraErrorKind::NotAnImage => {
                "The camera did not return a JPEG or PNG image.".to_string()
            }
            CameraErrorKind::NoSuchWebcam => {
                "The printer no longer lists a webcam with this name.".to_string()
            }
            CameraErrorKind::NoSnapshotUrl => {
                "The printer's webcam has no snapshot URL.".to_string()
            }
            CameraErrorKind::HostMismatch => {
                "The printer's webcam points at a different host. Use a manual snapshot URL."
                    .to_string()
            }
            CameraErrorKind::UnsupportedAdapter => {
                "This printer's connection can't list its webcams. Use a manual snapshot URL."
                    .to_string()
            }
            CameraErrorKind::WebcamListFailed => {
                "farm3d could not read the printer's webcam list.".to_string()
            }
        }
    }
}

impl std::fmt::Display for CameraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for CameraError {}

/// One fetched frame, in memory only. Never stored by this module.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Frame {
    pub content_type: CameraContentType,
    pub bytes: Vec<u8>,
    pub captured_at: DateTime<Utc>,
}

impl Frame {
    /// The binary-frame header for this frame (`snapshot_id` is set by
    /// `snapshot_image` only).
    pub fn header(&self, snapshot_id: Option<String>) -> FrameHeader {
        FrameHeader {
            content_type: self.content_type,
            captured_at: self
                .captured_at
                .to_rfc3339_opts(SecondsFormat::Millis, true),
            byte_len: self.bytes.len() as i64,
            snapshot_id,
        }
    }
}

/// D7 "Binary frame": bytes 0–3 are the header length `H` (u32,
/// big-endian, at most 4096), bytes 4..4+H the UTF-8 JSON header, and the
/// rest the image.
pub fn encode_frame(header: &FrameHeader, image: &[u8]) -> Vec<u8> {
    let json = serde_json::to_vec(header).expect("a frame header always serializes");
    debug_assert!(json.len() <= MAX_FRAME_HEADER_BYTES);
    let mut out = Vec::with_capacity(4 + json.len() + image.len());
    out.extend_from_slice(&(json.len() as u32).to_be_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(image);
    out
}

/// The image type by magic bytes: JPEG `FF D8 FF`, PNG
/// `89 50 4E 47 0D 0A 1A 0A`. Anything else (an MJPEG stream's multipart
/// boundary included) is `None`.
pub fn sniff(bytes: &[u8]) -> Option<CameraContentType> {
    if bytes.starts_with(JPEG_MAGIC) {
        Some(CameraContentType::Jpeg)
    } else if bytes.starts_with(PNG_MAGIC) {
        Some(CameraContentType::Png)
    } else {
        None
    }
}

/// A transport failure's kind. The `reqwest::Error` (which quotes the
/// URL) is dropped here, unformatted.
fn transport_error(error: reqwest::Error) -> CameraError {
    CameraError::new(if error.is_timeout() {
        CameraErrorKind::Timeout
    } else {
        CameraErrorKind::Unreachable
    })
}

/// The bounded frame reader. Cheap to share: one client per process.
pub struct FrameFetcher {
    client: Option<reqwest::Client>,
    budget: Duration,
}

impl FrameFetcher {
    /// A fetcher whose every request (connect plus body) has `budget`.
    pub fn new(budget: Duration) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(budget)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .build()
            .ok();
        Self { client, budget }
    }

    pub fn budget(&self) -> Duration {
        self.budget
    }

    /// `GET`s one frame from `url` (an `http://` URL that already passed
    /// `cameras::resolve`).
    pub async fn fetch(&self, url: &str) -> Result<Frame, CameraError> {
        match tokio::time::timeout(self.budget, self.fetch_unbounded(url)).await {
            Ok(result) => result,
            Err(_) => Err(CameraError::new(CameraErrorKind::Timeout)),
        }
    }

    async fn fetch_unbounded(&self, url: &str) -> Result<Frame, CameraError> {
        let client = self
            .client
            .as_ref()
            .ok_or(CameraError::new(CameraErrorKind::Unreachable))?;
        let mut response = client
            .get(url)
            .timeout(self.budget)
            .send()
            .await
            .map_err(transport_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(CameraError::http_status(status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_FRAME_BYTES as u64)
        {
            return Err(CameraError::new(CameraErrorKind::TooLarge));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
            if bytes.len() + chunk.len() > MAX_FRAME_BYTES {
                // Dropping the response closes the connection: nothing
                // past the limit is read.
                return Err(CameraError::new(CameraErrorKind::TooLarge));
            }
            bytes.extend_from_slice(&chunk);
        }
        let content_type = sniff(&bytes).ok_or(CameraError::new(CameraErrorKind::NotAnImage))?;
        Ok(Frame {
            content_type,
            bytes,
            captured_at: Utc::now(),
        })
    }
}
