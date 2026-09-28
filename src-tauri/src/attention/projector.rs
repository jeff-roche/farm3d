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
//! plans only unchanged amendments).

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
use crate::jobs::{CancelReason, Job, JobEventKind, JobState, PrinterSnapshot, ReconciliationRequirement};
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
    self, FarmView, JobEndedBy, JobFacts, LatestJob, PrinterFacts, PrinterWatch, SpoolFacts,
    StatusFacts,
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
        self.events.iter().map(|applied| applied.event.clone()).collect()
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
    let latest_resolved = attention_repository::latest_resolved_by_key(tx)?;
    let (mut view, context) = read_view(tx, &open, live.catalog)?;
    view.supervisors_started_at = live.supervisors_started_at;
    observe::update_watch(watch, &view.printers, live.statuses, now);
    for printer in &mut view.printers {
        printer.status = live.statuses.get(&printer.id).map(StatusFacts::from_facts);
        printer.unreachable_since = watch.unreachable_since(&printer.id);
    }
    let observed = observe::observe(&view, now);
    let actions = plan::plan(&open, &latest_resolved, &observed);
    let changes = apply(tx, &actions, origin, &view, &context, now)?;
    Ok(PassOutcome {
        changes,
        next_deadline: observe::next_deadline(&view, now),
    })
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

fn strings(conn: &Connection, sql: &str, args: impl rusqlite::Params) -> Result<Vec<String>, RepositoryError> {
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
    text.as_deref().and_then(parse_time).ok_or(RepositoryError::Storage(
        StorageError::CorruptData {
            source_name: "database",
            source_sha256: None,
        },
    ))
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
fn terminal_ended_by(tx: &Transaction<'_>, job_id: &str) -> Result<Option<JobEndedBy>, RepositoryError> {
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
        MaterialFamily::Other => spool.material_other.clone().unwrap_or_else(|| "Other".to_string()),
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
    let mut requirement_ids: HashSet<String> =
        requirements.iter().map(|requirement| requirement.id.clone()).collect();
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
    job_ids.extend(requirements.iter().map(|requirement| requirement.job_id.clone()));
    for (id, state, ended_at) in ended_failures(tx)? {
        let after_epoch = parse_time(&ended_at).is_some_and(|at| at >= attention_epoch);
        let kind = if state == JobState::Failed {
            ConditionKind::JobFailed
        } else {
            ConditionKind::JobHostCancelled
        };
        if after_epoch && !has_event(tx, &dedup_key(kind, &id))? {
            job_ids.insert(id);
        }
    }
    let mut jobs = Vec::with_capacity(job_ids.len());
    for id in &job_ids {
        if let Some(job) = jobs_repository::load_job(tx, id)? {
            jobs.push(job_facts(tx, &job)?);
        }
    }

    // Spools: every active one, plus every one an open Event, a Job, or a
    // requirement in the view names (for subjects).
    let mut spool_ids: HashSet<String> = open.iter().filter_map(|event| event.spool_id.clone()).collect();
    spool_ids.extend(jobs.iter().map(|job| job.spool_id.clone()));
    spool_ids.extend(requirements.iter().filter_map(|requirement| requirement.spool_id.clone()));
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

/// Every `failed` or `cancelled{hostCancelled}` Job with an end time.
fn ended_failures(tx: &Transaction<'_>) -> Result<Vec<(String, JobState, String)>, RepositoryError> {
    let mut statement = tx
        .prepare(
            "SELECT id, state, ended_at FROM jobs
             WHERE ended_at IS NOT NULL
               AND (state = ?1 OR (state = ?2 AND cancel_reason = ?3))",
        )
        .map_err(storage_error)?;
    let rows = statement
        .query_map(
            params![
                encode_enum(JobState::Failed),
                encode_enum(JobState::Cancelled),
                encode_enum(CancelReason::HostCancelled)
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(storage_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(storage_error)?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, state, ended_at)| {
            decode_enum::<JobState>(&state)
                .ok()
                .map(|state| (id, state, ended_at))
        })
        .collect())
}

fn has_event(tx: &Transaction<'_>, key: &str) -> Result<bool, RepositoryError> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM attention_events WHERE dedup_key = ?1)",
        [key],
        |row| row.get(0),
    )
    .map_err(storage_error)
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
        ConditionKind::RequirementJobOutcomeUnknown => Some(IncidentKind::RequirementJobOutcomeUnknown),
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
                let (event, did) = attention_repository::acknowledge(tx, event_id, AckBy::System, now)?;
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
            let (Some(printer_id), Some(job_id)) = (&condition.printer_id, &condition.job_id) else {
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
            if let EventChange::Inserted { recurred } = applied.change {
                notify.push(NotifyCandidate {
                    event: applied.event.clone(),
                    recurred,
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
