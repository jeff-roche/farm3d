# P7 Queue Entries, Jobs, and Dispatch Design

## Status

Draft for controller review, 2026-09-26.

This is the focused design for GitHub issue #17 (P7). It is Task 1 of
`docs/superpowers/plans/2026-09-26-p7-queue-entries-jobs-dispatch.md`.
It turns the plan's Design reference D1–D9 into final decisions and binds
Tasks 2–17. Where this spec and the plan differ, this spec wins. Task 1
also edited the plan wherever this spec changed a name or signature a
later task uses.

ADR-0013 (`docs/adr/0013-queue-and-job-state-machines.md`) records the two
hard-to-reverse choices: the Job owns dispatch while Host Operations stay
the only write path, and eligibility is derived, never stored.

The owner's ten decisions in the plan ("Owner decisions (fixed)") are
fixed. This spec applies them and does not reopen them. The ones that
shape it most:

- **Decision 1:** unattended start ships, only from `Ready`, only on a
  Printer whose Start-safety rule is `unattended`.
- **Decision 2:** anything automatic needs `upload`, `start`, `hostState`,
  and `artifactIdentity` supported with `tier: sim` evidence.
- **Decision 4:** a Queue Entry can't exist without a material estimate.
- **Decisions 5 and 6:** a completed Job deducts its estimate in the same
  transaction; a failed or cancelled one needs a settlement.
- **Decision 9:** P6's raw controls step aside while a Printer has an
  active Job, and farm3d never adopts a print it didn't start.

### Fail-safe principle

P6's principle still holds: farm3d never repeats a write it can't prove
did not happen. P7 adds one rule on top:

> A Job's **end** (completed, failed, or cancelled) comes only from
> farm3d's own committed records (an operator command, a Host
> Operation's proved outcome) or from the host's print history for the
> exact history job farm3d pinned. A status string is a hint that
> triggers a check, never proof of an end. When farm3d can't prove how a
> print ended, the Job becomes **Outcome unknown**, or, if the host can't
> be reached at all, the operator may declare the end after 30 minutes
> (D9), and the operator decides.

One exception is deliberate: `printing` ⇄ `paused` also follows the
live status on the Job's own file (D7). Both are active, reversible
states, neither moves material, and the next status or history read
corrects them.

## Goal

An operator can:

- **Add** a Slice Revision to the Queue, one or many copies, each with a
  Dispatch Policy, a dispatch preference, and a fixed material estimate.
- See **why** each Queue Entry can or can't run, and which Printers
  qualify, in ranked order.
- **Assign** an entry to a Printer and a Spool. The Job and the Spool
  reservation are created atomically.
- Have farm3d **stage** the Job at once, then **start** it behind the
  bed-clear confirmation (or unattended, per decision 1), then **pause**,
  **resume**, and **cancel** it through the Job.
- Trust that farm3d **tracks** the print to its end from the host's
  history, and survives a restart at every step.
- **Settle** the material of a failed or cancelled Job once: the
  estimate, a measured weight, or defer. A completed Job deducts by
  itself, with an optional measured correction.
- **Release** a Job before it starts, **retry** a finished Job, and
  **reorder** the Queue, without losing lineage or history.

## Scope

### In scope

- The `queue_entries`, `jobs`, `job_events`, and
  `reconciliation_requirements` tables, and the `host_operations.job_id`
  link (migration 0008).
- The Queue Entry and Job state machines, the eligibility gates and
  ranking, the automatic evaluator, the dispatch driver, the outcome
  tracker, settlement, and startup recovery.
- The 18 commands and the `queue` event stream.
- Lifecycle guards on Printers, Slice Revisions, Printer import, and
  Connection changes.
- The Spool `reconciliation` facet and the P3 reservation extensions.
- Frontend: the Queue destination, Queue screen, Add to Queue, Job views,
  the Monitor Queue preview, and the `ReorderHandle` control.
- The tracer on `FakeMoonraker` (CI) and the simulator (evidence).

### Non-goals

- Attention Events, Incidents, cameras, and notifications (P8). P7 keeps
  durable Reconciliation Requirements that P8 will project.
- History search, export, and pruning (P9).
- OctoPrint or ElegooLink command capabilities. OctoPrint stays
  `notVerified`, so no OctoPrint Printer can take a Job.
- Automatic retry, automatic re-staging after a failure, and multi-Spool
  Jobs (decision 3).
- Adopting a print farm3d didn't start (decision 9).
- Changing a Queue Entry's Slice Revision or estimate after creation
  (decision 4). Remove and add again instead.

## Product vocabulary

`CONTEXT.md` gains or updates these entries (Task 1 wrote them):

- **Queue Entry** (updated): one requested physical run of one Slice
  Revision, with a queue position, a Dispatch Policy, a dispatch
  preference, and a fixed material estimate. It becomes a Job only by
  assignment.
- **Job** (updated): one Queue Entry assigned to one Printer with one
  reserved Spool, tracked from assignment to its end. Its state changes
  only from farm3d's records and proved host history.
- **Dispatch Policy** (updated): Manual, Recommended, or Automatic.
- **Dispatch preference:** how Automatic and Recommended rank Printers:
  `loadedFirst` or `leastRecentlyUsed`.
- **Lineage:** the group of Queue Entries one Add to Queue created, plus
  their retries and release replacements.
- **Settlement:** how a Job's reserved material becomes a deduction.
- **Reconciliation Requirement:** a durable, identified thing the operator
  must settle: a failed or cancelled Job's material, or a Job whose
  outcome is unknown.
- **Awaiting material:** an `awaitingStart` Job whose Spool isn't loaded
  on its Printer.
- **Outcome unknown:** a Job whose end farm3d could not prove.

## Decisions

### D1. Vocabulary and ownership

**Decision: two Rust modules, `queue` and `jobs`, with the ownership
below. Rust owns every state; the frontend presents it.**

| Owner | Owns |
|---|---|
| `queue` | Queue Entries, order, lineage, eligibility, ranking, the automatic evaluator, the `queue` event stream |
| `jobs` | Jobs, the Job timeline (`job_events`), assignment, release, retry, the dispatch driver, the tracker, settlement, Reconciliation Requirements, recovery, lifecycle guards |
| `host_ops` (P6) | Every write to a host. `jobs` hands work to it through the Job-linked write-ahead (D4) and never talks to a host itself, except for the tracker's read-only history query |
| `spools` (P3) | Reservations and the ledger. `jobs` calls its primitives inside its own transactions |

- **Queue Entry** ids are `qen-<uuid v4>`. **Lineage** ids are
  `qln-<uuid v4>`. **Job** ids are `job-<uuid v4>`. **Job event** ids are
  `jev-<uuid v4>`. **Reconciliation Requirement** ids are
  `rrq-<uuid v4>`.
- **Lineage.** One `add_to_queue` with quantity N creates N entries with
  one new `lineage_id` and `copy_index` 1..N. A retry or a release
  replacement joins its origin's lineage, **keeps its origin's
  `copy_index`**, and records `origin_entry_id` and `origin_kind`
  (`retry` or `release`). The UI's "Copy 2 of 3" is `copyIndex` of
  `copyCount`, where `copyCount` is the lineage's highest `copy_index`.
  Lineage only groups. It never merges state: every entry is assigned,
  released, retried, cancelled, and settled on its own.
- **Reconciliation Requirement.** A durable row with a stable id, one per
  `(job, kind)`:
  - `materialReconciliation`: a failed or cancelled-after-start Job's
    material needs settling. `pending` → `deferred` → `resolved`, or
    `pending` → `resolved`.
  - `jobOutcomeUnknown`: a Job is `outcomeUnknown`. `pending` →
    `resolved` (by `declare_job_outcome`). It can't be deferred.

  P8 projects these into Attention. P7 emits no notification.

### D2. Queue Entry state machine

**States:** `queued`, `assigned`, `closed`. A closed entry has a
`closeReason`: `completed`, `failed`, `cancelled`, `released`, or
`removed`.

**Events** (`queue::state::EntryEvent`): `Assign`, `Remove`,
`JobTerminal(CloseReason::Completed | Failed | Cancelled)`, and
`Release`. Retry is not an entry event. It creates a new entry and leaves
its origin untouched.

**Legal and illegal transitions.** Every pair, 3 states × 6 events. "User"
rows are reachable from a command, and "internal" rows only from farm3d's
own code; an internal illegal pair is a bug and maps to `INTERNAL`.

| From | Event | To | Rejection if illegal | Reached from |
|---|---|---|---|---|
| `queued` | `Assign` | `assigned` | — | user (`assign_queue_entry`), evaluator |
| `queued` | `Remove` | `closed{removed}` | — | user (`remove_queue_entry`) |
| `queued` | `JobTerminal(completed)` | illegal | `INTERNAL` | internal |
| `queued` | `JobTerminal(failed)` | illegal | `INTERNAL` | internal |
| `queued` | `JobTerminal(cancelled)` | illegal | `INTERNAL` | internal |
| `queued` | `Release` | illegal | `INTERNAL` | internal |
| `assigned` | `Assign` | illegal | `QUEUE_ENTRY_ACTION_NOT_ALLOWED` | user |
| `assigned` | `Remove` | illegal | `QUEUE_ENTRY_ACTION_NOT_ALLOWED` ("Release or cancel its Job instead.") | user |
| `assigned` | `JobTerminal(completed)` | `closed{completed}` | — | tracker, declare |
| `assigned` | `JobTerminal(failed)` | `closed{failed}` | — | tracker, declare |
| `assigned` | `JobTerminal(cancelled)` | `closed{cancelled}` | — | tracker, declare, `cancel_job` before start |
| `assigned` | `Release` | `closed{released}` | — | `release_job` |
| `closed` | `Assign` | illegal | `QUEUE_ENTRY_ACTION_NOT_ALLOWED` | user |
| `closed` | `Remove` | illegal | `QUEUE_ENTRY_ACTION_NOT_ALLOWED` | user |
| `closed` | `JobTerminal(completed)` | illegal | `INTERNAL` | internal |
| `closed` | `JobTerminal(failed)` | illegal | `INTERNAL` | internal |
| `closed` | `JobTerminal(cancelled)` | illegal | `INTERNAL` | internal |
| `closed` | `Release` | illegal | `INTERNAL` | internal |

`update_queue_entry` (policy, preference) is allowed only on a `queued`
entry. `move_queue_entry` is allowed on any open entry (`queued` or
`assigned`). Both are refused on any other with
`QUEUE_ENTRY_ACTION_NOT_ALLOWED`. Neither changes the state.

**Retry precondition.** `retry_job(jobId)` needs a terminal Job whose
entry is `closed{completed | failed | cancelled}` with a `cancelReason`
other than `releasedBeforeStart`. A released entry already has its
replacement in the Queue, so retrying it would request a second run.
This departs from the plan's D2, which also listed `released`.

For the same reason, **a Job is retried at most once**. A second
`retry_job` for the same Job (with a new `operationId`) fails with
`JOB_ALREADY_RETRIED` and names the existing retry entry. The newest
entry in the chain can be retried in turn. A partial unique index on
`queue_entries(origin_entry_id)` enforces "one successor per entry" for
both retries and release replacements.

**Position.**

- `position` is dense, 1..n, over open (`queued` and `assigned`) entries.
  Closed entries have `position = NULL`. A partial **unique** index
  enforces it.
- New entries (Add to Queue, retry) go to the end, in `copy_index` order.
- Closing an entry (a Job terminal, remove) sets its position to NULL and
  moves every later open entry up by one, in the same transaction.
- **Release** first closes the released entry (position NULL), then
  inserts the replacement at the freed position. Nothing else moves. Both
  happen in the release transaction (controller ruling).
- `move_queue_entry(toPosition)` takes 1..n and renumbers the entries
  between the old and new positions by one.
- **Renumbering always parks first.** The positions are `CHECK (position
  >= 1)` and a partial UNIQUE index, and SQLite checks both row by row, so
  neither negating nor a single `position = position ± 1` works, and no
  row order is promised. Every renumber (move, close, remove, release,
  and a Job reaching a terminal state) is therefore:
  1. Free the slot the change needs: a closing entry's position becomes
     NULL; a moving entry is parked at `position + 1000000`.
  2. Park every other entry that must shift:
     `UPDATE queue_entries SET position = position + 1000000 WHERE
     position BETWEEN ?lo AND ?hi`. Its targets (1000001 and up) are all
     free, so no row collides.
  3. Write the final values from the parked ones, either in one statement
     (`SET position = position - 1000000 ± 1 WHERE position > 1000000`,
     whose targets were freed in step 1 and 2) or one row at a time, then
     set the moving entry's final position.

  This needs fewer than 1 000 000 open entries, which `add_to_queue`'s
  1..50 quantity and a single operator make safe. A test fills a queue
  and checks every renumber path (`renumber_paths_never_violate_the_unique_position_index`).
- Assignment is sticky. An assigned entry keeps its position, and moving
  it changes only its place in the list, never its Printer.

**Eligibility is derived** (D5). It is never a column and never a state.

### D3. Job state machine

**States** (`JobState`): `assigned`, `staging`, `awaitingStart`,
`starting`, `printing`, `paused`, `completed`, `failed`, `cancelled`,
`outcomeUnknown`. **Terminal:** `completed`, `failed`, `cancelled`.
**Active:** every other state. A partial unique index allows at most one
active Job per Printer.

**Cancel reason** (`CancelReason`, set exactly when the state is
`cancelled`): `releasedBeforeStart`, `cancelledBeforeStart`,
`cancelledByOperator` (a farm3d `cancel_job` after start, proved by
history), `hostCancelled` (cancelled on the printer, proved by history),
`operatorDeclared` (declared from `outcomeUnknown`).

**Events** (`JobEventKind`). The Job's insert writes an `assigned` event
row with no `from_state`, and is not a transition. Every other kind is:

| Event | From | To | Raised by |
|---|---|---|---|
| `StageHandedOff` | `assigned`, `awaitingStart` | `staging` | driver after assign, `stage_job` (in the write-ahead tx). From `awaitingStart` it stages again, for example after a Connection endpoint change (D8) or `STAGED_ARTIFACT_INVALID` |
| `StageSucceeded` | `staging` | `awaitingStart` | `apply_host_outcome`: upload `succeeded` |
| `StageFailed` | `staging` | `assigned` | `apply_host_outcome`: upload `failed` or `abandoned` |
| `StartHandedOff` | `awaitingStart` | `starting` | `start_job`, driver's unattended start (write-ahead tx) |
| `StartSucceeded` | `starting` | `printing` | `apply_host_outcome`: start `succeeded` |
| `StartFailed` | `starting` | `awaitingStart` | `apply_host_outcome`: start `failed` |
| `StartAbandoned` | `starting` | `outcomeUnknown` | `apply_host_outcome`: start `abandoned` |
| `HostJobPinned` | `printing`, `paused` | unchanged | tracker |
| `PauseHandedOff` | `printing` | `printing` | `pause_job` (write-ahead tx) |
| `ResumeHandedOff` | `paused` | `paused` | `resume_job` (write-ahead tx) |
| `CancelHandedOff` | `printing`, `paused` | unchanged | `cancel_job` after start (write-ahead tx) |
| `ControlFailed` | `printing`, `paused` | unchanged | `apply_host_outcome`: pause/resume/cancel `failed` or `abandoned` |
| `Paused` | `printing` | `paused` | `apply_host_outcome`: pause `succeeded`; tracker observing our file paused |
| `Resumed` | `paused` | `printing` | `apply_host_outcome`: resume `succeeded`; tracker observing our file printing |
| `Completed` | `printing`, `paused` | `completed` | tracker (history `completed`) |
| `Failed` | `printing`, `paused` | `failed` | tracker (history failure status) |
| `Cancelled` | `printing`, `paused` | `cancelled` | tracker (history `cancelled`) |
| `OutcomeUnknown` | `printing`, `paused` | `outcomeUnknown` | tracker (unprovable, D7) |
| `DeclaredCompleted` | `outcomeUnknown`, `printing`, `paused` | `completed` | `declare_job_outcome` (from `printing` or `paused` only while the host is unreachable, D9) |
| `DeclaredFailed` | `outcomeUnknown`, `printing`, `paused` | `failed` | as above |
| `DeclaredCancelled` | `outcomeUnknown`, `printing`, `paused` | `cancelled` | as above |
| `Released` | `assigned`, `awaitingStart` | `cancelled` | `release_job` |
| `CancelledBeforeStart` | `assigned`, `awaitingStart` | `cancelled` | `cancel_job` before start |
| `MaterialSettled` | `failed`, `cancelled` | unchanged | `settle_job_material` (estimated, measured) |
| `MaterialDeferred` | `failed`, `cancelled` | unchanged | `settle_job_material` (defer) |
| `MaterialCorrected` | `completed` | unchanged | `correct_job_material` |

Every `(state, event)` pair not in this table is illegal and writes
nothing. `jobs::state::transition` rejects it before any SQL with
`IllegalTransition`. The code a caller sees depends on who raised it:

| Illegal event raised by | Code |
|---|---|
| a user command (`StageHandedOff` from `stage_job`, `StartHandedOff` from `start_job`, `PauseHandedOff`, `ResumeHandedOff`, `CancelHandedOff`, `Released`, `CancelledBeforeStart`, the three `Declared*`, `MaterialSettled`, `MaterialDeferred`, `MaterialCorrected`) | `JOB_ACTION_NOT_ALLOWED` with `{ jobId, action, state }` |
| the driver, tracker, `apply_host_outcome`, or recovery | none: the event is dropped as already applied (D4 idempotency). It is logged only if the Job is in a state the event could never follow. |

`Settlement` rules (`MaterialSettled` on a `settled` Job, and so on) are
checked separately, in D4's settlement rules, and have their own codes.

**Job actions by state.** `Job.allowedActions` (Rust-computed) lists what
the frontend may offer. A command outside this table fails with
`JOB_ACTION_NOT_ALLOWED`.

| State | `stage` | `start` | `pause` | `resume` | `cancel` | `release` | `retry` | `declareOutcome` | `settleMaterial` | `correctMaterial` |
|---|---|---|---|---|---|---|---|---|---|---|
| `assigned` | yes | — | — | — | yes (before start) | yes | — | — | — | — |
| `staging` | — | — | — | — | — | — | — | — | — | — |
| `awaitingStart` | yes (stage again) | yes | — | — | yes (before start) | yes | — | — | — | — |
| `starting` | — | — | — | — | — | — | — | — | — | — |
| `printing` | — | — | yes | — | yes (after start) | — | — | if unreachable ≥ 30 min | — | — |
| `paused` | — | — | — | yes | yes (after start) | — | — | if unreachable ≥ 30 min | — | — |
| `outcomeUnknown` | — | — | — | — | — | — | — | yes | — | — |
| `completed` | — | — | — | — | — | — | yes, once | — | — | if not yet corrected |
| `failed` | — | — | — | — | — | — | yes, once | — | if `pending` or `deferred` | — |
| `cancelled` | — | — | — | — | — | — | yes, once, unless `releasedBeforeStart` | — | if `pending` or `deferred` | — |

- In `assigned`, `stage` is offered only while `lastFailure` is set or no
  stage has been handed off (the driver stages once by itself, D7).
- `start` is always listed in `awaitingStart`, and `startBlockers` says
  whether it can succeed now.
- "Unreachable ≥ 30 min" means `hostUnreachableSince` is at least
  `JobTimings.unreachable_declare_after` (30 min) before now (D7, D9).
  `allowedActions` is computed at read time, and the tracker republishes
  the Job when it crosses that mark.
- `retry` disappears once the Job has been retried (`JOB_ALREADY_RETRIED`).
- **Settle on a settled Job.** A `settleMaterial` or `correctMaterial`
  command is checked against the settlement before the state table. If
  `settlement` is `settled` (a completed Job, or a failed or cancelled one
  already settled), `settle_job_material` fails with
  `JOB_ALREADY_SETTLED`. A second correction fails the same way. Only
  when settlement is `open` or `notRequired`, or a correction targets a
  non-completed Job, is it `JOB_ACTION_NOT_ALLOWED`.
- **`staging` and `starting` offer no Job action.** Their exit is the
  linked Host Operation. P6's `reconcile_host_operation` (**Check again**)
  and `abandon_host_operation` (**Abandon check…**) are **not** guarded
  by `JOB_ACTIVE`, and `JobPanel` shows both for the Job's active Host
  Operation, with P6's own enabling rules. Abandoning an upload takes the
  Job `staging` → `assigned` (`StageFailed`). Abandoning a start takes it
  `starting` → `outcomeUnknown` (`StartAbandoned`). The same holds for an
  uncertain pause, resume, or cancel op of a `printing` or `paused` Job
  (`ControlFailed`).

**Awaiting material** is not a state. `Job.startBlockers` is
Rust-computed on every read of an `awaitingStart` Job (D7), and the UI
labels a `SPOOL_NOT_LOADED` blocker "Awaiting material".

**Settlement** (`Settlement`) is orthogonal to the state:

| Settlement | Meaning | Set by |
|---|---|---|
| `open` | The Job is active. The reservation is `active`. | Job insert |
| `notRequired` | Cancelled before start. The reservation was released. | `Released`, `CancelledBeforeStart` |
| `settled` | The reservation is consumed. `settlementMethod` is `estimated` or `measured`. | `Completed`, `DeclaredCompleted` (estimated); `MaterialSettled` |
| `pending` | Failed or cancelled after start. The reservation is `unresolved`, and a `materialReconciliation` requirement is `pending`. | `Failed`, `Cancelled`, `DeclaredFailed`, `DeclaredCancelled` |
| `deferred` | As `pending`, and the operator deferred. The requirement is `deferred`. | `MaterialDeferred` |

`jobs::state::settlement_after(event) -> Option<Settlement>` gives the
settlement a state-changing event sets, and `None` for events that leave
it alone. `open` is new compared with the plan's D3. The plan had no
value for an active Job, and `notRequired` would have been false for one.

While settlement is `pending` or `deferred`, the reservation is
`unresolved`. Its amount stays unavailable, and the Spool shows the
`reconciliation` facet.

### D4. Transaction boundaries and operation ids

**Decision: every durable transition is one `Storage::write_repo`
transaction that also claims the command's operation id. Events and
in-process broadcasts go out after commit, never on a replay.**

| Boundary | One transaction contains | Lock |
|---|---|---|
| Add to Queue | claim `addToQueue`; validate the revision and estimate; N entries at the end, one lineage | none |
| Update | claim `updateQueueEntry`; expected-revision check; the entry row (revision + 1) | none |
| Move | claim `moveQueueEntry`; expected-revision check; the renumbered rows (each revision + 1) | none |
| Remove | claim `removeQueueEntry`; expected-revision check; entry → `closed{removed}`; renumber | none |
| Assign | claim `assignQueueEntry`; the entry is `queued`; the Printer has no active Job; eligibility re-evaluated **inside the tx** by `check_assignment` (D5); `reservations::reserve` with holder `("job", jobId)`; Job insert (`assigned`, settlement `open`) and its `assigned` event; entry → `assigned` with `job_id` | Printer |
| Stage, start, pause, resume, cancel after start (handoff) | P6's write-ahead: its ledger claim (the Host Operation's derived id), the Connection and unresolved-row re-checks, `insert_dispatching` with `job_id` — **and**, through `LinkInTx`, the Job command's own claim, the Job re-check, the Job transition, and its event | Printer (taken by `host_ops::api`) |
| Host outcome → Job | `jobs::dispatch::apply_host_outcome(tx, &op)`: the Job transition, its event, and for `StartAbandoned` the `jobOutcomeUnknown` requirement. Idempotent: if the Job has already moved past the event, it writes nothing. | Printer |
| Tracker terminal | Job → terminal with its event; entry → `closed`; renumber. Completed: `reservations::consume(estimate)`, settlement `settled/estimated`. Failed or cancelled: `reservations::mark_unresolved`, settlement `pending`, a `materialReconciliation` requirement (`pending`). | Printer |
| Tracker progress, pin, reachability | `max_progress_pct`, `inconclusive_checks`, `host_unreachable_since`, `host_job_id` (the pin writes `HostJobPinned`; the others write no event) | Printer |
| `OutcomeUnknown` (tracker) | Job → `outcomeUnknown`, event, `jobOutcomeUnknown` requirement | Printer |
| Declare | claim `declareJobOutcome`; Job → terminal; entry → `closed`; renumber; the `jobOutcomeUnknown` requirement, if any, → `resolved` with `{ kind: "declared", outcome }`; then exactly the tracker-terminal settlement work for that outcome | Printer |
| Settle / defer | claim `settleJobMaterial`; estimated: `consume(ceil(estimateMg × maxProgressPct / 100))`; measured: `consume_measured(entry)`; defer: nothing on the reservation; the Job's settlement; the requirement's status; the event | Printer |
| Correct | claim `correctJobMaterial`; one `Measurement` ledger row that references the reservation and is marked as a correction; `correction_event_id`; the event | Printer |
| Release | claim `releaseJob`; Job → `cancelled{releasedBeforeStart}`, settlement `notRequired`; `reservations::release`; entry → `closed{released}` (position NULL); the replacement entry inserted at the freed position | Printer |
| Cancel before start | claim `cancelJob`; Job → `cancelled{cancelledBeforeStart}`, settlement `notRequired`; `release`; entry → `closed{cancelled}`; renumber | Printer |
| Retry | claim `retryJob`; the Job has no retry yet (`JOB_ALREADY_RETRIED`); a linked entry (`origin_kind = retry`) at the end | none |

**The printer lock.** `host_ops::services.printer_lock(printer_id)` is a
non-reentrant `tokio::sync::Mutex`. Job writes that hand off to P6 call
`host_ops::api::{stage, start, control}`, which take the lock
themselves, so the Job code **must not** hold it around that call. Every
other Job write above takes it first. The evaluator's assignment does
too.

**Operation ids.**

| Write | Ledger id claimed by the Job side | `OperationKind` | Host Operation's `operation_id` (claimed by P6) |
|---|---|---|---|
| Queue and Job commands | the client's `operationId` | the command's kind (below) | — |
| `stage_job`, `start_job`, `pause_job`, `resume_job`, `cancel_job` after start | the client's `operationId` | `stageJob`, `startJob`, `pauseJob`, `resumeJob`, `cancelJob` | `<operationId>#hostOperation`, with P6's own kind (`stageSliceRevision`, and so on) |
| The driver's stage after assignment, or after a restart (D7) | `drv-<uuid v4>` | `stageJob` | `drv-<uuid v4>#hostOperation` |
| The driver's unattended start | `drv-<uuid v4>` | `startJob` | `drv-<uuid v4>#hostOperation` |
| The evaluator's assignment | `auto-<uuid v4>` | `assignQueueEntry` | — |

Two ids are needed because P6's `insert_dispatching` claims the Host
Operation's `operation_id` in the same ledger, and one id can't be claimed
under two kinds. The derived suffix is deterministic, so a replayed Job
command finds the same Host Operation.

**Replay.** A Job command checks the ledger read-only first, the way P6's
`is_replay` does. On a replay with the same kind and digest, it returns
the current rows (`QueueChange`) with no pre-check, no host call, and no
event. A reused id with a different request is `VALIDATION` on
`operationId` (`RepositoryError::OperationIdReused`, as P3–P6 map it). No
`OPERATION_ID_REUSED` code exists, and none is added. A rejected command
rolls back and never burns its id.

`OperationKind` gains 15 camelCase variants: `addToQueue`,
`updateQueueEntry`, `moveQueueEntry`, `removeQueueEntry`,
`assignQueueEntry`, `stageJob`, `startJob`, `pauseJob`, `resumeJob`,
`cancelJob`, `releaseJob`, `retryJob`, `declareJobOutcome`,
`settleJobMaterial`, `correctJobMaterial`.

**Ledger digests** (a struct, fields in this order):

| Command | Digest |
|---|---|
| `add_to_queue` | `{ sliceRevisionId, quantity, policy, preference, materialEstimate, manualPrinterId }` |
| `update_queue_entry` | `{ entryId, expectedRevision, policy, preference }` |
| `move_queue_entry` | `{ entryId, expectedRevision, toPosition }` |
| `remove_queue_entry` | `{ entryId, expectedRevision }` |
| `assign_queue_entry` | `{ entryId, printerId, spoolId, acknowledgeManualFacts, assignedBy }` |
| `stage_job`, `pause_job`, `resume_job`, `release_job`, `retry_job`, `cancel_job` | `{ jobId }` |
| `start_job` | `{ jobId, priorState, acknowledgement }` |
| `declare_job_outcome` | `{ jobId, outcome, acknowledgement }` |
| `settle_job_material` | `{ jobId, choice }` |
| `correct_job_material` | `{ jobId, entry }` |

**Restart matrix.** One row per durable transition. "Rebuild" means
dropping `RuntimeServices` at the crash point and rebuilding it over the
same roots (`p6_tracer.rs`'s pattern). Every row also asserts **no
duplicate upload and no unconfirmed start** from `FakeMoonraker`'s
request counts. Tests live in `src-tauri/tests/p7_restart_matrix.rs`.

| # | Crash point | State on disk | Startup outcome | Test |
|---|---|---|---|---|
| R1 | Inside the assign tx, before commit | nothing: the entry is `queued`, no Job, no reservation, the id not burned | unchanged; the same `operationId` can assign again | `restart_inside_assign_rolls_back_everything` |
| R2 | After assign committed, before the driver's stage | Job `assigned`, no `lastFailure`, no upload op; reservation `active`; entry `assigned` | the driver stages once the runtime starts (one upload) | `restart_after_assign_stages_once` |
| R3 | Stage write-ahead committed, executor not yet sent (`dispatched_at` NULL) — P6 covers the Host Operation half | Job `staging`; upload op `dispatching`, unsent | P6 recovery: op `failed{neverSent}`; Job recovery: `StageFailed`, Job `assigned` with `lastFailure`; nothing re-staged by itself | `restart_before_upload_send_returns_the_job_to_assigned` |
| R4 | Upload sent, response lost | Job `staging`; upload op `dispatching`, sent | op `uncertain{interruptedByRestart}`; Job stays `staging`; P6 reconciles → `succeeded` → Job `awaitingStart` | `restart_during_upload_reconciles_then_awaits_start` |
| R5 | Upload `succeeded` committed, `apply_host_outcome` not run | Job `staging`; upload op `succeeded` | Job recovery applies it: Job `awaitingStart`, `upload_host_operation_id` set | `restart_after_upload_success_applies_the_outcome` |
| R6 | Start write-ahead committed, unsent | Job `starting`; start op `dispatching`, unsent | op `failed{neverSent}`; Job `StartFailed` → `awaitingStart` with `lastFailure`; no start sent | `restart_before_start_send_returns_to_awaiting_start` |
| R7 | Start sent, reply lost | Job `starting`; start op `dispatching`, sent | op `uncertain`; Job stays `starting`; P6 reconciles from print state or history → `succeeded` → Job `printing`; **no second start** | `restart_after_start_reply_lost_reconciles_without_a_second_start` |
| R8 | Start `succeeded` committed, `apply_host_outcome` not run | Job `starting`; start op `succeeded` | Job `printing`, `started_at` set; the tracker pins the history job | `restart_after_start_success_applies_the_outcome` |
| R9 | During printing | Job `printing`, maybe unpinned; `max_progress_pct` last persisted | the tracker re-checks history on its first pass; the Job stays `printing` and pins if unpinned | `restart_during_printing_keeps_tracking` |
| R10 | Host completed; farm3d closed before the tracker saw it | Job `printing`; history job `completed` | first tracker pass: Job `completed`, estimate consumed once, entry closed | `restart_after_host_completed_completes_once` |
| R11 | Host cancelled or failed; farm3d closed before the tracker saw it | Job `printing`; history `cancelled` or `klippy_shutdown` | Job `cancelled{hostCancelled}` or `failed`, reservation `unresolved`, requirement `pending` | `restart_after_host_failed_opens_one_requirement` |
| R12 | After the terminal commit, "before settlement" | cannot happen: settlement work is in the terminal tx. Disk shows `failed`, settlement `pending`, requirement `pending`, reservation `unresolved` | unchanged; nothing re-consumed or re-opened | `restart_after_terminal_keeps_settlement_pending` |
| R13 | Inside a settle tx, before commit | settlement `pending`; no ledger row | unchanged; the same `operationId` settles once | `restart_inside_settle_rolls_back` |
| R14 | After a defer | settlement `deferred`, requirement `deferred`, reservation `unresolved` | unchanged; the amount stays unavailable | `restart_after_defer_keeps_the_amount_unavailable` |
| R15 | Inside the release tx, before commit | Job `awaitingStart`, entry `assigned` at position p, reservation `active` | unchanged; release again succeeds | `restart_inside_release_rolls_back` |
| R16 | After release committed | Job `cancelled{releasedBeforeStart}`, reservation `released`, old entry `closed{released}`, replacement `queued` at p | unchanged; the evaluator may assign the replacement | `restart_after_release_keeps_the_replacement_in_place` |
| R17 | Start `abandoned` committed, `apply_host_outcome` not run | Job `starting`; start op `abandoned` | Job `outcomeUnknown`, one `jobOutcomeUnknown` requirement | `restart_after_abandoned_start_is_outcome_unknown` |
| R18 | Cancel handoff committed, reply lost | Job `printing`, cancel op `dispatching`, sent | op `uncertain` → reconciled; the tracker proves `cancelled{cancelledByOperator}` from history | `restart_after_cancel_reply_lost_ends_cancelled_once` |
| R19 | Pause `succeeded` committed, not applied | Job `printing`, pause op `succeeded` | Job `paused` | `restart_after_pause_success_applies_the_outcome` |
| R20 | After declare committed | Job terminal, requirement `resolved`; for failed or cancelled a new `materialReconciliation` `pending` | unchanged | `restart_after_declare_is_stable` |
| R21 | While a `printing` Job's Printer is unreachable (offline, or every history read fails) | Job `printing`, `host_unreachable_since` set | `host_unreachable_since` is kept across the restart (never reset by it); a successful read clears it; once it is 30 min old, `declare_job_outcome` is allowed from `printing` | `restart_keeps_host_unreachable_since_and_allows_declare_after_30_minutes` |
| R22 | After a Connection endpoint change during `printing` (no unresolved Host Operation), then a restart | Job `printing`, the Printer's new Connection | the tracker reads history from the new endpoint and pins or completes the Job normally; nothing is written to either endpoint | `endpoint_change_during_printing_moves_tracking_to_the_new_endpoint` |

**Recovery order** (`lib.rs`), before any command is served:
`slicing::operations::recover_after_restart` → `host_ops::recover_after_restart`
→ `jobs::recover_after_restart` → `restore_persisted_connections` →
`start_host_ops_runtime` → `start_jobs_runtime` (the driver and tracker)
→ `queue::evaluator` first run, after the driver's first pass and the
first host-ops reconciliation pass.

`jobs::recover_after_restart(storage, now)`:

1. For every `staging` or `starting` Job, and every `printing` or `paused`
   Job with an `active_host_operation_id`, load that Host Operation and
   run `apply_host_outcome`. Each Job is its own transaction.
2. Return the changed Jobs, which the runtime publishes once it starts.
3. `assigned` Jobs with no `lastFailure` and no upload op are left for
   the driver's first pass (R2). `printing` and `paused` Jobs are left for
   the tracker's first pass (R9–R11).

### D5. Eligibility (pure, deterministic)

**Decision: `queue::eligibility::evaluate` is a pure function of an
`EligibilityInput` (no I/O, no clock; `now` is passed in). Commands, the
evaluator, and the assign transaction all call it, and the assign
transaction feeds it rows read inside that transaction.**

Gates run in this order per Printer. A Printer is a candidate only if it
passes all of them. A failure is a `Blocker { code, message, detail,
recovery, printerIds }`. Printers are the unarchived Printers, plus the
entry's `manualPrinterId` even if archived, so the operator sees why.

**Gate 0, the entry.** An entry pinned to a Printer (`manualPrinterId`)
considers only that Printer. Every other Printer gets
`PINNED_TO_OTHER_PRINTER` and is omitted from `printers[]`.

**Gate 1, the Printer is schedulable** (every policy):

| Check | Blocker |
|---|---|
| `archived_at` is set | `PRINTER_ARCHIVED` |
| Setup incomplete (no Connection, or an unsupported adapter) | `SETUP_INCOMPLETE` |
| `connectionState` is `error` | `CONNECTION_ERROR` |
| `connectionState` is `offline` or `connecting` | `PRINTER_OFFLINE` |
| the Printer has an active Job | `JOB_ACTIVE` |
| the Printer has an unresolved Host Operation | `HOST_OPERATION_PENDING` |
| `operationalState` is `printing`, `paused`, or `busy` (it has no active Job, or the row above would have fired) | `PRINTER_BUSY_EXTERNAL` |
| **Automatic only:** `operationalState` is not `ready`, `finished`, or `cancelled`, or freshness is not `fresh` | `PRINTER_NOT_IDLE` |

**Gate 2, the profile is compatible.** `slicing::compat::profile_compatible(facts, profile)`
compares the revision's facts with the Printer's resolved profile
(`resolve_printer`):

- Bed shape. A rectangle matches when width, depth, and both origins are
  each within 0.01 mm. A polygon matches when it has the same number of
  points, in the same order, each coordinate within 0.01 mm. A rectangle
  never matches a polygon.
- Printable height within 0.01 mm.
- The Printer has exactly one nozzle (`nozzle_diameter_mm.len() == 1`),
  within 0.01 mm of the `nozzleDiameterMm` fact.
- `nozzleType` and `gcodeFlavor` are equal strings.
- `bedExcludeAreas` are not compared (as in P5's matching count).

A mismatch is `PROFILE_MISMATCH`, with `detail` naming the first one
("Nozzle 0.6 mm; this Slice needs 0.4 mm."). An absent `printerProfile`
or `nozzleDiameterMm` fact skips its comparison **only** under the Manual
policy, and the assignment then needs `acknowledgeManualFacts: true`. Any
other policy is blocked with `NEEDS_MANUAL_PRINTER` for every Printer.
`add_to_queue` and `update_queue_entry` already refuse a non-Manual
policy on such a revision, so that blocker appears only in `explain` for
data older than the rule, and in the fixtures.

**Gate 3, capability** (`services.capabilities(&printer)`):

| Check | Blocker |
|---|---|
| any of `upload`, `start`, `hostState`, `artifactIdentity` is `unsupported` | `CAPABILITY_UNSUPPORTED`, `detail` = the first capability's `detail` |
| **Automatic only:** any of the four has evidence with `tier` other than `sim` | `ADAPTER_NOT_PROVEN` |

**Gate 4, material.** The candidate Spools for a Printer are the Spools
that are **loaded in one of its Material Slots**, plus, under Manual and
Recommended only, Spools **in storage**. A Spool loaded on another
Printer is never a candidate. A candidate Spool must:

- be `active`;
- match `materialFamily` (and, for `OTHER`, match `materialOther`
  case-insensitively after trimming);
- have a `diameter` within 0.01 mm of `filamentDiameterMm` (`1.75` →
  1.75, `2.85` → 2.85);
- have `availableMg ≥ estimateMg`.

An absent `materialFamily` or `filamentDiameterMm` fact skips that
comparison under Manual only (with `acknowledgeManualFacts`).

| Result | Blocker |
|---|---|
| no Spool matches material and diameter | `NO_COMPATIBLE_SPOOL` |
| matching Spools exist but none has enough available | `INSUFFICIENT_MATERIAL` (`detail`: the best `availableMg`) |
| **Automatic only:** a sufficient matching Spool exists only in storage | `SPOOL_NOT_LOADED` |
| a `materialFamily` or `filamentDiameterMm` fact is absent and the policy is not Manual | `NEEDS_MANUAL_PRINTER` (every Printer; see F5) |

**Choosing the Spool for a Printer.** Among its candidate Spools: loaded
on this Printer first, then the smallest sufficient `availableMg`, then
the lowest Spool number. Every candidate Spool is also listed, in that
order, as `spoolOptions`, so the operator can pick another under Manual
or Recommended.

**Ranking the candidate Printers:**

| Preference | Sort key (ascending unless marked) |
|---|---|
| `loadedFirst` | chosen Spool loaded on this Printer (loaded first); chosen Spool's `availableMg` (**descending**); `name.to_lowercase()`; `id` (byte order) |
| `leastRecentlyUsed` | `lastUsedAt` (the latest `ended_at` of this Printer's terminal Jobs; a Printer with none sorts first); `name.to_lowercase()`; `id` |

**Verdict** (`EligibilityVerdict`) for an open `queued` entry:

| Verdict | When |
|---|---|
| `blocked` | no candidate |
| `awaitingOperator` | at least one candidate, and the policy is Manual or Recommended |
| `ready` | at least one candidate, and the policy is Automatic (the evaluator will assign it) |

`assigned` entries have no verdict. Their Job's state and
`startBlockers` explain them.

**Aggregated blockers.** `QueueEntryEligibility.blockers` groups the
Printers' blockers by code, in gate order, with every affected Printer id
in `printerIds`. `topBlocker` (in the summary) is the first of them.

**Recovery per blocker:**

| Blocker | Message (example) | `recovery` |
|---|---|---|
| `PRINTER_ARCHIVED` | "This Printer is archived." | `UNARCHIVE_PRINTER` |
| `SETUP_INCOMPLETE` | "This Printer's setup is incomplete." | `OPEN_PRINTER_SETUP` |
| `CONNECTION_ERROR` | "This Printer's Connection has an error." | `CHECK_CONNECTION` |
| `PRINTER_OFFLINE` | "This Printer is offline." | `CHECK_CONNECTION` |
| `JOB_ACTIVE` | "This Printer already has a Job." | `OPEN_JOB` |
| `HOST_OPERATION_PENDING` | "This Printer has a pending printer operation." | `OPEN_PRINTER_JOB` |
| `PRINTER_BUSY_EXTERNAL` | "This Printer is printing a file farm3d didn't start." | none |
| `PRINTER_NOT_IDLE` | "Automatic assignment waits for this Printer to be idle: <state label>." | none |
| `PINNED_TO_OTHER_PRINTER` | "This entry is pinned to <name>." | none |
| `PROFILE_MISMATCH` | the first mismatch | none |
| `NEEDS_MANUAL_PRINTER` | "This Slice has facts nobody confirmed. Assign it by hand." | `ASSIGN_MANUALLY` |
| `CAPABILITY_UNSUPPORTED` | the capability's `detail` | none |
| `ADAPTER_NOT_PROVEN` | "Automatic dispatch needs a simulator-proven Connection. Assign by hand." | `ASSIGN_MANUALLY` |
| `NO_COMPATIBLE_SPOOL` | "No Spool of <material> <diameter> mm is available." | `LOAD_SPOOL` |
| `INSUFFICIENT_MATERIAL` | "No matching Spool has <estimate> g available." | `LOAD_SPOOL` |
| `SPOOL_NOT_LOADED` | "Automatic assignment needs the Spool loaded on this Printer." | `LOAD_SPOOL` |
| `PRINTER_NOT_READY` (start blockers only, D7) | "The printer can't start now: <state label>." | none |

`check_assignment(input, printerId, spoolId, mode)` runs the same gates
for one Printer and one Spool, and fails with every blocker it finds.
The operator may choose any listed `spoolOption`, not only the chosen
one.

**Tie-break fixtures.** Task 5 copies these verbatim as
`#[test] fn tie_break_fixture_N()`. Amounts are in mg. Every Spool is
PLA 1.75 unless a fixture says otherwise, and every entry needs PLA 1.75.
Every Printer passes gates 1–3 (Moonraker, sim evidence, `ready`, fresh)
unless a fixture says otherwise.

**F1: equal names, different ids.** Recommended, `loadedFirst`,
estimate 50 000.

| Printer | Name | Loaded Spool (availableMg) | Last used |
|---|---|---|---|
| `prn-b` | Voron | `spl-1` (900 000) | never |
| `prn-a` | Voron | `spl-2` (900 000) | never |

Expected: `[prn-a, prn-b]`.

**F2: case-only name differences.** Recommended, `loadedFirst`, estimate
50 000.

| Printer | Name | Loaded Spool (availableMg) | Last used |
|---|---|---|---|
| `prn-3` | Bravo | `spl-3` (900 000) | never |
| `prn-2` | alpha | `spl-2` (900 000) | never |
| `prn-1` | Alpha | `spl-1` (900 000) | never |

Expected: `[prn-1, prn-2, prn-3]`. "Alpha" and "alpha" are equal
lowercased, so id decides; "Bravo" sorts after both although `B` < `a`
in byte order.

**F3: `loadedFirst` versus `leastRecentlyUsed`.** Estimate 300 000.

| Printer | Name | Loaded Spool (availableMg) | Last used |
|---|---|---|---|
| `prn-a` | A | `spl-1` (500 000) | 2026-09-20T10:00:00Z |
| `prn-b` | B | none | never |
| `prn-c` | C | `spl-3` (700 000) | 2026-09-25T10:00:00Z |

Stored: `spl-9` (800 000).

| Entry | Expected ranked candidates (chosen Spool) |
|---|---|
| Recommended, `loadedFirst` | `[prn-c (spl-3), prn-a (spl-1), prn-b (spl-9)]` |
| Recommended, `leastRecentlyUsed` | `[prn-b (spl-9), prn-a (spl-1), prn-c (spl-3)]` |
| Automatic, `loadedFirst` | `[prn-c (spl-3), prn-a (spl-1)]`; `prn-b` blocked `SPOOL_NOT_LOADED` |
| Automatic, `leastRecentlyUsed` | `[prn-a (spl-1), prn-c (spl-3)]`; `prn-b` blocked `SPOOL_NOT_LOADED` |

**F4: an insufficient but loaded Spool.** Estimate 300 000.

| Printer | Name | Loaded Spool (availableMg) | Last used |
|---|---|---|---|
| `prn-a` | A | `spl-1` (200 000) | never |
| `prn-b` | B | `spl-3` (400 000) | never |

Stored: `spl-2` (1 000 000).

| Entry | Expected |
|---|---|
| Recommended, `loadedFirst` | `[prn-b (spl-3), prn-a (spl-2)]`. `prn-a`'s loaded Spool is insufficient, so its chosen Spool is the stored one, and it ranks in the not-loaded group. `prn-b`'s `spoolOptions` are `[spl-3, spl-2]`. |
| Automatic, `loadedFirst` | `[prn-b (spl-3)]`; `prn-a` blocked `SPOOL_NOT_LOADED` (a sufficient Spool exists only in storage) |

**F5: an absent material fact under Manual.** The revision's
`materialFamily` fact is absent; `filamentDiameterMm` is 1.75. Estimate
100 000.

| Printer | Name | Loaded Spool (family, availableMg) | Last used |
|---|---|---|---|
| `prn-a` | A | `spl-1` (PETG, 500 000) | never |
| `prn-b` | B | `spl-4` (PLA, 900 000) | never |

Stored: `spl-2` (PLA, 900 000).

| Entry | Expected |
|---|---|
| Manual, `loadedFirst`, `manualPrinterId = prn-a` | `[prn-a]` with `spoolOptions` `[spl-1, spl-2]` (family not compared), `manualFactsAcknowledgementRequired: true`; `prn-b` is omitted (`PINNED_TO_OTHER_PRINTER`) |
| Recommended, `loadedFirst` | `[]`; every Printer blocked `NEEDS_MANUAL_PRINTER`; verdict `blocked` |

**F6: two automatic entries competing for one Printer.** Estimate
100 000 each. One Printer, `prn-a` "A", loaded `spl-1` (900 000). Entry
E1 at position 1 and E2 at position 2, both Automatic, `loadedFirst`.

| Step | Expected |
|---|---|
| Evaluator run | E1 is assigned to `prn-a` with `spl-1`. E2 is not offered `prn-a` (claimed earlier in the run). |
| E2's summary after the run | `eligibleCount: 0`, `topBlocker: JOB_ACTIVE` with `printerIds: [prn-a]`, verdict `blocked` |
| `nextAutomaticAction` | `{ kind: "waiting", entryId: E2, blocker: JOB_ACTIVE }` |

**F7: an unproven adapter under Automatic.** Estimate 100 000.

| Printer | Name | Adapter / evidence | Loaded Spool (availableMg) |
|---|---|---|---|
| `prn-m` | M | Moonraker, `readOnlyHardware` evidence on all four capabilities | `spl-1` (900 000) |
| `prn-o` | O | OctoPrint (`notVerified`) | `spl-2` (900 000) |
| `prn-s` | S | Moonraker, `sim` | `spl-3` (100 000) |

| Entry | Expected |
|---|---|
| Automatic, `loadedFirst` | `[prn-s]`; `prn-m` blocked `ADAPTER_NOT_PROVEN`; `prn-o` blocked `CAPABILITY_UNSUPPORTED` |
| Recommended, `loadedFirst` | `[prn-m (spl-1), prn-s (spl-3)]`; `prn-o` blocked `CAPABILITY_UNSUPPORTED` |

**F8: choosing among equal Spools.** Recommended, `loadedFirst`,
estimate 100 000. One Printer `prn-a` with nothing loaded. Stored:
`spl-7` (#7, 400 000), `spl-3` (#3, 400 000), `spl-5` (#5, 250 000).

Expected: `[prn-a (spl-5)]`, `spoolOptions` `[spl-5, spl-3, spl-7]`
(smallest sufficient, then Spool number).

### D6. Automatic evaluator

**Decision: one tokio task, one coalescing trigger channel, one run at a
time.**

- **Channel:** `mpsc` with capacity 1. `EvaluatorHandle::poke(trigger)`
  uses `try_send` and ignores `Full`, because a run is already pending.
  The task drains the channel, then runs once.
- **First run:** after `jobs::recover_after_restart`, the driver's first
  pass, and the first host-ops reconciliation pass (D4 order).
- **Before the evaluator exists or has run** (ruling R3): `list_queue`
  computes each `queued` entry's `EligibilitySummary` synchronously with
  `eligibility::evaluate` over current rows and statuses, and reports
  `nextAutomaticAction: { kind: "evaluatorNotRunning" }`. Task 6 ships
  exactly that. Once the evaluator has completed a run, `list_queue`
  returns the evaluator's cached summaries and `nextAutomaticAction`
  instead (Task 10).
- **Triggers** (`Trigger`):

  | Trigger | Source |
  |---|---|
  | `Startup` | the first run |
  | `QueueChanged` | an entry was created, updated, moved, or removed |
  | `JobChanged` | a Job reached a terminal state, was released, or was cancelled before start |
  | `HostOperationChanged` | `host_ops` `subscribe_changes` (an unresolved row resolving frees a Printer) |
  | `StatusChanged(printerId)` | `ConnectionManager::subscribe_status` |
  | `CapabilitiesChanged` | host facts refreshed |
  | `InventoryChanged` | `RuntimeServices.inventory_changes` (loads, moves, amounts, settlements) |
  | `PrinterChanged` | a Printer was created, edited, archived, unarchived, or deleted |

- **A run** is split into a pure pass and an async driver, so the pure
  part needs no lock:
  - `evaluate_pass(input: &EligibilityInput-set, claimed: &BTreeSet<String>,
    refused: &BTreeMap<String, Blocker>) -> PassResult { summaries,
    proposal: Option<Proposal { entry_id, printer_id, spool_id }> }` is
    pure and synchronous. It evaluates every `queued` entry top to bottom,
    never offers a Printer in `claimed`, skips entries in `refused` (their
    summary is the recorded blocker), and returns the **first** Automatic
    entry with a candidate as the proposal (top candidate, chosen Spool).
  - `async fn run_once(services) -> Result<EvaluationRun, …>` loops:
    read a fresh input from storage and live state; call `evaluate_pass`;
    if there is a proposal, take `host_ops.printer_lock(printer_id)` and
    call **the same** `jobs::assign::assign` as the command, in its own
    `write_repo`, with operation id `auto-<uuid>`,
    `mode = Automatic`, and `assigned_by = automatic`; on success add
    the Printer to `claimed`; on refusal add the entry to `refused`; then
    loop. It stops when a pass has no proposal. Each loop re-reads the
    input, so Spool availability after an assignment is always current.
  - **Refused assignment.** If the in-transaction re-check refuses, the
    entry's summary records the refusal's first blocker as `topBlocker`:
    `ASSIGNMENT_BLOCKED` contributes its first `Blocker`; `JOB_ACTIVE`
    becomes the `JOB_ACTIVE` blocker; `INSUFFICIENT_MATERIAL` becomes
    `INSUFFICIENT_MATERIAL`; `SPOOL_NOT_RESERVABLE` becomes
    `NO_COMPATIBLE_SPOOL`. The Printer is **not** claimed (it got no Job),
    so later entries may still be offered it. The evaluator pokes itself
    once more.
  - A Printer that *was* claimed now has a Job, so later entries see it
    blocked with `JOB_ACTIVE` in the next pass. No separate "claimed"
    code exists.
- **After a run,** it publishes one `queue.eligibility.changed` if any
  summary or `nextAutomaticAction` changed, plus the entry, Job, and
  Spool events of every assignment.
- **`NextAutomaticAction`** is what the last run concluded:

  ```ts
  type NextAutomaticAction =
    | { kind: "evaluatorNotRunning" }
    | { kind: "noAutomaticEntries"; evaluatedAt: string }
    | { kind: "waiting"; evaluatedAt: string; entryId: string; blocker: Blocker }
    | { kind: "assigned"; evaluatedAt: string; entryId: string; jobId: string; printerId: string };
  ```

  - `noAutomaticEntries`: no `queued` Automatic entry exists.
  - `waiting`: the first `queued` Automatic entry the run could not
    assign, and its top blocker.
  - `assigned`: the run assigned at least one entry and none is waiting;
    it names the last one.
- **No automatic retry.** Nothing re-queues a failed Job, re-stages a
  failed upload, or re-sends a start. Only `retry_job` and `stage_job`
  do.

### D7. Dispatch driver and outcome tracker

**The driver** is one tokio task per process (`start_jobs_runtime`). It
selects on the host-ops change broadcast, the status broadcast, a poll
interval, and a queue of "stage now" requests. On a
`RecvError::Lagged` it re-reads every active Job from storage.

- **Staging.** After each committed assignment, and on its first pass for
  every `assigned` Job with no `lastFailure` and no upload op (R2), the
  driver calls `jobs::dispatch::stage_job` with a `drv-*` id, whatever the
  policy. If P6 refuses before writing a row (for example
  `PRINTER_UNREACHABLE`), the driver records `lastFailure = { kind:
  "refused", code, message }` on the `assigned` Job in its own
  transaction. The state doesn't change and no event row is written, but
  `revision` goes up and the Job is published. **It never stages again
  by itself.** The operator uses **Stage** (`stage_job`) or **Release**.
- **`apply_host_outcome(tx, op, now)`**, pure over the transaction,
  idempotent:

  | Linked op kind | Op state | Job state required | Job event |
  |---|---|---|---|
  | upload | `dispatching`, `uncertain`, `reconciling` | any | none |
  | upload | `succeeded` | `staging` | `StageSucceeded` (sets `upload_host_operation_id`, `host_path`, clears `lastFailure`) |
  | upload | `failed` | `staging` | `StageFailed` (`lastFailure = { kind: "hostOperationFailed", hostOperationId, failure }`) |
  | upload | `abandoned` | `staging` | `StageFailed` (`lastFailure = { kind: "hostOperationAbandoned", hostOperationId }`) |
  | start | unresolved | any | none |
  | start | `succeeded` | `starting` | `StartSucceeded` (sets `started_at`, and `history_mark` from the op) |
  | start | `failed` | `starting` | `StartFailed` (`lastFailure`) |
  | start | `abandoned` | `starting` | `StartAbandoned`, plus a `jobOutcomeUnknown` requirement |
  | pause | `succeeded` | `printing` | `Paused` |
  | resume | `succeeded` | `paused` | `Resumed` |
  | cancel | `succeeded` | `printing`, `paused` | none; the tracker checks history at once |
  | pause, resume, cancel | `failed`, `abandoned` | `printing`, `paused` | `ControlFailed` (`detail`: the op id and failure) |
  | any | any | any other state | none (already applied) |

  - An op whose `job_id` is NULL is ignored.
  - **Clearing `active_host_operation_id`** is separate from the event
    table and runs in **every** Job state, terminal ones included: when
    the op is terminal and equals the Job's `active_host_operation_id`,
    the column is set to NULL (a column update with no event, `revision`
    + 1). So a cancel op that resolves after the tracker already made the
    Job `cancelled` only clears the column. It writes no `ControlFailed`,
    even if the op failed, because the Job is terminal.
  - `StartHandedOff` sets `start_host_operation_id`, which is never
    cleared (the tracker's pin rule reads its `dispatched_at`).
- **Start.** `start_job(operationId, jobId, priorState, acknowledgement:
  "bedClear")`:
  1. Replay check.
  2. The Job exists (`NOT_FOUND`) and is `awaitingStart`
     (`JOB_ACTION_NOT_ALLOWED`); `acknowledgement` is `"bedClear"`
     (`VALIDATION` on `acknowledgement`).
  3. `start_blockers` is empty, else `JOB_START_BLOCKED` with the
     blockers.
  4. `host_ops::api::start(…, upload_host_operation_id, priorState, link)`.
     P6 runs its whole D9 order (capability, unresolved row, the Start
     rule, host re-read, history mark, `locate`, the Start rule again),
     and its errors pass through unchanged.
  5. The link, inside the write-ahead transaction: claims `startJob`,
     re-loads the Job and requires `awaitingStart`, requires
     `active_job_for_printer` to be this Job, requires
     `upload_host_operation_id` to be the op's `source_host_operation_id`,
     then `StartHandedOff` with `start_confirmation = bedClear` and
     `active_host_operation_id`.
- **`start_blockers(job, printer, status, loaded, caps, unresolved)`**,
  pure, for an `awaitingStart` Job, in this order:

  | Check | Blocker |
  |---|---|
  | the Job's Spool is not loaded in one of this Printer's Material Slots | `SPOOL_NOT_LOADED` (UI: "Awaiting material: load Spool #12 on <Printer>") |
  | the Printer has an unresolved Host Operation | `HOST_OPERATION_PENDING` |
  | `start` or `artifactIdentity` is unsupported | `CAPABILITY_UNSUPPORTED` |
  | P6's `start_rule` offers no start (not Ready, Finished, or Cancelled with fresh telemetry) | `PRINTER_NOT_READY` |

- **Unattended start** (decision 1). `may_start_unattended` holds when:
  the Printer's `startSafety` is `unattended`; `operationalState` is
  `ready` with `fresh` telemetry (never `finished` or `cancelled`);
  `start_blockers` is empty; and `upload`, `start`, `hostState`, and
  `artifactIdentity` have `tier: sim` evidence. The driver checks it
  whenever an `awaitingStart` Job's Printer status changes, the Job
  enters `awaitingStart`, or an inventory change touches its Spool. Then
  it calls the same start path with a `drv-*` id, `priorState: ready`,
  and `start_confirmation = unattended`. A `ConfirmBedClear` Printer never
  starts without `start_job`.
- **Staging again.** `stage_job` in `awaitingStart` stages the same Slice
  Revision again (`StageHandedOff` → `staging`). The operator uses it
  after a Connection endpoint change or a `STAGED_ARTIFACT_INVALID`
  start. The driver never does it by itself.
- **Pause, resume, cancel after start.** `pause_job`, `resume_job`, and
  `cancel_job` (in `printing` or `paused`) call `host_ops::api::control`.
  P6's control rule and host re-read apply unchanged. The link also
  requires the op's `host_path` (the file the host reports) to equal the
  Job's `host_path`. Otherwise it fails with `JOB_NOT_ON_PRINTER`, and
  the write-ahead rolls back, so nothing is sent.
- **Raw P6 writes step aside** (decision 9). `stage_slice_revision`,
  `start_staged_artifact`, `pause_host_print`, `resume_host_print`, and
  `cancel_host_print` refuse with `JOB_ACTIVE` when the Printer has an
  active Job, checked inside P6's write-ahead through
  `jobs::repository::active_job_for_printer`.

**The tracker** runs inside the driver task. For each `printing` or
`paused` Job:

- **Inputs:** the status broadcast for its Printer (progress, file,
  state), and a `job_history(HistoryQuery { since_epoch_s: None, limit:
  50 })` poll every `JobTimings.history_poll` (10 s by default; tests
  inject shorter ones). A status that says `finished`, `cancelled`, or
  `failed` while the reported file is the Job's `host_path` triggers a
  poll at once. So does a succeeded cancel op. A status is never proof.
- **Endpoint.** Each poll builds its `HostStateQuery` from the Printer's
  **current** Connection, like P6's per-operation capability objects. A
  Connection endpoint change during `printing` (allowed by D8 when no
  Host Operation is unresolved) moves tracking to the new endpoint.
- **Pinning** reuses P6's start rule (b). While `host_job_id` is NULL,
  the first history job (lowest `job_id`) that has `filename ==
  host_path`, **and** `job_id > history_mark`, **and** `start_time_epoch_s
  ≥ dispatched_at − 30 s` (the Job's start Host Operation's
  `dispatched_at`, P6's `START_SKEW_TOLERANCE`) is pinned: `host_job_id`
  is set and `HostJobPinned` is written.
- **Verdict** (`verdict_from_history(job, history) -> (Option<u64>,
  TrackerVerdict)`, pure):

  | Pinned history job's `status` | Verdict | Job event |
  |---|---|---|
  | `in_progress` | `StillRunning` | none |
  | `completed` | `Completed` | `Completed` |
  | `cancelled` | `Cancelled` | `Cancelled`, with the reason below |
  | `error`, `klippy_shutdown`, `klippy_disconnect`, `server_exit` | `Failed` | `Failed` |
  | any other string | `Inconclusive` | none |
  | the pinned job is not in the page | `Inconclusive` | none |
  | unpinned, and no history job qualifies, and the Printer reports a different file or no print | `Inconclusive` | none |
  | unpinned, no history job qualifies yet, and the Printer reports our file | `StillRunning` | none |

- **Cancel reason.** When history proves `cancelled`, the reason is
  `cancelledByOperator` if the Job handed off a cancel (`CancelHandedOff`)
  whose Host Operation is anything but `failed`: `succeeded`, still
  `dispatching`, `uncertain`, or `reconciling`, or `abandoned`. A failed
  cancel was definitively not applied, so a cancel seen in history then
  came from the printer: `hostCancelled`, as it is with no cancel handoff
  at all. The Job ends `cancelled` either way. A still-unresolved cancel
  op keeps reconciling on its own (P6), and its resolution later only
  clears `active_host_operation_id`.
- **Progress.** `max_progress_pct = max(max_progress_pct,
  floor(telemetry.progress × 100))`, written only when it grows by a
  whole percent, and only while the reported file is the Job's
  `host_path`.
- **Paused and printing** are also followed from status on our file
  (`Paused` / `Resumed` events).
- **Unprovable outcome.** Each `Inconclusive` poll adds one to
  `inconclusive_checks`. A conclusive poll resets it to 0. At
  `JobTimings.inconclusive_limit` (3), the Job becomes `outcomeUnknown`.
  A poll that could not run (the Printer is offline, or the query errors)
  counts as neither.
- **Unreachable host** (ruling R5). A poll that could not run sets
  `host_unreachable_since` to now if it is NULL (no event; `revision` + 1;
  published). Any successful host read (a history poll, or a status
  observation from the Printer's Connection) sets it back to NULL. Once it
  is at least `JobTimings.unreachable_declare_after` (30 min) old,
  `declareOutcome` appears in the Job's `allowedActions` and
  `declare_job_outcome` is accepted from `printing` or `paused` (D9). The
  Printer then isn't trapped: without it, a Job whose Printer never
  answers again could never end, so the Printer couldn't be archived and
  its Spool's reservation would never settle.
- **The tracker never adopts a foreign print.** It only considers the
  history job it pinned, and it pins only a job for this Job's
  `host_path` above this Job's `history_mark`.

`JobTimings { history_poll: Duration, inconclusive_limit: u32,
unreachable_declare_after: Duration }`, defaulting to 10 s, 3, and 30 min.
Tests inject short ones. The
plan's quick-print gap from P6 closes for Jobs: a start that finished
before the first poll still pins its history job and completes (R10's
pattern).

### D8. Lifecycle guards

Every guard runs inside the mutation's own transaction.

| Mutation | Blocked when | Result |
|---|---|---|
| Printer archive | the Printer has an active Job (any non-terminal state, including `outcomeUnknown`) | `LIFECYCLE_BLOCKED`, blocker `JOB_ACTIVE`: "Finish, cancel, or release this Printer's Job before archiving." |
| Printer delete | any Job references the Printer (decision 7) | `LIFECYCLE_BLOCKED`, blocker `JOB_HISTORY_EXISTS`: "This Printer has Job history. Archive it instead." |
| Printer delete | an open Queue Entry is pinned to it (`manual_printer_id`) | `LIFECYCLE_BLOCKED`, blocker `QUEUE_ENTRY_PINNED`: "A Queue Entry is pinned to this Printer. Remove it or change its Printer first." |
| Printer import (`replace_all`) | any Job exists, or any open Queue Entry is pinned to a Printer | `JOBS_EXIST` for the whole import, nothing written |
| Slice Revision delete | any Queue Entry (any state) or Job references it | `LIFECYCLE_BLOCKED`, blocker `QUEUE_REFERENCES_REVISION`: "Queue Entries or Jobs use this Slice Revision." |
| Model delete | already blocked transitively (`SLICE_REVISIONS_EXIST`) | unchanged |
| Spool archive, mark empty | open (`active` or `unresolved`) reservations | `LIFECYCLE_BLOCKED`, `SPOOL_RESERVED` (P3, unchanged) |
| Connection endpoint change, Connection clear, credential clear | the Printer has an **unresolved Host Operation** (P6's guard, unchanged). An active Job with no unresolved Host Operation does **not** block (ruling R5). | `CONNECTION_IN_USE`. When the unresolved op is linked to a Job, `details` also carry `jobId`, `recovery` is `[OPEN_JOB]`, and the message is the Job one (§Error codes) |

- **Why import needs no Job at all, not just no active one.**
  `replace_all` deletes every Printer, and `jobs.printer_id` is `ON
  DELETE RESTRICT`. Any Job, even a finished one, would make the delete
  fail mid-import. Decision 7 keeps Job history, and P9 owns pruning, so
  P7 rejects the import up front with a clear code. This departs from the
  plan's D8, which named `JOBS_ACTIVE` for active or unsettled Jobs only.
- `queue_entries.manual_printer_id` is `ON DELETE SET NULL`, so a closed
  entry never blocks a Printer delete. The guard blocks for open entries.
- `jobs.slice_revision_id` and `queue_entries.slice_revision_id` are `ON
  DELETE RESTRICT`, which backs up `QUEUE_REFERENCES_REVISION`.
- `LifecycleBlockerCode` gains `JOB_ACTIVE`, `JOB_HISTORY_EXISTS`,
  `QUEUE_ENTRY_PINNED`, and `QUEUE_REFERENCES_REVISION`.
- **Why an active Job alone doesn't block a Connection change.** A
  printing Job whose Printer moved to a new address would otherwise be
  stuck: the tracker could never read it, and the Job could never end.
  After the change, the tracker reads from the new endpoint (D7). A staged
  file of an `awaitingStart` Job is on the old endpoint, so its start
  fails `STAGED_ARTIFACT_INVALID`, and the operator stages again
  (`stage_job` from `awaitingStart`). This departs from the plan's D8,
  which blocked any active Job past `assigned`.
- Archiving keeps Job identity: Jobs keep `printer_id` and
  `printer_snapshot_json` (name, location, catalog reference, resolved
  profile at assignment).

### D9. Manual host-job handling

- **A Printer printing a file farm3d didn't start is ineligible.** Gate 1
  blocks it with `PRINTER_BUSY_EXTERNAL` while its state is `printing`,
  `paused`, or `busy` and it has no active Job. A print started by a raw
  P6 `start_staged_artifact` is "not started as a Job" and counts too.
- **A Job never adopts a foreign print.** The tracker pins only a history
  job for the Job's own `host_path` above its own `history_mark` (D7). A
  foreign file the Printer reports makes a poll `Inconclusive`, never a
  verdict.
- **An `awaitingStart` Job whose Printer starts printing something
  else** stays `awaitingStart`. Its `startBlockers` show
  `PRINTER_NOT_READY`, and P6's host re-read refuses a start
  (`START_NOT_ALLOWED`). farm3d never cancels the foreign print.
- **A raw P6 write on a Printer with an active Job** (upload, start,
  pause, resume, or cancel) is refused with `JOB_ACTIVE`, and writes no
  row. While a Job is active, pause, resume, and cancel go through the
  Job. If a foreign print runs while the Job is `awaitingStart`, farm3d
  offers no control for it; the operator uses the printer, and the Job
  panel says so.
- **Outcome unknown.** A Job becomes `outcomeUnknown` when its start
  Host Operation is `abandoned`, or when the tracker hits the
  inconclusive limit. It stays active: the Printer is still `JOB_ACTIVE`
  and can't be archived, its reservation stays `active`, and a
  `jobOutcomeUnknown` requirement is `pending`. farm3d stops checking.
- **`declare_job_outcome(operationId, jobId, outcome, acknowledgement:
  "hostStateUnknown")`**, with `outcome` one of `completed`, `failed`, or
  `cancelled`:
  - allowed in `outcomeUnknown`, and in `printing` or `paused` once
    `hostUnreachableSince` is at least 30 minutes old (ruling R5);
    otherwise `JOB_ACTION_NOT_ALLOWED`;
  - from `printing` or `paused` there is no `jobOutcomeUnknown`
    requirement to resolve; everything else below is the same;
  - `acknowledgement` must be `"hostStateUnknown"` (`VALIDATION`);
  - `completed`: the estimate is consumed and settlement is
    `settled/estimated`;
  - `failed` or `cancelled` (`cancelReason = operatorDeclared`): the
    reservation becomes `unresolved`, settlement is `pending`, and a
    `materialReconciliation` requirement opens;
  - the `jobOutcomeUnknown` requirement, if any, is resolved with
    `{ kind: "declared", outcome }`, and the entry closes;
  - farm3d sends nothing to the host.

## Material settlement

`jobs::settlement`, per decisions 5 and 6:

| Choice | Allowed when | Reservation | Ledger | Settlement | Requirement |
|---|---|---|---|---|---|
| automatic on completion | `Completed`, `DeclaredCompleted` | `consume(estimateMg)` | one `Consumption` (`estimated`) | `settled/estimated` | none |
| `estimated` | `pending` or `deferred` | `consume(estimatedUseMg)` | one `Consumption` | `settled/estimated` | `resolved` `{ kind: "settled", method: "estimated", usedMg }` |
| `measured { entry }` | `pending` or `deferred` | `consume_measured(entry)` | one `Measurement` (`measured`) with the reservation id | `settled/measured` | `resolved` `{ kind: "settled", method: "measured", usedMg }` |
| `defer` | `pending` only | stays `unresolved` | none | `deferred` | `deferred` |
| `correct_job_material { entry }` | `completed`, not yet corrected | already consumed | one `Measurement` marked as a correction (note "Correction for Job <id>") | unchanged | none |

- `estimatedUseMg = ceil(estimateMg × maxProgressPct / 100)` (integer
  math: `(estimateMg * pct + 99) / 100`). It is 0 when the Job never
  printed.
- `Job.settlementPreview = { estimatedUseMg }` only while settlement is
  `pending` or `deferred`, and `null` otherwise (ruling R4). The dialog
  shows it before confirming.
- `settle_job_material` on any Job whose settlement is `settled`
  (including every completed Job) fails with `JOB_ALREADY_SETTLED`
  (`reason: "settled"`). A second correction fails the same way, with
  `reason: "corrected"`. `settle` when settlement is `open` or
  `notRequired`, and `correct` on anything but `completed`, fail with
  `JOB_ACTION_NOT_ALLOWED` (D3).
- Every settlement path publishes the Job, the requirement (if any), and
  the Spool (`spools::events::publish_ids`) once, and sends one
  `InventoryChange`.

## Backend model

### Module layout

| File | Content |
|---|---|
| `queue/mod.rs` | wire types: `QueueEntry`, `QueueEntryState`, `CloseReason`, `DispatchPolicy`, `DispatchPreference`, `MaterialEstimate`, `EstimateSource`, `OriginKind`, `QueueEntryDisplay`, `QueueEntryAction`, `QueueSnapshot`, `QueueChange`, `QueueEntryEligibility`, `EligibilityVerdict`, `EligibilitySummary`, `PrinterEligibility`, `Candidate`, `SpoolOption`, `Blocker`, `BlockerCode`, `NextAutomaticAction` |
| `queue/state.rs` | the D2 transition function |
| `queue/repository.rs` | create with lineage, order, update, remove, load, list |
| `queue/eligibility.rs` | D5, pure |
| `queue/evaluator.rs` | D6 |
| `queue/events.rs` | `QueueStream`, `QueueEventType`, `QueueEventPayload`, `publish` |
| `queue/commands.rs` | the six queue commands |
| `jobs/mod.rs` | wire types: `Job`, `JobState`, `CancelReason`, `Settlement`, `SettlementMethod`, `AssignedBy` (ruling R1), `StartConfirmation` (ruling R2), `JobAction`, `JobFailure`, `JobEvent`, `JobEventKind`, `PrinterSnapshot`, `SettlementPreview`, `ReconciliationRequirement`, `RequirementKind`, `RequirementStatus`, `RequirementResolution`, `JobHistory`, `DeclaredOutcome`, `SettleChoice` |
| `jobs/state.rs` | the D3 transition function and `settlement_after` |
| `jobs/repository.rs` | insert, transition, events, requirements, snapshot |
| `jobs/assign.rs` | assign, release, retry, cancel before start |
| `jobs/dispatch.rs` | stage, start, and control handoff; `apply_host_outcome`; `start_blockers`; `may_start_unattended` |
| `jobs/tracker.rs` | D7 tracker, `verdict_from_history` |
| `jobs/settlement.rs` | settlement |
| `jobs/recovery.rs` | `recover_after_restart` |
| `jobs/guards.rs` | D8 sources and checks |
| `jobs/services.rs` | `JobServices<R>`, `JobTimings`, the driver task |
| `jobs/commands.rs` | the twelve Job commands |
| `slicing/compat.rs` | `profile_compatible`, `material_compatible` (P5's `matching_printer_ids` calls it) |

### Schema (migration `0008_p7_queue_jobs.sql`)

`CURRENT_SCHEMA_VERSION` becomes 8. The migration is one transaction.

```sql
CREATE TABLE queue_entries (
  id TEXT PRIMARY KEY CHECK (id GLOB 'qen-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  slice_revision_id TEXT NOT NULL REFERENCES slice_revisions(id) ON DELETE RESTRICT,
  lineage_id TEXT NOT NULL CHECK (lineage_id GLOB 'qln-*'),
  copy_index INTEGER NOT NULL CHECK (copy_index BETWEEN 1 AND 50),
  origin_entry_id TEXT REFERENCES queue_entries(id) ON DELETE RESTRICT,
  origin_kind TEXT CHECK (origin_kind IN ('retry','release')),
  state TEXT NOT NULL CHECK (state IN ('queued','assigned','closed')),
  close_reason TEXT CHECK (close_reason IN ('completed','failed','cancelled','released','removed')),
  position INTEGER CHECK (position >= 1),
  policy TEXT NOT NULL CHECK (policy IN ('manual','recommended','automatic')),
  preference TEXT NOT NULL CHECK (preference IN ('loadedFirst','leastRecentlyUsed')),
  estimate_mg INTEGER NOT NULL CHECK (estimate_mg > 0),
  estimate_source TEXT NOT NULL CHECK (estimate_source IN ('sliceEstimate','fileClaimConfirmed','operatorEntered')),
  manual_printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL,
  job_id TEXT REFERENCES jobs(id) ON DELETE RESTRICT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  closed_at TEXT,
  CHECK ((state = 'closed') = (close_reason IS NOT NULL)),
  CHECK ((state = 'closed') = (position IS NULL)),
  CHECK ((state = 'closed') = (closed_at IS NOT NULL)),
  CHECK ((origin_entry_id IS NULL) = (origin_kind IS NULL)),
  CHECK (state <> 'assigned' OR job_id IS NOT NULL),
  CHECK (state <> 'queued' OR job_id IS NULL),
  CHECK (manual_printer_id IS NULL OR policy = 'manual' OR state = 'closed')
) STRICT;
CREATE UNIQUE INDEX queue_entries_open_position ON queue_entries(position) WHERE position IS NOT NULL;
CREATE INDEX queue_entries_lineage ON queue_entries(lineage_id, copy_index);
CREATE INDEX queue_entries_slice_revision ON queue_entries(slice_revision_id);
CREATE UNIQUE INDEX queue_entries_one_successor ON queue_entries(origin_entry_id) WHERE origin_entry_id IS NOT NULL;
CREATE INDEX queue_entries_manual_printer ON queue_entries(manual_printer_id) WHERE manual_printer_id IS NOT NULL;

CREATE TABLE jobs (
  id TEXT PRIMARY KEY CHECK (id GLOB 'job-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  queue_entry_id TEXT NOT NULL UNIQUE REFERENCES queue_entries(id) ON DELETE RESTRICT,
  slice_revision_id TEXT NOT NULL REFERENCES slice_revisions(id) ON DELETE RESTRICT,
  printer_id TEXT NOT NULL REFERENCES printers(id) ON DELETE RESTRICT,
  printer_snapshot_json TEXT NOT NULL CHECK (json_valid(printer_snapshot_json)),
  spool_id TEXT NOT NULL REFERENCES spools(id) ON DELETE RESTRICT,
  reservation_id TEXT NOT NULL UNIQUE REFERENCES spool_reservations(id) ON DELETE RESTRICT,
  estimate_mg INTEGER NOT NULL CHECK (estimate_mg > 0),
  state TEXT NOT NULL CHECK (state IN ('assigned','staging','awaitingStart','starting','printing',
                                       'paused','completed','failed','cancelled','outcomeUnknown')),
  cancel_reason TEXT CHECK (cancel_reason IN ('releasedBeforeStart','cancelledBeforeStart',
                                              'cancelledByOperator','hostCancelled','operatorDeclared')),
  settlement TEXT NOT NULL CHECK (settlement IN ('open','notRequired','pending','deferred','settled')),
  settlement_method TEXT CHECK (settlement_method IN ('estimated','measured')),
  assigned_by TEXT NOT NULL CHECK (assigned_by IN ('operator','automatic')),
  manual_facts_acknowledged INTEGER NOT NULL DEFAULT 0 CHECK (manual_facts_acknowledged IN (0,1)),
  start_confirmation TEXT CHECK (start_confirmation IN ('bedClear','unattended')),
  upload_host_operation_id TEXT REFERENCES host_operations(id) ON DELETE RESTRICT,
  active_host_operation_id TEXT REFERENCES host_operations(id) ON DELETE RESTRICT,
  start_host_operation_id TEXT REFERENCES host_operations(id) ON DELETE RESTRICT,
  host_path TEXT,
  history_mark INTEGER CHECK (history_mark IS NULL OR history_mark >= 0),
  host_job_id INTEGER CHECK (host_job_id IS NULL OR host_job_id >= 0),
  max_progress_pct INTEGER NOT NULL DEFAULT 0 CHECK (max_progress_pct BETWEEN 0 AND 100),
  inconclusive_checks INTEGER NOT NULL DEFAULT 0 CHECK (inconclusive_checks >= 0),
  host_unreachable_since TEXT,
  last_failure_json TEXT CHECK (last_failure_json IS NULL OR json_valid(last_failure_json)),
  correction_event_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  started_at TEXT,
  ended_at TEXT,
  CHECK ((state = 'cancelled') = (cancel_reason IS NOT NULL)),
  CHECK ((state IN ('completed','failed','cancelled')) = (ended_at IS NOT NULL)),
  CHECK ((state IN ('completed','failed','cancelled')) = (settlement <> 'open')),
  CHECK ((settlement = 'settled') = (settlement_method IS NOT NULL)),
  CHECK (settlement <> 'notRequired' OR cancel_reason IN ('releasedBeforeStart','cancelledBeforeStart')),
  CHECK (state NOT IN ('awaitingStart','starting','printing','paused') OR upload_host_operation_id IS NOT NULL),
  CHECK (state NOT IN ('starting','printing','paused') OR start_confirmation IS NOT NULL),
  CHECK (state NOT IN ('printing','paused') OR (started_at IS NOT NULL AND history_mark IS NOT NULL)),
  CHECK (state NOT IN ('starting','printing','paused') OR start_host_operation_id IS NOT NULL),
  CHECK (host_unreachable_since IS NULL OR state IN ('printing','paused')),
  CHECK (correction_event_id IS NULL OR state = 'completed')
) STRICT;
CREATE UNIQUE INDEX jobs_one_active_per_printer ON jobs(printer_id)
  WHERE state NOT IN ('completed','failed','cancelled');
CREATE INDEX jobs_printer_ended ON jobs(printer_id, ended_at);
CREATE INDEX jobs_slice_revision ON jobs(slice_revision_id);
CREATE INDEX jobs_state ON jobs(state);

CREATE TABLE job_events (
  id TEXT PRIMARY KEY CHECK (id GLOB 'jev-*'),
  job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE RESTRICT,
  sequence INTEGER NOT NULL CHECK (sequence >= 1),
  kind TEXT NOT NULL,
  from_state TEXT,
  to_state TEXT NOT NULL,
  operation_id TEXT,
  host_operation_id TEXT REFERENCES host_operations(id) ON DELETE RESTRICT,
  detail_json TEXT CHECK (detail_json IS NULL OR json_valid(detail_json)),
  at TEXT NOT NULL,
  UNIQUE (job_id, sequence)
) STRICT;
CREATE TRIGGER job_events_append_only_u BEFORE UPDATE ON job_events
  BEGIN SELECT RAISE(ABORT, 'job_events is append-only'); END;
CREATE TRIGGER job_events_append_only_d BEFORE DELETE ON job_events
  BEGIN SELECT RAISE(ABORT, 'job_events is append-only'); END;

CREATE TABLE reconciliation_requirements (
  id TEXT PRIMARY KEY CHECK (id GLOB 'rrq-*'),
  job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE RESTRICT,
  kind TEXT NOT NULL CHECK (kind IN ('materialReconciliation','jobOutcomeUnknown')),
  status TEXT NOT NULL CHECK (status IN ('pending','deferred','resolved')),
  spool_id TEXT REFERENCES spools(id) ON DELETE RESTRICT,
  reservation_id TEXT REFERENCES spool_reservations(id) ON DELETE RESTRICT,
  opened_at TEXT NOT NULL,
  deferred_at TEXT,
  resolved_at TEXT,
  resolution_json TEXT CHECK (resolution_json IS NULL OR json_valid(resolution_json)),
  UNIQUE (job_id, kind),
  CHECK (kind <> 'materialReconciliation' OR (spool_id IS NOT NULL AND reservation_id IS NOT NULL)),
  CHECK (kind <> 'jobOutcomeUnknown' OR status <> 'deferred'),
  CHECK ((status = 'resolved') = (resolved_at IS NOT NULL AND resolution_json IS NOT NULL)),
  CHECK (status <> 'deferred' OR deferred_at IS NOT NULL)
) STRICT;
CREATE INDEX reconciliation_requirements_open ON reconciliation_requirements(status) WHERE status <> 'resolved';

ALTER TABLE host_operations ADD COLUMN job_id TEXT REFERENCES jobs(id);
CREATE INDEX host_operations_job ON host_operations(job_id) WHERE job_id IS NOT NULL;
CREATE INDEX spool_reservations_holder ON spool_reservations(holder_kind, holder_id);
```

Then it rebuilds `operations` exactly as 0007:92–104 does (create
`operations_p7` with every earlier kind plus the 15 new ones, copy, drop,
rename).

Notes:

- `ALTER TABLE … ADD COLUMN … REFERENCES` is legal because the new column
  defaults to NULL. The P6 terminal trigger is unaffected. It doesn't list
  `job_id`, so any UPDATE of `job_id` on a terminal row raises, and P7
  never updates it: `job_id` is written only at insert
  (`NewHostOperation.job_id`).
- `queue_entries.job_id` and `jobs.queue_entry_id` reference each other.
  The assign transaction inserts the Job first (its entry exists), then
  sets the entry's `job_id`, so immediate foreign keys hold.
- `host_job_id` is an INTEGER (the parsed Moonraker id, like
  `history_mark`), not TEXT as in the plan. Matching is numeric.
- No column name contains `credential`, `secret`, or `key`.
  `printer_snapshot_json` holds `{ name, location, catalogRef, adapterKind,
  profile }` and never an endpoint or credential reference.

### Wire types (ts-rs, camelCase)

```ts
// queue
type QueueEntryState = "queued" | "assigned" | "closed";
type CloseReason = "completed" | "failed" | "cancelled" | "released" | "removed";
type DispatchPolicy = "manual" | "recommended" | "automatic";
type DispatchPreference = "loadedFirst" | "leastRecentlyUsed";
type EstimateSource = "sliceEstimate" | "fileClaimConfirmed" | "operatorEntered";
type MaterialEstimate = { amountMg: number; source: EstimateSource };
type OriginKind = "retry" | "release";
type QueueEntryAction = "assign" | "update" | "move" | "remove";
type QueueEntryDisplay = {
  modelId: string; modelName: string; plateLabel: string | null; targetLabel: string;
  materialFamily: MaterialFamily | null; materialOther: string | null; printSeconds: number | null;
};
type QueueEntry = {
  id: string; revision: number; sliceRevisionId: string;
  lineageId: string; copyIndex: number; copyCount: number;
  originEntryId: string | null; originKind: OriginKind | null;
  state: QueueEntryState; closeReason: CloseReason | null; position: number | null;
  policy: DispatchPolicy; preference: DispatchPreference; estimate: MaterialEstimate;
  manualPrinterId: string | null; jobId: string | null;
  requiresManualPrinterSelection: boolean;
  allowedActions: QueueEntryAction[];
  display: QueueEntryDisplay;
  createdAt: string; updatedAt: string; closedAt: string | null;
};
type BlockerCode =
  | "PRINTER_ARCHIVED" | "SETUP_INCOMPLETE" | "CONNECTION_ERROR" | "PRINTER_OFFLINE"
  | "JOB_ACTIVE" | "HOST_OPERATION_PENDING" | "PRINTER_BUSY_EXTERNAL" | "PRINTER_NOT_IDLE"
  | "PINNED_TO_OTHER_PRINTER" | "PROFILE_MISMATCH" | "NEEDS_MANUAL_PRINTER"
  | "CAPABILITY_UNSUPPORTED" | "ADAPTER_NOT_PROVEN"
  | "NO_COMPATIBLE_SPOOL" | "INSUFFICIENT_MATERIAL" | "SPOOL_NOT_LOADED" | "PRINTER_NOT_READY";
type Blocker = { code: BlockerCode; message: string; detail: string | null;
                 recovery: RecoveryCode | null; printerIds: string[] };
type SpoolOption = { spoolId: string; spoolNumber: number; loadedOnPrinter: boolean; availableMg: number };
type Candidate = {
  printerId: string; printerName: string; rank: number;   // 1-based
  spool: SpoolOption; spoolOptions: SpoolOption[];
  loadedMatch: boolean; lastUsedAt: string | null;
  manualFactsAcknowledgementRequired: boolean;
};
type PrinterEligibility = { printerId: string; printerName: string; eligible: boolean; blockers: Blocker[] };
type EligibilityVerdict = "blocked" | "awaitingOperator" | "ready";
type QueueEntryEligibility = {
  entryId: string; verdict: EligibilityVerdict; candidates: Candidate[];
  printers: PrinterEligibility[]; blockers: Blocker[]; evaluatedAt: string;
};
type EligibilitySummary = {           // ruling R3, plus `verdict`
  entryId: string; verdict: EligibilityVerdict; eligibleCount: number;
  topBlocker: Blocker | null; candidatePrinterIds: string[];
};
type QueueSnapshot = {
  streamId: string; snapshotSequence: number;
  entries: QueueEntry[];              // open entries in position order, then the newest 100 closed
  jobs: Job[];                        // every active Job, plus every Job of a listed entry
  requirements: ReconciliationRequirement[];   // every pending or deferred one
  eligibility: EligibilitySummary[];  // one per queued entry
  nextAutomaticAction: NextAutomaticAction;
};
type QueueChange = { entries: QueueEntry[]; jobs: Job[]; requirements: ReconciliationRequirement[] };

// jobs
type JobState = "assigned" | "staging" | "awaitingStart" | "starting" | "printing" | "paused"
  | "completed" | "failed" | "cancelled" | "outcomeUnknown";
type CancelReason = "releasedBeforeStart" | "cancelledBeforeStart" | "cancelledByOperator"
  | "hostCancelled" | "operatorDeclared";
type Settlement = "open" | "notRequired" | "pending" | "deferred" | "settled";
type SettlementMethod = "estimated" | "measured";
type AssignedBy = "operator" | "automatic";
type StartConfirmation = "bedClear" | "unattended";
type JobAction = "stage" | "start" | "pause" | "resume" | "cancel" | "release" | "retry"
  | "declareOutcome" | "settleMaterial" | "correctMaterial";
type JobFailure =
  | { kind: "hostOperationFailed"; at: string; hostOperationId: string; failure: HostOperationFailure }
  | { kind: "hostOperationAbandoned"; at: string; hostOperationId: string }
  | { kind: "refused"; at: string; code: ErrorCode; message: string };
type PrinterSnapshot = { name: string; location: string | null; catalogRef: CatalogRef | null;
                         adapterKind: string | null; profile: PrinterProfile };
type Job = {
  id: string; revision: number; queueEntryId: string; sliceRevisionId: string;
  printerId: string; printerSnapshot: PrinterSnapshot;
  spoolId: string; reservationId: string; estimateMg: number;
  state: JobState; cancelReason: CancelReason | null;
  settlement: Settlement; settlementMethod: SettlementMethod | null;
  settlementPreview: { estimatedUseMg: number } | null;   // ruling R4
  corrected: boolean;
  assignedBy: AssignedBy; startConfirmation: StartConfirmation | null;
  uploadHostOperationId: string | null; activeHostOperationId: string | null;
  hostPath: string | null; maxProgressPct: number;
  hostUnreachableSince: string | null;   // ruling R5; printing/paused only
  lastFailure: JobFailure | null;
  startBlockers: Blocker[];            // non-empty only in awaitingStart
  allowedActions: JobAction[];
  createdAt: string; updatedAt: string; startedAt: string | null; endedAt: string | null;
};
type JobEventKind = "assigned" | "stageHandedOff" | "stageSucceeded" | "stageFailed"
  | "startHandedOff" | "startSucceeded" | "startFailed" | "startAbandoned" | "hostJobPinned"
  | "pauseHandedOff" | "resumeHandedOff" | "cancelHandedOff" | "controlFailed"
  | "paused" | "resumed" | "completed" | "failed" | "cancelled" | "outcomeUnknown"
  | "declaredCompleted" | "declaredFailed" | "declaredCancelled"
  | "released" | "cancelledBeforeStart"
  | "materialSettled" | "materialDeferred" | "materialCorrected";
type JobEvent = { id: string; jobId: string; sequence: number; kind: JobEventKind;
                  fromState: JobState | null; toState: JobState;
                  operationId: string | null; hostOperationId: string | null;
                  detail: Record<string, unknown> | null; at: string };
type RequirementKind = "materialReconciliation" | "jobOutcomeUnknown";
type RequirementStatus = "pending" | "deferred" | "resolved";
type DeclaredOutcome = "completed" | "failed" | "cancelled";
type RequirementResolution =
  | { kind: "settled"; method: SettlementMethod; usedMg: number }
  | { kind: "declared"; outcome: DeclaredOutcome };
type ReconciliationRequirement = {
  id: string; jobId: string; kind: RequirementKind; status: RequirementStatus;
  spoolId: string | null; reservationId: string | null;
  openedAt: string; deferredAt: string | null; resolvedAt: string | null;
  resolution: RequirementResolution | null;
};
type SettleChoice = { kind: "estimated" } | { kind: "measured"; entry: AmountEntry } | { kind: "defer" };
type JobHistory = {
  job: Job; entry: QueueEntry; lineage: QueueEntry[];
  events: JobEvent[];                 // sequence order
  reservations: Reservation[]; hostOperations: HostOperation[];
  requirements: ReconciliationRequirement[];
};
```

`HostOperation` gains `jobId: string | null`. `SpoolFacets` gains
`reconciliation: boolean` (any `unresolved` reservation). The Rust
backend-only fields are `history_mark`, `host_job_id`,
`start_host_operation_id`, and `inconclusive_checks`.

### Commands

18 commands, each registered per Global Constraint 8. Count assertions
add 18 to `main`'s expression (`58 + 21 + 2 + 8 + 18`). Arguments are
top-level camelCase fields.

| Command | Arguments | Result |
|---|---|---|
| `list_queue` | `{}` | `QueueSnapshot`. Until the evaluator has run, `eligibility` is computed synchronously and `nextAutomaticAction` is `{ kind: "evaluatorNotRunning" }` (R3, D6) |
| `add_to_queue` | `{ operationId, sliceRevisionId, quantity: 1..=50, policy, preference, materialEstimate?: MaterialEstimate, manualPrinterId?: string }` | `QueueChange` (the N entries) |
| `update_queue_entry` | `{ operationId, entryId, expectedRevision, policy?, preference? }` | `QueueChange` |
| `move_queue_entry` | `{ operationId, entryId, expectedRevision, toPosition }` | `QueueChange` (every renumbered entry) |
| `remove_queue_entry` | `{ operationId, entryId, expectedRevision }` | `QueueChange` (the closed entry and every renumbered one) |
| `explain_queue_entry` | `{ entryId }` | `QueueEntryEligibility` |
| `assign_queue_entry` | `{ operationId, entryId, printerId, spoolId, acknowledgeManualFacts?: boolean }` | `QueueChange` (entry and Job) |
| `stage_job` | `{ operationId, jobId }` | `QueueChange` (the Job, `staging`) |
| `start_job` | `{ operationId, jobId, priorState: PriorState, acknowledgement: "bedClear" }` | `QueueChange` (the Job, `starting`) |
| `pause_job` | `{ operationId, jobId }` | `QueueChange` |
| `resume_job` | `{ operationId, jobId }` | `QueueChange` |
| `cancel_job` | `{ operationId, jobId }` | `QueueChange`: before start, the Job `cancelled` and the entry closed; after start, the Job with its cancel op active |
| `release_job` | `{ operationId, jobId }` | `QueueChange` (the Job, the closed entry, the replacement) |
| `retry_job` | `{ operationId, jobId }` | `QueueChange` (the new entry). A second retry of the same Job is `JOB_ALREADY_RETRIED` |
| `declare_job_outcome` | `{ operationId, jobId, outcome: DeclaredOutcome, acknowledgement: "hostStateUnknown" }` | `QueueChange` |
| `settle_job_material` | `{ operationId, jobId, choice: SettleChoice }` | `QueueChange` |
| `correct_job_material` | `{ operationId, jobId, entry: AmountEntry }` | `QueueChange` |
| `get_job_history` | `{ jobId }` | `JobHistory` |

**`add_to_queue` validation**, all `VALIDATION` with the field path:

- `quantity` outside 1..=50: `quantity`.
- The Slice Revision must exist (`NOT_FOUND`).
- `materialEstimate`:
  - a farm3d revision with `filamentGrams` present: must be absent or
    equal `{ amountMg: grams_to_mg_round_up(filamentGrams), source:
    "sliceEstimate" }`. farm3d fills it when absent.
  - an External revision, or `filamentGrams` null: required, with source
    `fileClaimConfirmed` (then `amountMg` must equal the rounded-up
    claimed grams) or `operatorEntered` (any `amountMg > 0`). Missing:
    `materialEstimate`.
- `requiresManualPrinterSelection`: `policy` must be `manual`
  (`policy`), and `manualPrinterId` is required (`manualPrinterId`).
- `manualPrinterId` is allowed only with the Manual policy, and must name
  an existing Printer (`NOT_FOUND`).

`update_queue_entry` applies the same policy rules and rejects a policy
other than Manual on an entry with `manualPrinterId` set (`policy`).

**`assign_queue_entry`** takes the Printer lock, then runs D4's assign
transaction. The entry must be `queued` (`QUEUE_ENTRY_ACTION_NOT_ALLOWED`)
and the Printer have no active Job (`JOB_ACTIVE`).
`check_assignment(…, AssignMode::Operator { acknowledge_manual_facts })`
must pass (`ASSIGNMENT_BLOCKED`). An absent fact without
`acknowledgeManualFacts: true` is `VALIDATION` on
`acknowledgeManualFacts`. After commit, it publishes and asks the driver
to stage.

Every mutating command, after commit: `QueueStream::publish` of the
changed rows, `spools::events::publish_ids` for touched Spools, one
`InventoryChange` if a reservation changed, and one evaluator poke.
Nothing is published for a replay.

### Events

The **`queue` stream** is its own sequence on `farm3d-event-v1`, in the
usual `EventEnvelope`, following `host_ops/events.rs`. The payload is a
tagged union, as `InventoryEventPayload` is.

| Type | Subject | Payload |
|---|---|---|
| `queue.entry.changed` | `{ kind: "queueEntry", id }` | `{ type: "entryChanged", entry: QueueEntry }` |
| `queue.job.changed` | `{ kind: "job", id }` | `{ type: "jobChanged", job: Job }` |
| `queue.requirement.changed` | `{ kind: "reconciliationRequirement", id }` | `{ type: "requirementChanged", requirement: ReconciliationRequirement }` |
| `queue.eligibility.changed` | `{ kind: "queue", id: "eligibility" }` | `{ type: "eligibilityChanged", summaries: EligibilitySummary[], nextAutomaticAction: NextAutomaticAction }` (the full set, not a diff) |

- One event per changed row, entries first, then Jobs, then
  requirements, after commit only, never for a replay.
- `list_queue` reads `snapshot_sequence()` before it reads rows (listen
  before backfill).
- A Job's `startBlockers` depend on live status, which has no queue
  event. The frontend recomputes nothing; when that Printer's status
  changes, the driver republishes `queue.job.changed` for its
  `awaitingStart` Job if its `startBlockers` changed.
- Host Operations keep their own `hostOperations` stream. Spool changes
  go through `spools::events::publish_ids`.
- `QueueEventType` and `QueueEventPayload` are exported.

### Error codes

`ErrorCode` gains the codes below. `details` values are camelCase JSON
and never carry a credential, host body, or host URL.

| Code | Raised by | `details` | `recovery` | Message |
|---|---|---|---|---|
| `JOB_ACTIVE` | `assign_queue_entry`; the raw P6 write commands | `{ printerId, jobId }` | `[OPEN_JOB]` | "This Printer has an active Job. Use the Job's controls." |
| `JOB_ACTION_NOT_ALLOWED` | every Job command whose action the state doesn't allow (D3 table) | `{ jobId, action: JobAction, state: JobState }` | `[RELOAD]` | "This Job can't <action> while it is <state label>." |
| `QUEUE_ENTRY_ACTION_NOT_ALLOWED` | assign, update, move, remove on an entry whose state doesn't allow it | `{ entryId, action: QueueEntryAction, state: QueueEntryState }` | `[RELOAD]` | "This Queue Entry is <state label>." (plus "Release or cancel its Job instead." for remove on `assigned`) |
| `ASSIGNMENT_BLOCKED` | `assign_queue_entry` when `check_assignment` fails inside the tx | `{ entryId, printerId, spoolId, blockers: Blocker[] }` | `[RELOAD]` | the first blocker's message |
| `JOB_START_BLOCKED` | `start_job` when `start_blockers` is non-empty | `{ jobId, blockers: Blocker[] }` | the first blocker's `recovery`, if any | the first blocker's message |
| `JOB_NOT_ON_PRINTER` | `pause_job`, `resume_job`, `cancel_job` after start, when the host reports a file other than the Job's | `{ jobId, printerId }` | `[RELOAD]` | "The printer is printing a different file than this Job's." |
| `JOB_ALREADY_SETTLED` | `settle_job_material` on `settled`; `correct_job_material` on a corrected Job | `{ jobId, reason: "settled" \| "corrected" }` | `[RELOAD]` | "This Job's material is already settled." / "This Job already has a correction." |
| `JOB_ALREADY_RETRIED` | `retry_job` on a Job whose entry already has a retry | `{ jobId, retryEntryId }` | `[RELOAD]` | "This Job was already retried." |
| `JOBS_EXIST` | `import_printers` (`replace_all`) | `{ printerIds: string[], jobIds: string[], queueEntryIds: string[] }` (each list at most 20) | `[]` | "Printers with Job history can't be replaced by an import." |
| `INSUFFICIENT_MATERIAL` | `ReservationError::InsufficientAvailable` (backstop; the gates normally catch it) | `{ spoolId, availableMg, requiredMg }` | `[RELOAD]` | "Spool #<n> no longer has enough material." |
| `SPOOL_NOT_RESERVABLE` | `ReservationError::SpoolNotReservable` | `{ spoolId, lifecycle }` | `[RELOAD]` | "Spool #<n> is <lifecycle> and can't be reserved." |
| `RESERVATION_STATE` | `ReservationError::InvalidTransition` | `{ reservationId, state }` | `[RELOAD]` | "The reservation changed. Reload." |

Existing codes reused, unchanged: `VALIDATION` (including a reused
`operationId`), `NOT_FOUND`, `CONFLICT` (a stale `expectedRevision`),
`LIFECYCLE_BLOCKED` (with the four new `LifecycleBlockerCode`s),
`CONNECTION_IN_USE` (below), and every P6 code that `host_ops::api` returns
(`HOST_OPERATION_PENDING`, `CAPABILITY_UNSUPPORTED`, `START_NOT_ALLOWED`,
`START_PRECONDITION_CHANGED`, `CONTROL_NOT_ALLOWED`,
`STAGED_ARTIFACT_INVALID`, `PRINTER_UNREACHABLE`, `TIMEOUT`,
`AUTHENTICATION_FAILED`, `PROTOCOL_ERROR`).

**`CONNECTION_IN_USE` for a Job.** P6 raises it while the Printer has an
unresolved Host Operation. When that operation is linked to a Job
(`job_id` set), `details` are `{ printerId, hostOperationId, jobId }`,
`recovery` is `[OPEN_JOB]`, and the message is: "This Printer's Job is
waiting on a printer operation. Let it finish, or abandon the check from
the Job, before changing this Connection." A raw (unlinked) operation
keeps P6's details, recovery, and message. `RepositoryError::ConnectionInUse`
gains `job_id: Option<String>`.

`RecoveryCode` gains `OPEN_JOB` (open the Job in Queue),
`OPEN_PRINTER_SETUP`, `UNARCHIVE_PRINTER`, `LOAD_SPOOL` (open Spools
filtered to compatible Spools), and `ASSIGN_MANUALLY` (switch the entry to
Manual and open Assign). `SETTLE_MATERIAL` (open the settle dialog) is
used by the Spool dock for a `reconciliation` Spool.

`RepositoryError` gains `IllegalQueueEntryTransition`,
`IllegalJobTransition` (both `INTERNAL` when unmapped, D2/D3),
`QueueEntryActionNotAllowed`, `JobActionNotAllowed`, `JobActive`,
`AssignmentBlocked`, `JobNotOnPrinter`, `JobAlreadySettled`,
`JobAlreadyRetried`, and `JobsExist`. Each maps in `CommandError::from_repository` to
the code above.

## Frontend architecture

Rust is the only source of state, eligibility, blockers, allowed actions,
settlement, and order. TypeScript presents them.

- **`src/queue/queue-store.ts`**: listen-before-backfill over `queue.*`
  through `createSequencedStream`, backfilled from `list_queue`. Reads:
  `entries()` (open entries in `position` order), `history()` (closed),
  `entry(id)`, `jobFor(entryId)`, `job(id)`, `activeJobFor(printerId)`,
  `requirements()`, `eligibility(entryId)`, `nextAutomaticAction()`,
  `syncState`. Writes: one per command, each with a fresh
  `crypto.randomUUID()` reused on one transport retry. Each patches the
  store from the returned `QueueChange`.
- **`views.ts`**: `QueueView = "awaitingOperator" | "ready" | "assigned" |
  "blocked" | "printing" | "history"`. `viewOf(entry, job, summary)` maps:
  `queued` → the summary's `verdict`; `assigned` with Job `printing` or
  `paused` → `printing`; other `assigned` → `assigned`; `closed` →
  `history`.
- **`presentation.ts`**: labels for every `JobState`, `CancelReason`,
  `Settlement`, `BlockerCode`, `RequirementKind`, `JobEventKind`, and new
  `RecoveryCode`. A `SPOOL_NOT_LOADED` start blocker reads "Awaiting
  material". `outcomeUnknown` reads "Outcome unknown".
- **Components** (plan Tasks 13–15): `ReorderHandle`, `QueueScreen`,
  `QueueEntryDetail`, `AddToQueueDialog`, `AssignJobDialog`, `JobPanel`,
  `StartJobDialog`, `SettleMaterialDialog`, `DeclareOutcomeDialog`, and
  `QueuePreview`. Every action button is rendered from `allowedActions`
  and disabled with visible text from `startBlockers` or the blocker.
- **Navigation:** the Queue destination's selection kind stays `job`
  (`queue: Set(["job"])`). The id is a Queue Entry id (`qen-*`) or a Job
  id (`job-*`), resolved by prefix. After Add to Queue the app navigates
  to `#nav=v1/queue/job/<first entry id>`.
- **The Job's Host Operation in the UI.** `JobPanel` shows the Job's
  active Host Operation from the host-operations store, with P6's **Check
  again** and **Abandon check…** (P6's enabling rules). That is the only
  exit from a Job stuck in `staging` or `starting` (D3).
- **Decision 9 in the UI:** with an active Job, `PrinterJobPanel`
  renders `JobPanel` and hides P6's raw Stage and Start. Pause, Resume,
  and Cancel go through the Job.
- **Deferred requirements** show in the Queue (History view and
  `QueuePreview`) and on the Spool (`reconciliation` facet, "Needs
  reconciliation" chip, **Settle…** in the Spool dock).
- The activity bar gets a Queue button. Its badge counts entries whose
  verdict is `blocked` or `awaitingOperator`, plus open requirements.
  Its aria-label is "Queue (N need attention)".

## Accessibility and adaptation

- Reordering never depends on dragging. `ReorderHandle` supports
  Alt+ArrowUp/Down and Alt+Home/End, a companion menu (Move to top, up,
  down, to bottom), and a pointer drag that Escape cancels. A polite live
  region announces "Moved <label> to position N of M".
- Every dialog is a Kobalte `Dialog` or `AlertDialog`, traps focus, and
  returns it. The Start confirmation and the Declare acknowledgement are
  real checkboxes. Confirm stays disabled until they're ticked, and its
  reason is visible text.
- Job state changes are announced through one `aria-live="polite"` region
  in `JobPanel`. Failures use `role="alert"` with their recovery action.
- State, blockers, and settlement always pair an icon, text, and color.
- The layout works at 1440 × 900 and 1024 × 700, where the dock becomes
  an overlay. Tokens, CSS Modules, and Kobalte only, in the dense editor
  aesthetic.

## Acceptance criteria

1. **Migration.** 0008 applies to a P6 database and keeps every
   `operations` and `host_operations` row. The new kinds are accepted.
   The partial unique indexes reject a second active Job per Printer and
   a duplicate open position. 0008 is atomic. No new column name contains
   `credential`, `secret`, or `key`.
2. **State machines.** Every pair in D2's table and every `(JobState,
   JobEventKind)` pair is tested against the tables here. Illegal pairs
   are rejected before SQL.
3. **Transactions and concurrency.** Assignment is atomic; a failed
   reserve leaves nothing. Two concurrent assigns of one entry, or of two
   entries to one Printer, yield one Job. Replays publish nothing.
4. **Restart matrix.** One test per row R1–R22, with no duplicate upload
   and no unconfirmed start.
5. **Eligibility.** One test per gate, per failing and passing case, and
   fixtures F1–F8 verbatim.
6. **Evaluator.** It waits for recovery, assigns top to bottom, never
   assigns Manual or Recommended entries, never runs two passes at once,
   and never assigns behind an unproven adapter.
7. **Tracker.** Every history status in D7 maps as stated. A foreign
   print is never adopted. A quick print still completes. Three
   inconclusive polls give `outcomeUnknown`.
8. **Lineage.** Three copies stay linked but independently assignable,
   releasable, retryable, cancellable, and settleable.
9. **Settlement.** Completed, failed, and cancelled Jobs, through
   estimated, measured, and deferred choices, each settle exactly once.
   A deferred amount stays unavailable, and a later entry needing it is
   `INSUFFICIENT_MATERIAL`.
10. **Guards.** Every D8 row is blocked and then allowed as stated. No
    row is orphaned (`PRAGMA foreign_key_check`).
11. **Decision 9.** A raw P6 write on a Printer with an active Job is
    `JOB_ACTIVE`.
12. **Secrets.** A seeded credential never appears in a Job, event,
    requirement, snapshot, error, or log line.
13. **Frontend.** The Task 12–15 tests pass, and screenshots exist at both
    viewports.
14. **Tracer.** The plan's Task 16 tracer passes on `FakeMoonraker` (CI)
    and the simulator, with the manifest committed.

## Delivery

| Task | Delivers from this spec |
|---|---|
| 2 | Schema, `OperationKind`s, both state machines, `settlement_after` |
| 3 | Queue and Job repositories, positions, lineage, events, requirements |
| 4 | `for_holder`, `consume_measured`, the `reconciliation` facet, the reservation error codes |
| 5 | `slicing/compat.rs`, D5 gates, ranking, fixtures F1–F8 |
| 6 | The queue commands, assign, release, retry, cancel before start, `get_job_history`, the `queue` stream, `QueueChange`, `EligibilitySummary` and `evaluatorNotRunning` (R3) |
| 7 | `host_ops::api`, `NewHostOperation.job_id`, the broadcasts, the raw-write `JOB_ACTIVE` guard |
| 8 | Driver, `apply_host_outcome`, start, control, tracker, recovery, declare, R1–R22, `host_unreachable_since` |
| 9 | Settlement, `settlementPreview` (R4) |
| 10 | The evaluator and `NextAutomaticAction` |
| 11 | D8 guards, `JOBS_EXIST`, and the Job case of `CONNECTION_IN_USE` |
| 12–15 | Frontend |
| 16 | Tracer |
| 17 | Verification record |

## Decisions made in this spec

Each departs from, or sharpens, the plan's Design reference.

1. **`Settlement` gains `open`** for active Jobs, and
   `settlement_after` takes the event, not `(state, reached_starting)`.
   A start that failed definitively and was then cancelled printed
   nothing, so "reached starting" was the wrong test.
2. **Retry is refused for a released Job.** Its replacement is already
   queued.
3. **A retry or release replacement keeps its origin's `copy_index`**,
   so "Copy 2 of 3" stays true.
4. **The Printer import guard is `JOBS_EXIST` (any Job)**, not
   `JOBS_ACTIVE`, because `replace_all` deletes every Printer and Job
   history is `RESTRICT`.
5. **`manual_printer_id` is `ON DELETE SET NULL`**, and the delete guard
   blocks only for open entries.
6. **Blocker codes added:** `PRINTER_OFFLINE`, `PRINTER_NOT_IDLE`,
   `PINNED_TO_OTHER_PRINTER`, and `PRINTER_NOT_READY`.
7. **Error codes named:** `JOB_ACTION_NOT_ALLOWED`,
   `QUEUE_ENTRY_ACTION_NOT_ALLOWED`, `ASSIGNMENT_BLOCKED`,
   `JOB_START_BLOCKED`, `JOB_NOT_ON_PRINTER`, and `JOBS_EXIST`. A reused
   `operationId` stays `VALIDATION`, as in P3–P6. The plan's Global
   Constraint 9 said `OPERATION_ID_REUSED`, which doesn't exist.
8. **Two operation ids per handoff** (`<id>#hostOperation` for the Host
   Operation), because P6 claims its own id in the same ledger.
9. **Job code never holds the Printer lock around `host_ops::api`**,
   which takes it itself. The lock is non-reentrant.
10. **The driver stages once.** After a refusal or failure it waits for
    the operator. A restart re-stages only a Job that never tried.
11. **`host_job_id` is an INTEGER**, matched numerically.
12. **Pause, resume, and cancel check the host's file** against the Job's
    (`JOB_NOT_ON_PRINTER`), inside the write-ahead link.
13. **`EligibilitySummary` carries `verdict`** beside ruling R3's fields,
    so TypeScript never derives it.
14. **`Job.allowedActions` and `QueueEntry.allowedActions`** are
    Rust-computed, so the UI never decides what is offered.
15. **Every mutating command returns `QueueChange`**, the same rows its
    events carry.
16. **`add_to_queue` refuses a non-Manual policy, or a missing
    `manualPrinterId`, for a revision with absent facts.** The Queue
    never holds an entry that can't be dispatched as configured.
17. **The plan's single `OutcomeDeclared` is three events** (`DeclaredCompleted`,
    `DeclaredFailed`, `DeclaredCancelled`), so the transition function
    needs no payload.
18. **`MaterialSettled` and `MaterialDeferred` apply to `failed` and
    `cancelled` only; `MaterialCorrected` to `completed` only.** Settling
    a `settled` Job is `JOB_ALREADY_SETTLED`, checked before the state
    table.
19. **Ruling R5 (fix round 1).** `declare_job_outcome` is also allowed
    from `printing` or `paused` after 30 minutes without a successful host
    read (`host_unreachable_since`). An active Job alone never blocks a
    Connection change; only an unresolved Host Operation does. `stage_job`
    may stage again from `awaitingStart`.
20. **Renumbering parks rows at `position + 1000000`** before writing
    final positions, because the CHECK and the unique index are checked
    row by row.
21. **A Job is retried at most once** (`JOB_ALREADY_RETRIED`), enforced
    by a unique index on `origin_entry_id`.
22. **The evaluator is a pure pass plus an async loop.** Only the loop
    takes the Printer lock.
23. **The tracker's pin reuses P6's rule (b)**: `job_id > history_mark`
    and `start_time ≥ dispatched_at − 30 s`. The Job keeps
    `start_host_operation_id` for this.
24. **The fail-safe principle covers a Job's end.** `printing` ⇄ `paused`
    may follow status on the Job's own file.

## Residual risks

- Moonraker history pagination: the tracker reads the newest 50 jobs. A
  farm that runs more than 50 other jobs on one Printer during one Job
  would lose the pinned job from the page and end `outcomeUnknown`.
- Declaring a Job's end after 30 minutes without the host is an operator
  judgment. If the printer was in fact still printing, the next entries
  see it as `PRINTER_BUSY_EXTERNAL` once it is reachable again.
- A foreign print during an `awaitingStart` Job gets no farm3d control;
  the operator uses the printer (decision 9).
- A Printer import is impossible once any Job exists, until P9 adds
  history pruning. Printers can still be added, edited, and archived one
  at a time.
- Unattended start trusts `Ready` with fresh telemetry to mean an empty
  bed, per decision 1. farm3d cannot see the bed.
- The Snapmaker fork was never written (P6). Its history statuses are
  assumed to match the simulator's.
