//! P8 D4 "Camera sources", resolution: a stored source becomes the one URL
//! a single fetch uses, in a `Zeroizing<String>` that lives for that fetch
//! only and is never persisted, logged, or returned.
//!
//! - `snapshotUrl`: the stored URL, re-checked.
//! - `hostWebcam`: Moonraker's `server.webcams.list` through the Printer's
//!   current Connection (the crate-private `WebcamSnapshotSource`), the
//!   entry whose `name` equals `webcamName`, its `snapshot_url`, resolved
//!   as a URL reference (`Url::join`) against
//!   `http://<Connection host>:<webPort or 80>/`. The resolved URL must be
//!   `http`, carry no userinfo, and name the Connection host (ASCII
//!   case-insensitive), else `hostMismatch`; its fragment is dropped.

use std::time::Duration;

use reqwest::Url;
use zeroize::Zeroizing;

use crate::connections::ConnectionConfig;

use super::fetch::CameraError;
use super::{CameraErrorKind, CameraSource};

/// Resolves a host webcam's reported `snapshot_url` against the Connection
/// host and `web_port` (80 when `None`).
pub fn resolve_webcam_url(
    connection_host: &str,
    web_port: Option<u16>,
    snapshot_url: &str,
) -> Result<Zeroizing<String>, CameraErrorKind> {
    let bare_host = unbracketed(connection_host);
    let url_host = if bare_host.contains(':') {
        format!("[{bare_host}]")
    } else {
        bare_host.to_string()
    };
    // A Connection host that can't form a URL can't be reached either.
    let base = Url::parse(&format!("http://{url_host}:{}/", web_port.unwrap_or(80)))
        .map_err(|_| CameraErrorKind::Unreachable)?;
    // A value that is not a URL reference at all is as good as none.
    let mut resolved = base
        .join(snapshot_url.trim())
        .map_err(|_| CameraErrorKind::NoSnapshotUrl)?;
    let same_host = resolved
        .host_str()
        .is_some_and(|host| unbracketed(host).eq_ignore_ascii_case(bare_host));
    if resolved.scheme() != "http"
        || !resolved.username().is_empty()
        || resolved.password().is_some()
        || !same_host
    {
        return Err(CameraErrorKind::HostMismatch);
    }
    resolved.set_fragment(None);
    Ok(Zeroizing::new(String::from(resolved)))
}

fn unbracketed(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host)
}

/// What a webcam-list lookup found, resolved: no entry is `noSuchWebcam`,
/// an empty or blank `snapshot_url` is `noSnapshotUrl`, and anything else
/// goes through [`resolve_webcam_url`].
pub fn resolve_listed(
    connection_host: &str,
    web_port: Option<u16>,
    listed: Option<Zeroizing<String>>,
) -> Result<Zeroizing<String>, CameraErrorKind> {
    match listed {
        None => Err(CameraErrorKind::NoSuchWebcam),
        Some(value) if value.trim().is_empty() => Err(CameraErrorKind::NoSnapshotUrl),
        Some(value) => resolve_webcam_url(connection_host, web_port, &value),
    }
}

/// The Connection a `hostWebcam` source is looked up through, with its
/// credential (sent to the printer host only, never to the camera).
#[derive(Clone)]
pub struct WebcamHost {
    pub config: ConnectionConfig,
    pub api_key: Option<Zeroizing<String>>,
}

/// Resolves `source` to one fetch's URL. The webcam-list lookup is bounded
/// by `budget` (`webcamListFailed` when it runs out).
pub async fn resolve(
    source: &CameraSource,
    host: Option<&WebcamHost>,
    budget: Duration,
) -> Result<Zeroizing<String>, CameraError> {
    match source {
        CameraSource::SnapshotUrl { snapshot_url } => {
            // Stored values passed `validate_snapshot_url`; re-checked so a
            // row written any other way can't send a request elsewhere.
            if !super::config::is_valid_snapshot_url(snapshot_url) {
                return Err(CameraError::new(CameraErrorKind::NoSnapshotUrl));
            }
            Ok(Zeroizing::new(snapshot_url.clone()))
        }
        CameraSource::HostWebcam {
            webcam_name,
            web_port,
            ..
        } => {
            let host = host.ok_or(CameraError::new(CameraErrorKind::WebcamListFailed))?;
            let build = crate::connections::adapters::descriptor(&host.config.kind)
                .and_then(|descriptor| descriptor.webcam_snapshot)
                .ok_or(CameraError::new(CameraErrorKind::UnsupportedAdapter))?;
            let lookup = build(&host.config, host.api_key.clone());
            let listed = match tokio::time::timeout(budget, lookup.snapshot_url(webcam_name)).await
            {
                Ok(Ok(listed)) => listed,
                // The Printer is unreachable, too slow, or answered badly;
                // the `ConnectionError` is dropped unformatted.
                Ok(Err(_)) | Err(_) => {
                    return Err(CameraError::new(CameraErrorKind::WebcamListFailed))
                }
            };
            resolve_listed(&host.config.host, *web_port, listed).map_err(CameraError::new)
        }
    }
}

/// Whether an adapter `kind` can resolve a `hostWebcam` source.
pub fn supports_host_webcams(kind: &str) -> bool {
    crate::connections::adapters::descriptor(kind)
        .is_some_and(|descriptor| descriptor.webcam_snapshot.is_some())
}
