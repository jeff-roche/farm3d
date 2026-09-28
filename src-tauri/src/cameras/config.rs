//! P8 D4 "Camera sources": the `printer_cameras` repository and the
//! validation every camera source passes before it is stored or tested.
//!
//! A manual snapshot URL is stored only in `printer_cameras.snapshot_url`.
//! No validation message quotes the submitted value: a rejected URL can
//! carry a credential (`user:pass@`) or a LAN address.

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::contracts::command::CommandError;

use super::{CameraSource, CameraSourceKind, PrinterCamera};

/// A manual snapshot URL's length bounds, in characters (the column's
/// CHECK).
pub const SNAPSHOT_URL_MIN_CHARS: usize = 8;
pub const SNAPSHOT_URL_MAX_CHARS: usize = 2048;
/// A host webcam's name and reported service (the columns' CHECKs).
pub const WEBCAM_NAME_MAX_CHARS: usize = 128;
pub const WEBCAM_SERVICE_MAX_CHARS: usize = 64;

const SCHEME_PREFIX: &str = "http://";

fn invalid_url(field_path: &str) -> CommandError {
    CommandError::validation_at(
        field_path,
        "Enter a plain HTTP snapshot URL (8 to 2048 characters) with a host, \
         no user name or password, and no fragment.",
    )
}

/// D4 "Manual URL validation": scheme exactly `http`, a non-empty host, no
/// userinfo, no fragment, a port (if any) of 1–65535, and 8–2048
/// characters. A query string is allowed. Returns the value to store (the
/// submission with surrounding whitespace trimmed). `VALIDATION` on
/// `field_path` otherwise; the message never quotes the value.
pub fn validate_snapshot_url(value: &str, field_path: &str) -> Result<String, CommandError> {
    let trimmed = value.trim();
    if !is_valid_snapshot_url(trimmed) {
        return Err(invalid_url(field_path));
    }
    Ok(trimmed.to_string())
}

/// The rules of [`validate_snapshot_url`] as a predicate over the exact
/// string (no trimming): the resolver re-checks a stored URL with it.
pub(crate) fn is_valid_snapshot_url(value: &str) -> bool {
    let length = value.chars().count();
    if !(SNAPSHOT_URL_MIN_CHARS..=SNAPSHOT_URL_MAX_CHARS).contains(&length) {
        return false;
    }
    // The WHATWG parser silently drops tabs and newlines and trims spaces,
    // so a value with any of them would be stored as one URL and fetched as
    // another.
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    // Scheme exactly `http` (the column's `GLOB 'http://*'` is case
    // sensitive, and the parser would lower-case `HTTP`).
    let Some(rest) = value.strip_prefix(SCHEME_PREFIX) else {
        return false;
    };
    // The raw authority: the parser skips extra slashes (`http:///x` has
    // host `x`) and drops an empty userinfo (`http://@host`), so both are
    // checked before parsing.
    let authority_end = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    url.scheme() == "http"
        && url.host_str().is_some_and(|host| !host.is_empty())
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.port() != Some(0)
}

fn trimmed_text(
    value: &str,
    max_chars: usize,
    field_path: String,
    message: &str,
) -> Result<String, CommandError> {
    let trimmed = value.trim();
    let length = trimmed.chars().count();
    if length == 0 || length > max_chars || trimmed.chars().any(char::is_control) {
        return Err(CommandError::validation_at(field_path, message));
    }
    Ok(trimmed.to_string())
}

/// Validates a submitted source and returns the one to store, with the
/// host-webcam name and service trimmed (a blank service is no service).
/// Field paths are `<prefix>.webcamName`, `<prefix>.webcamService`,
/// `<prefix>.webPort`, and `<prefix>.snapshotUrl` (`prefix` is `source`
/// for the camera commands, `camera` for `create_printer`).
pub fn validate_source(source: &CameraSource, prefix: &str) -> Result<CameraSource, CommandError> {
    match source {
        CameraSource::HostWebcam {
            webcam_name,
            webcam_service,
            web_port,
        } => {
            let webcam_name = trimmed_text(
                webcam_name,
                WEBCAM_NAME_MAX_CHARS,
                format!("{prefix}.webcamName"),
                "The webcam name must be 1 to 128 characters with no control characters.",
            )?;
            let webcam_service = match webcam_service.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(service) => Some(trimmed_text(
                    service,
                    WEBCAM_SERVICE_MAX_CHARS,
                    format!("{prefix}.webcamService"),
                    "The webcam service must be at most 64 characters with no control characters.",
                )?),
            };
            if *web_port == Some(0) {
                return Err(CommandError::validation_at(
                    format!("{prefix}.webPort"),
                    "The web port must be between 1 and 65535.",
                ));
            }
            Ok(CameraSource::HostWebcam {
                webcam_name,
                webcam_service,
                web_port: *web_port,
            })
        }
        CameraSource::SnapshotUrl { snapshot_url } => Ok(CameraSource::SnapshotUrl {
            snapshot_url: validate_snapshot_url(snapshot_url, &format!("{prefix}.snapshotUrl"))?,
        }),
    }
}

// --- batch camera templates (P8 "Wire types", `CameraTemplate`) -------------

/// A batch `snapshotUrl` template's path bound, in characters.
pub const TEMPLATE_PATH_MAX_CHARS: usize = 1024;

/// A batch `snapshotUrl` template's path: starts with `/`, at most 1024
/// characters, with no `#`, whitespace, or control characters (a query
/// string is allowed). `VALIDATION` on `field_path` otherwise; the message
/// never quotes the value (a query can carry a token).
pub fn validate_template_path(path: &str, field_path: &str) -> Result<String, CommandError> {
    let valid = path.starts_with('/')
        && path.chars().count() <= TEMPLATE_PATH_MAX_CHARS
        && !path.contains(['#', '\\'])
        && !path.chars().any(|c| c.is_whitespace() || c.is_control());
    if !valid {
        return Err(CommandError::validation_at(
            field_path,
            "The snapshot path must start with \"/\", be at most 1024 characters, and have \
             no spaces, backslash, or fragment.",
        ));
    }
    Ok(path.to_string())
}

/// Whether `host` is a bare host (a name, an IPv4 address, or an IPv6
/// literal, bracketed or not) with no userinfo, port, path, query, or
/// fragment folded into it.
fn is_bare_host(host: &str) -> bool {
    if host.is_empty()
        || host.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '/' | '?' | '#' | '@' | '\\' | '%')
        })
    {
        return false;
    }
    match host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
    {
        Some(inner) => inner.parse::<std::net::Ipv6Addr>().is_ok(),
        None => {
            !host.contains(['[', ']'])
                && (!host.contains(':') || host.parse::<std::net::Ipv6Addr>().is_ok())
        }
    }
}

/// A batch row's snapshot URL: `http://<host>:<port><path>`, where `host`
/// is the row's camera host override or its Connection host, validated
/// like a manual URL ([`validate_snapshot_url`]). `None` when `host` is not
/// a bare host or the URL would not be valid; the caller reports the field.
pub fn template_snapshot_url(host: &str, port: u16, path: &str) -> Option<String> {
    let host = host.trim();
    if port == 0 || !is_bare_host(host) {
        return None;
    }
    let authority_host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let url = format!("{SCHEME_PREFIX}{authority_host}:{port}{path}");
    if !is_valid_snapshot_url(&url) {
        return None;
    }
    // The parsed URL must keep exactly this port and path: nothing in the
    // host may have moved part of it into another component.
    let parsed = reqwest::Url::parse(&url).ok()?;
    (parsed.port_or_known_default() == Some(port)).then_some(url)
}

// --- the `printer_cameras` repository ---------------------------------------

/// One `printer_cameras` row, as [`SELECT`] lists its columns.
fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<PrinterCamera> {
    let corrupt = || {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            "an unreadable printer_cameras row".into(),
        )
    };
    let source_kind: String = row.get(2)?;
    let source = match source_kind.as_str() {
        "hostWebcam" => CameraSource::HostWebcam {
            webcam_name: row.get::<_, Option<String>>(3)?.ok_or_else(corrupt)?,
            webcam_service: row.get(4)?,
            web_port: row
                .get::<_, Option<i64>>(5)?
                .map(|port| u16::try_from(port).map_err(|_| corrupt()))
                .transpose()?,
        },
        "snapshotUrl" => CameraSource::SnapshotUrl {
            snapshot_url: row.get::<_, Option<String>>(6)?.ok_or_else(corrupt)?,
        },
        _ => return Err(corrupt()),
    };
    Ok(PrinterCamera {
        printer_id: row.get(0)?,
        revision: row.get(1)?,
        source,
        updated_at: row.get(7)?,
    })
}

const SELECT: &str = "SELECT printer_id, revision, source_kind, webcam_name, webcam_service, \
                      web_port, snapshot_url, updated_at FROM printer_cameras";

/// The Printer's camera source, or `None` when it has none.
pub fn get(connection: &Connection, printer_id: &str) -> rusqlite::Result<Option<PrinterCamera>> {
    connection
        .query_row(
            &format!("{SELECT} WHERE printer_id = ?1"),
            [printer_id],
            decode,
        )
        .optional()
}

/// Every Printer's camera source, by Printer id (the Printers export and
/// import; a manual URL included, so never for a command result).
pub fn list_all(connection: &Connection) -> rusqlite::Result<Vec<PrinterCamera>> {
    let mut statement = connection.prepare(&format!("{SELECT} ORDER BY printer_id"))?;
    let rows = statement.query_map([], decode)?;
    rows.collect()
}

/// Every Printer with a camera source and its kind, by Printer id. Never a
/// URL.
pub fn list_sources(connection: &Connection) -> rusqlite::Result<Vec<(String, CameraSourceKind)>> {
    let mut statement = connection
        .prepare("SELECT printer_id, source_kind FROM printer_cameras ORDER BY printer_id")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.map(|row| {
        let (printer_id, kind) = row?;
        let kind = match kind.as_str() {
            "hostWebcam" => CameraSourceKind::HostWebcam,
            _ => CameraSourceKind::SnapshotUrl,
        };
        Ok((printer_id, kind))
    })
    .collect()
}

/// Stores `source` (already validated) as the Printer's camera: inserts it
/// at revision 1, or replaces a different one and bumps the revision.
/// Returns the stored row and whether anything changed (an identical
/// source is a no-op: no revision bump).
pub fn put(
    tx: &Transaction<'_>,
    printer_id: &str,
    source: &CameraSource,
    now: &str,
) -> rusqlite::Result<(PrinterCamera, bool)> {
    let existing = get(tx, printer_id)?;
    if let Some(existing) = &existing {
        if existing.source == *source {
            return Ok((existing.clone(), false));
        }
    }
    let (webcam_name, webcam_service, web_port, snapshot_url) = match source {
        CameraSource::HostWebcam {
            webcam_name,
            webcam_service,
            web_port,
        } => (
            Some(webcam_name.as_str()),
            webcam_service.as_deref(),
            web_port.map(i64::from),
            None,
        ),
        CameraSource::SnapshotUrl { snapshot_url } => {
            (None, None, None, Some(snapshot_url.as_str()))
        }
    };
    let kind = crate::spools::encode_enum(source.kind());
    tx.execute(
        "INSERT INTO printer_cameras(printer_id, revision, source_kind, webcam_name, \
         webcam_service, web_port, snapshot_url, updated_at) \
         VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(printer_id) DO UPDATE SET revision = revision + 1, \
         source_kind = excluded.source_kind, webcam_name = excluded.webcam_name, \
         webcam_service = excluded.webcam_service, web_port = excluded.web_port, \
         snapshot_url = excluded.snapshot_url, updated_at = excluded.updated_at",
        params![
            printer_id,
            kind,
            webcam_name,
            webcam_service,
            web_port,
            snapshot_url,
            now
        ],
    )?;
    let stored = get(tx, printer_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    Ok((stored, true))
}

/// Removes the Printer's camera source. `true` when there was one.
pub fn delete(tx: &Transaction<'_>, printer_id: &str) -> rusqlite::Result<bool> {
    Ok(tx.execute(
        "DELETE FROM printer_cameras WHERE printer_id = ?1",
        [printer_id],
    )? > 0)
}
