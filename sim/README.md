# Printer simulators

Container-based printer simulators for automated adapter tests. The
decisions behind them, including which ones count as live evidence, are in
[ADR-0012](../docs/adr/0012-printer-simulators-for-adapter-tests.md).

## Quick start

```sh
just sim-up      # build the Klipper image if needed, start everything, wait until ready
just test-sim    # run the simulator-backed Rust tests
just sim-down    # stop everything and delete its state
```

The first `just sim-up` builds the Klipper + simulavr image from source,
which takes 10–20 minutes and about 5.5 GB. Later runs start in seconds.
Each running simulavr keeps one CPU core busy, so run `just sim-down` when
you are done.

`just test-sim` skips with a message if the simulators are not running.
Set `FARM3D_SIM_REQUIRED=1` (CI does) to make that a failure instead.
Every run writes `manifest.json` and `test.log` to
`src-tauri/target/sim-runs/<UTC time>/`. Cite that manifest in any
evidence that relies on a simulator.

Needs Linux, Docker or podman with a compose tool (`docker compose`,
`docker-compose`, `podman-compose`, or `podman compose`), `curl`, and
`python3`. `simctl` uses podman if it is installed, otherwise Docker. Set
`FARM3D_SIM_ENGINE=docker` or `podman` to choose.

## What runs

| Simulator | Adapter connects to | Harness control path | Contents |
| --- | --- | --- | --- |
| `moonraker` | `127.0.0.1:27125` | `127.0.0.1:27126` | Klipper on simulavr, one extruder and a heated bed ([`moonraker/printer.cfg`](moonraker/printer.cfg)), behind Moonraker |
| `moonraker-multi` | `127.0.0.1:27135` | `127.0.0.1:27136` | The same with four toolheads, `extruder` to `extruder3` ([`moonraker/printer-multi.cfg`](moonraker/printer-multi.cfg)) |
| `octoprint` | `127.0.0.1:25000` | `127.0.0.1:25001` | OctoPrint 1.11.8 with its Virtual Printer attached; API key `farm3d-sim-octoprint-key-0123456789` (a fixture, not a secret) |
| `toxiproxy` | — | `127.0.0.1:28474` | The fault proxy that owns ports 27125, 27135, and 25000 |
| ElegooLink | in-process | — | A fake SDCP server inside the Rust tests; see [`elegoolink/README.md`](elegoolink/README.md) |

Everything binds loopback only and uses host networking, so the ports are
fixed. They do not overlap the ports other local tools use (7125, 5000,
17125).

Only the MCU and its sensors are emulated. Temperatures are fixed
readings, heaters cannot really heat (keep targets at 0 or 1 °C, or
Klipper's heater check shuts it down), and simulavr cannot reset its MCU:
`FIRMWARE_RESTART` does not recover a shut-down Klipper, so the harness
restarts the Klipper container instead.

## Driving the simulators

`sim/simctl --help` lists every command. `just sim <args>` passes through,
for example:

```sh
just sim fault klipper-shutdown             # M112: Klipper goes to "shutdown"
just sim fault klipper-restart moonraker-multi
just sim fault host-down octoprint          # the proxy refuses connections
just sim fault slow moonraker 2000          # 2 s of latency on every response
just sim fault cut moonraker 400            # close after 400 response bytes
just sim fault hang moonraker               # connections stay open, nothing arrives
just sim fault clear                        # remove every fault
just sim reset                              # clear faults and recover Klipper
just sim logs moonraker
just sim manifest
```

Keep `slow` latency below the adapter's timeouts. Toxiproxy cannot drop a
latency toxic quickly while it is still holding data back; use `hang` to
test a timeout.

## The Rust harness

`src-tauri/tests/sim/` is the shared harness, and `sim_moonraker.rs`,
`sim_octoprint.rs`, and `sim_elegoolink.rs` are the tests. The harness
finds the simulators through the `FARM3D_SIM_*` variables that
`sim/simctl env` prints, and `just test-sim` sets them. Rules every test
follows:

- Take `sim::exclusive()` for the whole test, then call the simulator's
  `reset()` first, so tests never see each other's faults.
- Use `require_sim!(...)`, so a missing simulator skips the test with a
  message instead of failing it.
- Talk to the adapter through the proxied address and drive the simulator
  through the control path.

The harness refuses a `FARM3D_SIM_*` address that is not loopback. These
tests write (G-code, M112, restarts, cut connections), and automated tests
never write to a real printer.

## Relationship to the live-evidence tier

The simulators are live evidence for Moonraker (#9, P6) and OctoPrint
(#10). To run a live-evidence test against them, point its own variables
at the proxied port. For example, the #9 and #10 harnesses read
`FARM3D_MOONRAKER_HOST=127.0.0.1` with `FARM3D_MOONRAKER_PORT=27125`, and
`FARM3D_OCTOPRINT_HOST=127.0.0.1` with `FARM3D_OCTOPRINT_PORT=25000` and
the fixture API key. Against real hardware those tests must stay
read-only: probe, subscribe, and query only.
