//! P6 D1/D10: Moonraker's capability adapter — staging, print control,
//! host-state queries, and camera discovery — over HTTP only. The WebSocket
//! stays the supervisor's observation channel.
//!
//! Deliberately thin: request shapes, response classification, and parsing
//! live in `files`, which has no I/O. What is left here is the HTTP client
//! and the order of the calls.
//!
//! Client rules (the OctoPrint adapter's): one client per operation, no
//! redirects (reqwest strips only its own auth headers on a cross-host
//! redirect, never `X-Api-Key`), no system proxy, and the key sent only as a
//! header marked sensitive. The key never appears in an error or in `Debug`
//! output.

use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use reqwest::header::HeaderValue;
use reqwest::{Body, Client, Method, RequestBuilder};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, ReadBuf};

use super::files::{self, LocateStep, UploadForm, WriteKind};
use crate::connections::capabilities::{
    ArtifactStaging, CameraDiscovery, CameraInfo, CommandFailure, HistoryJob, HistoryQuery,
    HostFacts, HostJobState, HostOperationFailureCode, HostStateQuery, KlippyState, LocateOutcome,
    PrintControl, StagedArtifact,
};
use crate::connections::{ConnectionConfig, ConnectionError, TLS_UNSUPPORTED_MESSAGE};

const API_KEY_HEADER: &str = "X-Api-Key";
const MIB: u64 = 1024 * 1024;
/// How much of the upload body is read from the source per chunk.
const UPLOAD_CHUNK: usize = 64 * 1024;

/// D10's timeouts. `default()` holds the production values; tests inject
/// short ones. A timeout is `Indeterminate` for a write and an `Err` for a
/// read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MoonrakerTimings {
    /// Every request's connect phase.
    pub connect: Duration,
    /// Every JSON `GET`.
    pub query: Duration,
    /// `POST /printer/print/{start,pause,resume,cancel}`. Long, because a
    /// queued start answered after 15.8 s (Gate E).
    pub control: Duration,
    /// The upload and the `locate` download: `transfer_base` plus
    /// `transfer_per_started_mib` for each started MiB (see [`Self::transfer`]).
    pub transfer_base: Duration,
    pub transfer_per_started_mib: Duration,
    /// D5's verification window after a pause, resume, or cancel `ok`, and
    /// how often it polls `host_job_state`. The executor reads these.
    pub verify_window: Duration,
    pub verify_poll_interval: Duration,
}

impl Default for MoonrakerTimings {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            query: Duration::from_secs(10),
            control: Duration::from_secs(60),
            transfer_base: Duration::from_secs(60),
            transfer_per_started_mib: Duration::from_secs(1),
            verify_window: Duration::from_secs(10),
            verify_poll_interval: Duration::from_millis(500),
        }
    }
}

impl MoonrakerTimings {
    /// `TRANSFER_TIMEOUT(size)`: the base plus one step per started MiB. LAN
    /// throughput is unmeasured (Gate C), so this is a timeout, never a size
    /// limit.
    pub fn transfer(&self, size: u64) -> Duration {
        let started_mib = size.div_ceil(MIB);
        let steps = u32::try_from(started_mib).unwrap_or(u32::MAX);
        self.transfer_base
            .saturating_add(self.transfer_per_started_mib.saturating_mul(steps))
    }
}

/// Why no request could be built. Nothing was sent in either case.
enum NotSent {
    Tls,
    InvalidKey,
    Client,
}

impl NotSent {
    fn as_read_error(&self) -> ConnectionError {
        match self {
            NotSent::Tls => ConnectionError::Protocol(TLS_UNSUPPORTED_MESSAGE.to_string()),
            NotSent::InvalidKey => {
                ConnectionError::Auth("the API key contains invalid characters".into())
            }
            NotSent::Client => ConnectionError::Unreachable("the HTTP client failed".into()),
        }
    }

    fn as_write_failure(&self) -> CommandFailure {
        CommandFailure::Definitive(match self {
            NotSent::InvalidKey => HostOperationFailureCode::AuthRejected,
            NotSent::Tls | NotSent::Client => HostOperationFailureCode::HostUnreachable,
        })
    }
}

/// Moonraker's four capabilities over one Printer's Connection config and
/// credential. Built per operation by the adapter registry's builders.
pub struct MoonrakerCapabilities {
    base_url: String,
    use_tls: bool,
    api_key: Option<zeroize::Zeroizing<String>>,
    timings: MoonrakerTimings,
}

impl std::fmt::Debug for MoonrakerCapabilities {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MoonrakerCapabilities")
            .field("base_url", &self.base_url)
            .field("use_tls", &self.use_tls)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("timings", &self.timings)
            .finish()
    }
}

impl MoonrakerCapabilities {
    pub fn new(
        config: &ConnectionConfig,
        api_key: Option<zeroize::Zeroizing<String>>,
        timings: MoonrakerTimings,
    ) -> Self {
        Self {
            base_url: format!("http://{}:{}", config.host, config.port),
            use_tls: config.use_tls,
            api_key,
            timings,
        }
    }

    fn client(&self) -> Result<Client, NotSent> {
        // D6 rule 3 keeps TLS away from writes; this refuses a stored TLS
        // Connection outright instead of speaking plain HTTP to it.
        if self.use_tls {
            return Err(NotSent::Tls);
        }
        Client::builder()
            .connect_timeout(self.timings.connect)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| NotSent::Client)
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        timeout: Duration,
    ) -> Result<RequestBuilder, NotSent> {
        let mut request = self
            .client()?
            .request(method, format!("{}{path}", self.base_url))
            .timeout(timeout);
        if let Some(key) = self.api_key.as_ref().filter(|key| !key.is_empty()) {
            let mut value = HeaderValue::from_str(key).map_err(|_| NotSent::InvalidKey)?;
            value.set_sensitive(true);
            request = request.header(API_KEY_HEADER, value);
        }
        Ok(request)
    }

    async fn get_json(&self, path: &str) -> Result<Value, ConnectionError> {
        let response = self
            .request(Method::GET, path, self.timings.query)
            .map_err(|not_sent| not_sent.as_read_error())?
            .send()
            .await
            .map_err(read_transport_error)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(read_transport_error)?;
        files::classify_read_response(status, &body)?;
        serde_json::from_slice(&body)
            .map_err(|_| ConnectionError::Protocol("the response was not JSON".into()))
    }

    async fn server_info(&self) -> Result<files::ServerInfo, ConnectionError> {
        files::parse_server_info(&self.get_json("/server/info").await?)
    }

    async fn objects(&self) -> Result<Vec<String>, ConnectionError> {
        files::parse_objects_list(&self.get_json("/printer/objects/list").await?)
    }

    async fn send_write(
        &self,
        request: Result<RequestBuilder, NotSent>,
        kind: WriteKind,
    ) -> Result<(), CommandFailure> {
        let request = request.map_err(|not_sent| not_sent.as_write_failure())?;
        let response = request.send().await.map_err(write_transport_failure)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(write_transport_failure)?;
        files::classify_write_response(kind, status, &body)
    }

    async fn control(&self, path: &str) -> Result<(), CommandFailure> {
        let request = self.request(Method::POST, path, self.timings.control);
        self.send_write(request, WriteKind::Control).await
    }
}

/// A read that got no usable answer. `without_url` keeps the host out of
/// the message; reqwest never puts headers or bodies in one.
fn read_transport_error(error: reqwest::Error) -> ConnectionError {
    if error.is_timeout() {
        ConnectionError::Timeout
    } else {
        ConnectionError::Unreachable(error.without_url().to_string())
    }
}

/// D5: a connect failure sent nothing (definitive); anything else may have
/// reached the host.
fn write_transport_failure(error: reqwest::Error) -> CommandFailure {
    files::classify_write_transport_failure(error.is_connect())
}

/// Adapts the upload source to the chunk stream reqwest's body wants.
fn upload_stream(
    mut reader: Box<dyn AsyncRead + Send + Unpin>,
) -> impl futures_util::Stream<Item = std::io::Result<Vec<u8>>> + Send + 'static {
    let mut buffer = vec![0u8; UPLOAD_CHUNK];
    futures_util::stream::poll_fn(move |context| {
        let mut read = ReadBuf::new(&mut buffer);
        match Pin::new(&mut reader).poll_read(context, &mut read) {
            Poll::Ready(Ok(())) if read.filled().is_empty() => Poll::Ready(None),
            Poll::Ready(Ok(())) => Poll::Ready(Some(Ok(read.filled().to_vec()))),
            Poll::Ready(Err(error)) => Poll::Ready(Some(Err(error))),
            Poll::Pending => Poll::Pending,
        }
    })
}

#[async_trait::async_trait]
impl ArtifactStaging for MoonrakerCapabilities {
    /// D4: `root`, `path`, `checksum`, then the streamed `file`. A 201 is
    /// not proof of the bytes; the executor follows it with `locate`.
    async fn upload(
        &self,
        artifact: &StagedArtifact,
        body: Box<dyn AsyncRead + Send + Unpin>,
    ) -> Result<(), CommandFailure> {
        let multipart = UploadForm::for_artifact(artifact)
            .into_multipart(Body::wrap_stream(upload_stream(body)), artifact.size);
        let request = self
            .request(
                Method::POST,
                "/server/files/upload",
                self.timings.transfer(artifact.size),
            )
            .map(|request| request.multipart(multipart));
        self.send_write(request, WriteKind::Upload).await
    }

    /// D4 download-and-hash: one streamed `GET`. Only a 404 is `Absent`.
    async fn locate(&self, artifact: &StagedArtifact) -> Result<LocateOutcome, ConnectionError> {
        let mut response = self
            .request(
                Method::GET,
                &files::locate_path(&artifact.host_path),
                self.timings.transfer(artifact.size),
            )
            .map_err(|not_sent| not_sent.as_read_error())?
            .send()
            .await
            .map_err(read_transport_error)?;
        let step = files::locate_step(
            response.status().as_u16(),
            response.content_length(),
            artifact.size,
        )?;
        if let LocateStep::Decided(outcome) = step {
            return Ok(outcome);
        }
        let mut hasher = Sha256::new();
        let mut byte_count: u64 = 0;
        while let Some(chunk) = response.chunk().await.map_err(read_transport_error)? {
            byte_count += chunk.len() as u64;
            // A body longer than the artifact is not ours; stop rather than
            // read (or wait for) the rest of it.
            if let Some(outcome) = files::overran_size(artifact, byte_count) {
                return Ok(outcome);
            }
            hasher.update(&chunk);
        }
        Ok(files::compare_download(
            artifact,
            byte_count,
            &format!("{:x}", hasher.finalize()),
        ))
    }
}

#[async_trait::async_trait]
impl PrintControl for MoonrakerCapabilities {
    async fn start(&self, host_path: &str) -> Result<(), CommandFailure> {
        self.control(&files::start_path(host_path)).await
    }

    async fn pause(&self) -> Result<(), CommandFailure> {
        self.control("/printer/print/pause").await
    }

    async fn resume(&self) -> Result<(), CommandFailure> {
        self.control("/printer/print/resume").await
    }

    async fn cancel(&self) -> Result<(), CommandFailure> {
        self.control("/printer/print/cancel").await
    }
}

#[async_trait::async_trait]
impl HostStateQuery for MoonrakerCapabilities {
    async fn host_facts(&self) -> Result<HostFacts, ConnectionError> {
        let info = self.server_info().await?;
        let objects = self.objects().await?;
        let cameras = self.cameras().await?;
        Ok(files::host_facts_from(&info, &objects, cameras.len()))
    }

    /// `server.info` first; objects only while Klipper is ready (Moonraker
    /// cannot answer them otherwise).
    async fn host_job_state(&self) -> Result<HostJobState, ConnectionError> {
        let info = self.server_info().await?;
        if info.klippy_state != KlippyState::Ready {
            return Ok(files::not_ready_job_state(info.klippy_state));
        }
        let objects = self.objects().await?;
        let status = self
            .get_json(&files::job_state_query_path(&objects))
            .await?;
        files::parse_host_job_state(&status, &objects)
    }

    async fn job_history(&self, query: HistoryQuery) -> Result<Vec<HistoryJob>, ConnectionError> {
        files::parse_history_list(&self.get_json(&files::history_list_path(&query)).await?)
    }
}

#[async_trait::async_trait]
impl CameraDiscovery for MoonrakerCapabilities {
    async fn cameras(&self) -> Result<Vec<CameraInfo>, ConnectionError> {
        files::parse_webcams_list(&self.get_json("/server/webcams/list").await?)
    }
}
