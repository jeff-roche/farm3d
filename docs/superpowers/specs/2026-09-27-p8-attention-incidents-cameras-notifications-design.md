# P8 Attention, Incidents, Cameras, and Notifications Design

## Status

Approved by the controller, 2026-09-27, after one review round (issue #18
decision gate satisfied).

This is the focused design for GitHub issue #18 (P8). It is Task 1 of
`docs/superpowers/plans/2026-09-27-p8-attention-incidents-cameras-notifications.md`.
It turns the plan's Design reference D1–D9 into final decisions and binds
Tasks 2–18. Where this spec and the plan differ, this spec wins. Task 1
also edited the plan wherever this spec changed a name or signature a
later task uses ("Decisions made in this spec" lists every change).

Two ADRs record the hard-to-reverse choices:

- ADR-0014 (`docs/adr/0014-attention-is-a-projection-of-conditions.md`):
  Attention is a level-triggered projection of normalized Conditions,
  idempotent by a unique open dedup key, and the startup backfill is the
  same code path. It is not an append-only event feed.
- ADR-0015 (`docs/adr/0015-desktop-notifications-over-dbus.md`): farm3d
  sends desktop notifications through its own D-Bus service, not
  `tauri-plugin-notification`, because the plugin can't report a click.

The owner's decisions 1–3 in the plan ("Owner decisions (fixed)") are
fixed, and the planner defaults 4–15 were accepted as written. This spec
applies them and does not reopen them:

- **Decision 1:** a camera source is a host webcam (by name) or a manual
  plain-HTTP snapshot URL. The backend fetches every frame. The frontend
  never receives a camera URL, except the Setup editor's own manual URL.
- **Decision 2:** notifications go through farm3d's own D-Bus service.
- **Decision 3:** Incidents block Printer deletion. Attention Events
  don't; deleting a Printer resolves its open Events `sourceRemoved`.
- **Decisions 4–15:** the Condition catalogue, the offline grace,
  recurrence and amendment, notification classes, deep-link targets, the
  Attention center, capture timing, retention and the disk cap, per-Printer
  alert defaults, batch camera templates, the U1 read-only evidence, and
  Linux-only verification.

### The projection principle

> Rust observes what is true now (P1 status, P7 Jobs and Reconciliation
> Requirements, P3 Spool facets) and reduces it to a set of
> **Conditions**, each with a stable dedup key. A pure planner compares
> that set with the open Attention Events and returns inserts,
> amendments, acknowledgements, and resolutions. Applying the plan twice
> changes nothing the second time. An observation farm3d can't judge
> right now is **unknown**, and unknown never opens or resolves anything:
> every live-status family during the startup backfill, a Printer still
> inside its offline grace, and, for `printer.connectionError`, any
> status that can't show whether the configuration is still wrong (the
> host unreachable, connecting, or hydrated from the cache at startup).
> This is planner default 5: an unknown status neither opens nor
> resolves a `printer.*` Event.

## Goal

An operator can:

- See every condition that needs them as one durable, deduplicated
  **Attention Event**, with its severity, source, and whether it needs
  action, in an Attention center reachable from the top bar.
- **Read**, **acknowledge**, and (for manual Conditions) **resolve** an
  Event. The three are separate. Reading and acknowledging never resolve.
- Trust that an Event resolves by itself when its source clears or its
  action completes, and that a recurrence is a new Event linked to the
  old one. History is kept either way.
- Review an **Incident** for each failure: what happened, the linked
  Events, the Job's own timeline, notes, and camera evidence.
- Configure an optional **camera source** per Printer (single and batch
  setup), preview it, test it, capture snapshots, pin evidence, and see
  retention and disk use.
- Get a **desktop notification** while farm3d is unfocused, for the
  classes they enabled, and land on the exact source by clicking it.

## Scope

### In scope

- Migration 0009: `attention_events`, `incidents`, `incident_events`,
  `printer_cameras`, `printer_alert_defaults`, `camera_snapshots`, the new
  `settings` columns, and the `operations` rebuild.
- The pure observer and planner, the Attention lifecycle rules, the
  projector with its startup backfill and wake loop, and the `attention`
  event stream.
- Incidents and their append-only timelines.
- Camera sources, host-webcam resolution, bounded frame fetches, preview
  health, the media store, capture triggers, retention, the disk cap, and
  pinning.
- The notification policy, focus tracking, the Linux D-Bus notification
  service, click activation, and the `farm3d-navigate-v1` event.
- Per-Printer alert defaults, and camera and alert setup in single and
  batch creation, Printers export schema 4, settings export schema 3.
- Lifecycle guards: Incidents and pinned evidence on Printer delete, and
  evidence on Printer import.
- 22 new commands.
- Frontend: the Attention trigger and center, the Monitor badge, Event and
  Incident dock views, the Printer Status tab's open Events, the Camera
  tab, camera and alert setup sections, and the "Notifications and
  retention" dialog.
- The tracer on the in-process fakes (CI) and the simulator (evidence),
  a static-JPEG simulator camera, and a read-only U1 camera probe.

### Non-goals

- A Settings workspace or destination; history search, export,
  backup/restore, and diagnostics (P9).
- OctoPrint or ElegooLink camera or command capabilities.
- Continuous recording, timelapse, camera authentication, TLS, RTSP, and
  WebRTC.
- Webhooks, email, or any remote notification.
- Changing Monitor's own card severity. P1's `MonitorSeverity` stays
  frontend-derived. P8's Condition severity is a separate, Rust-owned
  value.
- Notifications on Windows and macOS (a `NullNotificationSink` reports
  `unsupported` there; the Attention center works everywhere).

## Product vocabulary

`CONTEXT.md` gains or updates these entries (Task 1 wrote them):

- **Condition** (new): a current fact that needs the operator's
  attention, with a stable dedup key. Rust computes it from normalized
  state and never stores it.
- **Attention Event** (updated): the durable record of one occurrence of
  a Condition, with three separate dimensions (read, acknowledged,
  resolved). A Condition that returns after its Event resolved creates a
  new Event linked to the previous one.
- **Incident** (updated): the record of one failure on one Printer,
  optionally tied to one Job, with a Printer identity snapshot, an
  append-only timeline, and its evidence. `open` until every actionable
  Event linked to it resolves, then `closed`. At most one per Job.
- **Snapshot** (new): one captured camera frame, taken for an Incident, a
  completion, or by hand. It may be pinned. Pruning removes its image but
  keeps its record.
- **Camera Source** (new): a Printer's optional host webcam (by name) or
  manual snapshot URL. Configuration only; preview health is runtime
  state.
- **Alert defaults** (new): a Printer's offline grace, notification
  muting, and capture toggles.
- **Reconciliation Requirement** (updated _Avoid_ note): P8 projects each
  open one into an Attention Event; the requirement stays the durable
  record.

## Decision gate

The umbrella P8 decision gate names eight items. Each has a named
decision here.

| Gate item | Decision | Section |
|---|---|---|
| Deduplication keys | `<condition>:<sourceKind>:<sourceId>`, one open Event per key, enforced by a partial UNIQUE index | D2 "Dedup keys" |
| Recurrence | A recurring Condition that returns after resolution inserts a new Event with `recurrenceOf`; once-per-source Conditions never recur | D2 "Recurrence" |
| Auto-resolution | By resolution mode: `auto` resolves on absent (`conditionCleared`), `action` on the source record completing (`actionCompleted`), `manual` only by the operator; unknown never resolves | D2 "Planner rules" |
| Camera source types | `hostWebcam` (name, resolved per fetch, URL never stored) and `snapshotUrl` (plain HTTP, no userinfo) | D4 |
| Capture timing | One frame at Incident open and at Job completion (per-Printer toggles), manual Capture; Setup tests and preview never stored | D4 "Capture triggers" |
| Notification mechanism and permissions | farm3d's own zbus `org.freedesktop.Notifications` client; no permission model on Linux; `notification_status` reports availability | D6 |
| Retention concurrency | One `MediaJanitor` mutex serializes captures and pruning; rows first, files after commit; a startup sweep repairs crashes | D5 |
| Disk-budget enforcement | Usage is the sum of stored `byteLen` over unpruned rows; checked inside the janitor lock before each capture; oldest unpinned first; skip when only pinned remain | D5 |

## Decisions

### D1. Vocabulary and ownership

**Decision: four Rust modules own P8, plus an alert-defaults repository.
Rust owns every Condition, dedup key, severity, transition, retention
decision, and notification decision. TypeScript presents them.**

| Owner | Owns |
|---|---|
| `attention` | Conditions (`observe`), the planner (`plan`), the Attention lifecycle, `attention_events`, the projector and backfill, the `attention` event stream, deep-link targets for notifications |
| `incidents` | `incidents`, `incident_events`, the Incident timeline, the lifecycle blockers for Incidents and pinned evidence, the import guard |
| `cameras` | `printer_cameras`, host-webcam resolution, the `FrameFetcher`, preview health, the media store, `camera_snapshots`, capture, retention, pinning, the `MediaJanitor` |
| `notifications` | The notify policy (`decide`, the rate limiter), focus, the sinks (D-Bus, null, recording), click activation, `farm3d-navigate-v1` |
| `printers::alerts` | `printer_alert_defaults` |
| `jobs`, `queue` (P7) | Unchanged logic. `QueueStream::publish` gains one in-process broadcast (D2 "Wakes") |
| `connections` (P1, P6) | Unchanged status logic. The status map gains a Rust-only error cause (D2 "Reachability"); Moonraker gains a URL-keeping webcam lookup used only by `cameras` |

- **Attention Event** ids are `att-<uuid v4>`. **Incident** ids are
  `inc-<uuid v4>`. **Incident event** ids are `iev-<uuid v4>`.
  **Snapshot** ids are `snp-<uuid v4>`.
- An Attention Event's **three dimensions** are independent columns:
  `read_at`, `acknowledged_at`, and `resolved_at` (with `resolution`).
  Acknowledging or resolving sets `read_at` if it is NULL. Nothing ever
  clears `read_at`, `acknowledged_at`, or `resolved_at`. An acknowledged
  Event stays open and actionable until it resolves.
- A resolution by the system (`conditionCleared`, `actionCompleted`,
  `sourceRemoved`) also sets `read_at`. That is the umbrella's rule
  ("resolving implies read"): the Unread filter only ever shows open
  Events.

### D2. The projection (ADR-0014)

**Decision: a pure observer turns a `FarmView` into `ObservedConditions`;
a pure planner diffs them against the open Events; the projector applies
the plan in one IMMEDIATE transaction. The startup backfill is the same
three steps with every live-status family unknown.**

#### Condition catalogue

Exactly ten Conditions. Severity, action requirement, resolution mode,
recurrence, Incident behavior, and notification class are fixed per
Condition (no Condition changes severity while open).

| `ConditionKind` | Source kind | Severity | `requiresAction` | Resolution mode | Recurs | Incident | Notification class |
|---|---|---|---|---|---|---|---|
| `printer.offline` | `printer` | `warning` | yes | `auto` | yes | no | `connectivity` |
| `printer.connectionError` | `printer` | `warning` | yes | `auto` | yes | no | `connectivity` |
| `printer.hostFailed` | `printer` | `fatal` | yes | `auto` | yes | opens a new one (no Job) | `fatal` |
| `job.startConfirmation` | `job` | `info` | yes | `action` | yes | links the Job's, if any | `confirmation` |
| `job.failed` | `job` | `fatal` | yes | `manual` | no | opens the Job's, or links it | `fatal` |
| `job.hostCancelled` | `job` | `warning` | yes | `manual` | no | opens the Job's, or links it | `connectivity` |
| `requirement.materialReconciliation` | `reconciliationRequirement` | `warning` | yes | `action` | no | links the Job's, if any | `reconciliation` |
| `requirement.jobOutcomeUnknown` | `reconciliationRequirement` | `fatal` | yes | `action` | no | opens the Job's, or links it | `fatal` |
| `spool.low` | `spool` | `warning` | no | `auto` | yes | no | `inventory` |
| `job.completed` | `job` | `info` | no | `auto` | no | no | `completion` |

- **Nothing is raised for the operator's own actions**: a Job cancelled
  by the operator (`cancelledByOperator`, `cancelledBeforeStart`),
  released (`releasedBeforeStart`), or declared (`operatorDeclared`, and
  the `declared*` ends) raises no `job.*` Condition. Its Reconciliation
  Requirement, if one opens, still projects.
- **The attention epoch.** `job.failed`, `job.hostCancelled`, and
  `job.completed` are present only for a Job whose `ended_at` is at or
  after the attention epoch: the `applied_at` of migration 0009 in
  `schema_migrations`. Both are parsed as RFC 3339 instants and compared
  as `DateTime<Utc>`, never as text. Jobs that ended under P7 raise none
  of them.
  Reconciliation Requirements and low Spools project whatever their age
  (the umbrella requires every durable P7 requirement to project).
- `printer.*` Conditions are never present for an archived or Setup
  incomplete Printer. Archiving resolves them `conditionCleared` on the
  next pass.

#### Dedup keys

`dedup_key = "<ConditionKind>:<AttentionSourceKind>:<sourceId>"`, for
example `printer.offline:printer:prn-7f3c…`,
`requirement.jobOutcomeUnknown:reconciliationRequirement:rrq-…`. The
condition and source kind contain no `:`, and the id is last, so a key is
unique without escaping. Keys are compared, never parsed.

#### Reachability

A Printer's status reduces to one `Reach`, computed by
`attention::observe::reach(status, cause)`:

| Status (`PrinterStatus.connection_state`, and the Rust-only cause) | `Reach` |
|---|---|
| `online` | `Reachable` |
| `offline` or `connecting` | `Unreachable` |
| `error` with cause `unreachable` or `timeout` | `Unreachable` |
| `error` with cause `auth` or `protocol` | `Misconfigured(cause)` |
| `error` with no recorded cause | `Unknown` |
| no status in the map | `Unknown` |

**Code fact that shapes this:** an unplugged or unroutable Moonraker host
does **not** show as `offline`. The supervisor's reconnect fails with
`ConnectionError::Unreachable` or `Timeout`, and `apply_error_to`
(`connections/supervisor.rs:1002`) publishes `ConnectionState::Error`
("The Printer could not be reached."). `offline` appears for a hydrated
startup status and for Moonraker's own health reports (Klippy shut down or
disconnected). So `printer.offline` must cover an unreachable `error`, and
`printer.connectionError` means authentication or protocol failure only.

To tell them apart without matching message strings, the status map
records a Rust-only `ConnectionErrorCause` (`unreachable`, `timeout`,
`auth`, `protocol`) beside each `error` status: `apply_error_to` maps it
from the `ConnectionError` variant (`Unreachable`, `Timeout`, `Auth`,
`Protocol`/`HostNotReady`), and `apply_connection_error` (`report_error`)
records `protocol`. The unsupported-kind path records nothing (that
Printer is Setup incomplete, so the cause is never read). The cause is
never serialized: `PrinterStatus` and its generated contract are
unchanged. `ConnectionManager::status_facts() -> HashMap<String,
PrinterStatusFacts>` (crate-visible) returns each status with its cause.

#### The offline watch

The projector keeps an in-memory `PrinterWatch` per Printer:
`unreachable_since: Option<DateTime<Utc>>`. The pure
`observe::update_watch(watch, printers, statuses, now)` runs at the start
of every pass:

- `Reach::Unreachable` and `unreachable_since` is `None`: set it to `now`.
- `Reachable` or `Misconfigured`, or the Printer is archived or Setup
  incomplete: clear it.
- `Unknown`: leave it.

Passes run on every status broadcast (coalesced to one per second, see
"Runtime"), so `now` is within a second of the change. The watch is not
persisted: after a restart, it starts again at the first pass that sees
the Printer unreachable, which the startup window below makes safe.

#### Observation rules

`observe` emits, for every source in the view, one entry per Condition
that source can have: `Present(Condition)`, `Absent`, or `Unknown`. A key
the view doesn't mention at all is treated as `Unknown` by the planner.

`grace(P)` is `P.alertDefaults.offlineAfterMinutes` minutes, and
`started` is `FarmView.supervisors_started_at` (`None` during the
backfill). "Not applicable" means the Printer is archived or Setup
incomplete.

| Condition | `Present` when | `Absent` when | `Unknown` otherwise, including |
|---|---|---|---|
| `printer.offline` | applicable, grace not off, `started` set, reach `Unreachable`, and `now ≥ max(unreachable_since, started) + grace` | not applicable; grace off; reach `Reachable` or `Misconfigured` | backfill; reach `Unknown`; `Unreachable` within the grace |
| `printer.connectionError` | applicable, `started` set, reach `Misconfigured(cause)` | not applicable (archived, or Setup incomplete, which includes having no Connection); reach `Reachable` | backfill; reach `Unknown`; reach `Unreachable` (offline, connecting, a hydrated startup status, or an unreachable/timeout error: none of them shows whether the credentials or protocol are still wrong) |
| `printer.hostFailed` | applicable, `started` set, reach `Reachable`, `operationalState` `failed`, and not covered by a Job | not applicable; reach `Reachable` and `operationalState` neither `failed` nor `unknown`; reach `Reachable`, `failed`, and covered by a Job | backfill; reach not `Reachable`; `operationalState` `unknown` |
| `job.startConfirmation` | the Job is `awaitingStart`, and its Printer's Start-safety rule is `confirmBedClear` or the Job has a `lastFailure` | the Job is in any other state; or `awaitingStart` on an `unattended` Printer with no `lastFailure` | the Job isn't in the view |
| `job.failed` | the Job is `failed`, ended by the tracker (its terminal event is `failed`), `ended_at ≥` epoch | any other state or end | the Job isn't in the view |
| `job.hostCancelled` | the Job is `cancelled` with `cancelReason: hostCancelled`, `ended_at ≥` epoch | any other state or reason | the Job isn't in the view |
| `requirement.materialReconciliation` | the requirement is `pending` or `deferred` (with `acknowledge: true` when `deferred`) | it is `resolved` | it isn't in the view |
| `requirement.jobOutcomeUnknown` | the requirement is `pending` | it is `resolved` | it isn't in the view |
| `spool.low` | the Spool's `low` facet is true | `low` is false (including every `empty` or `archived` Spool, whose `low` is always false) | the Spool isn't in the view |
| `job.completed` | the Job is `completed`, ended by the tracker, `ended_at ≥` epoch, it is its Printer's latest Job, the Printer is not archived, `started` set, reach `Reachable`, `operationalState` `finished`, and the reported file equals the Job's `hostPath` | the Job isn't `completed`; ended by a declaration; before the epoch; not the latest Job; the Printer is archived; reach `Reachable` with `operationalState` neither `finished` nor `unknown`, or a different reported file | backfill; the Job isn't in the view; reach not `Reachable`; `operationalState` `unknown` |

- **Covered by a Job** (`printer.hostFailed`): the host's reported file
  (`telemetry.job_name`) is present and equals the `hostPath` of the
  Printer's latest Job, **and** that Job can still carry the failure: it
  is `starting`, `printing`, `paused`, or `outcomeUnknown` (its
  `job.failed` or `requirement.jobOutcomeUnknown` is still to come), or
  the tracker ended it `failed` (`ended_by: tracker`; its `job.failed` is
  the carrier). One rule, by file: a Job that is merely `assigned` or
  `awaitingStart` never covers a failure, even if its staged file has the
  same name (the host printed it without farm3d), and neither does a
  latest Job that ended `completed`, `cancelled`, or by a declaration
  (for example, the operator reprints the same file from the host's own
  UI after the Job completed: no Job Event would ever carry that
  failure). While covered, `job.failed` (once the tracker proves it) or
  `requirement.jobOutcomeUnknown` carries the failure, so
  `printer.hostFailed` raises no Event **and** no Incident. This is a
  deliberate refinement of planner default 4 ("Decisions made in this
  spec" 7 and 34).
- **Ended by** (`JobFacts.ended_by`) comes from the Job's terminal
  `job_events` row: `completed`, `failed`, `cancelled` → `tracker`; the
  three `declared*` → `declared`; `released`, `cancelledBeforeStart` →
  `operator`.
- **Latest Job** of a Printer: its Job with the greatest `(created_at,
  id)`.

#### Condition detail and subject

Each `Condition` carries a `detail` (an `AttentionDetail`, tagged by
`kind`) and a `subject` (an `AttentionSubject`, fixed at insert):

| Condition | `detail` |
|---|---|
| `printer.offline` | `{ kind: "printerOffline", unreachableSince }` |
| `printer.connectionError` | `{ kind: "printerConnectionError", cause: "auth" \| "protocol" }` |
| `printer.hostFailed` | `{ kind: "printerHostFailed" }` |
| `job.startConfirmation` | `{ kind: "jobStartConfirmation", awaitingMaterial }` (the Job's Spool is not loaded on its Printer) |
| `job.failed` | `{ kind: "jobFailed", endedAt }` |
| `job.hostCancelled` | `{ kind: "jobHostCancelled", endedAt }` |
| `requirement.materialReconciliation` | `{ kind: "requirementMaterialReconciliation", requirementStatus: "pending" \| "deferred", spoolId }` |
| `requirement.jobOutcomeUnknown` | `{ kind: "requirementJobOutcomeUnknown" }` |
| `spool.low` | `{ kind: "spoolLow", currentMg, lowThresholdMg }` |
| `job.completed` | `{ kind: "jobCompleted", endedAt }` |

- `subject` is `{ printerName, printerLocation, jobLabel, spoolNumber,
  spoolLabel }`, each nullable. `jobLabel` is the Queue Entry's
  `display.modelName`, plus " — <plateLabel>" when there is one.
  `spoolLabel` is "<manufacturer> <product or material>". It never holds
  a host, port, URL, or credential reference.
- `summary` (Rust-built, one sentence, shown in the center and used as the
  notification body) is derived from the kind and the subject, for
  example "Voron (Bay A) is offline.", "Cube — Plate 1 failed on Voron.",
  "Spool #12 is low (80 g left)."
- **`detail` is planner-owned.** Every `detail` field is a pure function
  of the `FarmView`, and nothing but `Insert` and `Amend` writes
  `detail_json`. That is what makes the planner a fixed point for every
  Condition.
- **Evidence is not detail.** The completion capture's outcome is
  written to its own column, `attention_events.evidence_json` (wire:
  `AttentionEvent.evidence: EvidenceOutcome | null`), only for
  `job.completed`, by `attention::repository::record_evidence(tx,
  eventId, outcome)` (D4). `Amend` never reads or writes it, so a later
  pass can't erase it. `record_evidence` bumps `revision` and emits
  `attention.event.changed`; it writes once (a second call for the same
  Event is a no-op).

#### Planner rules

`plan(open, latest_resolved, observed)` walks `observed` in key order
(a `BTreeMap`). For each key, with `e` the open Event for the key (if
any) and `r` the latest resolved Event for the key (if any):

| Observation | Open Event `e` exists | No open Event, `r` exists | Neither |
|---|---|---|---|
| `Present(c)` | `Amend { e, detail, severity, changed }`, where `changed` is true when `detail` or `severity` differs from `e`'s; then, if `c.acknowledge` and `e` is unacknowledged, `Acknowledge { e }` | recurring Condition: `Insert { c, recurrenceOf: r, acknowledged: c.acknowledge }`; once-per-source Condition: nothing | `Insert { c, recurrenceOf: null, acknowledged: c.acknowledge }` |
| `Absent` | mode `auto`: `Resolve { e, conditionCleared }`; mode `action`: `Resolve { e, actionCompleted }`; mode `manual`: nothing | nothing | nothing |
| `Unknown` | nothing | nothing | nothing |

- Only `plan` resolves on observation. `manual` Events resolve only
  through `resolve_attention_event`. `sourceRemoved` comes only from the
  Printer delete and import transactions (D8).
- **Recurrence.** "Recurs" in the catalogue. A once-per-source Condition
  (`job.failed`, `job.hostCancelled`, both `requirement.*`, and
  `job.completed`) never gets a second Event for the same key: a
  terminal Job stays failed after the operator resolves `job.failed`, and
  must not raise it again.
- **Amendment.** Every `Present` observation of an open Event amends
  `last_observed_at` and `observation_count` (+1). `detail` and `severity`
  are rewritten only when they changed. An amendment with `changed: false`
  does not bump `revision` and emits nothing; with `changed: true` it
  bumps `revision` and emits `attention.event.changed`. No amendment ever
  notifies.
- **Fixed point.** For any input, applying `plan`'s actions and planning
  again over the result yields only `Amend { changed: false }` actions.

#### Lifecycle rules

`attention::lifecycle::apply(event: &AttentionEvent, op: LifecycleOp,
now) -> Result<LifecycleChange, LifecycleError>` is pure. `LifecycleOp`
is `MarkRead`, `Acknowledge { by: AckBy }` (`operator` or `system`), or
`Resolve(AttentionResolution)`. `LifecycleChange` carries the new
`read_at`, `acknowledged_at`, `resolved_at`, `resolution`, and `changed:
bool`. The Event's state is one of five:

| From | `MarkRead` | `Acknowledge` | `Resolve(operatorResolved)` | `Resolve(conditionCleared \| actionCompleted \| sourceRemoved)` |
|---|---|---|---|---|
| unread, unacknowledged, open | read | read, acknowledged | manual: read, resolved; else `NotManual` | read, resolved |
| read, unacknowledged, open | no-op | acknowledged | manual: resolved; else `NotManual` | resolved |
| read, acknowledged, open | no-op | no-op | manual: resolved; else `NotManual` | resolved |
| read, unacknowledged, resolved | no-op | no-op | manual: no-op; else `NotManual` | no-op |
| read, acknowledged, resolved | no-op | no-op | manual: no-op; else `NotManual` | no-op |

- Unread-and-acknowledged and unread-and-resolved are impossible (a
  CHECK enforces both).
- A no-op returns `changed: false`. The command still succeeds, claims its
  `operationId`, returns the unchanged row, bumps nothing, and publishes
  nothing.
- `NotManual` maps to `ATTENTION_NOT_MANUAL`. It is checked before the
  resolved check, so the answer for a given Condition never depends on
  timing.
- The planner never produces a system `Resolve` for a resolved Event; the
  table row exists so the function is total.

#### Apply

`projector::apply(tx, actions, origin, now) -> AppliedChanges` runs inside
the pass's transaction. Per action:

| Action | Writes |
|---|---|
| `Insert` | A new `attention_events` row (`origin`, `first_observed_at = last_observed_at = now`, `observation_count = 1`, `acknowledged_at = read_at = now` when `acknowledged`). Then the Incident rule below. |
| `Amend` | `last_observed_at`, `observation_count + 1`; when `changed`, `detail_json`, `severity`, `revision + 1`. Never `evidence_json`, `incident_id`, or the lifecycle columns |
| `Acknowledge` | `lifecycle::apply(Acknowledge { by: system })`; `revision + 1`; an `eventAcknowledged` timeline row if linked |
| `Resolve` | `lifecycle::apply(Resolve(reason))`; `revision + 1`; an `eventResolved` timeline row if linked |

**Incident rule** (in the same transaction, after every action of the
pass has been applied, in this order):

1. **Opening rules first.** For each `Insert` in plan order:
   - `printer.hostFailed`: open a new Incident (`job_id` NULL) and write
     `opened { eventId }`;
   - `job.failed`, `job.hostCancelled`, or
     `requirement.jobOutcomeUnknown`: if the Job has an Incident, link
     the Event to it (`eventLinked`; `reopened` first if it was closed);
     otherwise open one for the Job (`opened { eventId }`).
2. **Then linking rules.** For each `Insert` of any other actionable
   Condition whose Job now has an Incident (in practice
   `requirement.materialReconciliation`, including one inserted in the
   same pass as the Job's `job.failed`): link it (`eventLinked`,
   `reopened` first if it was closed).
3. **Then the close check, once**, over every Incident touched by any
   action of the pass (an open, a link, or an acknowledged or resolved
   linked Event): if it is open and every linked actionable Event is
   resolved, close it (`closed`, `closed_at = now`). A resolution and a
   new link in the same pass therefore never close and reopen the same
   Incident.

An opened Incident with `origin: live` and the Printer's
`snapshotOnIncident` on yields a `CaptureIntent::Incident`. An inserted
`job.completed` with `origin: live` and `snapshotOnCompletion` on yields a
`CaptureIntent::Completion`. Every inserted Event with `origin: live`
yields a notify candidate. Backfilled inserts yield neither.

`AppliedChanges { events: Vec<AppliedEvent>, incidents: Vec<Incident>,
capture: Vec<CaptureIntent>, notify: Vec<NotifyCandidate> }`, where
`AppliedEvent { event: AttentionEvent, change: EventChange }` and
`EventChange` is `Inserted { recurred: bool }`, `Amended`, `Acknowledged`,
`Resolved`, or `Read`. `events` lists only rows that emit (not
`changed: false` amendments).

#### The pass

One pass is one `Storage::write_repo` transaction (SQLite `BEGIN
IMMEDIATE`):

1. Outside the transaction: take `ConnectionManager::status_facts()` and
   run `update_watch`.
2. Inside: read the `FarmView`'s durable part (below), the open Events,
   and `latest_resolved` (the newest resolved Event id per key).
3. `observe` → `plan` → `apply`.
4. Commit, then publish on the `attention` stream, hand `capture` to
   `CameraServices` (never awaited by the projector), and hand `notify` to
   the `NotificationService`.

Reading the open Events inside the write transaction means a command that
resolved or acknowledged an Event a moment earlier is always seen, so a
pass can't undo it. The partial UNIQUE index on the open dedup key is the
backstop: a second concurrent insert of the same key fails the
transaction, which rolls back and is retried once as a full pass.

**The durable part of the `FarmView`** (read in the transaction):

- `printers`: every Printer, with its archive state, Setup facts (the same
  `PrinterSetupFacts` the supervisor uses), Start-safety rule, alert
  defaults (the stored row or the defaults), active Job id, and latest
  Job (`id`, `host_path`).
- `jobs`: every Job that is active, or is some Printer's latest Job, or is
  referenced by an open Event, or is `failed` or `cancelled{hostCancelled}`
  with `ended_at ≥` epoch and no Event (open or resolved) for its
  `job.failed` / `job.hostCancelled` key. Each with its state, cancel
  reason, `ended_by`, `ended_at`, `started` (state `starting`, `printing`,
  `paused`, or `outcomeUnknown`, or `started_at` set), `host_path`, Spool
  id, `last_failure` present, and label.
- `requirements`: every open requirement, plus every requirement
  referenced by an open Event.
- `spools`: every `active` Spool, plus every Spool referenced by an open
  Event, with number, label, lifecycle, `low`, `currentMg`,
  `lowThresholdMg`, and loaded Printer.
- `attention_epoch`.

#### Startup backfill

`attention::projector::backfill(storage, now) -> Result<AppliedChanges,
RepositoryError>` runs once in `build_runtime_services`, right after
`jobs::recover_after_restart` and before `restore_persisted_connections`
(no command is served yet). It is the pass above with an empty status map
and `supervisors_started_at: None`, so every `printer.*` and `job.completed`
entry is `Unknown`: a restart neither opens nor resolves them. Its inserts
have `origin: backfill`, never notify, and never capture. Its changes are
published once the attention runtime starts (as `jobs` publishes its
recovered Jobs).

Running the backfill any number of times, or rebuilding `RuntimeServices`
over the same database, yields the same rows: the second run plans only
`Amend { changed: false }`.

#### Runtime and wakes

`start_attention_runtime(services, app)` runs after `start_jobs_runtime`.
It records `supervisors_started_at = now`, subscribes to every wake
source, **then** spawns the projector task, whose first action is a full
pass. Anything published before it subscribed is covered by that pass.

| Wake | Source |
|---|---|
| Status | `ConnectionManager::subscribe_status()` (Printer id; capacity 256) |
| Queue | `QueueStream::subscribe_changes()`: new, sent inside `QueueStream::publish` after the Tauri emits, carrying `QueueChangeIds { entry_ids, job_ids, requirement_ids }` (capacity 256). All three P7 publish paths (`queue::commands::publish_rows`, `jobs::services::publish`, the evaluator's assignments) go through it |
| Inventory | `RuntimeServices.inventory_changes` (capacity 64) |
| Poke | `AttentionServices::poke()`, called beside every existing `Trigger::PrinterChanged` poke (create, edit, archive, unarchive, delete, import, Connection changes) and by `set_printer_alert_defaults` |
| Deadline | a sleep until `observe::next_deadline(view, now)`: the earliest `max(unreachable_since, started) + grace` still in the future |
| Safety tick | every 60 s |

- `RecvError::Lagged` on any receiver means a full pass (every pass is
  full), as P7's driver treats it.
- **Coalescing.** The task drains every ready wake, then runs one pass,
  and runs passes at most once per `AttentionTimings.pass_min_interval`
  (1 s; tests inject 0). A wake during that interval schedules one
  trailing pass.
- `AttentionTimings { pass_min_interval: 1 s, safety_tick: 60 s }`. Tests
  also get `AttentionServices::pass_barrier()`, which resolves after the
  next completed pass (the P7 driver's barrier pattern).
- `jobs::services::publish` only reaches `QueueStream::publish` once the
  jobs runtime has its `AppHandle`; the safety tick and the first full
  pass cover anything earlier.

Host-operation changes and host-facts broadcasts are **not** wakes: no
Condition reads them, and every Job consequence of a Host Operation
arrives on the queue broadcast.

### D3. Incidents

**Decision: an Incident is opened and linked only by the projector's
transaction (D2 "Apply"). Its timeline is append-only in both code and
database. The Job's own timeline is merged in at read time, never copied.**

- **Kind.** `incidents.kind` is the `ConditionKind` that opened it:
  `printer.hostFailed`, `job.failed`, `job.hostCancelled`, or
  `requirement.jobOutcomeUnknown`. Later links never change it.
- **State.** `open` while any linked actionable Event is open; `closed`
  (with `closed_at`) otherwise. Linking a new actionable Event to a closed
  Incident reopens it (`reopened`, `closed_at` NULL). At most one Incident
  per Job (a partial UNIQUE index on `job_id`). A `printer.hostFailed`
  recurrence opens a new Incident.
- **Revision and publishing.** Every `incident_events` append, and every
  change to the `incidents` row itself (open, close, reopen), bumps
  `incidents.revision` by one in the same transaction. After commit, each
  Incident touched by that transaction is published **once**, as one
  `attention.incident.changed` carrying its final row (final `revision`),
  however many entries the transaction appended. The same holds for the
  projector's pass, every command, the capture path, the `MediaJanitor`,
  and the startup sweep (whose changes are published once the runtime
  starts). An `Incident`'s derived fields (`linkedEventIds`,
  `openLinkedEventCount`, `snapshotCount`) only change through such a
  transaction, so the published row is always current.
- **Printer identity.** `printer_snapshot_json` is a P7 `PrinterSnapshot`
  (`name`, `location`, `catalogRef`, `adapterKind`, `profile`): the Job's
  own `printer_snapshot_json` for a Job Incident, or built from the
  Printer row at open for `printer.hostFailed`. Never an endpoint.
- **Timeline kinds** (`IncidentEntryKind`), each with its `detail`:

  | Kind | `detail` | Written by |
  |---|---|---|
  | `opened` | `{ eventId }` | projector (Incident rule 1–2) |
  | `eventLinked` | `{ eventId }` | projector (rules 2–3) |
  | `reopened` | `{ eventId }` | projector, before `eventLinked` on a closed Incident |
  | `eventAcknowledged` | `{ eventId, by: "operator" \| "system" }` | `acknowledge_attention_event`; projector (deferral) |
  | `eventResolved` | `{ eventId, resolution }` | `resolve_attention_event`; projector; Printer delete |
  | `evidenceCaptured` | `{ snapshotId, trigger }` | capture (D4) |
  | `evidenceSkipped` | `{ reason: "cameraError" \| "diskCap", errorKind: CameraErrorKind \| null }` | capture (D4) |
  | `evidencePruned` | `{ snapshotId, reason: PruneReason }` | `MediaJanitor`, startup sweep |
  | `evidencePinned` | `{ snapshotId }` | `set_snapshot_pinned` |
  | `evidenceUnpinned` | `{ snapshotId }` | `set_snapshot_pinned` |
  | `noteAdded` | `{ text }` | `add_incident_note` |
  | `closed` | `{}` | projector; `resolve_attention_event`; Printer delete |

- **Operator actions on the Job** (settle, defer, declare, correct) are
  not copied. `get_incident` merges the Incident's `incident_events` with
  its Job's `job_events` into one `timeline`, ordered by `at`, then
  incident entries before Job events at the same instant, then sequence.
  A retry is a new Job and a new entry, so it appears on the new Job, not
  here.
- **Notes** are operator text (1–2000 characters after trimming), stored
  verbatim. farm3d never writes a URL, host, or credential into a
  timeline row itself.

### D4. Cameras

**Decision: a camera source is configuration; frames are fetched only by
the backend, through one bounded `FrameFetcher`; health is in memory; and
no camera path can fail, block, or slow any other workflow.**

#### Camera sources

| `CameraSource.kind` | Stored | Resolved at each fetch |
|---|---|---|
| `hostWebcam` | `webcamName` (1–128 chars), `webcamService` (as reported when chosen, informational), `webPort` (1–65535, or null = 80) | Moonraker `GET /server/webcams/list` through the Printer's current Connection, the entry whose `name` equals `webcamName`, its `snapshot_url`. The value is resolved as a URL reference (`Url::join`) against the base `http://<Connection host>:<webPort or 80>/`, so a path-relative, absolute, or scheme-relative (`//other-host/…`) value is handled alike. The **resolved** URL is then checked: scheme `http` (else `hostMismatch`); no userinfo (else `hostMismatch`); its host equal to the Connection host, ASCII case-insensitive (else `hostMismatch`); its fragment dropped. The resolved URL lives in a `Zeroizing<String>` for that fetch only and is never persisted, logged, or returned |
| `snapshotUrl` | `snapshotUrl` | used as stored |

- **Manual URL validation** (`cameras::config::validate_snapshot_url`,
  `VALIDATION` with the field path): scheme exactly `http`; a non-empty
  host; no userinfo (`user@` or `user:pass@`); no fragment; port absent or
  1–65535; total length 8–2048. A query string is allowed (some cameras
  take a token there). The URL, query included, is stored only in
  `printer_cameras.snapshot_url`. Exactly one command returns it:
  `get_printer_camera` (Global Constraint 3's single exception, for the
  Setup editor). `set_printer_camera` returns a redacted
  `PrinterCameraSummary`, and no event carries it. The only other place
  it appears is the Printers export **file** (schema 4), which Rust
  writes to the path the operator picks, exactly as it writes the
  Connection host; `export_printers` itself returns only the outcome
  (path, time, record count), never the document.
- **Host-webcam lookup.** Moonraker gains
  `files::parse_webcam_snapshot_url(body, name) ->
  Result<Option<Zeroizing<String>>, ConnectionError>` (the entry's
  `snapshot_url`, `None` when no entry has that name).
  `parse_webcams_list` and its URL-discarding test stay unchanged. The
  lookup is exposed only inside the crate: `CameraDiscovery` is a public
  trait, so it gets a crate-private sibling trait (`WebcamSnapshotSource`,
  built for Moonraker only) rather than a new method. An adapter without
  it (OctoPrint) gives `unsupportedAdapter` for `hostWebcam`; a
  `snapshotUrl` source still works on any adapter.
- **Credentials.** The webcam-list call uses the Connection's API key,
  like every Moonraker read. The frame fetch itself never sends the API
  key or any other credential: the camera is another service.
- A Printer with no Connection can have a `snapshotUrl` source, but not a
  `hostWebcam` one (`VALIDATION` on `source.kind`).

#### The `FrameFetcher`

One `reqwest` client built like the Moonraker client
(`moonraker/control.rs:140-155`): no TLS backend, `redirect::Policy::none()`,
`retry::never()`, `no_proxy()`.

- `GET` only, with a 5 s connect-plus-total timeout
  (`CameraTimings.fetch`, injectable).
- The body is streamed and cut off after 10 MiB (10 485 760 bytes);
  nothing past the limit is buffered.
- The type is decided by magic bytes, never by `Content-Type`: JPEG starts
  `FF D8 FF`, PNG starts `89 50 4E 47 0D 0A 1A 0A`. Anything else
  (including an MJPEG stream's multipart boundary) is `notAnImage`.
- A 3xx is `httpStatus` (redirects are never followed), as is any non-2xx.

`CameraErrorKind` (every error is typed and carries no URL, host, or
port, in `Display`, `Debug`, or serialization):

| Kind | When |
|---|---|
| `unreachable` | connect refused, DNS failure, reset |
| `timeout` | the 5 s budget ran out |
| `httpStatus` | a non-2xx answer (`httpStatus: u16` beside it) |
| `tooLarge` | more than 10 MiB |
| `notAnImage` | neither JPEG nor PNG magic |
| `noSuchWebcam` | the host lists no webcam with that name |
| `noSnapshotUrl` | the webcam has an empty or missing `snapshot_url` |
| `hostMismatch` | an absolute webcam URL on another host |
| `unsupportedAdapter` | `hostWebcam` on an adapter without the webcam lookup |
| `webcamListFailed` | the webcam-list query itself failed (the Printer is unreachable, or answered badly) |

#### Health and preview

- `CameraServices` keeps one in-memory `CameraHealth` per Printer:
  `state` (`notConfigured`, `unsupported`, `unknown`, `ok`, `failing`),
  `sourceKind`, `lastSuccessAt`, `lastFailureAt`, `lastFailureKind`.
  Every fetch (preview, test of a saved Printer, capture) updates it.
- `camera.health.changed` is published only when `state`, `sourceKind`,
  or `lastFailureKind` changes. Timestamps alone never publish; each
  frame's own header carries its time.
- **At most one fetch per Printer at a time.** A preview request while a
  fetch for that Printer is running waits for it and shares its result. A
  preview request within `CameraTimings.preview_min_interval` (1 s) of the
  last successful preview frame gets that frame again from memory. The
  last preview frame is held in memory only, per Printer, and dropped
  when the Printer's source changes.
- The frontend polls `camera_preview_frame` about once a second only while
  the Camera tab is visible and the dock is open. Nothing polls when no
  one is looking.
- Setting or clearing a Printer's source resets its health to `unknown`
  or `notConfigured` and publishes.

#### Capture triggers

| Trigger | When | Stored | Linked to |
|---|---|---|---|
| `incident` | one frame when the projector opens an Incident (`origin: live`), if the Printer's `snapshotOnIncident` is on | yes | the Incident, and its Job if any |
| `completion` | one frame when the projector inserts `job.completed` (`origin: live`), if `snapshotOnCompletion` is on | yes | the Job |
| `manual` | `capture_snapshot` (the Camera tab's Capture) | yes | the Printer's active Job, if any |
| Setup test | `test_camera` | never | — |
| Live preview | `camera_preview_frame` | never | — |

- **Best-effort.** Capture intents run on `CameraServices`' own task, after
  the projector committed. Nothing waits for them.
- **Recorded outcome.** For an `incident` capture: `evidenceCaptured` on
  success, `evidenceSkipped { reason: "cameraError", errorKind }` on a
  fetch failure, `evidenceSkipped { reason: "diskCap" }` when the cap
  can't be met (D5). For a `completion` capture:
  `attention::repository::record_evidence` writes the same outcome to
  the `job.completed` Event's `evidence` field (D2 "Condition detail and
  subject"), never to its planner-owned `detail`. P8 adds no kinds to
  P7's `job_events`.
- **Nothing is recorded** when the toggle is off or the Printer has no
  camera source: no capture was expected. `evidenceSkipped` always means
  farm3d tried and could not.

#### Optionality

Global Constraint 5 holds structurally: no monitoring, slicing,
assignment, or Job path calls into `cameras`; the projector hands capture
intents off without awaiting them; and every camera command has its own
timeout. Task 8's four tests (a camera hanging for 30 s while
`printer_statuses`, a slice start, `assign_queue_entry`, and `start_job`
each complete within their normal bounds) prove it.

### D5. The media store and retention

**Decision: images live in a farm3d-owned media root, rows are written
before files are removed, one janitor lock serializes every capture and
prune, and a startup sweep repairs whatever a crash left.**

- **Root.** `StoragePaths` gains `media_root()` =
  `<app data>/farm3d-media/v1`, created and checked like `content_root`
  (contained, not overlapping any other tree). Images are at
  `snapshots/<yyyy>/<mm>/<snp-id>.<jpg|png>` (UTC capture time);
  in-progress writes go to `tmp/`. `rel_path` is stored relative to the
  root and never sent to the frontend.
- **Capture order.** Fetch into memory (≤ 10 MiB) → take the janitor lock
  → `plan_prune` for the incoming size → write `tmp/<snp-id>.part`, fsync,
  rename into place → one transaction: mark the planned rows pruned,
  insert the new row, write the timeline row or Event amendment (and, for
  `capture_snapshot`, claim the `operationId`) → commit → release the lock
  → unlink the pruned files. A failure before commit unlinks the renamed
  file.
- **Pruning.** `plan_prune(rows, retention, now, incoming_bytes) ->
  PrunePlan { prune: Vec<PruneAction { id, reason }>, fits: bool }` is pure:
  1. every unpinned, unpruned row older than `retentionDays` → `age`;
  2. then, while `used − pruned + incoming > cap`, the oldest remaining
     unpinned row (by `captured_at`, then `id`) → `diskCap`;
  3. `fits` is whether usage now fits. When it doesn't (only pinned rows
     are left), the `diskCap` actions are dropped (evidence is never
     removed for nothing), the `age` actions stay, and a capture is
     skipped (`evidenceSkipped { reason: "diskCap" }`, or
     `SNAPSHOT_DISK_CAP` for `capture_snapshot`).
- **Usage** is `SUM(byte_len)` over rows with `pruned_at IS NULL`, pinned
  or not. Never a filesystem walk.
- **Pinned** rows count toward usage and are never pruned `age` or
  `diskCap`.
- **A pruned row stays.** `pruned_at` and `prune_reason` are set; the row,
  its links, and every timeline row that names it remain. An
  Incident-linked prune writes `evidencePruned`; a pin or unpin writes
  `evidencePinned` / `evidenceUnpinned`.
- **Pinning a pruned row.** `set_snapshot_pinned(pinned: true)` on a
  pruned row (any reason) fails `EVIDENCE_PRUNED`. `set_snapshot_pinned
  (pinned: false)` on a pruned row is allowed (unpinning is harmless): it
  clears `pinned_at`, bumps `revision`, writes `evidenceUnpinned` if
  linked, and emits. Unpinning an already-unpinned row is a no-op.
- **`MediaJanitor`.** One tokio task and one `tokio::sync::Mutex` shared
  with every capture. Its prune pass (`plan_prune` with `incoming_bytes =
  0`, marked in one transaction, unlinked after commit) runs at startup
  after the sweep, hourly (`CameraTimings.janitor_every`), and when
  `save_settings` changes the retention settings.
- **Startup sweep** (in `build_runtime_services`, before any command is
  served, like the content store's): delete everything in `tmp/`; delete
  every image file with no **unpruned** row (an orphan from a crash after
  rename, or a pruned file whose unlink never ran); mark every unpruned
  row whose file is missing `pruned` with `missingFile` (plus
  `evidencePruned` if linked). A pinned row whose file is missing is also
  marked `missingFile`: the image is gone either way.
- **Printer delete** removes the unpinned, unattached manual rows of that
  Printer in its transaction and unlinks their files after commit. A crash
  in between leaves orphan files that the sweep deletes. No cleanup table
  is needed.

### D6. Notifications (ADR-0015)

**Decision: a Rust `NotificationService` decides purely whether to notify,
sends through a platform sink, and on a click raises the window and
navigates. Linux uses farm3d's own `org.freedesktop.Notifications`
client over zbus; other platforms use a null sink.**

#### Policy

`notifications::policy::decide(candidate: &NotifyCandidate, classes:
&NotificationClassSettings, alerts: &AlertDefaults, focused: bool,
limiter: &mut RateLimiter, now) -> Option<Notification>` is pure. It
returns a notification only when all hold:

1. the Event was inserted (`Inserted`, new or recurred) with `origin:
   live` (amendments, acknowledgements, resolutions, and backfill never
   notify);
2. its class is enabled in settings;
3. its Printer's `notifications` alert default is `follow` (Job and
   requirement Events use their Job's Printer; `spool.low` has no Printer
   and is never muted);
4. the window is not focused;
5. the rate limiter allows it.

**Rate limiter.**

- **Per key.** A dedup key notifies at most once per 10 minutes
  (`last_shown` per key). A suppressed candidate is dropped and not
  counted.
- **Burst.** If 3 notifications were shown in the last 10 s, the next
  candidate is not shown by itself: it joins a **summary** notification
  ("N new Attention Events"), which opens the Attention center. The first
  summary in a window is a new notification; later candidates in the same
  10 s window replace it in place (freedesktop `replaces_id`) with the new
  count. A candidate folded into a summary counts as shown for the
  per-key rule.

**Content.** `summary` (the freedesktop title) is the Condition's label
("Printer offline", "Job failed", …). `body` is the severity word, a
colon, and the Event's `summary`: "Fatal: Cube — Plate 1 failed on
Voron." The body never depends on the icon, and never contains a host,
URL, camera text, or credential. When the server advertises
`body-markup`, `&`, `<`, and `>` are escaped.

**Classes and defaults** (settings columns, D9):

| Class | Conditions | Default |
|---|---|---|
| `fatal` | `printer.hostFailed`, `job.failed`, `requirement.jobOutcomeUnknown` | on |
| `confirmation` | `job.startConfirmation` | on |
| `completion` | `job.completed` | on |
| `reconciliation` | `requirement.materialReconciliation` | off |
| `connectivity` | `printer.offline`, `printer.connectionError`, `job.hostCancelled` | off |
| `inventory` | `spool.low` | off |

#### Focus

`Focus` is an `AtomicBool` fed by the main window's
`on_window_event(WindowEvent::Focused(f))`, never by `is_focused()`
(tauri#11323). It starts `true`, so nothing notifies before the first
focus-out.

Task 2 observations (KDE Plasma 6.7.5 Wayland; baseline
`docs/superpowers/baselines/2026-09-27-p8-notification-spike.md`):

- `Focused(false)` fires on `minimize` and `hide`, and `Focused(true)` on
  `show` and on `set_focus` of a visible window: confirmed (Task 2). On
  alt-tab and around a GTK file dialog: deferred to Task 17.
- `is_focused()` agreed with the events in every step of both runs. The
  tauri#11323 mismatch did not reproduce, but the event stays the source.
- Contradicted (Task 2): "nothing notifies before the first focus-out"
  assumes the window opens focused. In one of two runs KWin opened it
  unfocused and sent no `Focused(false)`. Starting `true` then keeps
  notifications off until the operator focuses and leaves the window once.
  That errs on the quiet side, and the Attention center still has every
  Event. Seeding from `is_focused()` once the window is shown would close
  the gap; the controller decides before Task 9.

#### Sinks

```rust
#[async_trait]
pub trait NotificationSink: Send + Sync {
    async fn show(&self, notification: &Notification) -> Result<NotificationHandle, NotifyError>;
    fn status(&self) -> NotifierStatus;
}
```

| Sink | Where | Behavior |
|---|---|---|
| `DbusNotificationSink` | `#[cfg(target_os = "linux")]` | one long-lived zbus session connection; `Notify` as below; signal listeners |
| `NullNotificationSink` | every other target | `show` fails `Unsupported`; `status` is `unsupported` |
| `RecordingSink` | tests | records every notification; tests drive clicks and closes |

**`Notify` call.** `app_name` "farm3d"; `replaces_id` 0, or the current
summary's id; `app_icon` the icon name `farm3d` when an icon theme under
the XDG data dirs has `hicolor/*/apps/farm3d.png` (the deb installs it),
otherwise the absolute path of the bundled icon from the resource dir
(`just dev`, AppImage); `actions` `["default", "Open"]`; hints
`desktop-entry` = "farm3d" and `urgency` (`fatal` 2, `warning` 1, `info`
0); `expire_timeout` −1.

**Permissions.** The freedesktop protocol has no permission model, and
farm3d asks for none. A reachable `org.freedesktop.Notifications` is
available; Do Not Disturb is the daemon's business, and the Attention
center keeps every Event regardless. `tauri-plugin-notification` is not
added. At startup the sink calls `GetServerInformation` and
`GetCapabilities`; `notification_status` reports the result. Confirmed
(Task 2): Plasma answers `("Plasma", "KDE", "6.7.5", "1.2")` and
advertises `actions`, `body-markup`, `persistence`, and `icon-static`, so
the body escaping above applies there. It also declares the
`ActivationToken` signal in its introspection data.

```ts
type NotifierStatus =
  | { state: "available"; serverName: string; serverVendor: string; serverVersion: string;
      specVersion: string; actions: boolean; bodyMarkup: boolean }
  | { state: "unavailable"; reason: "noSessionBus" | "noNotificationServer" | "callFailed" }
  | { state: "unsupported" };
```

A missing session bus or server is `unavailable`, never a panic, and the
service retries the connection on the next `notification_status` or
`send_test_notification`.

#### Click activation

Task 2 ran the automatable parts on KDE Plasma 6.7.5 Wayland (KWin
6.7.5, GTK 3.24.52); anything that needs a click waits for Task 17 (owner
at the desktop). Each item below is marked "confirmed (Task 2)",
"contradicted (Task 2)", or "deferred to Task 17". Task 17's pass may
still change steps 2–3, and the spec's Status line will record the
confirmed sequence.

**The `Notify` call** above: accepted as specified (`app_name` "farm3d",
an absolute icon path, `["default", "Open"]`, `desktop-entry`, a byte
`urgency`, `expire_timeout` −1); it returns an id. Confirmed (Task 2).
Whether Plasma attributes it to farm3d with farm3d's icon: deferred to
Task 17 (the spike host has no installed `farm3d.desktop`, so under
`just dev` the hint cannot resolve there).

1. The service keeps `outstanding: id → OutstandingNotification { target,
   eventId: Option<String>, openAttentionCenter }`, bounded at 256 (oldest
   evicted first), and a short-lived `activation_tokens: id → token`
   (dropped after 10 s). The bound is needed, confirmed (Task 2): Plasma
   sent no `NotificationClosed` for a notification whose 2 s
   `expire_timeout` had passed (3.5 s later), so an untouched id can stay
   outstanding.
2. `ActivationToken(id, token)` for a known id stores the token. The
   server declares the signal: confirmed (Task 2). Whether KWin sends it,
   and before `ActionInvoked`: deferred to Task 17.
3. `ActionInvoked(id, "default" | "Open")` for a known id, on the main
   thread (`AppHandle::run_on_main_thread`): if a token arrived, apply it
   to the GTK window (`set_startup_id(token)`) first; then `unminimize`,
   `show`, `set_focus`. If no `Focused(true)` arrives within 500 ms,
   `request_user_attention(Informational)`. Receiving `ActionInvoked` at
   all, and whether the token raises the window (minimized, or behind
   another app): deferred to Task 17.

   Contradicted (Task 2): without a valid token, `unminimize` and
   `set_focus` do not bring back a minimized window on KWin Wayland. GTK
   has no un-minimize request on Wayland, and `is_minimized()` stays
   `false` throughout, so tao's `set_focus` guard never sees the window as
   minimized (the "`set_focus` right after `unminimize` is dropped" case
   applies to X11 only). An invalid token changed nothing. What did
   work: a `hide` and then a `show` brought a minimized window back
   focused in both runs; in the second run `hide`, `show`, `set_focus` in
   one tick did so twice, with `Focused(true)` within 4 ms; and `set_focus`
   alone activated a visible window that had opened unfocused. The named
   fallback (Task 2's proposal, approved or not with its baseline report):
   if no `Focused(true)` within 500 ms, run `hide`, `show`, `set_focus` in
   one tick and wait another 500 ms (Task 17 checks that the re-mapped
   window keeps its place and contents); only then
   `request_user_attention(Informational)`. Tao maps that call to GTK's
   urgency hint, which may be a no-op on Wayland (Task 17 looks); step 4
   still navigates, and the Attention center keeps the Event, whatever
   the raise does.
4. Emit `farm3d-navigate-v1` with `NavigateRequest { contractVersion: 1,
   target, openAttentionCenter }`.
5. If the notification names one Event: mark it read through
   `attention::repository::mark_read` (the function
   `mark_attention_read` uses, without an `operationId`), and publish the
   change.

- **The target.** Decision 8's table, via
  `attention::deep_link::target_for(event, source_exists)`, evaluated at
  click time: the source's target when the source row still exists,
  otherwise `monitor/attention/<eventId>`. A summary's target is
  `{ destination: "monitor" }` with `openAttentionCenter: true`.
- `NotificationClosed(id, reason)` removes `id` from `outstanding`.
  Confirmed (Task 2): `CloseNotification` produced `NotificationClosed(id,
  3)` on the long-lived listener.
- Signals are matched on path `/org/freedesktop/Notifications`, interface
  `org.freedesktop.Notifications`, and sender = the current unique owner
  of `org.freedesktop.Notifications`. Ids not in `outstanding` are
  ignored. Confirmed (Task 2): a `NotificationClosed` forged by another
  connection reached an unfiltered match but not the sender-filtered one;
  Plasma broadcasts its signals (no destination), so another client's
  `NotificationClosed` did reach the filtered listener and was dropped by
  the id check.
- **The summary's `replaces_id`.** Confirmed (Task 2): `Notify` with
  `replaces_id` set to a live id returns that same id, updates in place,
  and emits no `NotificationClosed` for it.
- `send_test_notification` shows one notification whatever the focus and
  classes ("farm3d test notification"), with target `{ destination:
  "monitor" }`.

The candidate channel from the projector to the service is an `mpsc` of
256. A full channel drops the candidate (the Event is still in the
center). After `show` succeeds, the service writes `notified_at` on each
Event it covered (no `revision` bump, no event).

### D7. Commands and events

**Decision: 22 new commands and one new event stream. Every mutating
command takes an `operationId`, claimed through `spools::operations::claim`
in its transaction. Payloads are in "Backend model".**

| Group | Commands |
|---|---|
| Attention (4) | `list_attention`, `mark_attention_read`, `acknowledge_attention_event`, `resolve_attention_event` |
| Incidents (3) | `list_incidents`, `get_incident`, `add_incident_note` |
| Cameras (7) | `get_printer_camera`, `set_printer_camera`, `clear_printer_camera`, `list_host_webcams`, `test_camera` (binary), `camera_preview_frame` (binary), `capture_snapshot` |
| Snapshots (4) | `list_snapshots`, `snapshot_image` (binary), `set_snapshot_pinned`, `media_usage` |
| Alerts and notifications (4) | `get_printer_alert_defaults`, `set_printer_alert_defaults`, `notification_status`, `send_test_notification` |

Changed existing commands: `save_settings` and `load_settings` (the new
settings), `export_settings` / `import_settings` (schema 3),
`create_printer` and `create_printers_batch` (camera and alert defaults),
`export_printers` / `import_printers` (schema 4, the evidence guard),
`delete_printer` (D8).

Events: the `attention` stream on `farm3d-event-v1`
(`attention.event.changed`, `attention.incident.changed`,
`attention.snapshot.changed`, `camera.health.changed`), and the separate,
unsequenced `farm3d-navigate-v1`.

### D8. Lifecycle guards

Every guard runs inside the mutation's own transaction.

| Mutation | Blocked when | Result |
|---|---|---|
| Printer delete | any Incident references the Printer | `LIFECYCLE_BLOCKED`, blocker `INCIDENT_HISTORY_EXISTS`: "This Printer has Incident history. Archive it instead." |
| Printer delete | a pinned, **unpruned** `manual` snapshot of the Printer has no Incident and no Job. A pruned row (any `pruneReason`, including a pinned `missingFile` one) never counts: it has no image left to protect | `LIFECYCLE_BLOCKED`, blocker `PINNED_EVIDENCE_EXISTS`: "This Printer has pinned camera evidence. Unpin it or archive the Printer instead." |
| Printer import (`replace_all`) | any Incident or any `camera_snapshots` row exists | `EVIDENCE_EXISTS` for the whole import, nothing written |
| Printer archive | nothing new | archive proceeds; the next pass resolves `printer.*` and `job.completed` `conditionCleared` |

- `IncidentBlockers` (`incidents::guards`) joins `blocker_sources()` after
  `JobBlockers`. `LifecycleBlockerCode` gains `INCIDENT_HISTORY_EXISTS`
  and `PINNED_EVIDENCE_EXISTS`.
- **Every other snapshot is covered already:** an `incident` snapshot by
  `INCIDENT_HISTORY_EXISTS`; a `completion` snapshot, or a `manual` one
  linked to a Job, by P7's `JOB_HISTORY_EXISTS`. So a delete that passes
  the guards leaves no evidence that references the Printer except the
  unattached manual rows it removes.
- **`PrinterRepository::delete`**, in its existing transaction, after the
  blocker check and before `DELETE FROM printers`:
  1. resolve every open Event with `printer_id` = the Printer as
     `sourceRemoved` (with `eventResolved` / `closed` rows if linked — in
     practice none can be, since an Incident blocks the delete);
  2. delete the Printer's `manual` snapshot rows with no Incident and no
     Job that are unpinned **or** pruned (pinned-and-unpruned ones have
     already blocked the delete), collecting the `rel_path`s of the
     unpruned ones;
  3. `DELETE FROM printers`: `attention_events.printer_id` becomes NULL
     (`ON DELETE SET NULL`), and `printer_cameras` and
  `printer_alert_defaults` rows cascade;
  4. after commit: unlink the collected files, publish the resolved
     Events, and poke the projector.
- **`PrinterRepository::replace_all`** calls
  `incidents::guards::check_import` beside P7's `check_import`, and
  resolves the open Events of every replaced Printer `sourceRemoved`
  before its `DELETE FROM printers`. Camera and alert rows cascade and are
  re-inserted from the imported document (schema 4).
- **Deep links after delete.** An Event keeps `source.kind`, `source.id`,
  and `subject`. With the source gone it opens `monitor/attention/<id>`,
  whose detail shows the subject with "Printer deleted".
- **Archived Printers** stay valid `monitor/printer/<id>` targets, keep
  every Incident and Event, and their Incidents stay openable.

### D9. Schema

Migration `0009_p8_attention.sql` (full SQL in "Backend model").
`CURRENT_SCHEMA_VERSION` becomes 9. The migration is one transaction.

## Condition fixtures

Task 4 copies this section verbatim as table-driven tests in
`src-tauri/tests/p8_observe_plan.rs`: one `#[test]` per fixture id, named
`fixture_<id>` with `-` as `_` (for example `fixture_c1_p_n`).

### Baseline world

Every fixture starts from this world and changes only what its row says.

- `NOW` = `2026-09-27T12:00:00Z`. `STARTED` (`supervisors_started_at`) =
  `2026-09-27T11:00:00Z`. `EPOCH` (attention epoch) =
  `2026-09-27T10:00:00Z`.
- Printer `prn-1`: name "Voron", location "Bay A", not archived, Setup
  complete, `startSafety: confirmBedClear`, alert defaults
  `{ offlineAfterMinutes: 5, notifications: "follow", snapshotOnIncident:
  true, snapshotOnCompletion: true }`. Status: `online`, no error cause,
  `operationalState: ready`, `freshness: fresh`, reported file none.
  `unreachable_since: None`. No active Job; latest Job none.
- When a row names `job-1`: on `prn-1`, `host_path: "cube.gcode"`, Spool
  `spl-1`, label "Cube — Plate 1", `lastFailure` none, and when terminal
  `ended_at: 2026-09-27T11:30:00Z`. It is `prn-1`'s latest Job, and its
  active Job while non-terminal.
- When a row names `rrq-1`: a requirement of `job-1`, with `spoolId:
  spl-1` for `materialReconciliation`.
- Spool `spl-1`: number 12, "Polymaker PLA", `active`, `currentMg: 500000`,
  `lowThresholdMg: 100000`, loaded on `prn-1`.
- **Priors.** `N`: no Event exists for the key. `O`: an open Event
  `att-open` exists for the key, unread and unacknowledged, with the
  catalogue's severity and the same `detail` the row's `P` observation
  produces. `R`: no open Event; `att-prev` is the key's latest resolved
  Event.

"Insert" means `Insert { condition, recurrenceOf: null, acknowledged:
false }` with the catalogue values; "Insert↻" means the same with
`recurrenceOf: att-prev`; "Amend=" means `Amend { att-open, changed:
false }`; "—" means no action.

### Catalogue fixtures (30 observations × 3 priors = 90)

| Id | Condition | Obs. | Facts (changes from baseline) | `N` | `O` | `R` |
|---|---|---|---|---|---|---|
| c1-p | `printer.offline` | Present | status `offline`, `unreachable_since` 11:50:00 | Insert | Amend= | Insert↻ |
| c1-a | `printer.offline` | Absent | baseline (`online`) | — | Resolve `conditionCleared` | — |
| c1-u | `printer.offline` | Unknown | status `offline`, `unreachable_since` 11:57:00 (3 min < 5 min) | — | — | — |
| c2-p | `printer.connectionError` | Present | status `error`, cause `auth` | Insert | Amend= | Insert↻ |
| c2-a | `printer.connectionError` | Absent | baseline | — | Resolve `conditionCleared` | — |
| c2-u | `printer.connectionError` | Unknown | no status for `prn-1` | — | — | — |
| c3-p | `printer.hostFailed` | Present | `operationalState: failed` (fresh), no Jobs | Insert, opens Incident | Amend= | Insert↻, opens Incident |
| c3-a | `printer.hostFailed` | Absent | baseline (`ready`) | — | Resolve `conditionCleared` | — |
| c3-u | `printer.hostFailed` | Unknown | status `offline`, `unreachable_since` 11:59:00 | — | — | — |
| c4-p | `job.startConfirmation` | Present | `job-1` `awaitingStart` | Insert (`awaitingMaterial: false`) | Amend= | Insert↻ |
| c4-a | `job.startConfirmation` | Absent | `job-1` `starting` | — | Resolve `actionCompleted` | — |
| c4-u | `job.startConfirmation` | Unknown | `job-1` not in the view | — | — | — |
| c5-p | `job.failed` | Present | `job-1` `failed`, ended by `failed` | Insert, opens Incident | Amend= | — |
| c5-a | `job.failed` | Absent | `job-1` `failed`, ended by `declaredFailed` | — | — | — |
| c5-u | `job.failed` | Unknown | `job-1` not in the view | — | — | — |
| c6-p | `job.hostCancelled` | Present | `job-1` `cancelled`, `hostCancelled` | Insert, opens Incident | Amend= | — |
| c6-a | `job.hostCancelled` | Absent | `job-1` `cancelled`, `cancelledByOperator` | — | — | — |
| c6-u | `job.hostCancelled` | Unknown | `job-1` not in the view | — | — | — |
| c7-p | `requirement.materialReconciliation` | Present | `job-1` `failed` (by `declaredFailed`), `rrq-1` material `pending` | Insert | Amend= | — |
| c7-a | `requirement.materialReconciliation` | Absent | `rrq-1` `resolved` | — | Resolve `actionCompleted` | — |
| c7-u | `requirement.materialReconciliation` | Unknown | `rrq-1` not in the view | — | — | — |
| c8-p | `requirement.jobOutcomeUnknown` | Present | `job-1` `outcomeUnknown`, `rrq-1` outcome `pending` | Insert, opens Incident | Amend= | — |
| c8-a | `requirement.jobOutcomeUnknown` | Absent | `rrq-1` `resolved` | — | Resolve `actionCompleted` | — |
| c8-u | `requirement.jobOutcomeUnknown` | Unknown | `rrq-1` not in the view | — | — | — |
| c9-p | `spool.low` | Present | `spl-1` `currentMg: 80000` | Insert | Amend= | Insert↻ |
| c9-a | `spool.low` | Absent | baseline (`500000`) | — | Resolve `conditionCleared` | — |
| c9-u | `spool.low` | Unknown | `spl-1` not in the view | — | — | — |
| c10-p | `job.completed` | Present | `job-1` `completed` by `completed`; `prn-1` `finished`, reported file `cube.gcode` | Insert | Amend= | — |
| c10-a | `job.completed` | Absent | as c10-p, but `prn-1` `ready` | — | Resolve `conditionCleared` | — |
| c10-u | `job.completed` | Unknown | as c10-p, but `prn-1` status `offline`, `unreachable_since` 11:59:00 | — | — | — |

Notes that bind the fixtures:

- c5-a and c6-a are `manual`/once-per-source, so even an open Event is not
  resolved by observation.
- c7-p uses a declared failure so that `job.failed` stays absent and the
  fixture isolates the requirement.
- "Opens Incident" is asserted on `apply` (Task 6), not on `plan`; the
  `plan` fixture asserts the `Insert` only.
- In each row, only the named Condition's key is asserted; the other keys
  the view produces follow their own rows.

### Supplementary fixtures

| Id | Facts (changes from baseline) | Prior | Expected |
|---|---|---|---|
| s1 | `STARTED` 11:58:00; `prn-1` `offline`, `unreachable_since` 11:50:00 (before startup) | `N` | — (`max(11:50, 11:58) + 5 min` = 12:03 > NOW) |
| s2 | backfill (`STARTED` none), no status map | `O` for `printer.offline` | — |
| s3 | `prn-1` `error`, cause `timeout`, `unreachable_since` 11:50:00 | `N` | `printer.offline` Insert; `printer.connectionError` — |
| s4 | `prn-1` archived, `offline`, `unreachable_since` 11:00:00 | `O` for `printer.offline` | Resolve `conditionCleared` |
| s5 | `prn-1` Setup incomplete | `O` for `printer.connectionError` | Resolve `conditionCleared` |
| s6 | `offlineAfterMinutes: null` (off), `prn-1` `offline`, `unreachable_since` 11:00:00 | `O` / `N` | Resolve `conditionCleared` / — |
| s7 | `job-1` `printing` (started); `prn-1` `failed`, reported file `cube.gcode` | `N` / `O` for `printer.hostFailed` | — / Resolve `conditionCleared` (covered) |
| s8 | `job-1` `failed` by `failed`; `prn-1` `failed`, reported file `cube.gcode` | `N` for `printer.hostFailed` | — (covered by `job-1`); `job.failed` Insert |
| s9 | `rrq-1` material `deferred` | `N` | Insert with `acknowledged: true` |
| s10 | `rrq-1` material `deferred` | `O` (unacknowledged, detail `pending`) | Amend `changed: true`, then Acknowledge |
| s11 | `rrq-1` material `deferred` | `O` (acknowledged, detail `deferred`) | Amend= |
| s12 | `spl-1` `currentMg: 80000` | `O` with detail `currentMg: 90000` | Amend `changed: true` |
| s13 | `job-1` `failed` by `failed`, `ended_at` 09:00:00 (before EPOCH) | `N` | — |
| s14 | `job-1` `awaitingStart`; `prn-1` `startSafety: unattended`, no `lastFailure` | `N` / `O` | — / Resolve `actionCompleted` |
| s15 | as s14, with `lastFailure` set | `N` | Insert |
| s16 | `job-1` `awaitingStart`; `spl-1` in storage | `N` / `O` (detail `awaitingMaterial: false`) | Insert (`awaitingMaterial: true`) / Amend `changed: true` |
| s17 | as c10-p, plus a newer `job-2` on `prn-1` (`assigned`) | `N` / `O` for `job-1` | — / Resolve `conditionCleared` |
| s18 | as c10-p, but ended by `declaredCompleted` | `N` | — |
| s19 | as c10-p, but `prn-1` archived | `O` | Resolve `conditionCleared` |
| s20 | `prn-1` `error`, no recorded cause | `O` for `printer.connectionError` | — |
| s21 | an open `printer.offline` Event for `prn-9`, which the view doesn't contain | `O` | — |
| s22 | the c1-p world, planned, applied, and planned again | — | only Amend= actions |
| s23 | `prn-1` `operationalState: unknown` (stale), `online` | `O` for `printer.hostFailed` | — |
| s24 | `job-1` `awaitingStart`, a second resolved Event `att-older` and `att-prev` (newer) for the key | `R` | Insert with `recurrenceOf: att-prev` |
| s25 | `prn-1` `connecting` (live, after a reconnect) | `O` for `printer.connectionError` (cause `auth`) | — (reach `Unreachable` is `Unknown` for `printer.connectionError`) |
| s26 | `prn-1` `offline`, hydrated from the cache (the startup status before any live observation), `STARTED` 11:59:59 | `O` for `printer.connectionError` (cause `auth`) | — |
| s27 | the c10-p world | `O` for `job.completed`, with `evidence: { status: "captured", snapshotId: "snp-1" }` recorded | Amend= (`changed: false`); the applied row still has that `evidence` |
| s28 | `job-1` `assigned` (`host_path` none); `prn-1` `failed`, reported file `other.gcode` | `N` for `printer.hostFailed` | Insert (not covered: `job-1` never started); `apply` opens an Incident with `job_id` NULL |
| s29 | `job-1` `awaitingStart` (`host_path` `cube.gcode`, not started); `prn-1` `failed`, reported file `cube.gcode` | `N` for `printer.hostFailed` | Insert, opens an Incident (a staged but unstarted Job never covers a failure) |

`next_deadline` fixtures (Task 4, same file):

| Id | Facts | Expected |
|---|---|---|
| d1 | `prn-1` `offline`, `unreachable_since` 11:58:00 | `12:03:00` |
| d2 | `STARTED` 11:59:00, `unreachable_since` 11:58:00 | `12:04:00` |
| d3 | offline alerts off | `None` |
| d4 | `unreachable_since` 11:50:00 (already past) | `None` |
| d5 | two Printers, deadlines 12:03 and 12:01 | `12:01:00` |

## Backend model

### Module layout

| File | Content |
|---|---|
| `attention/mod.rs` | wire types: `AttentionEvent`, `ConditionKind`, `AttentionSeverity`, `ResolutionMode`, `AttentionResolution`, `AttentionOrigin`, `AttentionSourceKind`, `AttentionSource`, `AttentionSubject`, `AttentionDetail`, `EvidenceOutcome`, `NotificationClass`, `AttentionAction` (wire enum of allowed actions), `AttentionChange`, `AttentionBackfill`, `AttentionCursor`; Rust-only `Condition` and the catalogue (`ConditionKind::spec()`) |
| `attention/lifecycle.rs` | `apply`, `LifecycleOp`, `LifecycleChange`, `LifecycleError`, `AckBy` |
| `attention/observe.rs` | `FarmView` and its facts, `Reach`, `reach`, `PrinterWatch`, `update_watch`, `observe`, `next_deadline`, `ObservedConditions`, `Observation` |
| `attention/plan.rs` | `plan`, `PlannedAction` |
| `attention/repository.rs` | insert, amend, acknowledge, resolve, `mark_read`, `mark_notified`, `open_events`, `latest_resolved_by_key`, `list_attention`, `resolve_for_printer` (delete/import) |
| `attention/projector.rs` | the pass, `apply`, `backfill`, `AppliedChanges` |
| `attention/deep_link.rs` | `target_for` |
| `attention/services.rs` | `AttentionServices` (stream, poke, timings, barrier), the projector task |
| `attention/events.rs` | `AttentionStream`, `AttentionStreamEventType`, `AttentionStreamPayload`, `AttentionStreamEvent` |
| `attention/commands.rs` | the four Attention commands |
| `incidents/mod.rs` | `Incident`, `IncidentState`, `IncidentEntry`, `IncidentEntryKind`, `IncidentEntryDetail`, `IncidentTimelineItem`, `IncidentDetail`, `IncidentPage` |
| `incidents/repository.rs` | open, link, reopen, `append_entry`, `close_if_settled`, `for_job`, list, get |
| `incidents/guards.rs` | `IncidentBlockers`, `check_import` |
| `incidents/commands.rs` | the three Incident commands |
| `cameras/mod.rs` | `CameraSource`, `CameraSourceKind`, `CameraSourceInput`, `PrinterCamera`, `CameraHealth`, `CameraHealthState`, `CameraErrorKind`, `CameraSnapshot`, `SnapshotTrigger`, `PruneReason`, `MediaUsage`, `FrameHeader`, `HostWebcam` |
| `cameras/config.rs` | `printer_cameras` repository, URL validation |
| `cameras/resolve.rs` | source → `Zeroizing<String>` URL for one fetch |
| `cameras/fetch.rs` | `FrameFetcher`, `Frame`, `CameraError` |
| `cameras/media.rs` | the media root, the capture write, the startup sweep |
| `cameras/retention.rs` | `plan_prune`, `PrunePlan`, `MediaJanitor` |
| `cameras/capture.rs` | `CaptureIntent` handling |
| `cameras/services.rs` | `CameraServices` (health, in-flight fetches, last preview frame, janitor), `CameraTimings` |
| `cameras/commands.rs` | the seven camera and four snapshot commands |
| `notifications/mod.rs` | `NotificationSink`, `Notification`, `NotificationHandle`, `NotifierStatus`, `NotifyError`, `NavigateRequest`, `NAVIGATE_EVENT` |
| `notifications/policy.rs` | `decide`, `RateLimiter`, `NotifyCandidate` |
| `notifications/focus.rs` | `Focus` |
| `notifications/dbus.rs` | `DbusNotificationSink` (`#[cfg(target_os = "linux")]`) |
| `notifications/null.rs` | `NullNotificationSink` |
| `notifications/activation.rs` | raise the window, emit `farm3d-navigate-v1`, mark read |
| `notifications/services.rs` | `NotificationService` (candidate channel, outstanding map, tokens) |
| `notifications/commands.rs` | `notification_status`, `send_test_notification` |
| `printers/alerts.rs` | `AlertDefaults`, `PrinterAlertDefaults`, `OfflineAlertMinutes`, `NotificationMode`, the repository; `get`/`set` commands |

`RuntimeServices` gains `attention: Arc<AttentionServices<R>>`,
`cameras: Arc<CameraServices<R>>`, and `notifications:
Arc<NotificationService<R>>`.

### Schema (migration `0009_p8_attention.sql`)

```sql
CREATE TABLE incidents (
  id TEXT PRIMARY KEY CHECK (id GLOB 'inc-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  kind TEXT NOT NULL CHECK (kind IN ('printer.hostFailed','job.failed','job.hostCancelled',
                                     'requirement.jobOutcomeUnknown')),
  printer_id TEXT NOT NULL REFERENCES printers(id) ON DELETE RESTRICT,
  job_id TEXT REFERENCES jobs(id) ON DELETE RESTRICT,
  printer_snapshot_json TEXT NOT NULL CHECK (json_valid(printer_snapshot_json)),
  opened_at TEXT NOT NULL,
  closed_at TEXT,
  CHECK ((kind = 'printer.hostFailed') = (job_id IS NULL))
) STRICT;
CREATE UNIQUE INDEX incidents_one_per_job ON incidents(job_id) WHERE job_id IS NOT NULL;
CREATE INDEX incidents_printer ON incidents(printer_id, opened_at);
CREATE INDEX incidents_open ON incidents(opened_at) WHERE closed_at IS NULL;

CREATE TABLE attention_events (
  id TEXT PRIMARY KEY CHECK (id GLOB 'att-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  dedup_key TEXT NOT NULL CHECK (length(dedup_key) BETWEEN 5 AND 700),
  condition TEXT NOT NULL CHECK (condition IN ('printer.offline','printer.connectionError',
    'printer.hostFailed','job.startConfirmation','job.failed','job.hostCancelled',
    'requirement.materialReconciliation','requirement.jobOutcomeUnknown','spool.low',
    'job.completed')),
  severity TEXT NOT NULL CHECK (severity IN ('fatal','warning','info')),
  requires_action INTEGER NOT NULL CHECK (requires_action IN (0,1)),
  resolution_mode TEXT NOT NULL CHECK (resolution_mode IN ('auto','action','manual')),
  notification_class TEXT NOT NULL CHECK (notification_class IN ('fatal','confirmation',
    'completion','reconciliation','connectivity','inventory')),
  source_kind TEXT NOT NULL CHECK (source_kind IN ('printer','job','reconciliationRequirement','spool')),
  source_id TEXT NOT NULL CHECK (length(CAST(source_id AS BLOB)) BETWEEN 1 AND 512),
  printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL,
  job_id TEXT REFERENCES jobs(id) ON DELETE RESTRICT,
  spool_id TEXT REFERENCES spools(id) ON DELETE RESTRICT,
  requirement_id TEXT REFERENCES reconciliation_requirements(id) ON DELETE RESTRICT,
  incident_id TEXT REFERENCES incidents(id) ON DELETE RESTRICT,
  subject_snapshot_json TEXT NOT NULL CHECK (json_valid(subject_snapshot_json)),
  detail_json TEXT NOT NULL CHECK (json_valid(detail_json)),
  summary TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 500),
  origin TEXT NOT NULL CHECK (origin IN ('live','backfill')),
  first_observed_at TEXT NOT NULL,
  last_observed_at TEXT NOT NULL,
  observation_count INTEGER NOT NULL DEFAULT 1 CHECK (observation_count >= 1),
  recurrence_of TEXT REFERENCES attention_events(id) ON DELETE RESTRICT,
  read_at TEXT,
  acknowledged_at TEXT,
  resolved_at TEXT,
  resolution TEXT CHECK (resolution IN ('conditionCleared','actionCompleted','operatorResolved',
                                        'sourceRemoved')),
  notified_at TEXT,
  evidence_json TEXT CHECK (evidence_json IS NULL
                            OR (json_valid(evidence_json) AND condition = 'job.completed')),
  CHECK (dedup_key = condition || ':' || source_kind || ':' || source_id),
  CHECK (acknowledged_at IS NULL OR read_at IS NOT NULL),
  CHECK (resolved_at IS NULL OR read_at IS NOT NULL),
  CHECK ((resolved_at IS NULL) = (resolution IS NULL)),
  CHECK (resolution <> 'operatorResolved' OR resolution_mode = 'manual'),
  CHECK (source_kind <> 'printer' OR printer_id IS NULL OR printer_id = source_id),
  CHECK (source_kind <> 'job' OR (job_id IS NOT NULL AND job_id = source_id)),
  CHECK (source_kind <> 'reconciliationRequirement'
         OR (requirement_id IS NOT NULL AND requirement_id = source_id)),
  CHECK (source_kind <> 'spool' OR (spool_id IS NOT NULL AND spool_id = source_id)),
  CHECK (recurrence_of IS NULL OR recurrence_of <> id)
) STRICT;
CREATE UNIQUE INDEX attention_events_one_open_per_key ON attention_events(dedup_key)
  WHERE resolved_at IS NULL;
CREATE INDEX attention_events_key_resolved ON attention_events(dedup_key, resolved_at);
CREATE INDEX attention_events_resolved ON attention_events(resolved_at, id);
CREATE INDEX attention_events_open_severity ON attention_events(severity, first_observed_at)
  WHERE resolved_at IS NULL;
CREATE INDEX attention_events_printer ON attention_events(printer_id) WHERE printer_id IS NOT NULL;
CREATE INDEX attention_events_job ON attention_events(job_id) WHERE job_id IS NOT NULL;
CREATE INDEX attention_events_incident ON attention_events(incident_id) WHERE incident_id IS NOT NULL;

CREATE TABLE camera_snapshots (
  id TEXT PRIMARY KEY CHECK (id GLOB 'snp-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  printer_id TEXT NOT NULL REFERENCES printers(id) ON DELETE RESTRICT,
  incident_id TEXT REFERENCES incidents(id) ON DELETE RESTRICT,
  job_id TEXT REFERENCES jobs(id) ON DELETE RESTRICT,
  trigger TEXT NOT NULL CHECK (trigger IN ('incident','completion','manual')),
  operation_id TEXT UNIQUE,
  captured_at TEXT NOT NULL,
  content_type TEXT NOT NULL CHECK (content_type IN ('image/jpeg','image/png')),
  byte_len INTEGER NOT NULL CHECK (byte_len BETWEEN 1 AND 10485760),
  sha256 TEXT NOT NULL CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
  rel_path TEXT NOT NULL UNIQUE CHECK (rel_path GLOB 'snapshots/*' AND rel_path NOT GLOB '*..*'),
  pinned_at TEXT,
  pruned_at TEXT,
  prune_reason TEXT CHECK (prune_reason IN ('age','diskCap','missingFile')),
  CHECK ((pruned_at IS NULL) = (prune_reason IS NULL)),
  CHECK (pinned_at IS NULL OR pruned_at IS NULL OR prune_reason = 'missingFile'),
  CHECK (trigger <> 'incident' OR incident_id IS NOT NULL),
  CHECK (trigger <> 'completion' OR (job_id IS NOT NULL AND incident_id IS NULL)),
  CHECK ((trigger = 'manual') = (operation_id IS NOT NULL))
) STRICT;
CREATE INDEX camera_snapshots_printer ON camera_snapshots(printer_id, captured_at);
CREATE INDEX camera_snapshots_incident ON camera_snapshots(incident_id) WHERE incident_id IS NOT NULL;
CREATE INDEX camera_snapshots_job ON camera_snapshots(job_id) WHERE job_id IS NOT NULL;
CREATE INDEX camera_snapshots_prunable ON camera_snapshots(captured_at, id)
  WHERE pruned_at IS NULL AND pinned_at IS NULL;

CREATE TABLE incident_events (
  id TEXT PRIMARY KEY CHECK (id GLOB 'iev-*' AND length(id) BETWEEN 5 AND 64),
  incident_id TEXT NOT NULL REFERENCES incidents(id) ON DELETE RESTRICT,
  sequence INTEGER NOT NULL CHECK (sequence >= 1),
  kind TEXT NOT NULL CHECK (kind IN ('opened','eventLinked','reopened','eventAcknowledged',
    'eventResolved','evidenceCaptured','evidenceSkipped','evidencePruned','evidencePinned',
    'evidenceUnpinned','noteAdded','closed')),
  attention_event_id TEXT REFERENCES attention_events(id) ON DELETE RESTRICT,
  snapshot_id TEXT REFERENCES camera_snapshots(id) ON DELETE RESTRICT,
  detail_json TEXT NOT NULL CHECK (json_valid(detail_json)),
  operation_id TEXT,
  at TEXT NOT NULL,
  UNIQUE (incident_id, sequence),
  CHECK (kind NOT IN ('opened','eventLinked','reopened','eventAcknowledged','eventResolved')
         OR attention_event_id IS NOT NULL),
  CHECK (kind NOT IN ('evidenceCaptured','evidencePruned','evidencePinned','evidenceUnpinned')
         OR snapshot_id IS NOT NULL)
) STRICT;
CREATE TRIGGER incident_events_append_only_u BEFORE UPDATE ON incident_events
  BEGIN SELECT RAISE(ABORT, 'incident_events is append-only'); END;
CREATE TRIGGER incident_events_append_only_d BEFORE DELETE ON incident_events
  BEGIN SELECT RAISE(ABORT, 'incident_events is append-only'); END;

CREATE TABLE printer_cameras (
  printer_id TEXT PRIMARY KEY REFERENCES printers(id) ON DELETE CASCADE,
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  source_kind TEXT NOT NULL CHECK (source_kind IN ('hostWebcam','snapshotUrl')),
  webcam_name TEXT CHECK (webcam_name IS NULL OR length(webcam_name) BETWEEN 1 AND 128),
  webcam_service TEXT CHECK (webcam_service IS NULL OR length(webcam_service) BETWEEN 1 AND 64),
  web_port INTEGER CHECK (web_port IS NULL OR web_port BETWEEN 1 AND 65535),
  snapshot_url TEXT CHECK (snapshot_url IS NULL
                           OR (length(snapshot_url) BETWEEN 8 AND 2048 AND snapshot_url GLOB 'http://*')),
  updated_at TEXT NOT NULL,
  CHECK (source_kind <> 'hostWebcam' OR (webcam_name IS NOT NULL AND snapshot_url IS NULL)),
  CHECK (source_kind <> 'snapshotUrl' OR (snapshot_url IS NOT NULL AND webcam_name IS NULL
                                          AND webcam_service IS NULL AND web_port IS NULL))
) STRICT;

CREATE TABLE printer_alert_defaults (
  printer_id TEXT PRIMARY KEY REFERENCES printers(id) ON DELETE CASCADE,
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  offline_after_minutes INTEGER CHECK (offline_after_minutes IS NULL OR offline_after_minutes IN (1,5,15)),
  notifications TEXT NOT NULL CHECK (notifications IN ('follow','muted')),
  snapshot_on_incident INTEGER NOT NULL CHECK (snapshot_on_incident IN (0,1)),
  snapshot_on_completion INTEGER NOT NULL CHECK (snapshot_on_completion IN (0,1)),
  updated_at TEXT NOT NULL
) STRICT;

ALTER TABLE settings ADD COLUMN notify_fatal INTEGER NOT NULL DEFAULT 1 CHECK (notify_fatal IN (0,1));
ALTER TABLE settings ADD COLUMN notify_confirmation INTEGER NOT NULL DEFAULT 1 CHECK (notify_confirmation IN (0,1));
ALTER TABLE settings ADD COLUMN notify_completion INTEGER NOT NULL DEFAULT 1 CHECK (notify_completion IN (0,1));
ALTER TABLE settings ADD COLUMN notify_reconciliation INTEGER NOT NULL DEFAULT 0 CHECK (notify_reconciliation IN (0,1));
ALTER TABLE settings ADD COLUMN notify_connectivity INTEGER NOT NULL DEFAULT 0 CHECK (notify_connectivity IN (0,1));
ALTER TABLE settings ADD COLUMN notify_inventory INTEGER NOT NULL DEFAULT 0 CHECK (notify_inventory IN (0,1));
ALTER TABLE settings ADD COLUMN snapshot_retention_days INTEGER NOT NULL DEFAULT 30
  CHECK (snapshot_retention_days BETWEEN 1 AND 365);
ALTER TABLE settings ADD COLUMN snapshot_disk_cap_mb INTEGER NOT NULL DEFAULT 2048
  CHECK (snapshot_disk_cap_mb BETWEEN 100 AND 102400);
```

Then it rebuilds `operations` exactly as 0008:140–149 does: every one of
0008's 26 kinds, plus the 9 new ones (D9 "Operation kinds and digests"):

```sql
CREATE TABLE operations_p8 (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('moveSpool','archivePrinter','spoolLifecycle','startSlice',
    'createExternalSliceRevision','stageSliceRevision','startStagedArtifact','pauseHostPrint',
    'resumeHostPrint','cancelHostPrint','abandonHostOperation','addToQueue','updateQueueEntry',
    'moveQueueEntry','removeQueueEntry','assignQueueEntry','stageJob','startJob','pauseJob',
    'resumeJob','cancelJob','releaseJob','retryJob','declareJobOutcome','settleJobMaterial',
    'correctJobMaterial',
    'markAttentionRead','acknowledgeAttention','resolveAttention','addIncidentNote',
    'setPrinterCamera','clearPrinterCamera','captureSnapshot','setSnapshotPinned',
    'setPrinterAlertDefaults')),
  request_digest TEXT NOT NULL,
  created_at TEXT NOT NULL
) STRICT;
INSERT INTO operations_p8(id, kind, request_digest, created_at)
  SELECT id, kind, request_digest, created_at FROM operations;
DROP TABLE operations;
ALTER TABLE operations_p8 RENAME TO operations;
```

Notes:

- `incidents` is created before `attention_events` and `camera_snapshots`
  before `incident_events`, so every foreign key's target exists.
- No `printer_alert_defaults` row means the defaults (`5`, `follow`,
  true, true). A row with `offline_after_minutes` NULL means "off".
- `snapshot_disk_cap_mb` is MiB (2048 = 2 GiB; 100 MiB–100 GiB).
- `attention_events.printer_id` is set for `printer.*` (the source) and
  for `job.*` and `requirement.*` (the Job's Printer); it is NULL for
  `spool.low`. It is the only reference to a Printer that a delete may
  NULL.
- `camera_snapshots.operation_id` is set exactly for `manual` captures
  (the `capture_snapshot` replay lookup).
- No new column name contains `credential`, `secret`, `token`, `password`,
  or `key`. `snapshot_url` is the only column that can hold a camera URL.
- The `ALTER TABLE settings … ADD COLUMN … CHECK` form is the one 0002
  uses.
- `attention_events` has no append-only trigger: its lifecycle columns
  change. `resolved_at` is final in code (`lifecycle::apply`).

### Operation kinds and digests

`OperationKind` gains 9 camelCase variants:

| Variant (wire) | Command | Digest (a struct, fields in this order; SHA-256 of its JSON, as `spools::operations::digest`) |
|---|---|---|
| `markAttentionRead` | `mark_attention_read` | `{ eventIds }` |
| `acknowledgeAttention` | `acknowledge_attention_event` | `{ eventId }` |
| `resolveAttention` | `resolve_attention_event` | `{ eventId }` |
| `addIncidentNote` | `add_incident_note` | `{ incidentId, text }` |
| `setPrinterCamera` | `set_printer_camera` | `{ printerId, source }` |
| `clearPrinterCamera` | `clear_printer_camera` | `{ printerId }` |
| `captureSnapshot` | `capture_snapshot` | `{ printerId }` |
| `setSnapshotPinned` | `set_snapshot_pinned` | `{ snapshotId, pinned }` |
| `setPrinterAlertDefaults` | `set_printer_alert_defaults` | `{ printerId, alertDefaults }` |

The digest is a hash, so a manual URL in `set_printer_camera`'s request
is never stored in `operations`. A replay (same id, kind, and digest)
returns the current rows with no side effect and no event. A reused id
with a different request is `VALIDATION` on `operationId`
(`RepositoryError::OperationIdReused`). A rejected command rolls back and
never burns its id. `capture_snapshot` checks for a replay before it
fetches (the row's `operation_id`), so a replay never touches the camera.

### Wire types (ts-rs, camelCase)

```ts
// attention
type ConditionKind =
  | "printer.offline" | "printer.connectionError" | "printer.hostFailed"
  | "job.startConfirmation" | "job.failed" | "job.hostCancelled"
  | "requirement.materialReconciliation" | "requirement.jobOutcomeUnknown"
  | "spool.low" | "job.completed";
type AttentionSeverity = "fatal" | "warning" | "info";
type ResolutionMode = "auto" | "action" | "manual";
type AttentionResolution = "conditionCleared" | "actionCompleted" | "operatorResolved" | "sourceRemoved";
type AttentionOrigin = "live" | "backfill";
type AttentionSourceKind = "printer" | "job" | "reconciliationRequirement" | "spool";
type AttentionSource = { kind: AttentionSourceKind; id: string };
type NotificationClass = "fatal" | "confirmation" | "completion" | "reconciliation" | "connectivity" | "inventory";
type AttentionSubject = {
  printerName: string | null; printerLocation: string | null; jobLabel: string | null;
  spoolNumber: number | null; spoolLabel: string | null;
};
type EvidenceOutcome =
  | { status: "captured"; snapshotId: string }
  | { status: "skipped"; reason: "cameraError" | "diskCap"; errorKind: CameraErrorKind | null };
type AttentionDetail =
  | { kind: "printerOffline"; unreachableSince: string }
  | { kind: "printerConnectionError"; cause: "auth" | "protocol" }
  | { kind: "printerHostFailed" }
  | { kind: "jobStartConfirmation"; awaitingMaterial: boolean }
  | { kind: "jobFailed"; endedAt: string }
  | { kind: "jobHostCancelled"; endedAt: string }
  | { kind: "requirementMaterialReconciliation"; requirementStatus: "pending" | "deferred"; spoolId: string }
  | { kind: "requirementJobOutcomeUnknown" }
  | { kind: "spoolLow"; currentMg: number; lowThresholdMg: number }
  | { kind: "jobCompleted"; endedAt: string };
type AttentionAction = "markRead" | "acknowledge" | "resolve";
type AttentionEvent = {
  id: string; revision: number; dedupKey: string;
  condition: ConditionKind; severity: AttentionSeverity; requiresAction: boolean;
  resolutionMode: ResolutionMode; notificationClass: NotificationClass;
  source: AttentionSource;
  printerId: string | null; jobId: string | null; spoolId: string | null;
  requirementId: string | null; incidentId: string | null;
  subject: AttentionSubject; detail: AttentionDetail; summary: string;
  origin: AttentionOrigin;
  firstObservedAt: string; lastObservedAt: string; observationCount: number;
  recurrenceOf: string | null;
  readAt: string | null; acknowledgedAt: string | null;
  resolvedAt: string | null; resolution: AttentionResolution | null;
  notifiedAt: string | null;
  evidence: EvidenceOutcome | null;    // job.completed only; written by record_evidence, never by Amend
  allowedActions: AttentionAction[];   // Rust-computed: markRead if unread; acknowledge if open and
                                       // unacknowledged; resolve if open and manual
};
type AttentionCursor = string;         // opaque; encodes (resolvedAt, id)
type AttentionBackfill = {
  streamId: string; snapshotSequence: number;
  open: AttentionEvent[];              // every open Event: severity (fatal, warning, info),
                                       // then firstObservedAt descending, then id
  resolved: AttentionEvent[];          // up to `limit` (default 200), resolvedAt descending, then id descending
  resolvedCursor: AttentionCursor | null;   // null when no older resolved Event exists
  openIncidents: Incident[];
  cameraHealth: CameraHealth[];        // one per Printer with a camera source
};
type AttentionChange = { events: AttentionEvent[]; incidents: Incident[] };

// incidents
type IncidentState = "open" | "closed";
type Incident = {
  id: string; revision: number;
  kind: "printer.hostFailed" | "job.failed" | "job.hostCancelled" | "requirement.jobOutcomeUnknown";
  state: IncidentState; printerId: string; jobId: string | null;
  printerSnapshot: PrinterSnapshot;    // P7's type
  openedAt: string; closedAt: string | null;
  linkedEventIds: string[]; openLinkedEventCount: number; snapshotCount: number;
};
type IncidentEntryKind = "opened" | "eventLinked" | "reopened" | "eventAcknowledged" | "eventResolved"
  | "evidenceCaptured" | "evidenceSkipped" | "evidencePruned" | "evidencePinned" | "evidenceUnpinned"
  | "noteAdded" | "closed";
type IncidentEntryDetail =
  | { kind: "opened" | "eventLinked" | "reopened"; eventId: string }
  | { kind: "eventAcknowledged"; eventId: string; by: "operator" | "system" }
  | { kind: "eventResolved"; eventId: string; resolution: AttentionResolution }
  | { kind: "evidenceCaptured"; snapshotId: string; trigger: SnapshotTrigger }
  | { kind: "evidenceSkipped"; reason: "cameraError" | "diskCap"; errorKind: CameraErrorKind | null }
  | { kind: "evidencePruned"; snapshotId: string; reason: PruneReason }
  | { kind: "evidencePinned" | "evidenceUnpinned"; snapshotId: string }
  | { kind: "noteAdded"; text: string }
  | { kind: "closed" };
type IncidentEntry = {
  id: string; incidentId: string; sequence: number; kind: IncidentEntryKind;
  detail: IncidentEntryDetail; operationId: string | null; at: string;
};
type IncidentTimelineItem =
  | { source: "incident"; entry: IncidentEntry }
  | { source: "job"; event: JobEvent };          // P7's type, read from job_events
type IncidentDetail = {
  incident: Incident; timeline: IncidentTimelineItem[];
  events: AttentionEvent[]; snapshots: CameraSnapshot[];
};
type IncidentPage = { incidents: Incident[]; nextCursor: string | null };

// cameras
type CameraSourceKind = "hostWebcam" | "snapshotUrl";
type CameraSource =
  | { kind: "hostWebcam"; webcamName: string; webcamService: string | null; webPort: number | null }
  | { kind: "snapshotUrl"; snapshotUrl: string };
type CameraSourceInput = CameraSource;
type PrinterCamera = { printerId: string; revision: number; source: CameraSource; updatedAt: string };
                                       // get_printer_camera only: the one command result with a manual URL
type PrinterCameraSummary = {          // set_printer_camera's result: the URL redacted
  printerId: string; revision: number; sourceKind: CameraSourceKind;
  webcamName: string | null; webcamService: string | null; webPort: number | null;  // hostWebcam only
  hasSnapshotUrl: boolean;             // true exactly for snapshotUrl
  updatedAt: string;
};
type HostWebcam = { name: string; service: string };      // never a URL
type CameraErrorKind = "unreachable" | "timeout" | "httpStatus" | "tooLarge" | "notAnImage"
  | "noSuchWebcam" | "noSnapshotUrl" | "hostMismatch" | "unsupportedAdapter" | "webcamListFailed";
type CameraHealthState = "notConfigured" | "unsupported" | "unknown" | "ok" | "failing";
type CameraHealth = {
  printerId: string; state: CameraHealthState; sourceKind: CameraSourceKind | null;
  lastSuccessAt: string | null; lastFailureAt: string | null; lastFailureKind: CameraErrorKind | null;
};
type SnapshotTrigger = "incident" | "completion" | "manual";
type PruneReason = "age" | "diskCap" | "missingFile";
type CameraSnapshot = {
  id: string; revision: number; printerId: string; incidentId: string | null; jobId: string | null;
  trigger: SnapshotTrigger; capturedAt: string; contentType: "image/jpeg" | "image/png";
  byteLen: number; sha256: string;
  pinnedAt: string | null; prunedAt: string | null; pruneReason: PruneReason | null;
};                                     // never rel_path
type SnapshotPage = { snapshots: CameraSnapshot[]; nextCursor: string | null };
type MediaUsage = {
  usedBytes: number; pinnedBytes: number; capBytes: number; retentionDays: number;
  snapshotCount: number; pinnedCount: number; prunedCount: number;
};
type FrameHeader = {
  contentType: "image/jpeg" | "image/png"; capturedAt: string; byteLen: number;
  snapshotId: string | null;           // set by snapshot_image only
};

// alerts and notifications
type NotificationMode = "follow" | "muted";
type AlertDefaults = {
  offlineAfterMinutes: 1 | 5 | 15 | null;   // null = off
  notifications: NotificationMode; snapshotOnIncident: boolean; snapshotOnCompletion: boolean;
};
type PrinterAlertDefaults = { printerId: string; revision: number | null;  // null: no row, the defaults
                              alertDefaults: AlertDefaults; updatedAt: string | null };
type NotificationClassSettings = {
  fatal: boolean; confirmation: boolean; completion: boolean;
  reconciliation: boolean; connectivity: boolean; inventory: boolean;
};
type SnapshotRetention = { retentionDays: number; diskCapMb: number };
type NavigateRequest = { contractVersion: 1; target: NavigationTarget; openAttentionCenter: boolean };
// NotifierStatus: see D6
```

- `SettingsRecord` gains `notifications: NotificationClassSettings` and
  `snapshotRetention: SnapshotRetention`.
- `LifecycleBlockerCode` gains `INCIDENT_HISTORY_EXISTS` and
  `PINNED_EVIDENCE_EXISTS`.
- `CreatePrinterOptions` gains `camera: Option<CameraSourceInput>` and
  `alert_defaults: Option<AlertDefaults>`. `create_printer` gains optional
  `camera` and `alertDefaults` arguments, validated and written in the
  create transaction (`VALIDATION` with `camera.<field>` or
  `alertDefaults.<field>` rejects the whole create).
- `BatchShared` gains `cameraTemplate?: CameraTemplate` and
  `alertDefaults?: AlertDefaults`; `BatchRowInput` gains
  `cameraHostOverride?: string`:

  ```ts
  type CameraTemplate =
    | { kind: "hostWebcam"; webcamName: string; webPort: number | null }
    | { kind: "snapshotUrl"; path: string; port: number };   // path starts with "/", ≤ 1024, no "#"
  ```

  Each row's camera is built on its own: `hostWebcam` stores the name
  (and `webPort`) and resolves against that row's Connection at fetch
  time; `snapshotUrl` builds `http://<host>:<port><path>` with `host` =
  the row's `cameraHostOverride`, else its Connection host, and validates
  it like a manual URL (`rows[<i>].cameraHostOverride` or
  `shared.cameraTemplate.path` on failure). A row with no Connection and
  no override gets no camera (and is Setup incomplete, as today). Nothing
  else (tests, health, snapshots) is copied between rows. CSV and paste
  intake gain no camera columns.
- The Printers export **file** (schema 4, written to disk by Rust; never a
  command result or event) gives each Printer `camera: CameraSource
  | null` (a manual URL included, as the Connection host is) and
  `alertDefaults: AlertDefaults | null`. Import accepts schemas 1–4;
  schemas 1–3 import with no camera and default alert defaults.
- The Settings export (schema 3) adds `notifications` and
  `snapshotRetention`. Import accepts 1–3 (`deny_unknown_fields` stays);
  missing fields take the defaults.

### Commands

22 commands, each registered per Global Constraint 9: `lib.rs`
(`COMMAND_NAMES` and `generate_handler!`), `contracts/inventory.rs` (array
length, decl string, visitor), `tests/export_contracts.rs`,
`tests/f1_contract_path.rs` and `tests/p3_contract_path.rs` (count,
names, handlers, one case each), `tests/f1_residual_acceptance.rs` (its
count), and `src/ipc/client.ts` (`CommandMap`, or `BinaryCommandMap` for
the three binary commands). Count assertions become `58 + 21 + 2 + 8 + 11
+ 4 + 1 + 2 + 22`. Arguments are top-level camelCase fields.

| Command | Arguments | Result |
|---|---|---|
| `list_attention` | `{ resolvedBefore?: AttentionCursor, limit?: 1..=200 }` | `AttentionBackfill`. Without `resolvedBefore`: every open Event and the first resolved page. With it: `open: []`, `openIncidents: []`, `cameraHealth: []`, and the next resolved page |
| `mark_attention_read` | `{ operationId, eventIds: string[] (1..=200, distinct) }` | `AttentionChange` (the Events that changed; unknown ids are `NOT_FOUND` for the whole batch) |
| `acknowledge_attention_event` | `{ operationId, eventId }` | `AttentionChange` (the Event, and its Incident if linked) |
| `resolve_attention_event` | `{ operationId, eventId }` | `AttentionChange` (the Event, and its Incident, closed if this was its last open linked Event). Non-manual: `ATTENTION_NOT_MANUAL` |
| `list_incidents` | `{ state?: "open" \| "closed" \| "all" (default "all"), printerId?, before?: string, limit?: 1..=200 }` | `IncidentPage`, `openedAt` descending, then id |
| `get_incident` | `{ incidentId }` | `IncidentDetail` |
| `add_incident_note` | `{ operationId, incidentId, text }` | `IncidentDetail` |
| `get_printer_camera` | `{ printerId }` | `PrinterCamera \| null` (the only command that returns a manual URL) |
| `set_printer_camera` | `{ operationId, printerId, source: CameraSourceInput }` | `PrinterCameraSummary` (the URL redacted; a replay returns the current summary) |
| `clear_printer_camera` | `{ operationId, printerId }` | `{ printerId: string; cleared: boolean }` |
| `list_host_webcams` | `{ printerId?: string, connection?: ConnectionSubmission }` (exactly one) | `HostWebcam[]` |
| `test_camera` | `{ printerId?: string, connection?: ConnectionSubmission, source: CameraSourceInput }` | binary frame (below); never stored. With `printerId`, it also updates that Printer's health |
| `camera_preview_frame` | `{ printerId }` | binary frame; never stored |
| `capture_snapshot` | `{ operationId, printerId }` | `CameraSnapshot` |
| `list_snapshots` | `{ printerId?, incidentId?, jobId?, includePruned?: boolean (default true), before?: string, limit?: 1..=200 }` | `SnapshotPage`, `capturedAt` descending, then id |
| `snapshot_image` | `{ snapshotId }` | binary frame with `snapshotId` |
| `set_snapshot_pinned` | `{ operationId, snapshotId, pinned: boolean }` | `CameraSnapshot`. Same state: no-op success. `pinned: true` on a pruned row: `EVIDENCE_PRUNED`; `pinned: false` on a pruned row: allowed |
| `media_usage` | `{}` | `MediaUsage` |
| `get_printer_alert_defaults` | `{ printerId }` | `PrinterAlertDefaults` |
| `set_printer_alert_defaults` | `{ operationId, printerId, alertDefaults: AlertDefaults }` | `PrinterAlertDefaults` |
| `notification_status` | `{}` | `NotifierStatus` |
| `send_test_notification` | `{}` | `{ sent: true }`; `NOTIFICATIONS_UNAVAILABLE` otherwise |

- **Binary frame** (`test_camera`, `camera_preview_frame`,
  `snapshot_image`, as `tauri::ipc::Response`): bytes 0–3 are the header
  length `H` (u32, big-endian, ≤ 4096); bytes 4..4+H are a UTF-8 JSON
  `FrameHeader`; the rest is the image. `src/cameras/frame.ts` parses it;
  the frontend makes an object URL from the image bytes with the header's
  `contentType`.
- `set_printer_camera` and `set_printer_alert_defaults` take no
  `expectedRevision`: they are single-row, last-writer-wins settings of a
  single-user app, and the operation id makes a retry safe.
- `list_host_webcams` and `test_camera` with `connection` (the Setup
  wizard and batch rows, before the Printer exists) use the submitted
  Connection exactly as `probe_connection` does (no Printer id, no
  storage, no credential-store access), and never store it. A
  `hostWebcam` source needs exactly one of `printerId` and `connection`
  (`VALIDATION` on `printerId` otherwise); a `snapshotUrl` source needs
  neither, and a `printerId`, if given, only selects whose health a
  `test_camera` updates.
- Printer existence: `NOT_FOUND` for every command naming a missing
  Printer, Event, Incident, or snapshot. Archived Printers are allowed.
- **After commit**, every mutating command publishes its rows on the
  `attention` stream; `set_printer_camera` / `clear_printer_camera`
  publish `camera.health.changed`; `set_printer_alert_defaults` pokes the
  projector; `save_settings` pokes the `MediaJanitor` when the retention
  changed. Nothing is published for a replay or a no-op.
- `save_settings` gains two optional arguments, `notifications?:
  NotificationClassSettings` and `snapshotRetention?: SnapshotRetention`.
  Absent means "keep the stored value", so the existing theme and Monitor
  callers are unchanged. It keeps its `expectedRevision` check (no
  `operationId`). Out-of-range retention is `VALIDATION` on
  `snapshotRetention.retentionDays` / `snapshotRetention.diskCapMb`.

### Events

The **`attention` stream** is its own sequence on `farm3d-event-v1`, in
the usual `EventEnvelope`, following `queue/events.rs`. The payload is a
tagged union. The envelope type is `AttentionStreamEvent`
(`AttentionEvent` is the record).

| Type | Subject | Payload |
|---|---|---|
| `attention.event.changed` | `{ kind: "attentionEvent", id }` | `{ type: "eventChanged", event: AttentionEvent }` |
| `attention.incident.changed` | `{ kind: "incident", id }` | `{ type: "incidentChanged", incident: Incident }` |
| `attention.snapshot.changed` | `{ kind: "cameraSnapshot", id }` | `{ type: "snapshotChanged", snapshot: CameraSnapshot }` |
| `camera.health.changed` | `{ kind: "printer", id: printerId }` | `{ type: "cameraHealthChanged", health: CameraHealth }` |

- After commit only, never for a replay or no-op. Within one change:
  Events, then Incidents, then snapshots.
- `list_attention` reads `snapshot_sequence()` before it reads rows
  (listen before backfill).
- The frontend replaces a record only by a higher `revision`.
- `attention.incident.changed` is published once per Incident per
  committed transaction, with the final row (D3 "Revision and
  publishing"). An open Incident's detail is refetched with `get_incident`
  when one with a higher revision arrives; timeline rows are not
  streamed.

**`farm3d-navigate-v1`** is a separate, unsequenced event with a
`NavigateRequest` payload. It has no backfill. The frontend routes it
through its `navigate`, and opens the Attention center when
`openAttentionCenter` is true.

### Error codes

`ErrorCode` gains the codes below. `details` values are camelCase JSON
and never carry a URL, host, port, credential, or host response body.

| Code | Raised by | `details` | `recovery` | Message |
|---|---|---|---|---|
| `ATTENTION_NOT_MANUAL` | `resolve_attention_event` on an `auto` or `action` Event | `{ eventId, condition: ConditionKind, resolutionMode }` | `[RELOAD]` | "This Attention Event resolves by itself when its cause clears." |
| `CAMERA_NOT_CONFIGURED` | `camera_preview_frame`, `capture_snapshot` on a Printer without a source | `{ printerId }` | `[OPEN_PRINTER_SETUP]` | "This Printer has no camera." |
| `CAMERA_FAILED` | a frame fetch failed (every `CameraErrorKind` except `hostMismatch` and `unsupportedAdapter`) | `{ printerId: string \| null, kind: CameraErrorKind, httpStatus: number \| null }` | `[RETRY]`, plus `[OPEN_PRINTER_SETUP]` for `noSuchWebcam`, `noSnapshotUrl` | per kind, for example "The camera did not answer in time." |
| `CAMERA_HOST_MISMATCH` | a `hostWebcam` whose absolute URL is on another host | `{ printerId: string \| null }` | `[OPEN_PRINTER_SETUP]` | "The printer's webcam points at a different host. Use a manual snapshot URL." |
| `EVIDENCE_PRUNED` | `snapshot_image`, and `set_snapshot_pinned` with `pinned: true`, on a pruned snapshot (`pinned: false` is allowed, D5) | `{ snapshotId, reason: PruneReason }` | `[]` | "This snapshot's image was removed (<reason label>)." |
| `SNAPSHOT_DISK_CAP` | `capture_snapshot` when only pinned snapshots would be left to prune | `{ usedBytes, capBytes, pinnedBytes }` | `[]` | "The snapshot disk cap is full of pinned evidence. Unpin some or raise the cap." |
| `NOTIFICATIONS_UNAVAILABLE` | `send_test_notification` with an `unavailable` or `unsupported` notifier | `{ status: NotifierStatus }` | `[]` | "Desktop notifications aren't available here." |
| `EVIDENCE_EXISTS` | `import_printers` (`replace_all`) | `{ printerIds, incidentIds, snapshotIds }` (each at most 20) | `[]` | "Printers with Incidents or camera evidence can't be replaced by an import." |

- `unsupportedAdapter` maps to the existing `CAPABILITY_UNSUPPORTED`
  (`{ printerId, capability: "camera", reason: "adapter", detail }`).
- Reused, unchanged: `VALIDATION` (URL rules, ranges, a reused
  `operationId`), `NOT_FOUND`, `CONFLICT` (`save_settings`),
  and `LIFECYCLE_BLOCKED` (with the two new blocker codes). Web mode
  refuses camera preview and test with the frontend's existing
  `needsDesktopError` (`PERSISTENCE_UNAVAILABLE`).
- No `RecoveryCode` is added.
- `RepositoryError` gains `AttentionNotManual`, `CameraNotConfigured`,
  `EvidencePruned`, `SnapshotDiskCap`, and `EvidenceExists`, each mapped in
  `CommandError::from_repository`.

## Frontend architecture

Rust is the only source of Conditions, severities, resolution, allowed
actions, retention, and notification decisions. TypeScript presents them.

- **`src/attention/attention-store.ts`**: listen-before-backfill over the
  `attention` stream through `createSequencedStream`, backfilled from
  `list_attention`. Reads: `open()`, `resolved()` (paged with
  `resolvedCursor`), `event(id)`, `incident(id)`, `openIncidents()`,
  `cameraHealth(printerId)`, `actionableCount()` (open and
  `requiresAction`, acknowledged or not), `unreadCount()`,
  `highestOpenSeverity()`, `eventsForPrinter(printerId)`, `syncState`.
  Writes: one per command, each with a fresh `crypto.randomUUID()` reused
  on one transport retry, patched from the returned `AttentionChange`.
  Records are replaced only by a higher `revision`.
- **`src/attention/presentation.ts`**: labels for every `ConditionKind`,
  `AttentionSeverity` (the words "Fatal", "Warning", "Info"),
  `AttentionResolution`, `IncidentEntryKind`, `CameraErrorKind`,
  `CameraHealthState`, `PruneReason`, and the filter predicates:
  Actionable (open and `requiresAction`; the default), Unread (`readAt`
  null), All open, Resolved; and a severity filter (all, fatal, warning,
  info).
- **`src/attention/deep-link.ts`**: decision 8's table. `targetForSource
  (event)` and `openTargetFor(event, availableIds)`, which falls back to
  `monitor/attention/<id>` when the source id is not available. It
  mirrors `attention::deep_link::target_for`; both are tested against the
  same table.
- **`src/incidents/incident-store.ts`** (`get_incident`,
  `list_incidents`, `add_incident_note`) and **`src/cameras/camera-store.ts`**
  (health from the attention store; `usePreview(printerId, visible)`,
  which polls `camera_preview_frame` about once a second while `visible`,
  revokes the previous object URL on each frame and on stop; snapshots;
  pinning; `media_usage`). `src/cameras/frame.ts` parses binary frames.
  The camera store never holds a camera URL. The Setup camera section
  reads `get_printer_camera` into its own form state only; after
  `set_printer_camera` it keeps the URL it just submitted in that form
  state (the result, a `PrinterCameraSummary`, has none).
- **Web mode** loads deterministic fixtures (`web-fixtures.ts`): every
  Condition kind, one recurrence chain, one open Incident with a snapshot
  (a generated PNG test pattern as a data URL), and one pruned snapshot.
  Camera preview and test return `needsDesktopError`, and the UI says so.
- **Shell.** `AttentionTrigger` sits in the top bar after the rosters. Its
  label is the actionable count, with a `SeverityMarker` for the highest
  open severity. It opens `AttentionCenter` in a non-modal Kobalte
  `Popover` (its own trigger): the filters (a `SegmentedControl` for
  Actionable / Unread / All open / Resolved, and a `Select` for severity),
  kept across open and close; the list; distinct empty ("Nothing needs
  your attention.") and filtered-empty ("No Events match this filter.")
  states. Selecting a row navigates to `monitor/attention/<id>` and closes
  the popover. The Monitor rail button's badge shows the same actionable
  count ("Monitor (N need attention)").
- **Monitor dock.** `PrinterDashboard` shows `AttentionEventDetail` for an
  `attention` selection and `IncidentDetail` for an `incident` selection,
  in the reusable dock. Event detail: `SeverityMarker` (shape, label,
  color), the summary and subject, source, timestamps, observation count,
  origin, the recurrence link, the Incident link, Acknowledge (when
  allowed), Resolve (manual only), and "Open source" via `deep-link.ts`.
  Opening the detail marks the Event read. A deleted source shows the
  subject with "Printer deleted".
- **Printer detail dock.** Tabs are Status, Setup, Job, Camera (Camera
  appended; today's order kept). The Status tab lists the Printer's open
  Events (from `eventsForPrinter`) with their severity markers and links.
  The Camera tab: no source → an explanation plus "Set up camera" (switches
  to Setup, the `open-printer-job.ts` pattern); `unsupported` → the
  reason; otherwise the preview with its health text and last-frame time,
  Capture, the snapshot history (pinned and pruned states), and the
  retention summary from `media_usage`.
- **Incident detail**: header (kind, state, Printer snapshot identity,
  times), the merged timeline (`Timeline`), every kind rendered as text,
  linked Events, notes (`add_incident_note`), and evidence thumbnails
  (alt text, timestamp, trigger; a pruned item shows "Evidence pruned
  (<reason>)" and no image). `SnapshotViewerDialog` shows one snapshot
  with Pin/Unpin.
- **Setup.** `CameraSourceSection` in the wizard's Equip step (None — the
  default, which never blocks Next —, host webcam from
  `list_host_webcams` once Connect has a Connection, or a manual URL with
  inline validation from the Rust field path; Test snapshot shows the
  image or a typed error and saves nothing) and in the dock's Setup tab.
  `AlertDefaultsSection` in the Operate step and the Setup tab. Review
  summarizes both. `PrinterBatchDialog`'s shared step gets the camera
  template and alert defaults; `BatchRowsTable` gets a camera host
  override column, shown only for a `snapshotUrl` template; batch test
  snapshots report per row.
- **Settings.** `SettingsMenu` gains "Notifications and retention…",
  which opens `NotificationSettingsDialog`: the six class switches, the
  retention days (`NumberField`, 1–365) and disk cap (`NumberField`, MiB,
  100–102400), the notifier status text ("Available: <server>",
  "Unavailable: no notification service", "Not supported on this
  platform"), and "Send test notification". A revision conflict reloads
  and explains.
- **Navigation.** `App.tsx` adds Attention Event and Incident ids to
  `navigationContext.availableIds` (with a pending allowance while the
  attention store loads, as Queue has), adds `startAttention()` to the
  startup chain after Queue, and listens for `farm3d-navigate-v1`.
- **Design system.** No new component is required. `SeverityMarker`,
  `Timeline`, `Popover`, `SegmentedControl`, `Select`, `Tabs`, `Switch`,
  `NumberField`, `Dialog`, and `DataTable` cover P8. If review asks for a
  reusable count badge, it goes into `components/index.ts` and
  `Showcase.tsx`.

## Accessibility and adaptation

- Severity always pairs the `SeverityMarker` icon shape, its label word,
  and its color.
- The trigger's accessible name is "Attention: N actionable, highest
  <severity word>" (or "Attention: nothing needs action"). It opens with
  Enter or Space, Escape closes the popover and returns focus to it, and
  the list is keyboard navigable.
- One polite live region in `AttentionTrigger` announces each new live
  `fatal` or `warning` Event ("New fatal Attention Event: <summary>").
- Camera content always shows a textual state ("Live", "Camera not
  answering (timeout)", "No camera", "Not supported by this Connection")
  and a timestamp ("Last frame 12:00:03"). Every image has alt text:
  "<trigger> snapshot of <Printer> at <time>".
- Reduced motion: no fade between preview frames.
- Every dialog is a Kobalte `Dialog` that traps and returns focus.
  Destructive or irreversible actions (Resolve) never rely on an icon
  alone.
- The layout works at 1440 × 900 and 1024 × 700, where the dock is an
  overlay and the popover isn't clipped. Tokens, CSS Modules, and Kobalte
  only, in the dense editor aesthetic: no elevation, no ripple, and no
  toast stack in place of the Attention center.

## Acceptance criteria

1. **Migration.** 0009 applies to a P7 database and keeps every row; the
   settings defaults match D6 and D5; 0009 is atomic
   (`apply_through_failing_before_commit`). The partial UNIQUE index
   rejects a second open Event per key and allows one after resolution.
   The `incident_events` triggers reject UPDATE and DELETE. The read
   CHECKs reject acknowledged-but-unread and resolved-but-unread. No new
   column name contains `credential`, `secret`, `token`, `password`, or
   `key`.
2. **Lifecycle.** Every cell of D2's lifecycle table is a test.
3. **Observer and planner.** Every fixture in "Condition fixtures" (90
   catalogue, 29 supplementary, 5 deadline) passes verbatim, and the
   fixed-point property holds.
4. **Projector and backfill.** Repeated passes, restarts, lagged
   receivers, and 100 concurrent wakes never create a second open Event
   for a key. Resolving a requirement resolves its Event
   `actionCompleted` and keeps both histories. Acknowledge never
   resolves. Backfill never notifies or captures.
5. **Incidents.** Every D3 rule, including one Incident per Job, linking,
   reopening, closing, and the merged timeline.
6. **Cameras.** Every D4 URL rule and `CameraErrorKind`; no camera URL in
   any error, event, log, persisted row other than `printer_cameras`, or
   frontend state other than the Setup editor.
7. **Media.** `plan_prune` table tests; the capture order under injected
   crashes; the sweep; 20 concurrent captures against a prune pass keep
   usage within the cap with no orphan and no dangling row.
8. **Optionality.** Task 8's four hanging-camera tests.
9. **Notifications.** `decide` tabled over class × muted × focus × origin
   × change, the per-key and burst limits, body content, and the
   `RecordingSink` click path (navigate and mark read).
10. **Setup.** Single and batch create persist camera and alert defaults
    per Printer and copy no endpoint, test result, health, or evidence
    between rows. Export schema 4 and settings schema 3 round-trip; older
    schemas import with defaults.
11. **Guards.** Every D8 row; no orphaned evidence after any allowed
    delete (`PRAGMA foreign_key_check`); deleted Printers' Events keep
    their subject; archived Printers stay deep-linkable.
12. **Secrets.** A seeded corpus (a credential, a URL with userinfo, a URL
    with a query token, a LAN-style host `192.0.2.10`) never appears in
    an Event, Incident row, snapshot row, error, log line, emitted
    payload, notification body, or fixture.
13. **Frontend.** The Task 12–15 tests pass, and screenshots exist at both
    viewports.
14. **Tracer.** The plan's Task 16 tracer passes on the fakes (CI) and the
    simulator, with the manifest committed.
15. **Installed bundle.** Task 17's checklist on the installed deb (Linux
    x86_64), with Windows and macOS recorded as unverified.

## Delivery

| Task | Delivers from this spec |
|---|---|
| 2 | Evidence for D6 "Click activation" (automatable parts) |
| 3 | Schema, `OperationKind`s, wire types, `attention::lifecycle` |
| 4 | `observe`, `update_watch`, `next_deadline`, `plan`, and "Condition fixtures" |
| 5 | Attention, Incident, and alert-default repositories |
| 6 | The projector, backfill, wakes, `attention` stream, the queue broadcast, the Rust-only error cause, the Attention and Incident commands |
| 7 | Camera sources, webcam lookup, `FrameFetcher`, health, the camera commands except capture |
| 8 | The media root, capture, retention, pinning, the sweep, the snapshot commands, optionality |
| 9 | Notifications, focus, sinks, activation, `farm3d-navigate-v1`, settings columns and schema 3 |
| 10 | Camera and alert defaults in single and batch create, Printers schema 4 |
| 11 | D8 guards, delete-time resolution, `EVIDENCE_EXISTS` |
| 12–15 | Frontend |
| 16 | Tracer, simulator camera, read-only U1 probe |
| 17 | Installed-bundle pass (owner at the desktop) |
| 18 | Verification record |

## Decisions made in this spec

Each departs from, or sharpens, the plan's Design reference.

1. **`printer.offline` covers an unreachable `error`.** Code fact: an
   unreachable Moonraker host is `ConnectionState::Error` ("could not be
   reached"), not `offline`. `printer.connectionError` is authentication or
   protocol failure only.
2. **A Rust-only `ConnectionErrorCause`** on the status map tells the two
   apart without matching message strings; `PrinterStatus` is unchanged.
3. **Recurrence is per Condition.** `job.failed`, `job.hostCancelled`, both
   `requirement.*`, and `job.completed` are once per source; a terminal
   Job would otherwise raise `job.failed` again after the operator
   resolved it.
4. **`job.failed` and `job.hostCancelled` need a tracker-proven end**;
   declared ends are the operator's own act.
5. **The attention epoch** (0009's `applied_at`) stops P7-era Job ends from
   flooding the center at upgrade. Requirements project whatever their
   age.
6. **`job.startConfirmation`** is raised only when the operator must start
   the Job (Start-safety `confirmBedClear`, or a refused unattended
   start).
7. **"Covered by a Job" is by file, and it suppresses the Event too.** A
   host failure is covered only when the reported file is the latest
   Job's `hostPath` and farm3d started that Job (`starting`, `printing`,
   `paused`, `outcomeUnknown`, or ended with `startedAt`; narrowed by 34
   to a Job that can still carry the failure). While covered,
   `printer.hostFailed` raises neither an Incident nor an Event. This
   deliberately refines planner default 4, which suppressed only the
   Incident ("yes, only when no farm3d Job was active"): the Job's own
   `job.failed` (or `requirement.jobOutcomeUnknown`) carries the same
   failure as a fatal, actionable Event with its Incident, so a second
   Event would duplicate it. A Job that is merely assigned or staged never
   covers a failure (fixtures s28, s29).
8. **Renames:** the table `snapshots` is `camera_snapshots` and the type
   `Snapshot` is `CameraSnapshot` (the codebase already has DB
   `snapshot_root`, `printer_status_snapshots`, and backfill "snapshot"
   types); the backfill type `AttentionSnapshot` is `AttentionBackfill`;
   the stream envelope is `AttentionStreamEvent`; `BatchRowInput`'s
   `camera_endpoint` is `cameraHostOverride` (a host string).
9. **The Incident timeline merges `job_events` at read time.** The plan's
   `operatorAction { jobEventId }` kind and `incident_events.job_event_id`
   are dropped; `reopened` is added; `attention_event_id` and
   `snapshot_id` columns keep references checkable.
10. **Completion evidence is recorded on the `job.completed` Event's own
    `evidence` field** (`attention_events.evidence_json`), not in its
    planner-owned `detail` (which the next pass would overwrite) and not
    in `job_events` (so P7's closed timeline gets no new kinds).
11. **`evidenceSkipped` means "tried and couldn't"**: nothing is recorded
    when the toggle is off, there is no source, or the Incident came from
    the backfill.
12. **Wakes** drop host-operation changes and host facts (no Condition reads
    them) and add a projector poke at every `PrinterChanged` site.
13. **An unchanged amendment bumps no revision** and emits nothing.
14. **A system resolution also marks the Event read** (the umbrella's
    "resolving implies read").
15. **Offline alerts "off" makes `printer.offline` absent**, resolving an
    open one.
16. **Media root** is `<app data>/farm3d-media/v1`, mirroring
    `farm3d-content/v1`; no cleanup-intent table (the sweep is the
    backstop).
17. **`EVIDENCE_EXISTS`** guards Printer import, since `replace_all` would
    hit `RESTRICT` on Incidents and snapshots mid-import.
18. **22 commands**: `get_printer_alert_defaults` is added.
19. **Binary frames carry a JSON header** (content type, capture time), so
    the frontend never sniffs bytes.
20. **Camera and alert setters take no `expectedRevision`.**
21. **`save_settings` gains optional objects**; absent keeps the stored
    value.
22. **`NotificationSink` is async**, since zbus is.
23. **`CameraErrorKind` gains `noSnapshotUrl` and `webcamListFailed`.**
24. **`test_camera` and `list_host_webcams` accept an unsaved Connection**
    for the Setup wizard and batch rows.
25. **A manual capture links the Printer's active Job**, if any.
26. **`SNAPSHOT_DISK_CAP`** reports a manual capture that the cap refuses.
27. **The host-webcam lookup is a crate-private sibling trait**, because a
    public trait can't have a crate-private method.
28. **The Printer Status tab lists the Printer's open Events** (umbrella
    "Printer detail dock"), and the Camera tab is appended after Job.
29. **`printer.hostFailed` and `job.completed` need a live status**, so the
    backfill treats them as unknown, like the other `printer.*` families.
30. **(Fix round 1) `printer.connectionError` is `Unknown`, not `Absent`,
    while the host is unreachable** (offline, connecting, hydrated, or an
    unreachable/timeout error): none of those shows whether the
    credentials are still wrong. Otherwise a restart (statuses hydrate
    `offline`, then `connecting`) would resolve an open auth failure and
    the next failure would insert a live recurrence that notifies. It is
    `Absent` only when the host is reachable or the Printer is archived or
    Setup incomplete.
31. **(Fix round 1) Only `get_printer_camera` returns a manual URL.**
    `set_printer_camera` returns `PrinterCameraSummary` (URL redacted);
    the Printers export file carries the URL as it carries the Connection
    host.
32. **(Fix round 1) Pruned snapshots never count toward
    `PINNED_EVIDENCE_EXISTS`**, the Printer delete removes pruned
    unattached manual rows with the unpinned ones, and unpinning a pruned
    row is allowed, so a pinned image that went missing can't make a
    Printer undeletable.
33. **(Fix round 1) One `attention.incident.changed` per Incident per
    commit**, and every timeline append bumps the Incident's revision.
34. **(Controller ruling, Task 4 review) A Job covers a host failure only
    while it can still carry it.** Decision 7's "or ended with
    `startedAt`" is narrowed: the latest Job covers a failure on its file
    only while it is `starting`, `printing`, `paused`, or
    `outcomeUnknown`, or after the tracker ended it `failed` (its
    `job.failed` is the carrier). A latest Job that ended `completed`,
    `cancelled`, or by a declaration never covers one, so a failed
    reprint of the same file from the host's own UI raises
    `printer.hostFailed` and its Incident instead of nothing.

## Residual risks

- **The Wayland raise depends on the daemon** (to be measured by Task 2 and
  Task 17). Without an `ActivationToken`, a click may only flash the
  taskbar entry; navigation still happens.
- **A GTK file dialog counts as unfocused**, so a notification can fire
  while the operator is in farm3d's own dialog.
- **A host-webcam absolute URL is compared by host string.** A Connection
  by hostname and a webcam URL by address (or the reverse) is refused
  `hostMismatch`; the operator uses a manual URL instead. Only the U1's
  layout is evidenced.
- **Relative webcam URLs assume the web frontend on port 80** unless
  `webPort` says otherwise.
- **`list_host_webcams` queries live**, so an offline Printer can't list
  webcams in Setup; the saved name still works once it is back.
- **`job.completed` needs the host to still report `finished` with the
  Job's file** when the projector sees the completion. A host reset in
  between raises no completion Event (and no completion capture).
- **An offline grace restarts at every farm3d restart** (the watch is in
  memory); with the startup window, an offline Event can open up to one
  grace later than a continuous run would.
- **Settings export v3 and Printers export v4** are schema bumps P9's backup
  format must absorb.
- **Notes are verbatim operator text.** farm3d never writes a URL into
  one, but an operator can.
