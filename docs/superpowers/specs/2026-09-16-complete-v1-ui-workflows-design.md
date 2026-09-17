# Complete v1 UI and user workflows

## Status

**Approved product direction.** This is the umbrella interaction design for
farm3d v1. It supersedes conflicting UI scope and layout decisions in
`2026-08-19-print-farm-screens-design.md`,
`2026-08-20-printer-catalog-and-printers-design.md`, and
`2026-08-20-settings-system-design.md`. It retains their data-integrity,
printer-catalog, Connection, credential, and adapter decisions unless this
document explicitly says otherwise.

This document is intentionally too broad for one implementation plan. Its
purpose is to keep independently delivered subsystems consistent. Each major
area under "Implementation decomposition" requires its own focused spec and
plan before implementation.

## Product boundary

farm3d v1 is a single-user, local desktop application for operating a small
3D-printer farm. It is optimized by default for 5–30 Printers, with configurable
density that remains usable below and above that range.

Managing Printers, slicing, and the Library remain co-equal v1 pillars per
ADR-0001. Their **interaction emphasis**, based on operator frequency, is:

1. **Monitor the Farm.** Understand readiness, active Jobs, progress, material,
   and exceptions without navigating between Printers.
2. **Queue and operate.** Prepare ordered work, resolve blockers, turn Queue
   Entries into Jobs, and control starts safely.
3. **Prepare Models.** Organize Models into Projects, prepare build plates, and
   create immutable Slice Revisions.

Spool inventory, attention handling, history, backup, and diagnostics support
those three activities rather than becoming competing dashboards.

### In scope

- Printer setup individually and in batches.
- Moonraker, OctoPrint, and ElegooLink as network Connection choices once their
  adapters exist.
- Configurable Printer-card density and grouping.
- Managed and linked Library Models.
- Organizational Project folders.
- STL, 3MF, and pre-sliced G-code inputs.
- Operational slicing presets with an interface that can deepen later.
- Per-Queue-Entry manual, recommended, or automatic dispatch.
- Per-Printer unattended-start safety policy, defaulting to operator
  confirmation.
- First-class Spools and single- or multi-material Printer slots.
- Optional cameras, incident/completion snapshots, and local desktop
  notifications.
- Job and Incident history, backup/restore, and redacted diagnostics.

### Out of scope

- Multiple users, accounts, permissions, or remote Farm access.
- Mobile or web administration.
- Customer orders, due dates, fulfillment, or billing. Projects are folders,
  not orders.
- Cloud synchronization or hosted notifications.
- Webhooks and messaging integrations.
- Serial/USB Printer Connections.
- Barcode-driven Spool handling.
- A full replacement for OrcaSlicer's expert setting surface in v1.
- A spatial 3D Farm floor plan.

## Design contract

The existing editor-tool visual language remains authoritative:

- Dense, flat, neutral panels with one farm-green accent.
- Background shade steps, borders, and geometry establish hierarchy; shadows
  are limited to temporary overlays.
- Small radii and restrained motion.
- Tabler-style line icons and the existing typography.
- Existing `--f3d-*` tokens and Kobalte primitives. No hardcoded theme colors,
  font sizes, or radii in production component CSS.
- The 3D viewport belongs to Model inspection and plate preparation only.
- Operational state outranks configuration metadata.
- Exact values remain textual; charts do not replace values operators need to
  act on.
- Severity always uses icon, text, and color together.

Every persistent region must perform work. The UI does not add decorative KPI
cards, generic welcome dashboards, novelty navigation, or a kanban board where
an ordered production list is clearer.

## Information architecture

### Persistent shell

The shell retains the current top bar, activity rail, content area, and status
bar. The primary activity-rail destinations are:

1. **Monitor** — telemetry-trace icon; badge is unresolved actionable
   Attention Events.
2. **Queue** — ordered-list-entering-execution icon; badge is queued or blocked
   Queue Entries requiring visibility.
3. **Library** — 3D-object icon; contains Projects, Models, plates, and Slice
   Revisions.
4. **Spools** — physical-spool icon; badge is low or reconciliation-needed
   inventory.
5. **Settings** — gear at the bottom of the rail.

The top bar contains the farm3d identity, current destination, total Printer
count, active Job count, open actionable Attention Event count, and the
Attention trigger. The footer reports Connection-adapter health and age of the
last live event, not duplicate navigation.

Any aggregate Printer count in a header, group heading, or footer is hoverable
and keyboard-focusable. It opens a bounded roster showing Printer names and
states. Rosters show at most eight Printers before an additional **View all**
row; they never create an unbounded tooltip.

### Reusable right dock

Monitor uses one learned right-side dock:

- Queue preview is the default.
- Selecting a Printer, Job, Attention Event, or Incident reuses the dock for
  that object's detail.
- Closing detail returns to Queue preview.
- The Monitor canvas remains visible while detail is open.
- When the dock would leave less than 15 rem per comfortable card or less than
  44 rem for the main workspace, it becomes an overlay rather than squeezing
  content below its useful width.

## Monitor

Monitor is the default and primary destination. It is calm during normal
operation and visually prioritizes exceptions only when they exist.

### Controls

- Filters: All, Attention, Printing, Ready, Offline, Setup incomplete.
- Monitor Sections: location, Printer Model, operational state, or none.
- Density: comfortable cards by default; compact rows for larger Farms.
- Search by Printer name, Model, location, or current Job.
- Add Printer and batch-add entry points.

Monitor Section and density preferences persist locally. Location is the
default sectioning once locations exist; otherwise Printer Model is the
fallback. This individual-card design supersedes the earlier same-Model
multi-instance card UI; it does not change catalog identity or persistence.

### Printer card hierarchy

Cards display, in order:

1. Printer name and current operational state.
2. Active Job or readiness reason.
3. Progress and estimated remaining time when printing.
4. Current and target hotend/bed temperatures where reported.
5. Loaded Spool summary or material mismatch when relevant.
6. Freshness when stale or disconnected.

Catalog drift, Profile overrides, host kind, and low-priority setup facts live
in Printer detail rather than competing with operational state on every card.

Recoverable attention uses a warning-triangle icon. Fatal conditions use an
octagonal stop/error icon. The icons precede the message on cards, dock rows,
Attention entries, and desktop notifications.

### Printer detail dock

Printer detail has four stable tabs:

- **Status:** readiness, temperatures, Material Slots, current Attention
  Events, and recovery actions.
- **Job:** current run, progress, controls, Slice facts, reservations, and event
  timeline.
- **Camera:** live preview where available, snapshot history, retention, and
  stream health.
- **Setup:** identity, Profile, Connection, slots, safety, camera source, notes,
  archive, and guarded deletion.

## Printer lifecycle

### Single Printer setup

The setup sequence is progressive:

1. **Identify:** choose catalog Model and exact nozzle variant, then name and
   locate the physical Printer. V1 cannot add hardware absent from the bundled
   catalog; it explains how to request or contribute a catalog update.
2. **Connect:** choose a discovered candidate or manually enter protocol, host,
   port, TLS, and credential. Discovery is never required.
3. **Equip:** configure named Material Slots, optional camera source, and loaded
   Spools.
4. **Operate:** choose the unattended-start safety policy and alert defaults.
5. **Review:** confirm capability mismatches, credential-store location, and
   resulting Farm impact before saving.

Testing a Connection does not save the Printer or start monitoring. A user can
save a Profile-only Printer and finish later; Monitor calls this **Setup
incomplete**, not **Offline**.

### Batch setup

Batch setup accelerates physically identical Printers without creating a
permanent shared-configuration entity.

1. Select the shared catalog Model, variant, bed setup, Material Slot layout,
   camera template, and safety default once.
2. Generate instance rows from quantity, naming pattern, and one or more bays;
   paste tabular rows; or import CSV.
3. Edit per-instance name, location, host, and credential choice.
4. Map discovered hosts to rows. Already-configured hosts and ambiguous matches
   are identified rather than silently selected.
5. Apply protocol, port, TLS, shared credentials, or safety settings to selected
   rows explicitly.
6. Probe Connections with bounded concurrency and show identity mismatch, auth,
   duplicate-host, timeout, and success per row.
7. Create valid independent Printer records. Failed or incomplete rows remain
   recoverable and may be saved as Setup incomplete.

Shared setup is copied at creation. It does not establish hidden inheritance.
Later batch editing requires explicit Printer selection and previews every
affected value. Host, credential reference, loaded Spools, runtime state,
notes, camera endpoint, and Job history are always instance-specific.

### Maintenance and retirement

- A nozzle change rebinds the Printer to a catalog variant rather than
  overriding nozzle diameter.
- Catalog drift presents old and new values with **Accept** (rebaseline),
  **Keep my value** (create supported field overrides), or variant rebind where
  identity no longer resolves.
- Connection changes preserve history and validate before replacing a working
  configuration.
- Archiving is the default retirement action. It removes the Printer from
  scheduling while preserving Jobs, Incidents, Spool movement, and immutable
  Profile snapshots.
- Permanent deletion is secondary, unavailable while active Jobs or unresolved
  reservations exist, and requires explicit confirmation.

## Queue Entries and Job dispatch

Queue is an ordered list of Queue Entries, not a kanban board. The
top-to-bottom order is scheduling priority; users do not manage a second
competing priority system. Reordering supports pointer and keyboard movement.

### Queue views

- Awaiting operator.
- Ready.
- Assigned.
- Blocked.
- Printing now.
- Completed, failed, cancelled, and archived history.
- Filters for each Dispatch Policy.

Rows show Queue Entry name, Project when present, material, estimate,
destination or eligible count, state, Dispatch Policy, and queue position.
Selecting a row opens Dispatch, Artifact, and History detail.

### Dispatch Policies

- **Manual:** the operator selects a compatible Printer and assigns the Queue
  Entry.
- **Recommended:** farm3d ranks compatible Printers and explains the ranking;
  the operator confirms assignment.
- **Automatic:** farm3d assigns the first qualifying currently idle Printer
  according to the selected preference.

Assignment converts the Queue Entry into a Job targeted at one Printer.
Assignment and starting are separate. The Printer's safety rule determines
whether farm3d may start unattended. The default rule is **assign/upload, then
confirm the bed is clear**.

### Eligibility and reservations

A Queue Entry can become a Job only when these gates pass:

- Slice Revision is valid and complete.
- Printer Profile, build volume, nozzle, and required features are compatible.
- Connection can perform the required upload/control operation.
- A compatible Spool is selected for each required material. Automatic
  assignment requires it already loaded in a compatible Material Slot; manual
  and recommended assignment may create a Job awaiting an explicit load action.
- Available Spool amount covers the estimate and existing reservations.
- Printer is schedulable and not archived, in error, or Setup incomplete.

Assignment reserves the Printer and selected Spool amounts atomically. A
blocker states what failed, which Printers qualify, and the direct recovery
action. Automation never hides what it will do next.

The automatic evaluator runs after startup reconciliation and whenever queue
order, Queue Entry requirements, Printer readiness, Connection capability,
Material Slot loading, or Spool availability changes. It considers Queue
Entries top to bottom. Candidate ties use the selected preference, then Printer
name and stable id. Assignments are sticky until an operator releases them;
later reordering does not silently move an assigned Job.

Release is available only before printing begins. It cancels the assigned Job
with reason **released before start**, atomically frees Printer and Spool
reservations, and creates a linked replacement Queue Entry from the same Slice
Revision at the prior queue position. The cancelled Job remains in history.

### Queue Entry and Job lifecycle

The user-visible transition is:

`prepared Slice Revision -> queued Queue Entry -> eligible/blocked -> assigned
Job -> staging/uploading -> awaiting material or start -> starting -> printing
-> completed/failed/cancelled`

Staging may upload before operator start confirmation; it never issues the host
start command. **Starting** begins only after material and safety gates pass and
is the host start-command transition. Paused is a running Job state, not a Queue
location. Retry creates a linked Queue Entry from the same immutable Slice
Revision; history is not rewritten.

Quantity creates linked independent Queue Entries. Each can become a separate
Job and be assigned, retried, cancelled, and reconciled independently.

Upload and start commands carry a persisted operation id. After restart or an
uncertain response, farm3d queries host state and artifact identity before
retrying; it never automatically starts when it cannot prove the prior command
did not take effect.

## Library, Projects, and slicing

### Projects and Models

Projects are organizational folders only. They carry no quantities, deadlines,
priority, or fulfillment status. Models may remain Unfiled.

The Library workspace has:

- Project and saved-view navigation on the left.
- Model grid/list and central Model/build-plate inspector.
- A right-side preparation panel.

Opening an ad-hoc supported file previews it without adding it to the Library.
Adding it asks for Project and storage mode:

- **Managed:** farm3d copies the source into managed local storage.
- **Linked:** farm3d retains the source path, imports an immutable Model Source
  Revision for current work, and watches for later source changes.

Linked source removal preserves metadata, thumbnails, imported Model Source
Revisions, prior Slice Revisions, and history. Recovery offers **Locate source**;
after the source is located, **Convert to managed** copies it into managed
storage. A duplicate-content import offers reuse existing, add as another
Model, or replace the managed source without silently choosing.

### Input behavior

- **STL:** geometry, placement, rotation, scale, arrangement, and slicing are
  editable.
- **Geometry-only 3MF:** same behavior, including multiple plates where present.
- **Rich 3MF:** import supported plates and metadata; identify unsupported
  slicer-specific fields before discarding them.
- **G-code:** read-only preview and metadata validation. Import wraps it as an
  externally produced Slice Revision. The user must confirm target Printer
  Profile, nozzle, and material facts not trusted from metadata. Missing facts
  force manual dispatch to an explicitly selected compatible Printer. G-code
  never presents fake editable slicing settings.

### Plate preparation

V1 supports orbit, pan, zoom, reset, standard views, measure, select, move,
rotate, scale, lay-flat, arrange, and multiple plate tabs. Transform operations
also expose keyboard-focusable numeric fields; every toolbar operation has a
keyboard command. The viewport always shows the target build volume and
out-of-bounds geometry.

### Operational slicing controls

The preparation panel exposes:

- Target Printer Profile, with matching Farm Printer count.
- Material and quality Profiles.
- Infill, walls, supports, and adhesion.
- Compact Strength and Support sections.
- A collapsed Advanced Overrides section containing only settings explicitly
  supported by the focused slicing spec; it is searchable once it exceeds 12
  fields.

The structure may deepen into a full slicer later: settings sections expand and
plate tools grow, but the Library, Project, viewport, target Profile, and Slice
Revision concepts remain stable.

### Slice workflow

1. Select target Printer Profile, material, quality, and overrides.
2. Validate geometry, placement, build volume, nozzle, material, and slicer
   availability.
3. Run OrcaSlicer as a cancellable background operation. Logs remain collapsed
   during success and expand automatically on failure; users can expand them at
   any time.
4. Save success as an immutable Slice Revision with estimates and compatibility
   facts.
5. Choose copies, Dispatch Policy, queue position, and add Queue Entries.

Import and every linked-source content change create a Model Source Revision.
Unsliced preparation against an older revision becomes stale and requires the
user to reload or deliberately continue. Each plate creates its own Slice
Revision referencing one Model Source Revision and plate identity. Changing
source content or settings never mutates an artifact referenced by an existing
Queue Entry or Job.

## Spool inventory and Material Slots

### Spool record

A Spool includes manufacturer, product, material family, color, diameter,
nominal net weight, current net weight, low threshold, storage location, and a
measured-or-estimated confidence marker. Color is supplemental data; identity
and status remain textual.

Current amount can be entered directly or derived from scale weight minus a
reusable empty-spool tare.

### Inventory views and facets

Spool lifecycle is active, empty, or archived. Location is either storage or
one Material Slot. Loaded, reserved, low, measured, and estimated are
orthogonal facets that may appear together. Inventory filters compose these
facets with material, location, and Printer.

Single-extruder Printers have one Material Slot. AMS/MMU/tool-changing Profiles
expose multiple named slots and constraints. One physical Spool cannot occupy
two slots.

### Loading and movement

Loading can begin from a Spool or Printer Setup:

1. Select the destination Printer and Material Slot.
2. If occupied, assign the displaced Spool a storage location in the same
   transaction.
3. Confirm compatibility and commit the movement.
4. Preserve movement history for Jobs and Incidents.

### Reservation and reconciliation

- Job assignment reserves estimated material on specific Spools but does not
  deduct it.
- Successful completion deducts the Slice estimate and offers an optional
  measured-weight correction.
- Failed or cancelled Jobs require reconciliation: use estimated consumption,
  enter a measured weight, or defer.
- Deferred reconciliation is an Attention Event and blocks overcommitting the
  uncertain amount.
- A later measurement replaces the current estimate and records a correction;
  it does not rewrite historical usage entries.

## Attention, Incidents, and notifications

### Severity and lifecycle

Attention Events have severity, source object, timestamp, action requirement,
visibility, acknowledgment, and resolution:

- Visibility is **unread** or **read**.
- Acknowledgment is **unacknowledged** or **acknowledged**.
- Resolution is **open** or **resolved**.
- Acknowledging or resolving implies read. An acknowledged Event may remain
  open and stays in the actionable badge until resolved.

An automatically detected condition resolves when its source condition clears;
manual confirmations and reconciliation resolve when their action completes.
If a resolved condition recurs, it creates a new Event linked to the prior one.
Repeated telemetry updates amend one open Event rather than flooding the feed.

Fatal Printer failures, start confirmations, material reconciliation, offline
Printers, low inventory, and completed Jobs may produce Attention Events.
Severity icons distinguish fatal errors, recoverable alerts, and informational
events without relying on color.

### Desktop notifications

Desktop notifications are enabled by default for fatal failures, operator
confirmations, and Job completion while farm3d is unfocused. Each deep-links to
the exact Printer, Job, Spool, or Incident. Global settings configure event
classes; the in-app Attention center always retains the event.

### Cameras and Incident evidence

Cameras are optional. A configured source supports live preview and snapshots
around Incidents and completion. Camera absence never blocks monitoring,
slicing, or dispatch.

Unpinned snapshots default to 30-day retention with a configurable time and
disk cap. Pinned evidence survives automatic pruning. Removing media never
removes the textual Incident or Job event.

## History

Queue provides searchable completed, failed, cancelled, and archived Job
history. Each Job preserves:

- Immutable event timeline.
- Slice Revision and Printer Profile snapshot.
- Printer identity snapshot even after archival.
- Spool reservations, movements, deductions, and corrections.
- Operator actions.
- Linked Incidents and surviving evidence.

History uses snapshots for facts that must not change retroactively while
retaining links to current objects where useful.

## Settings and local data

Settings is a full workspace reached from the activity rail, superseding the
earlier theme-only gear menu once v1 has settings beyond appearance. Categories
are:

- General.
- Appearance.
- Slicing.
- Notifications and retention.
- Storage and backup.
- Connections.
- Diagnostics.
- About farm3d.

Changes save immediately unless an operation requires validation or explicit
commit, such as moving storage or restoring a backup.

### Backup and restore

A backup contains a manifest and selectable settings, Printers, Projects,
managed Models, Slice Revisions, Spools, Jobs, Incidents, and snapshots. Secrets
are excluded; credential references are documented as requiring re-entry on a
new machine.

Restore previews counts and conflicts before writing and creates a safety backup
of existing data first. It never silently replaces the current Farm.

### Diagnostics and reset

Diagnostics export is selectable and redacts credentials. It includes versions,
adapter health, relevant logs, and configuration summaries. Destructive reset
names every affected data class and requires typed confirmation.

## Required states and recovery

### First run

Monitor explains a direct three-step path:

1. Add one or more Printers.
2. Add Spools or skip inventory until needed.
3. Import a Model.

Each action enters the real workflow; there is no separate onboarding tour.

### Loading and stale data

After initial startup, retain last-known content with freshness labels during
refresh. Do not replace the Farm with a blocking spinner. On restart, load
persisted state immediately, mark live fields stale, restart Connection
supervisors, and reconcile queued assignments before automation resumes.

### Empty and filtered-empty

Distinguish "nothing exists" from "nothing matches this filter." Preserve the
filter and provide one precise recovery action.

### Errors and partial failure

- Corrupt local data retains the existing quarantine behavior and links to
  recovery details.
- Failed slicing preserves choices and source state for retry.
- Partial batch setup commits valid independent records and retains failed rows.
- Connection loss never erases last-known Printer status.
- Backup/restore and storage operations are transactional or recoverable.

## Window adaptation

farm3d remains a desktop-first tool. It does not turn into a mobile layout, but
it must remain operable in compact desktop windows.

- At 1440 × 900 and above, show the activity rail, primary workspace, and right
  dock.
- At 1024 × 700, the right dock is an overlay and all primary workflows remain
  operable without clipped controls.
- Printer cards become compact rows before losing operational fields.
- Tables retain column meaning and may scroll horizontally rather than
  collapsing unrelated values into ambiguous cards.
- The activity rail and current primary action remain reachable.
- The Model viewport yields space to settings through explicit panel toggles,
  not uncontrolled shrinking.

## Accessibility and interaction

- Kobalte primitives back dialogs, menus, tabs, forms, popovers, tooltips, and
  other interactive controls.
- All hover disclosures, including Printer-count rosters, are keyboard
  focusable.
- Queue reordering has pointer and keyboard equivalents.
- File drop surfaces include a keyboard-operable file picker.
- Viewport transforms have numeric fields and keyboard commands; pointer-only
  direct manipulation is never required.
- Focus order follows activity rail, toolbar, primary content, dock, status bar.
- Existing design-system focus rings remain visible.
- Severity uses shape, label, and color.
- Reduced-motion preference disables nonessential transitions.
- Camera content always has textual state and timestamp.
- Destructive actions never depend on icon-only meaning.

## Design-system additions

Extend the existing system only when repeated product needs justify it. The
focused specs must evaluate these shared additions:

- Data table with keyboard row selection and column overflow behavior.
- Bounded roster popover for aggregate counts.
- Severity marker for fatal, warning, informational, and resolved states.
- Split-pane/dock behavior with persisted width and compact overlay mode.
- Tree/list navigation for Projects and settings categories.
- File drop/import surface.
- Background-operation progress with cancel and expandable logs.
- Timeline/event-list pattern.
- Confirmed queue reordering control with keyboard support.

Every new design-system component must follow `DESIGN.md`, be exported through
the component index, and appear in the Showcase.

## Acceptance criteria

The complete v1 design is satisfied when an operator can:

1. Add one Printer or batch-add physically identical Printers across multiple
   bays without re-entering shared configuration.
2. Identify normal, stale, blocked, alerting, and fatal Printer states from
   Monitor without opening every Printer.
3. Inspect any aggregate Printer count by pointer or keyboard.
4. Configure Material Slots and filter the orthogonal measured, estimated,
   reserved, loaded, and low facets of active, empty, or archived Spools.
5. Import managed or linked STL, 3MF, and G-code, recover missing links, and
   organize Models into Projects.
6. Prepare plates, create an immutable Slice Revision, and add one or many Queue
   Entries.
7. Understand why a Queue Entry is eligible or blocked and exactly what
   automatic dispatch will do next.
8. Require operator start confirmation by default while optionally permitting
   unattended starts per Printer.
9. Monitor and recover an active Job using telemetry, camera evidence where
   available, and explicit control actions.
10. Account for estimated material after every Job and reconcile it by weight
    when desired or required, without confusing reservation with consumption.
11. Follow Attention Events from desktop notification to source, distinguish
    reading from acknowledgment and resolution, and retain textual history after
    media pruning.
12. Archive Printers without losing historical identity.
13. Back up, preview-restore, diagnose, and deliberately reset local data.
14. Complete every workflow with keyboard access at both 1440 × 900 and
    1024 × 700 desktop viewport sizes.

## Implementation decomposition

Do not create one implementation plan from this document. Produce focused specs
and plans in dependency order:

1. **Shell, Monitor, and operational status vocabulary.** Establish the new
   navigation, reusable dock, card/row modes, inspectable counts, severity, and
   stale-state behavior over current Printer data.
2. **Printer lifecycle and batch setup.** Deepen current add/edit flows,
   archive semantics, safety rule, and explicit batch operations without yet
   promising material or camera integrations.
3. **Spools and Material Slots.** Add inventory, slot capabilities, movements,
   measured/estimated confidence, and correction history.
4. **Library persistence and Projects.** Define managed/linked storage, file
   parsing boundaries, source watching, duplicate handling, and missing-link
   recovery.
5. **Runtime slicing and Slice Revisions.** Implement Model Source Revisions,
   plate preparation,
   operational settings, OrcaSlicer process management, validation, immutable
   artifacts, and estimates.
6. **Connection command capabilities.** Research and implement upload, start,
   pause, resume, cancel, host-state reconciliation, and camera capabilities per
   adapter. ElegooLink remains blocked on its real-hardware protocol spike.
7. **Queue Entries, Jobs, and dispatch.** Define persisted transitions,
   eligibility, Spool reservations, safety gates, idempotent command handoff,
   retry, cancellation, and restart reconciliation.
8. **Attention, Incidents, cameras, and desktop notifications.** Add event
   lifecycle, deduplication, snapshots, retention, and deep links.
9. **History, backup/restore, and diagnostics.** Complete immutable timelines,
   conflict-aware portability, redaction, disk management, and destructive
   recovery.

Each focused spec must re-check its assumptions against the protocols and data
available at that point. In particular, Job control and camera behavior cannot
be assumed uniform across Moonraker, OctoPrint, and ElegooLink.
