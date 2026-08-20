# Printer adapters: OctoPrint and ElegooLink

## Status

Architecture-level — settled framing and an explicit spike gate ahead of an
implementation plan for either adapter. Deliberately not detailed further:
the whole point of sequencing this after phase 2
(`2026-08-20-printer-connections-design.md`) is to let a real, working
Moonraker adapter validate the `PrinterConnection` trait before more code is
written against it.

## Context

Phase 2 proves `PrinterConnection` against one protocol (Moonraker). This
phase adds two more adapters to reach parity with what the catalog already
declares is out there: of the machine presets in farm3d's printer catalog
(`2026-08-20-printer-catalog-and-printers-design.md`), `host_type` breaks down
roughly as OctoPrint the largest single group, ElegooLink covering the
Centauri Carbon line this project started from, and Moonraker-compatible
firmware elsewhere. OctoPrint and ElegooLink are the two that matter most
after Moonraker.

## Settled decisions

### Scope: exactly these two adapters, nothing else

- **OctoPrint** — plain REST with an `X-Api-Key` header, well-documented,
  the single largest `host_type` in the catalog. It's also the adapter most
  likely to stress-test whether phase 2's trait design holds up: it's the one
  protocol here that genuinely polls rather than pushing. Implementing
  `subscribe()` for OctoPrint means driving an internal interval and feeding
  the same `mpsc::Sender<PrinterStatus>` a WebSocket-based adapter would — if
  that doesn't fit cleanly, the trait needs to change, and finding that out
  here (rather than by guessing during phase 2) is the reason this phase comes
  second, not first.
- **ElegooLink** — Elegoo's SDCP protocol over WebSocket, covering the
  Centauri Carbon line. The least-documented of the three protocols in this
  arc.

Serial/USB (ADR-0002's other Connection kind) stays out of scope across all
three phases — it shares almost nothing with the network adapters and isn't
needed for any printer in this farm.

### ElegooLink requires a protocol spike before an implementation plan

Because SDCP is the least-documented of the three, **do not write an
implementation plan for the ElegooLink adapter directly from this spec.**
Run a spike first: connect to a real Elegoo printer, capture the actual
WebSocket message shapes for identity, status, and temperature reporting, and
confirm what farm3d's `ProbeResult`/`PrinterStatus` types (defined in phase 2)
need to represent. Planning against assumed message shapes for an
undocumented protocol is how phase 2's carefully-proven trait ends up
reshaped by guesswork instead of by evidence — the opposite of why this
project is sequenced the way it is.

**Specifically worth checking in that spike:** the Centauri Carbon catalog
variants declare `gcode_flavor: "klipper"`. If genuine Klipper firmware is
running behind Elegoo's proprietary control surface, the Moonraker adapter
built in phase 2 may partially apply — either as a fallback path or as
evidence that ElegooLink is a thin proprietary wrapper worth understanding on
its own terms rather than reimplementing Moonraker-shaped logic for. Either
finding could meaningfully change this phase's scope, which is exactly why
it's worth confirming before scoping the implementation plan rather than
after.

### Expected trait reshaping is not a phase-2 failure

If OctoPrint's polling behavior or ElegooLink's SDCP shape forces a change to
`PrinterConnection`, `ProbeResult`, or `PrinterStatus`, that's this phase
doing its job — validating the trait against real protocols rather than one.
Record what changed and why in this phase's implementation plan; don't treat
it as a defect in the phase 2 spec.

## Out of scope

- A fourth adapter, or any protocol beyond OctoPrint and ElegooLink.
- Serial/USB.
- Anything already out of scope in phase 2 (Job dispatch, Slicing
  integration).
