# P6 Connection Command Capabilities Design

## Status

Approved by the controller (owner-delegated), 2026-09-25, after task review and one fix round.

This is the focused design for GitHub issue #16 (P6, Moonraker). It is
Task 4 of `docs/superpowers/plans/2026-09-25-p6-connection-command-capabilities.md`.
It turns the plan's Design reference D1–D9 into final decisions, and it
binds Tasks 5–14: where this spec and the plan differ, this spec wins.

It rests on the approved Moonraker command spike,
`docs/superpowers/baselines/2026-09-25-p6-moonraker-command-spike.md`.
Claims about Moonraker behaviour cite a gate (Gate A … Gate I) or a
numbered spike decision ("spike 4a") from that report. ADR-0011
(`docs/adr/0011-composable-connection-capabilities.md`) records the
hard-to-reverse part of D1.

The owner's fourteen answers in the plan ("Owner decisions (fixed)") are
fixed. This spec applies them and does not reopen them. The ones that
shape it most:

- **Answer 1:** Start ships in the P6 UI, always behind a bed-clear
  confirmation.
- **Answer 2:** at most one unresolved write per Printer.
- **Answers 3 and 4:** Archive is blocked while a write is unresolved. A
  credential replacement on the same endpoint is allowed; clearing the
  credential or changing the endpoint is blocked.
- **Answer 5:** permanent Printer delete also deletes that Printer's
  terminal Host Operation rows.
- **Answers 8 and 14:** real printers are read-only; the simulators never
  run in CI.
- **Answer 9:** everything that touches tools handles N tools.
- **Answers 12 and 13:** `Finished`, `Cancelled`, and `Failed` are never
  Ready. Start is offered from Ready, Finished, or Cancelled, and never
  after Failed until the host stops reporting the error.

### Fail-safe principle

The spike found that Moonraker and Klipper can apply a command late, can
answer `ok` when nothing happened, and can deliver an upload after the
client gave up (Gates D, E, F; spike 4, 4a, 7). Every rule below follows
one principle:

> farm3d concludes "not applied" only from a definitive answer to the
> request itself, or, for an upload, from absence that has outlasted the
> settle period. It never concludes "not applied" from absence alone while
> a command could still be queued or an upload could still be finalizing.
> When it cannot prove either outcome, the operation stays `uncertain`
> until farm3d proves it applied or the operator abandons the check.

## Goal

For a Printer with a Moonraker Connection, an operator can:

- **Stage** one immutable Slice Revision on the Printer: upload it without
  starting it, and have farm3d verify the bytes on the host.
- **Start** a staged artifact behind the bed-clear confirmation, and
  **pause**, **resume**, and **cancel** the host's print.
- See the Printer's **capabilities**, with the evidence behind each one,
  including whether a camera is configured (no media; that is P8).
- Trust that every write is a durable **Host Operation**. A lost response
  makes it **uncertain**. farm3d then reconciles it against the host after
  reconnecting or restarting, and never re-uploads or re-starts on its own.
- Be unable to orphan an unresolved Host Operation through Archive,
  delete, Printer import, a Connection change, a credential clear, or
  deleting the Slice Revision it uses.
- **Abandon reconciliation** of an operation whose host is gone for good,
  explicitly and durably, accepting that the host state is unknown.

## Scope

### In scope

- The capability traits and the adapter registry's capability rows (D1,
  D6), with the `printer_capabilities` and `adapter_capability_matrix`
  commands.
- The `host_operations` table, its state machine, and its repository (D2,
  D3).
- The Moonraker capability adapter over HTTP: staging, identity
  verification (download-and-hash), control, host-state queries, and
  camera discovery (D4, D10).
- The executor, the reconciler, the Start rule, the control rule, abandon,
  the `hostOperations` event stream, and their commands (D5, D8, D9, D11).
- Guards on Printer lifecycle, Connection edits, import, and Slice Revision
  deletion (D7).
- Frontend: the capability and Host Operation stores, the Start and
  control offers, and the Job tab, Stage, Start, Abandon, and Capabilities
  UI.
- `FakeMoonraker` (CI), the simulator evidence suite, the read-only
  real-host suite, and the tracer.

### Non-goals

- OctoPrint and ElegooLink command capabilities. OctoPrint's row stays
  `notVerified` (plan "Out of scope" lists its prerequisites). ElegooLink is
  out of P6 (answer 10).
- Queue Entries, Jobs, scheduling, unattended start, retry policy, and
  material accounting (P7).
- Camera media, snapshots, and a Camera tab (P8). P6 reports only whether a
  camera is configured.
- TLS (answer 6).
- Deleting files from a printer, cleaning up Moonraker's orphaned upload
  temp files (Gate D), or any cleanup of what an earlier upload left.
- Upload progress reporting. The UI shows an indeterminate "Uploading".
- Resetting the host's print state (no `SDCARD_RESET_FILE`).

## Product vocabulary

These terms are added to `CONTEXT.md`, and **Connection** is updated:

- **Host Operation** — one write farm3d sends to a Printer's host (upload,
  start, pause, resume, or cancel), recorded durably before the first byte
  leaves farm3d, with the outcome farm3d has proved.
  _Avoid_: Job (that is P7), command, request.
- **Staged artifact** — a Slice Revision's G-code that a succeeded upload
  Host Operation put on a Printer's host at `farm3d/<slice-revision-id>.gcode`,
  and whose bytes farm3d verified there. Staging never starts a print.
  _Avoid_: uploaded file, remote copy.
- **Uncertain outcome** — the state of a Host Operation whose effect on the
  host farm3d could not prove either way, for example because the response
  was lost. farm3d keeps checking the host and never repeats the write by
  itself.
  _Avoid_: failed, unknown error.
- **Abandon reconciliation** — the operator's explicit, recorded decision
  to stop checking an uncertain Host Operation. The host's state stays
  unknown; abandoning is not success.
  _Avoid_: dismiss, clear, retry.
- **Capability** — one thing farm3d can do through a Printer's Connection
  (upload, start, pause, resume, cancel, host state, artifact identity,
  camera), with whether it is supported and the evidence for it.
  Unsupported is not the same as failed.
  _Avoid_: feature, permission.
- **Connection** — updated: Moonraker has the P6 command capabilities;
  OctoPrint is status-only; ElegooLink is planned.

## Decisions

### D1. Composable capability interfaces

**Decision: small capability traits plus the adapter registry (plan
option C).** ADR-0011 records it.

Four `async_trait` traits, each `Send + Sync`, live in
`src-tauri/src/connections/capabilities.rs`. A command builds its
capability objects per operation from the Printer's Connection config and
credential. They never share the supervisor's observation socket.

```rust
#[async_trait]
pub trait ArtifactStaging: Send + Sync {
    /// Streams `artifact` to `artifact.host_path`. Never starts a print.
    async fn upload(
        &self,
        artifact: &StagedArtifact,
        body: Box<dyn AsyncRead + Send + Unpin>,
    ) -> Result<(), CommandFailure>;
    /// Reads `artifact.host_path` and compares size and SHA-256 (D4).
    async fn locate(&self, artifact: &StagedArtifact) -> Result<LocateOutcome, ConnectionError>;
}

#[async_trait]
pub trait PrintControl: Send + Sync {
    async fn start(&self, host_path: &str) -> Result<(), CommandFailure>;
    async fn pause(&self) -> Result<(), CommandFailure>;
    async fn resume(&self) -> Result<(), CommandFailure>;
    async fn cancel(&self) -> Result<(), CommandFailure>;
}

#[async_trait]
pub trait HostStateQuery: Send + Sync {
    async fn host_facts(&self) -> Result<HostFacts, ConnectionError>;
    async fn host_job_state(&self) -> Result<HostJobState, ConnectionError>;
    /// Requests newest first (`order=desc`) with `since_epoch_s` on the
    /// job's `start_time`. Callers never rely on either; they re-check (D5).
    async fn job_history(&self, query: HistoryQuery) -> Result<Vec<HistoryJob>, ConnectionError>;
}

#[async_trait]
pub trait CameraDiscovery: Send + Sync {
    async fn cameras(&self) -> Result<Vec<CameraInfo>, ConnectionError>;
}
```

Supporting types (Rust-only unless marked ts-rs):

| Type | Fields |
|---|---|
| `StagedArtifact` | `host_path: String`, `sha256: String` (lower-case hex), `size: u64` |
| `LocateOutcome` | `Absent`; `Matches`; `Differs { reason: DiffersReason }` where `DiffersReason` is `Size { actual: u64 }` or `Hash` |
| `CommandFailure` | `Definitive(HostOperationFailureCode)`; `Indeterminate { reason: InconclusiveReason, no_longer_pending: bool }` |
| `HostJobState` | `klippy_state: KlippyState` (`Ready`, `Startup`, `Shutdown`, `Error`, `Disconnected`); `print: Option<PrintSnapshot>` (present only when `klippy_state` is `Ready`); `tools: Vec<ToolTemperature>` (#29's type, every tool in index order); `bed: Option<BedTemperature { temp_c, target_c }>` (absent on a no-bed printer, never zero) |
| `PrintSnapshot` | `state: PrintStatsState` (`Standby`, `Printing`, `Paused`, `Complete`, `Cancelled`, `Error`, `Other(String)`); `filename: Option<String>` (empty string maps to `None`); `is_paused: bool` |
| `HistoryQuery` | `since_epoch_s: Option<f64>`, `limit: u32` |
| `HistoryJob` | `job_id: u64` (parsed from Moonraker's hex string; a job whose id does not parse is dropped), `filename: String`, `status: String` (a hint only, spike 6), `start_time_epoch_s: f64` |
| `HostFacts` (ts-rs) | `components: string[]`, `hasVirtualSdcard`, `hasPauseResume`, `hasHistory`, `hasHeaterBed`, `toolCount: number`, `cameraCount: number`, `hostSoftware: string`, `apiVersion: string` |
| `CameraInfo` | `name: String`, `service: String`. URLs are **not** kept: a real host's `stream_url` embeds its LAN address (Gate H). |

There is no single "nozzle" field anywhere. `ConnectionError` is the
existing `connections::ConnectionError` (`Unreachable`, `Auth`,
`Protocol`, `Timeout`). Every error string is credential-free and never
contains a raw host response body (spike 3: bodies carry tracebacks with
host file paths).

`AdapterDescriptor` (in `connections/adapters.rs`) gains:

```rust
pub staging: Option<StagingBuilder>,
pub control: Option<ControlBuilder>,
pub host_state: Option<HostStateBuilder>,
pub camera: Option<CameraBuilder>,
pub evidence: &'static [(CapabilityKey, CapabilityEvidence)],
```

Each builder is a plain `fn(&ConnectionConfig, Option<Zeroizing<String>>)
-> Box<dyn Trait>`, like `ObserveBuilder`. `evidence` is **per
capability** (see D6), because the UI shows each capability's own tier.

### D2. Host Operation record

Migration `0007_p6_host_operations.sql` creates a STRICT table:

| Column | Type | Meaning |
|---|---|---|
| `id` | TEXT PK, `GLOB 'hop-*'`, length 5–64 | `hop-<uuid v4>` |
| `operation_id` | TEXT NOT NULL UNIQUE | The client `operationId` that created the row. A replay finds the row by it. |
| `printer_id` | TEXT NOT NULL, FK `printers(id)` `ON DELETE RESTRICT` | |
| `kind` | TEXT, CHECK in `upload`, `start`, `pause`, `resume`, `cancel` | |
| `slice_revision_id` | TEXT NULL, FK `slice_revisions(id)` `ON DELETE SET NULL` | Upload and start |
| `source_host_operation_id` | TEXT NULL, FK `host_operations(id)` `ON DELETE SET NULL` | Start only: the succeeded upload it starts |
| `gcode_sha256` | TEXT NULL | Copied at creation. Upload and start. |
| `gcode_size` | INTEGER NULL, `> 0` | Copied at creation. Upload and start. |
| `host_path` | TEXT NOT NULL | Upload and start: `farm3d/<slr-id>.gcode`. Pause, resume, cancel: the `print_stats.filename` the host was printing when farm3d checked (D9). |
| `history_mark` | INTEGER NULL, `>= 0` | Start only: the newest history `job_id` before dispatch, `0` for an empty history (D5) |
| `endpoint_json` | TEXT NOT NULL, `json_valid` | `{ "kind", "host", "port" }`. Never a credential or `credentialRef`. |
| `state` | TEXT, CHECK in `dispatching`, `uncertain`, `reconciling`, `succeeded`, `failed`, `abandoned` | D3 |
| `failure_json` | TEXT NULL, `json_valid` | `HostOperationFailure` (D11) when `failed` |
| `resolution_json` | TEXT NULL, `json_valid` | `HostOperationResolution` (D11) when `succeeded` |
| `attempts` | INTEGER NOT NULL DEFAULT 0 | Reconciliation attempts that ended inconclusive |
| `last_attempt_at` | TEXT NULL | RFC 3339 |
| `last_attempt_reason` | TEXT NULL | An `InconclusiveReason` code (D11). Replaces the plan's `last_attempt_error`. |
| `no_longer_pending` | INTEGER NOT NULL DEFAULT 0, CHECK 0/1 | Evidence that Klipper restarted after dispatch (D5). Informational. |
| `abandoned_at`, `abandon_note` | TEXT NULL | D8. The note is at most 500 characters. |
| `created_at` | TEXT NOT NULL | The write-ahead commit |
| `dispatched_at` | TEXT NULL | Committed immediately **before** the adapter opens its connection (D3) |
| `uncertain_since` | TEXT NULL | When farm3d stopped waiting for the dispatch's answer. Set **only** on `dispatching → uncertain` (the executor's indeterminate result, or startup recovery of a sent row, D3), and never changed afterwards: `reconciling → uncertain` and startup recovery of a `reconciling` row keep it. The upload settle period counts from it. |
| `resolved_at` | TEXT NULL | Set on entering a terminal state |

CHECK constraints:

- `host_path` is NOT NULL for every kind.
- `gcode_sha256` and `gcode_size` are NOT NULL when `kind IN
  ('upload','start')` and NULL otherwise. (`slice_revision_id` is not
  CHECKed, because deleting a revision after its rows are terminal sets it
  NULL.)
- `history_mark IS NOT NULL` exactly when `kind = 'start'`.
- `source_host_operation_id` is NULL unless `kind = 'start'`.

Indexes and triggers:

- A partial unique index on `printer_id WHERE state IN
  ('dispatching','uncertain','reconciling')`: at most one unresolved row
  per Printer (answer 2).
- A `BEFORE UPDATE` trigger that raises when `OLD.state` is `succeeded`,
  `failed`, or `abandoned`: terminal rows are immutable.
- An index on `(printer_id, created_at)` for listing.

The migration also rebuilds the `operations` ledger table, exactly as
`0006_p5_slicing.sql` does (create, copy, drop, rename), adding the kinds
`stageSliceRevision`, `startStagedArtifact`, `pauseHostPrint`,
`resumeHostPrint`, `cancelHostPrint`, and `abandonHostOperation`.
`CURRENT_SCHEMA_VERSION` becomes 7.

**Write-ahead.** The row is committed as `dispatching` (with its ledger
claim, in one transaction) before farm3d contacts the host for the write.
**Idempotency.** Each write command claims its `operationId` in the
operations ledger. A replay (same kind and request digest) returns the
current row found by `operation_id`, with no pre-check and no host call.
A reused id with a different request is the existing `OperationIdReused`
error. A rejected command rolls back and never burns its id.

### D3. State machine

```text
dispatching ─ definitive success (D5) ───────> succeeded
dispatching ─ definitive failure ────────────> failed
dispatching ─ indeterminate / timeout / panic ─> uncertain
dispatching ─ startup, never sent ───────────> failed{neverSent}
dispatching ─ startup, sent ─────────────────> uncertain
uncertain ─ reconcile attempt begins ────────> reconciling
reconciling ─ proved applied ────────────────> succeeded
reconciling ─ proved not applied (upload) ───> failed
reconciling ─ inconclusive ──────────────────> uncertain   (attempts += 1)
reconciling ─ startup ───────────────────────> uncertain   (attempts unchanged)
uncertain ─ operator abandons (D8) ──────────> abandoned
```

Every other pair is illegal. `host_ops/state.rs` implements this as a
pure `transition(from, event) -> Result<HostOperationState,
IllegalTransition>` and the repository calls it before any SQL.

**Two commits around the send.** The executor commits `dispatched_at`
(`mark_sent`) immediately before the adapter opens its connection.

- **Invariant: the executor sends nothing unless `mark_sent` returned
  `Ok`.** If `mark_sent` fails, the executor sends nothing and either
  commits `failed { neverSent }` or, if that commit also fails, leaves the
  row for startup recovery (which makes it `neverSent`, below).
- Every local precondition runs **before** `mark_sent`: building the
  capability object, reading the credential, and, for an upload, opening
  the bytes with `ContentStore::open_verified(gcode_sha256)`. A failure
  there sends nothing and commits `failed { neverSent }`.

So, at startup:

- a `dispatching` row with `dispatched_at IS NULL` at startup was never
  sent. Recovery makes it `failed` with `neverSent`. This is safe because
  the writer runs with `synchronous = FULL`.
- a `dispatching` row with `dispatched_at` set at startup may have been
  sent. Recovery makes it `uncertain` with `uncertain_since = now` and
  reason `interruptedByRestart`.
- a `reconciling` row at startup (a crash mid-attempt) returns to
  `uncertain`, without counting the attempt and keeping its
  `uncertain_since`.

Startup recovery (`recover_after_restart`) runs inside
`build_runtime_services`, before commands are served and before the first
reconcile pass.

An unsupported capability never creates a row (`CAPABILITY_UNSUPPORTED`).
Nor does any pre-check rejection in D9.

### D4. Staging and artifact identity

**Host path.** `farm3d/<slr-id>.gcode` in Moonraker's `gcodes` root.
Moonraker creates `farm3d/` on demand (Gate B).

**Transport.** The capability adapter uses Moonraker's **HTTP API only**
(see D10). The WebSocket stays the supervisor's observation channel.

**Upload request.** `POST /server/files/upload`, `multipart/form-data`,
parts in this order:

1. `root` = `gcodes`
2. `path` = `farm3d`
3. `checksum` = the revision's `gcode_sha256`, lower-case hex
4. `file`, filename `<slr-id>.gcode`, `Content-Type:
   application/octet-stream`, streamed from
   `ContentStore::open_verified(gcode_sha256)`

The form builder has **no way** to add a `print` field: its only inputs
are a `StagedArtifact` and a body stream. `FakeMoonraker` fails any test
whose upload carries a `print` field.

**Checksum decision.** farm3d always sends `checksum`. A 422 is a
definitive `checksumRejected` failure (Gate B: nothing is left behind).
farm3d does **not** rely on the host having checked it: whether
Snapmaker's fork verifies it is unknown, and a Moonraker that does not
register the field ignores it (spike 1). A 201 therefore never makes an
upload `succeeded` by itself. The executor then runs `locate`, and only a
`Matches` result makes it `succeeded` (D5).

**`locate` (download-and-hash).** No Moonraker endpoint exposes a content
hash, and size, `modified`, and the metadata `uuid` are not identity
(Gate C). `locate` is one streamed request:

- `GET /server/files/gcodes/<host_path>`, with each path segment
  percent-encoded.
- 404 → `Absent`.
- 200 with a `Content-Length` other than `size` → `Differs { Size }`
  (farm3d stops reading).
- 200 → stream the body through SHA-256, counting bytes. A byte count
  other than `size` → `Differs { Size }`. A digest other than `sha256` →
  `Differs { Hash }`. Otherwise `Matches`.
- A timeout, reset, 401, other status, or read error →
  `Err(ConnectionError)`: inconclusive, never `Absent`.

Size alone never proves identity. A 404 is filesystem-backed, so `locate`
does not depend on Moonraker's metadata cache.

**Partial-file rule.**

- Moonraker writes to `host_path` only after the whole body arrived and
  the checksum passed, so an interrupted upload leaves either the complete
  file or nothing at `host_path` (Gate D). farm3d still treats anything at
  `host_path` that is not a `Matches` as **not ours**.
- A file at `host_path` that `Differs` is treated like an absent one: it
  is `uploadSettling` until the settle period ends, then `failed` with
  `hostFileDiffers` (D5). A late body can still replace it with our bytes
  during the settle period.
- farm3d **never deletes or overwrites** a file on the host by itself.
  Staging again is a new, operator-initiated operation, and Moonraker then
  replaces the file (201) unless it is loaded (403, Gate B).
- Moonraker's orphaned `moonraker.upload-*.mru` temp files (Gate D) are
  invisible to its API. farm3d accepts that they build up and promises no
  cleanup.

### D5. Definitive responses and reconciliation

#### Definitive and indeterminate responses at dispatch

Classification is by HTTP status, and for 400 and 503 also by the error
`message` in Moonraker's JSON body. The body is parsed for that one field
and then dropped.

A **connect failure** (connection refused, connect timeout, DNS failure;
reqwest `is_connect()`) happens before any request byte is sent, so it is
definitive for every kind: `hostUnreachable`.

| Kind | Definitive success | Definitive failure → `failed` (code) | Indeterminate → `uncertain` (reason) |
|---|---|---|---|
| upload | 201, **then** `locate` = `Matches` | 401 `authRejected`; 403 `fileLoaded`; 422 `checksumRejected`; 400 `hostRejected`; connect failure `hostUnreachable` | 201 then `locate` not `Matches` or unreadable (`identityCheckFailed`, or `uploadSettling` for `Absent`/`Differs`); timeout, reset, or no response (`responseLost`); 5xx or any other status or body (`unexpectedResponse`) |
| start | 200 with `{"result":"ok"}` | 400 containing `SD busy` `hostBusy`; 400 containing `Unable to open file` `fileMissing`; any other 400 `hostRejected`; 503 `Klippy Host not connected` `hostNotReady`; 401 `authRejected`; connect failure `hostUnreachable` | 503 `Klippy Disconnected` (`klipperRestarted`, sets `no_longer_pending`); timeout, reset, or no response (`responseLost`); 5xx, other status, or a 200 whose result is not `"ok"` (`unexpectedResponse`) |
| pause, resume, cancel | 200 `{"result":"ok"}` **and** the verb's effect observed within the verification window (below) | as for start | as for start; also 200 `ok` with the effect not observed in the window (`effectNotObserved`) |

Pause, resume, and cancel answer `ok` even when nothing happened (Gate E),
so the response alone never proves them. After the `ok`, the executor
polls `host_job_state` every **500 ms** for up to **10 s** (the
verification window) and commits `succeeded` as soon as the D5 "proved
applied" rule for that verb holds.

A 200 `ok` to **start** is definitive success (Gate E: `printing` within
0.05 s). The Start rule (D9) keeps start from Paused, where Klipper would
also answer `ok` and restart the file from byte 0 (spike 8).

A panic in the executor after `mark_sent` makes the row `uncertain`
(`unexpectedResponse`); before `mark_sent` it makes the row `failed`
(`neverSent`). If the verification window's reads fail, the row becomes
`uncertain` with the last read's reason (`hostUnreachable`,
`hostNotReady`) or `effectNotObserved`.

#### Reconciliation rules

Reconciliation proves an outcome for an `uncertain` row by **reading** the
host. It never issues a write. It uses the row's `endpoint_json` (not the
Printer's current config) with the Printer's current credential.

Final constants:

| Name | Value | Source |
|---|---|---|
| `SETTLE_PERIOD` | 60 s, counted from `uncertain_since` | Gate D: the latest late arrival was 7.3 s after the client gave up; spike 7 |
| `START_SKEW_TOLERANCE` | 30 s | Gate F: measured offset −0.08…+0.01 s, detection delay ≈0.05 s; spike 5 |
| `HISTORY_QUERY_LIMIT` | 50 jobs | |

| Kind | Proved applied → `succeeded` | Proved not applied → `failed` | Otherwise → `uncertain` (reason) |
|---|---|---|---|
| upload | `locate` = `Matches` | `locate` = `Absent` and `now ≥ uncertain_since + SETTLE_PERIOD` → `notApplied`. `locate` = `Differs` and the same settle condition → `hostFileDiffers`. | `Absent`/`Differs` inside the settle period (`uploadSettling`); `locate` errored (`identityCheckFailed`, `hostUnreachable`, `authRejected`) |
| start | **(a)** `print_stats.state` is `printing` or `paused` and `print_stats.filename == host_path`; **or (b)** a history job with `filename == host_path`, `job_id > history_mark`, and `start_time ≥ dispatched_at − START_SKEW_TOLERANCE` (any `status`) | **Never** by reconciliation. Only a definitive response at dispatch fails a start. | no evidence yet (`noStartEvidence`); host printing or paused a different file (`differentFileOnHost`); Klipper not ready (`hostNotReady`); unreachable (`hostUnreachable`) |
| pause | `print_stats.state == paused` and `filename == host_path` | Never | `effectNotObserved`, or as above |
| resume | `print_stats.state` is `printing` or `complete`, and `filename == host_path` | Never | as above |
| cancel | `print_stats.state == cancelled` and `filename == host_path` | Never | as above |

Rules that make these exact:

- **Upload absence is fail-safe.** "Absent" right after the client gave up
  proves nothing: in Gate D a file absent at the first check appeared 7.3 s
  later. Only absence that has outlasted `SETTLE_PERIOD` counts. farm3d
  does not try to detect a Moonraker restart as a faster proof (see
  "Decisions made in this spec", item 3). 60 s is a margin chosen on the
  spike's evidence, not a proven bound, and it is recorded as a residual
  risk.
- **Start: `complete` alone is never proof** (answer 12, spike 6). A
  previous print of the same Slice Revision leaves the same `complete`
  and filename. Rule (b) disambiguates: the job must be newer than the
  high-water mark **and** have started after `dispatched_at` minus the
  tolerance.
- **The reconcile history query.** A start attempt reads
  `job_history({ since_epoch_s: Some(dispatched_at − START_SKEW_TOLERANCE),
  limit: HISTORY_QUERY_LIMIT })`
  (`GET /server/history/list?limit=50&order=desc&since=<epoch s>`), then
  applies rule (b) itself to every returned job: it re-checks
  `filename`, `job_id > history_mark`, and the `start_time` bound, and does
  not trust `since` or `order` to have filtered or sorted. No qualifying
  job means `noStartEvidence`.
- **Confirming the parameters.** Task 8's fixtures and Task 12's
  simulator run must confirm that Moonraker v0.11.0 honours `since` and
  `order=desc` on `/server/history/list`. If `order` is not honoured, the
  newest jobs could fall outside a 50-job page and the mark would be too
  low; the time rule still guards, but that result must be recorded, and
  the spec revisited, before Task 12 adds evidence rows.
- **Comparisons are numeric, never string.** `job_id` is parsed from
  Moonraker's hex string (for example `0000A1`) to `u64` and compared with
  the stored integer `history_mark`. `start_time` is Moonraker's epoch
  float. `dispatched_at` is parsed from RFC 3339 to epoch seconds before
  subtracting the tolerance. Filenames are compared as exact strings.
- **The high-water mark.** Before the write-ahead, the start command reads
  `job_history({ since_epoch_s: None, limit: HISTORY_QUERY_LIMIT })`
  (`GET /server/history/list?limit=50&order=desc`). The mark is the
  **maximum parsed `job_id` over the returned page**, never "the first
  job", so it does not depend on Moonraker honouring `order`; it is `0` if
  the page is empty. If the history cannot be read,
  the start is refused with no row written (D9). The mark is clock-free;
  the time rule guards against ids that Moonraker reuses after the newest
  jobs are deleted (Gate F).
- **Interrupted starts are applied.** A qualifying job whose status is
  `klippy_disconnect`, `klippy_shutdown`, or `server_exit` proves the start
  ran. The resolution records `interrupted: true`. It is never "not
  applied".
- **Job status is a hint.** Rule (b) ignores `status` for the decision,
  because Moonraker can record `completed` for a file that did not run, or
  `server_exit` for a print that continued (Gate F cases 2, 3).
- **No "not applied" path for control or start.** A start, pause, resume,
  or cancel whose response was lost can still be queued behind other
  G-code and apply later (Gate E: 36.5 s after dispatch; spike 4, 4a). So
  no observation proves it did not apply. Only a definitive response at
  dispatch fails it. Otherwise it stays `uncertain` until it is proved
  applied or abandoned.
- **Known gaps accepted.** A start that ran and finished within one
  Moonraker status batch can leave no history job and no `print_stats`
  change (Gate F case 1). That row stays `uncertain` until abandoned. A
  resume or cancel that another client performed is attributed to farm3d's
  operation when the state matches; the effect the operator asked for
  happened either way.
- **No longer pending (informational).** `no_longer_pending` is set, and
  stays set, when farm3d sees evidence that Klipper restarted after
  dispatch: a 503 `Klippy Disconnected` answer to the write itself, or a
  reconcile read that finds `klippy_state` not `ready` (including
  Moonraker's 503 `Klippy Host not connected`). Klipper was ready at
  dispatch (D9), so either means it went away afterwards and dropped any
  queued G-code (Gate E, fix round 1). It **never** changes the state: a
  short start can have run before the restart without a trace (spike 4).
  The UI uses it in the uncertain and abandon copy. `idle_timeout.state`
  is not used as evidence (spike 4: rejected).

#### When reconciliation runs

- **Startup**, after `restore_persisted_connections` in `lib.rs`: first
  `recover_after_restart` (D3), then one attempt for every `uncertain` row.
- **When the supervisor reports Online** for a Printer (a
  `ConnectionManager` hook), one attempt for that Printer's `uncertain`
  row. The same hook refreshes the Printer's cached `HostFacts` (D6).
- **Check again** (`reconcile_host_operation`): one attempt now, and the
  backoff resets.
- **Capability gate.** An attempt needs, per `capabilities_for`:
  `artifactIdentity` for an `upload` row (`locate`), and `hostState` for a
  `start`, `pause`, `resume`, or `cancel` row. If that capability is
  unsupported (for any reason), no attempt runs: the automatic triggers
  skip the row, and `reconcile_host_operation` returns
  `CAPABILITY_UNSUPPORTED`. Such a row is **structurally unreconcilable**,
  and D8 lets the operator abandon it without an attempt.
- **Retry.** After an inconclusive attempt, while the Printer is Online,
  the next attempt is scheduled after
  `supervisor::backoff_delay(attempts)` (1 s doubling to 60 s). For an
  upload whose reason is `uploadSettling`, an attempt is also scheduled
  for exactly `uncertain_since + SETTLE_PERIOD`. While the Printer is not
  Online, nothing is scheduled; the Online hook resumes it.
- **Serialization.** At most one attempt runs per Printer at a time, under
  the same per-Printer lock as the write commands. An attempt commits
  `uncertain → reconciling` first, then its outcome.
- The clock is injectable (`host_ops` takes a `Clock`), and
  `SETTLE_PERIOD`, `START_SKEW_TOLERANCE`, `HISTORY_QUERY_LIMIT`, and the
  retry backoff are fields of a `HostOpsTimings` config struct whose
  `default()` holds the values above. Tests inject a fake clock and short
  values, so no test waits 60 s in real time.

### D6. Capability matrix

```ts
type CapabilityKey = "upload" | "start" | "pause" | "resume" | "cancel"
  | "hostState" | "artifactIdentity" | "camera";
type CapabilityEvidence = {
  source: string;                       // the committed manifest copy, e.g. "docs/superpowers/baselines/<date>-p6-sim-manifest-<UTC>.json"
  tier: "sim" | "readOnlyHardware";
  verifiedHostVersions: string[];       // e.g. ["Moonraker v0.11.0-1 API 1.5.0"]
};
type CapabilityState =
  | { status: "supported"; evidence: CapabilityEvidence }
  | { status: "unsupported"; reason: "adapter" | "notVerified" | "host"; detail: string };
type PrinterCapabilities = {
  printerId: string;
  adapterKind: string | null;
  capabilities: Record<CapabilityKey, CapabilityState>;
  hostFacts: HostFacts | null;
  observedAt: string | null;            // when hostFacts were read
};
type AdapterCapabilityRow = {
  adapterKind: string;
  capabilities: Record<CapabilityKey, CapabilityState>;   // no host rules applied
};
```

Changed from the plan: evidence moved from `PrinterCapabilities` onto each
supported capability, because each capability can have its own tier
(camera's supporting evidence is partly read-only hardware, while writes
must be `sim`).

`capabilities_for(printer: &StoredPrinter, host_facts: Option<&HostFacts>)
-> PrinterCapabilities` is public Rust for P7. It applies these rules in
order; the first match for a capability wins:

1. No Connection → every capability `unsupported`, `adapter`, detail "No
   Connection".
2. Unknown `kind` → every capability `unsupported`, `adapter`, detail
   "farm3d can't use this Connection type."
3. `useTls: true` → every write capability (`upload`, `start`, `pause`,
   `resume`, `cancel`) `unsupported`, `notVerified`, with detail
   `TLS_UNSUPPORTED_MESSAGE`.
4. The descriptor has no builder for the capability (see mapping below),
   or no `evidence` row for it → `unsupported`, `notVerified`, detail "Not
   verified for this Connection type yet."
5. An evidence row whose tier is `readOnlyHardware` never makes a write
   capability `supported`: → `notVerified`.
6. Host rules, applied only when `host_facts` is `Some`:
   - no `virtual_sdcard` → `upload` and `start` `host`, "This printer has
     no virtual SD card.";
   - no `history` component → `start` `host`, "This printer's Moonraker
     keeps no job history, so farm3d can't confirm a start.";
   - no `pause_resume` → `pause` and `resume` `host`, "This printer has no
     pause and resume support.";
   - `cameraCount == 0` → `camera` `host`, "No camera is configured on this
     printer.".
7. Otherwise `supported`, carrying the evidence row.

Builder mapping: `upload` and `artifactIdentity` need `staging`; `start`,
`pause`, `resume`, `cancel` need `control`; `start` also needs
`host_state` (reconciliation); `hostState` needs `host_state`; `camera`
needs `camera`.

**Host facts.** `host_facts()` combines `GET /server/info` (components),
`GET /printer/objects/list`, and `cameras()`. Tools are the objects that
match `^extruder\d*$` exactly, never a prefix: the real host also has
`extruder_offset_calibration` (Gate H, spike 11). Host facts are cached in
memory per Printer, refreshed on each Online transition, and never
persisted. Before the first refresh, `hostFacts` is `null` and no host
rule applies; the write commands' own pre-checks still run.

**Rows at the end of P6.**

- Moonraker: Task 8 adds the four builders with every capability
  `notVerified` (no evidence rows). Task 12 adds evidence rows with
  `tier: "sim"` and the run's manifest path for `upload`, `start`,
  `pause`, `resume`, `cancel`, `hostState`, and `artifactIdentity`.
  `camera` gets a `sim` row for the query (the simulator has no webcam,
  Gate H); read-only results may add `verifiedHostVersions` but never a
  write capability.
- OctoPrint: no builders; every capability `notVerified`.
- Host facts list every tool and are never collapsed into one nozzle.

### D7. Guards

"Unresolved" means a row in `dispatching`, `uncertain`, or `reconciling`.
Every guard runs inside the mutation's own transaction.

| Mutation while unresolved | Result |
|---|---|
| Archive, delete a Printer | `LIFECYCLE_BLOCKED`, blocker code `HOST_OPERATION_UNRESOLVED`, messages "Finish or abandon the pending printer operation before archiving." / "Finish or abandon the pending printer operation before deleting." |
| Printer import (`import_printers`, `replace_all`) | `HOST_OPERATION_PENDING` for the whole import, nothing written |
| Change `kind`, `host`, `port`, or `useTls`; clear the Connection; clear the credential reference | `CONNECTION_IN_USE` |
| Replace the credential reference, endpoint unchanged | Allowed, and still probed (answer 4) |
| Delete a Slice Revision that an unresolved `upload` or `start` row references | `LIFECYCLE_BLOCKED`, blocker code `HOST_OPERATION_UNRESOLVED`, message "A printer operation using this Slice Revision is still pending. Finish or abandon it first." |
| A new write on the same Printer | `HOST_OPERATION_PENDING` (D9) |

- `succeeded`, `failed`, and `abandoned` rows never block.
- Permanent Printer delete removes that Printer's terminal rows in the same
  transaction, after the blocker check (answer 5). The `ON DELETE RESTRICT`
  foreign key backs up the guard.
- Printer import (`replace_all`) replaces every Printer, so once the
  import guard passes it deletes the terminal rows of every Printer it
  replaces, in the same transaction, before the Printers themselves. Their
  Host Operation history is therefore lost on re-import, even for a
  Printer the imported file brings back under the same id. (Controller
  ruling R16; the foreign key is `ON DELETE RESTRICT`, so the rows can't
  outlive their Printer.)
- Credential cleanup (`retry_pending_credential_cleanup_locked`) never
  deletes the credential of a Printer with an unresolved row.

### D8. Abandon reconciliation

`abandon_host_operation({ operationId, hostOperationId, acknowledgement:
"hostStateUnknown", note? })`:

- Allowed only when the row is `uncertain` **and** either `attempts >= 1`
  (at least one reconciliation attempt ended inconclusive) or the row is
  structurally unreconcilable (D5 "Capability gate": the capability its
  kind needs is unsupported now), in which case `attempts == 0` is
  allowed. Otherwise `HOST_OPERATION_NOT_ABANDONABLE`.
- `acknowledgement` must be the literal `"hostStateUnknown"`, else
  `VALIDATION` on `acknowledgement`. `note` is optional, trimmed, and at
  most 500 characters, else `VALIDATION` on `note`.
- The row becomes the terminal `abandoned` state with `abandoned_at`,
  `abandon_note`, and `resolved_at`. It is kept. Guards treat it as
  resolved.
- farm3d sends nothing to the host.
- Abandoned is **not** success. P7 treats it as reconciliation-required.
- The UI is a Kobalte `AlertDialog` with a required acknowledgement
  checkbox (Frontend).

### D9. Start rule and control rule

**Start table** (answers 1, 12, 13):

| Printer `OperationalState` (fresh telemetry) | Start | Allowed `priorState` | Required checkbox |
|---|---|---|---|
| `Ready` | Offered | `ready` | "The bed is clear." |
| `Finished` | Offered | `finished` | "The previous print finished. The bed is clear." |
| `Cancelled` | Offered | `cancelled` | "The previous print was cancelled. The bed is clear." |
| `Failed` | Disabled: "Clear the error on the printer first." | — | — |
| `Printing`, `Paused`, `Busy`, `Offline`, `Connecting`, `Unknown`, `Error`, `SetupIncomplete`, or freshness not `fresh` | Disabled, with the state as the reason | — | — |

- It applies whatever the Printer's Start-safety rule is. P6 never starts
  unattended. P6 sends no reset command.
- `host_ops/start_rule.rs` (pure):
  `check(status: &PrinterStatus, prior: PriorState) -> Result<(), StartRejection>`
  with `StartRejection::NotAllowed { observed_state, freshness }` and
  `StartRejection::PreconditionChanged { observed_state, freshness }`.
  `NotAllowed` when the state is not in the offered set or freshness is
  not `Fresh`; `PreconditionChanged` when it is offered but not for
  `prior`.

**`start_staged_artifact` order.** Under the per-Printer lock:

1. Replay check (D2).
2. The Printer exists (`NOT_FOUND`) and is not archived (`VALIDATION` on
   `printerId`: "Unarchive this Printer first.").
3. `capabilities_for` says `start` is supported, else
   `CAPABILITY_UNSUPPORTED`.
4. No unresolved row for the Printer, else `HOST_OPERATION_PENDING`.
5. `hostOperationId` names an `upload` row for this Printer in
   `succeeded`: else `NOT_FOUND` (no such row) or `VALIDATION` on
   `hostOperationId` (wrong kind, Printer, or state).
6. `start_rule::check` on the live `PrinterStatus` from
   `ConnectionManager`: `START_NOT_ALLOWED` or `START_PRECONDITION_CHANGED`.
7. **Host re-read**: `host_job_state`. If `klippy_state` is not `Ready`, or
   `print.state` is `printing` or `paused`, → `START_NOT_ALLOWED` with the
   observed state (the live status can lag the host by seconds, and start
   from Paused is accepted by Klipper, spike 8). Only a host state that
   maps to `ready`, `finished`, or `cancelled` goes on: `error` (the last
   print failed, ruling R21) and, fail-safe, a `print.state` farm3d doesn't
   know or no `print_stats` at all (`unknown`) are refused the same way.
8. **High-water mark**: the D5 query (max parsed `job_id` over a 50-job
   page).
9. **Identity**: `locate` of the staged artifact on the current endpoint.
   `Absent` → `STAGED_ARTIFACT_INVALID` (`reason: "absent"`); `Differs` →
   `STAGED_ARTIFACT_INVALID` (`reason: "differs"`). farm3d never starts
   bytes it has not just verified.
10. `start_rule::check` again on the now-current live status (steps 7–9 can
    take seconds).
11. Write-ahead: claim + insert `dispatching` (with `history_mark`,
    `source_host_operation_id`, and the upload row's `slice_revision_id`,
    `gcode_sha256`, `gcode_size`, `host_path`), commit, emit.
12. Return the row. The executor continues in the background:
    `mark_sent`, then `start(host_path)`.

A read failure in steps 7–9 maps through the existing network errors
(`PRINTER_UNREACHABLE`, `TIMEOUT`, `AUTHENTICATION_FAILED`,
`PROTOCOL_ERROR`) and writes no row.

**The write-ahead's own re-checks** (every write command). The
Connection commands don't take the per-Printer lock, so the write-ahead
transaction re-reads the Printer before it claims the operation id: it
still exists (`NOT_FOUND`) and is not archived; its Connection (`kind`,
`host`, `port`, `useTls`, and credential reference) is exactly the one the
pre-checks used, else no row and `START_PRECONDITION_CHANGED` for start
or `VALIDATION` on `printerId` ("The Printer's Connection changed. Try
again.") for stage and control; it has no unresolved row
(`HOST_OPERATION_PENDING`); and the row's Slice Revision, if any, still
exists (`NOT_FOUND`). (Final-review fix I1 and m9.)

**Control rule** (spike 9: pause from idle sets `is_paused`):

| Verb | Offered when `OperationalState` is (fresh) | Host re-read must show `print.state` |
|---|---|---|
| pause | `Printing` | `printing` |
| resume | `Paused` | `paused` |
| cancel | `Printing` or `Paused` | `printing` or `paused` |

`pause_host_print`, `resume_host_print`, and `cancel_host_print` run steps
1–4 above (with the verb's capability), then the control rule on the live
status and then on a host re-read, returning `CONTROL_NOT_ALLOWED` with no
row when either fails. The re-read's `print.filename` becomes the row's
`host_path` (D2, D5); a re-read with no filename is also
`CONTROL_NOT_ALLOWED`. Then write-ahead and the background executor.

**`stage_slice_revision` order.** Steps 1–4 (capability `upload`), then
the Slice Revision exists (`NOT_FOUND`), then the Printer's
`connectionState` is `online` (else `PRINTER_UNREACHABLE`, no row), then
write-ahead and the background executor (`mark_sent`, upload, then
`locate` on 201).

### D10. Moonraker HTTP client

- One `reqwest` client per operation, `default-features = false` plus
  `multipart` and `stream`, no TLS feature. No redirects
  (`redirect::Policy::none()`), no system proxy (`no_proxy()`), the same
  rules as the OctoPrint adapter.
- `X-Api-Key` on **every** request when the Printer has a credential,
  marked sensitive. It never appears in an error, `Debug` output, log,
  event, or row.
- Base URL `http://<host>:<port>`. `useTls` never reaches this code
  (D6 rule 3).

Timeouts (a timeout is always indeterminate for a write and inconclusive
for a read). They are fields of a `MoonrakerTimings` config struct passed to
the capability builders; the values below are its production defaults
(`MoonrakerTimings::default()`), and tests inject short ones:

| Name | Value | Applies to |
|---|---|---|
| `CONNECT_TIMEOUT` | 5 s | every request |
| `QUERY_TIMEOUT` | 10 s | every JSON `GET` |
| `CONTROL_TIMEOUT` | 60 s | `POST /printer/print/{start,pause,resume,cancel}`. Long, because a queued start answered after 15.8 s (Gate E). |
| `TRANSFER_TIMEOUT(size)` | 60 s + 1 s per started MiB of `gcode_size` | the upload and the `locate` download. LAN cost is unmeasured (Gate C), so the bound is a timeout, never a size limit. |
| `CONTROL_VERIFY_WINDOW` | 10 s, polled every 500 ms | D5 |

`CONTROL_VERIFY_WINDOW` is not a `MoonrakerTimings` field: the executor
runs the verification, so it reads the window and poll interval from
`HostOpsTimings` (`verify_window`, `verify_poll_interval`; ruling R19).

Endpoints (all relative to the gcodes root where a path applies):

| Use | Request |
|---|---|
| Upload | `POST /server/files/upload` (D4) |
| Locate | `GET /server/files/gcodes/<host_path>` |
| Start | `POST /printer/print/start?filename=<percent-encoded host_path>` |
| Pause / resume / cancel | `POST /printer/print/pause` / `resume` / `cancel` |
| Klippy state, components | `GET /server/info` |
| Objects | `GET /printer/objects/list` |
| Job state | `GET /printer/objects/query?webhooks&print_stats&pause_resume&heater_bed&<each extruder object>` (only when `klippy_state` is `ready`) |
| History | `GET /server/history/list?limit=<n>&order=desc[&since=<epoch s>]` |
| Cameras | `GET /server/webcams/list` |

### D11. Outcome vocabulary

**`HostOperationFailure`** (`failure_json`): `{ code: HostOperationFailureCode, message: string }`.

| Code | When | Message |
|---|---|---|
| `neverSent` | startup found the row unsent (D3) | "farm3d closed before sending this. Nothing reached the printer." |
| `hostUnreachable` | connect failure at dispatch | "farm3d couldn't connect to the printer. Nothing was sent." |
| `authRejected` | 401 | "The printer rejected farm3d's API key." |
| `checksumRejected` | upload 422 | "The printer found the upload damaged and discarded it." |
| `fileLoaded` | upload 403 | "The printer is using a file with this name, so it refused the upload." |
| `hostBusy` | 400 `SD busy` | "The printer is busy with another print." |
| `fileMissing` | 400 `Unable to open file` | "The printer couldn't find the staged file." |
| `hostRejected` | any other 400 | "The printer refused the request." |
| `hostNotReady` | 503 `Klippy Host not connected` | "Klipper isn't running on the printer." |
| `notApplied` | upload absent after the settle period (including after a 201 whose file later disappeared) | "The file isn't on the printer, and it didn't appear within a minute." |
| `hostFileDiffers` | a different file at `host_path` after the settle period | "A different file is at farm3d's path on the printer. Staging again replaces it." |

**`HostOperationResolution`** (`resolution_json`), a tagged union on
`kind`:

```ts
type HostOperationResolution =
  | { kind: "artifactVerified"; reconciled: boolean }
  | { kind: "startAccepted" }                           // 200 ok at dispatch
  | { kind: "startObserved"; source: "printStats" | "history";
      historyJobId: string | null; interrupted: boolean }
  | { kind: "stateObserved"; observedState: "printing" | "paused" | "complete" | "cancelled";
      reconciled: boolean };
```

`historyJobId` is Moonraker's hex id string, for display only.

**`InconclusiveReason`** (`last_attempt_reason`, and the reason an
executor result became `uncertain`):

| Code | Meaning / UI text |
|---|---|
| `responseLost` | "The printer's answer was lost." |
| `unexpectedResponse` | "The printer gave an answer farm3d doesn't understand." |
| `interruptedByRestart` | "farm3d closed while this was being sent." |
| `klipperRestarted` | "Klipper restarted while this was waiting." |
| `hostUnreachable` | "farm3d can't reach the printer to check." |
| `authRejected` | "The printer rejected farm3d's API key while checking." |
| `hostNotReady` | "Klipper isn't ready, so farm3d can't check yet." |
| `identityCheckFailed` | "farm3d couldn't read the file back to check it." |
| `uploadSettling` | "The file isn't on the printer, or doesn't match yet. farm3d waits a minute before deciding." |
| `noStartEvidence` | "The printer shows no sign that this print started." |
| `differentFileOnHost` | "The printer is busy with a different file." |
| `effectNotObserved` | "The printer hasn't shown the change yet." |

## Backend model

### Module layout

`src-tauri/src/host_ops/`:

| File | Content |
|---|---|
| `mod.rs` | `HostOperationServices` (repository, executor, reconciler, stream, host-facts cache, per-Printer locks, clock) and the ts-rs domain types |
| `state.rs` | D3 transition function |
| `repository.rs` | SQL: `insert_dispatching`, `mark_sent`, `transition`, `record_attempt`, `set_no_longer_pending`, `load`, `load_by_operation_id`, `list_for_printer`, `list_unresolved`, `snapshot`, `recover_after_restart`, `delete_terminal_for_printer`, `has_unresolved(printer_id)` |
| `executor.rs` | D5 dispatch classification and commit, with injectable fault points: before `mark_sent`, after `mark_sent` but before send, after send, and after the response but before commit |
| `reconciler.rs` | D5 rules and scheduling. Read-only. |
| `guards.rs` | `HostOperationBlockers` (Printer lifecycle), `UnresolvedHostOperationBlocksRevisionDeletion` (Slice Revision), and the Connection and import checks |
| `start_rule.rs` | D9 Start table and control rule, pure |
| `events.rs` | the `hostOperations` stream |
| `commands.rs` | Tauri commands |

Plus `connections/capabilities.rs` (D1, D6), `connections/moonraker/files.rs`
(pure: form parts, response classification, parsers) and
`connections/moonraker/control.rs` (the HTTP I/O). `RuntimeServices` gains
`host_ops: Arc<HostOperationServices>`. Startup order in `lib.rs`:
`build_runtime_services` (includes `recover_after_restart`) →
`restore_persisted_connections` → first reconcile pass.

### Wire types (ts-rs, `domain/`)

```ts
type HostOperationKind = "upload" | "start" | "pause" | "resume" | "cancel";
type HostOperationState = "dispatching" | "uncertain" | "reconciling"
  | "succeeded" | "failed" | "abandoned";
type PriorState = "ready" | "finished" | "cancelled";
type HostOperation = {
  id: string;                         // hop-*
  printerId: string;
  kind: HostOperationKind;
  state: HostOperationState;
  sliceRevisionId: string | null;
  sourceHostOperationId: string | null;
  gcodeSha256: string | null;
  gcodeSize: number | null;
  hostPath: string;
  endpoint: { kind: string; host: string; port: number };
  failure: HostOperationFailure | null;
  resolution: HostOperationResolution | null;
  attempts: number;
  lastAttempt: { at: string; reason: InconclusiveReason } | null;
  noLongerPending: boolean;
  abandonedAt: string | null;
  abandonNote: string | null;
  createdAt: string;
  dispatchedAt: string | null;
  uncertainSince: string | null;
  resolvedAt: string | null;
};
type HostOperationsSnapshot = {
  streamId: string;
  snapshotSequence: number;
  operations: HostOperation[];
};
```

`history_mark` is backend-only. Also exported: `HostOperationFailure`,
`HostOperationFailureCode`, `HostOperationResolution`,
`InconclusiveReason`, `CapabilityKey`, `CapabilityState`,
`CapabilityEvidence`, `PrinterCapabilities`, `AdapterCapabilityRow`,
`HostFacts`, `HostOperationsEventType`. Existing types gain:
`LifecycleBlockerCode::HostOperationUnresolved` (`HOST_OPERATION_UNRESOLVED` on the wire, matching the enum's existing SCREAMING_SNAKE serialization)
and the six `OperationKind` variants (D2).

### Commands

Final names, each registered per Global Constraint 7. Arguments are
top-level camelCase fields, as every existing command takes them. Count
assertions add these 10 to `main`'s total.

| Command | Arguments | Result |
|---|---|---|
| `printer_capabilities` | `{ printerId: string }` | `PrinterCapabilities` |
| `adapter_capability_matrix` | `{}` | `AdapterCapabilityRow[]` (registry order) |
| `list_host_operations` | `{ printerId?: string }` | `HostOperationsSnapshot` (the backfill) |
| `stage_slice_revision` | `{ operationId: string, printerId: string, sliceRevisionId: string }` | `HostOperation` (`dispatching`, or the replayed row) |
| `start_staged_artifact` | `{ operationId: string, printerId: string, hostOperationId: string, priorState: PriorState }` | `HostOperation` |
| `pause_host_print` | `{ operationId: string, printerId: string }` | `HostOperation` |
| `resume_host_print` | `{ operationId: string, printerId: string }` | `HostOperation` |
| `cancel_host_print` | `{ operationId: string, printerId: string }` | `HostOperation` |
| `reconcile_host_operation` | `{ hostOperationId: string }` | `HostOperation` after the attempt. A row that is not `uncertain` is returned unchanged. Gated on the capability the row's kind needs (D5 "Capability gate"): `CAPABILITY_UNSUPPORTED` with no attempt. |
| `abandon_host_operation` | `{ operationId: string, hostOperationId: string, acknowledgement: "hostStateUnknown", note?: string }` | `HostOperation` (`abandoned`) |

The write commands return right after the write-ahead commit. Outcomes
arrive as events. Ledger digests (a struct, fields in this order):
stage `{ printerId, sliceRevisionId }`; start `{ printerId,
hostOperationId, priorState }`; pause/resume/cancel `{ printerId }`;
abandon `{ hostOperationId, acknowledgement, note }`.

**`list_host_operations` snapshot contents**, per Printer (or for the one
Printer given): every unresolved row, the newest `succeeded` upload per
`host_path` (the staged artifacts), and the newest 20 other terminal rows,
ordered by `created_at` descending. Rows of a deleted Printer are gone
with it. There is no `removed` event; the frontend drops rows whose
`printerId` no longer exists.

### Events

The **`hostOperations` stream** is its own sequence on `farm3d-event-v1`,
in the usual `EventEnvelope`, following `slicing/events.rs`. Events go out
after commit only, never inside a transaction and never for a replay.

| Type | Subject | Payload |
|---|---|---|
| `hostOperations.operation.changed` | `{ kind: "hostOperation", id: <hop-id> }` | `HostOperation` |

It is emitted for every committed change to a row: the write-ahead
insert, `mark_sent`, every transition, every recorded attempt, and
`no_longer_pending` becoming true. Listeners filter on the
`hostOperations.` prefix; the Printer, Library, and slicing listeners
ignore it. Capability changes have no event: the frontend refetches
`printer_capabilities` when the Printer's status changes.

### Error codes

`ErrorCode` gains the following. `details` values are camelCase JSON.
None ever carries a credential, a raw host body, or a full host URL.

| Code (wire) | Raised by | `details` | `recovery` | `retryable` |
|---|---|---|---|---|
| `CAPABILITY_UNSUPPORTED` | every write command; `reconcile_host_operation` when the row's kind's reconcile capability is unsupported | `{ printerId, capability: CapabilityKey, reason: "adapter" \| "notVerified" \| "host", detail }` | `[]` | false |
| `HOST_OPERATION_PENDING` | a write command while the Printer has an unresolved row; `import_printers` | `{ printerIds: string[], hostOperationIds: string[] }` | `[OPEN_PRINTER_JOB]` | false |
| `HOST_OPERATION_NOT_ABANDONABLE` | `abandon_host_operation` | `{ hostOperationId, state: HostOperationState, attempts: number }` | `[RELOAD]` | false |
| `START_NOT_ALLOWED` | `start_staged_artifact` | `{ printerId, observedState: OperationalState, freshness: TelemetryFreshness }` | `[RELOAD]` | false |
| `START_PRECONDITION_CHANGED` | `start_staged_artifact` | `{ printerId, observedState: OperationalState, freshness: TelemetryFreshness, priorState: PriorState }` | `[RELOAD]` | false |
| `CONTROL_NOT_ALLOWED` | pause/resume/cancel | `{ printerId, verb: "pause" \| "resume" \| "cancel", observedState: OperationalState, freshness: TelemetryFreshness }` | `[RELOAD]` | false |
| `STAGED_ARTIFACT_INVALID` | `start_staged_artifact` | `{ hostOperationId, reason: "absent" \| "differs" }` | `[]` (**Stage again** is a local action of the Start dialog, not a `RecoveryCode`) | false |
| `CONNECTION_IN_USE` | `set_printer_connection`, `clear_printer_connection` | `{ printerId, hostOperationId }` | `[OPEN_PRINTER_JOB]` | false |

For a host re-read rejection (D9 step 7 and the control rule), the
`observedState` is the host's print state mapped to `OperationalState`
(`printing` → `printing`, `paused` → `paused`, Klipper not ready →
`error`), and `freshness` is `fresh`.

`RecoveryCode` gains `OpenPrinterJob` (`OPEN_PRINTER_JOB`): open the
Printer's Job tab.

`RepositoryError` gains `ConnectionInUse { printer_id, host_operation_id }`
and `HostOperationsPending { printer_ids, host_operation_ids }`, mapped in
`CommandError::from_repository`.

Existing codes reused, unchanged: `NOT_FOUND`, `VALIDATION`,
`LIFECYCLE_BLOCKED`, `PRINTER_UNREACHABLE`, `TIMEOUT`,
`AUTHENTICATION_FAILED`, `PROTOCOL_ERROR`, and the operation-id-reuse
error.

Messages:

| Code | Message |
|---|---|
| `CAPABILITY_UNSUPPORTED` | the capability's `detail` |
| `HOST_OPERATION_PENDING` | "This printer has a pending operation. Finish or abandon it first." |
| `HOST_OPERATION_NOT_ABANDONABLE` | "farm3d can only stop checking an uncertain operation after it has checked at least once." |
| `START_NOT_ALLOWED` | "The printer can't start a print now: <state label>." |
| `START_PRECONDITION_CHANGED` | "The printer's state changed. Confirm the bed again." |
| `CONTROL_NOT_ALLOWED` | "The printer isn't in a state to <verb> now: <state label>." |
| `STAGED_ARTIFACT_INVALID` | absent: "The staged file is no longer on the printer. Stage it again." differs: "The file on the printer no longer matches this Slice Revision. Stage it again." |
| `CONNECTION_IN_USE` | "Finish or abandon the pending printer operation before changing this Connection." |

## Frontend architecture

### State

- **`src/host-ops/capabilities-store.ts`**: `PrinterCapabilities` per
  Printer from `printer_capabilities`, refetched when that Printer's status
  changes. After an Online transition the backend re-reads the host facts
  in the background, so the store also refetches on a short, bounded
  backoff (about 15 s) until a fetch carries facts observed since that
  transition. `AdapterCapabilityRow[]` loaded once.
- **`src/host-ops/host-operations-store.ts`**: the only owner of Host
  Operations. Listen-before-backfill over `hostOperations.*` through
  `src/ipc/sequenced-stream.ts`, following `src/slicing/slicing-store.ts`.
  It drops rows whose Printer no longer exists. Derived views:
  `unresolvedFor(printerId)`, `stagedFor(printerId)` (succeeded uploads,
  newest per `hostPath`), `recentFor(printerId)`.
- **`src/host-ops/host-operations-store-mock.ts`** and **`web-fixtures.ts`**
  for `just web`: a Ready single-tool Moonraker Printer, a four-tool
  Moonraker Printer, an OctoPrint Printer (every capability
  `notVerified`), a `Finished` Printer with a staged artifact, a `Failed`
  Printer, and a Printer with an `uncertain` upload (`attempts: 1`).
- **`src/host-ops/start-rule.ts`**, pure, mirroring D9:
  - `startOffer(status, hasUnresolved) -> { offered: false, reason } |
    { offered: true, priorState, confirmLabel }`, with the three labels
    and "Clear the error on the printer first." for `Failed`;
  - `controlOffer(status, verb, hasUnresolved) -> { offered: boolean,
    reason? }`: pause only while `printing`, resume only while `paused`,
    cancel while either, all with fresh telemetry. With an unresolved row
    on the Printer it is not offered, with the reason "A printer operation
    is pending. You can still pause or cancel on the printer itself."
  - `startOffer` is likewise not offered when `hasUnresolved` is true,
    with the reason "A printer operation is pending."
- **`src/host-ops/presentation.ts`**, pure: the labels below, capability
  labels, failure and inconclusive copy (D11), and severities.

No store type has a credential field.

**Host Operation labels** (severity in brackets):

| Kind | `dispatching` | `uncertain` | `reconciling` | `succeeded` | `failed` | `abandoned` |
|---|---|---|---|---|---|---|
| upload | Uploading [info] | Upload uncertain [warning] | Checking [info] | Staged [success] | Upload failed [error] | Abandoned [warning] |
| start | Starting [info] | Start uncertain [warning] | Checking [info] | Started (or "Started, then interrupted" when `interrupted`) [success] | Start failed [error] | Abandoned [warning] |
| pause / resume / cancel | Pausing / Resuming / Cancelling [info] | Pause / Resume / Cancel uncertain [warning] | Checking [info] | Paused / Resumed / Cancelled [success] | Pause / Resume / Cancel failed [error] | Abandoned [warning] |

**Capability labels:** "Supported"; `adapter` → "Not supported by
<adapter>" (or "No Connection"); `notVerified` → "Not verified yet";
`host` → "Not available on this printer" plus the detail. Unsupported and
failed never share copy or severity: unsupported is neutral, failed is
error.

### Components

- **`PrinterJobPanel.tsx`**, a **Job** tab in `PrinterDetailDock.tsx`
  (after Status, Setup):
  - the host's current print from telemetry: file, state, progress, every
    tool's temperature and target from `telemetry.tools` (falling back to
    `nozzle_*` when `tools` is empty), and the bed;
  - **Pause**, **Resume**, **Cancel print…**: rendered only when the
    capability is supported, enabled only when `controlOffer` says so,
    with the reason as visible text otherwise. Cancel confirms in a Kobalte
    `AlertDialog`;
  - **Staged on this Printer**: staged artifacts with **Start…**, and
    unresolved and recent rows with their state, the inconclusive reason,
    **Check again** (enabled in `uncertain` when the reconcile capability
    is supported), and **Abandon check…** (enabled in `uncertain` with
    `attempts >= 1`, or when the reconcile capability is unsupported).
- **`StartStagedDialog.tsx`** (Kobalte `Dialog`): the checkbox label is
  `startOffer(status, hasUnresolved).confirmLabel`; Confirm is disabled until it is
  ticked; it sends `priorState`. While the command runs it shows
  "Checking the file on the printer…" (D9 steps 7–9 take seconds). On
  `START_PRECONDITION_CHANGED` or `START_NOT_ALLOWED` it clears the tick
  and re-renders for the new status; a tick is never reused. On
  `STAGED_ARTIFACT_INVALID` it shows the message with **Stage again**. This
  holds for both Start-safety rule values.
- **`StageOnPrinterDialog.tsx`** (Kobalte `Dialog` and `Select`), from
  **Stage on Printer…** in `SliceRevisionReview.tsx`: Printers whose
  `upload` is unsupported are listed but disabled with the capability
  reason; Offline Printers are disabled with "Offline"; a Printer with an
  unresolved row is disabled with "A printer operation is pending." Each
  reason is distinct copy. **Add to Queue…** stays disabled.
- **`AbandonReconciliationDialog.tsx`** (Kobalte `AlertDialog`): names the
  Printer, the operation, and the file; states that farm3d will stop
  checking and the printer's state stays unknown; includes the
  `noLongerPending` sentence when set ("Klipper restarted after farm3d
  sent this, so it is no longer waiting to run."). Required checkbox: "I
  understand the printer may still have this file or be printing it."
  Optional note (≤ 500 characters).
- **`CapabilityList.tsx`**, read-only, in the Setup tab: each capability's
  label, detail, and evidence tier ("Simulator" / "Read-only hardware").
- **`PrinterConnectionPanel.tsx`**: `CONNECTION_IN_USE` shows its message
  with a link that opens the Job tab (`OPEN_PRINTER_JOB`).

## Errors and recovery

| Situation | Behavior |
|---|---|
| Capability unsupported | The control is never an enabled button; the reason is visible. The command returns `CAPABILITY_UNSUPPORTED` with no row. |
| Upload response lost after the host stored it | `uncertain` (`responseLost`); reconcile → `Matches` → Staged. Exactly one upload was sent. |
| Upload cut mid-body | `uncertain`; absent until the settle period ends; then Upload failed (`notApplied`). Staging again is the operator's choice. |
| A different file at farm3d's path | `uncertain` (`uploadSettling`) during the settle period, then Upload failed (`hostFileDiffers`). Nothing is deleted. |
| Start response lost | `uncertain`; reconcile finds `printing` with our file or a qualifying history job → Started. Otherwise it stays uncertain. farm3d never sends a second start. |
| Start rejected by the host | Start failed with the code's text, no reconcile. |
| Host unreachable throughout | Stays uncertain; retried with backoff while Online; **Abandon check…** after one attempt; guards lift after abandon. |
| farm3d closed mid-operation | Unsent → Failed (`neverSent`). Sent → uncertain (`interruptedByRestart`), then reconciled. |
| Printer state changed before Start | `START_PRECONDITION_CHANGED` or `START_NOT_ALLOWED`; the dialog clears its tick. |
| Staged file gone or changed | `STAGED_ARTIFACT_INVALID`; **Stage again**. |
| Connection edit or Archive while pending | `CONNECTION_IN_USE` or `LIFECYCLE_BLOCKED`, with a link to the Job tab. |
| Host Operations stream uncertain | Content is kept, marked stale, and backfilled with backoff (P4/P5 behavior). |

Every failure renders inline as `role="alert"` with its recovery action.

## Accessibility and adaptation

- Every action is keyboard-operable; the dialogs trap focus and return it
  to their trigger.
- The Start and Abandon confirmations are real checkboxes with visible
  labels; Confirm stays disabled until ticked, and its disabled reason is
  visible text.
- Host Operation state changes are announced through one `aria-live=
  "polite"` region in the Job tab. Failures use `role="alert"`.
- State and capability always use icon, text, and colour together.
- Layout works at 1440 × 900 and 1024 × 700. Tokens, CSS Modules, and
  Kobalte only, in the dense editor aesthetic.

## Acceptance criteria

1. **Migration.** 0007 applies to a P5 database and keeps every
   `operations` row; the partial unique index rejects a second unresolved
   row; the trigger rejects any update to a terminal row; no column name
   contains `credential`, `secret`, or `key`.
2. **State machine.** Every legal D3 transition persists; every illegal
   pair is rejected before SQL. `recover_after_restart` produces
   `neverSent`, `interruptedByRestart`, and returns `reconciling` rows
   with `uncertain_since` unchanged. A failing `mark_sent` sends nothing.
3. **Capabilities.** One `capabilities_for` test per D6 rule; host facts
   for one extruder and for four reported out of order; a real-host-shaped
   object list with `extruder_offset_calibration` gives four tools; the
   registry consistency test fails for a `supported` capability without a
   builder or with no evidence.
4. **Adapter.** Every D5 dispatch-table row is produced by `FakeMoonraker`
   and classified as stated; the upload form never has a `print` field;
   `locate` returns `Absent`, `Matches`, `Differs { Size }`, and
   `Differs { Hash }`, and a download failure is an error, not `Absent`.
5. **Reconciliation.** Each D5 reconciliation row is covered, including:
   `complete` with our filename and no qualifying job stays uncertain; a
   job above the mark but older than `dispatched_at − 30 s` does not
   count; a job with an id at or below the mark does not count; an
   interrupted job gives `startObserved { interrupted: true }`; an upload
   absent before the settle period stays uncertain and after it fails.
6. **No duplicate writes.** No code path issues a second upload or start
   by itself; the reconciler issues no write (the fake records every
   request).
7. **Start and control rules.** One test per D9 table row, in Rust and in
   `start-rule.ts`, with the same results; no row is written on any
   pre-check rejection; Start is enabled again after `Failed` clears to
   `Ready`.
8. **Guards.** Every D7 row is blocked while unresolved and allowed after
   `succeeded`, `failed`, and `abandoned`; a same-endpoint credential
   replacement succeeds; permanent delete removes only that Printer's
   terminal rows; a concurrent insert-and-delete race leaves no orphan.
9. **Abandon.** Refused outside `uncertain`, and before any attempt
   unless the row is structurally unreconcilable (D5 "Capability gate");
   allowed after one inconclusive attempt; the row is kept.
10. **Secrets.** A seeded API key never appears in any event, error, row,
    `Debug` output, or log line.
11. **Events.** Emitted after commit only; listen-before-backfill holds;
    replays emit nothing.
12. **Simulator evidence** (`just test-sim`, manifest cited): stage with
    the response cut, restart, reconcile to Staged with one file; a
    mid-body cut stays uncertain until the settle period and then fails;
    a start with the response cut reconciles to Started; pause, resume,
    cancel; a second start from Finished with `priorState: finished`; a
    Klipper restart mid-print; host unreachable then abandon; capability
    detection on `moonraker`, `moonraker-multi`, `no-bed`, and `apikey`.
13. **Read-only tier** (`just p6-readonly`, optional, owner-run): zero
    recorded writes; a never-sent path is `Absent`; a seeded `uncertain`
    upload row whose `uncertain_since` is more than 60 s old reconciles to
    `failed { notApplied }`.
14. **Tracer** on `FakeMoonraker` (CI) and the simulator (evidence), per
    the plan's Task 13.
15. **UI.** The Task 11 tests pass; screenshots at 1440 × 900 and
    1024 × 700 per Task 14.

## Delivery

Tasks 5–14 of the plan, in its delivery order:

| Task | Delivers from this spec |
|---|---|
| 5 | D1 traits and types, D6 matrix, host-fact derivation, `printer_capabilities`, `adapter_capability_matrix` |
| 6 | D2 schema, D3 state machine, repository |
| 7 | D7 guards, `CONNECTION_IN_USE`, `HOST_OPERATION_PENDING` for import |
| 8 | D4, D10, the dispatch classification in D5, `FakeMoonraker` |
| 9 | D5 executor and reconciler, D8, D9, events, the write commands, the remaining error codes |
| 10 | Frontend stores, `start-rule.ts`, `presentation.ts` |
| 11 | Frontend components |
| 12 | Simulator evidence, evidence rows, the read-only suite |
| 13 | Tracer |
| 14 | Verification record and screenshots |

## Decisions made in this spec

Questions the spike, the owner answers, and the plan left open. Each
takes the fail-safe option.

1. **The capability adapter uses HTTP only.** Every call it needs has an
   HTTP endpoint that honours `X-Api-Key` (Gate A). This avoids the
   WebSocket's "upgrade succeeds, every call then fails" auth shape
   (spike 10) and gives one client with one set of timeouts. HTTP loses
   multi-line Klipper error text ("Unknown", spike 3), but classification
   needs only the status and the single-line 400/503 messages, and raw
   messages are never shown.
2. **A 201 never proves an upload by itself.** The executor always
   verifies by download-and-hash, so the proof does not depend on whether
   the host checked `checksum`, on any Moonraker version.
3. **No Moonraker-restart shortcut for upload absence.** Moonraker exposes
   no reliable process start time, and a missed restart would wrongly
   prove "not applied". Only the settle period proves absence. The cost is
   up to 60 s before an interrupted upload fails.
4. **A differing file is treated like an absent one** (settle period, then
   `hostFileDiffers`), because a late body can still replace it.
5. **Start requires the `history` component** (`host` unsupported
   otherwise), and a start whose history mark cannot be read is refused
   with no row. Without the mark, rule (b) would rest on the time rule
   alone, which a quick earlier print of the same file can satisfy.
6. **Start re-verifies the staged bytes and re-reads the host** before the
   write-ahead (D9 steps 7–10). Starting a different file than the
   operator confirmed, or starting from Paused, is a physical hazard;
   the cost is one download per start.
7. **Pause, resume, and cancel have a pre-check** (`CONTROL_NOT_ALLOWED`),
   on live status and on a host re-read, because pause from an idle
   printer sets `is_paused` (spike 9). Their rows record the file being
   printed, so reconciliation matches the effect on that file only.
8. **Resume is proved by `printing` or `complete`** on the same file: a
   paused print cannot complete without being resumed.
9. **Two commits around the send** (`created_at`, then `dispatched_at`
   before connecting). An unsent row fails as `neverSent` at startup
   instead of becoming an uncertain start that can never be proved.
10. **A connect failure is definitive** (`hostUnreachable`): no request
    byte was sent.
11. **Any status or body not in the D5 table is indeterminate.**
12. **Reconciliation uses the row's recorded endpoint**, with the current
    credential (answer 4 allows replacing it).
13. **New error codes** `HOST_OPERATION_PENDING`, `CONTROL_NOT_ALLOWED`,
    and `STAGED_ARTIFACT_INVALID`, and the recovery code `OPEN_PRINTER_JOB`.
    The plan named none for these cases.
14. **Evidence is per capability** (D6), because camera and the writes
    have different tiers.
15. **Write commands return after the write-ahead commit**; outcomes
    arrive as events. An upload can take minutes.
16. **`no_longer_pending` is informational only** (spike 4). It never
    resolves a row.
17. **Timeouts**: connect 5 s, query 10 s, control 60 s, transfer
    60 s + 1 s/MiB, verification window 10 s. Skew tolerance 30 s, settle
    period 60 s (spike 5, 7).
18. **Camera URLs are never stored or sent to the frontend** (Gate H:
    they can embed the printer's LAN address). P6 needs only the count.
19. **A structurally unreconcilable row can be abandoned without an
    attempt** (D8). If the capability its kind needs to reconcile is
    unsupported, no attempt can ever run, and requiring one would trap the
    Printer behind the guards.
20. **The history high-water mark is the maximum parsed `job_id` over a
    50-job page**, and the reconcile query re-checks every rule itself, so
    neither depends on Moonraker honouring `order` or `since`.
21. **The executor sends nothing unless `mark_sent` succeeded**, and every
    local precondition (including `open_verified`) runs before it (D3).
    Otherwise a `neverSent` row could have been sent.

## Residual risks

- The 60 s settle period is a margin on one simulator's evidence, not a
  bound. A host behind a slower buffering hop could deliver an upload
  later and turn a `notApplied` into a file that is present after all.
  That file is harmless (it is never started without re-verification),
  and staging again overwrites it.
- A start that ran and finished inside one Moonraker status batch leaves
  no evidence (Gate F case 1). It stays uncertain until abandoned.
- Answer 2 (one unresolved write per Printer) means an uncertain start
  blocks a farm3d cancel until it is resolved or abandoned. The operator
  can still cancel on the printer, and the Job tab says so.
- Under answer 2, staging a file while the Printer is printing blocks
  farm3d's own Pause and Cancel for the whole upload, and indefinitely if
  the upload becomes uncertain, until it resolves or is abandoned. The
  operator must pause or cancel on the printer itself, and the Job tab
  says so. This records the consequence; it does not reopen answer 2.
- Snapmaker's fork (API 1.4.0) was only read, never written (answer 8).
  Its write behaviour (checksum, 422, 403, queueing) is assumed to match
  the simulator. The pre-start verification and the fail-safe rules limit
  the damage if it does not.
