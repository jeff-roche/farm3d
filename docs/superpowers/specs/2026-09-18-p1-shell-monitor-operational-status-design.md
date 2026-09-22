# P1 Shell, Monitor, and Operational Status Design

## Status

Approved focused design for GitHub issue #11. This document narrows the
approved complete-v1 interaction design to the work owned by P1 and resolves
the decision gate recorded in the phase plan.

## Goal

Deliver a responsive Shell and Monitor that load persisted Printers and
last-known telemetry immediately, distinguish stale data from authoritative
live data, expose normalized operational/readiness state, support search,
filters, density and Monitor Sections, and open one reusable Printer detail
dock.

## Scope

### In scope

- The persistent Shell: top bar, activity rail, workspace, and status bar.
- Monitor as the default destination.
- Persisted Printer loading without a blocking Farm-wide spinner.
- A Rust-owned operational, readiness, and freshness policy.
- Last-known telemetry persistence as a reconstructable cache.
- Listener-before-backfill reconciliation and stale-to-live transitions.
- Printer search, operational filters, density, and Monitor Sections.
- Comfortable Printer cards and compact Printer rows.
- Inspectable aggregate Printer counts with bounded rosters.
- Severity that always combines icon, text, and color.
- One responsive, keyboard-operable Printer detail dock.
- First-run, loading, empty, filtered-empty, stale, missing-field, and error
  states.
- Responsive and keyboard verification at 1440 × 900 and 1024 × 700.

### Non-goals

- Queue Entry or Job persistence, dispatch, or controls.
- Attention Event or Incident persistence.
- Camera data or controls.
- Spool or Material Slot data.
- Batch Printer setup.
- Durable Printer location; P2 owns that field and its editing workflow.
- Mapping the discarded legacy `group` field to location.
- Treating raw host job strings as Farm3D Jobs.
- Persisting search, filter, selection, dock-open, dock-tab, focus, scroll, or
  dock-width display state.

## Product vocabulary and phase boundaries

- Adapter-reported activity is labeled **Host print** or **Host activity**.
  It is never labeled or counted as a Farm3D Job.
- The P1 Attention filter means current normalized warning or fatal Printer
  conditions. It does not mean persisted Attention Events.
- Active Job and actionable Attention counts remain absent until P7 and P8
  provide authoritative data.
- Queue and Spools remain absent from production navigation until wired.
- The dock has a neutral closed/default state when no Printer is selected.
  P1 does not fabricate the future Queue preview.
- The dock ships operational Status and existing setup surfaces. Empty Job and
  Camera tabs are not shown.
- First run makes Add Printer actionable. Future Spool and Model steps may be
  named as guidance, but cannot be inert buttons presented as available flows.

## Backend-owned operational model

Rust returns three orthogonal results rather than one overloaded status.

### Operational state

`OperationalState` has these values:

- `setupIncomplete`
- `error`
- `offline`
- `connecting`
- `printing`
- `paused`
- `busy`
- `ready`
- `unknown`

The precedence order is:

1. `setupIncomplete` when durable configuration cannot support monitoring,
   including a missing usable Connection or unresolved required Printer
   Profile identity.
2. `error` for a current terminal supervisor or normalized host-health fault.
3. `offline` for an explicitly disconnected configured Connection.
4. `connecting` while supervision or reconciliation is in progress.
5. `unknown` when connected but no fresh normalized observation exists.
6. `printing`, `paused`, or `busy` from adapter-normalized current activity.
7. `ready` only when the Printer is online, fresh, setup-complete, and the
   adapter reports an idle/ready condition.

Adapters normalize their raw host strings before central policy consumes
them. Raw adapter strings never appear in the cross-adapter operational enum.

### Readiness

`PrinterReadiness` contains:

- `state`: `ready | notReady`
- `reason`: a stable machine-readable reason code when not ready

Initial reason codes are:

- `setupIncomplete`
- `connectionError`
- `offline`
- `refreshing`
- `staleTelemetry`
- `printerBusy`
- `unknownState`

Readiness is `ready` only when operational state is `ready`. P7 may add
compatibility, reservation, archive, and start-safety reasons without parsing
adapter values or frontend labels.

### Freshness

`TelemetryFreshness` has these values:

- `fresh`
- `stale`
- `unavailable`

Policy:

- A cached snapshot loaded after restart starts stale regardless of timestamp.
- An explicit offline or error transition makes retained telemetry stale
  immediately.
- Online telemetry remains fresh for 30 seconds after the latest successful
  adapter-health observation.
- Each adapter supplies a health observation at least every 10 seconds using
  native traffic, polling, or a supervised liveness check.
- Rust emits a stale transition after 30 seconds without an observation.
- Retained stale values remain visible indefinitely with their age.
- A Printer with no prior telemetry is unavailable, not stale.
- Missing numeric fields remain absent and are never represented as zero.
- Rust supplies `lastObservedAt` and `freshUntil`. The frontend formats age but
  does not choose or reimplement the threshold.

A stale last-known print does not count as currently printing. Presentation
may say, for example, “Offline; last seen printing 3 minutes ago.”

## Runtime status and persistence

### Status model

The runtime status separates these facts:

- Connection/adapter health and its transition timestamp.
- Last telemetry observation and `lastObservedAt`.
- Adapter-normalized activity.
- Optional host activity name and telemetry values used by Monitor.
- Backend-derived operational state, readiness, and freshness.
- Event stream identity and sequence remain in the existing envelope/backfill
  protocol rather than becoming Printer status fields.

The existing frontend stream-reconciliation stale flag means “event and
backfill synchronization is uncertain.” It remains separate from telemetry
freshness.

### Last-known telemetry cache

Persist one reconstructable snapshot per Printer in a dedicated SQLite table.
The cache:

- Stores only normalized/validated Monitor telemetry and `lastObservedAt`.
- Does not store credentials, raw adapter responses, event history, or Farm3D
  Job identity.
- Does not increment the Printer's durable revision.
- Is excluded from editable Printer export and domain history.
- Is removed when its Printer is deleted.
- Coalesces writes to at most once every 30 seconds per Printer, plus meaningful
  activity transitions.
- Does not depend on graceful shutdown for its final write.

On startup, Rust loads durable Printers and cached snapshots, exposes cached
fields as stale, restores Connection Supervisors, and replaces cached values
after authoritative live observations arrive.

Cache read failure does not block durable Printer loading. Cache write failure
does not fail Printer commands or disconnect an adapter; the in-memory status
continues and a recoverable warning is reported.

### Connection transitions

- Connecting, offline, and error transitions retain prior safe telemetry.
- State transitions update adapter health without replacing the retained
  observation with empty values.
- A successful live observation updates telemetry, observation timestamps,
  freshness, operational state, and readiness as one coherent status.
- Clearing a Connection removes retained telemetry and its cache row, then
  publishes a telemetry-unavailable `setupIncomplete` status so the frontend
  cannot retain orphaned readings. Deleting a Printer emits the status
  removal/tombstone.
- Partial telemetry updates do not erase unrelated previously observed fields.

## Event and contract path

- Keep the existing `farm3d-event-v1` event channel and
  `printer.status.changed` event type.
- Keep listener-before-backfill ordering, stream ID, sequence cursor, buffering,
  deduplication, gap recovery, bounded retries, and old-stream rejection.
- `printer_statuses` and status events carry the same generated status shape.
- Consolidate duplicate Rust status DTOs so the runtime type is the single
  source exported to TypeScript.
- Generate all operational, readiness, freshness, health, and snapshot wire
  types from Rust. Frontend code cannot duplicate the state machine.

## Monitor preferences

Extend the revisioned SQLite Settings record with:

- `monitorSection`: `location | printerModel | operationalState | none`
- `monitorDensity`: `comfortable | compact`

Defaults:

- Density defaults to `comfortable`.
- P1 defaults sectioning to `printerModel` because durable location does not
  exist yet.
- P2 may default new users without an explicit choice to `location` once one or
  more Printers have durable locations.

The following remain ephemeral SolidJS display state:

- Search text and active filter.
- Selected Printer.
- Dock open state and active tab.
- Focus and scroll positions.
- Dock width.

Invalid saved values fall back to defaults without preventing Settings from
loading.

## Shell

The Shell retains the top bar, activity rail, workspace, and status bar.

- Monitor is the default destination and replaces the current “Printers” label.
- The rail exposes only destinations with real P1 behavior plus existing wired
  Library and Settings entry points.
- The top bar exposes total Printer count and approved operational Printer
  aggregates. It omits unavailable Job and Attention aggregates.
- Aggregate Printer counts are hoverable and keyboard-focusable.
- A count opens a bounded roster from the same snapshot used to compute the
  count. It shows at most eight Printer names and states, followed by a View all
  row when more exist.
- The status bar reports adapter health and age of the latest live event rather
  than duplicating navigation or connection-state counts.

Aggregate truth sources are:

| Aggregate | Source |
| --- | --- |
| Total Printers | Durable active Printer records |
| Printer Model section count | Durable Printer catalog identity |
| Ready, Printing, Offline, Setup incomplete | Current Rust-normalized state |
| Stale | Current Rust freshness result |
| P1 Attention | Current normalized warning/fatal conditions |
| Adapter health | Current Connection Supervisor state |
| Last-live-event age | Latest backend-observed live event timestamp |

Stale last-known printing does not contribute to current Printing counts.

## Monitor frontend architecture

### State and derivation

A focused Monitor store consumes durable Printers and generated runtime status.
It owns only display state and pure presentation derivation:

- Search text.
- Active filter.
- Selected section and density, initialized from Settings.
- Selected Printer identity.
- Filtering, sorting, sectioning, bounded roster membership, and card/row view
  models.

Search covers fields available in P1:

- Printer name.
- Printer Model/vendor.
- Host activity name.

The public search input accepts location later without changing Monitor
component contracts. Location grouping remains unavailable in P1 because no
durable location exists.

Filters are:

- All.
- Attention.
- Printing.
- Ready.
- Offline.
- Setup incomplete.

Each filter consumes generated normalized results. Frontend code does not parse
raw host states to assign a filter.

### Component boundaries

Decompose the current `PrinterDashboard` responsibilities into:

- Monitor composition/canvas.
- Monitor toolbar.
- Printer card.
- Printer compact row.
- Printer detail dock.
- Operational Status content.
- Setup content composed from current identity, Profile, and Connection panels.

Cards and rows consume the same view model so density changes presentation, not
meaning.

### Printer presentation hierarchy

Cards and compact rows show, in order:

1. Printer name and normalized operational state.
2. Host print/activity or readiness reason.
3. Progress when printing and reported. Remaining time appears only when a
   trustworthy source exists.
4. Current and target hotend/bed temperatures when reported.
5. Freshness whenever stale, unavailable, or disconnected.

P1 does not invent material information. Catalog drift, Profile overrides,
host kind, and low-priority setup facts stay in detail.

Severity always combines an icon, text label, and tokenized color. Recoverable
conditions use a warning marker; fatal conditions use a stop/error marker.

### Required states

- Loading persisted Printers while retaining any available content.
- First run with Add Printer as the direct action.
- Empty Farm.
- Filtered-empty results that preserve the active query/filter and offer one
  precise reset action.
- Telemetry unavailable because no observation exists.
- Cached or disconnected stale telemetry with age.
- Reconciliation uncertainty while retaining visible values.
- Partial telemetry with absent fields rendered as unavailable, never zero.
- Recoverable cache, listener, or backfill errors.

## Shared UI primitives

P1 adds shared components only where repeated use justifies them:

- A bounded Printer roster popover for top-bar and section counts. It supports
  pointer hover, keyboard focus, at most eight rows, and View all.
- A severity marker with fatal, warning, informational, and resolved variants.

The responsive Printer detail dock may remain a Monitor component in P1. It
should become a design-system primitive only if another implemented screen uses
the same behavior during this phase.

All components follow `DESIGN.md`: Kobalte primitives where applicable, CSS
Modules, `--f3d-*` tokens, visible focus rings, and no Material-style elevation
or ripple behavior. New design-system components are exported and included in
the Showcase.

## Printer detail dock

- Selecting a Printer opens one dock without replacing the Monitor canvas.
- Closing Printer detail returns to the neutral closed/default state.
- At 1440 × 900, the activity rail, Monitor workspace, and inline dock remain
  visible.
- At 1024 × 700, the dock is an overlay and does not clip primary controls.
- More generally, the dock overlays when an inline dock would leave less than
  44rem for the workspace or less than 15rem per comfortable card.
- The dock supports keyboard and pointer close behavior.
- If resizing remains available, it must have keyboard equivalents. P1 does
  not persist dock width.
- P1 shows operational Status and Setup. Setup composes existing catalog,
  Profile, Connection, and notes behavior.
- Unknown or deleted deep-linked Printers close the dock while retaining the
  Monitor destination.

## Error and recovery behavior

- Cache read failure falls back to durable Printers and unavailable telemetry.
- Cache write failure preserves in-memory operation and surfaces a recoverable
  warning.
- Listener/backfill failure keeps values visible, marks synchronization
  uncertain, and retries using the existing bounded backoff.
- Connection loss retains readings and immediately marks them stale.
- Missing fields do not erase unrelated known values.
- Invalid preferences fall back without blocking Settings.
- A status-listener startup failure is surfaced rather than becoming an
  unhandled promise rejection.
- App cleanup disposes a listener even when registration finishes after
  component cleanup began.
- Import and durable Printer replacement preserve status only for surviving
  Printer IDs; removed IDs are cleared.

## Accessibility and adaptation

- Kobalte primitives back menus, popovers, tabs, form controls, and overlays.
- Hover disclosures are keyboard-focusable.
- Focus order follows activity rail, toolbar, Monitor content, dock, and status
  bar.
- Card and row selection, roster opening, dock tabs, close, and any resize
  behavior have keyboard equivalents.
- Existing focus rings remain visible.
- Severity never depends on color alone.
- Reduced-motion preference disables nonessential transitions and indeterminate
  animation.
- The 1024 × 700 layout keeps navigation and primary actions reachable without
  clipped controls.

## Acceptance criteria

1. Persisted Printers render before live Connection refresh completes.
2. Cached telemetry renders immediately after restart as stale, including its
   last-observed age.
3. The first authoritative live observation changes cached fields from stale to
   fresh without replacing the Monitor or losing selection.
4. Rust applies the approved operational precedence and returns generated
   operational, readiness, and freshness contracts.
5. Raw host activity remains distinct from Farm3D Jobs in contracts, labels,
   search, and aggregate counts.
6. Connection loss and reconnect preserve last-known safe values; stale values
   never count as currently printing or ready.
7. Missing optional fields remain absent and do not erase unrelated retained
   observations.
8. Clearing a Connection removes retained telemetry from backend and frontend
   state and replaces it with telemetry-unavailable `setupIncomplete`; deleting
   a Printer removes its status entirely.
9. Search, the six approved filters, comfortable/compact density, and available
   Monitor Sections work over the same normalized Printer snapshot.
10. Density and Monitor Section persist through revisioned SQLite Settings;
    ephemeral view state does not persist.
11. Aggregate counts and bounded rosters use the same membership snapshot,
    support pointer and keyboard access, and cap visible Printer rows at eight.
12. First-run, loading, empty, and filtered-empty states are distinct.
13. One reusable Printer detail dock opens from card/row or deep-link selection,
    closes safely, and retains the Monitor canvas.
14. At 1440 × 900 the dock is inline; at 1024 × 700 it is an overlay and all
    primary controls remain operable.
15. New interactions pass keyboard verification with visible focus and correct
    focus order.
16. Rust tests cover policy, freshness boundaries, persistence/restart,
    connection transitions, and missing fields.
17. Frontend tests cover reconciliation, view derivation, cards/rows, rosters,
    dock behavior, required states, and keyboard interaction.
18. Integration tests prove listener-before-backfill, racing-event handling,
    restart hydration, stale-to-live transition, and status removal through the
    Tauri path.
19. `just build`, `just test`, and
    `source "$HOME/.cargo/env" && just test-rust` pass.

## Delivery strategy

Use contract-first vertical slices:

1. Land the Rust policy, persistence, and generated contracts.
2. Wire the frontend runtime model and freshness behavior.
3. Add justified shared UI primitives.
4. Deliver Shell, Monitor controls/presentation, and dock as separately
   reviewable tasks.
5. Complete the vertical tracer with Tauri-path, responsive, keyboard, and
   restart verification.

This ordering keeps Rust authoritative while producing independently testable
increments and avoids a frontend fixture model diverging from backend policy.
