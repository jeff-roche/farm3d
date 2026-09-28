# farm3d

farm3d is a single-user desktop app for managing a 3D-printer print farm:
connecting to printers, slicing models, and organizing a library of
printable models.

## Language

**Farm**:
The complete set of Printers a single user manages within farm3d.
_Avoid_: Fleet, network

**Printer**:
A physical 3D printer farm3d connects to, monitors, and can dispatch print
Jobs to.
_Avoid_: Device, machine

**Location**:
An optional free-text label for where a Printer physically sits, such as
"Bay A" or "Rack 2". It is durable Printer data. Monitor Sections may group
by it, but it is not a grouping entity itself. Batch setup's "bays" are
Location values, not Material Slots.
_Avoid_: Group

**Profile-only Printer**:
A Printer saved with a resolved Printer Profile and no Connection. It is
valid, durable, and shown as Setup incomplete.

**Setup incomplete**:
The derived state of a Printer whose durable configuration cannot support
monitoring — for example, a Profile-only Printer, or one whose Connection
uses an unsupported adapter. It differs from Offline, which describes a
configured Connection farm3d cannot currently reach.

**Start-safety rule**:
A per-Printer rule for whether farm3d may start a Job on it unattended. The
default is to confirm the bed is clear.
_Avoid_: Auto-start setting

**Archive**:
The default way to retire a Printer. It keeps the Printer's identity and
history, and removes it from monitoring and scheduling.
_Avoid_: Delete, remove

**Monitor Section**:
A temporary visual grouping of individual Printer cards by location, Printer
Model, or operational state. It is a view preference, not persisted Farm data.
_Avoid_: Printer Group, Fleet Group

**Printer Profile**:
The physical and material capabilities of a Printer (build volume, nozzle
size, material) that farm3d checks a Slice or Job's compatibility against.
It is the *resolved* capability set — a Printer Model/Variant reference
merged with any of the user's own overrides.
_Avoid_: Machine config, printer settings

**Printer Model**:
A vendor product line entry in farm3d's bundled printer catalog (e.g.
"Elegoo Centauri Carbon"), sourced from OrcaSlicer's printer presets. A
Printer's `catalogRef` points at one Printer Model. Distinct from **Model**
below, which is a 3D file to be printed.
_Avoid_: Catalog entry, preset

**Printer Variant**:
A specific nozzle-diameter configuration of a Printer Model (e.g. its
"0.4mm nozzle" variant) in the catalog. farm3d resolves a Printer's
`catalogRef` down to one Printer Variant to produce its Printer Profile.
_Avoid_: Preset, config

**Connection**:
The channel farm3d uses to communicate with a Printer — either a network
print-server API (Moonraker/OctoPrint/ElegooLink-style) or a direct USB/serial
link. Moonraker is implemented with monitoring and the P6 command
Capabilities (upload, start, pause, resume, cancel, host state, artifact
identity, camera query). OctoPrint
is implemented for status-only monitoring. ElegooLink is planned (see
ADR-0002). What a Connection can do is described by its Capabilities
(ADR-0011).
_Avoid_: Link, interface

**Capability**:
One thing farm3d can do through a Printer's Connection: upload, start,
pause, resume, cancel, host state, artifact identity, or camera. Each is
either supported, with the evidence behind it, or unsupported, with a
reason: the adapter can't, it isn't verified yet, or this printer lacks
it. Unsupported is not the same as failed.
_Avoid_: Feature, permission

**Host Operation**:
One write farm3d sends to a Printer's host (upload, start, pause, resume,
or cancel). farm3d records it durably before the first byte leaves, and
keeps the outcome it has proved. A Printer has at most one unresolved
Host Operation at a time, and while it has one, the Printer can't be
archived, deleted, or moved to another endpoint, and no Printer import can
run. Its states are `dispatching`, `uncertain`, and `reconciling`
(unresolved), then `succeeded`, `failed`, or `abandoned` (terminal).
Terminal rows are history: deleting a Printer, or replacing it through a
Printer import, deletes them with it.
_Avoid_: Job (that is P7), command, request

**Staged artifact**:
A Slice Revision's G-code that a succeeded upload Host Operation put on a
Printer's host, and whose bytes farm3d read back and verified. Staging
never starts a print; starting one is a separate Host Operation.
_Avoid_: Uploaded file, remote copy

**Uncertain outcome**:
The state of a Host Operation whose effect farm3d could not prove either
way, for example because the printer's answer was lost. farm3d keeps
checking the printer by reading from it, and never repeats the write by
itself.
_Avoid_: Failed, unknown error

**Reconciliation**:
farm3d reading a Printer's host to settle an uncertain Host Operation: is
the staged file there with the right bytes, did the print start, did the
pause take effect. While a check runs the operation is `reconciling`; it
then ends `succeeded` or `failed`, or goes back to `uncertain` when the
host can't yet say. It runs after a reconnect, after a restart, on a
backoff, and when the operator asks (Check again). It only ever reads.
_Avoid_: Retry, resync

**Abandon reconciliation**:
The operator's explicit, recorded decision to stop checking an uncertain
Host Operation, typically for a printer that is gone for good. The
printer's state stays unknown; abandoning is not success, but it releases
the Printer for other changes.
_Avoid_: Dismiss, clear, retry

**Connection Supervisor**:
The backend component that owns a Printer's Connection lifecycle — opening
it, watching it, and reconnecting with backoff when it drops.
_Avoid_: Watchdog, poller

**Credential**:
A secret (e.g. an API key) a Connection needs to authenticate to a Printer,
kept out of plain settings storage in a dedicated credential store and
referenced from a Printer's config by an opaque `credentialRef`.
_Avoid_: Password, token, secret

**Discovery**:
Bounded, best-effort scanning (currently mDNS) for print-server hosts on the
local network, surfaced to the user as candidate Printers to connect rather
than auto-added.
_Avoid_: Scan, auto-detect

**Model**:
A 3D file (STL/3MF) *or a pre-sliced G-code file* kept in the Library. A
G-code Model is inspectable and retained, but it is never sliced, and it
becomes dispatchable work only as an external Slice Revision. Distinct from
**Printer Model** above, which identifies a printer's make in the catalog,
not a printable file.
_Avoid_: File, part, design

**Library**:
The curated set of Models a user has explicitly saved in farm3d, distinct
from a Model merely opened for inspection.
_Avoid_: Collection, catalog

**Project**:
An organizational grouping of Models in the Library. A Model may belong
to any number of Projects, or none (Unfiled). A Project does not carry
production quantities, deadlines, or fulfillment state.
_Avoid_: Order, batch, job folder

**Unfiled**:
The state of a Model that belongs to no Project. It is not a Project.

**Managed Model**:
A Model whose source is farm3d's own stored copy. It changes only when the
user adds a revision.

**Linked Model**:
A Model that follows a file path outside farm3d. farm3d watches it and
captures a new Model Source Revision whenever its content changes.

**Source state**:
A Linked Model's current relationship to its file: `ok`, `missing`,
`unreadable`, `notAFile`, `invalidContent`, or `changing`. It never affects
existing revisions.

**Import selection**:
A short-lived, Rust-held set of files the user picked or dropped. It is not
persisted.

**Slice**:
The act of converting one Plate of a Model's Preparation into printable
G-code for a target Printer Profile. A user-installed OrcaSlicer does it,
run as a separate process (ADR-0003, ADR-0009). Each Plate is sliced by its
own operation, which can be cancelled.
_Avoid_: Export

**Slice Revision**:
An immutable result: the G-code bytes plus the facts needed to judge where
they may be printed. A **farm3d Slice Revision** comes from one Plate of one
Model Source Revision and records the Printer Profile, presets, slicing
choices, and slicer runtime it was made with. An External Slice Revision
wraps an imported G-code Model instead. A Job dispatches one Slice Revision
without changing it.
_Avoid_: G-code version, export

**External Slice Revision**:
A Slice Revision that wraps an imported G-code Model Source Revision. farm3d
did not slice it, so it has no Plate and no slicer runtime, and each of its
compatibility facts is either a Confirmed fact or absent. A G-code file's own
claims are shown, but never become facts. When any fact is absent, the
revision needs a Printer chosen by hand when it is queued.

**Confirmed fact**:
A compatibility fact on an External Slice Revision (Printer Profile, nozzle
diameter, material, or filament diameter) that the operator entered or
explicitly accepted from the file. Its opposite is an **absent fact**: one
the operator left empty.
_Avoid_: Inferred fact, detected value

**Preparation**:
A Model's editable, persisted draft for slicing. It holds the Plates, where
each object sits on them, the target Printer Profile, and the slicing
choices, and it is pinned to one Model Source Revision. A Model has at most
one. It is not a Slice Revision.
_Avoid_: Project (that means a Library grouping), job setup

**Plate**:
One build plate within a Preparation, with a stable identity, a name, and
an order. Each farm3d Slice Revision records the Plate it came from.
_Avoid_: Bed (the physical surface), tray

**Slicer runtime**:
The OrcaSlicer engine farm3d runs, plus the Preset source it takes presets
from. The user installs OrcaSlicer; farm3d finds it or is pointed at it,
and never bundles one (ADR-0009).
_Avoid_: Slicer install, engine config

**Preset source**:
An OrcaSlicer installation whose `resources/profiles` holds JSON presets.
It defaults to the engine itself. A build that ships only binary preset
caches needs another installation as its Preset source.

**Model Source Revision**:
An immutable snapshot of a Model's source content at import or linked-source
change time. Slice Revisions refer to one Model Source Revision so later file
changes cannot alter existing work.
_Avoid_: File version, Model version

**Queue Entry**:
One requested physical run of one Slice Revision. It has a queue position,
a Dispatch Policy, a Dispatch preference, and a material estimate fixed
when it is created. It is `queued`, `assigned` (it has a Job), or `closed`
(completed, failed, cancelled, released, or removed). Whether it can run
now is derived each time, never stored. It becomes a Job only by
assignment.
_Avoid_: Job, task, order

**Job**:
One Queue Entry assigned to one Printer with one reserved Spool, tracked
by farm3d from assignment to its end (ADR-0005, ADR-0013). It is staged,
started, and controlled only through Host Operations. Its end is proved
from the host's print history for the print farm3d started, never from a
status string alone. At most one Job is active on a Printer at a time.
_Avoid_: Print, task

**Dispatch Policy**:
A Queue Entry's rule for becoming a Job: Manual (the operator picks the
Printer and Spool), Recommended (farm3d ranks and explains, the operator
confirms), or Automatic (farm3d assigns the first qualifying Printer, and
only through a simulator-proven Connection). A Printer's Start-safety rule
remains a separate gate after assignment.
_Avoid_: Queue mode, automation level

**Dispatch preference**:
How a Queue Entry ranks qualifying Printers: `loadedFirst` (a Printer that
already has a matching Spool loaded first; the default) or
`leastRecentlyUsed`. Ties go to Printer name, ignoring case, then to the
Printer's id.
_Avoid_: Priority (queue order is the only priority)

**Lineage**:
The group of Queue Entries one Add to Queue created (copies 1..N), plus
their retries and release replacements. It only groups them: each entry
is still assigned, cancelled, retried, and settled on its own.
_Avoid_: Batch, order

**Settlement**:
How a Job's reserved material becomes a deduction. A completed Job
deducts its estimate by itself. A failed or cancelled one is settled by
the operator: the estimate scaled by how far it printed, a measured
weight, or deferred. Until it is settled, the amount stays unavailable,
and the Job has an open material Reconciliation Requirement.
_Avoid_: Deduction (settling can also defer); Host Operation
reconciliation, which only checks the printer and never touches material

**Reconciliation Requirement**:
A durable record, with a stable id, of something about a Job the operator
must settle: its material after it failed or was cancelled, or its
outcome when farm3d couldn't prove it. It stays until it is resolved.
Deferring keeps it open.
_Avoid_: Alert, notification, Attention Event (P8 projects each open one
into an Attention Event; the requirement stays the durable record, and
resolving it resolves that Event)

**Awaiting material**:
A Job that is staged and waiting to start, but whose Spool isn't loaded
on its Printer. It is a reason the Job can't start yet, not a separate
state.

**Outcome unknown**:
A Job whose end farm3d could not prove: its start was abandoned, or the
host's history stopped showing its print. farm3d stops checking, and the
operator declares whether it completed, failed, or was cancelled. The
operator may also declare the end of a printing Job whose printer farm3d
hasn't reached for 30 minutes.
_Avoid_: Failed, lost

**Spool**:
A physical supply of printable material, with an identity, location, and
measured or estimated amount remaining. Reserved material is still part of
the Spool until a Job consumes it.
_Avoid_: Filament profile, material preset

**Spool number**:
A small sequential integer shown as `#12`, meant for writing on the physical
Spool. The stable id stays internal.

**Tare**:
A reusable, named empty-spool weight (for example "Polymaker cardboard
1 kg"). Subtracting it from a scale reading gives the net amount.

**Storage label**:
An optional free-text name for where a Spool sits when it is not in a
Material Slot (for example "Dry box 2"). Like Printer Location, it is a
label, not an entity.

**Material Slot**:
A named physical position on a Printer or attached feeder that can hold one
Spool. A Printer has one or more Material Slots. Layouts are user-configured;
there is no automatic AMS topology.
_Avoid_: Bay, feeder (unless naming the hardware)

**Incident**:
The record of one failure on one Printer, optionally tied to one Job (a
failed or host-cancelled Job, a Job whose outcome is unknown, or a
printer-reported failure with no farm3d Job). It keeps a copy of the
Printer's identity, an append-only timeline, notes, and camera evidence.
It is `open` until every actionable Attention Event linked to it
resolves, then `closed`. A Job has at most one Incident. A Printer with
Incident history can be archived but not deleted.
_Avoid_: Notification, log entry

**Condition**:
A current fact that needs the operator's attention, such as a Printer
being offline, a Job waiting for its start confirmation, or a Spool
running low. farm3d computes Conditions from normalized state each time;
it never stores one. Each has a stable dedup key made of its kind and its
source, so the same fact always maps to the same open Attention Event
(ADR-0014). A fact farm3d can't observe yet (a Printer's status right
after startup) is unknown, and unknown never opens or resolves anything.
_Avoid_: Alert, event, trigger

**Attention Event**:
The durable record of one occurrence of a Condition, with a severity
(fatal, warning, or info), a source, and whether it needs action. Its
three dimensions are separate: unread or read, unacknowledged or
acknowledged, open or resolved. Acknowledging or resolving also marks it
read, and resolved is final. An acknowledged Event stays open and
actionable until it resolves. It resolves when its Condition clears, when
its action completes, or, for a failed or host-cancelled Job, when the
operator resolves it. While open, repeated observations amend it rather
than add Events. If a Printer Condition, a low Spool, or a Job's start
confirmation returns after resolution, a new Event is created and linked
to the previous one (a recurrence). A Job's end and a Reconciliation
Requirement get one Event each and never recur.
_Avoid_: Toast, notification, alert

**Snapshot**:
One camera frame farm3d captured and stored: when an Incident opened,
when a Job completed, or by hand. It may be pinned, which keeps it from
automatic pruning. Pruning removes its image but keeps its record, so the
textual history stays. Live preview frames and Setup test frames are
never Snapshots.
_Avoid_: Photo, frame (for a stored capture); Printer snapshot (a Job's or
Incident's copy of a Printer's identity)

**Camera Source**:
A Printer's optional camera configuration: a webcam its Moonraker host
reports, chosen by name, or a manual plain-HTTP snapshot URL. farm3d
fetches every frame itself, and never stores a host webcam's URL. A
missing or failing camera never blocks monitoring, slicing, assignment,
or Job control. Its preview health is runtime state, not part of the
Camera Source.
_Avoid_: Camera stream, webcam config

**Alert defaults**:
A Printer's own attention settings: how long it must be unreachable
before it counts as offline (off, 1, 5, or 15 minutes), whether its
notifications follow the global classes or are muted, and whether farm3d
captures a Snapshot when an Incident opens or a Job completes.
_Avoid_: Notification settings (those are the global classes)
