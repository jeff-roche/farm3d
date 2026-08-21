# Printer connections: trait, credentials, discovery, and Moonraker

## Status

**Plan-ready.** Originally written architecture-level with one open question
(the wire contract). That question is now resolved against Moonraker's
published API and every new dependency has been compile-verified together —
see "Wire contract" and "Verification performed while firming up this spec"
below. The implementation plan is
`docs/superpowers/plans/2026-08-20-printer-connections-and-moonraker.md`.

## Context

Phase 1 (`2026-08-20-printer-catalog-and-printers-design.md`) gives every
Printer a resolved capability Profile but leaves `connection: null` always —
nothing is ever actually contacted, and `PrinterDashboard`'s status is
permanently mock. This phase makes one Connection kind real end to end:
config UI, credential storage, discovery, and live status, proved against
Moonraker before two more adapters are built on the same trait in
`2026-08-20-printer-adapters-design.md`.

**Why this phase is *not* split into "config + test-connection" then "live
status" as two separate phases:** for a push-based protocol like Moonraker or
ElegooLink, connecting and receiving status are nearly the same code. A phase
that shipped a one-shot "test connection" alone would build a connection path
that gets rewritten weeks later when live status lands. Better to prove the
real shape once.

**Why this phase is *not* split horizontally across all three protocols
instead:** building `PrinterConnection` speculatively against three protocols
before any of them work end-to-end risks a trait shaped by guesswork. One real
adapter first validates the trait; phase 3 either confirms it or reshapes it
against the second and third implementations — that reshaping is the expected
outcome of phase 3, not a sign phase 2 got it wrong.

## Settled decisions

### New dependencies

All versions below were confirmed against crates.io and **compile-verified
together in a scratch crate** while firming up this spec — not merely looked
up:

| Crate | Version | For |
|---|---|---|
| `tokio` | 1.53 | async runtime — the first in this repo; `src-tauri` has none today |
| `tokio-tungstenite` | 0.30 | Moonraker's WebSocket JSON-RPC |
| `async-trait` | 0.1 | `PrinterConnection`'s async methods |
| `futures-util` | 0.3 | `SinkExt`/`StreamExt` over the WebSocket |
| `keyring` | 4.1 | OS secret store |
| `mdns-sd` | 0.21 | `_moonraker._tcp` / `_octoprint._tcp` browsing |

This is a significant step for a crate that currently has four dependencies
(`tauri`, `tauri-plugin-opener`, `serde`, `serde_json`) and zero async runtime.
Adding `tokio` means Tauri's async command support plus a managed
connection-supervisor task — this phase is where `src-tauri` stops being a
thin file-IO shim and becomes a long-running process with background state.
The implementation plan should call this out explicitly rather than let it
arrive as a side effect.

**`reqwest` is deferred to phase 3**, revising this spec's original
dependency list. Moonraker exposes the same JSON-RPC surface over both HTTP
and its WebSocket, so `probe()` can run over the *same* WebSocket connect path
`subscribe()` needs — which is this phase's own stated rationale for not
splitting "test connection" from "live status" into separate phases. Adding an
HTTP client to satisfy a probe that the WebSocket already serves would
contradict that reasoning and enlarge an already-significant dependency step.
OctoPrint genuinely needs REST; `reqwest` arrives with it.

**`keyring` v4 needs no explicit feature flags.** Its default feature (`v1`)
selects Keychain Services on macOS, Credential Manager on Windows, and
Secret Service on \*nix, and re-exports a v1-compatible `Entry` API.

### Credentials: OS keychain, with a specified (not discovered) fallback

Secrets go to the platform secret store via `keyring`; `printers.json` holds
only a reference (`credentialRef: "farm3d/printer/<id>/apikey"`), never a
secret. This is what keeps `printers.json` safe to hand-edit, copy between
machines, and paste into a bug report — a property phase 1 deliberately
designed `printers.json` to have.

**The fallback must be specified up front, not discovered during
implementation.** On Linux, `keyring` needs a running Secret Service
(libsecret/gnome-keyring/KWallet); a headless or minimal box may have none.
Behavior:

1. Attempt the keychain via `keyring`. **`keyring::Entry::store_status()` is
   the exact API for this** — it lazily initializes the platform store once
   and returns `&'static Result<()>`, so availability can be reported without
   first writing a credential. `Error::NoDefaultStore` means no backend;
   `Error::NoEntry` means the backend works but holds nothing for this key.
2. On failure, fall back to a sibling `credentials.json` in the app config dir
   with `0600` permissions (created if missing, permissions checked/reset on
   every write), referenced the same way as the keychain path.
3. **Surface which store is active** in the Connection tab — "Credentials
   stored in: OS keychain" vs "Credentials stored in: credentials.json (no OS
   keychain available)" — rather than silently downgrading. A user should
   never be surprised where their API key lives.
4. Never write a secret into `printers.json` itself, under any fallback tier.

### The `PrinterConnection` trait

```rust
#[async_trait]
pub trait PrinterConnection: Send + Sync {
    /// One-shot reachability + identity check. Powers "Test connection".
    async fn probe(&self) -> Result<ProbeResult, ConnectionError>;
    /// Long-lived status stream. Implementations choose their native transport:
    /// WebSocket subscription for Moonraker/ElegooLink, HTTP polling for OctoPrint.
    async fn subscribe(&self, tx: mpsc::Sender<PrinterStatus>) -> Result<(), ConnectionError>;
}
```

`subscribe` returns a stream rather than exposing a `poll()` the runtime
ticks — this is the **per-adapter-native** decision: Moonraker and ElegooLink
push over WebSocket, OctoPrint will drive its own interval internally in
phase 3, and neither shape leaks into the supervisor. The supervisor (a
`tokio` task per connected Printer, managed from `setup()`) owns
reconnect-with-backoff, so no adapter implements retry logic itself.

`ProbeResult` carries what the Connection tab needs to confirm the right
machine was reached: firmware/host version, reported printer name, and a
capability readout to cross-check against the catalog Profile. A mismatch
between the catalog's build volume and what the host reports is worth
surfacing to the user, not silently ignoring — it likely means the wrong
catalog variant was picked in phase 1's add-printer flow.

### Discovery: mDNS, always alongside manual entry

Browse `_moonraker._tcp.local.` (and `_octoprint._tcp.local.`, ready for
phase 3) via `mdns-sd`, with a bounded browse window (a few seconds), offered
as suggestions in the Connection tab.

**Manual host/port entry is always available and never gated behind
discovery.** A farm of printers at static IPs — the case this whole project
started from — must be fully configurable without mDNS ever succeeding.
Discovery is an accelerator, not a requirement.

**Discovery returns a `Vec` from one async command rather than streaming
Tauri events**, revising this spec's original wording. The browse window is a
few seconds and the result set is a handful of hosts; a returned vector needs
no listener lifecycle, no de-duplication across events, and is directly unit
testable. Live status still uses events — it is genuinely open-ended, which
discovery is not.

**No Tauri capability change is required** — this corrects this spec's
original guess. Verified: `capabilities/default.json` already grants
`core:default`, whose permission set includes `core:event:default` (the
frontend's `listen`), and app-defined `#[tauri::command]`s registered through
`generate_handler!` are never gated by capability entries — as the phase-1
commands already demonstrate under this exact capability file. Rust-side
sockets are outside Tauri's ACL entirely. macOS local-network prompting is a
packaging-time entitlement concern, not a capability-file one, and is out of
scope until this project ships a macOS bundle.

### UI

The Connection tab in the resizable detail aside — the placeholder phase 1
puts there (`PrinterProfilePanel.tsx`'s sibling tab). Contents: a kind
selector defaulting from the catalog's `suggestedHostType`, host/port fields,
a credential field that writes through to the keychain (never displayed back
in plaintext once saved), discovered-printer suggestions from the mDNS browse,
and a **Test connection** button that calls `probe()` and renders the
`ProbeResult` (or the error).

Live status, once `subscribe()` is wired, lands on the existing card fields
(`nozzleTempC`, `bedTempC`, `currentJob`) via the `runtimeStatus?: PrinterStatus`
field that phase 1's `summarizePrinters` already leaves room for as
`undefined`. The status badge gains connection state (connecting / online /
offline / error) distinct from the print-job state it shows today.

## Wire contract

*This section resolves what was previously this spec's one open question.
Derived from Moonraker's published external API and Klipper's status
reference; see "Verification performed" below for what was checked and how.*

### Moonraker's actual shapes

Moonraker speaks JSON-RPC 2.0 over `ws://<host>:<port>/websocket`, with an
`X-Api-Key` header on the upgrade request when the instance requires auth
(trusted-client instances need none). The four calls this phase uses:

| Call | Returns what farm3d needs |
|---|---|
| `server.info` | `klippy_connected`, `klippy_state`, `moonraker_version` |
| `printer.info` | `hostname`, `software_version` (Klipper's), `state`, `state_message` |
| `printer.objects.query` | one-shot status snapshot |
| `printer.objects.subscribe` | same params as query; streams later changes |

Both object calls take `{"objects": {"<name>": null | ["field", …]}}`, where
`null` means all attributes. farm3d subscribes to exactly:

```json
{"objects": {
  "extruder":       ["temperature", "target"],
  "heater_bed":     ["temperature", "target"],
  "print_stats":    ["filename", "state", "print_duration", "message"],
  "display_status": ["progress"],
  "toolhead":       ["axis_minimum", "axis_maximum"]
}}
```

**`toolhead.axis_minimum`/`axis_maximum` are the build-volume readout** this
spec asks `ProbeResult` to carry for cross-checking against the catalog
Profile. Both are 4-element `[x, y, z, e]` arrays whose fourth element is
always zero and formally deprecated — read indices 0-2 only, never the
length.

### The one behavior that must not be gotten wrong

`notify_status_update` sends **partial** updates: only the fields that
changed, as positional params — an array of `[status_object, eventtime]`, not
a named-params object:

```json
{"jsonrpc": "2.0", "method": "notify_status_update",
 "params": [{"extruder": {"temperature": 201.4}}, 578243.578]}
```

An adapter that treats each notification as a whole snapshot will blank the
bed temperature every time the nozzle temperature ticks. **The adapter must
merge each notification into a running snapshot**, seeded by the initial
`printer.objects.subscribe` reply. This is the single highest-risk detail in
the phase and earns dedicated tests over a pure merge function with no I/O in
it.

`notify_klippy_ready`, `notify_klippy_shutdown`, and `notify_klippy_disconnected`
carry no `params` at all and drive `ConnectionState` independently of the
socket staying open — a live socket to a shut-down Klipper is *not* `online`.

### farm3d's types

`ProbeResult` — what "Test connection" renders:

```rust
pub struct ProbeResult {
    pub kind: String,             // "moonraker"
    pub host_software: String,    // server.info.moonraker_version
    pub firmware: String,         // printer.info.software_version (Klipper)
    pub reported_name: String,    // printer.info.hostname
    pub state: String,            // server.info.klippy_state
    pub state_message: String,    // printer.info.state_message
    pub reported: ReportedCapabilities,
}

/// All `Option` — an unhomed or shut-down Klipper reports no axis limits.
pub struct ReportedCapabilities {
    pub bed_width_mm: Option<f64>,
    pub bed_depth_mm: Option<f64>,
    pub printable_height_mm: Option<f64>,
}
```

The catalog cross-check this spec calls for is computed **in the frontend**,
not in Rust: the Connection tab already holds the `ResolvedPrinter` whose
`profile` carries the catalog's build volume, so comparing it against
`reported` needs no extra round trip and no duplicated catalog access.

`PrinterStatus` — every field `Option`, because a partial update is the
normal case and because a printer can be online with no job loaded:

```rust
pub struct PrinterStatus {
    pub connection_state: ConnectionState,  // connecting|online|offline|error
    pub error: Option<String>,
    pub job_state: Option<String>,      // print_stats.state
    pub job_name: Option<String>,       // print_stats.filename
    pub progress: Option<f64>,          // display_status.progress, 0.0..=1.0
    pub nozzle_temp_c: Option<f64>,     // extruder.temperature
    pub nozzle_target_c: Option<f64>,   // extruder.target
    pub bed_temp_c: Option<f64>,        // heater_bed.temperature
    pub bed_target_c: Option<f64>,      // heater_bed.target
    pub print_duration_s: Option<f64>,  // print_stats.print_duration
    pub updated_at: String,             // RFC3339, farm3d's clock
}
```

`updated_at` is farm3d's own timestamp, deliberately not Moonraker's
`eventtime` — that value is a Klipper-uptime float, meaningless to the user
and incomparable across printers.

`ConnectionState` is farm3d's, not Moonraker's, and stays deliberately small
so phase 3's adapters can map onto it: `Connecting`, `Online`, `Offline`,
`Error`. It is orthogonal to `job_state`, which is the print-job state the
card shows today.

## Verification performed while firming up this spec

Every claim above was checked rather than assumed:

- **Wire shapes** read from Moonraker's external API docs (printer, server,
  and JSON-RPC notification pages) and Klipper's status reference, rather
  than from memory. The positional-array shape of `notify_status_update` and
  the deprecated 4th axis element are both easy to get wrong from
  recollection.
- **Every crate version compile-verified together** in a throwaway crate,
  including a `tokio-tungstenite` upgrade request carrying a custom
  `X-Api-Key` header, an `mdns-sd` browse over `_moonraker._tcp.local.`, and
  `keyring::Entry::store_status()`. All resolved and built clean.
- **Tauri capability surface** checked against this repo's generated
  `acl-manifests.json`, which is what turned the spec's original guess into
  the correction recorded above.

**Not verified: farm3d has not yet spoken to a live Moonraker instance.** The
contract above is documentation-derived. Phase 3's ElegooLink work is gated on
a protocol spike because SDCP is undocumented; Moonraker is documented well
enough not to need one, but the first real connection may still surface field
absences the docs do not warn about — which is why every `PrinterStatus` field
is `Option` and why the adapter must tolerate missing objects rather than
assume the subscription's shape.

## Out of scope

- OctoPrint and ElegooLink adapters — `2026-08-20-printer-adapters-design.md`.
- Serial/USB connections (ADR-0002's other Connection kind) — shares almost
  nothing with the network adapters and isn't needed for any printer in this
  farm. Not planned in any of the three phases.
- Dispatching Jobs over the connection, or any Slicing integration — ADR-0005
  territory, not this phase. This phase is status *in*, not commands *out*,
  beyond what `probe()` needs to confirm identity.
