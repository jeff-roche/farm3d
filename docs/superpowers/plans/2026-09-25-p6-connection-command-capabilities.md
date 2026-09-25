# P6 Connection Command Capabilities Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Draft; the open questions are answered, and the plan is not yet
approved. GitHub issue #16. Written on 2026-09-25 against `main` at
`5c9ead7`, with P5 (#15) merged and closed. A0.1 (#9, Moonraker live
validation) is **open**, so no Moonraker command behavior in this plan is
evidenced yet.

**User answers (2026-09-25).** These are fixed:

1. **Start in the P6 UI:** yes. **Start…** is on the Job tab, always
   behind a "bed is clear" confirmation (D9).
2. **Exclusivity:** at most one unresolved write per Printer (D2).
3. **Archive while a write is unresolved:** blocked (D7).
4. **Credential replacement while a write is unresolved:** allowed on the
   same endpoint, still probed (D7).
5. **Permanent Printer delete:** also deletes that Printer's finished
   (terminal) Host Operation rows, in the same transaction (D7).
6. **TLS:** staging over a `useTls` Connection stays unsupported until a
   TLS host is tested (D6).
7. **Evidence source:** a containerized Klipper + Moonraker with a
   simulated board counts as live evidence for #9 and for P6's Moonraker
   gates. The shared simulator harness is being built on
   `feature/printer-simulation-harness` (`sim/compose.yaml`,
   `just sim-up`, `just test-sim`, and its own ADR). The owner's
   Snapmaker U1 running Moonraker at `<U1 host>:7125` is available as
   a real-hardware tier (§Test tiers).
8. **The U1 is read-only (owner decision).** Agents must never write to
   the real U1. That rules out file uploads, interrupted uploads, file
   deletion, and `start`, `pause`, `resume`, and `cancel`, with or without
   an opt-in. The U1 is used only for read-only corroboration: probe,
   subscribe, status and file-list queries, and identity and capability
   detection. Every write gate runs on the simulator only.

P6 is planned and gated **per adapter**. This plan covers the shared
foundation plus the **Moonraker** path only. OctoPrint and ElegooLink get
their own short adapter plans later (see §Other adapters). Like P5, the
work has three stages, and each ends with an approval stop:

| Stage | Output | Stop |
|---|---|---|
| A. Moonraker command spike (Task 1) | `docs/superpowers/baselines/2026-09-2x-p6-moonraker-command-spike.md` | The user approves the spike report |
| B. Focused spec (Task 2) | `docs/superpowers/specs/2026-09-2x-p6-connection-command-capabilities-design.md` and ADR-0011 | The user approves the spec |
| C. Implementation (Tasks 3–13) | Code, tests, docs, verification record | Exit gate below |

Some Stage C tasks do not depend on live evidence and may start before
Stage A finishes. §Delivery order marks each task **Ready now**,
**After spec**, or **Needs the sim harness**. The U1 is used for read-only
checks only (answer 8).

**Goal:** For Moonraker, farm3d can:

- upload (stage) one immutable Slice Revision to a Printer **without
  starting it**;
- start a staged file, and pause, resume, and cancel the host's print;
- read the host state and artifact identity it needs to reconcile;
- report camera availability as a capability (no media; that is P8).

Every write to a host carries a durable **Host Operation** identity. When a
response is lost, the operation becomes **uncertain**, and farm3d
reconciles it against the host after reconnect or restart. It never
re-uploads or re-starts blindly. Printer archive/delete, Printer import,
Connection changes, and credential deletion cannot orphan an unresolved
operation. For a host that is gone for good, the operator can explicitly
**abandon reconciliation**, and that choice is recorded durably.

P6 ends at capability-aware, operator-invoked host control. It creates no
Queue Entries or Jobs, has no scheduling or unattended start, and does no
material accounting (P7).

**Architecture:** Rust owns the operation record, the adapters, and
reconciliation.

- **Adapter seam.** The observation-only `PrinterConnection` trait stays as
  it is. P6 adds small **capability traits** (artifact staging, print
  control, host-state query, camera discovery). An **adapter registry**
  builds them per Connection kind and derives an evidence-backed
  **capability matrix** (D1, D6).
- **Host Operations.** A new `src-tauri/src/host_ops/` module holds the
  `host_operations` table, a pure state machine, the executor
  (write-ahead intent, then wire I/O, then resolution), the reconciler, and
  a `hostOperations` event stream (D2–D5).
- **Guards.** New `LifecycleBlockerSource`, `SliceRevisionDeletionBlocker`,
  and Connection-mutation checks run inside the transactions they gate
  (D7).
- **Frontend.** A capability store and a host-operations store in
  `src/host-ops/`. A new **Job** tab in `PrinterDetailDock`, a
  **Stage on Printer…** action in `SliceRevisionReview`, an Abandon dialog,
  and a capabilities section in Setup (D9).

**Tech stack:** Rust, Tauri 2, rusqlite/SQLite, tokio, tokio-tungstenite
(existing WebSocket), `reqwest` 0.13 for HTTP upload and download (already
in `Cargo.lock` through Tauri; P6 adds it as a direct dependency with
`multipart` and `stream`), ts-rs, SolidJS, TypeScript, Kobalte, CSS
Modules, Vitest, and Rust unit and integration tests.

## Evidence gathered while planning

These facts come from the repository at `5c9ead7` and from the issues on
2026-09-25. Moonraker API facts below come from its documentation and are
**not** live evidence. Task 1 must confirm each one on a real instance.

**Issue state.**

- #15 (P5) is closed. P5 landed `slice_revisions` (`slr-*` ids, immutable
  by trigger), content-addressed G-code in `content_blobs`
  (`gcode_sha256`, `gcode_size`), `ContentStore::open_verified`, and the
  empty `slicing::blockers::slice_revision_blocker_sources()` registry.
  That is the artifact identity P6 builds on.
- #9 (A0.1 Moonraker live validation) is open. Nothing records a real
  Moonraker instance, versions, or behavior. P6's Moonraker command work is
  blocked on it (approach doc, issue #16).
- #10 (OctoPrint) is open. There is no OctoPrint code in `src-tauri`.
- #8 (ElegooLink) is open with no go decision.

**The connection seam today** (`src-tauri/src/connections/`):

- `PrinterConnection` has two methods, `probe` and `subscribe`. It is
  observation-only, and its doc comment already expects reshaping.
- `MoonrakerConnection` speaks JSON-RPC over WebSocket only. It has no HTTP
  client. It sends the API key as `X-Api-Key` on the upgrade.
- Framing and parsing are pure and live in `moonraker/protocol.rs`. The
  subscription already asks for `print_stats.filename`, `print_stats.state`,
  `print_duration`, and `message`.
- Construction goes through one factory, `supervisor::build`, and
  `ConnectionManager::connection_for` reuses it for probes. But the
  "is this kind supported?" check is repeated in five places:
  - `connections/commands.rs:300` (`set_printer_connection`);
  - `printers/create.rs:297`;
  - `printers/batch.rs:567`;
  - `printers/setup.rs:48` (`SetupGap::UnsupportedAdapter`);
  - `connections/supervisor.rs:963` (`build`).

  "Centralize adapter construction" in the issue therefore means one
  registry that owns both the kind check and every per-capability builder.
- `ConnectionManager::reconciliation_guard()` already exists. It
  serializes *supervisor* reconciliation. P6's reconciliation is a
  different thing, so this plan always says **host-operation
  reconciliation** to avoid confusion.

**TLS.** `tokio-tungstenite` is built without a TLS feature, so a
`useTls: true` Connection cannot connect today. P6 must not claim TLS
support for staging until the same TLS policy covers WebSocket and HTTP.
The user decided that staging over TLS stays unsupported until a TLS host
is tested (answer 6).

**Persistence and guards.**

- Printer lifecycle eligibility runs through
  `printers::lifecycle::blocker_sources()` inside the gating transaction.
  P2 and P3 registered sources there, and the doc comment reserves the
  slot for later phases.
- Archive keeps the Connection but stops supervision. An archived Printer
  no longer holds its host identity (the partial unique index
  `printers_active_host_identity`), so another Printer may take the same
  host.
- `PrinterRepository::delete` enqueues the credential for cleanup.
  `replace_all` (Printer import) deletes **every** Printer and enqueues
  orphaned credentials. That is a destructive path the issue does not
  name, but it can orphan an operation just as delete can.
- `set_connection` has no blocker hook. Credential cleanup
  (`retry_pending_credential_cleanup_locked`) deletes any queued reference
  that no Printer row still points at.
- The operations ledger (`spools::operations`, `OperationKind`, `claim`)
  gives idempotent command replay by `operationId`. Adding kinds means
  rebuilding the table's CHECK, as `0006_p5_slicing.sql` did.
- The highest migration is `0006_p5_slicing.sql`.

**Frontend.** `PrinterDetailDock` has only **Status** and **Setup** tabs.
The umbrella UI spec calls for **Job** and **Camera** tabs too.
`SliceRevisionReview` shows a disabled **Add to Queue…** with the reason
"The Queue arrives in a later version." The P5 slicing store uses the
sequenced stream (`src/ipc/sequenced-stream.ts`), which listens before it
backfills.

**Moonraker surface to verify in Task 1** (from Moonraker's documentation,
not observed):

- **Upload:** `POST /server/files/upload`, multipart, with fields `file`,
  `root` (`gcodes`), `path`, an optional `checksum` (SHA-256, checked by the
  server), and `print`. farm3d must **never** send `print=true`.
- **Identity:** `server.files.metadata` or `GET /server/files/metadata`
  (size, modified time), `server.files.list`, and a download at
  `GET /server/files/gcodes/<path>`.
- **Control:** `printer.print.start` (with a filename),
  `printer.print.pause`, `printer.print.resume`, and `printer.print.cancel`.
- **Host state:** `print_stats` (`state`, `filename`),
  `virtual_sdcard`, and `server.history.list` (per-job `filename`,
  `start_time`, `status`).
- **Camera:** `server.webcams.list`.

## Proposed decisions

The Task 2 spec makes these final. **(Decided)** marks a point the user
settled on 2026-09-25 (§Status). Moonraker-specific details in D3–D5 may
change with the spike. A change in product behavior needs the user.

### D1. Adapter seam: composable capability interfaces (the issue's decision gate)

The approach doc forbids adding methods before comparing three seams.

| Option | What it is | For | Against |
|---|---|---|---|
| **A. Extend `PrinterConnection`** | Add `upload`, `start`, `pause`, … with default "unsupported" bodies | Smallest diff; one object per Printer | A default body makes "not implemented" look the same as "the protocol can't". Every fake must grow. Observation and writes share one boxed object whose `subscribe` owns a long-lived socket. The trait keeps widening with every adapter. |
| **B. One separate `PrinterCommands` trait** | Read and write are separate; all eight commands in one trait | Clean read/write split | Still all-or-nothing per adapter: OctoPrint may upload but lack camera, and the gaps become `Unsupported` returns again. Camera and host-state queries are not commands. |
| **C. Composable capability traits plus a registry** (recommended) | Small traits, each built only when the adapter provides it | A capability is *present* when a builder exists, so unsupported is structural and cannot be confused with failure. Each trait has its own small fake. Adapters land one row at a time. | More types; a registry to keep honest. |

**Recommendation: C, kept small.** No plugin framework and no dynamic
discovery beyond the host checks in D6.

```rust
// connections/capabilities.rs (sketch; Task 2 fixes the exact signatures)
#[async_trait]
pub trait ArtifactStaging: Send + Sync {
    /// Uploads without starting. Must never ask the host to print.
    async fn upload(&self, artifact: &StagedArtifact, body: ArtifactBody) -> Result<UploadReceipt, CommandFailure>;
    /// Finds a staged artifact by its farm3d identity (D4).
    async fn locate(&self, artifact: &StagedArtifact) -> Result<ArtifactPresence, CommandFailure>;
}
#[async_trait]
pub trait PrintControl: Send + Sync {
    async fn start(&self, artifact: &StagedArtifact) -> Result<(), CommandFailure>;
    async fn pause(&self) -> Result<(), CommandFailure>;
    async fn resume(&self) -> Result<(), CommandFailure>;
    async fn cancel(&self) -> Result<(), CommandFailure>;
}
#[async_trait]
pub trait HostStateQuery: Send + Sync {
    async fn host_job_state(&self) -> Result<HostJobState, CommandFailure>;
    /// Host-recorded job starts for `artifact` at or after `since`.
    async fn job_history(&self, artifact: &StagedArtifact, since: DateTime<Utc>) -> Result<Vec<HostJobRecord>, CommandFailure>;
}
#[async_trait]
pub trait CameraDiscovery: Send + Sync {
    async fn cameras(&self) -> Result<Vec<CameraEndpoint>, CommandFailure>;
}

// connections/adapters.rs
pub struct AdapterDescriptor {
    pub kind: &'static str,
    pub observe: fn(&ConnectionConfig, Option<Zeroizing<String>>) -> Box<dyn PrinterConnection>,
    pub staging: Option<fn(&ConnectionConfig, Option<Zeroizing<String>>) -> Box<dyn ArtifactStaging>>,
    pub control: Option<fn(...) -> Box<dyn PrintControl>>,
    pub host_state: Option<fn(...) -> Box<dyn HostStateQuery>>,
    pub camera: Option<fn(...) -> Box<dyn CameraDiscovery>>,
    pub evidence: CapabilityEvidence, // the recorded row (D6)
}
pub fn registry() -> &'static [AdapterDescriptor];
pub fn descriptor(kind: &str) -> Option<&'static AdapterDescriptor>;
```

- **Wire code stays adapter-local and pure.** Multipart construction,
  RPC framing, and response parsing go in `moonraker/protocol.rs` (or a
  sibling `moonraker/files.rs`) with no I/O. The I/O layer stays thin, as
  today.
- **The supervisor keeps observing.** Command traits are built per
  operation, not held by the supervisor task. A command never shares the
  subscription's socket.
- **The registry replaces the five kind checks** listed in the evidence
  section. `supervisor::build` becomes `registry` lookups, and
  `ConnectionManager::with_clock_and_factory` keeps its test-injection
  seam by taking a registry instead of one factory function.
- A test asserts that each descriptor's `evidence` row agrees with which
  builders are present. A capability marked supported with no builder, or
  a builder with no evidence, fails the build.
- **ADR-0011** records this choice. ADR-0002 (pluggable connectivity)
  stays binding.

### D2. Durable Host Operation identity

A new STRICT table in `0007_p6_host_operations.sql`:

| Column | Meaning |
|---|---|
| `id` | `hop-<uuid>`; the durable identity P7 Jobs will reference |
| `printer_id` | FK → `printers(id)` `ON DELETE RESTRICT` |
| `kind` | `upload`, `start`, `pause`, `resume`, `cancel` |
| `slice_revision_id` | For `upload` and `start`; FK → `slice_revisions(id)` `ON DELETE SET NULL` |
| `gcode_sha256`, `gcode_size` | Copied from the revision so identity survives revision deletion |
| `host_path` | The deterministic staged path (D4) |
| `endpoint_json` | Snapshot of `kind`, `host`, `port`, `useTls` at dispatch. **Never** the credential value, and not the `credentialRef` either (see D7). |
| `state` | See D3 |
| `failure_json`, `resolution_json` | Structured, credential-free evidence |
| `attempts`, `last_attempt_at`, `last_attempt_error` | Reconciliation bookkeeping |
| `abandoned_at`, `abandon_note` | D8 |
| `created_at`, `dispatched_at`, `resolved_at` | Timestamps |

Rules:

- **Write-ahead.** The row is committed in state `dispatching` **before**
  any byte goes to the host. A crash after that commit can only produce
  `uncertain` on restart, never "not sent".
- **One unresolved write per Printer.** A partial unique index on
  `printer_id WHERE state IN ('dispatching','uncertain','reconciling')`.
  This keeps reconciliation unambiguous and makes a second upload while
  one is uncertain structurally impossible. Pause, resume, and cancel for
  an active print are still possible after the upload has resolved.
  **(Decided)**: one per Printer, not one per kind.
- **Idempotent commands.** Every write command takes a client
  `operationId` and claims it in the existing operations ledger (new
  `OperationKind`s, CHECK rebuild as in 0006). A replay returns the
  existing Host Operation.
- Rows are never updated after they reach a terminal state, and nothing
  deletes them except permanent Printer deletion (D7, decided). They are
  history for P7–P9.

### D3. Operation state machine

```text
dispatching ──(host confirms)──────────────> succeeded
     │  └────(host definitively rejects, no side effect)──> failed
     └──(timeout / lost response / crash / restart)──> uncertain
uncertain ──(reconcile begins)──> reconciling
reconciling ──(proved applied)───────> succeeded
            ──(proved not applied)───> failed { notApplied }
            ──(host unreachable, auth, inconclusive)──> uncertain
uncertain ──(operator, risk-confirmed)──> abandoned
```

- `succeeded`, `failed`, and `abandoned` are terminal and immutable
  (trigger, as for Slice Revisions).
- "Definitively rejects" means the protocol says the command had no effect,
  for example an HTTP 4xx before the body is accepted, or a JSON-RPC error
  for `printer.print.start` when Klipper is not ready. Task 1 records
  which responses qualify. Anything else is `uncertain`.
- The state machine is a pure module with an exhaustive transition-table
  test. Illegal transitions are unrepresentable in the repository API.
- **Unsupported is not a state.** A command for a capability the adapter
  lacks never creates a row. It fails before the write-ahead commit with
  `CAPABILITY_UNSUPPORTED` (D6).

### D4. Artifact identity and upload without start

- **Staged path:** `farm3d/<slice-revision-id>.gcode` under the host's
  `gcodes` root. It is deterministic, so a retry targets the same path
  and cannot create a second copy. The id is a UUID, so collisions with
  user files are not a practical concern. The spike confirms Moonraker
  accepts a subdirectory in `path`.
- **Bytes** come from `ContentStore::open_verified(gcode_sha256)` and are
  streamed. farm3d never uploads bytes that fail verification.
- **Integrity at upload:** send `checksum=<sha256>` when the host supports
  it (spike Gate B), so the host rejects a corrupted body.
- **Never start.** The pure request builder has no way to set `print`. A
  unit test asserts the multipart body never contains a `print` field,
  and the fake Moonraker fails any test that sends one.
- **Presence check** (`locate`): metadata at `host_path`. It matches when
  the size equals `gcode_size` and, if the host offers no checksum, a
  download-and-hash equals `gcode_sha256`. Size alone is not proof. Whether
  the hash download is needed depends on spike Gate C.
- **Re-upload after `failed { notApplied }`** is allowed, but only as an
  explicit operator action that creates a new Host Operation. P6 never
  retries a write automatically.

### D5. Reconciliation (per kind, Moonraker)

The reconciler runs:

- at startup, after `restore_persisted_connections`, for every
  unresolved row (a `dispatching` row becomes `uncertain` first);
- when the supervisor reports the Printer `Online` again (a hook on the
  existing health observation);
- when the operator presses **Check again**.

It serializes per Printer and backs off (reusing
`supervisor::backoff_delay`).

| Kind | Proved applied | Proved not applied | Otherwise |
|---|---|---|---|
| `upload` | `locate` = present and matching (D4) | Absent at `host_path` | `uncertain` |
| `start` | `print_stats.filename` = `host_path` and state is printing, paused, or complete, **or** a history record for `host_path` starting after `dispatched_at` | Host standby with no such history record | `uncertain`; if the host is running a *different* file, stays uncertain with a "host is busy with another print" note |
| `pause` / `resume` / `cancel` | Observed state matches the request | Observed state proves the request did not take effect | `uncertain` |

- **A start is never retried automatically** (umbrella UI spec: "never
  automatically starts when it cannot prove the prior command did not take
  effect"). The operator may start again only after `failed { notApplied }`.
- Clock skew between farm3d and the host matters for the history rule. The
  spike measures it, and the spec picks a tolerance.
- A partially written upload: Gate D records whether an interrupted upload
  can leave a file at `host_path`. If it can, a size mismatch counts as
  "not applied" only after farm3d deletes the partial file, and that
  deletion is itself recorded on the operation.

### D6. Capability matrix contract (SolidJS and Jobs)

```ts
type CapabilityKey = "upload" | "start" | "pause" | "resume" | "cancel"
  | "hostState" | "artifactIdentity" | "camera";
type CapabilityState =
  | { status: "supported" }
  | { status: "unsupported"; reason: "adapter" | "notVerified" | "host"; detail: string };
type PrinterCapabilities = {
  printerId: string;
  adapterKind: string | null;
  evidence: { source: string; verifiedHostVersions: string[] } | null;
  capabilities: Record<CapabilityKey, CapabilityState>;
  observedAt: string | null;
};
```

- `adapter`: the protocol cannot do it (recorded with evidence).
- `notVerified`: nobody has produced live evidence yet. OctoPrint and
  ElegooLink rows start here. The UI reads it as unsupported, never as
  "try it".
- `host`: the adapter can, but this host lacks it. Examples: no
  `virtual_sdcard`, `pause_resume` not configured, no webcam configured.
  Derived from the probe and cached with `observedAt`.
- **Failure is not a capability state.** A failed command is a
  `CommandError`, or a Host Operation in `failed` or `uncertain`. The UI
  shows the two in different places with different copy.
- Rust exposes `capabilities_for(printer, host_facts) -> PrinterCapabilities`
  for P7's eligibility check, so Jobs do not go through the IPC type.
- Commands: `printer_capabilities(printerId)` and
  `adapter_capability_matrix()` (the static rows, for Setup and docs).
- Moonraker's row starts at `notVerified` everywhere. Task 11 flips each
  entry to `supported` or `adapter` only with live evidence from Task 1 or
  Task 11. Simulator evidence counts (answer 7). `evidence.verifiedHostVersions`
  names each source, for example "Moonraker vX / Klipper vY (sim)" and
  "Snapmaker U1 (Moonraker vZ)", so the row shows which tier proved it.
- **TLS (decided).** For a Connection with `useTls: true`, `upload`,
  `start`, `pause`, `resume`, and `cancel` report
  `unsupported { reason: "notVerified", detail: "TLS connections are not verified for staging yet" }`
  until a TLS host passes the spike's TLS gate. No TLS feature is added to
  `reqwest` or `tokio-tungstenite` in P6.

### D7. Guards: archive/delete, import, Connection, credential, revision

All guards run inside the transaction of the write they gate. "Unresolved"
means `dispatching`, `uncertain`, or `reconciling`.

| Mutation | While unresolved | New code |
|---|---|---|
| Archive Printer | **Blocked (decided).** Archive stops supervision and releases the host identity, so reconciliation could end up talking to a host another Printer now owns. | `HOST_OPERATION_UNRESOLVED` (a `LifecycleBlockerCode`) |
| Delete Printer | Blocked | same |
| Import Printers (`replace_all`) | Rejected | same, as a whole-document error |
| Change endpoint (kind, host, port, TLS) or clear the Connection | Blocked | `CONNECTION_IN_USE` |
| Clear the credential | Blocked | `CONNECTION_IN_USE` |
| Replace the credential on the same endpoint | **Allowed (decided)**, still probed, so an operator can recover from a key rotated on the host. Reconciliation uses the Printer's *current* credential, so it needs no snapshot of the old one. | — |
| Delete a Slice Revision referenced by an unresolved upload or start | Blocked (register a `SliceRevisionDeletionBlocker`) | `HOST_OPERATION_UNRESOLVED` |

- Terminal rows never block anything. `ON DELETE RESTRICT` would still
  block deleting a Printer that has only terminal rows, so **(decided)**
  permanent delete also deletes that Printer's terminal Host Operations in
  the same transaction. P6 owns no history view. P7 and P9 revisit this
  when Jobs and history exist.
- Credential cleanup needs no new rule: the endpoint guard keeps the
  Printer's current `credentialRef` referenced, and cleanup already skips
  referenced credentials. A test proves that a queued cleanup never
  deletes a credential that an unresolved operation's Printer uses.
- The existing eligibility UI (Archive/Delete buttons and their blocker
  messages) shows the new blocker with no new component.

### D8. Abandon reconciliation

For a host that is gone for good.

- **Command:** `abandon_host_operation(operationId, hostOperationId,
  acknowledgement: "hostStateUnknown", note?: string)`.
- **Allowed** only from `uncertain`, and only after at least one
  reconciliation attempt has failed. The operator can see what farm3d
  tried.
- **Confirmation:** a Kobalte `AlertDialog`. The operator must tick
  "I understand the printer may still have this file or be printing it."
  Copy draft: "farm3d can't reach *Voron 1* to find out whether
  *cube.gcode* was uploaded. If you abandon this check, farm3d stops
  checking and treats the result as unknown. The printer may still be
  printing."
- **Durable result:** state `abandoned`, `abandoned_at`, the optional
  note, and the last reconciliation evidence. The row is kept. Guards
  treat it as resolved, so Connection changes, archive, and delete become
  available.
- Abandoned is **not** success. P7 must treat any Job whose operation was
  abandoned as reconciliation-required, and P8 projects it into an
  Attention Event. P6 records the state only.

### D9. Frontend

- **Printer detail → new Job tab** (the umbrella spec's tab; P7 fills in
  Jobs later):
  - the host's current print from telemetry (file, state, progress), with
    **Pause**, **Resume**, and **Cancel print…** when the capability is
    supported. Cancel asks for confirmation;
  - **Staged on this Printer:** the Host Operations list with the states
    Uploading, Staged, Uncertain, Checking, Failed, and Abandoned;
  - per-row actions: **Check again**, **Abandon check…**, and
    **Start…** for a staged artifact.
- **Start… ships in P6 (decided)** and always asks the operator to
  confirm the bed is clear, whatever the Printer's start-safety rule says.
  The confirm button stays disabled until the operator ticks "The bed is
  clear". P6 never starts unattended; the `Unattended` rule stays for P7.
- **Slice Revision review:** a **Stage on Printer…** dialog (Kobalte
  Dialog and Select). Printers whose `upload` is unsupported are listed
  but disabled, with the reason. Offline Printers are disabled with
  "Offline", which is a different message. **Add to Queue…** stays
  disabled.
- **Setup tab:** a read-only **Capabilities** list (Supported / Not
  supported by *Moonraker* / Not verified yet / Not available on this
  printer). Connection edit and clear show `CONNECTION_IN_USE` with a
  link to the Job tab.
- **Unsupported vs failed.** An unsupported control is not rendered as an
  enabled button. It is either absent with a text explanation or disabled
  with `aria-describedby` giving the reason. A failure is an inline
  `role="alert"` with a recovery action. Tests assert the two never share
  copy or styling tokens.
- The Camera tab is P8. P6 shows camera only as a capability row.
- `just web` uses deterministic mock fixtures: one supported Moonraker
  Printer, one `notVerified` OctoPrint Printer, and one Printer with an
  uncertain upload.
- Tokens only, CSS Modules, Kobalte primitives, and the editor aesthetic
  (AGENTS.md).

### D10. What P6 leaves to P7

No Queue Entry, Job, reservation, start-safety automation, retry policy,
material accounting, or product-level Job transition. `host_operations` has
no `job_id` column; P7 adds one. P6's operator-invoked Stage and Start are
explicit manual host actions, not Jobs.

## Global constraints

**Carried over from P4 and P5:**

- **Contract registration.** Every new command goes in:
  - `lib.rs` `COMMAND_NAMES` and `generate_handler!`;
  - `contracts/inventory.rs` (`COMMAND_CONTRACTS`, its array length, and
    the `CommandContracts` declaration and visitor);
  - `tests/export_contracts.rs`;
  - `src/ipc/client.ts` `CommandMap`.

  Count assertions are updated by adding P6's count to whatever `main`
  has; never hard-code a total.
- **Schema version.** `0007_p6_host_operations.sql` (if 0006 is still the
  highest), with `CURRENT_SCHEMA_VERSION` matching it. Tests read the
  constant.
- **Generated contracts.** Never hand-edit `src/generated/contracts/**`.
  Run `just gen-contracts`.
- **Events** are emitted after commit only, on the `hostOperations` stream
  (type prefix `hostOperations.`), with listen-before-backfill.
- **Frontend conventions:** Kobalte primitives, CSS Modules, `--f3d-*`
  tokens, the editor aesthetic, and `pointerDown`/`pointerUp` in Select
  and Menu tests.

**P6-specific:**

- **Credentials** never enter `host_operations`, events, errors, logs, or
  `resolution_json`. A seeded-secret scan test covers every new path.
- **No automatic write retries.** Only the reconciler's *read* queries run
  on their own.
- **Test tiers.** The write tests run on the fake and the sim only. The U1
  runs a separate **read-only** suite.

  | Tier | Target | How it runs | Counts as live evidence | Writes allowed |
  |---|---|---|---|---|
  | Fake | `tests/p6_harness/fake_moonraker.rs` | `just test-rust`, in CI | No | Yes (no hardware) |
  | Sim | The shared container harness (`sim/compose.yaml`) | `just sim-up`, then `just test-sim` | **Yes** (answer 7), and the only tier for any write gate | Yes (simulated board) |
  | Real (read-only) | The owner's Snapmaker U1, Moonraker at `<U1 host>:7125` | A P6 recipe (`just test-moonraker-real`, name to confirm with the harness) that runs only the read-only suite | As read-only corroboration only | **Never** (answer 8) |

- **Endpoint variables come from the shared harness.** The harness branch
  (`feature/printer-simulation-harness`) owns the Moonraker endpoint and
  API-key variable names and the `test-sim` recipe. It had not landed when
  this plan was revised, so its names are not yet known. P6 does **not**
  define its own. The live-host tests read whatever the harness exports,
  and until then this plan calls them *the harness endpoint* and *the
  harness key*. P6 registers its `#[ignore]` live tests with
  `just test-sim` rather than adding a separate sim recipe. The real tier
  reuses the same variables, pointed at the U1. Task 11 records the final
  names here.
- **The owner's real network details are never committed.** `<U1 host>`
  stands for the U1's address. At run time it comes only from the harness's
  real-tier endpoint variable. Code, fixtures, spike reports, verification
  records, and screenshots never contain the owner's real IPs, hostnames,
  serials, MAC addresses, or tokens. Captured U1 responses are scrubbed
  before they become fixtures: identifiers are replaced, and any example
  address uses the RFC 5737 documentation range (`192.0.2.x`). A test scans
  `tests/fixtures/` and the P6 docs for private-range addresses outside an
  allowlist of the generic fixtures already on `main`.
- **The U1 is read-only, and the code enforces it.** The real-tier suite
  is a separate test module whose tests may use only read APIs:
  - probe;
  - subscribe;
  - `print_stats` and other status queries;
  - `server.files.list` and `metadata`;
  - `server.history.list`;
  - `server.webcams.list`;
  - capability detection.

  Two safeguards back this up:
  - The module builds its client through a read-only wrapper that exposes
    no `ArtifactStaging::upload`, no `PrintControl`, and no file delete.
  - A test that sends any HTTP `POST` or `DELETE`, or any
    `printer.print.*` RPC, through that wrapper fails.

  There is no opt-in variable that enables writes on the real tier. Writes
  on the sim need no safety opt-in (the board is simulated), but the sim
  write fixture still has no heating or motion (comments and `M117`
  only).
- **The U1 is a vendor build.** Snapmaker's firmware may run a modified
  Moonraker and Klipper. A difference between the sim and the U1 is
  recorded as a finding for that host. It is not averaged away, and it
  becomes a `host` reason in the capability row (D6) when it applies.

## File and module map

### Backend

| File | Responsibility |
|---|---|
| `connections/adapters.rs` | `AdapterDescriptor`, `registry()`, `descriptor(kind)`; the single "is this kind supported" answer |
| `connections/capabilities.rs` | The four capability traits, `CommandFailure` (definitive vs indeterminate), `CapabilityKey`, `CapabilityState`, `PrinterCapabilities`, `capabilities_for` |
| `connections/moonraker/files.rs` | Pure: upload request parts, metadata and history parsing, `print_stats` → `HostJobState` |
| `connections/moonraker/control.rs` | Thin I/O for `ArtifactStaging`, `PrintControl`, `HostStateQuery`, and `CameraDiscovery` |
| `host_ops/mod.rs` | `HostOperationServices` wired into `RuntimeServices` |
| `host_ops/state.rs` | Pure state machine and transition table |
| `host_ops/repository.rs` | SQL for `host_operations` |
| `host_ops/executor.rs` | Write-ahead, dispatch, and resolve, with injectable fault points |
| `host_ops/reconciler.rs` | Startup, on-Online, and manual reconciliation; backoff |
| `host_ops/guards.rs` | The lifecycle blocker source, the revision deletion blocker, the Connection-mutation check, and the import check |
| `host_ops/events.rs` | The `hostOperations` stream and backfill |
| `host_ops/commands.rs` | The Tauri commands |
| `migrations/0007_p6_host_operations.sql` | Table, indexes, terminal-state trigger, operations-ledger CHECK rebuild |

Also modified:

- `connections/mod.rs` (docs), `connections/supervisor.rs` (registry and
  the on-Online hook);
- `connections/commands.rs`, `printers/create.rs`, `printers/batch.rs`,
  `printers/setup.rs` (registry lookups);
- `printers/lifecycle.rs` (new code and source registration);
- `printers/repository.rs` (`set_connection` and `replace_all` checks);
- `slicing/blockers.rs` (register the revision blocker);
- `spools/operations.rs` (new `OperationKind`s);
- `contracts/command.rs` (`CAPABILITY_UNSUPPORTED`, `CONNECTION_IN_USE`,
  `HOST_OPERATION_UNRESOLVED`, `HOST_OPERATION_NOT_ABANDONABLE`);
- `lib.rs` (startup order), `contracts/inventory.rs`, `Cargo.toml`
  (`reqwest`).

Test support: `tests/p6_harness/fake_moonraker.rs` (HTTP and WebSocket on
one `TcpListener`, scripted faults) and `tests/p6_harness/cut_proxy.rs` (a
TCP proxy that forwards a request and then drops the response, for
failure injection against the sim only; it is never pointed at the U1). The
container harness itself
(`sim/`, `just sim-up`, `just test-sim`) belongs to
`feature/printer-simulation-harness`, and P6 does not modify it. If P6
needs a harness change, such as a `pause_resume` or webcam variant of the
Klipper config, it asks that branch's owner rather than forking `sim/`.

### Frontend

| File | Responsibility |
|---|---|
| `src/host-ops/capabilities-store.ts` | `PrinterCapabilities` per Printer; refetch on status change |
| `src/host-ops/host-operations-store.ts` (+ `-mock.ts`, `web-fixtures.ts`) | Host Operations; sequenced stream |
| `src/host-ops/presentation.ts` | State → label and severity; unsupported vs failed copy (pure, unit-tested) |
| `src/screens/PrinterJobPanel.tsx` | The Job tab |
| `src/screens/StageOnPrinterDialog.tsx` | Stage from Slice Revision review |
| `src/screens/AbandonReconciliationDialog.tsx` | D8 |
| `src/screens/StartStagedDialog.tsx` | The required "bed is clear" confirmation for Start |
| `src/screens/CapabilityList.tsx` | The Setup capabilities list |

Also modified: `PrinterDetailDock.tsx` (the Job tab),
`SliceRevisionReview.tsx` (Stage on Printer…), `PrinterConnectionPanel.tsx`
(`CONNECTION_IN_USE`), `App.tsx` (store startup), and `src/ipc/client.ts`.

### Docs

- ADR-0011, "Composable connection capability interfaces".
- `CONTEXT.md`: add **Host Operation**, **Staged artifact**,
  **Uncertain outcome**, **Abandon reconciliation**, and **Capability**.
  Update **Connection** ("command capabilities per adapter").
- The approach doc: close the "Adapter command/camera capabilities" row
  for Moonraker only.
- `docs/verification/2026-09-2x-p6-moonraker-commands.md` and
  `docs/screenshots/p6-*.png`.

## Tasks

### Task 1: Moonraker command spike (Stage A)

**Owner:** Protocol. **Prerequisites:** the shared sim harness for every
gate, and the U1 for the read-only corroboration column. Running #9
against the same sim in the same session is recommended, since the sim now
counts for #9 too.
**Status:**

- The U1's **read-only** checks (Gates A, C, and H, all reads) are **ready
  now**.
- Every gate's required sim column, which includes every write, **needs
  the harness** to land.
- Nothing in this task writes to the U1 (answer 8).

**Output:** the spike report, with one row per gate **per tier** (PASS,
FAIL, or Unavailable), its evidence, the decision taken, and the versions.
For the sim, record the harness commit, the Klipper and Moonraker versions,
and the config variant. For the U1, record the firmware, Moonraker and
Klipper versions as reported, and whether `pause_resume`,
`virtual_sdcard`, and webcams are configured. Scratch code lives in a
throwaway crate outside the repo, as in P4 and P5.

Which tier must pass each gate before the spec is approved:

| Gate | Sim (required: answer 7) | U1 (read-only corroboration, answer 8) |
|---|---|---|
| A Auth | Required, with the harness's auth setting on and off | Ready now: read-only requests (`server.info`, a file list) with and without a key |
| B Upload without start | Required | **Not run** (write) |
| C Identity | Required, on the file B staged | Ready now: `list` and `metadata` shapes for files already on the U1, read-only |
| D Interrupted upload | Required (`cut_proxy`) | **Not run** (write) |
| E Control | Required (simulated board, so no hardware risk) | **Not run** (write) |
| F Start reconciliation | Required | Read-only part only: the `print_stats` and `server.history.list` shapes for jobs the owner has already run |
| G Host restart | Required (restart the containers) | **Not run** (restarting host services is a write) |
| H Capability detection | Required, for each config variant the harness offers | Ready now: `server.info`, `printer.objects.list`, `server.webcams.list` |
| I TLS | Expected Unavailable (TLS stays unsupported, answer 6) | Expected Unavailable (read-only check of whether TLS is offered) |

A gate that passes on the sim but whose read-only shapes differ on the U1
is recorded as a U1 finding (§Global constraints). It does not fail the
gate. A "Not run" U1 cell is reported as **Not run (read-only host)**,
never as passed.

- [ ] **Gate A — Auth for HTTP.** Does `X-Api-Key` work on the HTTP file
  endpoints as on the WebSocket? Record the 401 and 403 shapes. Try
  trusted-client access with no key.
- [ ] **Gate B — Upload without start.** Upload the no-motion fixture to
  `farm3d/<id>.gcode` with no `print` field. Confirm:
  - the directory is created;
  - `print_stats` does not change;
  - the response shape;
  - whether `checksum` is accepted and a wrong checksum is rejected (and
    with which status).
- [ ] **Gate C — Identity.** What `metadata` and `list` return for the
  staged file (size, modified, any hash). Time the download of a 20 MB file
  to decide whether download-and-hash is acceptable during reconciliation.
- [ ] **Gate D — Interrupted upload.** Drop the TCP connection mid-body,
  and separately right after the full body but before the response (with
  `cut_proxy`). Record whether a file exists and its size in each case.
- [ ] **Gate E — Control.** `start`, `pause`, `resume`, and `cancel` from
  each relevant state. Record the error shapes for wrong-state calls
  (start while printing, pause while idle) and for Klipper not ready.
  Classify each response as definitive or indeterminate (D3).
- [ ] **Gate F — Start reconciliation evidence.** After a start whose
  response was cut: the `print_stats` sequence, and whether
  `server.history.list` records the job with a `start_time` usable against
  `dispatched_at`. Measure the host clock skew.
- [ ] **Gate G — Host restart.** Restart Moonraker, then Klipper, mid-upload
  and mid-print. Record what persists (files, history, state) and how the
  WebSocket reports it.
- [ ] **Gate H — Capability detection.** How to tell from `server.info`,
  `printer.objects.list`, and `server.webcams.list` that a host lacks
  `virtual_sdcard`, `pause_resume`, or a webcam. These are the `host`
  reasons in D6.
- [ ] **Gate I — TLS.** Whether any test instance offers TLS. If none
  does, record TLS as unavailable. The spec keeps `useTls` staging
  unsupported (answer 6).
- [ ] **Write the report** and stop for approval.

### Task 2: Focused spec and ADR-0011 (Stage B)

**Owner:** Wiring, with Protocol. **Prerequisites:** Task 1 approved for the
Moonraker-specific decisions. The adapter-neutral parts (D1, D2, D3, D6,
D7, D8, D9, D10) **may be drafted now**.

- [ ] Write the spec in the form of the P5 spec: status, goal, scope and
  non-goals, vocabulary, decisions D1…Dn (finalizing this plan's D1–D10
  with the spike outcomes), backend model, wire types, commands and events,
  frontend architecture, errors, accessibility, acceptance criteria, and
  delivery.
- [ ] Write ADR-0011 for D1. The simulator harness adds its own ADR under
  a different number. Before committing, check `docs/adr/` on `main` and
  take the next free number if 0011 is taken.
- [ ] Carry the user's answers (§Status) into the spec as fixed
  decisions. There are no open questions left from this plan. Any new
  question the spike raises goes to the user before approval.
- [ ] Update this plan's tasks if the spike changed an interface. Stop for
  approval.

### Task 3: Adapter registry (refactor, no behavior change)

**Owner:** Backend. **Prerequisites:** none. **Status: Ready now.** It needs
no spec decision beyond "one registry", which D1 options B and C both
require.

- [ ] Add `connections/adapters.rs` with a Moonraker descriptor that has
  only `observe`.
- [ ] Replace the five kind checks (`commands.rs:300`, `create.rs:297`,
  `batch.rs:567`, `setup.rs:48`, and `supervisor::build`) with
  `descriptor(kind)`.
- [ ] Change `ConnectionManager::with_clock_and_factory` to take a registry
  (or keep the closure and build it from the registry). The injected-fake
  tests keep working.
- **Acceptance:** an unsupported kind still produces `UNSUPPORTED_ADAPTER`
  for set, create, and batch, `SetupGap::UnsupportedAdapter`, and the
  supervisor's "not supported by this build" status. `grep MOONRAKER_KIND`
  outside `connections/` and tests returns nothing.
- **Tests:** the existing suite passes unchanged. Add one test per call site
  that uses a registry with a second fake kind, to prove the lookup and not
  a string compare decides.

### Task 4: Capability model, matrix, and contracts

**Owner:** Backend and Wiring. **Prerequisites:** Task 2 (D1, D6 approved),
Task 3. **Status: After spec.**

- [ ] Add `connections/capabilities.rs` with the four traits,
  `CommandFailure { Definitive(..) | Indeterminate(..) }`, and the ts-rs
  types from D6.
- [ ] Add `evidence` to `AdapterDescriptor`. The Moonraker row is
  `notVerified` everywhere.
- [ ] Add `capabilities_for` and the host-fact derivation from probe
  results (Gate H). Before Gate H, host facts are empty and never produce
  `supported`.
- [ ] Add the commands `printer_capabilities` and
  `adapter_capability_matrix`.
- [ ] Apply the TLS rule from D6: a `useTls` Connection reports the five
  write capabilities as `notVerified`.
- **Acceptance:** a Printer with no Connection, an unsupported kind, a
  Moonraker Connection, and a Moonraker Connection with `useTls` each
  return the expected matrix. `notVerified` is never reported as
  `supported`.
- **Tests:** the descriptor-consistency test (D1); serialization snapshots;
  a contract export; each `CapabilityState` variant round-trips.

### Task 5: Schema, state machine, and repository

**Owner:** Backend. **Prerequisites:** Task 2 (D2, D3). **Status: After
spec.**

- [ ] Add `0007_p6_host_operations.sql`: the table, the partial unique
  index, the terminal-state immutability trigger, and the operations
  ledger CHECK rebuild with the new kinds. Bump `CURRENT_SCHEMA_VERSION`.
- [ ] Add `host_ops/state.rs` with a pure transition function and an
  exhaustive table.
- [ ] Add repository functions: `insert_dispatching`, `transition`,
  `list_unresolved`, `list_for_printer`, `load`, and
  `mark_dispatching_uncertain` (the startup step).
- **Acceptance:** every legal transition in D3 persists, and every illegal
  one is rejected. A terminal row cannot be updated. A second unresolved
  row for the same Printer violates the index.
- **Tests:**
  - A migration over a P5-shaped database, and a restart.
  - The transition table (every pair).
  - The trigger fires.
  - The partial unique index.
  - Operation-ledger replay returns the same `hop-*`.
  - No column can hold a credential (a schema-level assertion that no
    `credential` column exists, plus the seeded-secret scan in Task 8).

### Task 6: Guards

**Owner:** Backend. **Prerequisites:** Task 5. **Status: After spec.** It
needs no live evidence.

- [ ] Register `HostOperationBlockers` in
  `printers::lifecycle::blocker_sources()` for Archive and Delete, with
  `LifecycleBlockerCode::HostOperationUnresolved`.
- [ ] Register `UnresolvedHostOperationBlocksRevisionDeletion` in
  `slicing::blockers::slice_revision_blocker_sources()`.
- [ ] Add the endpoint/clear/credential-clear check inside
  `PrinterRepository::set_connection`'s transaction →
  `RepositoryError::ConnectionInUse` → `CONNECTION_IN_USE`.
- [ ] Add the `replace_all` check → a whole-import rejection.
- [ ] On permanent delete, delete that Printer's terminal rows in the same
  transaction (decided, answer 5). The unresolved-row blocker runs first,
  so only terminal rows are ever deleted this way.
- **Acceptance:** each row of the D7 table, in both directions. Blocked
  while unresolved, allowed after `succeeded`, `failed`, or `abandoned`.
- **Tests:**
  - One test per mutation per terminal state.
  - A race test: an operation inserted concurrently with delete. Exactly
    one wins, and no orphan remains.
  - A credential-cleanup test: a queued cleanup of the Printer's current
    ref is skipped while an operation is unresolved.
  - Credential replacement on the same endpoint is allowed and probed.
  - `printer_lifecycle_eligibility` reports the new blocker.
  - Archive is blocked while a write is unresolved (answer 3).
  - Permanent delete removes the Printer's terminal rows and leaves every
    other Printer's rows alone.

### Task 7: Moonraker capability adapter and fake Moonraker

**Owner:** Protocol. **Prerequisites:** Task 1 approved (its sim column),
Task 4. **Status: Needs the sim harness**, through Task 1. The fake and the
pure functions can start after Task 2 from the documented API. The U1's
read-only captures from Task 1 (file list, metadata, `print_stats`,
history, webcams, `server.info`) can seed the **read-side** parser
fixtures now. Every write-side fixture (upload responses, control
responses, error shapes) comes from the sim only. The fake's behavior must
be corrected to match the sim before this task is done. Nothing in this
task runs against the U1.

- [ ] Add `moonraker/files.rs` (pure): upload parts builder with no
  `print` field, the checksum field, metadata, history and `print_stats`
  parsers, and response classification (definitive vs indeterminate, per
  Gates D and E).
- [ ] Add `moonraker/control.rs` (I/O) implementing the four traits over
  `reqwest` and the existing WebSocket framing.
- [ ] Update the Moonraker descriptor with the builders and its
  (still `notVerified`) evidence row.
- [ ] Add `tests/p6_harness/fake_moonraker.rs` with scripted faults:
  - lose the response after storing the file;
  - store a partial file;
  - delay past the timeout;
  - 401 and 403;
  - apply a start but lose the response;
  - reject a start definitively;
  - restart the host (clearing live state, keeping files and history);
  - fail any test that sends `print`.
- **Acceptance:**
  - Every write-side pure function is covered by sim fixtures.
  - Every read-side parser is covered by sim fixtures and, where the shape
    differs, by U1 read-only fixtures.
  - The adapter passes the full fake scenario suite.
- **Tests:** unit tests for the builder and parsers (including "no
  `print` field"); adapter tests against the fake for every Gate D–H
  behavior.

### Task 8: Executor, reconciler, abandon, events, and commands

**Owner:** Backend and Wiring. **Prerequisites:** Tasks 3–6. It develops
against fake capability traits and wires to Moonraker after Task 7.
**Status: After spec** (the Moonraker wiring waits for Task 7).

- [ ] `executor.rs`: write-ahead `dispatching` → call the trait → resolve.
  Injectable fault points: before send, after send, after response before
  commit.
- [ ] `reconciler.rs`: the D5 rules. Startup runs after
  `restore_persisted_connections` in `lib.rs`. The supervisor's Online
  transition triggers it. Per-Printer serialization and backoff.
- [ ] `abandon_host_operation` with the D8 preconditions.
- [ ] The `hostOperations` stream and backfill.
- [ ] Commands: `stage_slice_revision`, `start_staged_artifact`,
  `pause_host_print`, `resume_host_print`, `cancel_host_print`,
  `reconcile_host_operation`, `abandon_host_operation`, and
  `list_host_operations`. Each capability command checks the matrix
  first and returns `CAPABILITY_UNSUPPORTED` **without** writing a row.
- **Acceptance:** the restart matrix below holds, and no command path ever
  issues a second upload or start by itself.
- **Tests (the restart matrix, against a fake trait and then the fake
  Moonraker):**

  | Crash or fault point | Kind | Expected after restart |
  |---|---|---|
  | After the write-ahead commit, before send | upload | `uncertain` → reconcile → absent → `failed { notApplied }`, no re-upload |
  | After send, response lost | upload | `uncertain` → present and matching → `succeeded`, exactly one upload seen by the fake |
  | Partial file stored | upload | `uncertain` → mismatch → handled per D5 |
  | After send, response lost | start | `uncertain` → the host is printing `host_path` → `succeeded`, exactly one start seen |
  | Start definitively rejected | start | `failed`, and no reconciliation |
  | Host unreachable for the whole run | any | stays `uncertain`; abandon allowed; guards lift after abandon |
  | Host running a different file | start | stays `uncertain` with the busy note; never auto-starts |

  Also: the seeded-secret scan over events, errors, rows, and logs;
  listen-before-backfill ordering; operation-id replay for every command.

### Task 9: Frontend contracts and stores

**Owner:** Frontend. **Prerequisites:** Task 4 and Task 8 contract types
(`just gen-contracts`). **Status: After spec.**

- [ ] Add `capabilities-store.ts`, `host-operations-store.ts` (sequenced
  stream), `presentation.ts`, the mock, and the web fixtures (D9).
- **Tests:** stream ordering and backfill; presentation maps every
  `CapabilityState` and Host Operation state; unsupported and failed
  produce different copy and different severity.

### Task 10: Frontend UI

**Owner:** Frontend. **Prerequisites:** Task 9. **Status: After spec.**

- [ ] Add the Job tab (`PrinterJobPanel`), `StageOnPrinterDialog`,
  `AbandonReconciliationDialog`, `StartStagedDialog` (answer 1),
  `CapabilityList`, `CONNECTION_IN_USE` handling in
  `PrinterConnectionPanel`, and **Stage on Printer…** in
  `SliceRevisionReview`. Add any new design-system component to
  `components/index.ts` and `Showcase.tsx`.
- **Acceptance:**
  - An unsupported control is never an enabled button.
  - A failed operation shows an alert with a recovery action.
  - Abandon cannot be confirmed without the acknowledgement.
  - Start cannot be confirmed until "The bed is clear" is ticked, for
    both start-safety rules.
  - Everything is keyboard-operable.
  - Layout works at 1440 × 900 and 1024 × 700.
- **Tests:** component tests with `@solidjs/testing-library`
  (`pointerDown`/`pointerUp` for Select and Menu). Each dialog's disabled
  and blocked states. The Stage dialog disables unsupported Printers and
  Offline Printers with different reasons.

### Task 11: Live-host tests (sim writes, U1 read-only) and failure injection

**Owner:** Protocol and Wiring. **Prerequisites:** Tasks 7 and 8, and the
sim harness merged or rebased under this branch. **Status: Needs the sim
harness.** The U1 read-only suite needs only Task 7's read side.

- [ ] Add `#[ignore]` live-host tests that read the harness endpoint and
  key (§Global constraints). Register them with `just test-sim`. Record
  the harness's final variable names in this plan.
- [ ] **Sim tier (required):**
  - Put `cut_proxy` between farm3d and the sim to lose responses for
    upload and start.
  - Rebuild `RuntimeServices` over the same roots (a restart) and assert
    reconciliation.
  - Restart the Moonraker and Klipper containers mid-upload and mid-print.
  - Stop the containers for longer than the backoff ceiling, then abandon.
- [ ] **Real tier (U1): a read-only suite only (answer 8).** It runs
  through the read-only wrapper (§Global constraints) against
  `<U1 host>:7125`, and covers:
  - probe, subscribe, and capability detection, compared with the sim;
  - read-side parsing of the U1's real `print_stats`, file list,
    metadata, history, and webcams;
  - **reconciliation reads with no host write:**
    1. Seed an `uncertain` upload row directly in the local database. It
       targets a `farm3d/<new-uuid>.gcode` path that was never sent.
    2. Reconcile against the U1. Assert `locate` reports absent and the
       row resolves to `failed { notApplied }`.
    3. Assert the wrapper recorded zero `POST`, zero `DELETE`, and zero
       `printer.print.*` calls.
  - **host unreachable:** block the port on the farm3d machine (never
    touch the printer), assert the row stays `uncertain`, then abandon it.
    Abandon is a local write only.

  The P6 recipe for this suite is `just test-moonraker-real`, unless the
  harness already provides a real-host recipe. It runs no upload, no
  interrupted upload, no file deletion, no control command, and no service
  restart on the U1.
- [ ] Flip each Moonraker matrix entry from `notVerified` to `supported` or
  `adapter`, citing the sim evidence. U1 read-only results may add `host`
  facts and version corroboration, but never the evidence for a write
  capability. Update the descriptor.
- **Acceptance:**
  - Every D5 row is observed on the sim.
  - The U1 suite passes with zero recorded writes.
  - Any U1 check that could not run is recorded as **unavailable**,
    never as passed.
  - The capability row cites the evidence and its tier.

### Task 12: Tracer

**Owner:** Wiring. **Prerequisites:** Tasks 8, 10, and 11.

- [ ] Add `src-tauri/tests/p6_tracer.rs`. The full tracer below runs
  against the fake in CI and against the sim under `just test-sim` (the
  evidence that counts). It **never** runs against the U1, because step 2
  uploads.
- [ ] Add a separate read-only U1 variant, `u1_reconciliation_tracer`. It
  has **no upload** and goes through the read-only wrapper:
  1. Seed a Slice Revision and an `uncertain` upload row for a never-sent
     `farm3d/<new-uuid>.gcode`.
  2. Restart by rebuilding `RuntimeServices`.
  3. Reconcile against the U1 → `failed { notApplied }`.
  4. Assert the guards were blocked before step 3 and lifted after it.
  5. Assert zero host writes were recorded.

  The full sim tracer:
  1. Import and slice (or import an external revision) to get one Slice
     Revision. Record its sha256.
  2. `stage_slice_revision` to a Moonraker Printer, with the response cut
     by the proxy (or the fake).
  3. Restart: rebuild `RuntimeServices` over the same roots.
  4. Reconcile. Assert `succeeded`, exactly one file at `host_path` whose
     hash matches, **no** second upload request, and **no** start request
     (the host is still standby; history has no new job).
  5. Assert that delete, archive, and Connection clear were blocked during
     step 2–4 and are available after step 4.
  6. Repeat with the host unreachable: abandon, then assert the guards
     lift and the row is kept as `abandoned`.

### Task 13: Docs and verification

**Owner:** Wiring. A Verification owner then does a fresh pass.
**Prerequisites:** all.

- [ ] ADR-0011, `CONTEXT.md`, and the approach-doc row.
- [ ] **Full verification:**
  - `just build`, `just test`, and `source "$HOME/.cargo/env" && just
    test-rust`;
  - `just gen-contracts` plus `git diff --exit-code src/generated`;
  - `just sim-up` then `just test-sim`, and the U1 read-only suite;
  - `just package`, then stage one revision **to the sim** from the
    installed package on Linux x86_64. The phase adds a direct dependency
    and changes network behavior, so the installed-bundle check applies.
    The installed app may connect to the U1 for monitoring and the
    Capabilities list only.
- [ ] A manual pass in `just dev` at 1440 × 900 and 1024 × 700, with every
  Stage, Start, and control action aimed at a sim Printer (a U1 Printer
  may appear only to show its read-only Capabilities list and status): the
  Job tab, Stage, an uncertain row, Check again, Abandon, the blocked
  Connection edit, blocked archive, and the Capabilities list. Screenshots
  go in `p6-*.png`. Any check that could not run is recorded as
  **unavailable**.
- [ ] Verification doc: map the evidence to each #16 acceptance criterion.

## Delivery order and parallel work

```text
Task 3 registry ───────────────────────────────┐
Task 1 spike ─> Task 2 spec ─┬─> Task 4 matrix ┴─┬─> Task 7 Moonraker adapter ─┐
                             └─> Task 5 schema ──┴─> Task 6 guards ─> Task 8 ──┼─> Task 11 real-host ─> Task 12 tracer ─> Task 13
                                            Task 4 + Task 8 types ─> Task 9 stores ─> Task 10 UI ─┘
```

| Task | Status today | Blocked by |
|---|---|---|
| 1 Spike | U1 **read-only** checks (A, C, H, and the read part of F) **ready now**; every write gate needs the sim harness | Sim harness landing |
| 2 Spec + ADR-0011 | **Ready now** to draft in full: every product question is answered. Approval needs the Task 1 sim column | Task 1 (sim) |
| 3 Registry refactor | **Ready now** | — |
| 4 Capability model | After spec (can start on approval of the adapter-neutral decisions) | Task 2, Task 3 |
| 5 Schema / state machine | After spec (same) | Task 2 |
| 6 Guards | After spec (same) | Task 5 |
| 7 Moonraker adapter | Pure parts and fake after spec; done needs the sim | Task 1 (sim), Task 4 |
| 8 Executor / reconciler | After spec (fake traits) | Tasks 3–6; Task 7 for wiring |
| 9 Stores | After spec | Tasks 4, 8 (types) |
| 10 UI | After spec | Task 9 |
| 11 Live-host tests | Sim suite needs the harness; the U1 read-only suite needs Tasks 7 (read side) and 8 | Tasks 7, 8; sim harness |
| 12 Tracer | Waits on tasks | Tasks 8, 10, 11 |
| 13 Docs / verification | Waits on tasks | All |

- #15 is **not** a blocker any more (closed, merged at `5c9ead7`).
- #9 is no longer a hardware blocker. The sim counts for it (answer 7), so
  #9 and the Task 1 sim column can run together as soon as the harness
  lands.
- The only external blocker left is the harness branch
  (`feature/printer-simulation-harness`). Every write gate depends on it.
  The U1 is reachable now for read-only checks only (answer 8).
- **Ready now, in total:**
  - Task 3 (the registry refactor);
  - drafting the full Task 2 spec;
  - the U1 read-only spike checks (Gates A, C, and H, and the read-only
    part of F).
- Tasks 4, 5, and 6 can run in parallel with Task 1 once Task 2's
  adapter-neutral decisions are approved. If the user prefers one approval
  for the whole spec, they wait for Task 1's sim column.
- Tasks 9 and 10 can run in parallel with Task 7.

## Other adapters

Nothing in the shared foundation assumes parity.

- **OctoPrint** needs #10 complete, then a separate command-research spike
  (the same gate list as Task 1, against OctoPrint's `/api/files` and
  `/api/job`), then a short adapter plan that does only the equivalents of
  Tasks 7, 11, and 12. Its registry row stays `notVerified` until then. The
  schema, state machine, guards, matrix, and UI do not change. Monitoring
  support from #10 never flips a command capability.
- **ElegooLink** stays out of the registry entirely until #8 records a go
  decision. After that, its plan follows the OctoPrint shape.
- One completed Moonraker path is enough for the first P7 tracer. It does
  not confer parity on the others.

## External dependencies and blockers

- **Shared sim harness (blocking the sim column).**
  - Branch: `feature/printer-simulation-harness`, owned by a separate
    agent.
  - Provides `sim/compose.yaml`, `just sim-up`, `just test-sim`, the
    endpoint and key variables, and its own ADR.
  - P6 needs Klipper with `virtual_sdcard`, and ideally a config variant
    with `pause_resume` and a webcam entry for Gate H. Any variant P6
    needs is requested from that branch's owner.
  - P6 live-host work rebases onto the harness once it merges.
- **Snapmaker U1 (available now).** Moonraker at `<U1 host>:7125`, owned
  by the repo owner.
  - It is **read-only** corroboration (answer 8), not the required tier.
    Agents never upload, interrupt an upload, delete files, send
    start/pause/resume/cancel, or restart services on it. There is no
    opt-in that changes this.
  - Allowed on the U1: probe, subscribe, status, file-list, metadata,
    history, webcam queries, and capability detection.
  - Its firmware is a vendor build, so differences in read-only shapes
    from the sim are expected findings.
- **`reqwest`.** Already in `Cargo.lock` through Tauri (0.13.5), so it adds
  no new crates. It becomes a direct dependency with `multipart` and
  `stream`, and no TLS features (answer 6).
- **CI** has no Moonraker and no container runtime assumption. CI runs the
  fake. Sim and U1 evidence lives in the verification doc.
- **Platforms.** F0 declares only Linux x86_64 supported. Windows and macOS
  compile and run unit tests, and make no claim.

## Coordination items (not product questions)

The user answered every product question on 2026-09-25 (§Status). What
remains is coordination with the harness branch:

- The harness's final names for the endpoint variable, the key variable,
  and any real-host recipe. P6 adopts them (§Global constraints). If the
  harness defines a real-host *write* opt-in, P6 does not use it against
  the U1.
- Whether the harness offers `pause_resume` and webcam config variants.
  Without them, Gate H's `host` reasons are proved only for the default
  config.
- ADR numbering: P6 takes the next free number after the harness ADR if
  0011 is taken.

## Exit gate (from issue #16, for Moonraker)

- [ ] An evidence-backed Moonraker capability row records supported and
  unsupported results (Tasks 1, 4, 11).
- [ ] Live-host command tests on the sim (the required tier, answer 7)
  cover uncertainty, interruption, restart, and reconciliation. The U1
  read-only suite's results are recorded alongside them, with zero writes
  to the U1 (Tasks 8, 11).
- [ ] Archive/delete, import, and Connection/credential mutations cannot
  orphan an active or uncertain operation, and become available only after
  resolution or explicit abandonment (Tasks 6, 8, 12).
- [ ] Capability-aware frontend behavior distinguishes unsupported from
  failed operations (Tasks 9, 10).
- [ ] The tracer completes on the sim with no duplicate upload and no
  accidental start. The U1 read-only reconciliation variant completes with
  no upload (Task 12).
