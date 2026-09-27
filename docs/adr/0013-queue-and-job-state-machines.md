# Queue and Job state machines

**Status:** Accepted, 2026-09-26, with the P7 spec.

## Context

P7 turns Slice Revisions into physical prints. An operator queues work as
Queue Entries, farm3d assigns each one to a Printer and a Spool as a Job,
and the Job is staged, started, tracked to its end, and its material
settled.

Two things already exist that P7 has to fit around:

- **P6 Host Operations** (ADR-0011, the P6 spec) are the only way farm3d
  writes to a printer. Each write is committed as a row before the first
  byte leaves, becomes `uncertain` when its outcome is lost, and is
  resolved only by proof read back from the host, or by the operator
  abandoning it. P6 never tracks a print to its end.
- **P3 reservations** hold Spool material for a holder, and the ledger
  records what was used.

P7 must survive a restart at every step, never repeat a write it can't
prove didn't happen, never adopt a print it didn't start, and explain at
any moment why each Queue Entry can or can't run.

Two choices here are hard to reverse, because every later phase (P8's
Attention, P9's history) builds on them.

### 1. Who owns dispatch

| Option | For | Against |
|---|---|---|
| A. Jobs write to the host themselves, beside Host Operations | One module owns the whole print | Two write paths, two recovery stories, and P6's guarantees (write-ahead, one unresolved write per Printer, reconciliation) would have to be rebuilt |
| B. Jobs are a view derived from Host Operations | No new state | A Job has states no Host Operation has (assigned, awaiting start, outcome unknown), and material settlement needs a durable owner |
| C. The Job owns dispatch as a state machine, and hands every write to P6 through a Job-linked write-ahead | One write path; P6's recovery covers the write, and the Job's recovery covers the Job | A Job transition and a Host Operation row must commit together, which needs a seam in P6 |

### 2. Where eligibility lives

| Option | For | Against |
|---|---|---|
| A. Store an eligibility state on each Queue Entry | Cheap to read | It goes stale whenever a Printer, Spool, capability, or status changes, and every one of those writers would have to update it |
| B. Derive it on demand from current facts | Always current; one pure function to test | Recomputed often; the automatic evaluator must be serialized so two runs don't race |

## Decision

**1. Option C.** The Job owns dispatch; Host Operations stay the only
write path.

- `host_ops` exposes `api::{stage, start, control}` with an optional
  `LinkInTx` closure. The closure runs inside P6's write-ahead
  transaction, right after the Host Operation row is inserted with its
  `job_id`. The Job command's own ledger claim, its re-checks, and the
  Job's transition commit in that same transaction, or nothing does.
- Host Operation outcomes reach the Job through
  `jobs::apply_host_outcome`, which is idempotent. It runs from P6's
  in-process change broadcast and again at startup, so a crash between
  P6's commit and the Job's converges.
- The Job's end comes from the host's print history, for the one history
  job the tracker pinned (the Job's own file, above the start's history
  mark). A status string only triggers a check. When the end can't be
  proved, the Job becomes `outcomeUnknown` and the operator declares it.
- While a Printer has an active Job, P6's raw write commands refuse with
  `JOB_ACTIVE`.

**2. Option B.** Eligibility is derived, never stored.

- `queue::eligibility::evaluate` is a pure function of the entry, its
  Slice Revision's facts, the Printers with their profiles, statuses, and
  capabilities, the Spools, and the active Jobs. It returns a verdict,
  ranked candidates, and blockers with recovery actions.
- Assignment re-runs the same check inside its own transaction, on rows
  read in that transaction, so the check and the reservation can't
  disagree.
- One serialized evaluator task runs it after startup recovery and on
  every change that could affect it, and it assigns Automatic entries
  through the same `assign` the command uses.

The P7 spec
(`docs/superpowers/specs/2026-09-26-p7-queue-entries-jobs-dispatch-design.md`)
holds the full state tables, transaction boundaries, and restart matrix.

## Consequences

- Every write keeps P6's guarantees: at most one unresolved write per
  Printer, no repeated upload or start, and reconciliation after a lost
  response or a restart.
- A Job command's operation id and its Host Operation's id differ (the
  latter is `<id>#hostOperation`), because both are claimed in one
  ledger.
- `host_operations` gains a `job_id` column, and P6's commands become
  thin wrappers over `host_ops::api`.
- Code that writes a Job must not hold the per-Printer lock around a
  `host_ops::api` call, because that call takes the lock itself.
- A print whose end the host's history can't prove needs an operator
  decision. That is deliberate: farm3d never guesses a completion, which
  would deduct material wrongly.
- Eligibility shown in the UI and used by the evaluator is always
  current. The price is recomputation on each trigger, and a coalescing
  trigger channel so only one evaluation runs at a time.
- Nothing stores "blocked" or "ready", so there is no stale state to
  migrate, repair, or reconcile.
