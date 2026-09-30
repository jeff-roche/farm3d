//! D13's six section collectors. Each reads typed facts only: enums,
//! counts, booleans, versions that pass the token rule, and ids, which the
//! bundle turns into per-bundle pseudonyms before anything is serialized.
//! No collector reads a name, a note, a host, a URL, a path, a host-
//! supplied string, or a credential.
//!
//! [`collect`] reads everything in the caller's read transaction (and the
//! log files); [`Collected::note_ids`] records every id for the bundle's
//! pseudonyms; [`Collected::render`] serializes the entries.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{OptionalExtension, Transaction};
use serde::Serialize;

use super::about::{version_token, AboutInfo};
use super::bundle::DiagnosticsSection;
use super::egress::EntryFormat;
use super::storage::{usage_in, StorageUsage};
use super::log::{is_valid_code, LOG_FILE, MAX_ENTRIES, MAX_FILES};
use super::pseudonym::{AssignedPseudonyms, BundlePseudonyms};
use crate::catalog::Catalog;
use crate::connections::capabilities::{
    CapabilityKey, CapabilityState, PrinterCapabilities, UnsupportedReason,
};
use crate::connections::{ConnectionErrorCause, ConnectionState, PrinterStatus};
use crate::contracts::command::ErrorCode;
use crate::persistence::integrity::{self, IntegrityOutcome, IntegrityRoots, IntegrityRule};
use crate::persistence::{RepositoryError, StorageError, StoragePaths};
use crate::printers::StoredPrinter;
use crate::settings::commands::{MonitorDensity, MonitorSection, SnapshotRetention};

/// The Attention Events the `recentProblems` section keeps.
pub const RECENT_PROBLEMS: usize = 50;

/// The themes farm3d ships; any other theme mode is reported as `custom`.
const BUILT_IN_THEME_MODES: &[&str] = &["system", "farm3d-light", "farm3d-dark"];

/// Runtime facts the collectors need beside the database.
pub struct CollectContext<'a> {
    pub about: AboutInfo,
    pub catalog: &'a Catalog,
    pub statuses: HashMap<String, PrinterStatus>,
    pub error_causes: HashMap<String, ConnectionErrorCause>,
    pub capabilities: &'a dyn Fn(&StoredPrinter) -> PrinterCapabilities,
    pub integrity_roots: IntegrityRoots,
    pub log_root: &'a Path,
    pub storage_paths: &'a StoragePaths,
    /// The app cache directory the Slicer keeps `orca-profiles/` under.
    pub slicer_cache: &'a Path,
    pub now: DateTime<Utc>,
}

/// What each selected section collected, before pseudonymization.
#[derive(Default)]
pub struct Collected {
    about: Option<AboutInfo>,
    health: Option<Vec<HealthRaw>>,
    storage: Option<StorageSection>,
    configuration: Option<ConfigurationSection>,
    logs: Option<Vec<LogFileRaw>>,
    recent_problems: Option<Vec<ProblemRaw>>,
}

// ------------------------------------------------------------- vocabulary

/// A closed-vocabulary word from the database (a CHECK-constrained enum or
/// a farm3d code): `[A-Za-z0-9_.:+-]`, 1–64 characters, else `other`.
fn word(text: &str) -> String {
    if super::egress::is_word(text) {
        text.to_string()
    } else {
        "other".to_string()
    }
}

/// A Connection kind farm3d has an adapter for, else `other`.
fn adapter_kind(kind: &str) -> String {
    if crate::connections::is_supported_kind(kind) {
        kind.to_string()
    } else {
        "other".to_string()
    }
}

/// An RFC 3339 time re-rendered by farm3d (milliseconds, `Z`), or `None`.
fn timestamp(text: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(text).ok().map(|time| {
        time.with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    })
}

fn error_code(cause: ConnectionErrorCause) -> ErrorCode {
    match cause {
        ConnectionErrorCause::Unreachable => ErrorCode::PrinterUnreachable,
        ConnectionErrorCause::Timeout => ErrorCode::Timeout,
        ConnectionErrorCause::Auth => ErrorCode::AuthenticationFailed,
        ConnectionErrorCause::Protocol => ErrorCode::ProtocolError,
    }
}

// ----------------------------------------------------------------- health

struct HealthRaw {
    id: String,
    adapter_kind: Option<String>,
    archived: bool,
    setup_incomplete: bool,
    connection_state: Option<ConnectionState>,
    error_code: Option<ErrorCode>,
    seconds_since_last_observation: Option<u64>,
    capabilities: Vec<HealthCapability>,
    host_software_version: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthCapability {
    capability: CapabilityKey,
    state: &'static str,
    reason: Option<UnsupportedReason>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthPrinter<'a> {
    printer: String,
    adapter_kind: &'a Option<String>,
    archived: bool,
    setup_incomplete: bool,
    connection_state: Option<ConnectionState>,
    error_code: Option<ErrorCode>,
    seconds_since_last_observation: Option<u64>,
    capabilities: &'a [HealthCapability],
    host_software_version: &'a Option<String>,
}

#[derive(Serialize)]
struct HealthSection<'a> {
    printers: Vec<HealthPrinter<'a>>,
}

fn printer_ids(tx: &Transaction<'_>) -> rusqlite::Result<Vec<String>> {
    let mut statement = tx.prepare("SELECT id FROM printers ORDER BY id")?;
    let ids = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(ids)
}

fn collect_health(
    tx: &Transaction<'_>,
    context: &CollectContext<'_>,
) -> Result<Vec<HealthRaw>, RepositoryError> {
    let mut printers = Vec::new();
    for id in printer_ids(tx).map_err(storage_error)? {
        let stored = crate::printers::repository::load_in(tx, &id)?;
        let (facts, _) = crate::printers::setup::derive_setup_facts(&stored, context.catalog);
        let status = context.statuses.get(&id);
        let connection_state = status.map(|status| status.connection_state);
        let error_code = (connection_state == Some(ConnectionState::Error))
            .then(|| context.error_causes.get(&id).copied().map(error_code))
            .flatten();
        let seconds_since_last_observation = status
            .and_then(|status| status.last_observed_at.as_deref())
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|observed| {
                (context.now - observed.with_timezone(&Utc))
                    .num_seconds()
                    .max(0) as u64
            });
        let capabilities = (context.capabilities)(&stored);
        let host_software_version = capabilities
            .host_facts
            .as_ref()
            .and_then(|facts| version_token(&facts.host_software));
        let rows = CapabilityKey::ALL
            .iter()
            .map(|key| match &capabilities.capabilities[*key] {
                CapabilityState::Supported { .. } => HealthCapability {
                    capability: *key,
                    state: "supported",
                    reason: None,
                },
                CapabilityState::Unsupported { reason, .. } => HealthCapability {
                    capability: *key,
                    state: "unsupported",
                    reason: Some(*reason),
                },
            })
            .collect();
        printers.push(HealthRaw {
            adapter_kind: stored
                .connection
                .as_ref()
                .map(|connection| adapter_kind(&connection.kind)),
            archived: stored.archived_at.is_some(),
            setup_incomplete: !(facts.has_usable_connection && facts.profile_resolved),
            connection_state,
            error_code,
            seconds_since_last_observation,
            capabilities: rows,
            host_software_version,
            id,
        });
    }
    Ok(printers)
}

// ---------------------------------------------------------------- storage

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IntegrityRow {
    rule: IntegrityRule,
    outcome: IntegrityOutcome,
    count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CodeCount {
    code: String,
    count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReasonCount {
    reason: String,
    count: i64,
}

/// `usage` is `StorageUsage` (`diagnostics/storage.rs`): fixed class names
/// and numbers.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageSection {
    usage: StorageUsage,
    integrity: Vec<IntegrityRow>,
    migration_warnings: Vec<CodeCount>,
    pending_credential_cleanup: Vec<ReasonCount>,
    pending_blob_cleanup: i64,
}

/// `(word, count)` rows of a `GROUP BY`, merged after [`word`] maps any
/// unexpected text to `other`.
fn grouped(tx: &Transaction<'_>, sql: &str) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut statement = tx.prepare(sql)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut merged = BTreeMap::new();
    for (text, count) in rows {
        *merged.entry(word(&text)).or_insert(0) += count;
    }
    Ok(merged.into_iter().collect())
}

fn collect_storage(
    tx: &Transaction<'_>,
    report: &integrity::IntegrityReport,
    context: &CollectContext<'_>,
) -> rusqlite::Result<StorageSection> {
    Ok(StorageSection {
        usage: usage_in(
            tx,
            context.storage_paths,
            context.slicer_cache,
            context.now,
        )?,
        integrity: report
            .findings
            .iter()
            .map(|finding| IntegrityRow {
                rule: finding.rule,
                outcome: finding.outcome,
                count: finding.count,
            })
            .collect(),
        migration_warnings: grouped(
            tx,
            "SELECT code, count(*) FROM migration_warnings GROUP BY code",
        )?
        .into_iter()
        .map(|(code, count)| CodeCount { code, count })
        .collect(),
        pending_credential_cleanup: grouped(
            tx,
            "SELECT reason, count(*) FROM pending_credential_cleanup GROUP BY reason",
        )?
        .into_iter()
        .map(|(reason, count)| ReasonCount { reason, count })
        .collect(),
        pending_blob_cleanup: tx.query_row(
            "SELECT count(*) FROM pending_blob_cleanup",
            [],
            |row| row.get(0),
        )?,
    })
}

// ---------------------------------------------------------- configuration

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsFacts {
    revision: i64,
    theme_mode: String,
    monitor_section: MonitorSection,
    monitor_density: MonitorDensity,
    notifications: crate::notifications::NotificationClassSettings,
    snapshot_retention: SnapshotRetention,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PrinterCounts {
    total: i64,
    by_adapter_kind: BTreeMap<String, i64>,
    archived: i64,
    profile_only: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigurationSection {
    settings: SettingsFacts,
    table_counts: BTreeMap<String, i64>,
    camera_sources: BTreeMap<String, i64>,
    printers: PrinterCounts,
    slicer_runtime_configured: bool,
}

fn collect_configuration(
    tx: &Transaction<'_>,
    report: &integrity::IntegrityReport,
) -> rusqlite::Result<ConfigurationSection> {
    let record = crate::settings::repository::load_record(tx)?;
    let theme_mode = if BUILT_IN_THEME_MODES.contains(&record.theme_mode.as_str()) {
        record.theme_mode.clone()
    } else {
        "custom".to_string()
    };
    let mut camera_sources: BTreeMap<String, i64> = [
        ("hostWebcam".to_string(), 0),
        ("snapshotUrl".to_string(), 0),
    ]
    .into();
    for (kind, count) in grouped(
        tx,
        "SELECT source_kind, count(*) FROM printer_cameras GROUP BY source_kind",
    )? {
        *camera_sources.entry(kind).or_insert(0) += count;
    }
    let mut by_adapter_kind = BTreeMap::new();
    let (mut total, mut archived, mut profile_only) = (0, 0, 0);
    {
        let mut statement = tx.prepare(
            "SELECT CASE WHEN json_valid(connection_json) THEN
                      CASE WHEN json_type(connection_json) = 'object' THEN
                           COALESCE(CASE WHEN json_type(connection_json, '$.kind') = 'text'
                                         THEN json_extract(connection_json, '$.kind') END, '')
                      END
                    END,
                    archived_at IS NOT NULL
               FROM printers",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, Option<String>>(0)?, row.get::<_, bool>(1)?))
        })?;
        for row in rows {
            let (kind, is_archived) = row?;
            total += 1;
            archived += i64::from(is_archived);
            match kind {
                Some(kind) => *by_adapter_kind.entry(adapter_kind(&kind)).or_insert(0) += 1,
                None => profile_only += 1,
            }
        }
    }
    let slicer_runtime_configured = tx
        .query_row(
            "SELECT engine_path IS NOT NULL OR preset_source_path IS NOT NULL
               FROM slicer_runtime_config WHERE singleton_id = 1",
            [],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false);
    Ok(ConfigurationSection {
        settings: SettingsFacts {
            revision: record.revision,
            theme_mode,
            monitor_section: record.monitor_section,
            monitor_density: record.monitor_density,
            notifications: record.notifications,
            snapshot_retention: record.snapshot_retention,
        },
        table_counts: report.counts.clone(),
        camera_sources,
        printers: PrinterCounts {
            total,
            by_adapter_kind,
            archived,
            profile_only,
        },
        slicer_runtime_configured,
    })
}

// ------------------------------------------------------------------- logs

/// One log file's normalized lines.
struct LogFileRaw {
    /// `farm3d.log`, `farm3d.1.log`, …
    name: String,
    lines: Vec<LogLineRaw>,
    /// The file's size on disk (the preview's estimate).
    bytes: u64,
}

struct LogLineRaw {
    ts: Option<String>,
    level: &'static str,
    code: String,
    /// `(key, kind, raw id)`.
    ids: Vec<(String, String, String)>,
    fields: Vec<(String, serde_json::Value)>,
}

/// The log file names, active first, in rotation order.
pub fn log_file_names() -> Vec<String> {
    (0..MAX_FILES)
        .map(|index| {
            if index == 0 {
                LOG_FILE.to_string()
            } else {
                format!("farm3d.{index}.log")
            }
        })
        .collect()
}

/// A log key: a camelCase identifier.
fn is_log_key(key: &str) -> bool {
    (1..=64).contains(&key.len())
        && key.as_bytes()[0].is_ascii_lowercase()
        && key.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// The id kind a log key names: `printerId` → `printer`.
fn id_kind(key: &str) -> String {
    match key.strip_suffix("Id") {
        Some(kind) if !kind.is_empty() => kind.to_string(),
        _ => key.to_string(),
    }
}

/// A field value as `LogSafe` renders one: a number, a boolean, null, or
/// a string of a shape `LogSafe` produces (an enum or variant name, an RFC
/// 3339 time, a process pseudonym). Anything else, such as a file name, is
/// `redacted`.
fn log_field(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            value.clone()
        }
        serde_json::Value::String(text) if super::egress::is_log_safe_string(text) => value.clone(),
        _ => serde_json::Value::String("redacted".into()),
    }
}

/// Parses one line; `None` drops it (not a well-formed log line).
fn parse_log_line(line: &str) -> Option<LogLineRaw> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let object = value.as_object()?;
    let level = match object.get("level")?.as_str()? {
        "error" => "error",
        "warn" => "warn",
        "info" => "info",
        _ => return None,
    };
    let code = object.get("code")?.as_str()?;
    if !is_valid_code(code) {
        return None;
    }
    let ts = object
        .get("ts")
        .and_then(serde_json::Value::as_str)
        .and_then(timestamp);
    let ids = object
        .get("ids")
        .and_then(serde_json::Value::as_object)
        .map(|ids| {
            ids.iter()
                .filter(|(key, _)| is_log_key(key))
                .filter_map(|(key, value)| {
                    let id = value.as_str()?;
                    Some((key.clone(), id_kind(key), id.to_string()))
                })
                .take(MAX_ENTRIES)
                .collect()
        })
        .unwrap_or_default();
    let fields = object
        .get("fields")
        .and_then(serde_json::Value::as_object)
        .map(|fields| {
            fields
                .iter()
                .filter(|(key, _)| is_log_key(key))
                .map(|(key, value)| (key.clone(), log_field(value)))
                .take(MAX_ENTRIES)
                .collect()
        })
        .unwrap_or_default();
    Some(LogLineRaw {
        ts,
        level,
        code: code.to_string(),
        ids,
        fields,
    })
}

/// The log files that exist, each normalized line by line.
fn collect_logs(log_root: &Path) -> Result<Vec<LogFileRaw>, StorageError> {
    let mut files = Vec::new();
    for name in log_file_names() {
        let path = log_root.join(&name);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(StorageError::Filesystem),
        };
        let text = String::from_utf8_lossy(&bytes);
        files.push(LogFileRaw {
            lines: text.lines().filter_map(parse_log_line).collect(),
            bytes: bytes.len() as u64,
            name,
        });
    }
    Ok(files)
}

// -------------------------------------------------------- recent problems

struct ProblemRaw {
    condition: String,
    severity: String,
    source_kind: String,
    source_id: String,
    first_observed_at: Option<String>,
    resolved_at: Option<String>,
    resolution: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Problem<'a> {
    condition: &'a str,
    severity: &'a str,
    source_kind: &'a str,
    source: String,
    first_observed_at: &'a Option<String>,
    resolved_at: &'a Option<String>,
    resolution: &'a Option<String>,
}

#[derive(Serialize)]
struct ProblemsSection<'a> {
    events: Vec<Problem<'a>>,
}

fn collect_recent_problems(tx: &Transaction<'_>) -> rusqlite::Result<Vec<ProblemRaw>> {
    let mut statement = tx.prepare(
        "SELECT condition, severity, source_kind, source_id, first_observed_at, resolved_at,
                resolution
           FROM attention_events
          ORDER BY first_observed_at DESC, id DESC
          LIMIT ?1",
    )?;
    let rows = statement
        .query_map([RECENT_PROBLEMS as i64], |row| {
            Ok(ProblemRaw {
                condition: word(&row.get::<_, String>(0)?),
                severity: word(&row.get::<_, String>(1)?),
                source_kind: word(&row.get::<_, String>(2)?),
                source_id: row.get(3)?,
                first_observed_at: timestamp(&row.get::<_, String>(4)?),
                resolved_at: row
                    .get::<_, Option<String>>(5)?
                    .as_deref()
                    .and_then(timestamp),
                resolution: row.get::<_, Option<String>>(6)?.as_deref().map(word),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------- collect

fn storage_error(error: rusqlite::Error) -> RepositoryError {
    RepositoryError::Storage(StorageError::from(error))
}

/// Reads every selected section inside `tx` (and the log files).
pub fn collect(
    tx: &Transaction<'_>,
    sections: &BTreeSet<DiagnosticsSection>,
    context: &CollectContext<'_>,
) -> Result<Collected, RepositoryError> {
    use DiagnosticsSection as S;
    let mut collected = Collected::default();
    let report = if sections.contains(&S::Storage) || sections.contains(&S::Configuration) {
        Some(
            integrity::check(tx, Some(&context.integrity_roots))
                .map_err(RepositoryError::Storage)?,
        )
    } else {
        None
    };
    for section in sections {
        match section {
            S::About => collected.about = Some(context.about.clone()),
            S::Health => collected.health = Some(collect_health(tx, context)?),
            S::Storage => {
                let report = report
                    .as_ref()
                    .ok_or(RepositoryError::Storage(StorageError::OperationFailed))?;
                collected.storage = Some(collect_storage(tx, report, context).map_err(storage_error)?);
            }
            S::Configuration => {
                let report = report
                    .as_ref()
                    .ok_or(RepositoryError::Storage(StorageError::OperationFailed))?;
                collected.configuration =
                    Some(collect_configuration(tx, report).map_err(storage_error)?);
            }
            S::Logs => {
                collected.logs =
                    Some(collect_logs(context.log_root).map_err(RepositoryError::Storage)?);
            }
            S::RecentProblems => {
                collected.recent_problems =
                    Some(collect_recent_problems(tx).map_err(storage_error)?);
            }
        }
    }
    Ok(collected)
}

// ----------------------------------------------------------------- render

/// Closed paths of each entry (see `egress::ScanEntry::closed_paths`).
pub const ABOUT_CLOSED: &[&str] = &["platform.os", "platform.arch", "credentialStore.kind"];
pub const HEALTH_CLOSED: &[&str] = &[
    "printers[].printer@pseudonym",
    "printers[].adapterKind",
    "printers[].connectionState",
    "printers[].errorCode",
    "printers[].capabilities[].capability",
    "printers[].capabilities[].state",
    "printers[].capabilities[].reason",
];
pub const STORAGE_CLOSED: &[&str] = &[
    "usage.classes[].class",
    "usage.measuredAt@time",
    "integrity[].rule",
    "integrity[].outcome",
    "migrationWarnings[].code",
    "pendingCredentialCleanup[].reason",
];
pub const CONFIGURATION_CLOSED: &[&str] = &[
    "settings.themeMode",
    "settings.monitorSection",
    "settings.monitorDensity",
];
pub const LOGS_CLOSED: &[&str] = &[
    "ts@time",
    "level",
    "code",
    "ids.*@pseudonym",
    "fields.*@logsafe",
];
pub const PROBLEMS_CLOSED: &[&str] = &[
    "events[].condition",
    "events[].severity",
    "events[].sourceKind",
    "events[].source@pseudonym",
    "events[].firstObservedAt@time",
    "events[].resolvedAt@time",
    "events[].resolution",
];

/// One serialized bundle entry.
pub struct RenderedEntry {
    pub section: DiagnosticsSection,
    /// The entry's name in the zip.
    pub name: String,
    pub bytes: Vec<u8>,
    pub format: EntryFormat,
    pub closed_paths: &'static [&'static str],
}

fn json_entry(
    section: DiagnosticsSection,
    name: &str,
    value: &impl Serialize,
    closed_paths: &'static [&'static str],
) -> Result<RenderedEntry, serde_json::Error> {
    Ok(RenderedEntry {
        section,
        name: name.to_string(),
        bytes: serde_json::to_vec_pretty(value)?,
        format: EntryFormat::Json,
        closed_paths,
    })
}

/// The pseudonym ordinal, for sorting rows by pseudonym (never by raw id).
fn ordinal(pseudonym: &str) -> u64 {
    pseudonym
        .rsplit('-')
        .next()
        .and_then(|number| number.parse().ok())
        .unwrap_or(0)
}

impl Collected {
    /// Records every id the bundle will show.
    pub fn note_ids(&self, pseudonyms: &mut BundlePseudonyms) {
        for printer in self.health.iter().flatten() {
            pseudonyms.note("printer", &printer.id);
        }
        for file in self.logs.iter().flatten() {
            for line in &file.lines {
                for (_, kind, id) in &line.ids {
                    pseudonyms.note(kind, id);
                }
            }
        }
        for problem in self.recent_problems.iter().flatten() {
            pseudonyms.note(&problem.source_kind, &problem.source_id);
        }
    }

    /// The log files' sizes on disk, if the logs section was collected.
    pub fn log_file_bytes(&self) -> Option<u64> {
        self.logs
            .as_ref()
            .map(|files| files.iter().map(|file| file.bytes).sum())
    }

    /// Serializes each section's entries, in canonical order.
    pub fn render(
        &self,
        pseudonyms: &AssignedPseudonyms,
    ) -> Result<Vec<RenderedEntry>, serde_json::Error> {
        use DiagnosticsSection as S;
        let mut entries = Vec::new();
        if let Some(about) = &self.about {
            entries.push(json_entry(S::About, "about.json", about, ABOUT_CLOSED)?);
        }
        if let Some(health) = &self.health {
            let mut printers: Vec<HealthPrinter<'_>> = health
                .iter()
                .map(|raw| HealthPrinter {
                    printer: pseudonyms.name("printer", &raw.id),
                    adapter_kind: &raw.adapter_kind,
                    archived: raw.archived,
                    setup_incomplete: raw.setup_incomplete,
                    connection_state: raw.connection_state,
                    error_code: raw.error_code,
                    seconds_since_last_observation: raw.seconds_since_last_observation,
                    capabilities: &raw.capabilities,
                    host_software_version: &raw.host_software_version,
                })
                .collect();
            printers.sort_by_key(|printer| ordinal(&printer.printer));
            entries.push(json_entry(
                S::Health,
                "health.json",
                &HealthSection { printers },
                HEALTH_CLOSED,
            )?);
        }
        if let Some(storage) = &self.storage {
            entries.push(json_entry(
                S::Storage,
                "storage.json",
                storage,
                STORAGE_CLOSED,
            )?);
        }
        if let Some(configuration) = &self.configuration {
            entries.push(json_entry(
                S::Configuration,
                "configuration.json",
                configuration,
                CONFIGURATION_CLOSED,
            )?);
        }
        for file in self.logs.iter().flatten() {
            let mut bytes = Vec::new();
            for line in &file.lines {
                let ids: serde_json::Map<String, serde_json::Value> = line
                    .ids
                    .iter()
                    .map(|(key, kind, id)| {
                        (
                            key.clone(),
                            serde_json::Value::String(pseudonyms.name(kind, id)),
                        )
                    })
                    .collect();
                let fields: serde_json::Map<String, serde_json::Value> =
                    line.fields.iter().cloned().collect();
                let rendered = serde_json::json!({
                    "ts": line.ts,
                    "level": line.level,
                    "code": line.code,
                    "ids": ids,
                    "fields": fields,
                });
                serde_json::to_writer(&mut bytes, &rendered)?;
                bytes.push(b'\n');
            }
            entries.push(RenderedEntry {
                section: S::Logs,
                name: format!("logs/{}", file.name),
                bytes,
                format: EntryFormat::JsonLines,
                closed_paths: LOGS_CLOSED,
            });
        }
        if let Some(problems) = &self.recent_problems {
            let events = problems
                .iter()
                .map(|raw| Problem {
                    condition: &raw.condition,
                    severity: &raw.severity,
                    source_kind: &raw.source_kind,
                    source: pseudonyms.name(&raw.source_kind, &raw.source_id),
                    first_observed_at: &raw.first_observed_at,
                    resolved_at: &raw.resolved_at,
                    resolution: &raw.resolution,
                })
                .collect();
            entries.push(json_entry(
                S::RecentProblems,
                "recent-problems.json",
                &ProblemsSection { events },
                PROBLEMS_CLOSED,
            )?);
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_line_keeps_only_typed_parts_and_drops_what_isnt_a_log_line() {
        let line = parse_log_line(
            r#"{"ts":"2026-01-01T00:00:00Z","level":"warn","code":"cameras.captureFailed",
                "ids":{"printerId":"prn-a","hostOperationId":"hop-a","bad key":"x","n":1},
                "fields":{"attempt":2,"kind":"timeout","free":"free text here","nested":{"a":1}},
                "extra":"http://192.0.2.1/"}"#,
        )
        .unwrap();
        assert_eq!(line.ts.as_deref(), Some("2026-01-01T00:00:00.000Z"));
        assert_eq!(line.level, "warn");
        assert_eq!(
            line.ids,
            vec![
                (
                    "hostOperationId".into(),
                    "hostOperation".into(),
                    "hop-a".into()
                ),
                ("printerId".into(), "printer".into(), "prn-a".into()),
            ]
        );
        let fields: BTreeMap<_, _> = line.fields.into_iter().collect();
        assert_eq!(fields["attempt"], 2);
        assert_eq!(fields["kind"], "timeout");
        assert_eq!(fields["free"], "redacted");
        assert_eq!(fields["nested"], "redacted");
        for dropped in [
            "not json",
            r#"{"level":"debug","code":"a.b"}"#,
            r#"{"level":"info","code":"free text"}"#,
            r#"[1,2]"#,
        ] {
            assert!(parse_log_line(dropped).is_none(), "{dropped}");
        }
    }

    #[test]
    fn words_and_timestamps_are_closed_vocabulary() {
        assert_eq!(word("printer.offline"), "printer.offline");
        assert_eq!(word("P9 Secret"), "other");
        assert_eq!(word(""), "other");
        assert_eq!(
            timestamp("2026-01-01T01:00:00+01:00").as_deref(),
            Some("2026-01-01T00:00:00.000Z")
        );
        assert_eq!(timestamp("yesterday"), None);
        assert_eq!(id_kind("printerId"), "printer");
        assert_eq!(id_kind("Id"), "Id");
        assert_eq!(id_kind("attempt"), "attempt");
    }
}
