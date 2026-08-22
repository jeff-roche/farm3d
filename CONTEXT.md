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

**Printer Group**:
The Fleet Dashboard's clustering of Printers that share the same Printer
Model/Variant, shown as one card with multiple instances. Distinct from a
Printer's free-text `group` field (e.g. "Bay 1"), which is just a
user-assigned location label, not this clustering.
_Avoid_: Group (unqualified) — always say "Printer Group" or "location
group" for the two different senses.

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
A 3D file (e.g. STL/3MF) representing an object that can be sliced and
printed. Distinct from **Printer Model** above, which identifies a printer's
make in the catalog, not a printable file.
_Avoid_: File, part, design

**Library**:
The curated set of Models a user has explicitly saved in farm3d, distinct
from a Model merely opened for inspection.
_Avoid_: Collection, catalog

**Slice**:
The act of converting a Model into printable G-code for a specific Printer
Profile, performed by farm3d's wrapped OrcaSlicer engine (ADR-0003). Not yet
implemented — OrcaSlicer is currently only invoked at build time to generate
the printer catalog, not at runtime to slice a Model.
_Avoid_: Export

**Job**:
A Slice dispatched to a specific Printer for printing, tracked by farm3d
from dispatch through completion (ADR-0005). Not yet implemented — no Job
dispatch/tracking code exists yet.
_Avoid_: Print, task
