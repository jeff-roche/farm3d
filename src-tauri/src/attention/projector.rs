//! P8 D2 "The pass", "Apply", and "Startup backfill" (ADR-0014): the
//! projector turns what is true now into Attention Events.
//!
//! One pass is one `Storage::write_repo` transaction (SQLite `BEGIN
//! IMMEDIATE`). Inside it, [`read_view`] reads the durable part of the
//! [`FarmView`] and the open Events; the caller's live statuses and the
//! in-memory [`PrinterWatch`] fill in the rest; then `observe` → `plan` →
//! [`apply`]. Reading the open Events inside the write transaction means a
//! command that acknowledged or resolved an Event a moment earlier is
//! always seen, so a pass can't undo it. The partial UNIQUE index on the
//! open dedup key is the backstop: a pass that loses a race fails,
//! rolls back, and is retried once as a full pass ([`run`]).
//!
//! [`backfill`] is the same pass with an empty status map and no
//! supervisor start, so every live-status family is `Unknown`: a restart
//! neither opens nor resolves a `printer.*` or `job.completed` Event. Its
//! inserts are `origin: backfill` and yield no capture intent and no
//! notify candidate. Running it again changes nothing (the second run
//! plans only unchanged amendments, which a pass persists at most once a
//! minute per Event: [`persisted_actions`], decision 40).

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::cameras::capture::CaptureIntent;
use crate::catalog::resolve::resolve_printer;
use crate::catalog::Catalog;
use crate::connections::PrinterStatusFacts;
use crate::incidents::repository as incidents_repository;
use crate::incidents::{Incident, IncidentEntryDetail, IncidentKind};
use crate::jobs::repository as jobs_repository;
use crate::jobs::{
    CancelReason, Job, JobEventKind, JobState, PrinterSnapshot, ReconciliationRequirement,
};
use crate::notifications::policy::NotifyCandidate;
use crate::persistence::{RepositoryError, Storage, StorageError};
use crate::printers::repository as printers_repository;
use crate::printers::setup::derive_setup_facts;
use crate::printers::{alerts, StoredPrinter};
use crate::queue::repository as queue_repository;
use crate::spools::{
    decode_enum, encode_enum, repository as spools_repository, MaterialFamily, SpoolLifecycle,
    SpoolLocation, SpoolRecord,
};

use super::lifecycle::AckBy;
use super::observe::{
    self, FarmView, JobEndedBy, JobFacts, LatestJob, Observation, ObservedConditions, PrinterFacts,
    PrinterWatch, SpoolFacts, StatusFacts,
};
use super::plan::{self, PlannedAction};
use super::repository as attention_repository;
use super::{dedup_key, AttentionEvent, AttentionOrigin, Condition, ConditionKind, IncidentRule};

/// The schema version whose `applied_at` is the attention epoch (D2).
const ATTENTION_EPOCH_VERSION: i64 = 9;

/// What one applied change did to an Event (D2 "Apply").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventChange {
    Inserted { recurred: bool },
    Amended,
    Acknowledged,
    Resolved,
    Read,
}

/// One Event row a pass changed, as committed.
#[derive(Clone, PartialEq, Debug)]
pub struct AppliedEvent {
    pub event: AttentionEvent,
    pub change: EventChange,
}

/// D2 "Apply": what a pass committed. `events` lists only rows that emit
/// (never an unchanged amendment), each once with its final row;
/// `incidents` each Incident the pass touched, once, with its final row.
/// `capture` and `notify` are the hand-offs for `CameraServices` and the
/// `NotificationService`: live inserts only, never a backfill's.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct AppliedChanges {
    pub events: Vec<AppliedEvent>,
    pub incidents: Vec<Incident>,
    pub capture: Vec<CaptureIntent>,
    pub notify: Vec<NotifyCandidate>,
}

impl AppliedChanges {
    /// The committed Event rows, in publish order.
    pub fn event_rows(&self) -> Vec<AttentionEvent> {
        self.events
            .iter()
            .map(|applied| applied.event.clone())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
            && self.incidents.is_empty()
            && self.capture.is_empty()
            && self.notify.is_empty()
    }
}

/// The live half of a pass's input: the supervisor's statuses (with their
/// Rust-only error causes), when the supervisors started (`None` during
/// the backfill), and the catalog Printers resolve their profiles against
/// (`None` during the backfill, see [`read_view`]).
pub struct LiveInputs<'a> {
    pub statuses: &'a HashMap<String, PrinterStatusFacts>,
    pub supervisors_started_at: Option<DateTime<Utc>>,
    pub catalog: Option<&'a Catalog>,
}

/// A committed pass: its changes, and the next offline-grace deadline
/// (`observe::next_deadline`) the runtime arms a sleep for.
pub struct PassOutcome {
    pub changes: AppliedChanges,
    pub next_deadline: Option<DateTime<Utc>>,
}

/// D2 "Startup backfill": the pass with every live-status family unknown.
/// Runs once in `build_runtime_services`, right after
/// `jobs::recover_after_restart`, before any command is served; its
/// changes are published once the attention runtime starts.
pub fn backfill(storage: &Storage, now: DateTime<Utc>) -> Result<AppliedChanges, RepositoryError> {
    let statuses = HashMap::new();
    let mut watch = PrinterWatch::default();
    let live = LiveInputs {
        statuses: &statuses,
        supervisors_started_at: None,
        catalog: None,
    };
    run(storage, &live, &mut watch, AttentionOrigin::Backfill, now).map(|outcome| outcome.changes)
}

/// One pass in its own IMMEDIATE transaction, retried once as a full pass
/// if it fails (D2: a concurrent insert of the same open key trips the
/// partial UNIQUE index and rolls the whole pass back).
pub fn run(
    storage: &Storage,
    live: &LiveInputs<'_>,
    watch: &mut PrinterWatch,
    origin: AttentionOrigin,
    now: DateTime<Utc>,
) -> Result<PassOutcome, RepositoryError> {
    match storage.write_repo(|tx| pass(tx, live, watch, origin, now)) {
        Ok(outcome) => Ok(outcome),
        Err(_) => storage.write_repo(|tx| pass(tx, live, watch, origin, now)),
    }
}

/// D2 "The pass", steps 1-3, inside the caller's transaction.
pub fn pass(
    tx: &Transaction<'_>,
    live: &LiveInputs<'_>,
    watch: &mut PrinterWatch,
    origin: AttentionOrigin,
    now: DateTime<Utc>,
) -> Result<PassOutcome, RepositoryError> {
    let open = attention_repository::open_events(tx)?;
    let (mut view, context) = read_view(tx, &open, live.catalog)?;
    view.supervisors_started_at = live.supervisors_started_at;
    observe::update_watch(watch, &view.printers, live.statuses, now);
    for printer in &mut view.printers {
        printer.status = live.statuses.get(&printer.id).map(StatusFacts::from_facts);
        printer.unreachable_since = watch.unreachable_since(&printer.id);
    }
    let observed = observe::observe(&view, now);
    let latest_resolved =
        attention_repository::latest_resolved_for_keys(tx, insertable_keys(&open, &observed))?;
    let actions = plan::plan(&open, &latest_resolved, &observed);
    let actions = persisted_actions(actions, &open, now);
    let changes = apply(tx, &actions, origin, &view, &context, now)?;
    Ok(PassOutcome {
        changes,
        next_deadline: observe::next_deadline(&view, now),
    })
}

/// The keys whose latest resolved Event `plan` can consult: every
/// `Present` observation with no open Event (the only case that may
/// insert, and so the only one that reads `latest_resolved`).
fn insertable_keys<'a>(
    open: &[AttentionEvent],
    observed: &'a ObservedConditions,
) -> impl Iterator<Item = &'a str> {
    let open_keys: HashSet<String> = open.iter().map(|event| event.dedup_key.clone()).collect();
    observed
        .iter()
        .filter(move |(key, observation)| {
            matches!(observation, Observation::Present(_)) && !open_keys.contains(key.as_str())
        })
        .map(|(key, _)| key.as_str())
}

/// Decision 40: how long after an Event's stored `last_observed_at` an
/// unchanged amendment is persisted again.
pub const UNCHANGED_AMEND_INTERVAL: chrono::TimeDelta = chrono::TimeDelta::seconds(60);

/// D2 "Apply" (decision 40): the actions a pass writes. `plan` stays pure
/// and keeps planning `Amend { changed: false }` for every `Present`
/// observation of an open Event; this drops each such amendment whose
/// Event (as read inside the pass's transaction) was last observed less
/// than [`UNCHANGED_AMEND_INTERVAL`] before `now`. A changed amendment,
/// and every other action, always stays. An unparseable
/// `last_observed_at` counts as due, so the write repairs it.
pub fn persisted_actions(
    actions: Vec<PlannedAction>,
    open: &[AttentionEvent],
    now: DateTime<Utc>,
) -> Vec<PlannedAction> {
    let last_observed: HashMap<&str, Option<DateTime<Utc>>> = open
        .iter()
        .map(|event| (event.id.as_str(), parse_time(&event.last_observed_at)))
        .collect();
    actions
        .into_iter()
        .filter(|action| match action {
            PlannedAction::Amend {
                event_id,
                changed: false,
                ..
            } => match last_observed.get(event_id.as_str()) {
                Some(Some(last)) => now - *last >= UNCHANGED_AMEND_INTERVAL,
                _ => true,
            },
            _ => true,
        })
        .collect()
}

/// What [`apply`] needs beyond the [`FarmView`]: each Printer's stored row
/// (a `printer.hostFailed` Incident snapshots it) and the catalog it
/// resolves against.
pub struct ViewContext<'a> {
    printers: HashMap<String, StoredPrinter>,
    catalog: Option<&'a Catalog>,
}

fn storage_error(error: rusqlite::Error) -> RepositoryError {
    RepositoryError::Storage(StorageError::from(error))
}

fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn strings(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<Vec<String>, RepositoryError> {
    let mut statement = conn.prepare(sql).map_err(storage_error)?;
    let rows = statement
        .query_map(args, |row| row.get::<_, String>(0))
        .map_err(storage_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(storage_error)?;
    Ok(rows)
}

/// D2: the attention epoch, migration 0009's `applied_at`, as an instant.
fn attention_epoch(conn: &Connection) -> Result<DateTime<Utc>, RepositoryError> {
    let text: Option<String> = conn
        .query_row(
            "SELECT applied_at FROM schema_migrations WHERE version = ?1",
            [ATTENTION_EPOCH_VERSION],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)?;
    text.as_deref()
        .and_then(epoch_from_applied_at)
        .ok_or(RepositoryError::Storage(StorageError::CorruptData {
            source_name: "database",
            source_sha256: None,
        }))
}

/// The epoch instant from migration 0009's `applied_at`, truncated to the
/// whole second: Job timestamps (`ended_at`) have whole-second precision,
/// so a Job that ended in the migration's own second would otherwise
/// compare as before the epoch and never raise its Condition.
fn epoch_from_applied_at(text: &str) -> Option<DateTime<Utc>> {
    use chrono::SubsecRound;
    parse_time(text).map(|at| at.trunc_subsecs(0))
}

/// Setup incomplete without a catalog (the backfill): only a missing or
/// unsupported Connection counts. An unresolved profile can't be judged
/// without the catalog, so such a Printer counts as applicable: its
/// `printer.*` entries are `Unknown` in the backfill anyway, and the first
/// live pass (with the catalog) judges it.
fn connection_incomplete(printer: &StoredPrinter) -> bool {
    printer
        .connection
        .as_ref()
        .is_none_or(|connection| !crate::connections::is_supported_kind(&connection.kind))
}

fn job_facts(tx: &Transaction<'_>, job: &Job) -> Result<JobFacts, RepositoryError> {
    let ended_by = if job.state.is_terminal() {
        terminal_ended_by(tx, &job.id)?
    } else {
        None
    };
    Ok(JobFacts {
        id: job.id.clone(),
        printer_id: job.printer_id.clone(),
        state: job.state,
        cancel_reason: job.cancel_reason,
        ended_by,
        ended_at: job.ended_at.as_deref().and_then(parse_time),
        host_path: job.host_path.clone(),
        started: matches!(
            job.state,
            JobState::Starting | JobState::Printing | JobState::Paused | JobState::OutcomeUnknown
        ) || job.started_at.is_some(),
        spool_id: job.spool_id.clone(),
        has_last_failure: job.last_failure.is_some(),
        label: job_label(tx, job)?,
    })
}

/// D2 "Ended by": from the Job's terminal `job_events` row.
fn terminal_ended_by(
    tx: &Transaction<'_>,
    job_id: &str,
) -> Result<Option<JobEndedBy>, RepositoryError> {
    let kinds = strings(
        tx,
        "SELECT kind FROM job_events WHERE job_id = ?1 ORDER BY sequence DESC",
        [job_id],
    )?;
    Ok(kinds
        .iter()
        .filter_map(|text| decode_enum::<JobEventKind>(text).ok())
        .find_map(JobEndedBy::from_terminal_event))
}

/// D2 "Condition detail and subject": the Queue Entry's
/// `display.modelName`, plus " — <plateLabel>" when there is one.
fn job_label(tx: &Transaction<'_>, job: &Job) -> Result<String, RepositoryError> {
    Ok(match queue_repository::load(tx, &job.queue_entry_id)? {
        Some(entry) => match entry.display.plate_label {
            Some(plate) => format!("{} — {plate}", entry.display.model_name),
            None => entry.display.model_name,
        },
        None => "A Job".to_string(),
    })
}

/// D2: `spoolLabel` is "<manufacturer> <product or material>".
fn spool_label(spool: &SpoolRecord) -> String {
    let material = match spool.material_family {
        MaterialFamily::Other => spool
            .material_other
            .clone()
            .unwrap_or_else(|| "Other".to_string()),
        family => encode_enum(family),
    };
    format!(
        "{} {}",
        spool.manufacturer,
        spool.product.clone().unwrap_or(material)
    )
}

fn spool_facts(spool: &SpoolRecord) -> SpoolFacts {
    SpoolFacts {
        id: spool.id.clone(),
        number: spool.spool_number,
        label: spool_label(spool),
        lifecycle: spool.lifecycle,
        low: spool.facets.low,
        current_mg: spool.availability.current_mg,
        low_threshold_mg: spool.low_threshold_mg,
        loaded_on: match &spool.location {
            SpoolLocation::Slot { printer_id, .. } => Some(printer_id.clone()),
            SpoolLocation::Storage { .. } => None,
        },
    }
}

/// D2 "The durable part of the `FarmView`", read inside the pass's
/// transaction. Statuses and the offline watch are the caller's.
pub fn read_view<'a>(
    tx: &Transaction<'_>,
    open: &[AttentionEvent],
    catalog: Option<&'a Catalog>,
) -> Result<(FarmView, ViewContext<'a>), RepositoryError> {
    let attention_epoch = attention_epoch(tx)?;

    // Printers, with their Setup facts, alert defaults, active and latest
    // Jobs.
    let printer_ids = strings(tx, "SELECT id FROM printers ORDER BY CAST(id AS BLOB)", [])?;
    let mut printers = Vec::with_capacity(printer_ids.len());
    let mut stored_printers = HashMap::with_capacity(printer_ids.len());
    let mut job_ids = BTreeSet::new();
    for id in printer_ids {
        let stored = printers_repository::load_in(tx, &id)?;
        let setup_incomplete = match catalog {
            Some(catalog) => {
                let (facts, _) = derive_setup_facts(&stored, catalog);
                !(facts.has_usable_connection && facts.profile_resolved)
            }
            None => connection_incomplete(&stored),
        };
        let active_job_id = strings(
            tx,
            "SELECT id FROM jobs WHERE printer_id = ?1 AND state NOT IN ('completed','failed','cancelled')",
            [&id],
        )?
        .pop();
        let latest_job = tx
            .query_row(
                "SELECT id, host_path FROM jobs WHERE printer_id = ?1
                 ORDER BY created_at DESC, id DESC LIMIT 1",
                [&id],
                |row| {
                    Ok(LatestJob {
                        id: row.get(0)?,
                        host_path: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(storage_error)?;
        job_ids.extend(active_job_id.iter().cloned());
        job_ids.extend(latest_job.iter().map(|latest| latest.id.clone()));
        printers.push(PrinterFacts {
            id: id.clone(),
            name: stored.name.clone(),
            location: stored.location.clone(),
            archived: stored.archived_at.is_some(),
            setup_incomplete,
            start_safety: stored.start_safety,
            status: None,
            unreachable_since: None,
            alert_defaults: alerts::get(tx, &id)?.alert_defaults,
            active_job_id,
            latest_job,
        });
        stored_printers.insert(id, stored);
    }

    // Requirements: every open one, plus every one an open Event names.
    let mut requirements: Vec<ReconciliationRequirement> = jobs_repository::open_requirements(tx)?;
    let mut requirement_ids: HashSet<String> = requirements
        .iter()
        .map(|requirement| requirement.id.clone())
        .collect();
    for event in open {
        if let (Some(requirement_id), Some(job_id)) = (&event.requirement_id, &event.job_id) {
            if requirement_ids.contains(requirement_id) {
                continue;
            }
            if let Some(requirement) = jobs_repository::requirements_for_job(tx, job_id)?
                .into_iter()
                .find(|requirement| &requirement.id == requirement_id)
            {
                requirement_ids.insert(requirement.id.clone());
                requirements.push(requirement);
            }
        }
    }

    // Jobs: active, some Printer's latest, named by an open Event, behind
    // an open requirement (so a requirement Event gets its Printer even
    // when its Job isn't the latest), or a failure/host cancel after the
    // epoch that has no Event yet.
    job_ids.extend(active_jobs(tx)?);
    job_ids.extend(open.iter().filter_map(|event| event.job_id.clone()));
    job_ids.extend(
        requirements
            .iter()
            .map(|requirement| requirement.job_id.clone()),
    );
    job_ids.extend(unprojected_failures(tx, attention_epoch)?);
    let mut jobs = Vec::with_capacity(job_ids.len());
    for id in &job_ids {
        if let Some(job) = jobs_repository::load_job(tx, id)? {
            jobs.push(job_facts(tx, &job)?);
        }
    }

    // Spools: every active one, plus every one an open Event, a Job, or a
    // requirement in the view names (for subjects).
    let mut spool_ids: HashSet<String> = open
        .iter()
        .filter_map(|event| event.spool_id.clone())
        .collect();
    spool_ids.extend(jobs.iter().map(|job| job.spool_id.clone()));
    spool_ids.extend(
        requirements
            .iter()
            .filter_map(|requirement| requirement.spool_id.clone()),
    );
    let spools = spools_repository::list_spools(tx)?
        .iter()
        .filter(|spool| spool.lifecycle == SpoolLifecycle::Active || spool_ids.contains(&spool.id))
        .map(spool_facts)
        .collect();

    Ok((
        FarmView {
            printers,
            jobs,
            requirements,
            spools,
            supervisors_started_at: None,
            attention_epoch,
        },
        ViewContext {
            printers: stored_printers,
            catalog,
        },
    ))
}

fn active_jobs(tx: &Transaction<'_>) -> Result<Vec<String>, RepositoryError> {
    strings(
        tx,
        "SELECT id FROM jobs WHERE state NOT IN ('completed','failed','cancelled')",
        [],
    )
}

/// Every `failed` or `cancelled{hostCancelled}` Job that ended at or after
/// `epoch` and has no Event (open or resolved) for its `job.failed` /
/// `job.hostCancelled` key yet. Both filters run in SQL, so the read no
/// longer grows with every failure the farm has ever projected.
///
/// `strftime('%s', ended_at)` is the end's whole second, so the SQL keeps
/// every row with `ended_at >= epoch` (exactly those, for the whole-second
/// epoch `attention_epoch` returns); a value SQLite can't parse is NULL and
/// drops out, as before. The RFC 3339 re-check in Rust keeps the old rule
/// for anything else (a value SQLite reads but RFC 3339 doesn't, say one
/// with no offset).
fn unprojected_failures(
    tx: &Transaction<'_>,
    epoch: DateTime<Utc>,
) -> Result<Vec<String>, RepositoryError> {
    let mut statement = tx
        .prepare(
            "SELECT id, ended_at FROM jobs
             WHERE ended_at IS NOT NULL
               AND (state = ?1 OR (state = ?2 AND cancel_reason = ?3))
               AND CAST(strftime('%s', ended_at) AS INTEGER) >= ?4
               AND NOT EXISTS (
                 SELECT 1 FROM attention_events a
                 WHERE a.dedup_key = (CASE WHEN jobs.state = ?1 THEN ?5 ELSE ?6 END) || jobs.id
               )",
        )
        .map_err(storage_error)?;
    let rows = statement
        .query_map(
            params![
                encode_enum(JobState::Failed),
                encode_enum(JobState::Cancelled),
                encode_enum(CancelReason::HostCancelled),
                epoch.timestamp(),
                dedup_key(ConditionKind::JobFailed, ""),
                dedup_key(ConditionKind::JobHostCancelled, ""),
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(storage_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(storage_error)?;
    Ok(rows
        .into_iter()
        .filter(|(_, ended_at)| parse_time(ended_at).is_some_and(|at| at >= epoch))
        .map(|(id, _)| id)
        .collect())
}

/// D3 "Printer identity": a `printer.hostFailed` Incident's snapshot,
/// built from the Printer row at open. Never an endpoint.
fn printer_snapshot(printer: &StoredPrinter, catalog: Option<&Catalog>) -> PrinterSnapshot {
    let empty;
    let catalog = match catalog {
        Some(catalog) => catalog,
        None => {
            empty = Catalog {
                generated_at: String::new(),
                source_tag: String::new(),
                notice: String::new(),
                models: Vec::new(),
            };
            &empty
        }
    };
    PrinterSnapshot {
        name: printer.name.clone(),
        location: printer.location.clone(),
        catalog_ref: Some(printer.catalog_ref.clone()),
        adapter_kind: printer
            .connection
            .as_ref()
            .map(|connection| connection.kind.clone()),
        profile: resolve_printer(catalog, printer).profile_resolution.profile,
    }
}

fn incident_kind(kind: ConditionKind) -> Option<IncidentKind> {
    match kind {
        ConditionKind::PrinterHostFailed => Some(IncidentKind::PrinterHostFailed),
        ConditionKind::JobFailed => Some(IncidentKind::JobFailed),
        ConditionKind::JobHostCancelled => Some(IncidentKind::JobHostCancelled),
        ConditionKind::RequirementJobOutcomeUnknown => {
            Some(IncidentKind::RequirementJobOutcomeUnknown)
        }
        _ => None,
    }
}

/// Incidents a pass (or a command) touched, in first-touch order.
#[derive(Default)]
struct Touched {
    order: Vec<String>,
    seen: HashSet<String>,
}

impl Touched {
    fn touch(&mut self, id: &str) {
        if self.seen.insert(id.to_string()) {
            self.order.push(id.to_string());
        }
    }
}

/// Changed Event rows in first-change order, each kept once (its last
/// change wins; the row is re-read at the end).
#[derive(Default)]
struct Changed {
    order: Vec<(String, EventChange)>,
}

impl Changed {
    fn record(&mut self, id: &str, change: EventChange) {
        match self.order.iter_mut().find(|(seen, _)| seen == id) {
            // An insert stays an insert (a notification's rule 1).
            Some((_, EventChange::Inserted { .. })) => {}
            Some((_, existing)) => *existing = change,
            None => self.order.push((id.to_string(), change)),
        }
    }
}

fn load_event(tx: &Transaction<'_>, id: &str) -> Result<AttentionEvent, RepositoryError> {
    attention_repository::load_event(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: id.to_string(),
    })
}

fn load_incident(tx: &Transaction<'_>, id: &str) -> Result<Incident, RepositoryError> {
    incidents_repository::load_incident(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: id.to_string(),
    })
}

/// D2 "Apply": writes `actions` in plan order, then the Incident rule
/// (opening rules, then linking rules, then one close check over every
/// Incident the pass touched), all inside the pass's transaction. Returns
/// the final rows and, for `origin: live`, the capture intents and notify
/// candidates.
pub fn apply(
    tx: &Transaction<'_>,
    actions: &[PlannedAction],
    origin: AttentionOrigin,
    view: &FarmView,
    context: &ViewContext<'_>,
    now: DateTime<Utc>,
) -> Result<AppliedChanges, RepositoryError> {
    let mut changed = Changed::default();
    let mut touched = Touched::default();
    let mut inserted: Vec<(&Condition, AttentionEvent)> = Vec::new();

    for action in actions {
        match action {
            PlannedAction::Insert {
                condition,
                recurrence_of,
                acknowledged,
            } => {
                let event = attention_repository::insert(
                    tx,
                    condition,
                    recurrence_of.as_deref(),
                    *acknowledged,
                    origin,
                    now,
                )?;
                changed.record(
                    &event.id,
                    EventChange::Inserted {
                        recurred: recurrence_of.is_some(),
                    },
                );
                inserted.push((condition, event));
            }
            PlannedAction::Amend {
                event_id,
                detail,
                severity,
                summary,
                changed: detail_changed,
            } => {
                attention_repository::amend(
                    tx,
                    event_id,
                    detail,
                    *severity,
                    summary,
                    *detail_changed,
                    now,
                )?;
                if *detail_changed {
                    changed.record(event_id, EventChange::Amended);
                }
            }
            PlannedAction::Acknowledge { event_id } => {
                let (event, did) =
                    attention_repository::acknowledge(tx, event_id, AckBy::System, now)?;
                if did {
                    if let Some(incident_id) = &event.incident_id {
                        incidents_repository::append_entry(
                            tx,
                            incident_id,
                            &IncidentEntryDetail::EventAcknowledged {
                                event_id: event_id.clone(),
                                by: AckBy::System,
                            },
                            None,
                            now,
                        )?;
                        touched.touch(incident_id);
                    }
                    changed.record(event_id, EventChange::Acknowledged);
                }
            }
            PlannedAction::Resolve {
                event_id,
                resolution,
            } => {
                let (event, did) = attention_repository::resolve(tx, event_id, *resolution, now)?;
                if did {
                    if let Some(incident_id) = &event.incident_id {
                        incidents_repository::append_entry(
                            tx,
                            incident_id,
                            &IncidentEntryDetail::EventResolved {
                                event_id: event_id.clone(),
                                resolution: *resolution,
                            },
                            None,
                            now,
                        )?;
                        touched.touch(incident_id);
                    }
                    changed.record(event_id, EventChange::Resolved);
                }
            }
        }
    }

    // Incident rule 1: opening rules, in plan order.
    let mut opened: Vec<(String, String, Option<String>)> = Vec::new();
    for (condition, event) in &inserted {
        match condition.spec().incident {
            IncidentRule::OpensNew => {
                let Some(printer) = condition
                    .printer_id
                    .as_ref()
                    .and_then(|id| context.printers.get(id))
                else {
                    continue;
                };
                let kind = incident_kind(condition.kind).expect("an opening Condition");
                let incident = incidents_repository::open(
                    tx,
                    kind,
                    &printer.id,
                    None,
                    &printer_snapshot(printer, context.catalog),
                    &event.id,
                    now,
                )?;
                touched.touch(&incident.id);
                opened.push((incident.id, printer.id.clone(), None));
            }
            IncidentRule::OpensOrLinksJob => {
                let Some(job_id) = &condition.job_id else {
                    continue;
                };
                match incidents_repository::for_job(tx, job_id)? {
                    Some(incident) => {
                        incidents_repository::link_event(tx, &incident.id, &event.id, now)?;
                        touched.touch(&incident.id);
                    }
                    None => {
                        let Some(job) = jobs_repository::load_job(tx, job_id)? else {
                            continue;
                        };
                        let kind = incident_kind(condition.kind).expect("an opening Condition");
                        let incident = incidents_repository::open(
                            tx,
                            kind,
                            &job.printer_id,
                            Some(job_id),
                            &job.printer_snapshot,
                            &event.id,
                            now,
                        )?;
                        touched.touch(&incident.id);
                        opened.push((incident.id, job.printer_id.clone(), Some(job_id.clone())));
                    }
                }
            }
            IncidentRule::LinksJob | IncidentRule::None => {}
        }
    }

    // Incident rule 2: link every other actionable insert whose Job now
    // has an Incident (including one opened just above).
    for (condition, event) in &inserted {
        let spec = condition.spec();
        if spec.incident != IncidentRule::LinksJob || !spec.requires_action {
            continue;
        }
        let Some(job_id) = &condition.job_id else {
            continue;
        };
        if let Some(incident) = incidents_repository::for_job(tx, job_id)? {
            incidents_repository::link_event(tx, &incident.id, &event.id, now)?;
            touched.touch(&incident.id);
        }
    }

    // Incident rule 3: one close check over every touched Incident.
    for incident_id in &touched.order {
        incidents_repository::close_if_settled(tx, incident_id, now)?;
    }

    // Final rows.
    let mut events = Vec::with_capacity(changed.order.len());
    for (id, change) in &changed.order {
        events.push(AppliedEvent {
            event: load_event(tx, id)?,
            change: *change,
        });
    }
    let mut incidents = Vec::with_capacity(touched.order.len());
    for id in &touched.order {
        incidents.push(load_incident(tx, id)?);
    }

    let mut capture = Vec::new();
    let mut notify = Vec::new();
    if origin == AttentionOrigin::Live {
        let alerts_of = |printer_id: &str| {
            view.printers
                .iter()
                .find(|printer| printer.id == printer_id)
                .map(|printer| printer.alert_defaults)
        };
        for (incident_id, printer_id, job_id) in opened {
            if alerts_of(&printer_id).is_some_and(|alerts| alerts.snapshot_on_incident) {
                capture.push(CaptureIntent::Incident {
                    incident_id,
                    printer_id,
                    job_id,
                });
            }
        }
        for (condition, event) in &inserted {
            if condition.kind != ConditionKind::JobCompleted {
                continue;
            }
            let (Some(printer_id), Some(job_id)) = (&condition.printer_id, &condition.job_id)
            else {
                continue;
            };
            if alerts_of(printer_id).is_some_and(|alerts| alerts.snapshot_on_completion) {
                capture.push(CaptureIntent::Completion {
                    event_id: event.id.clone(),
                    printer_id: printer_id.clone(),
                    job_id: job_id.clone(),
                });
            }
        }
        for applied in &events {
            if let EventChange::Inserted { .. } = applied.change {
                notify.push(NotifyCandidate {
                    event: applied.event.clone(),
                    change: applied.change,
                });
            }
        }
    }

    Ok(AppliedChanges {
        events,
        incidents,
        capture,
        notify,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Job's `ended_at` has whole-second precision; the migration's
    /// `applied_at` has milliseconds. A Job that ended in the migration's
    /// own second must still count as after the epoch.
    #[test]
    fn a_job_ended_in_the_migrations_second_is_after_the_epoch() {
        let epoch = epoch_from_applied_at("2026-09-28T06:48:56.458Z").unwrap();
        assert!(parse_time("2026-09-28T06:48:56Z").unwrap() >= epoch);
        assert!(parse_time("2026-09-28T06:48:55Z").unwrap() < epoch);
    }

    /// Final review I1(b): only the failed and host-cancelled Jobs that
    /// ended at or after the epoch and have no Event (open or resolved)
    /// yet, with the old epoch rule (the migration's own second counts,
    /// offsets are honoured), now filtered in SQL.
    #[test]
    fn unprojected_failures_are_those_since_the_epoch_with_no_event_yet() {
        let (_temp, storage) = storage();
        let hash = "d".repeat(64);
        let epoch = epoch_from_applied_at("2026-09-28T09:00:00.458Z").unwrap();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                tx.execute_batch(&format!(
                    "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
                       catalog_variant, catalog_model_id, catalog_printer_variant, notes,
                       overrides_json, created_at, updated_at)
                     VALUES ('prn-a', 1, 'Voron', '', '', '', '', '', '', '{{}}', '{T0}', '{T0}');
                     INSERT INTO content_blobs(sha256, size_bytes, created_at)
                       VALUES ('{hash}', 1, '{T0}');
                     INSERT INTO library_models(id, revision, name, format, storage_mode, created_at, updated_at)
                       VALUES ('mdl-a', 1, 'Model', 'gcode', 'managed', '{T0}', '{T0}');
                     INSERT INTO model_source_revisions(id, model_id, sequence, content_sha256, size_bytes,
                       format, origin, source_file_name, source_path, captured_at, inspector_version,
                       inspection_json)
                       VALUES ('msr-a', 'mdl-a', 1, '{hash}', 1, 'gcode', 'import', 'p.gcode',
                               '/p.gcode', '{T0}', 1, '{{}}');
                     INSERT INTO slice_revisions(id, kind, model_id, source_revision_id, gcode_sha256,
                       gcode_size, target_json, facts_json, requires_manual_printer_selection,
                       estimates_json, created_at)
                       VALUES ('slr-a', 'external', 'mdl-a', 'msr-a', '{hash}', 1,
                               '{{}}', '{{}}', 1, '{{}}', '{T0}');
                     INSERT INTO spools(id, revision, spool_number, manufacturer, material_family,
                       color_name, diameter, nominal_mg, current_mg, confidence, lifecycle, created_at,
                       updated_at)
                       VALUES ('spl-a', 1, 1, 'Acme', 'PLA', 'Black', '1.75', 1000000, 1000000,
                               'measured', 'active', '{T0}', '{T0}');"
                ))?;
                // (id, state, cancel_reason, ended_at)
                let jobs = [
                    ("job-before", "failed", "NULL", "2026-09-28T08:59:59Z"),
                    ("job-same-second", "failed", "NULL", "2026-09-28T09:00:00Z"),
                    ("job-projected", "failed", "NULL", "2026-09-28T09:30:00Z"),
                    ("job-host-cancel", "cancelled", "'hostCancelled'", "2026-09-28T09:30:00Z"),
                    ("job-operator", "cancelled", "'cancelledByOperator'", "2026-09-28T09:30:00Z"),
                    ("job-offset", "failed", "NULL", "2026-09-28T09:30:00+02:00"),
                    ("job-fraction", "failed", "NULL", "2026-09-28T10:00:00.250Z"),
                    ("job-completed", "completed", "NULL", "2026-09-28T10:30:00Z"),
                ];
                for (index, (id, state, cancel_reason, ended_at)) in jobs.iter().enumerate() {
                    tx.execute_batch(&format!(
                        "INSERT INTO spool_reservations(id, spool_id, holder_kind, holder_id, amount_mg,
                           state, operation_id, created_at)
                           VALUES ('rsv-{id}', 'spl-a', 'job', '{id}', 1000, 'active', 'op-{id}', '{T0}');
                         INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
                           state, position, policy, preference, estimate_mg, estimate_source, created_at,
                           updated_at)
                           VALUES ('qen-{id}', 1, 'slr-a', 'qln-{id}', 1, 'queued', {index} + 1, 'manual',
                                   'loadedFirst', 1000, 'operatorEntered', '{T0}', '{T0}');
                         INSERT INTO jobs(id, revision, queue_entry_id, slice_revision_id, printer_id,
                           printer_snapshot_json, spool_id, reservation_id, estimate_mg, state,
                           cancel_reason, settlement, assigned_by, created_at, updated_at, ended_at)
                           VALUES ('{id}', 1, 'qen-{id}', 'slr-a', 'prn-a', '{{}}', 'spl-a', 'rsv-{id}',
                                   1000, '{state}', {cancel_reason}, 'pending', 'operator',
                                   '{T0}', '{T0}', '{ended_at}');"
                    ))?;
                }
                // An Event for the key, even a resolved one, means projected.
                let condition = Condition {
                    kind: ConditionKind::JobFailed,
                    source_id: "job-projected".to_string(),
                    printer_id: Some("prn-a".to_string()),
                    job_id: Some("job-projected".to_string()),
                    spool_id: None,
                    requirement_id: None,
                    subject: crate::attention::AttentionSubject {
                        printer_name: Some("Voron".to_string()),
                        printer_location: None,
                        job_label: None,
                        spool_number: None,
                        spool_label: None,
                    },
                    detail: AttentionDetail::JobFailed {
                        ended_at: "2026-09-28T09:30:00Z".to_string(),
                    },
                    acknowledge: false,
                };
                let projected =
                    attention_repository::insert(tx, &condition, None, false, AttentionOrigin::Live, at(0))?;
                attention_repository::resolve(
                    tx,
                    &projected.id,
                    crate::attention::AttentionResolution::OperatorResolved,
                    at(0),
                )?;

                let mut found = unprojected_failures(tx, epoch)?;
                found.sort();
                assert_eq!(
                    found,
                    ["job-fraction", "job-host-cancel", "job-same-second"],
                    "before the epoch (by offset too), projected, operator-cancelled, and \
                     completed Jobs stay out"
                );
                Ok(())
            })
            .unwrap();
    }

    // --- Decision 40: unchanged amendments persist at most once a minute --

    use crate::attention::{AttentionDetail, AttentionSeverity};
    use crate::persistence::{MetadataRootLease, StoragePaths};

    const T0: &str = "2026-09-28T12:00:00Z";

    fn at(seconds: i64) -> DateTime<Utc> {
        T0.parse::<DateTime<Utc>>().unwrap() + chrono::TimeDelta::seconds(seconds)
    }

    fn amend(event_id: &str, changed: bool) -> PlannedAction {
        PlannedAction::Amend {
            event_id: event_id.to_string(),
            detail: AttentionDetail::SpoolLow {
                current_mg: 1,
                low_threshold_mg: 2,
            },
            severity: AttentionSeverity::Info,
            summary: "low".to_string(),
            changed,
        }
    }

    fn open_event(id: &str, last_observed_at: &str) -> AttentionEvent {
        let (_temp, storage) = storage();
        seed_low_spool(&storage, 80_000);
        let mut event = pass_at(&storage, at(0)).events[0].event.clone();
        event.id = id.to_string();
        event.last_observed_at = last_observed_at.to_string();
        event
    }

    #[test]
    fn an_unchanged_amend_is_kept_only_a_minute_after_the_last_persisted_observation() {
        let open = [
            open_event("att-recent", "2026-09-28T12:00:00Z"),
            open_event("att-garbled", "not a time"),
        ];
        let actions = vec![
            amend("att-recent", false),
            amend("att-recent", true),
            amend("att-garbled", false),
            PlannedAction::Acknowledge {
                event_id: "att-recent".to_string(),
            },
        ];
        // 59.999 s later: only the unchanged amendment of the recent Event
        // is dropped.
        let kept = persisted_actions(
            actions.clone(),
            &open,
            at(60) - chrono::TimeDelta::milliseconds(1),
        );
        assert_eq!(kept, actions[1..].to_vec());
        // Exactly 60 s later: every action is written.
        assert_eq!(persisted_actions(actions.clone(), &open, at(60)), actions);
    }

    fn storage() -> (tempfile::TempDir, Storage) {
        let temp = tempfile::tempdir().unwrap();
        let paths =
            StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
        let lease = MetadataRootLease::acquire(&paths).unwrap();
        let storage = Storage::open(paths, &lease).unwrap();
        (temp, storage)
    }

    fn seed_low_spool(storage: &Storage, current_mg: i64) {
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                tx.execute_batch(&format!(
                    "INSERT INTO spools(id, revision, spool_number, manufacturer, material_family,
                       color_name, diameter, nominal_mg, current_mg, confidence, lifecycle,
                       created_at, updated_at)
                     VALUES ('spl-a', 1, 12, 'Acme', 'PLA', 'Black', '1.75', 1000000, {current_mg},
                             'measured', 'active', '{T0}', '{T0}');"
                ))?;
                Ok(())
            })
            .unwrap();
    }

    fn set_current_mg(storage: &Storage, current_mg: i64) {
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                tx.execute(
                    "UPDATE spools SET current_mg = ?1 WHERE id = 'spl-a'",
                    [current_mg],
                )?;
                Ok(())
            })
            .unwrap();
    }

    /// One live pass at `now` (no Printers, so no statuses matter).
    fn pass_at(storage: &Storage, now: DateTime<Utc>) -> AppliedChanges {
        let statuses = HashMap::new();
        let live = LiveInputs {
            statuses: &statuses,
            supervisors_started_at: None,
            catalog: None,
        };
        run(
            storage,
            &live,
            &mut PrinterWatch::default(),
            AttentionOrigin::Live,
            now,
        )
        .unwrap()
        .changes
    }

    /// The total size of every `*-wal` file under `root`.
    fn wal_bytes(root: &std::path::Path) -> u64 {
        let mut total = 0;
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.to_string_lossy().ends_with("-wal") {
                    total += entry.metadata().unwrap().len();
                }
            }
        }
        total
    }

    fn the_event(storage: &Storage) -> AttentionEvent {
        storage
            .read(|connection| Ok(attention_repository::open_events(connection)))
            .unwrap()
            .unwrap()
            .pop()
            .expect("one open Event")
    }

    #[test]
    fn a_pass_persists_an_unchanged_amend_once_a_minute_and_a_changed_one_at_once() {
        let (temp, storage) = storage();
        seed_low_spool(&storage, 80_000);
        let inserted = pass_at(&storage, at(0));
        assert_eq!(inserted.events.len(), 1);
        let first = the_event(&storage);
        assert_eq!(first.observation_count, 1);

        // Passes inside the minute over the unchanged Event write nothing:
        // not the row, and not even a WAL frame for their empty commits.
        let wal_before = wal_bytes(temp.path());
        assert!(wal_before > 0, "the database runs in WAL mode");
        for seconds in [1, 10, 59] {
            let changes = pass_at(&storage, at(seconds));
            assert!(changes.is_empty(), "{changes:?}");
            assert_eq!(
                the_event(&storage),
                first,
                "no amendment written at +{seconds}s"
            );
        }
        assert_eq!(
            wal_bytes(temp.path()),
            wal_before,
            "a pass with nothing to write writes nothing"
        );

        // A minute on: one persisted observation, and still nothing published.
        let changes = pass_at(&storage, at(60));
        assert!(
            changes.is_empty(),
            "an unchanged amendment emits nothing: {changes:?}"
        );
        assert!(
            wal_bytes(temp.path()) > wal_before,
            "the WAL measure sees a real write"
        );
        let observed = the_event(&storage);
        assert_eq!(observed.observation_count, 2);
        assert_eq!(observed.last_observed_at, "2026-09-28T12:01:00Z");
        assert_eq!(observed.revision, first.revision);
        let changes = pass_at(&storage, at(61));
        assert!(changes.is_empty());
        assert_eq!(
            the_event(&storage),
            observed,
            "once, not on every later pass"
        );

        // A changed amendment inside the minute writes and publishes.
        set_current_mg(&storage, 70_000);
        let changes = pass_at(&storage, at(65));
        assert_eq!(changes.events.len(), 1, "{changes:?}");
        assert_eq!(changes.events[0].change, EventChange::Amended);
        let changed = the_event(&storage);
        assert_eq!(changed.revision, first.revision + 1);
        assert_eq!(changed.observation_count, 3);
        assert_eq!(changed.last_observed_at, "2026-09-28T12:01:05Z");
        assert_eq!(
            changed.detail,
            AttentionDetail::SpoolLow {
                current_mg: 70_000,
                low_threshold_mg: 100_000
            }
        );
        assert_eq!(changes.events[0].event, changed);
    }
}
