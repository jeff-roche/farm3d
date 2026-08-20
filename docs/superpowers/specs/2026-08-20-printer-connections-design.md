# Printer connections: trait, credentials, discovery, and Moonraker

## Status

Architecture-level — settled decisions and an open question recorded ahead of
its own implementation plan. Not yet detailed enough to hand to
`writing-plans`; firm up the open question first (see below), then flesh out
exact Rust signatures and command surfaces the way
`2026-08-20-printer-catalog-and-printers-design.md` does for phase 1.

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

All verified current and actively maintained on crates.io as of this writing:

| Crate | Version | For |
|---|---|---|
| `tokio` | 1.53 | async runtime — the first in this repo; `src-tauri` has none today |
| `tokio-tungstenite` | 0.30 | Moonraker's WebSocket JSON-RPC |
| `reqwest` | 0.13 | HTTP probe, and OctoPrint in phase 3 |
| `keyring` | 4.1 | OS secret store |
| `mdns-sd` | 0.21 | `_moonraker._tcp` / `_octoprint._tcp` browsing |

This is a significant step for a crate that currently has four dependencies
(`tauri`, `tauri-plugin-opener`, `serde`, `serde_json`) and zero async runtime.
Adding `tokio` means Tauri's async command support plus a managed
connection-supervisor task — this phase is where `src-tauri` stops being a
thin file-IO shim and becomes a long-running process with background state.
The implementation plan should call this out explicitly rather than let it
arrive as a side effect.

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

1. Attempt the keychain via `keyring`.
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

Browse `_moonraker._tcp` (and `_octoprint._tcp`, ready for phase 3) via
`mdns-sd`, with a bounded browse window (a few seconds), results pushed to the
frontend as a Tauri event and offered as suggestions in the Connection tab.

**Manual host/port entry is always available and never gated behind
discovery.** A farm of printers at static IPs — the case this whole project
started from — must be fully configurable without mDNS ever succeeding.
Discovery is an accelerator, not a requirement.

Note for the implementation plan: mDNS browsing triggers a local-network
access prompt on macOS, and Tauri's `capabilities/default.json` will likely
need adjusting for the network permission surface. Confirm the exact
capability during implementation rather than guessing here.

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

## Open question to resolve before writing the implementation plan

**Exact wire shape of `ProbeResult` and `PrinterStatus`**, informed by
Moonraker's actual JSON-RPC schema (`printer.info`, `printer.objects.query`,
and the `notify_status_update` subscription payload) — this spec states the
architecture but not the field-by-field contract. Read Moonraker's API docs
and, ideally, probe a real Moonraker instance before finalizing the types, the
same way phase 3's ElegooLink work is gated on a protocol spike.

## Out of scope

- OctoPrint and ElegooLink adapters — `2026-08-20-printer-adapters-design.md`.
- Serial/USB connections (ADR-0002's other Connection kind) — shares almost
  nothing with the network adapters and isn't needed for any printer in this
  farm. Not planned in any of the three phases.
- Dispatching Jobs over the connection, or any Slicing integration — ADR-0005
  territory, not this phase. This phase is status *in*, not commands *out*,
  beyond what `probe()` needs to confirm identity.
