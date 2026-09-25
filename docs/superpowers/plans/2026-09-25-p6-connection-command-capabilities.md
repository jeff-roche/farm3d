# P6 Connection Command Capabilities Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. One fresh implementer per `### Task N` section, and a review after each task. Every task section is written to stand alone. It still binds you to **§Global Constraints**, which the controller hands to every implementer with the task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Final. GitHub issue #16. It was written on 2026-09-25 and
rebased onto `main` at `eba01b5`, which includes:

- #27 (OctoPrint status adapter, `SUPPORTED_KINDS`, public
  `supervisor::build_connection`);
- #28 (the printer simulator harness `sim/`, ADR-0012, Toxiproxy);
- #29 (#9 Moonraker validation: `HostActivity`/`OperationalState`
  `Finished`/`Cancelled`/`Failed`, `ReadinessReason::BedNeedsClearing` and
  `PrintFailed`, `PrinterTelemetry.tools`, and TLS rejected everywhere).

Issues #7, #8, #9, and #10 are closed. #8 closed **without** a go decision,
so ElegooLink is out of P6.

**Approval.** The owner told the controller to finalise the plan and go
straight to implementation. There are no user approval stops. Where this
plan says **controller approves**, the controller (the orchestrating agent
running subagent-driven development) reviews the output, records the
approval in the document's own `Status` line, and continues. That applies
to the spike report (Task 3) and to the focused spec and ADR-0011
(Task 4).

## Owner decisions (fixed)

1. **Start** ships in the P6 UI, always behind a bed-clear confirmation.
2. At most **one unresolved write per Printer**.
3. **Archive** is blocked while a write is unresolved.
4. **Credential replacement** on the same endpoint is allowed while a
   write is unresolved, and still probed. Clearing the credential or
   changing the endpoint is blocked.
5. Permanent Printer **delete** also deletes that Printer's *terminal*
   Host Operation rows.
6. **TLS** stays unsupported. #29 already rejects `useTls` everywhere and
   hides the control, and P6 adds no TLS.
7. The local **container simulators** (`sim/`) are live evidence for
   Moonraker (ADR-0012).
8. **Real printers are read-only.** Agents never write to a real printer:
   no upload, no interrupted upload, no file delete, no
   start/pause/resume/cancel, no G-code, no service restart, with or
   without an opt-in. The real-hardware tier only probes, subscribes, and
   queries.
9. **Multi-toolhead printers** are supported. Anything that touches tools
   handles N tools.
10. **ElegooLink:** out of P6. #8 closed without a go decision.
11. **The adapter registry** builds on #27's `SUPPORTED_KINDS` and
    `build_connection`.
12. **#9 readiness:** `Finished`, `Cancelled`, and `Failed` are never
    Ready.
13. **Start is offered** when the Printer is Ready, **or** when it is
    `Finished` or `Cancelled`. In the latter two cases the bed-clear
    confirmation is mandatory and names the prior state ("The previous
    print finished. The bed is clear."). After `Failed`, Start stays
    disabled until the host no longer reports an error.
14. **Simulators do not run in CI** (too expensive). Evidence gates run
    locally with `just sim-up && just test-sim`. CI runs only the
    in-process fakes.

**Goal:** For Moonraker, farm3d can:

- **stage** (upload without starting) one immutable Slice Revision to a
  Printer;
- **start** a staged file, and **pause**, **resume**, and **cancel** the
  host's print;
- read the **host state** and **artifact identity** it needs to reconcile;
- report **camera** availability as a capability only (no media; that is
  P8).

Every host write carries a durable **Host Operation** identity. A lost
response makes the operation **uncertain**, and farm3d reconciles it
against the host after reconnect or restart. It never re-uploads or
re-starts blindly. Archive/delete, Printer import, Connection changes, and
credential deletion cannot orphan an unresolved operation. For a host that
is gone for good, the operator can explicitly **abandon reconciliation**,
and that is recorded durably.

**Out of scope (P7):** Queue Entries, Jobs, scheduling, unattended start,
retry policy, and material accounting.

## Evidence: the code on `main` at `eba01b5`

**Connections** (`src-tauri/src/connections/`):

- `PrinterConnection` (in `mod.rs`) is observation-only: `probe` and
  `subscribe`.
- `SUPPORTED_KINDS = [MOONRAKER_KIND, OCTOPRINT_KIND]` and
  `is_supported_kind` are in `mod.rs`. Every kind check calls
  `is_supported_kind`:
  - `connections/commands.rs:301`;
  - `printers/create.rs:298`;
  - `printers/batch.rs:567`;
  - `printers/setup.rs:47`.
- `supervisor::build_connection` (public, `supervisor.rs:964`) builds the
  observer. A supervisor test keeps it in step with `SUPPORTED_KINDS`.
- `reject_tls` and `TLS_UNSUPPORTED_MESSAGE` are in `mod.rs`, called from
  `set_printer_connection` and `test_printer_connection`.
- `MoonrakerConnection` is WebSocket JSON-RPC only; the pure framing is in
  `moonraker/protocol.rs`.
- `OctoPrintConnection` uses `reqwest` 0.13 (a direct dependency with
  `default-features = false`). It does not follow redirects and bypasses
  the system proxy.

**Status vocabulary (#29)** (`printers/operational.rs`,
`connections/status_repository.rs`):

- `HostActivity`: `Idle`, `Printing`, `Paused`, `Busy`, `Finished`,
  `Cancelled`, `Failed`, `Unknown`.
- `OperationalState` adds `Finished`, `Cancelled`, and `Failed` (never
  `Ready`).
- `ReadinessReason::BedNeedsClearing` (after Finished or Cancelled) and
  `ReadinessReason::PrintFailed` (after Failed).
- `PrinterTelemetry.tools: Vec<ToolTemperature { index, temp_c, target_c }>`,
  in index order. It is **empty for a single-tool printer**, whose one
  nozzle is `nozzle_temp_c`/`nozzle_target_c`. On a multi-tool printer
  `nozzle_*` repeats tool 0. The bed is `bed_temp_c`/`bed_target_c`.

**Persistence and guards:**

- The highest migration is `0006_p5_slicing.sql`, and
  `CURRENT_SCHEMA_VERSION = 6` is in `persistence/migrations.rs`.
- Printer lifecycle blockers: `printers::lifecycle::blocker_sources()`.
- Slice Revision deletion blockers:
  `slicing::blockers::slice_revision_blocker_sources()` (empty).
- The operations ledger is `spools::operations` (`OperationKind`, `claim`).
- `PrinterRepository::set_connection` and `replace_all` (import) have no
  operation guard.
- Credential cleanup: `retry_pending_credential_cleanup_locked` skips any
  reference a Printer row still holds.

**Content:** a Slice Revision has an `slr-*` id, `gcode_sha256`, and
`gcode_size`, and its bytes come from `ContentStore::open_verified`.

**Commands:** `lib.rs` `COMMAND_NAMES` has 79 entries today.

**Simulator harness (#28, ADR-0012, `sim/README.md`):**

| Simulator | Adapter target (through Toxiproxy) | Control path | Env vars (`sim/simctl env`) |
|---|---|---|---|
| `moonraker`: one extruder, heated bed, `virtual_sdcard`, `pause_resume` | `127.0.0.1:27125` | `127.0.0.1:27126` | `FARM3D_SIM_MOONRAKER`, `FARM3D_SIM_MOONRAKER_CONTROL` |
| `moonraker-multi`: four tools, `extruder` to `extruder3` | `127.0.0.1:27135` | `127.0.0.1:27136` | `FARM3D_SIM_MOONRAKER_MULTI`, `FARM3D_SIM_MOONRAKER_MULTI_CONTROL` |
| `octoprint`: 1.11.8 with Virtual Printer | `127.0.0.1:25000` | `127.0.0.1:25001` | `FARM3D_SIM_OCTOPRINT`, `FARM3D_SIM_OCTOPRINT_CONTROL`, `FARM3D_SIM_OCTOPRINT_API_KEY` |
| `toxiproxy` | — | `127.0.0.1:28474` | `FARM3D_SIM_TOXIPROXY` |

- **Recipes:** `just sim-up`, `just sim-down`, `just sim-status`,
  `just sim <args>`, and `just test-sim`. `just test-sim` runs `--test
  sim_moonraker --test sim_octoprint --test sim_elegoolink` with
  `--include-ignored --test-threads=1`, and writes `manifest.json` and
  `test.log` to `src-tauri/target/sim-runs/<UTC>/`.
  `FARM3D_SIM_REQUIRED=1` turns a skip into a failure.
- **Rust harness:** `src-tauri/tests/sim/`.
  - `require_sim!`, `sim::exclusive()`, and a loopback-only guard.
  - `MoonrakerSim::discover_variant(Variant::{Single, MultiTool})`,
    `.reset()`, `.gcode()`, `.query()`, `.heaters()`, and
    `.restart_klipper()`.
  - `Toxiproxy::{set_enabled, slow, cut_after, hang}`. Toxics are
    **downstream** (responses) only.
- **Sim gaps P6 needs filled:**
  - The sim Moonraker trusts loopback, so API keys are not covered.
  - `moonraker.conf` has no `[history]` section.
  - There is no no-bed variant.
  - There is no upstream (request-side) cut.
- **Retiring:** `scripts/moonraker-sim/` and the `just moonraker-sim`
  recipe predate `sim/`. They offer `apikey` mode, a `no-bed` variant, a
  `multi-tool` variant, and `netfault.py`.
- **Pending test:** `src-tauri/tests/sim_octoprint.rs` still has
  `octoprint_adapter_against_the_simulator_is_pending_10`, a placeholder
  test.

**Real-hardware read-only tier (#29):**

- `src-tauri/tests/a0_moonraker_live.rs` runs through `just moonraker-live
  probe|watch|drive`. It reads `FARM3D_MOONRAKER_HOST` (required),
  `FARM3D_MOONRAKER_PORT` (default 7125), `FARM3D_MOONRAKER_API_KEY`, and
  `FARM3D_MOONRAKER_API_KEY_FILE`.
- `drive` writes and is guarded to loopback.
- `scripts/check-private-hosts.sh` (`just check-hosts`) fails on owner
  hosts listed in `FARM3D_PRIVATE_HOSTS` or an untracked `.private-hosts`.

**CI** (`.github/workflows/pr-validation.yml`) runs:

- `npm run build`, `npm test`;
- `just gen-contracts` with a diff check;
- `cargo test --locked --features test-support`.

No simulator runs there.

## Design reference

Tasks restate what they need from here. This section is the single place
the design is argued.

### D1. Composable capability interfaces (the issue's decision gate)

| Option | For | Against |
|---|---|---|
| A. Add methods to `PrinterConnection` with default "unsupported" bodies | Smallest diff | "Not implemented" looks the same as "the protocol can't". Every fake grows. Observation and writes share the subscription's object. |
| B. One `PrinterCommands` trait | Read/write split | All-or-nothing per adapter; host-state and camera are not commands |
| **C. Small capability traits + a registry (chosen)** | Present when built, so unsupported is structural. Small fakes. Adapters land one capability at a time. | More types; a registry to keep honest |

The four traits are `ArtifactStaging` (`upload`, `locate`),
`PrintControl` (`start`, `pause`, `resume`, `cancel`), `HostStateQuery`
(`host_job_state`, `job_history`), and `CameraDiscovery` (`cameras`). An
`AdapterDescriptor { kind, observe, staging?, control?, host_state?,
camera?, evidence }` lives in `connections/adapters.rs`. The descriptor
row grows out of `SUPPORTED_KINDS`.

Commands build their capability objects per operation and never share the
supervisor's socket. Wire code stays pure and adapter-local. ADR-0011
records the choice. ADR-0012 is taken by the simulator harness.

### D2. Host Operation record

The `host_operations` table (migration `0007_p6_host_operations.sql`):

| Column | Meaning |
|---|---|
| `id` | `hop-<uuid>` |
| `printer_id` | FK `ON DELETE RESTRICT` |
| `kind` | `upload`, `start`, `pause`, `resume`, `cancel` |
| `slice_revision_id` | FK `ON DELETE SET NULL` |
| `gcode_sha256`, `gcode_size` | Copied from the revision |
| `host_path` | `farm3d/<slr-id>.gcode` |
| `endpoint_json` | `kind`, `host`, `port`, never a credential or `credentialRef` |
| `state` | See D3 |
| `failure_json`, `resolution_json` | Credential-free evidence |
| `attempts`, `last_attempt_at`, `last_attempt_error` | Reconciliation bookkeeping |
| `abandoned_at`, `abandon_note` | D8 |
| `created_at`, `dispatched_at`, `resolved_at` | Timestamps |

- **Write-ahead:** the row is committed as `dispatching` before any byte
  goes to the host.
- At most one unresolved row per Printer, enforced by a partial unique
  index `WHERE state IN ('dispatching','uncertain','reconciling')`.
- Terminal rows are immutable (trigger).
- Commands are idempotent by client `operationId`, through the operations
  ledger.

### D3. State machine

```text
dispatching ─ host confirms ─────────────> succeeded
dispatching ─ definitive rejection ──────> failed
dispatching ─ timeout / lost / restart ──> uncertain
uncertain ─ reconcile begins ────────────> reconciling
reconciling ─ proved applied ────────────> succeeded
reconciling ─ proved not applied ────────> failed{notApplied}
reconciling ─ unreachable / inconclusive ─> uncertain
uncertain ─ operator, risk-confirmed ────> abandoned
```

An unsupported capability never creates a row
(`CAPABILITY_UNSUPPORTED`).

### D4. Staging

- The host path is `farm3d/<slr-id>.gcode` in the `gcodes` root.
- Bytes are streamed from `open_verified(gcode_sha256)`.
- The upload sends `checksum=<sha256>` if the host accepts it (spike Gate
  B).
- The request builder has **no way** to set Moonraker's `print` field.
- `locate`: present at `host_path` with size == `gcode_size`. If the host
  gives no hash, a download-and-hash must equal `gcode_sha256`. Size alone
  is never proof.
- A re-upload happens only as a new operator-initiated operation.

### D5. Reconciliation

It runs at startup (after `restore_persisted_connections`), when the
supervisor reports Online, and on **Check again**.

| Kind | Proved applied | Proved not applied |
|---|---|---|
| upload | `locate` matches | absent at `host_path` |
| start | `print_stats.filename == host_path` and the state is printing, paused, or complete, **or** a history job for `host_path` started after `dispatched_at` (the spike sets the skew tolerance) | host standby with no such history job |
| pause/resume/cancel | observed state matches | observed state proves no effect |

Anything else stays `uncertain`. A host printing a *different* file stays
`uncertain`. A start is never retried automatically. The raw
`print_stats` `complete` is evidence that a start ran; it does not make
the Printer Ready (answer 12).

### D6. Capability matrix

```ts
type CapabilityKey = "upload" | "start" | "pause" | "resume" | "cancel"
  | "hostState" | "artifactIdentity" | "camera";
type CapabilityState =
  | { status: "supported" }
  | { status: "unsupported"; reason: "adapter" | "notVerified" | "host"; detail: string };
type PrinterCapabilities = {
  printerId: string;
  adapterKind: string | null;
  evidence: { source: string; tier: "sim" | "readOnlyHardware"; verifiedHostVersions: string[] } | null;
  capabilities: Record<CapabilityKey, CapabilityState>;
  observedAt: string | null;
};
```

- `readOnlyHardware` evidence can never make a write capability
  `supported`.
- OctoPrint's row is `notVerified` for every write capability (see §Out of
  scope).
- A `useTls` Connection (possible only from old data or import) reports
  every write capability as `notVerified`.
- Host facts list every tool, and are never collapsed into one nozzle.
- Rust exposes `capabilities_for(printer, host_facts)` for P7.

### D7. Guards (run inside the gated transaction)

| Mutation while unresolved | Result |
|---|---|
| Archive, delete | `LIFECYCLE_BLOCKED` with `HOST_OPERATION_UNRESOLVED` |
| Printer import | Rejected as a whole |
| Endpoint change, Connection clear, credential clear | `CONNECTION_IN_USE` |
| Credential replace, same endpoint | Allowed (probed) |
| Delete a referenced Slice Revision | `LIFECYCLE_BLOCKED` with `HOST_OPERATION_UNRESOLVED` |

Permanent delete removes the Printer's terminal rows in the same
transaction.

### D8. Abandon reconciliation

`abandon_host_operation(operationId, hostOperationId, acknowledgement:
"hostStateUnknown", note?)`:

- It is allowed only from `uncertain`, after at least one failed
  reconciliation attempt.
- It needs a Kobalte `AlertDialog` with a required acknowledgement
  checkbox.
- The result is the terminal `abandoned` state, and the row is kept.
  Guards treat it as resolved.
- Abandoned is not success: P7 treats it as reconciliation-required.

### D9. Start rule (answers 1, 12, 13)

| `OperationalState` (readiness reason) | Start | Required checkbox |
|---|---|---|
| `Ready` | Offered | "The bed is clear." |
| `Finished` (`BedNeedsClearing`) | Offered | "The previous print finished. The bed is clear." |
| `Cancelled` (`BedNeedsClearing`) | Offered | "The previous print was cancelled. The bed is clear." |
| `Failed` (`PrintFailed`) | Disabled: "Clear the error on the printer first." | — |
| Printing, Paused, Busy, Offline, Connecting, Unknown, Error, SetupIncomplete, or stale telemetry | Disabled, with the state as the reason | — |

- This applies whatever the Printer's `StartSafety` rule is. P6 never
  starts unattended.
- `start_staged_artifact` takes `priorState: "ready" | "finished" |
  "cancelled"`.
- Before the write-ahead commit, it re-reads the Printer's current status
  and rejects with no row written:
  - `START_NOT_ALLOWED`, carrying the observed state, when the state is
    not in the offered set;
  - `START_PRECONDITION_CHANGED` when `priorState` no longer matches.
- P6 sends no reset command (no `SDCARD_RESET_FILE`).

## Global Constraints

These rules bind every task. The controller gives this section to every
implementer.

1. **No writes to real printers.** Every write test (upload, control,
   G-code, faults) runs against the in-process fakes or the loopback
   simulators. The real-hardware tier (`FARM3D_MOONRAKER_HOST`) may only
   probe, subscribe, and query, through a read-only client. No variable
   enables writes to a non-loopback host.
2. **No owner network details in the repository.** Never commit an owner
   IP address, hostname, serial, MAC address, token, or local filesystem
   path. Use environment variables, placeholders like `<U1 host>`, or RFC
   5737 addresses (`192.0.2.x`). Scrub captured responses before they
   become fixtures. Run `just check-hosts` before committing.
3. **Credentials** never enter frontend state, events, errors, logs,
   `host_operations`, fixtures, or snapshots. Every new path gets a
   seeded-secret test.
4. **Frontend conventions** (AGENTS.md, DESIGN.md):
   - `--f3d-*` tokens only, with no hard-coded colors, font sizes, or
     radii.
   - CSS Modules next to components.
   - Kobalte primitives for anything interactive. Check
     `node_modules/@kobalte/core/src/<name>/` for props; don't guess.
   - The dense editor aesthetic.
   - Kobalte Select and Menu tests use `fireEvent.pointerDown` and
     `fireEvent.pointerUp`.
   - New design-system components go in `components/index.ts` and
     `Showcase.tsx`.
5. **Test-first.** Write the failing test, make it pass, then refactor.
   Commit per task with a conventional message.
6. **Gates** before a task is done. Run the ones the task touches; the
   controller runs all of them at review.

   ```sh
   export PATH="$HOME/.cargo/bin:$PATH"   # cargo is not on PATH in non-interactive shells
   just build
   just test
   just test-rust
   just check-hosts
   ```

   If a Rust type exported to TypeScript changed, also run
   `just gen-contracts && git diff --exit-code src/generated`. Never
   hand-edit `src/generated/**`.
7. **Contract registration.** A new Tauri command goes in:
   - `lib.rs` `COMMAND_NAMES` and `generate_handler!`;
   - `contracts/inventory.rs` (`COMMAND_CONTRACTS`, its length, the
     declaration, and the visitor);
   - `tests/export_contracts.rs`;
   - `src/ipc/client.ts` `CommandMap`.

   Count assertions add P6's commands to whatever `main` has, never a
   hard-coded total. Events are emitted after commit only.
8. **Simulators never run in CI.** Container-backed tests are
   `#[ignore]`, use `require_sim!`, and run locally through
   `just sim-up && just test-sim`. Evidence cites the run's
   `src-tauri/target/sim-runs/<UTC>/manifest.json`. Anything CI must
   check needs an in-process fake.
9. **No TLS, no P7.** Don't add TLS features. Don't add Queue, Job,
   scheduling, or material-accounting code.

## Test tiers

| Tier | What | How it runs | Counts as evidence | Writes |
|---|---|---|---|---|
| Unit and in-process fakes | Pure protocol functions; `FakeMoonraker` (Task 8) for server-internal faults: file stored then response lost, partial file, definitive rejections, host restart with state, detecting a `print` field | `just test-rust`, and CI | No (regression) | Yes (in-process) |
| Simulator | Real Klipper and Moonraker in `sim/`, with Toxiproxy for network faults (host down, slow, response cut, hang, request cut) | `just sim-up && just test-sim` (local only) | **Yes**, for every write gate | Yes (loopback only) |
| Real hardware, read-only | A real Moonraker named by `FARM3D_MOONRAKER_HOST`, `_PORT`, `_API_KEY`, `_API_KEY_FILE` | `just p6-readonly` (Task 12) | Read-side corroboration only | **Never** |

## Out of scope for P6

- **OctoPrint command capabilities.** A future OctoPrint P6 path first
  needs these #10 follow-ups, recorded here as its prerequisites:
  - OctoPrint reports `Operational` after a finished print, which the
    adapter maps to `Idle`, so the Printer shows **Ready** instead of
    Finished (it never reaches `BedNeedsClearing`).
  - Only `tool0` is read, so multi-tool OctoPrint printers show one tool.

  After those, it needs its own command-research spike against the
  OctoPrint simulator, and a short adapter plan. Its matrix row stays
  `notVerified`.
- **ElegooLink:** needs a future spike and a go decision (#8 closed
  without one).
- Camera media, snapshots, and the Camera tab (P8).
- Everything listed under P7.

## File map

**Backend (new):**

- `connections/adapters.rs`, `connections/capabilities.rs`;
- `connections/moonraker/files.rs` (pure) and
  `connections/moonraker/control.rs` (I/O);
- `host_ops/{mod,state,repository,executor,reconciler,guards,start_rule,events,commands}.rs`;
- `migrations/0007_p6_host_operations.sql`.

**Backend (modified):**

- `connections/mod.rs`, `connections/supervisor.rs`;
- `printers/lifecycle.rs`, `printers/repository.rs`;
- `slicing/blockers.rs`, `spools/operations.rs`;
- `contracts/command.rs`, `contracts/inventory.rs`;
- `lib.rs`, `Cargo.toml` (`reqwest` features `multipart` and `stream`).

**Tests:**

- `tests/common/fake_moonraker.rs`;
- `tests/p6_*.rs`;
- `tests/sim_moonraker.rs` (the P6 section), `tests/sim_octoprint.rs`;
- `tests/p6_moonraker_readonly.rs`;
- `tests/sim/toxiproxy.rs` (the upstream cut).

**Frontend:**

- `src/host-ops/{capabilities-store,host-operations-store,host-operations-store-mock,web-fixtures,presentation,start-rule}.ts`;
- `src/screens/{PrinterJobPanel,StageOnPrinterDialog,StartStagedDialog,AbandonReconciliationDialog,CapabilityList}.tsx`,
  each with a `.module.css`.

**Sim:** `sim/moonraker/*`, `sim/simctl`, `sim/README.md`; delete
`scripts/moonraker-sim/`.

**Docs:**

- `docs/adr/0011-composable-connection-capabilities.md`;
- the spec `docs/superpowers/specs/2026-09-2x-p6-connection-command-capabilities-design.md`;
- the spike `docs/superpowers/baselines/2026-09-2x-p6-moonraker-command-spike.md`;
- `docs/verification/2026-09-2x-p6-moonraker-commands.md`;
- `CONTEXT.md`.

## Tasks

### Task 1: Retire `scripts/moonraker-sim` into `sim/` and enable the OctoPrint simulator tests

**Owner:** Wiring. **Depends on:** nothing. **Status:** ready now.

**Why:** `scripts/moonraker-sim/` (`just moonraker-sim`) predates the
shared harness `sim/` (ADR-0012). It has features `sim/` lacks, and P6's
spike needs them:

- an API-key mode (`sim/` Moonraker trusts loopback, so API-key handling
  is untested);
- a `no-bed` variant.

P6 also needs Moonraker's `[history]` component, which neither has. The
OctoPrint adapter is now on `main`, but `sim_octoprint.rs` still has a
PENDING placeholder.

**Files:**

- `sim/moonraker/moonraker.conf`, `sim/moonraker/moonraker-multi.conf`:
  add `[history]`.
- `sim/moonraker/` and `sim/simctl`: add the two variants as a **mode of
  the existing `moonraker` service**, not new containers, because each
  simulavr pins a CPU core. Command:
  `sim/simctl variant moonraker default|no-bed|apikey`. It swaps
  `printer.cfg` (no-bed drops `[heater_bed]`, the same awk rule as
  `scripts/moonraker-sim/sim.sh`) or `moonraker.conf`, then restarts that
  service.
  - In `apikey` mode loopback is **not** trusted
    (`trusted_clients: 192.0.2.0/24`, as the old script does).
  - `simctl env` also exports `FARM3D_SIM_MOONRAKER_API_KEY` in `apikey`
    mode. Read it from Moonraker's database the way
    `scripts/moonraker-sim/sim.sh api-key` does.
  - `simctl reset` returns to `default`.
- `src-tauri/tests/sim/moonraker.rs`: add
  `MoonrakerSim::set_variant(Variant...)` or an equivalent
  `use_mode(Mode::{Default, NoBed, ApiKey})`. Make `connection()` pass the
  API key in `apikey` mode. `reset()` restores `default`.
- `src-tauri/tests/sim_moonraker.rs`: add tests for no-bed (the bed
  reading is absent, not zero) and apikey (no key gives an auth error,
  the right key works).
- `src-tauri/tests/sim_octoprint.rs`: replace
  `octoprint_adapter_against_the_simulator_is_pending_10` with tests that
  drive the production `OctoPrintConnection` (`sim.config()` plus
  `FARM3D_SIM_OCTOPRINT_API_KEY`):
  - `probe` returns OctoPrint 1.11.8;
  - `subscribe` yields telemetry and an Online health;
  - a disabled proxy makes `subscribe` end with an error;
  - a wrong key gives `ConnectionError::Auth`.

  Update the module doc comment.
- Delete `scripts/moonraker-sim/` (`sim.sh`, `printer.cfg`,
  `extruder1.cfg`, `moonraker.conf.in`, `netfault.py`) and the
  `moonraker-sim` recipe in `justfile`.
  - `netfault.py`'s drop, freeze, and pass map to Toxiproxy's
    `set_enabled(false)`, `hang`, and `reset`.
  - The old `multi-tool` variant is covered by `moonraker-multi`.
- `sim/README.md`: document `variant` and the new env var.
- `docs/verification/2026-09-25-a0-1-moonraker-live-validation.md`: it is a
  historical record, so don't rewrite its steps. Add one note at the top
  mapping `just moonraker-sim …` to the `sim/` equivalents, and
  `FARM3D_MOONRAKER_RESTART_CMD` to `sim/simctl fault klipper-restart
  moonraker`.
- Update any `FARM3D_MOONRAKER_RESTART_CMD` example in
  `src-tauri/tests/a0_moonraker_live.rs` doc comments that names the old
  script.

**Acceptance criteria:**

- `grep -rn "moonraker-sim" --exclude-dir=node_modules --exclude-dir=target .`
  finds only the note in the A0.1 verification record.
- `sim/simctl variant moonraker no-bed` then `sim/simctl status` shows
  Moonraker ready with no `heater_bed` object.
- `sim/simctl variant moonraker apikey` makes an unauthenticated
  `/server/info` on port 27125 return 401. `simctl env` exports the key.
- `sim/simctl reset` restores the default config.
- `just test-sim` passes with the simulators up (new tests included), and
  skips cleanly with them down.
- `just test-rust` still passes. The container tests stay `#[ignore]`.

**TDD:** write the new `sim_moonraker.rs` and `sim_octoprint.rs` tests
first. With the simulators up, the no-bed and apikey tests fail until the
variant exists.

**Commands:**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
just sim-up
just test-sim
just sim-down
just test-rust
just check-hosts
```

### Task 2: Adapter registry on top of #27

**Owner:** Backend. **Depends on:** nothing. **Status:** ready now.

**Context:** #27 put every kind check behind
`connections::is_supported_kind` (backed by `SUPPORTED_KINDS`), made
`supervisor::build_connection` public, and added a supervisor test that
keeps the list and the factory in step. P6 needs a registry of adapter
**descriptors**, so later tasks can hang capability builders and an
evidence row on each kind. This task changes no behavior.

**Files:**

- New `src-tauri/src/connections/adapters.rs`:

  ```rust
  pub type ObserveBuilder = fn(&ConnectionConfig, Option<zeroize::Zeroizing<String>>) -> Box<dyn PrinterConnection>;
  pub struct AdapterDescriptor {
      pub kind: &'static str,
      pub observe: ObserveBuilder,
      // Task 5 adds: staging, control, host_state, camera, evidence.
  }
  pub fn registry() -> &'static [AdapterDescriptor];   // Moonraker, OctoPrint
  pub fn descriptor(kind: &str) -> Option<&'static AdapterDescriptor>;
  ```

- `connections/mod.rs`: `SUPPORTED_KINDS` stays, as the kinds of
  `registry()` in order (a `const` array kept in step by a test is fine).
  `is_supported_kind(kind)` becomes `descriptor(kind).is_some()`. Don't
  edit the four call sites' logic.
- `connections/supervisor.rs`: `build_connection` delegates to
  `descriptor(&config.kind).map(|d| (d.observe)(config, api_key))`.
  `ConnectionManager::with_clock_and_factory` keeps its signature (tests
  inject closures).
- Replace the list-and-factory consistency test with a registry
  consistency test.

**Acceptance criteria:**

- Every existing test passes unchanged. That includes the P2 batch tests
  and #27's OctoPrint tests (`a0_octoprint_path`).
- An unknown kind still gives `UNSUPPORTED_ADAPTER` from set, create, and
  batch, `SetupGap::UnsupportedAdapter`, and the supervisor's "not
  supported by this build" status.
- `SUPPORTED_KINDS` equals `registry().iter().map(|d| d.kind)`.

**TDD:** first write a failing unit test in `adapters.rs`:

- `descriptor("moonraker")` and `descriptor("octoprint")` exist;
- `descriptor("elegoolink")` is `None`;
- each descriptor's `observe` builds a connection.

Then the consistency test.

**Commands:**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
just test-rust
just check-hosts
```

### Task 3: Moonraker command spike (simulator plus read-only real host)

**Owner:** Protocol. **Depends on:** Task 1 (apikey mode, `[history]`).
**Status:** ready after Task 1. The read-only real-host checks can start
now.

**Output:** `docs/superpowers/baselines/2026-09-2x-p6-moonraker-command-spike.md`.
It has one row per gate and per tier (PASS, FAIL, or Unavailable), with
the evidence, the decision, the sim run manifest path, and the versions
Moonraker and Klipper report.

**Scratch code:** a throwaway crate outside the repo, or a `#[ignore]`d
test that is not committed. Don't commit probing code. **The controller
approves** the report by recording approval in its Status line.

**Rules:**

- Every write happens on the simulator only (`FARM3D_SIM_MOONRAKER`,
  loopback).
- The real-host column is read-only: `server.info`, `printer.objects.list`
  and `printer.objects.query`, `server.files.list` and metadata for files
  already on the host, `server.history.list`, and `server.webcams.list`.
- Never write down the real host's address, hostname, or serial. Scrub
  captures (answer 8, Global Constraints 1–2).

| Gate | Simulator (required) | Real host (read-only) |
|---|---|---|
| A. HTTP auth | `variant apikey`: `X-Api-Key` on `/server/files/*` and on the WebSocket; the 401 shape | Record whether the key is needed for reads |
| B. Upload without start | `POST /server/files/upload` to `farm3d/<uuid>.gcode` with no `print` field. `print_stats` stays unchanged. Response shape. Is `checksum` accepted, and is a wrong one rejected (and with what status)? | Not run |
| C. Identity | `metadata` and `list` fields for the staged file (size, modified, any hash). Time a 20 MB download for download-and-hash | Field shapes for existing files |
| D. Interrupted upload | Response lost: `Toxiproxy::cut_after(Moonraker, 0)`. Mid-body: a request-side cut (see Task 12's upstream toxic). Is a file left behind, and what size? | Not run |
| E. Control | `printer.print.start/pause/resume/cancel` from each state. Wrong-state and Klippy-not-ready error shapes. Classify each as definitive or indeterminate | Not run |
| F. Start evidence | `print_stats` sequence and a `server.history.list` job after a start whose response was cut. Measure the clock skew | Shapes of existing history and `print_stats` |
| G. Host restart | Restart the Klipper and Moonraker containers mid-upload and mid-print. What persists? | Not run |
| H. Capability detection | `server.info` components, `printer.objects.list` (`virtual_sdcard`, `pause_resume`, every `extruder*`), `server.webcams.list`, on `moonraker`, `moonraker-multi`, and `variant no-bed` | Same queries |
| I. Clean start state | After a print completes, `print_stats.state == "complete"` persists until the next start. Confirm that `printer.print.start` from `complete` and from `cancelled` works (answer 13) | Not run |

**Acceptance criteria:**

- Every simulator cell is PASS, FAIL, or Unavailable with evidence.
- Every real-host cell is read-only or "Not run".
- The report lists every decision the spec must take: the checksum, the
  download-and-hash rule, which responses are definitive, the skew
  tolerance, and the partial-file handling.
- `just check-hosts` passes.

**Commands:**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
just sim-up
eval "$(sim/simctl env)"      # FARM3D_SIM_* for scratch probes
sim/simctl variant moonraker apikey
sim/simctl variant moonraker no-bed
sim/simctl reset
just sim-down
just check-hosts
```

### Task 4: Focused spec and ADR-0011

**Owner:** Wiring, with Protocol. **Depends on:** Task 3.

**Files:**

- `docs/superpowers/specs/2026-09-2x-p6-connection-command-capabilities-design.md`,
  in the P5 spec's shape: status, goal, scope and non-goals, vocabulary,
  decisions, backend model, wire types, commands and events, frontend,
  errors, accessibility, acceptance criteria, and delivery.
- `docs/adr/0011-composable-connection-capabilities.md`.
- `CONTEXT.md`: add **Host Operation**, **Staged artifact**,
  **Uncertain outcome**, **Abandon reconciliation**, and **Capability**.
  Update **Connection**.

**Content:**

- Turn this plan's Design reference D1–D9 into final decisions, using the
  spike's outcomes: the checksum, the download-and-hash rule, the
  definitive-rejection list, the skew tolerance, and partial-file
  handling.
- Every owner decision (answers 1–14) is fixed. Don't reopen them.
- Where the spike changes an interface, update the affected tasks in this
  plan in the same commit.

**The controller approves** by recording approval in the spec and the ADR.

**Acceptance criteria:**

- Every D-section has a final decision.
- Every command, event, and error code the later tasks use is named, with
  its payload.
- `just check-hosts` passes.

**Commands:**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
just check-hosts
```

### Task 5: Capability traits, the capability matrix, and its commands

**Owner:** Backend. **Depends on:** Tasks 2 and 4.

**Files:**

- New `src-tauri/src/connections/capabilities.rs`:
  - the traits `ArtifactStaging { upload, locate }`,
    `PrintControl { start, pause, resume, cancel }`,
    `HostStateQuery { host_job_state, job_history }`, and
    `CameraDiscovery { cameras }`, all `async_trait` and `Send + Sync`;
  - `CommandFailure { Definitive(String), Indeterminate(String) }`;
  - `StagedArtifact { host_path, sha256, size }`;
  - `HostJobState`, holding `print_stats` state and filename, and **every
    tool** as `Vec<ToolTemperature>` plus the bed. Reuse #29's
    `ToolTemperature`, and never keep a single "nozzle" field;
  - the ts-rs types `CapabilityKey`, `CapabilityState`,
    `PrinterCapabilities`, and `CapabilityEvidence { source, tier:
    Sim | ReadOnlyHardware, verified_host_versions }`.
- `adapters.rs`: `AdapterDescriptor` gains `staging`, `control`,
  `host_state`, and `camera` (each an `Option` of a builder fn), plus
  `evidence`. Moonraker and OctoPrint have all four as `None` for now,
  and every write capability is `notVerified`.
- `capabilities_for(printer, host_facts) -> PrinterCapabilities`:
  - no Connection gives all `unsupported`, with an "adapter" detail of
    "No Connection";
  - an unknown kind gives `adapter`;
  - a missing builder, or `evidence` with no row, gives `notVerified`;
  - `useTls: true` gives every write capability as `notVerified` with the
    detail "TLS connections are not supported yet." (reuse
    `TLS_UNSUPPORTED_MESSAGE`);
  - host facts saying `virtual_sdcard` is missing give `upload` and
    `start` as `host`; missing `pause_resume` gives `pause` and `resume`
    as `host`; no webcams gives `camera` as `host`;
  - a `ReadOnlyHardware` tier can never produce `supported` for upload,
    start, pause, resume, or cancel.
- Host-fact derivation (pure) from a Moonraker `printer.objects.list` and
  `server.webcams.list` response. It collects every `extruder*` object.
- Commands `printer_capabilities(printerId)` and
  `adapter_capability_matrix()`, registered per Global Constraint 7.

**Acceptance criteria:**

- A registry consistency test fails if a capability is `supported` with
  no builder, or has a builder with no evidence.
- Contracts regenerate cleanly.

**TDD tests (write first):**

- One `capabilities_for` test per rule above.
- Host-fact derivation for one extruder, and for four extruders reported
  out of order (`extruder2`, `extruder`, `extruder3`, `extruder1`).
- Serde round-trip snapshots.
- The two commands through the `tauri::test` IPC path (copy the pattern
  in `tests/p2_contract_path.rs`).

**Commands:**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
just test-rust
just gen-contracts && git diff --stat src/generated   # commit the regenerated files
just check-hosts
```

### Task 6: `host_operations` schema, state machine, and repository

**Owner:** Backend. **Depends on:** Task 4.

**Files:**

- `src-tauri/migrations/0007_p6_host_operations.sql`: a STRICT table with
  these columns:
  - `id` (CHECK `GLOB 'hop-*'`);
  - `printer_id` (FK `printers(id)` `ON DELETE RESTRICT`);
  - `kind` (CHECK in `upload`, `start`, `pause`, `resume`, `cancel`);
  - `slice_revision_id` (FK `ON DELETE SET NULL`), `gcode_sha256`,
    `gcode_size`, `host_path`;
  - `endpoint_json` (valid JSON);
  - `state` (CHECK in `dispatching`, `uncertain`, `reconciling`,
    `succeeded`, `failed`, `abandoned`);
  - `failure_json`, `resolution_json`;
  - `attempts`, `last_attempt_at`, `last_attempt_error`;
  - `abandoned_at`, `abandon_note`;
  - `created_at`, `dispatched_at`, `resolved_at`.

  It also adds:
  - a partial unique index on `printer_id WHERE state IN
    ('dispatching','uncertain','reconciling')`;
  - a `BEFORE UPDATE` trigger raising when `OLD.state` is terminal;
  - a rebuild of the `operations` ledger table adding the new kinds
    (`stageSliceRevision`, `startStagedArtifact`, `pauseHostPrint`,
    `resumeHostPrint`, `cancelHostPrint`, `abandonHostOperation`), with
    create, copy, drop, and rename exactly as `0006_p5_slicing.sql` does.
- `persistence/migrations.rs`: register 0007 and set
  `CURRENT_SCHEMA_VERSION = 7`. Tests read the constant.
- `spools/operations.rs`: the new `OperationKind` variants.
- `host_ops/state.rs`: a pure `transition(from, event) -> Result<State,
  IllegalTransition>` for the table in D3 (reproduced here):
  - `dispatching` goes to `succeeded`, `failed`, or `uncertain`;
  - `uncertain` goes to `reconciling` or `abandoned`;
  - `reconciling` goes to `succeeded`, `failed`, or `uncertain`;
  - every other pair is illegal.
- `host_ops/repository.rs`: `insert_dispatching`, `transition` (uses
  `state.rs`), `load`, `list_for_printer`, `list_unresolved`,
  `mark_dispatching_uncertain` (the startup step),
  `delete_terminal_for_printer`, and `has_unresolved(printer_id)`.
- `host_ops/mod.rs`: the module and the domain types, ts-rs exported
  (`HostOperation`, `HostOperationState`, `HostOperationKind`).

**Acceptance criteria:**

- Every legal transition persists, and every illegal one is rejected
  before SQL.
- The trigger stops an update to a terminal row.
- A second unresolved row for the same Printer violates the index.
- An operation-ledger replay returns the same `hop-*`.
- No column can hold a credential.

**TDD tests:**

- `state.rs`: the whole transition table (every pair).
- Repository tests over `crate::test_storage()`.
- `tests/p6_migration.rs`: migrate a P5-shaped database (copy
  `tests/p5_migration.rs`), assert that existing `operations` rows
  survive, then restart.
- A schema test asserting that no column name contains `credential`,
  `secret`, or `key`.

**Commands:**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
just test-rust
just gen-contracts && git diff --stat src/generated
just check-hosts
```

### Task 7: Guards: lifecycle, Connection, import, and Slice Revision

**Owner:** Backend. **Depends on:** Task 6. "Unresolved" means a
`host_operations` row in `dispatching`, `uncertain`, or `reconciling`.

**Files:**

- `printers/lifecycle.rs`:
  - add `LifecycleBlockerCode::HostOperationUnresolved`;
  - add `host_ops::guards::HostOperationBlockers` to `blocker_sources()`.
    It blocks **Archive** and **Delete** while unresolved, with the
    messages "Finish or abandon the pending printer operation before
    archiving." and "…before deleting."
- `printers/repository.rs`:
  - `set_connection` gets a check, inside its transaction, that returns
    the new `RepositoryError::ConnectionInUse` when the Printer has an
    unresolved row **and** the new config changes `kind`, `host`, `port`,
    or `use_tls`, or clears the Connection, or clears the credential
    reference;
  - replacing the credential reference with a new one, endpoint
    unchanged, is **allowed**;
  - `replace_all` (import) rejects the whole import if any unresolved row
    exists;
  - `delete` calls `delete_terminal_for_printer` in the same transaction,
    after the blocker check.
- `contracts/command.rs`: `ErrorCode::ConnectionInUse`, mapped from
  `RepositoryError::ConnectionInUse` in `CommandError::from_repository`.
- `slicing/blockers.rs`: register
  `UnresolvedHostOperationBlocksRevisionDeletion` in
  `slice_revision_blocker_sources()`. A revision with an unresolved
  upload or start cannot be deleted.
- The frontend needs no new component. The existing eligibility UI shows
  the new blocker message.

**Acceptance criteria**, each blocked while unresolved and allowed after
`succeeded`, `failed`, or `abandoned`:

- archive;
- delete;
- import;
- endpoint change;
- Connection clear;
- credential clear;
- deleting the referenced revision.

Also: a credential replacement on the same endpoint succeeds; permanent
delete removes only that Printer's terminal rows; and
`printer_lifecycle_eligibility` reports the blocker.

**TDD tests:**

- One test per mutation, per unresolved state and per terminal state.
- A race test: insert an operation and delete the Printer from two
  threads. Exactly one wins, and no orphan row remains.
- Credential cleanup (`retry_pending_credential_cleanup`) never deletes
  the credential of a Printer with an unresolved row.
- `tests/p6_guards.rs` drives `archive_printer`, `delete_printer`,
  `set_printer_connection`, `clear_printer_connection`, and
  `import_printers` through the IPC path.

**Commands:**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
just test-rust
just gen-contracts && git diff --stat src/generated
just check-hosts
```

### Task 8: Moonraker capability adapter and `FakeMoonraker`

**Owner:** Protocol. **Depends on:** Tasks 3, 4, and 5. Use the spike
report's recorded shapes for fixtures. Scrub any real-host capture
(Global Constraint 2).

**Files:**

- `Cargo.toml`: add the `multipart` and `stream` features to the existing
  `reqwest` dependency. Keep `default-features = false`, with no TLS
  feature.
- New `connections/moonraker/files.rs`, pure with no I/O:
  - the upload form parts (`file`, `root=gcodes`, `path=farm3d`, and
    `checksum` if the spec kept it). There is **no API to add a `print`
    field**;
  - metadata, `files.list`, `history.list`, and `print_stats` parsers;
  - `HostJobState` with every `extruder*` tool as `ToolTemperature` in
    index order, plus the bed;
  - response classification into `CommandFailure::Definitive` or
    `Indeterminate`, per the spec's list.
- New `connections/moonraker/control.rs`:
  - implements `ArtifactStaging`, `PrintControl`, `HostStateQuery`, and
    `CameraDiscovery` over `reqwest` (HTTP) and the existing WebSocket
    framing (`printer.print.*`, `printer.objects.query`,
    `server.history.list`, `server.webcams.list`);
  - the same HTTP client rules as the OctoPrint adapter: no redirects, no
    system proxy, and `X-Api-Key` marked sensitive;
  - timeouts: connect 5 s; upload per the spec; RPC 10 s. A timeout is
    `Indeterminate`.
- `adapters.rs`: the Moonraker descriptor gains the four builders. The
  `evidence` row stays `notVerified` until Task 12.
- New `tests/common/fake_moonraker.rs`: an in-process HTTP and WebSocket
  server on a loopback `TcpListener`. It stores uploads in memory and
  records every request. Scripted faults:
  - store the file, then drop the response;
  - store a partial file;
  - delay past the timeout;
  - 401;
  - apply a start, then drop the response;
  - reject a start definitively (with the spike's error shape);
  - "restart": clear live state but keep files and history.

  It fails the test if any upload has a `print` field.

**Acceptance criteria:**

- The pure functions are covered by fixtures from the spike's simulator
  captures.
- The adapter passes every `FakeMoonraker` scenario.
- No code path can put `print` into an upload.
- The seeded-secret test: an API key never appears in any error string or
  `Debug` output.

**TDD tests:**

- Unit tests in `files.rs` (including "the built form has no `print`
  field" and four-tool parsing).
- `tests/p6_moonraker_adapter.rs` against `FakeMoonraker`: every trait
  method's success path, and every scripted fault's
  `Definitive`/`Indeterminate` classification.

**Commands:**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
just test-rust
just check-hosts
```

### Task 9: Executor, reconciler, Start rule, abandon, events, and commands

**Owner:** Backend and Wiring. **Depends on:** Tasks 5, 6, 7, and 8.

**Files:**

- `host_ops/executor.rs`: `run(operation)` does a write-ahead
  `insert_dispatching` (commit), calls the capability trait, then commits
  `succeeded`, `failed` (on `Definitive`), or `uncertain` (on
  `Indeterminate`, a timeout, or a panic). Fault points are injectable
  for tests: before send, after send, and after the response but before
  commit.
- `host_ops/reconciler.rs` applies the D5 rules:

  | Kind | Proved applied | Proved not applied |
  |---|---|---|
  | upload | `locate` present, size matches, and the hash matches if checked | absent |
  | start | `print_stats.filename == host_path` and the state is printing, paused, or complete, or a history job for `host_path` started after `dispatched_at` (within the spec's skew tolerance) | standby with no such job |
  | pause/resume/cancel | observed state matches | observed state proves no effect |

  - Anything else is back to `uncertain`, with `attempts` and
    `last_attempt_error` bumped.
  - It **never** issues a write.
  - Triggers:
    - startup, after `restore_persisted_connections` in `lib.rs`: first
      `mark_dispatching_uncertain`, then reconcile each unresolved row;
    - when `ConnectionManager` publishes Online for a Printer (a hook);
    - `reconcile_host_operation`.
  - It serializes per Printer and backs off with
    `supervisor::backoff_delay`.
- `host_ops/start_rule.rs` (pure) implements the Start table:

  | Printer `OperationalState` | Allowed `priorState` |
  |---|---|
  | `Ready` | `ready` |
  | `Finished` | `finished` |
  | `Cancelled` | `cancelled` |
  | `Failed`, `Printing`, `Paused`, `Busy`, `Offline`, `Connecting`, `Unknown`, `Error`, `SetupIncomplete`, or stale freshness | none |

  `check(status, prior_state) -> Result<(), StartRejection { NotAllowed
  { observed }, PreconditionChanged { observed } }>`.
- `host_ops/commands.rs`, registered per Global Constraint 7:
  - `stage_slice_revision(operationId, printerId, sliceRevisionId)`;
  - `start_staged_artifact(operationId, printerId, hostOperationId,
    priorState)`: re-reads the Printer's live status from
    `ConnectionManager`, applies `start_rule::check` **before** the
    write-ahead, and writes no row on rejection;
  - `pause_host_print`, `resume_host_print`, `cancel_host_print`
    `(operationId, printerId)`;
  - `reconcile_host_operation(hostOperationId)`;
  - `abandon_host_operation(operationId, hostOperationId, acknowledgement:
    "hostStateUnknown", note?)`: allowed only from `uncertain` with
    `attempts >= 1`, otherwise `HOST_OPERATION_NOT_ABANDONABLE`;
  - `list_host_operations(printerId?)`.

  Every capability command checks `capabilities_for` first and returns
  `CAPABILITY_UNSUPPORTED` with **no** row written.
- New error codes in `contracts/command.rs`: `CapabilityUnsupported`,
  `HostOperationNotAbandonable`, `StartNotAllowed`, and
  `StartPreconditionChanged`, each with a `RecoveryCode` where one
  applies.
- `host_ops/events.rs`: the `hostOperations` stream (type prefix
  `hostOperations.`), `changed` events after commit, and a backfill
  command, using the same envelope and sequence as `slicing/events.rs`.
- `lib.rs`: add `HostOperationServices` to `RuntimeServices` and wire the
  startup order.

**Acceptance criteria:**

- No code path issues a second upload or start by itself.
- The restart matrix below holds.
- Credentials appear in no event, error, or row.

**TDD tests** (`tests/p6_host_ops.rs`, against `FakeMoonraker` and a
rebuilt `RuntimeServices` over the same roots for "restart"):

| Fault | Kind | Expected |
|---|---|---|
| Crash after write-ahead, before send | upload | `uncertain` → reconcile → absent → `failed{notApplied}`; no upload seen |
| Response lost after store | upload | `uncertain` → `succeeded`; exactly one upload seen |
| Partial file | upload | per the spec's partial-file rule |
| Start applied, response lost | start | `uncertain` → `succeeded`; exactly one start seen |
| Start definitively rejected | start | `failed`, not reconciled |
| Host unreachable throughout | any | stays `uncertain`; abandon allowed; guards lift after abandon |
| Host printing a different file | start | stays `uncertain`; no start sent |

Also test:

- `start_rule` unit tests, one per table row. `Finished` + `finished` is
  OK; `Finished` + `ready` gives `PreconditionChanged`; `Failed` +
  anything gives `NotAllowed`.
- Four-tool status inputs.
- Command-level Start tests: no row on `START_NOT_ALLOWED` or
  `START_PRECONDITION_CHANGED`, and allowed again after `Failed` clears
  to `Ready`.
- `CAPABILITY_UNSUPPORTED` for an OctoPrint Printer, with no row.
- Operation-id replay for every command.
- Listen-before-backfill ordering.
- The seeded-secret scan over events, errors, and rows.

**Commands:**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
just test-rust
just gen-contracts && git diff --stat src/generated
just check-hosts
```

### Task 10: Frontend stores, the Start rule, and presentation

**Owner:** Frontend. **Depends on:** the Task 5 and Task 9 contracts
(already generated in `src/generated/`).

**Files:**

- `src/host-ops/capabilities-store.ts`: `PrinterCapabilities` per Printer
  from `printer_capabilities`, refetched when the Printer's status
  changes.
- `src/host-ops/host-operations-store.ts`: the `hostOperations` stream
  through `src/ipc/sequenced-stream.ts`, listening before backfill (copy
  `src/slicing/slicing-store.ts`).
- `src/host-ops/host-operations-store-mock.ts` and `web-fixtures.ts` for
  `just web`:
  - a Ready single-tool Moonraker Printer;
  - a four-tool Moonraker Printer;
  - an OctoPrint Printer (every write `notVerified`);
  - a `Finished` Printer with a staged artifact;
  - a `Failed` Printer;
  - a Printer with an `uncertain` upload.
- `src/host-ops/start-rule.ts`, a pure mirror of the backend table.
  `startOffer(status) -> { offered: false, reason } | { offered: true,
  priorState, confirmLabel }`:
  - `ready` → "The bed is clear.";
  - `finished` → "The previous print finished. The bed is clear.";
  - `cancelled` → "The previous print was cancelled. The bed is clear.";
  - `Failed` → not offered, "Clear the error on the printer first.";
  - other states → not offered, with the state as the reason.
- `src/host-ops/presentation.ts`, pure:
  - Host Operation state labels (Uploading, Staged, Uncertain, Checking,
    Failed, Abandoned) with severity;
  - `CapabilityState` labels ("Supported", "Not supported by
    <adapter>", "Not verified yet", "Not available on this printer");
  - unsupported and failed use **different** copy and severity.

**Acceptance criteria:**

- The stores apply events in sequence and discard duplicates.
- `start-rule.ts` agrees with every backend `start_rule` row.
- No credential field exists in any store type.

**TDD tests (Vitest):**

- Stream ordering and backfill.
- `startOffer` for every `OperationalState`, and for stale freshness.
- Presentation for every state.
- Unsupported and failed never share copy.

**Commands:**

```sh
just build
just test
```

### Task 11: Frontend UI: Job tab, Stage, Start, Abandon, and Capabilities

**Owner:** Frontend. **Depends on:** Task 10.

**Files:**

- `src/screens/PrinterJobPanel.tsx` and `.module.css`, added as a **Job**
  tab in `PrinterDetailDock.tsx` (today: Status, Setup). It shows:
  - the host's current print from telemetry: file, state, progress, and
    every tool's temperature/target from `telemetry.tools` (fall back to
    `nozzle_*` when `tools` is empty) plus the bed;
  - **Pause**, **Resume**, and **Cancel print…** (cancel confirms), shown
    only when the capability is supported;
  - **Staged on this Printer**: Host Operations with state, and
    **Check again**, **Abandon check…**, and **Start…**.
- `src/screens/StartStagedDialog.tsx`: a Kobalte Dialog.
  - The checkbox label comes from `startOffer(status).confirmLabel`, and
    Confirm is disabled until it is ticked.
  - It sends `priorState`.
  - On `START_PRECONDITION_CHANGED` or `START_NOT_ALLOWED` it clears the
    tick and re-renders for the new status. A tick is never reused.
  - This holds for both `StartSafety` values.
- `src/screens/StageOnPrinterDialog.tsx`: opened from **Stage on
  Printer…** in `SliceRevisionReview.tsx`, using Kobalte Dialog and
  Select.
  - Printers whose `upload` is unsupported are listed but disabled, with
    the capability reason.
  - Offline Printers are disabled with "Offline", which is different
    copy.
  - **Add to Queue…** stays disabled.
- `src/screens/AbandonReconciliationDialog.tsx`: a Kobalte AlertDialog.
  - Required checkbox: "I understand the printer may still have this
    file or be printing it."
  - Body names the Printer and file, and states that farm3d will stop
    checking.
- `src/screens/CapabilityList.tsx`: a read-only list in the Setup tab.
  Each capability shows its label and evidence tier.
- `PrinterConnectionPanel.tsx`: show `CONNECTION_IN_USE` with a link to
  the Job tab.

**Acceptance criteria:**

- An unsupported control is never an enabled button.
- A failure is an inline `role="alert"` with a recovery action.
- Start follows the table: offered for Ready, Finished, and Cancelled
  (each with its own label), and disabled with a reason otherwise.
- Abandon and Start cannot be confirmed without their checkboxes.
- Everything is keyboard-operable.
- Layout works at 1440 × 900 and 1024 × 700.
- Tokens, CSS Modules, and Kobalte only.

**TDD tests** (`@solidjs/testing-library`, with
`pointerDown`/`pointerUp` for Select and Menu):

- One Start test per table row, with exact labels.
- The command receives the matching `priorState`.
- A `Failed` → `Ready` status event enables Start without a reload.
- `START_PRECONDITION_CHANGED` clears the tick.
- The Stage dialog's unsupported and Offline reasons differ.
- Abandon cannot be confirmed unticked.
- A four-tool Printer shows four tools.
- `CONNECTION_IN_USE` rendering.

**Commands:**

```sh
just build
just test
just web      # manual check at /#showcase and the Job tab with the web fixtures
```

### Task 12: Simulator evidence and the read-only real-host suite

**Owner:** Protocol and Wiring. **Depends on:** Tasks 1, 8, and 9.

**Files:**

- `src-tauri/tests/sim/toxiproxy.rs`: add `cut_request_after(proxy,
  bytes)`, an **upstream** `limit_data` toxic. The existing toxics are
  downstream only.
- `src-tauri/tests/sim_moonraker.rs`: a P6 section. Every test is
  `#[ignore]`, takes `sim::exclusive()`, calls `reset()` first, and uses
  `require_sim!`. It drives `host_ops` through a `RuntimeServices` whose
  Printer points at `sim.config()`.
  - **Safety precondition before any start:** read every heater from
    `sim.heaters()` (every `extruder*` and `heater_bed`) and refuse if any
    target is non-zero. Also refuse if the host is printing or paused.
    `Finished` and `Cancelled` are allowed starting states and pass the
    matching `priorState`.
  - Upload uses a no-motion fixture: comments and `M117` only, with no
    `M104`, `M109`, `M140`, `M190`, or `T<n>`.
  - Scenarios:
    - stage, then `cut_after(Moonraker, 0)`, which loses the response
      after the host stored the file;
    - rebuild the services (a restart), reconcile, and expect `succeeded`
      with exactly one file;
    - `cut_request_after` mid-body, then reconcile per the spec;
    - start with the response cut, then reconcile to `succeeded`;
    - pause, resume, and cancel;
    - a second start from `Finished` with `priorState: finished`
      (answer 13);
    - `set_enabled(false)` for longer than the backoff, then abandon;
    - Klipper restart mid-print;
    - capability detection on `moonraker`, `moonraker-multi` (four
      tools), `variant no-bed`, and `variant apikey`.
- New `src-tauri/tests/p6_moonraker_readonly.rs`, the real-hardware tier
  (`#[ignore]`):
  - It reads `FARM3D_MOONRAKER_HOST` (required, with no default),
    `FARM3D_MOONRAKER_PORT` (default 7125), `FARM3D_MOONRAKER_API_KEY`,
    and `FARM3D_MOONRAKER_API_KEY_FILE`, as `a0_moonraker_live.rs` does.
  - It builds its client through a **read-only wrapper** that exposes
    only `HostStateQuery`, `ArtifactStaging::locate`, `CameraDiscovery`,
    probe, and subscribe. It records every request, and fails on any
    HTTP `POST` or `DELETE` or any `printer.print.*` RPC.
  - Tests:
    - probe and capability detection;
    - `host_job_state` with every tool;
    - `locate` of a never-sent `farm3d/<uuid>.gcode` is absent;
    - a seeded local `uncertain` upload row for that path reconciles to
      `failed{notApplied}`;
    - blocking the port on the farm3d machine keeps the row `uncertain`,
      and abandon is local only;
    - each test asserts zero recorded writes.
  - It must never print or write the host value.
- `justfile`: a new recipe `p6-readonly` that fails without
  `FARM3D_MOONRAKER_HOST` and runs `cargo test --test
  p6_moonraker_readonly -- --ignored --test-threads=1 --nocapture`. Also
  add `p6_*` sim tests to `test-sim` if they live in a new file (prefer
  `sim_moonraker.rs`).
- `adapters.rs`: flip each Moonraker write capability to `supported` or
  `adapter` with `tier: Sim` and `source` naming this run's manifest.
  Read-only results may add `host` facts and `verified_host_versions`
  only.

**Acceptance criteria:**

- Every D5 row is observed on the simulator, with the manifest path
  recorded.
- The read-only suite passes with zero recorded writes, or is recorded as
  Unavailable.
- `just test-rust` (CI) passes without simulators.
- No host value appears in any committed file (`just check-hosts`).

**Commands:**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
just sim-up
FARM3D_SIM_REQUIRED=1 just test-sim
just sim-down
FARM3D_MOONRAKER_HOST=<U1 host> just p6-readonly     # optional, owner-run; never commit the value
just test-rust
just check-hosts
```

### Task 13: Tracer

**Owner:** Wiring. **Depends on:** Tasks 9, 11, and 12.

**Files:**

- `src-tauri/tests/p6_tracer.rs`: one test, run **twice**:
  - against `FakeMoonraker` in the normal suite (CI);
  - as `#[ignore]` against the simulator under `just test-sim`, which is
    the evidence run.

  Steps:
  1. Import `tests/fixtures/library/plain.gcode` and create an external
     Slice Revision from it. Record its sha256. On the simulator, use the
     no-motion fixture from Task 12 instead, since `plain.gcode` may
     contain motion or heating.
  2. `stage_slice_revision` with the response cut (the fake's
     drop-after-store fault, or `cut_after(Moonraker, 0)` on the sim).
  3. Assert that archive, delete, Connection clear, and revision delete
     are blocked.
  4. Restart: rebuild `RuntimeServices` over the same roots.
  5. Reconcile. Assert `succeeded`, exactly one file at
     `farm3d/<slr-id>.gcode` with a matching hash, **no** second upload,
     and **no** start (the host is still standby; no new history job).
  6. Assert that the guards have lifted.
  7. Repeat with the host unreachable (the fake down, or
     `set_enabled(false)`), then abandon. The guards lift, and the row is
     kept as `abandoned`.
- `p6_moonraker_readonly.rs`: add `readonly_reconciliation_tracer`. It
  does **no upload**:
  1. Seed an `uncertain` upload row for a never-sent path.
  2. Restart.
  3. Reconcile against the real host, expecting `failed{notApplied}`.
  4. Assert that the guards lifted, with zero writes recorded.

**Acceptance criteria:** both runs pass, and the sim run's manifest path
is recorded for Task 14.

**Commands:**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
just test-rust
just sim-up && FARM3D_SIM_REQUIRED=1 just test-sim && just sim-down
just check-hosts
```

### Task 14: Documentation and verification record

**Owner:** Wiring; a fresh verification pass follows. **Depends on:** all.

**Files:**

- `docs/verification/2026-09-2x-p6-moonraker-commands.md`, in the P2 and
  P5 verification shape. It maps every #16 acceptance criterion to
  evidence: test names, the sim manifest path, and read-only results
  (Unavailable if not run), with no host values.
- `docs/screenshots/p6-*.png` at 1440 × 900 and 1024 × 700:
  - the Job tab;
  - Stage;
  - an uncertain row;
  - Check again;
  - Abandon;
  - Start from Finished;
  - Start disabled after Failed;
  - the blocked Connection edit;
  - blocked archive;
  - the Capabilities list.

  Every write action targets a **simulator** Printer.
- `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`:
  close the "Adapter command/camera capabilities" row for Moonraker only.
- `CONTEXT.md`, if Task 4 left anything out.

**Acceptance criteria:**

- Every gate in the commands below passes, or is recorded as unavailable
  with a reason.
- Nothing in the record names an owner host.

**Commands:**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
just build
just test
just test-rust
just gen-contracts && git diff --exit-code src/generated
just sim-up && FARM3D_SIM_REQUIRED=1 just test-sim && just sim-down
just package        # then, from the installed .deb, stage one revision to the simulator
just dev            # manual pass; needs a display
just check-hosts
```

## Delivery order

```text
Task 1 (sim cleanup) ─> Task 3 (spike) ─> Task 4 (spec + ADR) ─┬─> Task 5 (capabilities) ─┬─> Task 8 (adapter) ─┐
Task 2 (registry) ─────────────────────────────────────────────┘                          │                     ├─> Task 9 (executor) ─┬─> Task 12 (sim + read-only) ─> Task 13 (tracer) ─> Task 14
                                               Task 4 ─> Task 6 (schema) ─> Task 7 (guards) ┘                                          └─> Task 10 (stores) ─> Task 11 (UI) ──────┘
```

| # | Task | Ready | Depends on |
|---|---|---|---|
| 1 | Retire moonraker-sim; fold variants; OctoPrint sim tests | **Now** | — |
| 2 | Adapter registry on #27 | **Now** | — |
| 3 | Moonraker command spike | After 1 (read-only checks now) | 1 |
| 4 | Spec + ADR-0011 | After 3 | 3 |
| 5 | Capability traits and matrix | After 4 | 2, 4 |
| 6 | Schema, state machine, repository | After 4 | 4 |
| 7 | Guards | After 6 | 6 |
| 8 | Moonraker adapter + FakeMoonraker | After 5 | 3, 4, 5 |
| 9 | Executor, reconciler, Start rule, commands | After 8 | 5–8 |
| 10 | Frontend stores and Start rule | After 9's contracts | 5, 9 |
| 11 | Frontend UI | After 10 | 10 |
| 12 | Sim evidence and read-only suite | After 9 | 1, 8, 9 |
| 13 | Tracer | After 11, 12 | 9, 11, 12 |
| 14 | Docs and verification | Last | all |

Tasks 1 and 2 run in parallel. Tasks 6 and 7 run in parallel with Tasks 5
and 8. Tasks 10–11 run in parallel with Task 12.

## Exit gate (issue #16, Moonraker)

- [ ] An evidence-backed Moonraker capability row, with `tier: sim` and a
  manifest citation (Tasks 3, 5, 12).
- [ ] Simulator command tests cover uncertainty, interruption, restart,
  and reconciliation. Read-only real-host results are recorded, with zero
  writes (Tasks 9, 12).
- [ ] Archive/delete, import, and Connection/credential mutations cannot
  orphan an unresolved operation, and they unblock only after resolution
  or abandonment (Tasks 7, 9, 13).
- [ ] The UI distinguishes unsupported from failed, and follows the Start
  table (Tasks 10, 11).
- [ ] The tracer completes on the simulator with no duplicate upload and
  no accidental start. The read-only variant completes with no upload
  (Task 13).
