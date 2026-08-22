# Printer adapters: OctoPrint and ElegooLink

## Status

**OctoPrint: Plan-ready.** Its wire contract is resolved below against
OctoPrint's published REST API docs, and `reqwest` 0.12 (`features =
["json"]`) has been compile-verified against this repo's existing
dependency set — including a plain `#[test]` (no async runtime, no
dev-dependencies) building a `Request` with an `X-Api-Key` header via
`RequestBuilder::build()`, matching the pattern phase 2 established for
`upgrade_request`. The implementation plan is
`docs/superpowers/plans/2026-08-21-octoprint-adapter.md`.

**ElegooLink: still Architecture-level**, gated on the protocol spike below —
do not write an implementation plan for it from this spec.

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

## OctoPrint wire contract

*Resolved from OctoPrint's published REST API documentation
(docs.octoprint.org), not from memory — see "Verification performed" below.*

### Authentication

Every endpoint below takes the API key as an `X-Api-Key` header, exactly
Moonraker's scheme. A missing or invalid key gets **403 Forbidden** on any
endpoint but `POST /api/login` — unlike Moonraker, OctoPrint does not use 401
for this, though this adapter classifies both as `ConnectionError::Auth`
defensively, matching Moonraker's `classify()`.

### The three calls `probe()` uses

| Call | Returns what farm3d needs |
|---|---|
| `GET /api/version` | `server` — OctoPrint's own version string |
| `GET /api/connection` | `current.state` (e.g. `"Operational"`, `"Closed"`), `current.printerProfile` (the active profile's id) |
| `GET /api/printerprofiles` | `profiles.<id>.name`, `profiles.<id>.volume.{width,depth,height}` |

None of these three ever return a non-2xx for a reachable, authenticated
OctoPrint instance regardless of whether a printer is connected to it —
unlike `GET /api/printer` (see below), which is why `probe()` uses these
three and not that one. The active profile is looked up from
`/api/printerprofiles`' `profiles` map by the id `/api/connection` reports,
rather than calling the single-profile endpoint
(`GET /api/printerprofiles/<id>`) — that endpoint's response shape is **not**
shown in OctoPrint's docs (only prose: "Returns a 200 OK with a profile"),
while the list endpoint's `{"profiles": {"<id>": {...}}}` shape is fully
documented with an example. Preferring the verified shape over the
plausible-but-unconfirmed one follows the same discipline phase 2 applied to
Moonraker.

**OctoPrint has no field for the underlying printer's firmware version** —
unlike Moonraker, which reports Klipper's `software_version` directly.
`ProbeResult.firmware` is left as `""` for this adapter rather than guessed
at. `reported_name` uses the printer profile's `name` (e.g. a user-renamed
"Voron 2.4") — the closest identity field OctoPrint exposes, and one a user
is likely to have set meaningfully. `state_message` is left as `""`:
`current.state` is already a self-describing string (OctoPrint folds error
detail into it, e.g. `"Error: ..."`), so there is no separate message to add
the way Moonraker's `printer.info.state_message` supplies one.

### The two calls `subscribe()` polls

Unlike Moonraker's push-based WebSocket, OctoPrint drives its own interval —
this is the trait-validating case the spec called out.

| Call | Returns what farm3d needs |
|---|---|
| `GET /api/job` | `state` (free text, includes `"Offline"`/`"Offline after error"` and `"Error: ..."` variants), `job.file.name`, `progress.completion`, `progress.printTime` |
| `GET /api/printer?exclude=sd` | `temperature.tool0.{actual,target}`, `temperature.bed.{actual,target}` (the `bed` key is absent entirely on a printer profile with no heated bed) |

**The one behavior that must not be gotten wrong:** `progress.completion` is
documented (data model page, not the possibly-stale endpoint-page example) as
a **percentage of completion**, i.e. `0`–`100` — not the `0.0..=1.0` fraction
farm3d's `PrinterStatus.progress` contract uses. The adapter must divide by
100. Getting this backwards renders a progress bar 100× too full for any
print past 1% complete, and would pass a casual glance at low-completion
values. This is OctoPrint's equivalent of Moonraker's partial-update-merge
risk and earns the same dedicated test.

**`GET /api/printer` returns 409 Conflict when the underlying printer is not
connected to OctoPrint** (not authenticated, not unreachable — OctoPrint
itself is fine, there is simply no printer attached over serial right now).
This is the direct equivalent of Moonraker's "Klippy down behind a healthy
Moonraker socket" case: the poll must treat a 409 here as "no temperature
reading this tick", not as a `ConnectionError`. `GET /api/job` never returns
409 under any documented circumstance and reports `"Offline"` in its own
`state` field when nothing is connected, which is what this adapter uses to
derive `ConnectionState` — `/api/printer`'s 409 therefore never needs to
drive connection state itself, only whether a temperature reading exists for
that tick.

There is no clean enum for `state`'s values the way Moonraker's
`klippy_state` behaves; the mapping to `ConnectionState` is prefix-based:
`"Offline"` (and its `"Offline after error"` variant) → `Offline`, an
`"Error…"` prefix → `Error`, anything else (`"Operational"`, `"Printing"`,
`"Paused"`, `"Cancelling"`, …) → `Online`. `PrinterStatus.job_state` passes
`/api/job`'s raw string through unchanged, exactly as Moonraker passes
`print_stats.state` through unchanged — neither is normalized to a shared
vocabulary, and nothing in the frontend pattern-matches on it today.

**Poll interval: 2 seconds.** Chosen to keep OctoPrint's Flask-based server
comfortably below Moonraker's own push cadence while still reading as live
on the dashboard; there is no documented rate limit to size against instead.

## Out of scope

- A fourth adapter, or any protocol beyond OctoPrint and ElegooLink.
- Serial/USB.
- Anything already out of scope in phase 2 (Job dispatch, Slicing
  integration).

## Verification performed while firming up the OctoPrint contract

- **Wire shapes** read from OctoPrint's REST API reference (`/api/version`,
  `/api/printer`, `/api/job`, `/api/connection`, `/api/printerprofiles`) and
  its data model appendix — the `progress.completion` percentage-vs-fraction
  question specifically resolved against the data model page's field
  description rather than the endpoint page's example value, which is
  ambiguous read alone.
- **`reqwest` 0.12 (`features = ["json"]`) compile-verified** in a scratch
  crate against this repo's exact existing `tokio` feature set
  (`sync`, `time`, `rt` — no `rt-multi-thread`, no `macros`): a `Client` was
  built, a GET request issued under `#[tokio::main(flavor = "current_thread")]`,
  and — in a separate scratch crate — a `Request` built and inspected
  (URL, `X-Api-Key` header) from a **plain, non-async `#[test]`**, confirming
  the adapter's URL/header-building helpers can be unit-tested the same way
  `moonraker::upgrade_request` is, with no dev-dependencies added.
- **Not verified:** farm3d has not yet spoken to a live OctoPrint instance.
  The contract above is documentation-derived, which is why the plan's
  end-to-end verification checklist against a real instance is not optional,
  and why `GET /api/printerprofiles`'s list shape was preferred over the
  single-profile endpoint's unconfirmed one.
