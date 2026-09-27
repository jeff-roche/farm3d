# P7 Queue Entries, Jobs, and Dispatch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. One fresh implementer per `### Task N` section, and a review after each task. Every task section is written to stand alone. It still binds you to **§Global Constraints**, which the controller hands to every implementer with the task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Final. The owner confirmed every recommended decision on 2026-09-26.
GitHub issue #17. Written on 2026-09-26 against `main` at `5b807b4` (the
P6 merge, PR #30). The issues it depends on (#11, #12, #13, #15, #16) are
closed. #16 closed with a complete Moonraker P6 path.

**Approval.** There are no user approval stops. Where this plan says
**controller approves**, the controller (the agent orchestrating
subagent-driven development) reviews the output, records the approval in
the document's own `Status` line, and continues. That applies to the
focused spec and ADR-0013 (Task 1).

**Goal:** Deliver durable, ordered Queue Entries and authoritative Jobs.
Eligibility is explainable. Printer and Spool assignment is
transactional. Staging and start are safe and handed off idempotently
through P6 Host Operations. Copy lineage is preserved, every transition
survives a restart, and material is accounted for exactly once when a Job
completes, fails, or is cancelled.

**Architecture:** Two new Rust domain modules. `queue` owns Queue Entries,
order, lineage, eligibility, and the automatic evaluator. `jobs` owns the
Job state machine, assignment transactions, the dispatch driver that
hands work to `host_ops`, outcome tracking, material settlement, and
startup recovery. `host_ops` gains a Rust-callable, Job-linked write-ahead
and an in-process change broadcast. `ConnectionManager` gains a status
broadcast. The frontend adds a Queue destination (`src/queue/`), Job
views, and a Monitor Queue preview. It presents Rust-computed states,
eligibility, and blockers, and never derives them from host strings.

**Tech Stack:** Rust (Tauri 2, rusqlite STRICT tables, tokio, ts-rs),
SolidJS + TypeScript, Kobalte, CSS Modules, vitest +
`@solidjs/testing-library`, the in-process `FakeMoonraker`, and the
`sim/` Klipper/Moonraker simulators (ADR-0012).

**Spec:** Task 1 writes
`docs/superpowers/specs/2026-09-26-p7-queue-entries-jobs-dispatch-design.md`.
Until it exists, this plan's §Design reference is the design. **Once the
spec is approved, the spec wins wherever the two differ**, and Task 1
updates the affected tasks in the same commit. The umbrella inputs are
`docs/superpowers/specs/2026-09-16-complete-v1-ui-workflows-design.md`
("Queue Entries and Job dispatch", "Reservation and reconciliation",
"History") and `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`
("Phase P7").

## Owner decisions (fixed)

The owner confirmed these on 2026-09-26. Don't reopen them.

1. **Unattended start ships in P7.** A Job on a Printer whose Start-safety
   rule is `Unattended` starts without confirmation once it is staged, its
   Spool is loaded, and the Printer is `Ready`, whatever its Dispatch
   Policy. It never starts from `Finished` or `Cancelled`, because the
   bed may not be clear. Every other start needs the P6 bed-clear
   confirmation. (Tasks 8, 10.)
2. **An "automatic" action needs a sim-proven adapter.** Automatic
   assignment and unattended start need the Printer's `upload`, `start`,
   `hostState`, and `artifactIdentity` capabilities to be `supported`
   with `tier: sim` evidence. Today only Moonraker qualifies. OctoPrint
   stays `notVerified`, so it can't be assigned at all. (Tasks 5, 10.)
3. **One Spool per Job.** P5 slices one filament per plate, so a Job
   reserves exactly one Spool. The schema doesn't prevent more later.
   (Tasks 2, 5.)
4. **The material estimate is fixed when a Queue Entry is created.**
   - For a farm3d Slice Revision it is `ceil(filamentGrams)` in
     milligrams.
   - For an External Slice Revision, or a farm3d one whose
     `filamentGrams` is null, the operator must either accept the file's
     claimed grams or enter an amount. Either way it is recorded with its
     source.
   - The Queue Entry can't be created without an estimate. (Tasks 3, 14.)
5. **Estimated use for a failed or cancelled Job** is
   `ceil(estimateMg × maxObservedProgress)`, where progress is the
   highest the host reported for this Job (0 if it never printed). The
   dialog shows this figure before the operator confirms it. (Task 9.)
6. **Completed Jobs deduct automatically.** The full estimate is consumed
   in the same transaction that completes the Job. The measured
   correction is optional and can be recorded once per Job. (Task 9.)
7. **A Printer with Job history can be archived, not deleted.** Deleting
   it would orphan Job history, and P9 owns history pruning. A Slice
   Revision that any Queue Entry or Job references can't be deleted
   either. (Task 11.)
8. **The dispatch preference is per Queue Entry**, and is either
   `loadedFirst` (the default: prefer a Printer that already has a
   matching Spool loaded) or `leastRecentlyUsed`. Ties then go to Printer
   name (case-insensitive), then Printer id. (Task 5.)
9. **P6's raw controls step aside for a Job.** While a Printer has an
   active Job, the Job tab replaces P6's raw Stage/Start controls with
   the Job's own controls, and pause, resume, and cancel act through the
   Job. Raw P6 controls stay available on a Printer with no active Job.
   farm3d never adopts a print it didn't start as a Job. (Tasks 8, 15.)
10. **Live evidence comes from the simulators** (ADR-0012). The
    live-host tracer runs against `sim/` Moonraker. P7 adds no write to
    real hardware, and the only real-host check it adds is an optional
    read-only one. (Task 16.)

## Evidence: the code on `main` at `5b807b4`

**P3 reservation primitives** (`src-tauri/src/spools/`):

- The `spool_reservations` table is in `0004_p3_spools_material_slots.sql`:
  - columns `id`, `spool_id`, `holder_kind`, `holder_id`, `amount_mg > 0`,
    `state IN ('active','unresolved','released','consumed')`,
    `operation_id`, `created_at`, `settled_at`;
  - an open index on `spool_id WHERE state IN ('active','unresolved')`;
  - no index on the holder, and no FK on it.
- The functions in `reservations.rs` all take `&Transaction`:
  - `reserve(tx, spool_id, &ReservationHolder, amount_mg, operation_id) -> Result<String /* rsv-* */, ReservationError>`
    (:163). It needs an `active` lifecycle, and fails with
    `InsufficientAvailable { available_mg }`.
  - `release` (:207).
  - `mark_unresolved` (:231). It only accepts `active`.
  - `consume(tx, reservation_id, used_mg, note) -> AmountEvent` (:253).
    It accepts `active` or `unresolved`, writes a `Consumption` ledger
    row with `Estimated` confidence, and clamps at 0.
  - `availability` (:315), `open_reservations` (:329), and `history`
    (:346).
- `ReservationHolder { kind, id }` is opaque. P7 uses `("job", <jobId>)`.
- `availableMg = currentMg − Σ(active + unresolved)`, and it can go
  negative (P3 D8).
- Reservation writes **don't bump `spools.revision` and emit no event**.
  The caller publishes through `spools::events::publish_ids`. The
  frontend store patches availability on a same-revision
  `spool.changed`.
- `spools/weight.rs:24` has `grams_to_mg_round_up(f64) -> i64`.
- The ledger (`ledger.rs:203`) is
  `append(tx, spool_id, AmountEventKind, after_mg, AmountConfidence, LedgerSnapshot)`.
  A `Measurement` or `Estimate` after a `Consumption` is a correction.
- `RuntimeServices.inventory_changes: broadcast::Sender<InventoryChange { spool_ids, printer_ids }>`
  (`lib.rs:67`) is sent after commit only. It has no subscriber yet;
  P3 left it for "P7's evaluator".
- Archiving or marking empty a Spool with open reservations fails with
  `SpoolReserved`. Moving a reserved Spool is allowed.
- There is **no** material matching between a Slice and a Spool, and no
  reconciliation-needed facet. The Spools badge counts `low` only
  (`App.tsx:288`, `ActivityBar.tsx:14`).

**P5 Slice Revisions** (`src-tauri/src/slicing/`):

- `slice_revisions` is immutable (triggers). It has `gcode_sha256` and
  `gcode_size` columns, but no DTO exposes them.
- `SliceFacts` (`facts.rs:103`) holds `printer_profile: Fact<ProfileSnapshot>`,
  `nozzle_diameter_mm: Fact<f64>`, `material_family: Fact<MaterialFamily>`,
  `material_other`, and `filament_diameter_mm: Fact<f64>`. Its
  `requires_manual_printer_selection()` (:115) is "P7 reads it".
- `ProfileSnapshot` (`facts.rs:76`) holds `catalog_ref`, `bed_shape`,
  `printable_height_mm`, `bed_exclude_areas`, `nozzle_type`, and
  `gcode_flavor`. It has no nozzle diameter; that is a fact of its own.
- `SliceEstimates { print_seconds, filament_grams: Option<f64>, … }`
  (`mod.rs:443`) are single totals. An External revision has
  `estimates: None` and untrusted `claimed_estimates`.
- Printed bounds aren't persisted; only `max_z_mm` is.
- `presets.rs:679 profiles_match` is private and uses exact `==`.
  `matching_printer_ids` (:691) is a display count, not eligibility.
- `slicing/blockers.rs:30 slice_revision_blocker_sources()` holds P6's
  `UnresolvedHostOperationBlocksRevisionDeletion`, with the comment "P7
  registers Queue Entries and Jobs here".
- `SliceRevisionReview.tsx:182` has a disabled **Add to Queue…** with the
  reason `QUEUE_LATER_REASON` (:48).

**P6 Host Operations** (`src-tauri/src/host_ops/`):

- The writes are Tauri command functions only:
  - `stage_slice_revision` (`commands.rs:323`);
  - `start_staged_artifact(…, host_operation_id /* the upload row */, prior_state)`
    (:458);
  - pause, resume, and cancel go through a private `control_command`
    (:612).
- The private `write_ahead` (:229) re-checks the Connection, the
  unresolved row, and the Slice Revision inside one `write_repo`
  transaction, calls `repository::insert_dispatching(tx, &NewHostOperation)`,
  then `services.publish` and `executor::spawn`.
- `NewHostOperation` (`repository.rs:258`) has no Job link.
- `services.printer_lock(printer_id)` is a per-Printer
  `tokio::sync::Mutex`, held across pre-check host I/O.
  `commit_outcome` (`services.rs:554`) commits and publishes under it.
- `services.capabilities(&printer)` applies cached host facts; prefer it
  over bare `capabilities_for`.
- A start is `Succeeded { StartAccepted }` on a 200. P6 **never tracks a
  print to its end**. The Reconciler's start rule uses `print_stats`, or
  history with `job_id > history_mark` and `start_time ≥ dispatched_at − 30 s`.
- A print that starts and finishes inside one status batch can stay
  `uncertain` until it is abandoned (a known gap in the P6 verification
  record).
- `HostStateQuery::host_job_state()` returns `HostJobState { klippy_state, print: Option<PrintSnapshot { state, filename, is_paused }>, … }`.
  `job_history(HistoryQuery { since_epoch_s, limit })` returns
  `Vec<HistoryJob { job_id: u64, filename, status: String, start_time_epoch_s }>`,
  with no `end_time` and no lookup by id.
- Events go out only to the app (`hostOperations.operation.changed`).
  There is **no in-process broadcast**.
- `ConnectionManager::set_online_hook` holds a single hook, and
  `host_ops` owns it.
- Only one row per Printer can be unresolved (a partial unique index).
- `recover_after_restart(storage, now)` runs at `lib.rs:380`.
- Abandoned is not success, and P7 treats it as reconciliation-required
  (P6 spec D8).
- The Start rule (`start_rule.rs:49`) is
  `check(status, PriorState) -> Result<(), StartRejection>`. It offers
  Ready, Finished, and Cancelled, and never reads `StartSafety`.

**Printers:**

- `StartSafety { ConfirmBedClear (default), Unattended }` is at
  `printers/mod.rs:79`, and nothing reads it.
- Archive state is `StoredPrinter.archived_at`.
- `OperationalState` is SetupIncomplete, Error, Offline, Connecting,
  Unknown, Printing, Paused, Busy, Finished, Cancelled, Failed, or Ready.
  `ReadinessReason::Archived` is reserved for P7.
- `printers::lifecycle::blocker_sources()` (:123) returns
  `[ArchiveStateBlockers, LoadedSpoolBlockers, HostOperationBlockers]`.
  `LifecycleBlockerCode` (:46) is where new codes go.
- `library/blockers.rs:26` holds the Model deletion sources.
- `catalog/resolve.rs:203 resolve_printer(catalog, stored) -> ResolvedPrinter`
  gives `.profile_resolution.profile: PrinterProfile` (with
  `nozzle_diameter_mm: Vec<f64>`).

**Persistence and contracts:**

- The highest migration is `0007_p6_host_operations.sql`, and
  `CURRENT_SCHEMA_VERSION = 7`.
- The `operations` ledger CHECK was last rebuilt in 0007:92–104, by
  create, copy, drop, and rename.
- `Storage::write_repo(|tx| …)` is IMMEDIATE on one writer.
- `COMMAND_NAMES: [&str; 89]`. `tests/f1_contract_path.rs:4` asserts
  `58 + 21 + 2 + 8`.
- A command is registered in `contracts/inventory.rs` (the array length,
  the decl string, and the visitor), `tests/export_contracts.rs`,
  `tests/f1_contract_path.rs` (the count, names, handlers, and cases),
  and `src/ipc/client.ts` `CommandMap`.
- Events go on `farm3d-event-v1` as `EventEnvelope`. The template is
  `host_ops/events.rs` (`HostOperationsStream` with `snapshot_sequence()`
  and `publish`).
- The frontend store pattern is `src/host-ops/host-operations-store.ts`:
  `createSequencedStream`, listen before backfill, web fixtures, a mock
  for screen tests, and a fresh `crypto.randomUUID()` operationId per
  write, retried once on transport failure.

**Frontend shell:**

- `ActivityBar.tsx` has `ScreenId = "monitor" | "library" | "spools"`,
  and no Queue button.
- `App.tsx:56` has `SCREEN_TITLE.queue`, but no Match.
  `availableDestinations = ["monitor","library","spools"]` (:95).
- Rust `NavigationDestination::Queue` and `NavigationSelectionKind::Job`
  exist. `navigation-store.ts` allows `queue: Set(["job"])`.
- The Monitor dock shows Printer detail only, with no Queue preview. Its
  tabs are Status, Setup, and Job (`PrinterJobPanel`).
- The design system has `DataTable`, `Timeline`, `SeverityMarker`,
  `PrinterRoster`, `AlertDialog`, `DropdownMenu`, `NumberField`, and
  `SegmentedControl`. It has **no reorder control**.

**Test assets:**

- `tests/common/fake_moonraker.rs` is HTTP-only. It holds settable
  `FakeState { print_state, print_filename, is_paused, klippy_state, history }`
  and `push_job`. A start adds an `in_progress` job, and a cancel closes
  it as `cancelled`. Tests complete or fail a print by editing the state.
- `p6_tracer.rs` runs one `run_reconciliation_tracer<B: Backend>` against
  both the fake and the simulator, and restarts by rebuilding
  `RuntimeServices`.
- In `tests/sim`, `quick_gcode` (8 MB, under 1 s) and `long_gcode`
  (120 MB, about 10 s) exist, as does `wait_for_print_state`.
  `emergency_stop` gives a Klipper shutdown. Sim tests are `#[ignore]`.

## Design reference

Tasks restate what they need from here. Task 1 turns this section into
the focused spec, and after that the spec wins.

### D1. Vocabulary and ownership

- A **Queue Entry** is one requested physical run of one Slice Revision.
  It has a queue position, a Dispatch Policy, a dispatch preference, and
  a material estimate. It becomes a Job only by assignment.
- A **Job** is one Queue Entry assigned to one Printer with one reserved
  Spool. Rust owns every Job state, and host strings never set one
  directly: an observation goes through the tracker's rules (D7).
- **Lineage:** one **Add to Queue** with quantity N creates N entries
  that share a `lineage_id` (`qln-*`), with `copy_index` 1..N. A retry,
  or the replacement a release creates, joins the same lineage, with
  `origin_entry_id` and `origin_kind` set to `retry` or `release`. Each
  entry stays independently assignable, retryable, cancellable, and
  reconcilable. Lineage is grouping only and never merges state.
- A **Reconciliation Requirement** is a durable row (`rrq-*`) with a
  stable identity. It names something the operator must settle: a Job's
  material after it failed or was cancelled (`materialReconciliation`),
  or a Job whose outcome is unknown (`jobOutcomeUnknown`). P8 projects
  these into Attention; P7 emits no notification.

### D2. Queue Entry state machine

```text
queued ─ assign (operator or evaluator) ─> assigned
queued ─ remove ─────────────────────────> closed{removed}
assigned ─ Job terminal ─────────────────> closed{completed | failed | cancelled}
assigned ─ release (Job cancelled before start) ─> closed{released}  + new linked entry (queued, same position)
closed{failed | cancelled | released | completed} ─ retry ─> (new linked entry queued at the end)
```

- Eligibility (**Blocked**, **Ready**, or **Awaiting operator**) is
  derived. The evaluator computes it and it is never stored as state.
- `position` is dense (1..n) over non-closed entries, and closed entries
  have `position = NULL`. Reordering renumbers inside the transaction.
  An assigned entry keeps its position, because assignment is sticky:
  reordering never moves an assigned Job to another Printer.
- New entries go to the end.

### D3. Job state machine

```text
assigned ─ stage handoff (write-ahead, same tx) ─> staging
staging ─ upload succeeded ─────────────────────> awaitingStart
staging ─ upload failed / abandoned ────────────> assigned  (lastDispatchFailure set)
awaitingStart ─ start handoff (same tx) ─────────> starting
starting ─ start succeeded ─────────────────────> printing
starting ─ start failed (definitive) ───────────> awaitingStart (lastDispatchFailure)
starting ─ start abandoned ─────────────────────> outcomeUnknown   (+ rrq jobOutcomeUnknown)
printing ⇄ paused (pause/resume succeeded, or observed on our file)
printing|paused ─ tracker proves completed ─────> completed        (estimate consumed, same tx)
printing|paused ─ tracker proves cancelled ─────> cancelled        (+ rrq materialReconciliation)
printing|paused ─ tracker proves failed ────────> failed           (+ rrq materialReconciliation)
printing|paused ─ outcome unprovable ───────────> outcomeUnknown   (+ rrq jobOutcomeUnknown)
outcomeUnknown ─ operator declares ─────────────> completed | failed | cancelled
assigned|awaitingStart ─ release ───────────────> cancelled{releasedBeforeStart} (reservation released, no settlement)
assigned|awaitingStart ─ cancel ────────────────> cancelled{cancelledBeforeStart} (reservation released, no settlement)
```

- **Terminal states** are `completed`, `failed`, and `cancelled`.
- There can't be two active Jobs on one Printer: a partial unique index
  on `jobs(printer_id)` covers the non-terminal states.
- **Awaiting material** is not a separate state. It is Rust-computed
  `startBlockers` on an `awaitingStart` Job (the Spool isn't loaded in a
  Material Slot on that Printer). The UI labels it "Awaiting material".
- **Settlement** is orthogonal to the Job state:

  | Settlement | When |
  |---|---|
  | `notRequired` | The Job never reached `starting` |
  | `settled` | `completed` (estimate consumed) |
  | `pending` → `deferred` → `settled` | failed, or cancelled after start |

  While settlement is pending or deferred, the reservation is
  `unresolved`: the amount stays unavailable, and the Spool shows the
  `reconciliation` facet.

### D4. Transaction boundaries (the crash matrix works from these)

| Boundary | One transaction contains |
|---|---|
| Add to Queue | ledger claim, N entries, lineage |
| Reorder / update / remove | ledger claim, entry rows, renumbering |
| Assign | ledger claim; checks the entry is `queued`, the Printer has no active Job, and eligibility is re-evaluated **inside the tx**; `reserve` on the Spool; Job insert (`assigned`); entry → `assigned`; `job_events` row |
| Stage / start / pause / resume / cancel handoff | P6 write-ahead row (with `job_id`) **and** the Job transition and event, committed together (Task 7). After commit, the executor runs. |
| Host outcome → Job | `jobs::apply_host_outcome(tx, &HostOperation)` is idempotent. It runs from the broadcast and again at startup, so a crash between the P6 commit and the Job transition converges. |
| Tracker terminal | Job → terminal; entry → closed; completed: `consume`; failed/cancelled: `mark_unresolved` plus a requirement row |
| Settle / defer / correct / declare | ledger claim, reservation settle, ledger row, Job settlement, requirement row |
| Release | ledger claim, Job → cancelled, `release` reservation, entry → closed{released}, a new entry at the old position |

Events are published after commit only, and never on a replay.

### D5. Eligibility (pure, deterministic)

Gates are evaluated in this order. Each failure is a
`BlockerCode` + message + `recovery` (a `RecoveryCode`) + optional
`printerIds`.

1. **Printer schedulable:** not archived; not Setup incomplete; no
   Connection error; no active Job; no unresolved Host Operation that a
   Job doesn't own; not printing a file farm3d didn't start (the "manual
   host job" case, `PRINTER_BUSY_EXTERNAL`).
2. **Profile compatible:** compares the revision facts with the
   Printer's resolved profile.
   - Bed shape and printable height are compared within 0.01 mm.
   - The Printer must have exactly one nozzle, equal to the
     `nozzleDiameterMm` fact within 0.01 mm.
   - `nozzleType` and `gcodeFlavor` must be equal.
   - An absent fact is skipped only under the Manual policy, with the
     manual choice recorded. Any other policy is blocked with
     `NEEDS_MANUAL_PRINTER`.
3. **Capability:** `upload`, `start`, `hostState`, and
   `artifactIdentity` must be `supported`, via `services.capabilities`.
   Automatic assignment additionally needs `tier: sim` evidence for each
   of them (decision 2).
4. **Material:** the Spool is `active` and its `MaterialFamily` equals
   the fact. `OTHER` also needs a case-insensitive `materialOther` match.
   The diameter enum is within 0.01 mm of `filamentDiameterMm`, and
   `availableMg ≥ estimateMg`.
   - Automatic assignment needs the Spool **loaded** on the candidate
     Printer.
   - Manual and Recommended assignment may pick a stored Spool, and the
     Job then shows Awaiting material.
   - An absent material fact means Manual only, with an explicit Spool
     choice.
5. **Ranking:**
   - **`loadedFirst`:** Printers with a matching loaded Spool come
     first. Then comes the most available material on the chosen Spool.
   - **`leastRecentlyUsed`:** the oldest last-Job-terminal time comes
     first, and never-used Printers go first.
   - Ties go to lowercase name, then id.
   - For each Printer, the Spool is chosen by: loaded first, then
     smallest sufficient `availableMg`, then Spool number.

Tie-break fixtures, the exact rows in the spec, become table tests in
Task 5.

### D6. Automatic evaluator

- It is one tokio task with a coalescing trigger channel, so evaluation
  is serialized.
- It first runs after `jobs::recover_after_restart` and the first
  host-ops reconciliation pass. It then runs on each of these triggers:
  - an entry was created, updated, moved, or removed;
  - a Job reached a terminal state or was released;
  - a Printer status changed (the `ConnectionManager` status broadcast);
  - capabilities changed;
  - `InventoryChange`;
  - a Printer was edited, archived, or unarchived.
- Each run evaluates every non-closed entry top to bottom:
  - It publishes `queue.eligibility.changed` with a summary per entry
    whenever a summary changes.
  - For an `automatic` entry with a qualifying candidate, it calls the
    **same** `jobs::assign` as the command, using operation id
    `auto-<uuid>` and `assigned_by = automatic`.
  - A Printer claimed earlier in the run is not offered to later
    entries.
- `QueueSnapshot.nextAutomaticAction` states what the evaluator will do
  next, or why it is waiting.
- There is no automatic retry. A failure never re-queues by itself.

### D7. Dispatch driver and outcome tracker

- **Staging:** after assignment, the driver stages at once, for every
  policy.
- **Start:** the driver auto-starts only under decision 1. Otherwise the
  Job waits in `awaitingStart` for `start_job` with the bed-clear
  acknowledgement. `start_job` applies P6's `start_rule::check` **plus**
  these Job checks:
  - the Spool is loaded on this Printer;
  - the staged upload is this Job's;
  - no other Job is active on this Printer.
- **Tracker:**
  - **Inputs:** the status broadcast, and while the Job is `printing` or
    `paused`, a `job_history` poll every 10 s (injectable timings).
  - **Pinning the history job:** after a start succeeds, the tracker
    looks for the first history job with `filename == host_path` and
    `job_id > history_mark`, and records `host_job_id`.
  - **Terminal outcome:** read from that history job's `status`:

    | History `status` | Job outcome |
    |---|---|
    | `completed` | completed |
    | `cancelled` | cancelled |
    | `error`, `klippy_shutdown`, `klippy_disconnect`, `server_exit` | failed |
    | `in_progress` | not terminal |

  - **Hints only:** a status stream that says Finished, Cancelled, or
    Failed on our file triggers an immediate history check. It is never
    proof by itself.
  - **Unprovable outcome:** a pinned job missing from history, or a
    Printer that reports a different file while we have no pinned job,
    means that after 3 inconclusive checks the Job becomes
    `outcomeUnknown`.
- **Progress:** `max_progress_pct` is persisted when it crosses a whole
  percent.
- **Recovery (startup, before commands are served, after
  `host_ops::recover_after_restart`):**
  - For every `staging` or `starting` Job, re-apply its linked Host
    Operation's state with `apply_host_outcome`.
  - `printing` and `paused` Jobs are re-checked by the tracker once the
    runtime starts.
  - The evaluator waits for that pass.

### D8. Lifecycle guards

| Mutation | Blocked when | Code |
|---|---|---|
| Printer archive | an active Job | `JOB_ACTIVE` |
| Printer delete | any Job references the Printer (decision 7), or a queued entry is pinned to it | `JOB_HISTORY_EXISTS`, `QUEUE_ENTRY_PINNED` |
| Printer import (replace all) | any active Job or unsettled settlement | `HOST_OPERATION_PENDING`-style whole-import rejection: `JOBS_ACTIVE` |
| Slice Revision delete | any Queue Entry or Job references it | `QUEUE_REFERENCES_REVISION` |
| Model delete | already blocked transitively (`SliceRevisionsExist`) | — |
| Spool archive / mark empty | open reservations (P3, unchanged) | `SPOOL_RESERVED` |
| Connection endpoint change / clear | an active Job past `assigned` | `CONNECTION_IN_USE` |

### D9. Commands and events (names fixed here, payloads in the spec)

Commands (18):

- `list_queue() -> QueueSnapshot { streamId, snapshotSequence, entries, jobs, requirements, eligibility, nextAutomaticAction }`
- `add_to_queue(operationId, sliceRevisionId, quantity 1..=50, policy, preference, materialEstimate?, manualPrinterId?)`
- `update_queue_entry(operationId, entryId, expectedRevision, policy?, preference?)`
- `move_queue_entry(operationId, entryId, expectedRevision, toPosition)`
- `remove_queue_entry(operationId, entryId, expectedRevision)`
- `explain_queue_entry(entryId) -> QueueEntryEligibility`
- `assign_queue_entry(operationId, entryId, printerId, spoolId, acknowledgeManualFacts?)`
- `stage_job(operationId, jobId)`
- `start_job(operationId, jobId, priorState, acknowledgement: "bedClear")`
- `pause_job`, `resume_job`, `cancel_job(operationId, jobId)`
- `release_job(operationId, jobId)`
- `retry_job(operationId, jobId)`
- `declare_job_outcome(operationId, jobId, outcome, acknowledgement: "hostStateUnknown")`
- `settle_job_material(operationId, jobId, choice: estimated | measured{entry: AmountEntry} | defer)`
- `correct_job_material(operationId, jobId, entry: AmountEntry)`
- `get_job_history(jobId) -> JobHistory { job, events, reservations, hostOperations }`

Events go on stream `queue`, channel `farm3d-event-v1`:
`queue.entry.changed`, `queue.job.changed`, `queue.requirement.changed`,
and `queue.eligibility.changed`. Spool changes reuse
`spools::events::publish_ids`.

## Global Constraints

These rules bind every task. The controller gives this section to every
implementer.

1. **No writes to real printers.** Every write test (upload, start,
   control, G-code, faults) runs against `FakeMoonraker` or the loopback
   simulators. Real-hardware tests may only probe, subscribe, and query,
   and take their host from an environment variable with no default.
   Nothing enables writes to a non-loopback host.
2. **No owner network details in the repository.** Never commit an IP
   address, hostname, serial, MAC address, token, or local path. Use
   environment variables or RFC 5737 addresses (`192.0.2.x`). Run
   `just check-hosts` before every commit.
3. **Credentials** never enter frontend state, events, errors, logs,
   `jobs`, `job_events`, requirements, fixtures, or snapshots. Every new
   persisted or emitted payload gets a seeded-secret test.
4. **Rust is persisted truth.** TypeScript never decides eligibility, Job
   state, settlement, or ordering. It presents what Rust returns, and
   settles optimistic UI from command results or events.
5. **Frontend conventions** (AGENTS.md, DESIGN.md):
   - `--f3d-*` tokens only, with no hard-coded colors, font sizes, or
     radii.
   - CSS Modules next to components.
   - Kobalte primitives for anything interactive. Read
     `node_modules/@kobalte/core/src/<name>/` for props; don't guess.
   - The dense editor aesthetic: no elevation, no ripple.
   - Kobalte Select and DropdownMenu tests use `fireEvent.pointerDown`
     and `pointerUp`.
   - New design-system components go in `components/index.ts` and
     `Showcase.tsx`.
6. **Test-first.** Write the failing test, watch it fail, make it pass,
   refactor, and commit, one conventional commit per task step group.
7. **Gates** before a task is done. Run the ones the task touches; the
   controller runs all of them at review.

   ```sh
   source "$HOME/.cargo/env"   # cargo is not on PATH in non-interactive shells
   just build
   just test
   just test-rust
   just check-hosts
   ```

   If a Rust type exported to TypeScript changed, also run
   `just gen-contracts && git diff --exit-code src/generated`. Never
   hand-edit `src/generated/**`.
8. **Contract registration.** A new command goes in:
   - `lib.rs` `COMMAND_NAMES` and `generate_handler!`;
   - `contracts/inventory.rs` (the array length, the decl string, and the
     visitor);
   - `tests/export_contracts.rs`;
   - `tests/f1_contract_path.rs` (the count, names, handlers, and one
     case per command);
   - `src/ipc/client.ts` `CommandMap`.

   Count assertions add P7's commands to `main`'s expression
   (`58 + 21 + 2 + 8 + <P7>`), never a hard-coded total.
9. **Idempotency.** Every mutating command takes a client `operationId`,
   claimed through `spools::operations::claim` in the same transaction.
   A replay returns the same result and publishes nothing. A reused id
   with a different request is `OPERATION_ID_REUSED`.
10. **Simulators never run in CI.** Container-backed tests are
    `#[ignore]`, use `require_sim!` and `sim::exclusive()`, and call
    `reset()` first. Anything CI must check needs an in-process fake.
11. **Scope.** No Attention center, Incidents, cameras, or desktop
    notifications (P8). No history search, export, or pruning (P9). No
    TLS. No OctoPrint or ElegooLink command capability.

## Test tiers

| Tier | What | How it runs | Writes |
|---|---|---|---|
| Pure unit | State machines, eligibility, ranking, tie-break fixtures, estimate math, settlement rules | `just test-rust`, CI | none |
| Repository | Migrations, transactions, crash injection, exactly-once, guards | `just test-rust` over `test_storage()` | SQLite only |
| In-process fake | Driver, tracker, evaluator, restart matrix, and the tracer, all through `FakeMoonraker` and a rebuilt `RuntimeServices` | `just test-rust`, CI | in-process |
| Simulator | The tracer and control paths against real Klipper/Moonraker | `just sim-up && FARM3D_SIM_REQUIRED=1 just test-sim` (local only) | loopback only |
| Frontend | Stores, presentation, screens, reorder keyboard/pointer, dialogs | `just test`, CI | none |

## File map

**Backend (new):**

- `src-tauri/migrations/0008_p7_queue_jobs.sql`
- `src-tauri/src/queue/mod.rs`: wire types (`QueueEntry`,
  `DispatchPolicy`, `DispatchPreference`, `MaterialEstimate`,
  `QueueSnapshot`, `QueueEntryEligibility`, `Blocker`, `Candidate`,
  `NextAutomaticAction`)
- `src-tauri/src/queue/state.rs`: the pure entry transition table
- `src-tauri/src/queue/repository.rs`: create with lineage, order,
  update, remove, load, and list
- `src-tauri/src/queue/eligibility.rs`: pure gates, ranking, and
  compatibility
- `src-tauri/src/queue/evaluator.rs`: the serialized automatic evaluator
- `src-tauri/src/queue/events.rs`: `QueueStream`, the event types, and
  `publish`
- `src-tauri/src/queue/commands.rs`: the queue commands
- `src-tauri/src/jobs/mod.rs`: wire types (`Job`, `JobState`,
  `Settlement`, `JobEvent`, `ReconciliationRequirement`, `JobHistory`)
- `src-tauri/src/jobs/state.rs`: the pure Job transition table
- `src-tauri/src/jobs/repository.rs`: insert, transition, events,
  requirements, and snapshot
- `src-tauri/src/jobs/assign.rs`: assign, release, retry, and
  cancel-before-start transactions
- `src-tauri/src/jobs/dispatch.rs`: the stage, start, and control
  handoff, and `apply_host_outcome`
- `src-tauri/src/jobs/tracker.rs`: outcome tracking and history pinning
- `src-tauri/src/jobs/settlement.rs`: complete, settle, defer, correct,
  and declare
- `src-tauri/src/jobs/recovery.rs`: `recover_after_restart`
- `src-tauri/src/jobs/guards.rs`: lifecycle, revision, import, and
  Connection blockers
- `src-tauri/src/jobs/services.rs`: `JobServices<R>` (driver task and
  timings)
- `src-tauri/src/jobs/commands.rs`: the Job commands

**Backend (modified):**

- `persistence/migrations.rs` (version 8)
- `spools/operations.rs` (new kinds)
- `spools/reservations.rs` (holder queries, `consume_measured`)
- `spools/repository.rs` (the `reconciliation` facet)
- `host_ops/{repository,commands,services,mod}.rs` (the `job_id` link,
  the Rust API, and the broadcast)
- `connections/supervisor.rs` (the status broadcast)
- `printers/lifecycle.rs` and `slicing/blockers.rs` (register P7
  sources)
- `printers/repository.rs` and `connections/commands.rs` (the import and
  Connection guards)
- `slicing/facts.rs`, or a new `slicing/compat.rs` (a public
  compatibility fn)
- `contracts/{command,inventory}.rs` (error codes and contracts)
- `lib.rs` (modules, startup order, and commands)

**Tests:**

- `tests/p7_migration.rs`
- `tests/p7_queue.rs`
- `tests/p7_jobs.rs`
- `tests/p7_settlement.rs`
- `tests/p7_evaluator.rs`
- `tests/p7_restart_matrix.rs`
- `tests/p7_guards.rs`
- `tests/p7_tracer.rs`
- `tests/common/fake_moonraker.rs` (helpers to finish or fail a print)
- `tests/sim_moonraker.rs` (the P7 section)

**Frontend:**

- `src/queue/`: `queue-store.ts`, `queue-store-mock.ts`,
  `web-fixtures.ts`, `types.ts`, `views.ts`, `presentation.ts`,
  `settlement.ts`, `test-records.ts`, plus `*.test.ts`
- `src/design-system/components/ReorderHandle.tsx`, with its
  `.module.css` and tests
- `src/screens/`:
  - `QueueScreen.tsx`, `QueueEntryDetail.tsx`, `AddToQueueDialog.tsx`,
    `AssignJobDialog.tsx`;
  - `JobPanel.tsx`, `StartJobDialog.tsx`, `SettleMaterialDialog.tsx`,
    `DeclareOutcomeDialog.tsx`, `QueuePreview.tsx`;
  - each with a `.module.css` and a `.test.tsx`
- Modified: `ActivityBar.tsx`, `AppShell.tsx`, `App.tsx`,
  `PrinterDashboard.tsx`, `PrinterJobPanel.tsx`, `SliceRevisionReview.tsx`,
  `SpoolInventory.tsx`, `SpoolDetailDock.tsx`, `src/spools/facets.ts`,
  `src/ipc/client.ts`, and `Showcase.tsx`

**Docs:**

- the spec `docs/superpowers/specs/2026-09-26-p7-queue-entries-jobs-dispatch-design.md`
- `docs/adr/0013-queue-and-job-state-machines.md`
- `CONTEXT.md`
- `docs/verification/2026-09-2x-p7-queue-jobs-dispatch.md`
- screenshots `docs/screenshots/p7-*`
- the v1 approach's known-unknowns row "Queue/Job state machine"

## Tasks

### Task 1: Focused spec, ADR-0013, and CONTEXT.md

**Owner:** Wiring, with Backend. **Depends on:** nothing (owner decisions
are fixed).

**Files:**

- Create `docs/superpowers/specs/2026-09-26-p7-queue-entries-jobs-dispatch-design.md`
  in the P6 spec's shape: status, goal, scope and non-goals, vocabulary,
  decisions D1–D9, backend model (schema with every column and CHECK),
  wire types, commands and events with payloads, error codes, frontend,
  accessibility, acceptance criteria, and delivery.
- Create `docs/adr/0013-queue-and-job-state-machines.md`. It records two
  choices: the Job owns dispatch while Host Operations remain the only
  write path (a Job-linked write-ahead in one transaction); and
  eligibility is derived, never stored.
- Modify `CONTEXT.md`:
  - update **Job** (remove "Not yet implemented"), **Queue Entry**, and
    **Dispatch Policy**;
  - add **Lineage**, **Settlement**, **Reconciliation Requirement**,
    **Awaiting material**, **Outcome unknown**, and **Dispatch
    preference**.

**Interfaces:**

- Consumes: this plan's §Design reference and §Owner decisions.
- Produces: the fixed names every later task uses. These are the
  commands, events, error codes, `JobState`, `QueueEntryState`,
  `Settlement`, `BlockerCode`, and `RequirementKind` variants, plus the
  restart matrix table.

- [ ] **Step 1:** Write the spec's **legal transition tables** for Queue
  Entry and Job as markdown tables that include every
  `(from, event) → to` pair and every illegal pair's rejection code.
  Task 2 turns them into exhaustive tests.
- [ ] **Step 2:** Write the **transaction boundary table** (D4) and the
  **restart matrix**. It has one row per durable transition, with
  columns *crash point*, *state on disk*, *startup outcome*, and *test
  name in `tests/p7_restart_matrix.rs`*. Include at least these crash
  points:
  - after assign;
  - between the write-ahead and the executor send (P6 covers this);
  - after the upload succeeded but before `apply_host_outcome`;
  - after the start was sent and a reply was lost;
  - after the start succeeded;
  - during printing;
  - after the host completed but before the tracker saw it;
  - after the terminal transition but before settlement;
  - after a defer;
  - during a release.
- [ ] **Step 3:** Write the **tie-break fixtures**. Each is a table
  giving Printers (name, id, loaded Spool, availableMg, last-used), an
  entry (policy, preference), and the expected ranked candidate list.
  Include at least 6: equal names with different ids; case-only name
  differences; `loadedFirst` versus `leastRecentlyUsed`; an
  insufficient-but-loaded Spool; an absent material fact under Manual;
  and two automatic entries competing for one Printer.
- [ ] **Step 4:** Write the **manual host-job handling** section:
  - a Printer printing a file farm3d didn't start is ineligible
    (`PRINTER_BUSY_EXTERNAL`);
  - a Job never adopts a foreign print;
  - a raw P6 upload or start on a Printer with an active Job is refused
    with `JOB_ACTIVE`;
  - how `outcomeUnknown` and `declare_job_outcome` work.
- [ ] **Step 5:** Name every error code with its message and recovery,
  and every command payload. Where the spec changes an interface, update
  the affected tasks in this plan in the same commit.
- [ ] **Step 6:** Write the ADR and the CONTEXT.md entries.
- [ ] **Step 7:** Run `just check-hosts`. Commit:
  `docs(p7): write the P7 queue/jobs spec and ADR-0013`.
- [ ] **Step 8 (controller approves):** Review against issue #17's
  acceptance criteria and the v1 approach's P7 decision gate: legal
  transitions, transaction boundaries, evaluator triggers, tie-break
  fixtures, manual host-job handling, operation IDs, and restart
  outcomes. Record the approval in the spec's and the ADR's `Status`,
  then commit `docs(p7): approve the P7 spec and ADR-0013`.

**Acceptance criteria:** every D-section has a final decision; every
command, event, and error code used in Tasks 2–17 is named with its
payload; the restart matrix names a test per row.

### Task 2: Migration 0008 and the pure state machines

**Owner:** Backend. **Depends on:** Task 1.

**Files:**

- Create `src-tauri/migrations/0008_p7_queue_jobs.sql`
- Modify `src-tauri/src/persistence/migrations.rs` (register 0008,
  `CURRENT_SCHEMA_VERSION = 8`, `[Migration; 8]`)
- Modify `src-tauri/src/spools/operations.rs` (new `OperationKind`
  variants)
- Create `src-tauri/src/queue/{mod.rs,state.rs}` and
  `src-tauri/src/jobs/{mod.rs,state.rs}`
- Modify `src-tauri/src/lib.rs` (`mod queue; mod jobs;`)
- Test: `src-tauri/tests/p7_migration.rs` (copy the shape of
  `tests/p6_migration.rs`)

**Interfaces:**

- Produces (the Rust shapes; the spec may rename fields):

```rust
// queue/mod.rs
pub enum QueueEntryState { Queued, Assigned, Closed }
pub enum CloseReason { Completed, Failed, Cancelled, Released, Removed }
pub enum DispatchPolicy { Manual, Recommended, Automatic }
pub enum DispatchPreference { LoadedFirst, LeastRecentlyUsed }
pub enum EstimateSource { SliceEstimate, FileClaimConfirmed, OperatorEntered }
pub struct MaterialEstimate { pub amount_mg: i64, pub source: EstimateSource }
pub enum OriginKind { Retry, Release }
// queue/state.rs
pub enum EntryEvent { Assign, Remove, JobTerminal(CloseReason), Release }
pub fn transition(from: QueueEntryState, event: &EntryEvent) -> Result<QueueEntryState, IllegalTransition>;
// jobs/mod.rs
pub enum JobState { Assigned, Staging, AwaitingStart, Starting, Printing, Paused,
                    Completed, Failed, Cancelled, OutcomeUnknown }
pub enum Settlement { NotRequired, Pending, Deferred, Settled }
pub enum JobEventKind { /* one per D3 arrow, e.g. */ Assigned, StageHandedOff, Staged,
                        StageFailed, StartHandedOff, Started, StartFailed, StartAbandoned,
                        Paused, Resumed, Completed, Failed, Cancelled, OutcomeUnknown,
                        OutcomeDeclared, Released, CancelledBeforeStart,
                        MaterialSettled, MaterialDeferred, MaterialCorrected }
// jobs/state.rs
pub fn transition(from: JobState, event: JobEventKind) -> Result<JobState, IllegalTransition>;
pub fn settlement_after(state: JobState, reached_starting: bool) -> Settlement;
```

- `OperationKind` gains `AddToQueue`, `UpdateQueueEntry`,
  `MoveQueueEntry`, `RemoveQueueEntry`, `AssignQueueEntry`, `StageJob`,
  `StartJob`, `PauseJob`, `ResumeJob`, `CancelJob`, `ReleaseJob`,
  `RetryJob`, `DeclareJobOutcome`, `SettleJobMaterial`, and
  `CorrectJobMaterial`, all with camelCase serde.

- [ ] **Step 1: Write the failing state-machine tests.** In
  `jobs/state.rs`, iterate every `(JobState, JobEventKind)` pair against
  an expected table copied from the spec:

```rust
#[test]
fn every_job_transition_pair_matches_the_spec_table() {
    for from in JobState::ALL {
        for event in JobEventKind::ALL {
            let expected = SPEC_TABLE.iter()
                .find(|(f, e, _)| *f == from && *e == event)
                .map(|(_, _, to)| *to);
            assert_eq!(transition(from, event).ok(), expected, "{from:?} + {event:?}");
        }
    }
}
```

  Write the same test for `queue/state.rs`. Add a test that a terminal
  `JobState` accepts only `MaterialSettled`, `MaterialDeferred`, and
  `MaterialCorrected`, and that each leaves the state unchanged. Add
  another that `OutcomeDeclared` is legal only from `OutcomeUnknown`.
- [ ] **Step 2:** Run `just test-rust`. Expect a FAIL because the modules
  don't exist yet.
- [ ] **Step 3:** Implement the enums (ts-rs exported, camelCase, and an
  `ALL` const) and both `transition` fns as `match` tables.
- [ ] **Step 4: Write the migration test first.** In
  `tests/p7_migration.rs`:
  - migrate a P6-shaped DB (`apply_through(&mut conn, 7)`) seeded with
    one `operations` row and one `host_operations` row, then apply 8;
  - assert the rows survive, the new kinds are accepted, the partial
    unique index rejects a second active Job per Printer, and
    `CURRENT_SCHEMA_VERSION == 8`;
  - use `apply_through_failing_before_commit` to prove 0008 is atomic;
  - add a schema test that no new column name contains `credential`,
    `secret`, or `key`.
- [ ] **Step 5:** Write `0008_p7_queue_jobs.sql`:

```sql
CREATE TABLE queue_entries (
  id TEXT PRIMARY KEY CHECK (id GLOB 'qen-*'),
  revision INTEGER NOT NULL DEFAULT 1,
  slice_revision_id TEXT NOT NULL REFERENCES slice_revisions(id) ON DELETE RESTRICT,
  lineage_id TEXT NOT NULL CHECK (lineage_id GLOB 'qln-*'),
  copy_index INTEGER NOT NULL CHECK (copy_index >= 1),
  origin_entry_id TEXT REFERENCES queue_entries(id),
  origin_kind TEXT CHECK (origin_kind IN ('retry','release')),
  state TEXT NOT NULL CHECK (state IN ('queued','assigned','closed')),
  close_reason TEXT CHECK (close_reason IN ('completed','failed','cancelled','released','removed')),
  position INTEGER CHECK (position >= 1),
  policy TEXT NOT NULL CHECK (policy IN ('manual','recommended','automatic')),
  preference TEXT NOT NULL CHECK (preference IN ('loadedFirst','leastRecentlyUsed')),
  estimate_mg INTEGER NOT NULL CHECK (estimate_mg > 0),
  estimate_source TEXT NOT NULL CHECK (estimate_source IN ('sliceEstimate','fileClaimConfirmed','operatorEntered')),
  manual_printer_id TEXT REFERENCES printers(id) ON DELETE RESTRICT,
  job_id TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL, closed_at TEXT,
  CHECK ((state = 'closed') = (close_reason IS NOT NULL)),
  CHECK ((state = 'closed') = (position IS NULL)),
  CHECK ((origin_entry_id IS NULL) = (origin_kind IS NULL))
) STRICT;
CREATE INDEX queue_entries_open_position ON queue_entries(position) WHERE state <> 'closed';
CREATE INDEX queue_entries_lineage ON queue_entries(lineage_id);

CREATE TABLE jobs (
  id TEXT PRIMARY KEY CHECK (id GLOB 'job-*'),
  revision INTEGER NOT NULL DEFAULT 1,
  queue_entry_id TEXT NOT NULL UNIQUE REFERENCES queue_entries(id),
  slice_revision_id TEXT NOT NULL REFERENCES slice_revisions(id) ON DELETE RESTRICT,
  printer_id TEXT NOT NULL REFERENCES printers(id) ON DELETE RESTRICT,
  printer_snapshot_json TEXT NOT NULL CHECK (json_valid(printer_snapshot_json)),
  spool_id TEXT NOT NULL REFERENCES spools(id),
  reservation_id TEXT NOT NULL REFERENCES spool_reservations(id),
  estimate_mg INTEGER NOT NULL CHECK (estimate_mg > 0),
  state TEXT NOT NULL CHECK (state IN ('assigned','staging','awaitingStart','starting','printing',
                                       'paused','completed','failed','cancelled','outcomeUnknown')),
  cancel_reason TEXT CHECK (cancel_reason IN ('releasedBeforeStart','cancelledBeforeStart','hostCancelled','operatorDeclared')),
  settlement TEXT NOT NULL CHECK (settlement IN ('notRequired','pending','deferred','settled')),
  settlement_method TEXT CHECK (settlement_method IN ('estimated','measured')),
  assigned_by TEXT NOT NULL CHECK (assigned_by IN ('operator','automatic')),
  start_confirmation TEXT CHECK (start_confirmation IN ('bedClear','unattended')),
  upload_host_operation_id TEXT REFERENCES host_operations(id),
  active_host_operation_id TEXT REFERENCES host_operations(id),
  host_path TEXT,
  history_mark INTEGER,
  host_job_id TEXT,
  max_progress_pct INTEGER NOT NULL DEFAULT 0 CHECK (max_progress_pct BETWEEN 0 AND 100),
  inconclusive_checks INTEGER NOT NULL DEFAULT 0,
  last_failure_json TEXT CHECK (last_failure_json IS NULL OR json_valid(last_failure_json)),
  correction_event_id TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL, started_at TEXT, ended_at TEXT
) STRICT;
CREATE UNIQUE INDEX jobs_one_active_per_printer ON jobs(printer_id)
  WHERE state NOT IN ('completed','failed','cancelled');

CREATE TABLE job_events (
  id TEXT PRIMARY KEY, job_id TEXT NOT NULL REFERENCES jobs(id),
  sequence INTEGER NOT NULL, kind TEXT NOT NULL, from_state TEXT, to_state TEXT,
  operation_id TEXT, host_operation_id TEXT,
  detail_json TEXT CHECK (detail_json IS NULL OR json_valid(detail_json)),
  at TEXT NOT NULL, UNIQUE (job_id, sequence)
) STRICT;
CREATE TRIGGER job_events_append_only_u BEFORE UPDATE ON job_events BEGIN SELECT RAISE(ABORT, 'job_events is append-only'); END;
CREATE TRIGGER job_events_append_only_d BEFORE DELETE ON job_events BEGIN SELECT RAISE(ABORT, 'job_events is append-only'); END;

CREATE TABLE reconciliation_requirements (
  id TEXT PRIMARY KEY CHECK (id GLOB 'rrq-*'),
  job_id TEXT NOT NULL REFERENCES jobs(id),
  kind TEXT NOT NULL CHECK (kind IN ('materialReconciliation','jobOutcomeUnknown')),
  status TEXT NOT NULL CHECK (status IN ('pending','deferred','resolved')),
  spool_id TEXT REFERENCES spools(id),
  reservation_id TEXT REFERENCES spool_reservations(id),
  opened_at TEXT NOT NULL, deferred_at TEXT, resolved_at TEXT,
  resolution_json TEXT CHECK (resolution_json IS NULL OR json_valid(resolution_json)),
  UNIQUE (job_id, kind)
) STRICT;

ALTER TABLE host_operations ADD COLUMN job_id TEXT REFERENCES jobs(id);
CREATE INDEX spool_reservations_holder ON spool_reservations(holder_kind, holder_id);
-- then rebuild `operations` exactly as 0007:92-104 does (create operations_p7 with the
-- full kind list including the 15 new kinds, INSERT … SELECT, DROP, RENAME).
```

  Before using `ALTER TABLE … ADD COLUMN … REFERENCES`, check whether
  it is allowed with the P6 terminal-row trigger on `host_operations`.
  If it isn't, rebuild that table as 0007 does for `operations`.
- [ ] **Step 6:** Register the migration and the `OperationKind`
  variants, then run `just test-rust`. Expect a PASS.
- [ ] **Step 7:** Run `just gen-contracts && git diff --stat src/generated`,
  then `just check-hosts`. Commit:
  `feat(queue): add the P7 schema and Queue Entry/Job state machines`.

**Acceptance criteria:** every legal and illegal pair in the spec's
tables is tested. 0008 is atomic and preserves P6 data. At most one
active Job per Printer is enforced by the index.

### Task 3: Queue and Job repositories (order, lineage, timeline, requirements)

**Owner:** Backend. **Depends on:** Task 2.

**Files:**

- Create `src-tauri/src/queue/repository.rs` and
  `src-tauri/src/jobs/repository.rs`
- Test: in-module tests over `crate::test_storage()`, plus
  `src-tauri/tests/p7_queue.rs`

**Interfaces:**

- Consumes: Task 2's enums and schema.
- Produces:

```rust
// queue/repository.rs
pub struct NewEntries { pub slice_revision_id: String, pub quantity: u8, pub policy: DispatchPolicy,
                        pub preference: DispatchPreference, pub estimate: MaterialEstimate,
                        pub manual_printer_id: Option<String> }
pub fn create_entries(tx: &Transaction<'_>, new: &NewEntries, now: &str) -> Result<Vec<QueueEntry>, RepositoryError>;
pub fn create_linked(tx: &Transaction<'_>, origin: &QueueEntry, kind: OriginKind, position: Option<i64>, now: &str) -> Result<QueueEntry, RepositoryError>;
pub fn move_entry(tx: &Transaction<'_>, id: &str, expected_revision: i64, to_position: i64, now: &str) -> Result<Vec<QueueEntry>, RepositoryError>;
pub fn update_entry(tx, id, expected_revision, policy: Option<DispatchPolicy>, preference: Option<DispatchPreference>, now) -> Result<QueueEntry, RepositoryError>;
pub fn apply(tx: &Transaction<'_>, id: &str, event: &EntryEvent, job_id: Option<&str>, now: &str) -> Result<QueueEntry, RepositoryError>; // uses state::transition, renumbers on close
pub fn load(conn: &Connection, id: &str) -> Result<Option<QueueEntry>, StorageError>;
pub fn list_open(conn: &Connection) -> Result<Vec<QueueEntry>, StorageError>;  // ordered by position
pub fn list_history(conn: &Connection, limit: u32) -> Result<Vec<QueueEntry>, StorageError>;
// jobs/repository.rs
pub fn insert_job(tx: &Transaction<'_>, new: &NewJob, now: &str) -> Result<Job, RepositoryError>;
pub fn transition(tx: &Transaction<'_>, job_id: &str, event: JobEventKind, change: JobChange, now: &str) -> Result<Job, RepositoryError>;
pub fn open_requirement(tx, job_id, kind: RequirementKind, spool_id: Option<&str>, reservation_id: Option<&str>, now) -> Result<ReconciliationRequirement, RepositoryError>;
pub fn set_requirement_status(tx, id, status: RequirementStatus, resolution: Option<&serde_json::Value>, now) -> Result<ReconciliationRequirement, RepositoryError>;
pub fn load_job(conn, id) -> Result<Option<Job>, StorageError>;
pub fn active_job_for_printer(conn, printer_id) -> Result<Option<Job>, StorageError>;
pub fn job_for_host_operation(conn, host_operation_id) -> Result<Option<Job>, StorageError>;
pub fn list_active(conn) -> Result<Vec<Job>, StorageError>;
pub fn history(conn, job_id) -> Result<JobHistory, StorageError>;
pub fn open_requirements(conn) -> Result<Vec<ReconciliationRequirement>, StorageError>;
```

  `JobChange` carries the optional column updates for a transition
  (`active_host_operation_id`, `host_path`, `history_mark`,
  `host_job_id`, `last_failure`, `cancel_reason`, `settlement`, and so
  on), so every transition writes its row **and** one `job_events` row
  with the next `sequence`.

- [ ] **Step 1: Write the failing tests** (each one is a test fn):
  - `create_three_copies_share_lineage_and_take_contiguous_positions_at_the_end`
  - `each_copy_is_independently_removable_and_renumbers_the_rest`
  - `move_entry_renumbers_densely_and_bumps_revision`
  - `move_entry_rejects_stale_revision_with_revision_conflict`
  - `closed_entries_have_no_position_and_keep_lineage`
  - `create_linked_release_takes_the_released_position_and_shifts_nothing_else`
  - `create_linked_retry_appends_at_the_end`
  - `job_transition_appends_one_event_with_the_next_sequence`
  - `illegal_job_transition_is_rejected_before_sql`
  - `requirement_is_unique_per_job_and_kind`
  - `job_history_returns_events_in_sequence_order`
- [ ] **Step 2:** Run `just test-rust` and confirm the new tests fail.
- [ ] **Step 3:** Implement both repositories. Ids are `qen-`, `qln-`,
  `job-`, `jev-`, and `rrq-` plus a UUID v4.
- [ ] **Step 4:** Run `just test-rust` and confirm they pass.
- [ ] **Step 5:** Commit:
  `feat(queue): persist Queue Entry order, lineage, and the Job timeline`.

### Task 4: P3 reservation extensions and the Spool reconciliation facet

**Owner:** Backend. **Depends on:** Task 2.

**Files:**

- Modify `src-tauri/src/spools/reservations.rs`, `spools/repository.rs`,
  and `spools/mod.rs`
- Modify `src-tauri/src/contracts/command.rs` (map `ReservationError`)
- Modify `src/spools/facets.ts`, `src/screens/SpoolInventory.tsx`,
  `src/screens/SpoolDetailDock.tsx`, `src/App.tsx`, and
  `src/screens/ActivityBar.tsx`

**Interfaces:**

- Produces:

```rust
pub fn for_holder(tx: &Transaction<'_>, holder: &ReservationHolder) -> Result<Vec<Reservation>, StorageError>;
/// Settles an active|unresolved reservation against a measured remaining amount:
/// marks it consumed and appends ONE `Measurement` ledger row (confidence Measured)
/// carrying `reservation_id`. Used = current − remaining (clamped ≥ 0) is reported.
pub fn consume_measured(tx: &Transaction<'_>, reservation_id: &str, entry: &AmountEntry, note: Option<&str>) -> Result<AmountEvent, ReservationError>;
impl From<ReservationError> for CommandError // INSUFFICIENT_MATERIAL, SPOOL_NOT_RESERVABLE, RESERVATION_STATE, NOT_FOUND
```

  `SpoolFacets` gains `reconciliation: bool`, which is true when the
  Spool has any `unresolved` reservation.
- On the TS side, `SpoolFacetKey` gains `"reconciliation"`. The Spools
  rail badge counts `low` **or** `reconciliation` Spools, with the
  aria-label "Spools (N need attention)".

- [ ] **Step 1: Write the failing Rust tests:**
  - `consume_measured_writes_one_measured_row_and_consumes_the_reservation`
  - `consume_measured_from_unresolved_is_allowed`
  - `consume_measured_twice_is_invalid_transition`
  - `for_holder_lists_every_state`
  - `unresolved_reservation_sets_the_reconciliation_facet`
  - `available_mg_excludes_unresolved_amounts`
- [ ] **Step 2: Write the failing TS tests:**
  - in `facets.test.ts`, the new facet filters;
  - in `SpoolInventory.test.tsx`, the "Needs reconciliation" chip, and
    the over-reserved warning when `availableMg < 0` (P3 D8 asked for
    it and it is missing);
  - in `SpoolDetailDock.test.tsx`, reservations rendered in the
    Timeline, each with its holder label ("Job").
- [ ] **Step 3:** Implement. Keep the rule that reservation writes don't
  bump `spools.revision`.
- [ ] **Step 4:** Run `just test-rust`, `just test`, and
  `just gen-contracts` (commit the regenerated files).
- [ ] **Step 5:** Commit:
  `feat(spools): add holder queries, measured settlement, and the reconciliation facet`.

### Task 5: Eligibility, compatibility, and ranking (pure)

**Owner:** Backend. **Depends on:** Tasks 2 and 4.

**Files:**

- Create `src-tauri/src/slicing/compat.rs`. Move and generalize the
  comparison from the private `presets.rs:679 profiles_match`, keep
  `matching_printer_ids` calling it, and use the tolerant comparison
  everywhere.
- Create `src-tauri/src/queue/eligibility.rs`

**Interfaces:**

- Consumes: `SliceFacts`, `PrinterProfile`, `PrinterCapabilities`,
  `SpoolRecord`, `PrinterStatus`, and Task 2's enums.
- Produces:

```rust
// slicing/compat.rs
pub enum CompatMismatch { BedShape, PrintableHeight, NozzleCount { found: usize }, NozzleDiameter { want: f64, have: f64 },
                          NozzleType, GcodeFlavor, FactAbsent(FactKey) }
pub fn profile_compatible(facts: &SliceFacts, printer: &PrinterProfile) -> Vec<CompatMismatch>; // empty = compatible
pub fn material_compatible(facts: &SliceFacts, spool: &SpoolRecord) -> Result<(), MaterialMismatch>;
// queue/eligibility.rs
pub struct PrinterView<'a> { pub printer: &'a StoredPrinter, pub profile: &'a PrinterProfile, pub status: Option<&'a PrinterStatus>,
                             pub capabilities: &'a PrinterCapabilities, pub active_job: bool, pub foreign_host_op: bool,
                             pub last_used_at: Option<&'a str>, pub loaded_spool_ids: &'a [String] }
pub struct EligibilityInput<'a> { pub entry: &'a QueueEntry, pub facts: &'a SliceFacts, pub printers: &'a [PrinterView<'a>],
                                  pub spools: &'a [SpoolRecord], pub claimed_printers: &'a BTreeSet<String> }
pub fn evaluate(input: &EligibilityInput<'_>) -> QueueEntryEligibility; // candidates ranked, blockers, next action
pub fn check_assignment(input: &EligibilityInput<'_>, printer_id: &str, spool_id: &str, mode: AssignMode) -> Result<(), Vec<Blocker>>;
pub enum AssignMode { Operator { acknowledge_manual_facts: bool }, Automatic }
```

  `BlockerCode` covers `PRINTER_ARCHIVED`, `SETUP_INCOMPLETE`,
  `CONNECTION_ERROR`, `JOB_ACTIVE`, `HOST_OPERATION_PENDING`,
  `PRINTER_BUSY_EXTERNAL`, `PROFILE_MISMATCH`, `NEEDS_MANUAL_PRINTER`,
  `CAPABILITY_UNSUPPORTED`, `ADAPTER_NOT_PROVEN`, `NO_COMPATIBLE_SPOOL`,
  `INSUFFICIENT_MATERIAL`, and `SPOOL_NOT_LOADED` (automatic only).

- [ ] **Step 1: Write the failing table tests.** Add one test per gate
  in D5, per failing and passing case. Add the spec's tie-break fixtures
  verbatim as `#[test] fn tie_break_fixture_N()`:

```rust
#[test]
fn tie_break_fixture_1_equal_names_order_by_id() {
    let world = World::new()
        .printer("prn-b", "Voron", loaded("spl-1", 900_000))
        .printer("prn-a", "voron", loaded("spl-2", 900_000));
    let entry = world.entry(DispatchPolicy::Recommended, DispatchPreference::LoadedFirst, 50_000);
    let result = evaluate(&world.input(&entry));
    assert_eq!(result.candidate_ids(), ["prn-a", "prn-b"]);
}
```

  Also write:
  - `readonly_hardware_evidence_never_qualifies_for_automatic`;
  - `octoprint_not_verified_is_capability_unsupported`;
  - `absent_material_fact_is_manual_only`;
  - `claimed_printers_are_excluded_later_in_the_same_run`;
  - `stored_spool_is_allowed_for_manual_and_blocked_for_automatic`;
  - `diameter_175_matches_1_75_fact_within_tolerance`;
  - `nozzle_list_with_two_entries_is_nozzle_count_mismatch`.
- [ ] **Step 2:** Run `just test-rust` and confirm the tests fail.
- [ ] **Step 3:** Implement `compat.rs`, then `eligibility.rs`. Keep
  both pure: no I/O and no clock (pass `now`).
- [ ] **Step 4:** Run `just test-rust` and confirm they pass. The
  existing P5 `matching_printer_ids` tests must still pass.
- [ ] **Step 5:** Commit:
  `feat(queue): add deterministic eligibility, compatibility, and ranking`.

### Task 6: Queue commands, assignment transaction, release, retry, and the queue event stream

**Owner:** Wiring, with Backend. **Depends on:** Tasks 3, 4, and 5.

**Files:**

- Create `src-tauri/src/queue/{events.rs,commands.rs}` and
  `src-tauri/src/jobs/{assign.rs,commands.rs}`. This task writes only
  `release_job`, `retry_job`, and cancel-before-start; Tasks 8 and 9
  add the rest.
- Modify `contracts/command.rs` (the new `ErrorCode`s and
  `RecoveryCode`s from the spec), `contracts/inventory.rs`, `lib.rs`,
  `tests/export_contracts.rs`, `tests/f1_contract_path.rs`, and
  `src/ipc/client.ts`
- Test: `src-tauri/tests/p7_queue.rs` and `src-tauri/tests/p7_jobs.rs`

**Interfaces:**

- Consumes: Tasks 3, 4, and 5.
- Produces:

```rust
// jobs/assign.rs
pub struct AssignRequest { pub operation_id: String, pub entry_id: String, pub printer_id: String, pub spool_id: String,
                           pub mode: AssignMode, pub assigned_by: AssignedBy }
pub fn assign(tx: &Transaction<'_>, world: &dyn WorldReader, req: &AssignRequest, now: &str) -> Result<Assigned, RepositoryError>;
pub fn release(tx, operation_id, job_id, now) -> Result<Released /* job, closed entry, replacement */, RepositoryError>;
pub fn retry(tx, operation_id, job_id, now) -> Result<QueueEntry, RepositoryError>;
pub fn cancel_before_start(tx, operation_id, job_id, now) -> Result<Job, RepositoryError>;
// queue/events.rs
pub struct QueueStream { /* like HostOperationsStream */ }
impl QueueStream { pub fn snapshot_sequence(&self) -> u64;
                   pub fn publish<R: Runtime>(&self, app: &AppHandle<R>, change: &QueueChange); }
pub struct QueueChange { pub entries: Vec<QueueEntry>, pub jobs: Vec<Job>, pub requirements: Vec<ReconciliationRequirement>,
                         pub spool_ids: Vec<String> }
```

  `WorldReader` builds the `EligibilityInput` from **inside the
  transaction** (Printers, profiles via `resolve_printer`, Spools, and
  active Jobs) plus the in-memory statuses and capabilities. That is
  how assignment re-checks eligibility atomically.
- The commands are `list_queue`, `add_to_queue`, `update_queue_entry`,
  `move_queue_entry`, `remove_queue_entry`, `explain_queue_entry`,
  `assign_queue_entry`, `release_job`, `retry_job`, `cancel_job` (the
  before-start branch only), and `get_job_history`.
- After commit, the command calls `queue.publish` and
  `spools::events::publish_ids` for the touched Spools, and
  `inventory_changes.send`.

- [ ] **Step 1: Write the failing integration tests.** Use real
  `RuntimeServices::for_test`, and copy the IPC path from
  `tests/p2_contract_path.rs`.
  - `add_to_queue_with_quantity_three_creates_three_linked_entries`
  - `add_to_queue_without_an_estimate_for_an_external_revision_is_rejected`
  - `assign_reserves_the_spool_and_creates_the_job_atomically`
  - `assign_rolls_back_everything_when_reserve_fails` (Insufficient →
    no Job, entry still `queued`, ledger id not burned)
  - `assign_replay_returns_the_same_job_and_publishes_nothing`
  - `assign_reused_operation_id_is_rejected`
  - `second_assign_to_a_printer_with_an_active_job_is_job_active`
  - `concurrent_assigns_of_one_entry_yield_exactly_one_job` (two threads,
    `std::sync::Barrier`)
  - `concurrent_assigns_of_two_entries_to_one_printer_yield_one_job`
  - `release_cancels_the_job_releases_the_reservation_and_replaces_the_entry_in_place`
  - `release_after_start_handoff_is_rejected_release_not_allowed`
  - `retry_creates_a_linked_entry_at_the_end_and_leaves_history_untouched`
  - `three_copies_are_independently_assignable_releasable_and_retryable`
  - `list_queue_snapshot_sequence_precedes_rows` (listen-before-backfill
    contract)
  - `events_carry_no_credential` (seeded secret)
- [ ] **Step 2:** Run `just test-rust` and confirm the tests fail.
- [ ] **Step 3:** Implement `assign.rs`, then `events.rs`, then the
  commands. Register every command per Global Constraint 8.
- [ ] **Step 4:** Run `just test-rust` and confirm they pass. Then run
  `just gen-contracts`, `just build` (the `CommandMap` compiles), and
  `just check-hosts`.
- [ ] **Step 5:** Commit:
  `feat(queue): add Queue commands, atomic assignment, release, and retry`.

### Task 7: Host-ops seams: the Job-linked write-ahead, the Rust API, and in-process broadcasts

**Owner:** Backend. **Depends on:** Task 2. It can run in parallel with
Tasks 3–6.

**Files:**

- Modify `src-tauri/src/host_ops/repository.rs` (`NewHostOperation.job_id`,
  and the `job_id` column in every row mapper), `host_ops/mod.rs`
  (`HostOperation.job_id: Option<String>`, ts-rs), and
  `host_ops/commands.rs` (extract the command bodies)
- Create `src-tauri/src/host_ops/api.rs`
- Modify `host_ops/services.rs` (the broadcast) and
  `connections/supervisor.rs` (the status broadcast)
- Test: `src-tauri/tests/p6_host_ops.rs` (must stay green) and the new
  `src-tauri/tests/p7_host_ops_seams.rs`

**Interfaces:**

- Produces:

```rust
// host_ops/api.rs — the command fns become thin wrappers over these.
pub(crate) type LinkInTx<'a> = Box<dyn FnOnce(&Transaction<'_>, &HostOperation) -> Result<(), RepositoryError> + Send + 'a>;
pub(crate) async fn stage<R: Runtime>(services: &Arc<HostOperationServices<R>>, operation_id: String, printer_id: String,
                                     slice_revision_id: String, link: Option<(String /*job_id*/, LinkInTx<'_>)>) -> Result<HostOperation, CommandError>;
pub(crate) async fn start<R: Runtime>(services: &Arc<HostOperationServices<R>>, operation_id: String, printer_id: String,
                                     upload_host_operation_id: String, prior: PriorState,
                                     link: Option<(String, LinkInTx<'_>)>) -> Result<HostOperation, CommandError>;
pub(crate) async fn control<R: Runtime>(services: &Arc<HostOperationServices<R>>, operation_id: String, printer_id: String,
                                       verb: ControlVerb, link: Option<(String, LinkInTx<'_>)>) -> Result<HostOperation, CommandError>;
// host_ops/services.rs
pub fn subscribe_changes(&self) -> tokio::sync::broadcast::Receiver<HostOperation>; // sent inside publish(), after commit
// connections/supervisor.rs
pub fn subscribe_status(&self) -> tokio::sync::broadcast::Receiver<String /* printer_id */>; // sent where printer.status.changed is emitted
```

- `write_ahead` runs `link` inside the same `write_repo` closure, right
  after `insert_dispatching`. If `link` errors, the whole write-ahead
  rolls back and nothing is sent.
- The raw P6 commands (`stage_slice_revision`, `start_staged_artifact`,
  pause, resume, and cancel) refuse with `JOB_ACTIVE` when the Printer
  has an active Job (decision 9). They check this inside the
  write-ahead, via `jobs::repository::active_job_for_printer`.

- [ ] **Step 1: Write the failing tests:**
  - `link_runs_in_the_write_ahead_transaction_and_commits_with_the_row`
  - `link_error_rolls_back_the_write_ahead_and_sends_nothing` (assert
    FakeMoonraker received zero uploads)
  - `crash_between_write_ahead_and_send_leaves_a_linked_dispatching_row`
    (use the existing `run_before_write_ahead` or executor injection
    hooks)
  - `subscribe_changes_sees_every_published_row_in_commit_order`
  - `subscribe_status_fires_on_apply_observation`
  - `raw_stage_on_a_printer_with_an_active_job_is_job_active`
  - `host_operation_job_id_serializes_and_is_null_for_raw_writes`
- [ ] **Step 2:** Run `just test-rust` and confirm the new tests fail
  and every P6 test still passes.
- [ ] **Step 3:** Extract the bodies of `stage_slice_revision`,
  `start_staged_artifact`, and `control_command` into `api.rs`
  unchanged, apart from the `link` parameter, and make the Tauri
  commands call them with `None`. Add the broadcasts: capacity 256, and
  a send error means "no subscribers", which is ignored.
- [ ] **Step 4:** Run `just test-rust` (all P6 tests included), then
  `just gen-contracts`.
- [ ] **Step 5:** Commit:
  `refactor(host-ops): expose a Job-linked write-ahead and change broadcasts for P7`.

### Task 8: Dispatch driver, outcome tracker, Job commands, and startup recovery

**Owner:** Backend, with Wiring. **Depends on:** Tasks 6 and 7.

**Files:**

- Create `src-tauri/src/jobs/{dispatch.rs,tracker.rs,recovery.rs,services.rs}`
- Modify `src-tauri/src/jobs/commands.rs`, adding `stage_job`,
  `start_job`, `pause_job`, `resume_job`, the after-start branch of
  `cancel_job`, and `declare_job_outcome`
- Modify `src-tauri/src/lib.rs`:
  - `jobs::recover_after_restart` right after
    `host_ops::recover_after_restart` (currently `lib.rs:380`);
  - `JobServices` in `RuntimeServices`;
  - `start_jobs_runtime` after `start_host_ops_runtime` (currently
    `lib.rs:468`).
- Modify `tests/common/fake_moonraker.rs`: add `finish_print(status: &str)`
  (sets `print_state` and closes the open job with the given history
  status) and `set_progress(f64)`.
- Test: `src-tauri/tests/p7_jobs.rs` and
  `src-tauri/tests/p7_restart_matrix.rs`

**Interfaces:**

- Consumes: `host_ops::api::{stage,start,control}` and
  `subscribe_changes` (Task 7); `ConnectionManager::subscribe_status`;
  `jobs::repository` (Task 3); `start_rule::check`.
- Produces:

```rust
// jobs/dispatch.rs
pub(crate) async fn stage_job<R>(jobs: &Arc<JobServices<R>>, operation_id: String, job_id: &str) -> Result<Job, CommandError>;
pub(crate) async fn start_job<R>(jobs: &Arc<JobServices<R>>, operation_id: String, job_id: &str, prior: PriorState,
                                 confirmation: StartConfirmation) -> Result<Job, CommandError>;
pub(crate) async fn control_job<R>(jobs: &Arc<JobServices<R>>, operation_id: String, job_id: &str, verb: ControlVerb) -> Result<Job, CommandError>;
/// Idempotent: maps a Job-linked HostOperation's current state onto its Job (D3). No-op when already applied.
pub fn apply_host_outcome(tx: &Transaction<'_>, op: &HostOperation, now: &str) -> Result<Option<Job>, RepositoryError>;
pub fn start_blockers(job: &Job, printer: &StoredPrinter, status: Option<&PrinterStatus>, loaded: &[String]) -> Vec<Blocker>;
pub fn may_start_unattended(job: &Job, printer: &StoredPrinter, status: &PrinterStatus, caps: &PrinterCapabilities, loaded: &[String]) -> bool;
// jobs/tracker.rs
pub enum TrackerVerdict { StillRunning { progress_pct: u8 }, Paused, Completed, Failed, Cancelled, Inconclusive }
pub fn verdict_from_history(job: &Job, history: &[HistoryJob]) -> (Option<String /* pinned host_job_id */>, TrackerVerdict); // pure
// jobs/recovery.rs
pub fn recover_after_restart(storage: &Storage, now: DateTime<Utc>) -> Result<Vec<Job>, StorageError>;
// jobs/services.rs
pub struct JobTimings { pub history_poll: Duration, pub inconclusive_limit: u32 }
pub struct JobServices<R: Runtime> { /* storage, host_ops, manager, queue stream, inventory tx, evaluator trigger, timings, clock */ }
```

- **Decision 1:** `start_job` needs `confirmation = BedClear`, and the
  driver may start with `Unattended` only when `may_start_unattended`
  holds (Printer `start_safety == Unattended`, status `Ready` and fresh,
  Spool loaded, and sim-proven capabilities).

- [ ] **Step 1: Write the failing pure tests** in `tracker.rs`. Cover:
  - pinning the first job above the mark;
  - ignoring an earlier print of the same file;
  - `completed`, `cancelled`, `error`, `klippy_shutdown`, `server_exit`,
    and `in_progress`;
  - a missing pinned job (`Inconclusive`);
  - a different filename while unpinned (`Inconclusive`).

  In `dispatch.rs`, test `apply_host_outcome` for every (Job state,
  host-op state) pair in the spec, including the no-op cases.
- [ ] **Step 2: Write the failing FakeMoonraker integration tests**
  (`p7_jobs.rs`):
  - `assignment_stages_immediately_and_reaches_awaiting_start`
  - `upload_failure_returns_the_job_to_assigned_with_last_failure`
  - `start_requires_bed_clear_and_the_loaded_spool`
    (`SPOOL_NOT_LOADED`, then load via `move_spool`, then start)
  - `start_from_finished_requires_prior_state_finished` (reuses P6's
    rule)
  - `unattended_printer_starts_without_confirmation_only_from_ready`
  - `confirm_bed_clear_printer_never_auto_starts`
  - `pause_resume_cancel_through_the_job_link_the_host_operations`
  - `tracker_completes_the_job_from_history_after_finish_print`
  - `tracker_fails_the_job_on_klippy_shutdown`
  - `tracker_cancels_the_job_on_host_cancel`
  - `tracker_never_adopts_a_foreign_print` (push a job for another file,
    and the Job stays `awaitingStart`, with the Printer
    `PRINTER_BUSY_EXTERNAL`)
  - `quick_print_that_finished_before_the_first_poll_is_still_completed`
    (history pinning closes P6's quick-start gap for Jobs)
  - `abandoned_start_makes_the_job_outcome_unknown_with_a_requirement`
  - `declare_outcome_requires_acknowledgement_and_is_exactly_once`
  - `job_commands_replay_and_reuse_follow_the_operations_ledger`
- [ ] **Step 3: Write the failing restart matrix** in
  `p7_restart_matrix.rs`. Add one test per spec row, each named as the
  spec names it. Every row follows the same pattern: run to the crash
  point (using the P6 injection hooks, or by dropping `RuntimeServices`
  at that point), rebuild `RuntimeServices` over the same roots, then
  assert the Job, entry, reservation, and Host Operation states plus
  **no duplicate upload or start** (FakeMoonraker request counts).
- [ ] **Step 4:** Run `just test-rust` and confirm the new tests fail.
- [ ] **Step 5:** Implement `dispatch.rs`, then `tracker.rs`, then
  `recovery.rs`, then `services.rs`, then the commands, then the
  startup wiring.
  - The driver is one tokio task selecting on the host-op broadcast,
    the status broadcast, and the poll interval.
  - On a `RecvError::Lagged` it re-reads every active Job from storage.
  - Every Job write takes `host_ops.printer_lock(printer_id)` first.
- [ ] **Step 6:** Run `just test-rust`, `just gen-contracts`, and
  `just check-hosts`.
- [ ] **Step 7:** Commit:
  `feat(jobs): stage, start, control, track, and recover Jobs through Host Operations`.

### Task 9: Material settlement (exactly once)

**Owner:** Backend. **Depends on:** Task 8.

**Files:**

- Create `src-tauri/src/jobs/settlement.rs`
- Modify `jobs/commands.rs` (`settle_job_material`,
  `correct_job_material`) and `jobs/tracker.rs` (call the settlement
  hooks inside the terminal transaction)
- Test: `src-tauri/tests/p7_settlement.rs`

**Interfaces:**

- Consumes: `reservations::{consume, consume_measured, mark_unresolved, release}`
  (Task 4) and `jobs::repository` requirements (Task 3).
- Produces:

```rust
pub fn on_completed(tx: &Transaction<'_>, job: &Job, now: &str) -> Result<(), RepositoryError>;           // consume(estimate) → settled/estimated
pub fn on_failed_or_cancelled(tx: &Transaction<'_>, job: &Job, now: &str) -> Result<(), RepositoryError>; // mark_unresolved + rrq materialReconciliation(pending)
pub fn estimated_use_mg(estimate_mg: i64, max_progress_pct: u8) -> i64;                                   // ceil(estimate × pct / 100)
pub enum SettleChoice { Estimated, Measured(AmountEntry), Defer }
pub fn settle(tx, operation_id, job_id, choice: &SettleChoice, now) -> Result<Settled, RepositoryError>;
pub fn correct(tx, operation_id, job_id, entry: &AmountEntry, now) -> Result<Settled, RepositoryError>;  // completed only, once
```

- **Rules:**
  - `settle` is allowed when settlement is `pending` or `deferred`.
  - `Defer` is allowed only from `pending`. It sets the requirement to
    `deferred`, and the amount stays unavailable.
  - `Estimated` and `Measured` consume the reservation, set the
    settlement to `settled` with its method, and resolve the
    requirement.
  - A second settle with a new operation id returns
    `JOB_ALREADY_SETTLED`.

- [ ] **Step 1: Write the failing tests:**
  - `completion_consumes_the_estimate_in_the_terminal_transaction`
  - `completion_correction_records_one_measurement_marked_as_correction`
  - `second_correction_is_rejected`
  - `failed_job_marks_the_reservation_unresolved_and_opens_a_pending_requirement`
  - `estimated_use_is_progress_proportional_and_rounds_up` (table: 0 %,
    37 %, 100 %)
  - `measured_settlement_consumes_once_and_resolves_the_requirement`
  - `defer_keeps_the_amount_unavailable_and_blocks_over_reservation`
    (a second entry needing that Spool is `INSUFFICIENT_MATERIAL`)
  - `deferred_then_measured_settles_exactly_once`
  - `settle_replay_returns_the_same_result_and_writes_no_second_ledger_row`
  - `cancelled_before_start_needs_no_settlement_and_releases_the_reservation`
  - `every_settlement_path_publishes_job_spool_and_requirement_changes_once`
    (count events)
  - `outcome_unknown_declared_failed_then_settled` (the declare →
    settle chain)
- [ ] **Step 2:** Run `just test-rust` and confirm the tests fail.
- [ ] **Step 3:** Implement the module and wire it into the tracker's
  terminal transaction and `declare_job_outcome`.
- [ ] **Step 4:** Run `just test-rust` and `just gen-contracts`.
- [ ] **Step 5:** Commit:
  `feat(jobs): settle Job material exactly once, with estimated, measured, and deferred choices`.

### Task 10: The automatic evaluator

**Owner:** Backend. **Depends on:** Tasks 5, 6, and 8.

**Files:**

- Create `src-tauri/src/queue/evaluator.rs`
- Modify `src-tauri/src/lib.rs` (start it after the Job recovery pass)
  and `queue/commands.rs` (fire triggers; `list_queue` returns
  `eligibility` and `nextAutomaticAction`)
- Test: `src-tauri/tests/p7_evaluator.rs`

**Interfaces:**

- Consumes: `eligibility::evaluate`, `jobs::assign::assign`,
  `inventory_changes`, `subscribe_status`, and `subscribe_changes`.
- Produces:

```rust
pub enum Trigger { Startup, QueueChanged, JobChanged, StatusChanged(String), InventoryChanged, PrinterChanged, CapabilitiesChanged }
pub struct EvaluatorHandle { tx: mpsc::Sender<Trigger> }
impl EvaluatorHandle { pub fn poke(&self, trigger: Trigger); }                 // coalescing, never blocks
pub fn run_once(storage: &Storage, world: &dyn WorldReader, now: &str) -> Result<EvaluationRun, RepositoryError>; // pure-ish: one serialized pass
pub struct EvaluationRun { pub summaries: Vec<EligibilitySummary>, pub assigned: Vec<Job>, pub next: NextAutomaticAction }
```

- [ ] **Step 1: Write the failing tests:**
  - `evaluator_waits_for_startup_recovery_before_the_first_run`
  - `top_to_bottom_the_first_automatic_entry_claims_the_printer`
  - `manual_and_recommended_entries_are_explained_but_never_assigned`
  - `assignment_is_sticky_when_the_queue_is_reordered`
  - `a_spool_load_triggers_assignment` (a `move_spool` into a slot
    leads to an assignment)
  - `a_printer_becoming_ready_triggers_assignment`
  - `triggers_coalesce_and_runs_never_overlap` (poke 100×, assert runs
    ≤ 2 and never concurrent)
  - `evaluator_and_operator_assign_race_yields_one_job`
  - `no_automatic_assignment_behind_an_unproven_adapter`
  - `next_automatic_action_states_the_entry_printer_or_waiting_reason`
  - `failed_job_is_never_retried_automatically`
- [ ] **Step 2:** Run `just test-rust` and confirm the tests fail.
- [ ] **Step 3:** Implement it as one task that drains the channel,
  then runs `run_once`, then publishes. Assignments go through the same
  `assign` in its own `write_repo`.
- [ ] **Step 4:** Run `just test-rust`.
- [ ] **Step 5:** Commit:
  `feat(queue): add the serialized automatic evaluator`.

### Task 11: Lifecycle guards and reference integrity

**Owner:** Backend. **Depends on:** Tasks 6 and 8.

**Files:**

- Create `src-tauri/src/jobs/guards.rs`
- Modify:
  - `printers/lifecycle.rs` (register a `JobBlockers` source, and the
    new `LifecycleBlockerCode`s `JobActive`, `JobHistoryExists`, and
    `QueueEntryPinned`);
  - `slicing/blockers.rs` (register `QueueReferencesRevision`);
  - `printers/repository.rs` (the import guard `JOBS_ACTIVE`);
  - `connections/commands.rs` and `printers/repository.rs` (the
    Connection guard);
  - `src/screens/*` blocker copy, if new codes need labels.
- Test: `src-tauri/tests/p7_guards.rs`

- [ ] **Step 1: Write the failing tests:**
  - `archive_is_blocked_by_an_active_job_and_allowed_after_terminal`
  - `archive_keeps_job_identity_and_printer_snapshot`
  - `delete_is_blocked_by_job_history` (decision 7)
  - `delete_is_blocked_by_a_pinned_queued_entry`
  - `slice_revision_delete_is_blocked_by_any_entry_or_job`
  - `model_delete_stays_blocked_transitively`
  - `printer_import_is_rejected_whole_while_a_job_is_active_or_unsettled`
  - `endpoint_change_is_blocked_while_a_job_is_past_assigned`
  - `spool_archive_is_blocked_by_an_unresolved_job_reservation`
  - `no_row_is_orphaned_after_every_allowed_lifecycle_action` (walk all
    FKs with `PRAGMA foreign_key_check`)
- [ ] **Step 2:** Run the tests and confirm they fail. Implement the
  guards, then run them again and confirm they pass.
- [ ] **Step 3:** Run `just test-rust` and `just gen-contracts`, then
  commit: `feat(jobs): guard Printer, revision, import, and Connection changes against Jobs`.

### Task 12: Frontend queue store, presentation, and web fixtures

**Owner:** Frontend, with Wiring. **Depends on:** the contracts from
Tasks 6, 8, 9, and 10 (the generated types).

**Files:**

- Create `src/queue/`:
  - `types.ts` re-exports the generated types and the type guard
    `isQueueEvent` (`type.startsWith("queue.")`).
  - `queue-store.ts` follows `host-operations-store.ts`: a
    `createSequencedStream` backfill from `list_queue`, then
    listen-before-backfill. Its read API is `queue.entries()`
    (position order), `queue.entry(id)`, `queue.jobFor(entryId)`,
    `queue.activeJobFor(printerId)`, `queue.requirements()`,
    `queue.eligibility(entryId)`, `queue.nextAutomaticAction()`,
    `queue.syncState`. Its writes cover every command in D9, each with a
    fresh `crypto.randomUUID()` and `retryOnTransportFailure`.
  - `views.ts` has `QueueView = "awaitingOperator" | "ready" | "assigned" | "blocked" | "printing" | "history"`,
    and `viewOf(entry, job, eligibility)` maps only Rust-provided
    fields.
  - `presentation.ts` holds the labels for every `JobState`,
    `BlockerCode`, `Settlement`, and `RecoveryCode` copy. "Awaiting
    material" comes from `startBlockers`.
  - `settlement.ts` formats `estimatedUseMg` (supplied by Rust in
    `Job.settlementPreview`).
  - `web-fixtures.ts` is deterministic. It holds three linked copies,
    one blocked entry, one printing Job, and one deferred requirement,
    and it reuses the `WEB_HOST_OPS_PRINTER_*` ids. Writes in web mode
    throw `needsDesktopError`.
  - `queue-store-mock.ts` and `test-records.ts`.

- [ ] **Step 1: Write the failing tests:**
  - in `queue-store.test.ts`, the backfill then events in sequence
    order, a stale event ignored, a gap triggering a resync, one
    operationId per write that is reused on a transport retry, and web
    mode refusing writes;
  - in `views.test.ts`, a table covering every (entry state, job state,
    eligibility) combination that maps to one view;
  - in `presentation.test.ts`, every enum variant has a label (an
    exhaustive `satisfies Record<…>`).
- [ ] **Step 2:** Run `just test` and confirm the tests fail. Implement,
  then run `just test` and `just build` and confirm they pass.
- [ ] **Step 3:** Commit:
  `feat(queue-ui): add the Queue store, views, presentation, and web fixtures`.

### Task 13: Design system: the `ReorderHandle` control

**Owner:** Frontend. **Depends on:** nothing. It can run any time after
Task 1.

**Files:**

- Create `src/design-system/components/ReorderHandle.tsx`, with its
  `.module.css` and `ReorderHandle.test.tsx`
- Modify `src/design-system/components/index.ts` and
  `src/design-system/Showcase.tsx`

**Interfaces:**

- Produces: `ReorderHandle(props: { label: string; index: number; count: number; onMove: (from: number, to: number) => void; disabled?: boolean })`.
  - **Keyboard:** Alt+ArrowUp and Alt+ArrowDown move by one; Alt+Home
    and Alt+End move to the top and bottom.
  - **Pointer:** a pointer drag with a drop indicator commits on
    pointerup, and Escape cancels.
  - **Announcements:** a polite live region says "Moved <label> to
    position N of M".
  - **Menu:** a companion `DropdownMenu` offers "Move to top", "Move
    up", "Move down", and "Move to bottom", so moving never depends on
    pointer drag alone.
  - It uses tokens only, and focus rings come from the design system.

- [ ] **Step 1: Write the failing tests:** keyboard moves emit
  `onMove(from, to)` and clamp at the ends; a pointer drag across two
  rows emits one move; Escape during a drag emits nothing; the live
  region text; menu items (`pointerDown`/`pointerUp`); and `disabled`
  blocks everything.
- [ ] **Step 2:** Implement it, add it to the index and the Showcase,
  then run `just test` and `just build`.
- [ ] **Step 3:** Commit:
  `feat(design-system): add a keyboard- and pointer-operable ReorderHandle`.

### Task 14: Queue destination, Queue screen, and Add to Queue

**Owner:** Frontend. **Depends on:** Tasks 12 and 13.

**Files:**

- Modify:
  - `src/screens/ActivityBar.tsx` (a Queue button with the
    ordered-list-entering-execution icon, as the umbrella spec's
    §Persistent shell describes, and a badge counting the Awaiting
    operator and Blocked entries);
  - `src/App.tsx` (add `"queue"` to `ScreenId`, `availableDestinations`,
    the `Match`, and `startQueue()` in the startup chain after
    `startHostOperations`);
  - `src/screens/AppShell.tsx` (the top bar's active Job count as a
    `PrinterRoster`-style focusable count).
- Create `src/screens/QueueScreen.tsx`:
  - view tabs (Kobalte `Tabs`), plus Dispatch Policy filter chips;
  - a `DataTable` whose columns are position, name (Model and plate),
    Project, material, estimate, destination or eligible count, state,
    and policy;
  - a `ReorderHandle` per open row, which calls `move_queue_entry`;
  - lineage shown as "Copy 2 of 3", with the lineage siblings
    highlighted on hover or focus;
  - empty and filtered-empty states that keep the filter and offer one
    recovery action.
- Create `src/screens/QueueEntryDetail.tsx`, a right dock with the tabs
  Dispatch, Artifact, and History:
  - **Dispatch:** blockers with recovery actions, ranked candidates with
    their reasons (from `explain_queue_entry`), policy and preference
    editing, and **Assign…**, **Remove**, and **Retry**;
  - **Artifact:** Slice Revision facts and estimate;
  - **History:** Job timelines and lineage links.
- Create `src/screens/AddToQueueDialog.tsx`, which asks for:
  - quantity (`NumberField`, 1–50) and policy (`RadioGroup`);
  - preference (`Select`);
  - a material estimate: prefilled from the revision; for an external
    or null-estimate revision, a required choice between accepting the
    claimed grams and entering an amount;
  - a manual Printer (`Select`), when `requiresManualPrinterSelection`.
- Modify `src/screens/SliceRevisionReview.tsx`: enable **Add to Queue…**,
  remove `QUEUE_LATER_REASON`, and navigate to
  `#nav=v1/queue/job/<id>` after success.

- [ ] **Step 1: Write the failing screen tests** (with the queue store
  mock):
  - rows render in position order;
  - each view tab filters;
  - Alt+ArrowDown on a row's handle calls `moveQueueEntry` with the
    right target;
  - a blocked row shows its first blocker and recovery;
  - three copies show "Copy n of 3";
  - Add to Queue with quantity 3 calls `addToQueue` once with
    `quantity: 3`;
  - an external revision without an estimate disables Submit, with the
    reason;
  - the activity-bar badge count and aria-label;
  - `#nav=v1/queue` renders the screen, not "not available".
- [ ] **Step 2:** Implement, then run `just test` and `just build`.
- [ ] **Step 3:** Run `just web` and check 1440 × 900 and 1024 × 700
  (the dock becomes an overlay at 1024). Save screenshots to
  `docs/screenshots/p7-queue-*.png`.
- [ ] **Step 4:** Commit:
  `feat(queue-ui): add the Queue destination, Queue screen, and Add to Queue`.

### Task 15: Job UI: assign, start, controls, settlement, the Monitor preview, and Printer Job tab

**Owner:** Frontend. **Depends on:** Task 14.

**Files:**

- Create:
  - `AssignJobDialog.tsx`: ranked candidates, a Spool choice per
    candidate, and the manual-facts acknowledgement checkbox when facts
    are absent;
  - `JobPanel.tsx`: state, progress, Slice facts, reservation,
    `startBlockers`, the Timeline from `get_job_history`, and the
    actions **Start…**, **Pause**, **Resume**, **Cancel…**,
    **Release**, **Retry**, **Settle material…**, **Correct weight…**,
    and **Declare outcome…**. Each is offered only when Rust's state
    allows it;
  - `StartJobDialog.tsx`: a Kobalte `AlertDialog` with P6's checkbox
    copy per prior state;
  - `SettleMaterialDialog.tsx`: a `RadioGroup` with **Use estimate
    (N g)**, **Enter measured remaining weight** (net or scale+tare,
    reusing P3's amount entry), and **Defer** (failed or cancelled
    only), stating that a deferred amount stays unavailable;
  - `DeclareOutcomeDialog.tsx`: an `AlertDialog` with a required
    acknowledgement;
  - `QueuePreview.tsx`: the Monitor dock default, showing the next 5
    entries, active Jobs, open requirements, and `nextAutomaticAction`.
- Modify:
  - `PrinterDashboard.tsx` (show `QueuePreview` when no object is
    selected; selecting a Job opens `JobPanel`);
  - `PrinterJobPanel.tsx` (with an active Job, render `JobPanel` and
    hide the raw P6 Stage/Start per decision 9);
  - `SpoolDetailDock.tsx` (a link from a reservation to its Job, and
    **Settle…** when a requirement is open).

- [ ] **Step 1: Write the failing screen tests:**
  - Assign lists the candidates in Rust order and calls
    `assignQueueEntry` with the chosen Spool;
  - Start stays disabled until the checkbox is checked, and is disabled
    with a reason when `startBlockers` is non-empty ("Awaiting
    material: load Spool #12 on …");
  - Settle offers Defer only for failed or cancelled Jobs, and the
    estimated option shows `settlementPreview`;
  - a deferred requirement shows in `QueuePreview` and on the Spool;
  - the Printer Job tab with an active Job hides the raw Stage;
  - Release is offered only in `assigned` and `awaitingStart`;
  - every dialog is fully keyboard operable (Tab order, Escape).
- [ ] **Step 2:** Implement, then run `just test` and `just build`.
- [ ] **Step 3:** Run `just web` at both viewports and save
  `docs/screenshots/p7-{assign,job,start,settle,preview}-*.png`. Also
  check reduced motion.
- [ ] **Step 4:** Commit:
  `feat(queue-ui): add Job assignment, start, control, settlement, and the Monitor Queue preview`.

### Task 16: The tracer: FakeMoonraker in CI and the simulator locally

**Owner:** Wiring. **Depends on:** Tasks 8–11. (Tasks 14 and 15 are
needed for the manual pass.)

**Files:**

- Create `src-tauri/tests/p7_tracer.rs` in the shape of `p6_tracer.rs`:
  one `run_queue_tracer<B: Backend>` run from two entry points, the
  `#[test]` FakeMoonraker one and the `#[ignore]` simulator one.
- Modify `justfile` (add `p7_tracer` to `test-sim`'s test list) and
  `tests/sim_moonraker.rs` (a P7 section with the failed and cancelled
  paths)

**Tracer steps** (the module doc lists them, numbered):

1. Seed one Moonraker Printer (sim-proven) and one matching Spool loaded
   in its Material Slot. Create a farm3d Slice Revision fixture.
2. Add to Queue with quantity 3. Assert 3 linked entries.
3. `explain_queue_entry` on copy 1 names the Printer as a candidate.
   Copy 2 is blocked with `JOB_ACTIVE` once copy 1 is assigned.
4. Assign copy 1 (operator). One Job, one reservation. **Restart.**
5. Staging completes, and the Job reaches `awaitingStart`. **Restart.**
6. Start without the acknowledgement is rejected. Start with `bedClear`
   succeeds. **Restart** during `printing`.
7. The print completes (sim: `quick_gcode`; fake: `finish_print("completed")`).
   The Job is completed and the estimate consumed exactly once. The
   entry is closed. **Restart**, then check nothing changed.
8. Record the optional measured correction. It appears as one
   correction.
9. Copy 2: assign, start, and cancel mid-print (sim: `long_gcode`). The
   Job is cancelled with a pending requirement. Defer. **Restart.** The
   requirement is still deferred, and the amount still unavailable.
   Settle with a measured amount; it settles exactly once.
10. Copy 3: assign and start. The sim `emergency_stop` gives a
    `klippy_shutdown`, which fails the Job. Settle with the estimate.
11. Retry copy 3. The new entry joins the same lineage, and the old
    history is unchanged.
12. Archive the Printer: blocked while a Job is active, allowed after.
    Delete stays blocked (`JOB_HISTORY_EXISTS`).
13. At every step, assert FakeMoonraker or sim request counts: no
    duplicate upload and no unconfirmed start.

- [ ] **Step 1:** Write the tracer against the fake, and run
  `just test-rust` until it passes.
- [ ] **Step 2:** Enable the sim variant, then run
  `just sim-up && FARM3D_SIM_REQUIRED=1 just test-sim && just sim-down`.
  Copy the run's `manifest.json` to
  `docs/superpowers/baselines/2026-09-2x-p7-sim-manifest-<UTC>.json`.
- [ ] **Step 3:** Run a manual `just dev` pass of the same flow against
  the simulator, with a display. Record it for Task 17.
- [ ] **Step 4:** Commit: `test(p7): add the Queue-to-settlement tracer on the fake and the simulator`.

### Task 17: Documentation and the verification record

**Owner:** Wiring (verification owner performs a fresh pass). **Depends
on:** everything.

**Files:**

- Create `docs/verification/2026-09-2x-p7-queue-jobs-dispatch.md` in the
  P6 record's shape: header, automated evidence table, simulator
  manifests, issue #17 acceptance criteria, the exit gate, visual
  verification, unavailable checks, known limitations, follow-ups, and
  files changed.
- Update `CONTEXT.md` (any drift from Task 1), `README.md` (if it has
  new recipes), and the v1 approach's known-unknowns row "Queue/Job
  state machine", marked **Resolved (P7)** with links.
- Move the screenshots under `docs/screenshots/p7-*`.

- [ ] **Step 1:** Run every gate fresh:

  ```sh
  source "$HOME/.cargo/env"
  just build && just test && just test-rust
  just gen-contracts && git diff --exit-code src/generated
  just sim-up && FARM3D_SIM_REQUIRED=1 just test-sim && just sim-down
  just check-hosts
  just package   # only if bundling changed; P7 should not change it — record "not required" otherwise
  ```

- [ ] **Step 2:** Write the record, citing test names for every issue
  #17 acceptance criterion.
- [ ] **Step 3:** Commit: `docs: record P7 queue, jobs, and dispatch verification`.

## Delivery order

```text
Task 1 (spec+ADR) ─> Task 2 (schema+state) ─┬─> Task 3 (repos) ─┐
                                            ├─> Task 4 (P3 ext) ─┼─> Task 5 (eligibility) ─> Task 6 (queue cmds) ─┐
                                            └─> Task 7 (host-ops seams) ──────────────────────────────────────────┴─> Task 8 (driver/tracker) ─┬─> Task 9 (settlement) ─┐
                                                                                                                                         ├─> Task 10 (evaluator) ─┼─> Task 16 (tracer) ─> Task 17
                                                                                                                                         └─> Task 11 (guards) ────┘
Task 13 (ReorderHandle, any time after 1)
Tasks 6/8/9/10 contracts ─> Task 12 (store) ─> Task 14 (Queue screen) ─> Task 15 (Job UI) ─> Task 16 manual pass
```

| # | Task | Depends on | Parallel with |
|---|---|---|---|
| 1 | Spec, ADR-0013, CONTEXT | — | 13 |
| 2 | Migration and state machines | 1 | 13 |
| 3 | Repositories | 2 | 4, 7 |
| 4 | P3 extensions and facet | 2 | 3, 7 |
| 5 | Eligibility | 2, 4 | 7 |
| 6 | Queue commands, assign, release, retry | 3, 4, 5 | 7 |
| 7 | Host-ops seams | 2 | 3–6 |
| 8 | Driver, tracker, recovery | 6, 7 | — |
| 9 | Settlement | 8 | 10, 11 |
| 10 | Evaluator | 5, 6, 8 | 9, 11 |
| 11 | Guards | 6, 8 | 9, 10 |
| 12 | Frontend store | contracts from 6, 8, 9, 10 | 11 |
| 13 | ReorderHandle | 1 | everything |
| 14 | Queue screen and Add to Queue | 12, 13 | — |
| 15 | Job UI | 14 | 16 (fake part) |
| 16 | Tracer | 8–11 (+ 15 for the manual pass) | — |
| 17 | Docs and verification | all | — |

Tasks that run in parallel touch disjoint files, except for the
contract registration files (`lib.rs`, `inventory.rs`,
`export_contracts.rs`, `f1_contract_path.rs`, and `client.ts`). The
controller serializes merges of those files. If two implementers run at
once, each uses its own worktree.

## Exit gate (issue #17)

- [ ] The state-machine model, transaction/concurrency, and
  restart-matrix tests pass (Tasks 2, 6, 8, 10;
  `p7_restart_matrix.rs`).
- [ ] Three-copy lineage stays linked but independently actionable
  (Tasks 3, 6, 16).
- [ ] Successful, failed, cancelled, estimated, measured, and deferred
  reconciliation each settle exactly once (Tasks 9, 16).
- [ ] The archive/delete reference tests and the frontend workflow tests
  pass (Tasks 11, 14, 15).
- [ ] The live-host tracer completes on the simulator, with the manifest
  committed (Task 16).
- [ ] Automatic dispatch is not enabled behind an unproven adapter
  (Tasks 5, 10: `no_automatic_assignment_behind_an_unproven_adapter`,
  `readonly_hardware_evidence_never_qualifies_for_automatic`).
