# Complete v1 Implementation Approach

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:writing-plans` to create each focused phase plan, then
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to execute it. This is an umbrella approach,
> not a task-by-task plan, and must not be executed directly.

**Goal:** Deliver the complete local-desktop farm3d v1 through independently
reviewable vertical phases with explicit frontend, backend, and cross-tier
wiring ownership.

**Architecture:** Product domains remain separate deep modules behind small
interfaces. Rust owns persisted truth, transactional invariants, protocol
behavior, and restart reconciliation. SolidJS owns presentation, interaction,
ephemeral view state, and accessible input. A named wiring owner for each phase
owns the Tauri command/event contract and proves the vertical path end to end.

**Tech Stack:** Tauri 2, Rust, Tokio, SolidJS, TypeScript, Kobalte, CSS Modules,
Vitest, Rust unit/integration tests, and OrcaSlicer as a separate runtime process
where the slicing phase proves its command contract.

**Spec:** `docs/superpowers/specs/2026-09-16-complete-v1-ui-workflows-design.md`

## Document purpose

This document answers four questions for the v1 program:

1. In what order should the work be delivered?
2. Which role owns each phase and each technical layer?
3. Which existing modules are changed, and which new seams must be designed?
4. Which unknowns block implementation and must be resolved without guessing?

It does not define final data schemas, Rust traits, Tauri payloads, third-party
libraries, or protocol behavior where the repository has no evidence. Those
belong in the focused spec for the phase that owns the decision.

## Global constraints

- No delivery phase starts implementation without its own approved focused spec
  and task-level implementation plan.
- Existing `CONTEXT.md`, ADR, catalog-integrity, credential-secrecy, and
  Connection decisions remain binding unless an approved document explicitly
  supersedes them.
- Production component CSS uses `--f3d-*` tokens; shared interactive behavior
  uses Kobalte primitives where one exists.
- Rust-related commands in non-interactive shells source `$HOME/.cargo/env`.
- Frontend-affecting work is not complete until `just build` and `just test`
  pass; backend-affecting work also requires `just test-rust`.
- Live protocol, camera, notification, filesystem, process, and packaging claims
  require evidence from the relevant real system or installed bundle.
- Credentials never enter general persistence, frontend state, events, backups,
  diagnostics, or logs.

> [!IMPORTANT]
> **Role ownership is explicit; individual assignment is not.** The repository
> contains no staffing or CODEOWNERS map. “Frontend owner,” “Backend owner,”
> “Wiring owner,” and “Protocol owner” below are accountable roles. Before a
> phase starts, the delivery lead must assign one named person or agent session
> to each required role. A role may share the same person, but accountability
> may not be left as “the team.”

> [!CAUTION]
> **This approach does not authorize implementation from the umbrella spec.**
> Each phase has decision gates because core facts remain unknown: persistence
> technology, multi-material topology, file parsers, OrcaSlicer runtime behavior,
> adapter write capabilities, camera protocols, and platform notifications.

## Current baseline

The approach is based on the source at `e925d86` plus the approved v1 design and
domain updates in the working tree.

Implemented today:

- Tauri/SolidJS shell with Printers and Library destinations.
- Custom tokenized design system and Kobalte-backed controls.
- Bundled OrcaSlicer-derived Printer catalog and Profile resolution.
- Independent persisted Printer records and Profile overrides.
- Settings file containing theme mode.
- Credential storage, mDNS discovery, Connection configuration, and probing.
- Moonraker status subscription, reconnection supervision, and live telemetry.
- One Tauri event, `printer-status`, with command-based status backfill.

Not implemented today:

- OctoPrint or ElegooLink adapters.
- Transactional cross-domain persistence or schema migrations.
- Archive, batch Printer creation, or unattended-start policy.
- Spools or Material Slots.
- Library persistence, Projects, source revisions, or format parsing.
- Runtime slicing or Slice Revisions.
- Adapter upload/start/pause/resume/cancel/camera capabilities.
- Queue Entries, Jobs, assignment, reservations, or dispatch.
- Attention Events, Incidents, snapshots, desktop notifications, or deep links.
- Complete history, backup/restore, diagnostics, or reset.

> [!WARNING]
> `PrinterStatus.jobState` and `jobName` are raw host telemetry. They are not
> farm3d Queue Entries or Jobs and must never become their persistence source.

## Ownership model

### Product and design owner

Accountable for:

- Confirming a focused phase spec conforms to the umbrella product spec.
- Resolving workflow and copy decisions.
- Reviewing required states at 1440 × 900 and 1024 × 700.
- Approving intentional deviations before implementation.

Not accountable for Rust invariants, protocol truth, or cross-tier type parity.

### Frontend owner

Accountable for:

- SolidJS screen and store modules.
- Ephemeral selection, panel, filter, and form state.
- Accessible keyboard/pointer interaction.
- CSS Modules using existing design tokens.
- Kobalte-backed behavior and design-system extensions.
- Frontend unit/component tests and realistic `just web` fixtures.

The frontend owner does not implement authoritative scheduling, eligibility,
reservation, migration, protocol, or reconciliation logic in TypeScript.

### Backend owner

Accountable for:

- Rust domain modules and invariants.
- Durable persistence, migrations, transactions, and crash recovery.
- Background task supervision and restart reconciliation.
- File/process/media ownership.
- Tauri command implementations after the wiring contract is agreed.
- Rust unit/integration tests.

The backend owner does not encode product navigation or duplicate presentation
formatting needed only by SolidJS.

### Wiring owner

Accountable for the seam between SolidJS and Rust:

- Command and event names, payloads, errors, and ordering constraints.
- Rust/TypeScript type parity.
- Registration in `src-tauri/src/lib.rs` and frontend subscription/invocation.
- Listener-before-backfill sequencing for open-ended events.
- Deep-link destination and selected-object semantics.
- Cross-domain orchestration where no single screen or Rust module can prove the
  workflow alone.
- Integration tests and the phase tracer bullet.

The wiring owner does not create a second copy of domain policy in a frontend
store or Tauri command wrapper.

### Protocol owner

Required only for adapter, camera, and OrcaSlicer process work. Accountable for:

- Primary-source protocol research and real-system evidence.
- Pure wire framing/parsing tests.
- Capability and failure-mode documentation.
- Identifying unsupported behavior explicitly.

The protocol owner cannot infer ElegooLink behavior from Moonraker or OctoPrint.

### Verification owner

Accountable for independently checking:

- Focused spec coverage.
- `just build`, `just test`, and `just test-rust`.
- Required desktop viewport states.
- Live-system checks where the phase requires hardware or OrcaSlicer.
- Migration, restart, partial failure, and secret-redaction evidence.

This role may be the wiring owner for a small phase, but must perform a fresh
verification pass rather than rely on implementer claims.

## Cross-layer rules

1. **Rust is persisted truth.** Frontend stores may optimistically present an
   operation, but must settle from a command result or authoritative event.
2. **One domain, one owning module.** `printer-store.ts` must not become a
   universal Farm store, and `printers.rs` must not absorb every new entity.
3. **Commands change bounded state; events report open-ended change.** A command
   returns the resulting authoritative state needed by its caller. Long-running
   status and background work use events plus a command backfill.
4. **The wiring interface includes behavior.** Payload types alone are
   insufficient; focused specs must define ordering, retries, idempotency,
   stale data, errors, and restart behavior.
5. **Adapter modules translate protocols.** They do not own queue order,
   scheduling, material accounting, or product-level Job transitions.
6. **Frontend display state is not domain state.** Open panels, active filters,
   focus, and unsaved form edits stay in SolidJS; Jobs, reservations, archive,
   and reconciliation live in Rust.
7. **Web fallback behavior is explicit per phase.** It may use deterministic
   in-memory fixtures, but must not silently pretend unsupported Tauri behavior
   works.
8. **Every phase lands vertically.** A phase is incomplete if frontend and
   backend pieces exist but the real Tauri path, startup behavior, or recovery
   path is unwired.
9. **Every persisted change has migration and rollback treatment.** “Add a
   field” is not complete until old data and interrupted writes are handled.
10. **Secrets stay behind the credential seam.** Backups, diagnostics, errors,
    events, and frontend state never contain credential values.

## Dependency graph

```text
F0 Baseline verification
│
├── A0 Adapter-readiness lane
│   ├── A0.1 Moonraker live validation
│   ├── A0.2 OctoPrint monitoring implementation + live validation
│   └── A0.3 ElegooLink real-hardware protocol spike
│
└── F1 Persistence and cross-tier contract foundation
    ├── P1 Shell, Monitor, and status vocabulary
    │   └── P2 Printer lifecycle and batch setup
    │       └── P3 Spools and Material Slots ──────────────┐
    │                                                      │
    └── P4 Library persistence and Projects                │
        └── P5 Runtime slicing and Slice Revisions ────────┤
                                                           │
P5 + one adapter-specific A0 evidence path ──> P6 command capability
                                                           │
P1 + P2 + P3 + P5 + one complete P6 adapter ────────────> P7 Queue Entries,
                                                               Jobs, dispatch
P1 + P6 + P7 ───────────────────────────────────────────> P8 Attention,
                                                               Incidents,
                                                               cameras,
                                                               notifications
Stable P2–P8 persisted schemas ─────────────────────────> P9 History,
                                                               backup/restore,
                                                               diagnostics
```

Hard dependencies and safe parallelism:

- P1 implementation waits for F1 command/event and preference-persistence
  contracts. Visual prototypes that write no production state may happen
  earlier, but cannot define those contracts.
- P2 follows P1 because it extends the shell, reusable dock, status vocabulary,
  and Printer detail established there.
- A0 runs in parallel with F1 and P1–P5.
- P4 may run after F1 in parallel with P1–P3. It does not depend on Printer
  lifecycle, but its release checkpoint follows P2 to keep delivery coherent.
- P3 follows P2 because Material Slots attach to stable Printer identity and
  archive semantics.
- P5 frontend viewport work may overlap P6 protocol research after artifact
  identity is agreed.
- P6 is adapter-specific. Moonraker command work may proceed after A0.1 and P5;
  unavailable OctoPrint or ElegooLink evidence does not block Moonraker.
- P7 is the convergence point. It may not begin from guessed P3, P5, or P6
  contracts.

## Phase F0: Verify and freeze the baseline

**Primary owner:** Wiring owner.

**Supporting owners:** Frontend, Backend, Protocol, Verification.

**Purpose:** Record the actual behavior that migrations and redesigns must
preserve. Unchecked historical plan boxes are not evidence.

**Frontend responsibility:**

- Inventory current shell, Printer dashboard/detail, settings, Library scaffold,
  stores, and `just web` behavior.
- Record current event-listener ordering and error presentation.
- Identify current component tests that pin behavior.

**Backend responsibility:**

- Record current `settings.json`, `printers.json`, credential, catalog, and
  status behavior.
- Verify quarantine, Profile drift, variant rebind, supervisor restart, and
  credential cleanup behavior.
- Capture command/event registration from `src-tauri/src/lib.rs`.

**Wiring responsibility:**

- Run the complete automated gate.
- Trace one existing Printer from create through Connection save, restart, and
  status backfill.
- Produce a short baseline report referenced by later migration specs.
- Produce a supported-platform matrix that distinguishes the current development
  host from platforms the project intends to package and release. No platform is
  called supported solely because `tauri.conf.json` uses a broad bundle target.

**Likely files inspected, not necessarily changed:**

- `src/App.tsx`
- `src/printers/printer-store.ts`
- `src/screens/PrinterDashboard.tsx`
- `src/screens/PrinterAddDialog.tsx`
- `src/screens/PrinterConnectionPanel.tsx`
- `src-tauri/src/lib.rs`
- `src-tauri/src/printers.rs`
- `src-tauri/src/settings.rs`
- `src-tauri/src/connections/`

> [!CAUTION]
> **Blocking gate:**
> A real Moonraker verification needs reachable hardware or a representative
> instance. If none is available, record the absence; do not relabel the
> documentation-derived protocol tests as live verification.

**Exit gate:** Baseline report, passing automated suite, known persisted-file
fixtures, supported-platform matrix with an evidence requirement per platform,
and explicit live-hardware evidence or absence.

## Phase A0: Adapter-readiness lane

This lane is independent research/delivery work that feeds P6. It does not wait
for UI phases.

### A0.1 Moonraker live validation

**Primary owner:** Protocol owner.

**Backend responsibility:** Exercise current probe/subscription against a real
instance and record missing fields, lifecycle notifications, authentication,
and reconnect behavior.

**Frontend responsibility:** Confirm the current UI handles real missing/partial
readings and does not render absent temperatures as zero.

**Wiring responsibility:** Verify `printer-status` event and `printer_statuses`
backfill across restart and listener timing.

**Exit gate:** Reproducible live-validation notes containing hardware/software
versions and results for authentication, missing objects, partial updates,
Klipper ready/shutdown transitions, disconnect, reconnect, listener backfill,
and app restart. Any mismatch with current behavior is either corrected and
covered by a regression test or recorded as a blocking issue for P1/P6.

### A0.2 OctoPrint monitoring adapter

**Primary owner:** Protocol/Backend owner.

**Existing plan:**
`docs/superpowers/plans/2026-08-21-octoprint-adapter.md`

**Frontend responsibility:** Add OctoPrint only after the backend can construct,
probe, subscribe/poll, and report it. Preserve the same status presentation.

**Backend responsibility:** Implement the existing status-only plan, including
percentage conversion, 409 handling, polling, registration, and tests.

**Wiring responsibility:** Verify setup, credential behavior, discovery,
restart, and status display against a real OctoPrint instance.

> [!IMPORTANT]
> This plan does not provide upload, start, pause, resume, cancel, artifact
> identity, or camera capabilities. Those remain P6 work.

**Exit gate:** Existing OctoPrint plan complete plus live-instance evidence.

### A0.3 ElegooLink protocol spike

**Primary owner:** Protocol owner.

**Frontend responsibility:** None beyond supplying the user-visible facts the
spike must seek. Do not add a selectable adapter.

**Backend responsibility:** Capture real identity, telemetry, authentication,
transport lifecycle, and any observed command/media behavior from supported
hardware.

**Wiring responsibility:** Compare observed fields with farm3d status and
capability needs; document required interface changes.

> [!CAUTION]
> **Blocking gate:**
> No ElegooLink implementation plan may be written without real-hardware
> evidence. Moonraker compatibility behind an Elegoo surface must be observed,
> not inferred from G-code flavor or third-party descriptions.

**Exit gate:** Protocol capture, evidence-backed focused spec, and an explicit
go/no-go decision for v1.

## Phase F1: Persistence and cross-tier contract foundation

**Primary owner:** Backend owner.

**Supporting owners:** Wiring and Frontend.

**Purpose:** Establish the storage and interface guarantees required before
cross-record domains are introduced.

**Frontend responsibility:**

- Document existing direct `invoke()` and listener patterns.
- Participate in the decision on manually mirrored versus generated cross-tier
  types; do not select a generator without a focused evaluation.
- Define domain-store boundaries so later stores remain independent.

**Backend responsibility:**

- Choose between a transactional database and an explicitly journaled,
  atomic/versioned document store.
- Define schema versioning, migrations, stable IDs, transaction boundaries,
  write serialization, backup-safe snapshots, and crash recovery.
- Define storage roots for metadata and large immutable content without yet
  inventing every domain schema.
- Preserve the separate credential store.
- Plan migration of existing `printers.json` and `settings.json` without data
  loss.

**Wiring responsibility:**

- Define command success/error conventions and machine-readable recovery codes.
- Define event envelope, listener-before-backfill convention, and event version
  compatibility.
- Define navigation/deep-link identity for destination plus selected object.
- Prove one migrated Printer read/write and one event/backfill path end to end.

**Current files affected:**

- `src-tauri/src/printers.rs`
- `src-tauri/src/settings.rs`
- `src-tauri/src/lib.rs`
- `src/printers/printer-store.ts`
- `src/settings/settings-store.ts`

**Proposed seam, subject to the focused spec:**

- `src-tauri/src/persistence/`

> [!CAUTION]
> The proposed path is not an approved library or schema. The focused F1 spec
> must compare options using required invariants: atomic slot movement, atomic
> assignment/reservation, immutable history, restore, and interrupted writes.

> [!IMPORTANT]
> Do not migrate everything merely to establish the seam. The tracer bullet is
> one existing Printer and one durable event contract. New domain schemas remain
> owned by their phases.

**Exit gate:** Approved persistence ADR/spec, migration fixture tests, concurrent
write test, interrupted-write recovery test, and one vertically wired proof.

## Phase P1: Shell, Monitor, and operational status

**Primary owner:** Frontend owner.

**Supporting owners:** Backend and Wiring.

**Tracer bullet:** At 1440 × 900 and 1024 × 700, Monitor loads persisted
Printers immediately, marks live fields stale until refreshed, supports
filter/search/density/Monitor Sections, and opens one reusable detail dock.

**Frontend responsibility:**

- Expand navigation in `src/App.tsx`, `src/screens/ActivityBar.tsx`, and
  `src/screens/AppShell.tsx`.
- Decompose `PrinterDashboard.tsx` so Monitor canvas, toolbar, Printer
  presentation, and dock are separate modules.
- Implement individual cards and compact rows, bounded count rosters, severity,
  freshness, first-run state, and responsive overlay.
- Add only reusable primitives justified by repeated use to
  `src/design-system/components/`, its index, tests, and Showcase.

**Backend responsibility:**

- Supply timestamps and adapter-health facts needed for freshness.
- Preserve last-known live values through connection loss and restart according
  to the focused status decision.
- Keep raw host job strings distinct from farm3d operational state.
- Own the normalized operational/readiness policy in a Rust domain module. The
  mapping must be consumable later by P7 eligibility without importing adapter
  strings or frontend presentation rules.

**Wiring responsibility:**

- Own Rust/TypeScript parity for the backend-owned operational/readiness result.
- Define which aggregate counts come from durable state versus live telemetry.
- Prove listener, backfill, stale-to-live transition, and missing-field behavior.

**Current files likely modified:**

- `src/App.tsx`
- `src/screens/ActivityBar.tsx`
- `src/screens/AppShell.tsx`
- `src/screens/PrinterDashboard.tsx`
- `src/printers/types.ts`
- `src/printers/printer-store.ts`
- `src-tauri/src/connections/mod.rs`
- `src-tauri/src/connections/supervisor.rs`
- `src-tauri/src/lib.rs`

> [!IMPORTANT]
> **Decision gate:**
> The focused spec must define operational-state precedence, freshness
> thresholds, `group`-to-location semantics, persisted view preferences, and
> whether last-known telemetry is persisted or reconstructed. None is settled
> by current code.

**Explicit non-scope:** Queue persistence, Attention persistence, camera data,
and Job control. UI affordances for later destinations may be inert only if
clearly labeled and excluded from production navigation until wired.

**Exit gate:** Frontend/Rust tests, responsive visual verification, keyboard
verification, restart/freshness integration test, and tracer bullet complete.

## Phase P2: Printer lifecycle and batch setup

**Primary owner:** Wiring owner, because partial batch success and credential/
supervisor ordering cross both tiers.

**Supporting owners:** Frontend and Backend.

**Tracer bullet:** Create one Profile-only Printer and one connected Printer;
batch-create independent Printers across two bays with one failed row retained;
archive one without losing identity.

**Frontend responsibility:**

- Replace the current add flow with Identify, Connect, Operate, and Review for
  this phase. Equip/material and camera steps remain absent until their owners
  land.
- Include shared bed setup using only the catalog-backed bed field already
  supported by Printer Profile overrides. Preview the copied value per row;
  unsupported physical setup fields are not invented.
- Implement batch generation, paste/CSV intake, discovery mapping, row
  selection, per-row results, retry, and explicit shared-field application.
- Extend Setup detail for location, safety, Connection, archive, and guarded
  deletion.

**Backend responsibility:**

- Persist location, Setup-incomplete derivation, archive, and start-safety
  policy.
- Apply the selected supported bed setup independently to each created Printer;
  later edits do not propagate through a batch entity.
- Implement bounded batch probing and partial commits without hidden shared
  inheritance.
- Validate duplicate hosts, deletion prerequisites available at this phase,
  Connection replacement, variant rebind, and drift handling.
- Preserve supervisor and credential cleanup ordering.
- Expose one archive/delete eligibility interface that later domain modules can
  extend with references and blockers; P2 does not claim final Job, Spool, or
  Incident integrity before those records exist.

**Wiring responsibility:**

- Define batch request/result contracts with stable row correlation and
  machine-readable row errors.
- Prove testing does not save or start supervision.
- Prove partial success does not discard failed input.
- Prove shared credentials never appear in frontend results or diagnostics.

**Current files likely modified:**

- `src/screens/PrinterAddDialog.tsx`
- `src/screens/PrinterConnectionPanel.tsx`
- `src/screens/PrinterStatusPanel.tsx`
- `src/screens/PrinterProfilePanel.tsx`
- `src/printers/types.ts`
- `src/printers/printer-store.ts`
- `src-tauri/src/printers.rs` or its focused replacement modules
- `src-tauri/src/connections/commands.rs`
- `src-tauri/src/connections/discovery.rs`
- `src-tauri/src/lib.rs`

**Proposed backend decomposition, subject to focused design:**

- `src-tauri/src/printers/commands.rs`
- `src-tauri/src/printers/store.rs`
- `src-tauri/src/printers/batch.rs`

> [!IMPORTANT]
> **Decision gate:**
> The focused spec must define CSV columns, canonical host identity, batch probe
> concurrency/cancellation, Setup-incomplete derivation, connection replacement,
> and whether incomplete rows become Printer records or persisted drafts.

> [!CAUTION]
> Material Slots and camera templates shown in the umbrella workflow are not P2
> implementation assumptions. P3 and P8 own those schemas and later extend the
> setup sequence through explicit integration tasks.

**Exit gate:** Migration tests, batch partial-failure tests, credential redaction,
archive/restart tests, initial delete-eligibility tests, frontend workflow tests,
and tracer bullet complete. Final cross-domain delete integrity remains an
explicit integration obligation in P3, P7, and P8.

## Phase P3: Spools and Material Slots

**Primary owner:** Backend owner.

**Supporting owners:** Frontend and Wiring.

**Tracer bullet:** Create a Spool, load it into one Material Slot, atomically
move an existing Spool to storage, restart, and inspect movement/confidence
history.

**Frontend responsibility:**

- Add the Spools destination, inventory table, composable facets, detail, and
  add/update-weight/move/unload flows.
- Extend Printer Status/Setup with named slots after the backend contract lands.
- Add Equip to single-Printer setup and shared batch setup using the P3 slot
  contract. Batch setup copies slot layout only; loaded Spools remain
  per-instance and require explicit row-level choices.
- Distinguish lifecycle, location, low, reserved, measured, and estimated facets.

**Backend responsibility:**

- Own Spool, tare, Material Slot, occupancy, movement, amount adjustment,
  confidence, and correction invariants.
- Enforce one physical Spool in at most one slot.
- Make displacement plus new loading one transaction.
- Expose reservation primitives for P7 without implementing Queue policy.
- Extend the P2 archive/delete eligibility interface so loaded Spools,
  unresolved movements, and future reservations preserve references and block
  permanent deletion when required.

**Wiring responsibility:**

- Define inventory commands/events and authoritative returned state.
- Prove concurrent movement rejection and UI settlement after optimistic input.
- Expose availability changes for future dispatch reevaluation.
- Prove archive preserves slot/movement history and deletion cannot orphan it.
- Prove single and batch setup create the intended independent slot layouts and
  never clone loaded-Spool occupancy across Printer instances.

**New module roots proposed, not pre-approved:**

- `src/spools/`
- `src/screens/SpoolInventory.tsx`
- `src-tauri/src/spools/`

> [!CAUTION]
> **Blocking gate:**
> `supportsMultiFilament` does not provide slot count, names, topology, or
> constraints. The phase cannot claim automatic AMS/MMU topology until catalog
> and hardware research identifies a trustworthy source. The focused spec must
> define a user-configurable fallback.

> [!IMPORTANT]
> **Decision gate:**
> Settle weight units/precision, material taxonomy, tare reuse, slot capability
> sources, reservation arithmetic, and correction-history semantics before
> schema implementation.

**Exit gate:** Transaction and concurrency tests, restart persistence, frontend
flow tests, single/batch setup integration, Printer-detail integration,
archive/delete reference tests, and tracer bullet complete.

## Phase P4: Library persistence and Projects

**Primary owner:** Backend owner for storage; Frontend owner for workspace. The
Wiring owner is accountable for phase completion.

**Tracer bullet:** Import one managed and one linked Model into a Project,
restart, modify/remove the linked source, and recover it without invalidating
prior Model Source Revisions.

**Frontend responsibility:**

- Replace hardcoded `MODELS` and the local `Model` interface.
- Implement Project/saved-view navigation, grid/list, import/file-picker/drop,
  managed/linked choice, duplicate resolution, and missing-link recovery.
- Keep viewport preparation state separate from Library persistence state.

**Backend responsibility:**

- Own Projects, Models, Model Source Revisions, managed content, linked-source
  watching, duplicate identity, and metadata extraction.
- Define file ownership and cleanup with immutable revision references.
- Parse or inspect STL, supported 3MF, and G-code through evidence-backed
  libraries selected by the focused spec.
- For imported G-code, persist inspection results and provenance needed by P5
  to create an external Slice Revision; P4 does not invent missing Printer,
  nozzle, or material facts.
- Persist the exact immutable imported G-code bytes as a Model Source Revision
  or managed/linked equivalent defined by the P4 spec. P4 may inspect and retain
  G-code, but production **Create Queue Entry** behavior remains unavailable
  until P5 atomically publishes an external Slice Revision.

**Wiring responsibility:**

- Wire native file selection/drop and define `just web` fixture behavior.
- Define source-change event/backfill ordering.
- Prove unsupported rich-3MF metadata is reported before loss.
- Prove G-code inspection/import failure or cancellation leaves no partial
  external Slice Revision and preserves enough immutable provenance for a later
  P5 retry.

**Current files likely replaced or substantially changed:**

- `src/App.tsx` hardcoded `MODELS`
- `src/screens/ModelLibrary.tsx`
- `src/screens/BuildPlate.tsx`

**New module roots proposed, not pre-approved:**

- `src/library/`
- `src-tauri/src/library/`

> [!IMPORTANT]
> **Decision gate:**
> Select parser libraries, content hash, managed storage layout, watcher
> behavior, path/symlink policy, duplicate semantics, thumbnail ownership, and
> rich-3MF preservation boundaries from fixtures and platform evidence.

> [!CAUTION]
> `tauri-plugin-opener` is not a file-picker or watcher. No required plugin or
> Rust crate is currently selected; capability changes must follow the chosen
> implementation rather than be guessed in advance.

**Exit gate:** Format fixture suite, managed/linked restart test, source-change
test, missing-link recovery, duplicate handling, frontend tests, and tracer
bullet complete. G-code is inspectable and durably retained but is not exposed
as dispatchable work before P5.

## Phase P5: Runtime slicing and Slice Revisions

**Primary owner:** Protocol/Backend owner for OrcaSlicer; Frontend owner for
plate preparation. Wiring owns completion.

**Tracer bullet:** Prepare a two-plate fixture, slice each plate into a distinct
immutable Slice Revision, import one pre-sliced G-code fixture as an external
Slice Revision, restart, and inspect all revisions unchanged.

**Frontend responsibility:**

- Replace static `BuildPlate` with the renderer selected and recorded by the P5
  focused spec, then implement its approved preparation tools.
- Provide keyboard/numeric transform equivalents.
- Implement target/material/quality controls, validation, progress, cancel,
  collapsed logs, failure logs, and Slice Revision review.
- Implement multiple plate tabs and a G-code fact-confirmation flow. Untrusted or
  absent target Profile, nozzle, and material facts are visibly distinguished.
- Expose a queue-handoff intent without creating Queue data before P7.

**Backend responsibility:**

- Discover or package supported OrcaSlicer versions.
- Build verified invocation inputs, supervise/cancel the process, capture logs,
  validate output, extract estimates, and publish immutable artifacts.
- Link one plate and one Model Source Revision to each farm3d-produced Slice
  Revision.
- Create externally produced Slice Revisions from P4 G-code inspection plus
  operator-confirmed facts. Preserve imported-G-code provenance without
  fabricating a plate identity, and mark missing facts so P7 can require manual
  Printer selection.
- Clean interrupted temporary output without deleting published revisions.

**Wiring responsibility:**

- Define background-operation command/event/backfill contracts.
- Map supported farm3d settings to verified OrcaSlicer inputs.
- Prove cancel, failure, app restart, and stale source behavior.
- Prove each plate maps to one revision and that an external G-code revision
  cannot silently gain inferred compatibility facts.

**New module roots proposed, not pre-approved:**

- `src/slicing/`
- `src-tauri/src/slicing/`

> [!CAUTION]
> **Blocking gate:**
> ADR-0003 chooses a separate OrcaSlicer process, but the repository does not
> establish executable discovery, supported versions, CLI arguments, preset
> sources, packaging, progress, or cancellation. A runtime spike must precede
> the focused implementation plan.

> [!WARNING]
> The bundled derived Printer catalog intentionally omits G-code templates and
> creative preset content. It must not be assumed sufficient to invoke
> OrcaSlicer correctly.

**Exit gate:** Approved runtime spike, deterministic invocation fixtures,
cancellation/restart tests, two-plate identity tests, external G-code provenance
and missing-fact tests, immutable artifact tests, accessible viewport
verification, packaged/installed OrcaSlicer execution on every platform declared
supported by F0, and tracer bullet complete.

## Phase P6: Connection command capabilities

**Primary owner:** Protocol owner.

**Supporting owners:** Backend, Wiring, and Frontend.

**Tracer bullet:** Upload one immutable Slice Revision without starting,
interrupt or obscure the response, restart, and reconcile artifact/host state
without duplicate upload or accidental start.

**Frontend responsibility:**

- Present capability-aware controls and explicit unsupported states.
- Show staging, uncertain outcome, reconciliation, and host-control errors.
- Do not render unavailable controls as if every adapter supports them.

**Backend responsibility:**

- Research and implement upload, start, pause, resume, cancel, host-state,
  artifact-identity, and camera capabilities per adapter.
- Centralize adapter construction currently duplicated in
  `connections/commands.rs` and `connections/supervisor.rs`.
- Keep wire framing/parsing pure and adapter-local.
- Persist operation identity and uncertain outcomes required by P7.
- Extend Printer archive/delete eligibility so active or uncertain host
  operations retain their Printer identity and block permanent deletion until
  reconciled.
- Block clearing/replacing the Connection and deleting its credential while an
  operation is active or uncertain. The focused spec must define an explicit
  **abandon reconciliation** recovery for permanently unreachable hosts, with
  risk confirmation and durable history, before destructive mutation proceeds.

**Wiring responsibility:**

- Define the capability matrix exposed to SolidJS and Jobs.
- Prove unsupported versus failed behavior is distinguishable.
- Run real-host failure injection and restart reconciliation.
- Prove archive preserves operation records and deletion cannot orphan an active
  or uncertain upload/control outcome.
- Prove Connection replacement/clear and credential deletion are rejected while
  reconciliation depends on the original endpoint, then become available after
  resolution or explicit abandonment.

P6 is planned and gated per adapter. A Moonraker-focused P6 plan may execute
after A0.1 and P5. OctoPrint command work requires A0.2 plus separate command
research. ElegooLink requires an A0.3 go decision. One adapter completing P6 is
sufficient to unblock the first P7 tracer; it does not confer parity on others.

**Current module root:** `src-tauri/src/connections/`

> [!IMPORTANT]
> **Decision gate:**
> The existing two-method `PrinterConnection` interface is observation-only.
> The focused spec must compare extending it, introducing a separate command
> interface, or composing capability-specific interfaces. Do not add methods
> incrementally before comparing these seams.

> [!CAUTION]
> **Blocking gate:**
> Adapter parity is not assumed. ElegooLink stays excluded until A0.3 passes.
> OctoPrint status support from A0.2 does not prove write or camera support.

**Exit gate, per adapter:** Evidence-backed capability row, real-host command
tests, uncertainty/restart tests, archive/delete operation-reference tests,
frontend capability behavior, and tracer bullet. Unsupported capabilities are
recorded as explicit results, not failed phase work.

## Phase P7: Queue Entries, Jobs, and dispatch

**Primary owner:** Backend owner for state machine/transactions; Wiring owner
for phase delivery.

**Tracer bullet:** Create three linked Queue Entries from one Slice Revision,
explain eligibility, atomically create one Job and reserve Printer/Spools,
stage, require bed-clear confirmation, start, complete, offer measured
correction, and survive restart at every transition. Separately exercise failed
and cancelled Jobs through estimated, measured, and deferred reconciliation.

**Frontend responsibility:**

- Add Queue destination and Monitor Queue preview.
- Implement ordered rows, filters, detail, keyboard/pointer reorder, blocker
  explanations, candidates, policy, assign/start separation, release, retry,
  cancel, and history links.
- Implement copy quantity as linked independent Queue Entries and preserve their
  lineage without combining their state.
- Implement completion/failed/cancelled reconciliation choices: accept estimated
  use, enter measured remaining weight, or defer where permitted. Deferred
  amount remains visibly unavailable.
- Surface durable deferred-reconciliation requirements in Queue and Spools even
  before P8 adds the global Attention center.
- Present authoritative transitions; do not synthesize them from host strings.

**Backend responsibility:**

- Own Queue Entry and Job state machines, queue order, eligibility, deterministic
  evaluator, assignment transactions, Spool reservations, command handoff,
  release/retry/cancel, material accounting, and startup reconciliation.
- Serialize automatic evaluation and make external command handoff idempotent or
  explicitly uncertain.
- Preserve copy lineage and make each Queue Entry/Job independently assignable,
  retryable, cancellable, and reconcilable.
- Apply successful estimated deductions, measured corrections, and deferred
  uncertainty to P3 accounting primitives; prevent uncertain amounts from being
  over-reserved.
- Persist a reconciliation-required condition with stable identity whenever the
  operator defers. This is the authoritative source P8 later projects into an
  Attention Event; P7 does not emit a transient placeholder notification.
- Extend Printer archive/delete eligibility for active Jobs, Queue assignments,
  unresolved Spool reservations, and preserved historical references.

**Wiring responsibility:**

- Define command/event contracts for every user-visible transition.
- Prove phase behavior at each crash boundary.
- Reconcile farm3d Job identity with host artifact/state without conflating an
  unrelated host print.
- Prove all reconciliation choices update Job, Spool availability, Attention
  projection intent, and UI state exactly once.
- Prove archive preserves Job identity and permanent deletion cannot orphan a
  Queue Entry, Job, reservation, or history record.

**New module roots proposed, not pre-approved:**

- `src/queue/`
- `src/jobs/`
- `src-tauri/src/queue/`
- `src-tauri/src/jobs/`

> [!CAUTION]
> **Blocking gate:**
> P7 may not start until P3 reservation primitives, P5 immutable artifact
> identity, and P6 command/reconciliation interfaces are approved and tested.

> [!IMPORTANT]
> **Decision gate:**
> The focused spec must enumerate legal transitions, transaction boundaries,
> evaluator triggers, tie-break fixtures, manual host-job handling, operation
> IDs, and restart outcomes. These become executable state-machine tests before
> automatic dispatch is enabled.

**Exit gate:** State-machine model tests, transaction/concurrency tests,
three-copy lineage tests, successful/failed/cancelled reconciliation tests,
archive/delete reference tests, restart matrix, frontend workflow tests,
live-host tracer bullet, and no automatic dispatch behind an unproven adapter.

## Phase P8: Attention, Incidents, cameras, notifications

**Primary owner:** Backend owner for event lifecycle; Wiring owner for deep links
and desktop behavior.

**Tracer bullet:** Cause one recoverable failure, create one deduplicated open
Attention Event, notify while unfocused, deep-link to source, acknowledge without
resolving, and resolve when recovery completes.

**Frontend responsibility:**

- Implement Attention trigger/center, badges, severity, filters, deep-linked
  object selection, Incident timeline, camera health, and snapshot review.
- Preserve the distinct unread/read, unacknowledged/acknowledged, and
  open/resolved dimensions.
- Extend Printer Setup with optional camera source, test snapshot, per-Printer
  alert defaults, and the absence/unsupported states.
- Add camera template and alert-default steps to single and shared batch setup.
  Batch templates copy non-address configuration defaults only; every camera
  endpoint is entered or mapped per instance. Test results, runtime media, and
  captured evidence also remain per-instance.
- Add notification-class and snapshot-retention settings required for P8. P9
  later integrates these into the complete Settings workspace rather than
  deferring their functionality.

**Backend responsibility:**

- Own Attention lifecycle, condition deduplication, recurrence, Incident
  records, notification dispatch, snapshots, retention, disk cap, and pinning.
- Consume normalized domain events rather than reparsing raw adapter strings.
- Project every durable P7 reconciliation-required condition into one Attention
  Event idempotently, including startup backfill; resolving reconciliation
  resolves the projected Event without deleting either history.
- Persist camera configuration and per-Printer alert defaults separately from
  runtime media state.
- Extend Printer archive/delete eligibility so Incidents and Attention history
  retain Printer identity and evidence references.

**Wiring responsibility:**

- Define deep links as destination plus selected object and fallback when the
  object is archived or removed.
- Verify app-focus detection and notification permissions on every platform in
  F0's supported-platform matrix.
- Connect source condition clearing to resolution without erasing history.
- Wire camera test/configuration and global notification settings through the
  same authoritative retention/permission state used by background capture.
- Prove single and batch setup apply camera/alert defaults without copying
  per-instance runtime state or evidence.
- Prove reconciliation-required backfill creates no duplicate Attention Events
  across restart or repeated source updates.
- Prove archived Printers remain deep-linkable through history and permanent
  deletion cannot orphan Incident evidence.

**New module roots proposed, not pre-approved:**

- `src/attention/`
- `src/incidents/`
- `src-tauri/src/attention/`
- `src-tauri/src/incidents/`
- `src-tauri/src/cameras/`

> [!IMPORTANT]
> **Decision gate:**
> Focused specs must define deduplication keys, recurrence, auto-resolution,
> camera source types, capture timing, notification plugin/permissions,
> retention concurrency, and disk-budget enforcement.

> [!CAUTION]
> Camera capability is optional. Its absence must not block Printer monitoring,
> slicing, assignment, or Job control.

**Exit gate:** Event lifecycle tests, deduplication tests, retention/pinning
tests, single/batch Printer Setup camera/alert tests, archive/delete reference
tests, deep-link tests, `just package` plus installed-bundle
notification/capability verification on each platform declared supported by F0,
and tracer bullet complete.

## Phase P9: History, Settings, backup/restore, diagnostics

**Primary owner:** Backend owner for portability and redaction; Frontend owner
for the Settings/history workspace. Wiring owns restore verification.

**Tracer bullet:** Back up a Farm containing one entity from each domain,
preview-restore into conflicting local data, restore with secrets excluded, and
export diagnostics that contain none of a seeded credential corpus.

**Frontend responsibility:**

- Complete searchable Job history and immutable timelines.
- Replace the theme-only gear menu with the approved Settings workspace while
  preserving theme preview behavior.
- Implement backup selection, restore conflict preview, storage/retention views,
  diagnostics selection, and typed destructive confirmation.

**Backend responsibility:**

- Expose complete history snapshots and timelines.
- Create versioned backup manifests and consistent snapshots.
- Validate restore, create a safety backup, resolve/reject conflicts according
  to explicit policy, and recover interrupted restore.
- Collect and redact diagnostics, manage disk use, and perform deliberate reset.
- Preserve archived Printer identity snapshots across backup/restore and enforce
  final delete eligibility across Spools, Queue Entries, Jobs, Attention Events,
  Incidents, media, and history.

**Wiring responsibility:**

- Define cross-domain backup inventory without coupling backup code to private
  storage internals.
- Verify reference integrity and counts after restore.
- Seed secrets in credentials, URLs, headers, errors, and logs and prove they do
  not escape.
- Run the final cross-domain archive/delete matrix and prove no deletion or reset
  path leaves dangling references.

**Current files likely modified:**

- `src/screens/SettingsMenu.tsx`
- `src/screens/ThemePopover.tsx`
- `src/settings/settings-store.ts`
- `src-tauri/src/settings.rs`
- `src-tauri/src/lib.rs`

**New module roots proposed, not pre-approved:**

- `src/screens/SettingsWorkspace.tsx`
- `src/history/`
- `src-tauri/src/history/`
- `src-tauri/src/backup/`
- `src-tauri/src/diagnostics/`

> [!CAUTION]
> **Blocking gate:**
> Backup/restore design must wait for stable persisted schemas from P2–P8.
> Copying live files is not a consistent backup strategy.

> [!IMPORTANT]
> **Decision gate:**
> The focused spec must define archive format, checksums, compatibility window,
> linked-path portability, conflict identity, merge/replace policy, safety-backup
> cleanup, log rotation, redaction threat model, and partial-reset recovery.

**Exit gate:** Versioned fixture backups, compatibility tests, restore failure
injection, post-restore integrity checks, diagnostics secret scan, Settings UI
tests, final archive/delete reference matrix, and tracer bullet complete.

## Responsibility matrix

| Phase | Primary role | Frontend owner | Backend owner | Wiring owner | Protocol owner |
|---|---|---|---|---|---|
| F0 Baseline | Wiring | Inventory UI behavior | Inventory persistence/runtime | Baseline report and tracer | Live Moonraker evidence |
| A0 Adapters | Protocol | Real-state tolerance | Adapter implementation | Setup/restart/status path | Evidence and wire contracts |
| F1 Foundation | Backend | Store/type strategy input | Persistence/migration | Command/event/deep-link conventions | Not required |
| P1 Monitor | Frontend | Shell, Monitor, dock, a11y | Freshness, health, normalized operational state | Type parity and live path | Adapter status consultation |
| P2 Printer lifecycle | Wiring | Single/batch workflows | Lifecycle/batch invariants | Partial success and credentials | Probe behavior consultation |
| P3 Spools | Backend | Inventory/slot workflows | Transactions/accounting | Commands/events and settlement | Catalog/hardware topology evidence required by focused spec |
| P4 Library | Split; Wiring accountable | Projects/import/recovery | Storage/revisions/parsers | Native files and source events | Format-library evidence |
| P5 Slicing | Split; Wiring accountable | Viewport/settings/progress | Process/artifacts/revisions | Operation events and Profile mapping | OrcaSlicer contract |
| P6 Commands | Protocol | Capability-aware controls | Command adapters/reconciliation | Capability matrix/end-to-end | Real-host evidence |
| P7 Queue/Jobs | Backend; Wiring accountable | Queue and controls | State machine/scheduler/transactions | Full production loop | Adapter command consultation |
| P8 Attention | Backend; Wiring accountable | Attention/Incident/camera UI | Lifecycle/media/notifications | Deep links and OS behavior | Camera/notification evidence |
| P9 Portability | Backend; Wiring accountable | History/Settings/restore UI | Backup/restore/redaction | Cross-domain integrity | Not required |

> [!IMPORTANT]
> “Split; Wiring accountable” means frontend and backend have separate module
> owners, while one wiring owner is responsible for declaring the phase done.
> It does not mean shared or ambiguous ownership.

## Per-phase documentation and review contract

Before implementation, each P phase produces:

1. A focused design spec resolving every **Decision gate** and **Blocking
   gate** relevant to that phase.
2. A file responsibility map separating frontend, backend, and wiring changes.
3. Exact Rust/TypeScript interfaces and behavioral invariants.
4. Migration and restart behavior for persisted state.
5. A task-level implementation plan with red/green test steps.
6. A standards review against `AGENTS.md`, `DESIGN.md`, `CONTEXT.md`, and ADRs.
7. A spec review against the focused spec and this approach.

No phase may borrow an unresolved decision from a later phase merely to make a
mock screen appear complete. Instead, omit the control, label it as unavailable
in development-only fixtures, or define a narrow integration seam that the later
phase owns.

## Verification contract

Every frontend-affecting phase runs:

```sh
just build
just test
```

Every backend-affecting phase runs:

```sh
source "$HOME/.cargo/env" && just test-rust
```

Every vertical phase also:

- Exercises its tracer bullet through the Tauri path, not only `just web`.
- Verifies 1440 × 900 and 1024 × 700 where UI is involved.
- Verifies keyboard operation for every new interaction.
- Tests restart at each durable transition it introduces.
- Tests partial failure without discarding valid user input.
- Scans diagnostics/events/errors for seeded secrets when credentials or logs
  are involved.
- Records hardware, OS, and third-party versions used for live verification.
- Runs `source "$HOME/.cargo/env" && just package` and verifies an installed
  bundle on every F0-declared supported platform when the phase adds or changes
  bundled executables, Tauri plugins, capabilities, permissions, or resources.

> [!NOTE]
> `just web` remains valuable for deterministic UI development, but it cannot
> prove filesystem, credential, process, notification, camera, protocol, or
> transactional behavior.

## Release checkpoints

1. **Observable Farm:** F0 + F1 + P1. Existing Printer management and
   Moonraker monitoring work through the redesigned shell.
2. **Configurable Farm:** P2. Printer lifecycle, batch setup, archive, and
   safety policy are durable.
3. **Preparation foundation:** P3 + P4. Inventory and Library data survive real
   workflows and restart.
4. **Immutable production artifact:** P5. farm3d can produce and retain a Slice
   Revision without dispatch.
5. **Safe host control:** One adapter-specific A0 evidence branch plus its P6
   implementation. At least one adapter can stage and control a real Printer
   with restart reconciliation.
6. **Core v1 production loop:** P7. Model Source Revision → Slice Revision →
   Queue Entry → Job → completion/material accounting works end to end.
7. **Operational resilience:** P8. Attention, Incidents, camera evidence where
   available, and desktop notifications are durable.
8. **Portable and supportable v1:** P9. History, backup/restore, diagnostics,
   disk management, and reset are verified.

Each checkpoint must be usable without later phases. For example, P5 ends at an
inspectable Slice Revision; it does not ship a fake Dispatch action. P6 exposes
capabilities; it does not create an in-memory Queue. P8 cameras remain optional.

## Known decisions that must not be reopened casually

- farm3d is single-user and local for v1.
- Printers are catalog-backed; unsupported custom Printer Profiles are not v1.
- Projects are organizational folders, not orders.
- Model storage is selected per import: managed or linked.
- Slice Revisions are immutable.
- Queue Entries become Jobs only when assigned to a Printer.
- Dispatch Policy is per Queue Entry.
- Printer start safety is a separate gate and defaults to confirmation.
- Spools and Material Slots are first-class; reservation is not consumption.
- Archive is the default Printer retirement behavior.
- Cameras are optional; snapshots default to configurable 30-day retention.
- Desktop notifications are included; webhooks are not v1.
- The UI remains a dense editor tool and uses the existing design system.

Changing one of these requires updating the umbrella spec and, when the decision
is hard to reverse and surprising, an ADR before changing a focused plan.

## Known unknowns register

The following have an owning phase and may not be silently decided elsewhere:

| Unknown | Owning phase | Required evidence |
|---|---|---|
| Persistence technology and migrations | F1 | Transaction, crash, migration, restore requirements |
| Cross-tier type strategy | F1 | Current manual mirrors versus evaluated alternatives |
| Navigation/deep-link representation | F1/P1 | Shell and notification selection requirements |
| Operational-state precedence/staleness | P1 | Adapter status fixtures and UX states |
| `group` to location migration | P1/P2 | Existing persisted files and approved vocabulary — **Resolved (P2)**: no migration exists or is needed. P2's `location` is new, durable Printer data (migration `0003_p2_printer_lifecycle.sql`); it does not derive from or replace any persisted `group` value. |
| Batch CSV and host matching | P2 | User workflow plus discovery fixtures — **Resolved (P2)**: see the P2 design's D2 (canonical host identity), D10 (CSV/paste intake format and rules), and D12 (discovery mapping). |
| Slot count/topology source | P3 | Catalog and real-hardware evidence — **Resolved (P3)**: see the P3 design's D4. `supportsMultiFilament` does not establish slot count, names, or topology (only 12 of 971 catalog variants set it, and the catalog has no AMS/MMU data), so P3 derives no slot layout automatically. Every Printer gets a user-configured, ordered layout of 1–16 named Material Slots, defaulting to one slot named "Main". |
| Weight precision/material taxonomy | P3 | Inventory and slicer requirements — **Resolved (P3)**: see the P3 design's D1 (integer milligrams everywhere below the UI, grams-only display, one-decimal entry) and D2 (`MaterialFamily` is a closed enum drawn from the OrcaSlicer `filament_type` values at the catalog's pinned `v2.4.2` tag, plus `OTHER`). |
| Parser and watcher libraries | P4 | Representative STL/3MF/G-code/platform fixtures |
| Managed-content layout and hashing | P4 | Duplicate, revision, cleanup, backup requirements |
| Geometry renderer | P5 | Accessibility, performance, format, and plate needs |
| OrcaSlicer runtime contract | P5 | Executable and installed-bundle spike on every F0-declared supported platform |
| Adapter command/camera capabilities | P6 | Real Moonraker/OctoPrint/ElegooLink evidence |
| Queue/Job state machine | P7 | Executable transition and crash matrix |
| Attention deduplication/recurrence | P8 | Source-condition scenarios |
| Notification plugin/OS behavior | P8 | Supported-platform spike |
| Backup format/conflict policy | P9 | Stable schemas and portability threat model |
| Diagnostics redaction | P9 | Seeded-secret corpus and export scan |

## Completion definition

The v1 program is complete only when all of these are true:

- Every umbrella-spec acceptance criterion maps to a delivered phase tracer.
- Each persisted domain has migrations, restart behavior, and backup treatment.
- Every frontend domain has loading, empty, stale, error, partial-success, and
  recovery states appropriate to its workflow.
- At least one real adapter supports the complete production loop safely.
- Unsupported adapter capabilities are explicit and do not present inert or
  misleading controls.
- Automated verification passes, required live checks are recorded, and no
  seeded credential appears in backups, events, errors, logs, or diagnostics.
- The focused phase specs and plans agree with `CONTEXT.md`, ADRs, `DESIGN.md`,
  and this approach.
