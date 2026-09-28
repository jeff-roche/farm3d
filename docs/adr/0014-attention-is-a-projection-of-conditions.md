# Attention is a projection of Conditions

**Status:** Proposed, 2026-09-27, with the P8 spec. Pending controller
review.

## Context

P8 gives the operator one place to see everything that needs them: a
Printer that went offline, a print that failed, a Job waiting for its
bed-clear confirmation, material to settle, a low Spool. Each becomes an
**Attention Event** with separate read, acknowledged, and resolved states
(the umbrella spec's "Attention, Incidents, and notifications").

The facts behind those Events already live elsewhere, and they change in
different ways:

- **P1 status** is a live stream. It changes many times a second, is
  unknown until a Connection reports after startup, and is rebuilt from a
  cache on every start.
- **P7 Jobs and Reconciliation Requirements** are durable rows. They
  change only in P7's own transactions, and a requirement stays until it
  is resolved.
- **P3 Spool facets** (such as `low`) are derived from durable rows each
  time they are read.

The umbrella and the plan also require that:

- repeated observations amend one open Event rather than flood the feed;
- a condition that clears resolves its Event, and one that returns
  creates a new Event linked to the old one;
- every durable P7 requirement is projected exactly once, including at
  startup, and resolving the requirement resolves its Event without
  erasing either history;
- a restart never duplicates an Event, and never resolves one just
  because the status isn't known yet.

This choice is hard to reverse: the Attention tables, the Incident rules,
notifications, and P9's history all build on it.

| Option | For | Against |
|---|---|---|
| A. Append-only event feed: each producer (the supervisor, the Job tracker, settlement, the Spool ledger) writes an Attention row when something happens | Simple to write at each site; a natural log | Every producer must know about Attention. Dedup and resolution need matching "cleared" events from every producer, and a missed or duplicated one leaves an Event open forever or opens two. Startup needs a separate backfill that can disagree with the live path. A restart replays nothing, so a condition that cleared while farm3d was closed never resolves |
| B. Edge-triggered subscribers: an Attention module listens to each change stream and reacts to transitions | Producers stay unaware | A lost or lagged message loses a transition, and the "last seen" state per source has to be persisted and reconciled. Backfill is still a separate path |
| C. Level-triggered projection: a pure observer reduces current state to a set of Conditions with stable dedup keys; a pure planner diffs them with the open Events; one transaction applies the diff | Idempotent: applying it twice changes nothing. Backfill is the same code with live status marked unknown. Lagged streams just mean "run a full pass". Producers stay unaware. Pure functions are table-testable | Every pass reads the current state (bounded by coalescing to one pass a second). "Unknown" must be modeled explicitly, or a restart would resolve everything. Each Condition's presence has to be definable from current state alone |

## Decision

**Option C.** Attention is a level-triggered projection of normalized
Conditions.

- **Conditions.** `attention::observe(view, now)` is pure. For every
  source it emits one of `Present(condition)`, `Absent`, or `Unknown` per
  Condition kind. A **Condition** is a current fact with a stable dedup
  key, `<condition>:<sourceKind>:<sourceId>`. It is computed, never
  stored. Rust owns every Condition, key, and severity; the frontend never
  derives one from host strings.
- **Unknown is first-class.** A status farm3d hasn't observed since
  startup (and the whole live-status side of the startup backfill) is
  `Unknown`. `Unknown` never opens and never resolves an Event.
- **The planner.** `attention::plan(open, latest_resolved, observed)` is
  pure. `Present` inserts (linked to the previous Event if it recurs) or
  amends. `Absent` resolves by the Condition's resolution mode: `auto`
  Conditions as `conditionCleared`, `action` Conditions as
  `actionCompleted`, and `manual` ones never (only the operator resolves
  them). A Condition that must not recur for the same source (a failed
  Job) is never inserted twice.
- **Idempotency.** The projector reads the open Events and applies the
  plan in one IMMEDIATE transaction. A partial UNIQUE index on
  `attention_events(dedup_key) WHERE resolved_at IS NULL` makes a second
  open Event for a key impossible. Applying the plan and planning again
  yields only no-op amendments.
- **Backfill is the same code path.** At startup, before any command is
  served, the projector runs one pass with every live-status family
  `Unknown`. Backfilled inserts are marked `origin: backfill` and never
  notify or capture. Running it again, or rebuilding the runtime, changes
  nothing.
- **Wakes, not messages.** Status, queue (a new in-process broadcast in
  `QueueStream::publish`), inventory changes, a poke from Printer
  lifecycle changes, the next offline-grace deadline, and a 60 s tick
  each schedule a full pass. A lagged receiver is just another wake.
- **History is kept.** Resolving never deletes. A P7 requirement and its
  Attention Event each keep their own history; resolving one resolves the
  other's Event by projection.

The P8 spec
(`docs/superpowers/specs/2026-09-27-p8-attention-incidents-cameras-notifications-design.md`)
holds the Condition catalogue, the observation rules, the planner and
lifecycle tables, and the exhaustive fixtures.

## Consequences

- Producers (the supervisor, P7, P3) stay unaware of Attention. P7 gains
  only an in-process broadcast in `QueueStream::publish`, and the status
  map gains a Rust-only error cause so an unreachable host and a
  misconfigured one can be told apart.
- Every Condition must be expressible from current state. A one-off
  happening with no durable trace can't be a Condition; it needs a
  durable record first (as P7's Reconciliation Requirements are).
- A condition that appears and clears between two passes (at most about a
  second apart, or while farm3d is closed) is never seen. That is
  accepted: Attention is about what needs the operator now, and durable
  facts (failed Jobs, requirements) are never missed.
- Offline grace needs a small in-memory watch per Printer. It restarts on
  every farm3d start, so an offline Event can open up to one grace later
  than a continuous run would.
- Each pass reads the relevant rows and writes open Events' observation
  counts. Passes are coalesced to one a second.
- The fixture tables are the contract: every Condition × present/absent/
  unknown × prior-Event case has a test.
