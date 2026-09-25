# Printer simulators for adapter tests

**Status:** Proposed. The per-adapter evidence rules and the read-only
real-hardware rule below record decisions the repo owner made on
2026-09-25; the rest awaits approval with the branch that adds `sim/`.
ADR-0011 is reserved for P6's connection capability interfaces.

## Context

farm3d talks to printers through adapters (ADR-0002): Moonraker today,
with OctoPrint (#10) and ElegooLink (#8) in progress. Until now each
adapter was tested two ways: against hand-written fakes inside the test
process, and by hand against real hardware. The fakes encode what we
believe a printer does, so they cannot catch a wrong belief. Real hardware
cannot run in CI, and some states (Klipper shutdown, a host that stops
answering, a response cut off part way) are hard or unsafe to cause on
demand.

Most of the printer software farm3d supports can run unmodified in a
container. Klipper runs against an emulated MCU (simulavr), and Moonraker
runs in front of it unchanged. OctoPrint ships a Virtual Printer plugin
that stands in for the serial device. Elegoo publishes nothing comparable
for SDCP.

## Decision

**Simulators are the default for automated adapter tests.** Every adapter
gets a simulator, and adapter tests run against it. One compose file
(`sim/compose.yaml`), driven by `sim/simctl` and the `just sim-up`,
`sim-down`, `sim-status`, and `test-sim` recipes, runs them all. A
fault-injection proxy (Toxiproxy) sits in front of each networked
simulator, so tests can make a host go away, respond slowly, cut a
response short, or go silent. Control scripts put Klipper into shutdown
and restart it.

**Per adapter:**

| Adapter | Simulator | Live-evidence gates | Regression |
| --- | --- | --- | --- |
| Moonraker | Real Klipper on simulavr, behind real Moonraker. Two printers: one extruder with a heated bed, and four toolheads (shaped like a Snapmaker U1). | **Satisfies** #9 and P6's Moonraker gates. | Yes |
| OctoPrint | Real OctoPrint with its bundled Virtual Printer plugin. | **Satisfies** #10's gate (the owner approved this instance earlier). | Yes |
| ElegooLink | An in-repo fake SDCP server, in the test process. | **Does not** satisfy #8, which needs real hardware. **Is** the evidence source for farm3d's ElegooLink command capabilities (see below). | Yes |

**The ElegooLink fake is built only from recorded real-hardware
captures,** never from desk research or assumptions. It loads #8's
redacted captures and derives its behavior from them when it starts: it
answers a request only if a capture holds the same request, it replays
the recorded frames, and it measures timings (such as the roughly 61 s
silent drop) from the captures. It answers nothing else, and it reports
every request it could not answer. Until #8's captures are on `main`, its
tests print PENDING and pass.

By owner decision, the Centauri Carbon's command capabilities (upload,
start, pause, resume, cancel, camera) are verified only against this fake,
so the fake is their evidence source. It may model a command only from
#8's passive captures of another client sending that command (runbook
scenarios S7, S7b, and S10p). Until those captures exist, the command side
is a documented stub: every command is unanswered.

**Real hardware stays an optional, opt-in tier, and it is read-only.**
Automated tests never send writes to a real printer: no upload, no print
control, no G-code, no heater changes, no emergency stop. A real-hardware
test may only probe, subscribe, and query. Every write and command test
runs against a simulator. To enforce this, the simulator harness refuses
any `FARM3D_SIM_*` target that is not a loopback address, and real hosts
come only from environment variables with no defaults.

**Evidence runs record what they ran against.** `just test-sim` writes a
manifest next to its log in `src-tauri/target/sim-runs/<UTC time>/`: the
image references and the digests actually present, the prind and Klipper
commits, the versions each simulator reports, the SHA-256 of every
simulator config file, and the repository commit. Any evidence that relies
on a simulator must cite such a manifest.

**Versions are pinned.** Pulled images are pinned by digest: Moonraker
`mkuf/moonraker:v0.11.0-1-g1cfb0c4`, OctoPrint `octoprint/octoprint:1.11.8`,
and Toxiproxy `2.12.0`. prind does not publish its simulavr image, so the
Klipper image is built locally from pinned prind commit `80600f7` with
Klipper pinned to `ce7002bed` (prind otherwise builds Klipper `master`).
Changing a pin changes what the evidence covers, so it is a reviewed
change.

**Host networking, fixed loopback ports.** Every container uses host
networking and binds `127.0.0.1` on fixed ports in the 25000–28474 range.
Rootless podman without a usable `/dev/net/tun` cannot create bridge
networks, and a port-mapping layer would sit between the proxy and the
service. This limits the simulators to Linux, which is the only platform
F0 supports. The same files work with Docker and with podman.

**`just test` and `just test-rust` stay hermetic.** The container-backed tests are
`#[ignore]`d and skip with a message when the simulators are absent. The
ElegooLink fake needs no container, so its tests run in the normal suite.

## Consequences

- Adapter behavior is checked against the real server software in CI, at
  the cost of a 10–20 minute image build per run. The CI job is therefore
  non-blocking until the Klipper image is cached or published.
- The Moonraker and OctoPrint live-evidence gates can be met without the
  owner's hardware, but only for what the simulators can show. Emulated
  sensors read fixed values, heaters cannot really heat, and simulavr
  cannot reset its MCU, so `FIRMWARE_RESTART` is modelled by restarting
  the Klipper container. Evidence that depends on real heating, motion,
  or a specific vendor firmware still needs a real host, through the
  read-only tier or by hand.
- The simulator's Moonraker trusts loopback clients, so API-key handling
  is not covered by the Moonraker simulator yet. OctoPrint's is, through a
  fixed fixture key.
- The ElegooLink fake can only ever be as complete as #8's captures. A
  command farm3d needs but no capture shows stays unsupported.
- Test plans that assumed a hand-written fake with failure injection for
  Moonraker (P6) can use the simulator and its proxy instead. Faults that
  need to happen inside the server (for example "store the file, then lose
  the response") still need a fake or the proxy's cut.
