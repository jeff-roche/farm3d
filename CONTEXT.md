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
link. Moonraker is the only adapter implemented today; OctoPrint and
ElegooLink are planned (see ADR-0002).
_Avoid_: Link, interface

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
A request to produce one physical run from a Slice Revision that has not yet
been assigned to a Printer. It carries queue order and a Dispatch Policy.
_Avoid_: Job, task, order

**Job**:
A Slice Revision assigned to a specific Printer for printing, tracked by
farm3d from assignment through completion (ADR-0005). Not yet implemented —
no Job dispatch/tracking code exists yet.
_Avoid_: Print, task

**Dispatch Policy**:
A Queue Entry's rule for becoming a Job: operator-selected,
farm3d-recommended, or automatically assigned. A Printer's own start-safety
rule remains a separate gate after assignment.
_Avoid_: Queue mode, automation level

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
A notable operational occurrence tied to a Printer or Job, preserving what
happened and any available evidence or operator action.
_Avoid_: Notification, log entry

**Attention Event**:
An operator-facing signal about a current or historical condition. Reading,
acknowledging, and resolving an Attention Event are distinct states.
_Avoid_: Toast, notification
