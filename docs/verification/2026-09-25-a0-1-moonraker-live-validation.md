# A0.1 Moonraker live validation (#9)

**Date:** 2026-09-25
**Branch:** `feature/a0-1-moonraker-live-validation`
**Scope:** the existing Moonraker monitoring path: probe, subscription,
supervisor, `farm3d-event-v1` status events, and status backfill. Upload,
print control, artifact identity, and cameras belong to P6 and were not
exercised.

The repo owner ruled that a containerised Klipper and Moonraker with a
simulated board counts as live evidence for this issue. Every evidence item
below names the target that produced it.

## Targets

| Target | What it is | Versions |
| --- | --- | --- |
| **SIM** | `scripts/moonraker-sim/`: real Moonraker and real klippy in containers; the MCU is an atmega644p emulated by simulavr. Host networking. | Moonraker `v0.11.0-1-g1cfb0c4-prind` (API 1.5.0), image `docker.io/mkuf/moonraker:v0.11.0-1-g1cfb0c4`, digest `sha256:c398e6e95def8f4112128ddf51d6dab3a7766e6ea28691e7b42f3bb98622a26d`. Klipper `v0.13.0-770-gce7002bed-prind` (host and MCU firmware), built locally as `localhost/farm3d-simulavr:latest` from mkuf/prind commit `80600f7d6f0d28d4d249728a054ac546a22743b9`, target `build-simulavr`, simulavr `release-1.1.0`. Container OS: Debian 13 (trixie), Python 3.12.14. |
| **U1** | The owner's Snapmaker U1 on the LAN. Read-only: probes, subscriptions, and queries only. | Klipper fork `1.5.2.13_20260722102206`; Moonraker fork `1.5.2` (API 1.4.0); Buildroot 2024.02, kernel 6.1.99, Python 3.11.8. No API key configured (trusted-client LAN). |

Validation host: Linux (CachyOS, kernel 7.2.6), podman rootless, the same
/24 as the U1. farm3d source: this branch.

## How to reproduce

The harness is `src-tauri/tests/a0_moonraker_live.rs`. Its three tests are
`#[ignore]`d and read the target from the environment, so no address is
committed. Each run writes JSON evidence to `FARM3D_MOONRAKER_OUT` (default
`src-tauri/target/a0-moonraker/<UTC time>/`).

| Test | Recipe | Sends to the printer |
| --- | --- | --- |
| `live_probe` | `just moonraker-live probe` | `server.info`, `printer.info`, `printer.objects.list`, `printer.objects.query`, `machine.system_info`, plus probes with no key and with a wrong key. Read-only. |
| `live_watch` | `just moonraker-live watch` | Subscriptions only. Read-only. Runs the production `ConnectionManager` for `FARM3D_MOONRAKER_WATCH_SECS` while a person causes events. |
| `live_lifecycle_drive` | `FARM3D_MOONRAKER_ALLOW_CONTROL=1 just moonraker-live drive` | **M112, FIRMWARE_RESTART, and a 1 °C heater target.** Idle printers only; it refuses a printing or paused one. Checks D1–D9, below. |

All three drive the production adapter and the production supervisor under
a mock Tauri runtime, and they log every `farm3d-event-v1` envelope. A second
"tap" socket logs every raw Moonraker frame. The tap re-subscribes after
`notify_klippy_ready`, as Mainsail and Fluidd do.

Simulator:

```sh
just moonraker-sim build              # once; builds the simulavr image (several minutes)
just moonraker-sim up                 # trusted mode, full printer
just moonraker-sim up apikey          # every request needs the key
just moonraker-sim up trusted no-bed  # no [heater_bed]: the missing-object case
just moonraker-sim api-key > key.txt  # then FARM3D_MOONRAKER_API_KEY_FILE=key.txt
just moonraker-sim restart klipper    # a Klippy disconnect as Moonraker sees it
just moonraker-sim down
```

simulavr cannot reset its emulated MCU, so FIRMWARE_RESTART cannot recover a
shut-down simulator. The drive scenario takes
`FARM3D_MOONRAKER_RESTART_CMD="scripts/moonraker-sim/sim.sh restart klipper"`
instead. Moonraker sees a Klippy disconnect and reconnect either way.

Network faults from the client side use `scripts/moonraker-sim/netfault.py`,
a TCP proxy. `drop` closes every connection and refuses new ones (a host
restart). `freeze` keeps sockets open and relays nothing (a pulled cable).
Point the harness at the proxy with `FARM3D_MOONRAKER_HOST=127.0.0.1
FARM3D_MOONRAKER_PORT=17125`.

## Evidence

### Authentication

| Case | Target | Result |
| --- | --- | --- |
| No key, trusted network | U1 | Probe and subscription succeed. |
| Wrong key, trusted network | U1 | **Accepted.** A trusted-client Moonraker ignores a bad `X-Api-Key`, so farm3d cannot validate a key there. |
| Correct key, API-key mode | SIM | Probe succeeds. |
| No key and wrong key, API-key mode | SIM | The WebSocket upgrade succeeds; every request fails with JSON-RPC code **-32602**, message `Unauthorized`. Before the fix farm3d reported "Unexpected response from the printer". It now reports `Auth` ("The printer rejected the credentials"). |

### Objects, fields, and partial updates

- **U1, SIM:** every requested object and attribute is reported (`extruder`,
  `heater_bed`, `print_stats`, `display_status`, `toolhead`).
- **SIM no-bed:** Moonraker does not omit a missing object. It answers
  `"heater_bed": {"temperature": null, "target": null}`. The adapter reads
  null as absent: `bedTempC` and `bedTargetC` are omitted from the status and
  the UI formatter renders "—" (D2 Pass).
- **U1:** idle bed temperature flickers between 22 and 23 °C, which produces
  bed-only `notify_status_update` frames. The nozzle reading, activity, and
  job name survive each one.
- **SIM:** setting the bed target to 1 °C produces a one-field update that
  keeps every other reading (D3 Pass).
- **SIM, U1:** an idle Klipper with no job reports `print_stats.filename: ""`.
  See mismatch M1.

### Lifecycle, disconnect, reconnect (SIM unless stated)

`live_lifecycle_drive` results after the fixes, on both the full and no-bed
simulator: **D1–D9 all Pass.**

| Check | Before fixes | After |
| --- | --- | --- |
| D1 first live status is online with telemetry | Pass | Pass |
| D2 absent readings stay absent | Inconclusive (harness treated null as present) | Pass (no-bed) |
| D3 one-field update keeps other readings | Pass | Pass |
| D4 M112 → `offline` | Pass (`notify_klippy_shutdown` within 0.2 s) | Pass |
| D5 stays offline while shut down | Pass | Pass |
| D6 supervision started while Klipper is shut down is not online | **Fail**: online, operational `ready`, readiness `ready` | Pass |
| D7 Klippy restart → `notify_klippy_ready` → online | Pass | Pass |
| D8 readings resume after the Klipper restart | **Fail**: tap saw updates; supervisor saw none and kept reporting `fresh` | Pass |
| D9 backfill equals the last event (stream id, sequence, status) | Pass | Pass |

Client-side network faults through `netfault.py`, **U1**, read-only:

| Fault | Before fixes | After |
| --- | --- | --- |
| `drop` for 25 s | `error` ("could not be reached") within 0.3 s; retries at 2, 4, 8, 16 s backoff; online again 5–6 s after the path returned | Same |
| `freeze` for 70 s | **Stayed `online`/`fresh` for the whole 70 s** while `lastObservedAt` aged past 100 s | `error` ("did not respond in time") 30 s into the freeze; online again after the path returned |

## Mismatches

### Corrected with regression tests

| # | Mismatch | Found on | Fix | Tests |
| --- | --- | --- | --- | --- |
| M1 | Idle `filename: ""` became `jobName: ""`. The telemetry cache rejects empty strings, so every cache write failed with a `save` cache warning while the printer sat idle. | SIM, U1 data | Empty strings read as absent. | `an_idle_printer_with_no_job_reports_no_job_name` |
| M2 | Supervision started while Klipper was shut down reported online and **Ready**. No lifecycle notification comes for a shutdown that already happened. | SIM | The subscription asks `server.info` for `klippy_state` and starts from it. | `subscribing_while_klipper_is_shut_down_reports_offline_and_publishes_no_readings`, `a_subscription_answer_that_beats_server_info_waits_for_the_klippy_state` |
| M3 | After any Klippy restart the readings stopped for good while the status stayed `fresh`. Moonraker drops every subscription when Klippy disconnects. | SIM (confirmed in Moonraker's `klippy_connection.py`) | Re-subscribe on `notify_klippy_ready`. | `klipper_becoming_ready_again_resubscribes`, `the_subscription_resubscribes_when_klipper_becomes_ready_again` |
| M4 | A silent network path (no FIN or RST) was never noticed; liveness kept the status `fresh` indefinitely. | U1 via `freeze` | Each liveness tick sends a WebSocket Ping; 25 s with no inbound frame ends the subscription with `Timeout`, and the supervisor reconnects. | `a_silent_connection_ends_the_subscription_with_a_timeout`, `a_quiet_but_answering_connection_stays_up` |
| M5 | A rejected key surfaced as "Unexpected response". Moonraker sends -32602 `Unauthorized`, not 401. | SIM (API-key mode) | `is_auth_error` also accepts -32602 with the `Unauthorized` message. | `moonraker_reports_a_rejected_credential_as_invalid_params` |
| M6 | Telemetry during a Klipper shutdown would flip the supervisor back to `online` (telemetry implies online). | Code review, SIM | Readings still merge but publish only while Klipper is ready. | `readings_during_a_shutdown_do_not_publish_until_klipper_is_ready` |

Also added: `an_object_the_printer_lacks_reads_as_absent_not_zero` pins the
observed null-object behavior.

### Recorded as blockers or decisions (not changed here)

| # | Observation | Target | Proposed owner |
| --- | --- | --- | --- |
| B1 | `print_stats.state` `complete` (and by the same code, `cancelled` and `error`) maps to `HostActivity::Unknown`, so a U1 that finished its last print shows **Unknown / not ready**. Mapping it to Idle would show **Ready** with a finished part still on the bed, which the Start-safety rule forbids. This needs a product decision. | U1 | P1 |
| B2 | The U1 has four toolheads (`extruder` … `extruder3`). farm3d subscribes to `extruder` only, so T1–T3 temperatures are invisible. | U1 | P1 (presentation), P6 |
| B3 | Reported build volume comes from `toolhead` axis limits, which on the U1 include parking and tool-change travel: X 0–271, Y 0–335, Z -6–275, so farm3d reports 271 × 335 × 281 mm for a machine sold as 270 × 270 × 270 mm. A catalog cross-check will flag a false mismatch. | U1 | P1 (or P2 profile check) |
| B4 | `useTls` is offered in the Connection fields, but `tokio-tungstenite` is built without a TLS feature. A `wss://` Connection fails and is reported as "could not be reached". Either enable a TLS feature or remove the option. Not observed live (no TLS Moonraker available); confirmed from the build's feature set. | Build | P1 or P6 |
| B5 | On a trusted-client Moonraker a wrong key is accepted (see Authentication), so a "key verified" message would be false there. | U1 | P1 (setup wording) |
| B6 | The U1 runs vendor forks (Klipper `1.5.2.13…`, Moonraker `1.5.2`) with extra components (`snapmakercloud`, `mqtt`, `exception_manager`, `client_manager`, `timelapse`, `repeater`) and objects (`machine_state_manager`, `print_task_config`, `filament_detect`, `defect_detection`, …). None is needed for monitoring today. The simulator reproduces none of them. | U1 | P6 (command research) |

## Simulator fidelity

What the simulator reproduces faithfully: Moonraker's JSON-RPC surface,
authorization, subscription semantics, lifecycle notifications, and error
codes, because they are the real Moonraker and klippy.

Where it differs from the U1:

- Upstream Moonraker `v0.11.0` (API 1.5.0) versus the U1's fork `1.5.2`
  (API 1.4.0); upstream Klipper versus the Snapmaker fork.
- One extruder, no enclosure sensors, none of the Snapmaker objects or
  components listed in B6.
- Temperatures are constant (the thermistor ADC inputs are not driven), so
  status updates happen only when something changes a field. The U1's idle
  bed flicker produces a steady trickle of real partial updates.
- FIRMWARE_RESTART cannot recover the emulated MCU; a klippy container
  restart stands in for it.
- The simulator's axis limits are ordinary (200 mm cube); the U1's include
  parking travel (B3).
- No real print ran on either target, so `printing`/`paused` states and
  progress during a print are unobserved.

## Still waiting on the owner

1. **Real Tauri path, by hand.** A display exists on the validation host,
   but the WebKitGTK window cannot be driven from this session. Run
   `just dev` and, against the simulator (`just moonraker-sim up`) so the
   real farm is untouched:
   - Add a Printer with a Moonraker Connection to `127.0.0.1:7125`. Confirm
     the card shows live temperatures.
   - `just moonraker-sim down`, then `up trusted no-bed`: the Bed reading
     shows "—", never "0 °C".
   - Send M112 to the simulator with
     `curl -X POST 127.0.0.1:7125/printer/emergency_stop`: the card turns
     Offline. `just moonraker-sim restart klipper`: it returns Online and
     temperatures update again.
   - Quit the app while the simulator is running and relaunch it: the card
     first shows cached readings as stale, then live.
   - Reload the webview (Ctrl+R) while events are flowing: the Monitor
     re-syncs without duplicated or missing state (listener-before-backfill).
2. **A print on the U1.** Start any print from the U1's own screen, then run
   `FARM3D_MOONRAKER_HOST=<U1 address> FARM3D_MOONRAKER_WATCH_SECS=300 just
   moonraker-live watch`. It records `printing`, progress, and job-name
   updates read-only.
3. **Optional: lifecycle on the U1 by hand.** The owner ruled the simulator
   sufficient. For extra confidence, run the same watch and trigger
   FIRMWARE_RESTART from the U1's screen or Fluidd.
4. **Decisions B1–B5** above.
