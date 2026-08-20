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

**Printer Profile**:
The physical and material capabilities of a Printer (build volume, nozzle
size, material) that farm3d checks a Slice or Job's compatibility against.
_Avoid_: Machine config, printer settings

**Connection**:
The channel farm3d uses to communicate with a Printer — either a network
print-server API (OctoPrint/Moonraker/Bambu-style) or a direct USB/serial
link.
_Avoid_: Link, interface

**Model**:
A 3D file (e.g. STL/3MF) representing an object that can be sliced and
printed.
_Avoid_: File, part, design

**Library**:
The curated set of Models a user has explicitly saved in farm3d, distinct
from a Model merely opened for inspection.
_Avoid_: Collection, catalog

**Slice**:
The act of converting a Model into printable G-code for a specific Printer
Profile, performed by farm3d's wrapped OrcaSlicer engine.
_Avoid_: Export

**Job**:
A Slice dispatched to a specific Printer for printing, tracked by farm3d
from dispatch through completion.
_Avoid_: Print, task
