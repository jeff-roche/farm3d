# P1 Shell, Monitor, and Operational Status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver a responsive Monitor that loads persisted Printers and stale last-known telemetry immediately, transitions to backend-normalized live status, supports search/filter/section/density controls, and opens one reusable Printer detail dock.

**Architecture:** Rust owns telemetry retention, freshness, adapter health, and operational/readiness policy. The existing listener-before-backfill stream carries generated contracts to a SolidJS Monitor store that owns only presentation state and derives one shared view model for Shell counts, rosters, cards, rows, and detail. SQLite stores reconstructable telemetry snapshots and the two approved Monitor preferences without changing Printer revisions.

**Tech Stack:** Rust, Tauri 2, rusqlite/SQLite, ts-rs, SolidJS, TypeScript, Kobalte, CSS Modules, Vitest, Rust unit/integration tests.

**Spec:** `docs/superpowers/specs/2026-09-18-p1-shell-monitor-operational-status-design.md`

## Global Constraints

- Rust remains persisted truth; frontend display state is not domain state.
- Operational precedence is `setupIncomplete → error → offline → connecting → unknown → printing/paused/busy → ready`.
- Readiness is `ready` only when operational state is `ready`.
- Cached restart telemetry starts stale; explicit offline/error makes retained telemetry stale immediately; online telemetry expires after 30 seconds.
- Adapters provide a health observation at least every 10 seconds.
- Raw adapter strings never become Farm3D operational states or Jobs.
- Do not map legacy `group` to location. P1 exposes Printer Model, operational state, and none as usable Monitor Sections.
- Persist only `monitorSection` and `monitorDensity`; keep search, filter, selection, dock state/tab/width, focus, and scroll ephemeral.
- Queue persistence, Attention persistence, camera data, Job controls, Spools, batch setup, and durable location are out of scope.
- Use Kobalte primitives for interactive behavior, CSS Modules for component styles, and only `--f3d-*` color/type/radius tokens.
- New design-system components must be exported from `src/design-system/components/index.ts` and shown in `src/design-system/Showcase.tsx`.
- Missing telemetry renders as unavailable, never zero.
- Complete frontend work with `just build` and `just test`; complete Rust work with `source "$HOME/.cargo/env" && just test-rust`.

## File and module map

### Backend

- Create `src-tauri/migrations/0002_p1_monitor.sql`: Settings columns and the telemetry-cache table.
- Modify `src-tauri/src/persistence/migrations.rs`: ordered migration registry and schema version 2 validation.
- Create `src-tauri/src/printers/operational.rs`: pure operational/readiness/freshness policy.
- Modify `src-tauri/src/printers/mod.rs`: export the policy module.
- Create `src-tauri/src/connections/status_repository.rs`: telemetry snapshot persistence and write coalescing.
- Modify `src-tauri/src/connections/mod.rs`: normalized activity, health, telemetry, and status contracts.
- Modify `src-tauri/src/connections/moonraker/{mod.rs,protocol.rs}`: normalized host activity and 10-second liveness observations.
- Modify `src-tauri/src/connections/supervisor.rs`: retained observations, expiration, persistence, tombstones, and event publication.
- Modify `src-tauri/src/connections/commands.rs`: richer backfill and status-removal behavior.
- Modify `src-tauri/src/settings/{repository.rs,commands.rs}`: persisted Monitor preferences and import/export schema compatibility.
- Modify `src-tauri/src/contracts/domain.rs`: remove or alias duplicate status DTOs.
- Modify `src-tauri/src/lib.rs`: hydrate cache before restoring supervisors.
- Modify `src-tauri/tests/{export_contracts.rs,f0_tauri_path.rs}` and add focused repository/policy tests.

### Generated contracts and frontend state

- Regenerate `src/generated/contracts/**` with `just gen-contracts`; never hand-edit generated files.
- Modify `src/printers/{types.ts,printer-store.ts,printer-status-store.ts}` and tests: expose normalized runtime status and reconciliation state safely.
- Modify `src/settings/settings-store.ts` and tests: defaults and serialized preference updates.
- Create `src/monitor/monitor-store.ts` and test: all pure Monitor presentation derivation.

### Frontend presentation

- Create `src/design-system/components/{SeverityMarker,PrinterRoster}.{tsx,module.css}` and update component exports/tests/Showcase.
- Modify `src/screens/{ActivityBar,AppShell}.{tsx,module.css}` and add focused tests.
- Create `src/screens/MonitorToolbar.{tsx,module.css}`, `PrinterCard.{tsx,module.css}`, `PrinterCompactRow.{tsx,module.css}`, and tests.
- Create `src/screens/PrinterDetailDock.{tsx,module.css}` and test.
- Refactor `src/screens/PrinterDashboard.{tsx,module.css,test.tsx}` into the Monitor composition layer.
- Refactor `src/screens/PrinterStatusPanel.{tsx,module.css,test.tsx}` into operational Status content; reuse current Profile/Connection panels under Setup.
- Modify `src/App.{tsx,module.css}` and add `src/App.test.tsx` for startup/deep-link integration.

---

### Task 1: Schema v2 and Monitor preferences

**Owner:** Backend engineer  
**Prerequisites:** Approved spec only.  
**Handoff:** Tasks 3 and 5 consume schema v2 and generated Settings fields.

**Files:**
- Create: `src-tauri/migrations/0002_p1_monitor.sql`
- Modify: `src-tauri/src/persistence/migrations.rs`
- Modify: `src-tauri/src/settings/repository.rs`
- Modify: `src-tauri/src/settings/commands.rs`
- Modify: `src-tauri/src/persistence/database.rs` tests if migration assertions live there
- Modify: `src/settings/settings-store.ts`
- Modify: `src/settings/settings-store.test.ts`

**Interfaces:**
- Produces Rust/TS enum values `MonitorSection = Location | PrinterModel | OperationalState | None` and `MonitorDensity = Comfortable | Compact`.
- Extends `SettingsRecord` with `monitorSection` and `monitorDensity`.
- Extends `save_settings` input with both fields while retaining optimistic revision checks.

- [ ] **Step 1: Write migration and repository tests that fail on schema version 1**

Add assertions that opening a v1 database produces schema version 2, defaults existing rows, and preserves theme/revision. Add repository round-trip and validation cases:

```rust
assert_eq!(settings.monitor_section, MonitorSection::PrinterModel);
assert_eq!(settings.monitor_density, MonitorDensity::Comfortable);

let saved = repository.save(
    settings.revision,
    "farm3d-dark",
    MonitorSection::OperationalState,
    MonitorDensity::Compact,
)?;
assert_eq!(saved.monitor_section, MonitorSection::OperationalState);
assert_eq!(saved.monitor_density, MonitorDensity::Compact);
```

Run:

```sh
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml settings
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml migration
```

Expected: FAIL because schema v2 and the enum fields do not exist.

- [ ] **Step 2: Add the immutable schema v2 migration and ordered migration runner**

Use these columns and cache table; Task 3 will implement cache access:

```sql
ALTER TABLE settings
ADD COLUMN monitor_section TEXT NOT NULL DEFAULT 'printerModel'
CHECK (monitor_section IN ('location', 'printerModel', 'operationalState', 'none'));

ALTER TABLE settings
ADD COLUMN monitor_density TEXT NOT NULL DEFAULT 'comfortable'
CHECK (monitor_density IN ('comfortable', 'compact'));

CREATE TABLE printer_status_snapshots (
    printer_id TEXT PRIMARY KEY
        REFERENCES printers(id) ON DELETE CASCADE,
    telemetry_json TEXT NOT NULL
        CHECK (json_valid(telemetry_json) AND json_type(telemetry_json) = 'object'),
    last_observed_at TEXT NOT NULL,
    persisted_at TEXT NOT NULL
) STRICT;
```

Replace the one-off migration branch with an ordered registry containing exact version, name, SQL, and checksum. Apply each unapplied migration in one exclusive transaction and validate every applied row against the registry.

- [ ] **Step 3: Extend Settings contracts, repository, commands, and documents**

Define generated enums next to the command DTOs and update all load/save/import/export conversions. Settings document schema 1 remains readable by using serde defaults; export schema 2 with explicit fields:

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/MonitorSection.ts")]
pub enum MonitorSection { Location, PrinterModel, OperationalState, None }

impl Default for MonitorSection {
    fn default() -> Self { Self::PrinterModel }
}
```

Use the same pattern for `MonitorDensity`, defaulting to `Comfortable`. Import rejects invalid enum values but accepts schema-1 documents with omitted P1 fields.

- [ ] **Step 4: Extend the frontend Settings cache**

Use generated enum types and defaults:

```ts
const DEFAULT_SETTINGS: Settings = {
  revision: 1,
  themeMode: "system",
  monitorSection: "printerModel",
  monitorDensity: "comfortable",
  updatedAt: "",
};
```

Serialize all three mutable fields on every save so revision conflicts cannot lose a preference. Add tests proving web defaults and Tauri save payloads.

- [ ] **Step 5: Run focused and broad checks**

```sh
npm test -- src/settings/settings-store.test.ts
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml settings
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml migration
source "$HOME/.cargo/env" && just test-rust
```

Expected: PASS.

- [ ] **Step 6: Commit the schema and preference slice**

```sh
git add src-tauri/migrations/0002_p1_monitor.sql src-tauri/src/persistence/migrations.rs src-tauri/src/settings src/settings/settings-store.ts src/settings/settings-store.test.ts
git commit -m "feat: persist Monitor preferences"
```

### Task 2: Pure Rust operational policy

**Owner:** Backend engineer  
**Prerequisites:** Task 1 only for generated enum conventions; otherwise independent.  
**Handoff:** Task 4 constructs policy inputs; Tasks 5–10 consume outputs.

**Files:**
- Create: `src-tauri/src/printers/operational.rs`
- Modify: `src-tauri/src/printers/mod.rs`
- Modify: `src-tauri/tests/export_contracts.rs`

**Interfaces:**
- Produces `OperationalState`, `ReadinessState`, `ReadinessReason`, `PrinterReadiness`, and `TelemetryFreshness`.
- Produces pure function `evaluate_operational_status(input: &OperationalInput, now: DateTime<Utc>) -> OperationalResult`.
- `OperationalInput` contains setup completeness, Connection state/error, normalized host activity, `last_observed_at`, `fresh_until`, and `hydrated_from_cache`.
- `PrinterReadiness` contains `state: ReadinessState` and `reason: Option<ReadinessReason>`; ready has no reason.

- [ ] **Step 1: Write a table-driven failing policy test**

Cover every precedence branch and freshness boundary with an injected clock:

```rust
let cases = [
    (input().setup_complete(false).build(), OperationalState::SetupIncomplete, Some(ReadinessReason::SetupIncomplete)),
    (input().connection(ConnectionState::Error).build(), OperationalState::Error, Some(ReadinessReason::ConnectionError)),
    (input().connection(ConnectionState::Offline).build(), OperationalState::Offline, Some(ReadinessReason::Offline)),
    (input().connection(ConnectionState::Connecting).build(), OperationalState::Connecting, Some(ReadinessReason::Refreshing)),
    (input().online().activity(HostActivity::Printing).fresh().build(), OperationalState::Printing, Some(ReadinessReason::PrinterBusy)),
    (input().online().activity(HostActivity::Idle).fresh().build(), OperationalState::Ready, None),
];
```

Add exact boundary assertions: `now == fresh_until` is fresh; `now > fresh_until` is stale; hydrated cache is stale regardless of age; no observation is unavailable.

Run:

```sh
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml operational
```

Expected: FAIL because the module and types do not exist.

- [ ] **Step 2: Implement generated policy types and the pure evaluator**

Use a closed normalized input vocabulary:

```rust
pub enum HostActivity { Idle, Printing, Paused, Busy, Unknown }

pub struct OperationalResult {
    pub operational_state: OperationalState,
    pub readiness: PrinterReadiness,
    pub freshness: TelemetryFreshness,
}
```

Keep labels, colors, icons, Moonraker strings, and frontend filters out of this module. Make setup incomplete the first branch and ready the final branch.

- [ ] **Step 3: Register generated contracts and prove serialization**

Add every public P1 enum/struct to `export_registry()` and assert camelCase values, especially `setupIncomplete`, `notReady`, and `stale`.

- [ ] **Step 4: Run focused tests and contract generation**

```sh
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml operational
source "$HOME/.cargo/env" && just gen-contracts
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml export_contracts
```

Expected: PASS and generated files include every policy type.

- [ ] **Step 5: Commit the policy slice**

```sh
git add src-tauri/src/printers src-tauri/tests/export_contracts.rs src/generated/contracts
git commit -m "feat: define Printer operational policy"
```

### Task 3: Telemetry snapshot repository

**Owner:** Backend engineer  
**Prerequisites:** Tasks 1–2.  
**Handoff:** Task 4 hydrates and writes snapshots through this repository.

**Files:**
- Create: `src-tauri/src/connections/status_repository.rs`
- Modify: `src-tauri/src/connections/mod.rs`
- Modify: `src-tauri/src/lib.rs` test helpers

**Interfaces:**
- Produces `StoredTelemetrySnapshot { printer_id, telemetry, last_observed_at }`.
- Produces `StatusRepository::list()`, `get(printer_id)`, `save_if_due(snapshot, transition)`, and `delete(printer_id)`.
- Cache writes never update `printers.revision`.

- [ ] **Step 1: Write failing repository tests**

Use a temporary SQLite database and controlled timestamps. Assert:

```rust
repository.save_if_due(&snapshot_at("00:00:00Z"), SnapshotWrite::Periodic)?;
repository.save_if_due(&snapshot_at("00:00:10Z"), SnapshotWrite::Periodic)?;
assert_eq!(repository.get("prn-1")?.unwrap().last_observed_at, "00:00:00Z");

repository.save_if_due(&snapshot_at("00:00:10Z"), SnapshotWrite::ActivityTransition)?;
assert_eq!(repository.get("prn-1")?.unwrap().last_observed_at, "00:00:10Z");
```

Also assert a 30-second periodic write persists, deleting a Printer cascades its snapshot, malformed JSON returns a cache-specific read error, and Printer revision remains unchanged.

- [ ] **Step 2: Implement validated JSON storage and coalescing**

Serialize only the normalized Monitor telemetry struct. Store RFC3339 UTC timestamps. Compare the existing `persisted_at` to the injected/current clock for periodic writes; bypass coalescing for activity transitions.

- [ ] **Step 3: Isolate cache failures**

Return a narrow `StatusCacheError` to the manager. Callers log/report a recoverable warning but do not translate cache failure into Connection failure or Printer command failure.

- [ ] **Step 4: Run focused and broad Rust tests**

```sh
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml status_repository
source "$HOME/.cargo/env" && just test-rust
```

Expected: PASS.

- [ ] **Step 5: Commit the cache repository**

```sh
git add src-tauri/src/connections/status_repository.rs src-tauri/src/connections/mod.rs src-tauri/src/lib.rs
git commit -m "feat: persist last-known Printer telemetry"
```

### Task 4: Supervisor retention, liveness, and status events

**Owner:** Backend engineer  
**Prerequisites:** Tasks 2–3.  
**Handoff:** Task 5 receives the final generated `PrinterStatus` contract.

**Files:**
- Modify: `src-tauri/src/connections/mod.rs`
- Modify: `src-tauri/src/connections/moonraker/mod.rs`
- Modify: `src-tauri/src/connections/moonraker/protocol.rs`
- Modify: `src-tauri/src/connections/supervisor.rs`
- Modify: `src-tauri/src/connections/commands.rs`
- Modify: `src-tauri/src/contracts/domain.rs`
- Modify: `src-tauri/src/printers/mod.rs`
- Modify: `src-tauri/src/printers/commands.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/tests/f0_tauri_path.rs`
- Modify: `src-tauri/tests/export_contracts.rs`

**Interfaces:**
- Replace raw `jobState` with generated `hostActivity: HostActivity`; retain optional `hostActivityName`.
- `PrinterStatus` carries Connection health, retained telemetry, `lastObservedAt`, `freshUntil`, policy result, and a status update timestamp.
- Add `PrinterStatusEventType::{Changed, Removed}` serialized as `printer.status.changed` and `printer.status.removed`, plus tagged `PrinterStatusEventPayload::{Changed { status }, Removed}`; use `EventEnvelope<PrinterStatusEventType, PrinterStatusEventPayload>` on the event channel.
- Add `PrinterSetupFacts { has_usable_connection: bool, profile_resolved: bool }`; the manager evaluates setup-incomplete status for every durable Printer, including Printers with no Connection.
- `ConnectionManager::new(app, status_repository)` hydrates status before supervisors start.

- [ ] **Step 1: Write failing supervisor and protocol tests**

Add tests proving:

1. Moonraker strings normalize to `Idle`, `Printing`, `Paused`, `Busy`, or `Unknown` without leaving the adapter module.
2. A successful frame updates `lastObservedAt` and sets `freshUntil = lastObservedAt + 30s`.
3. A 10-second liveness tick emits a health observation even when telemetry is unchanged.
4. Error/reconnect updates Connection health but retains temperatures, progress, and host activity as stale.
5. Hydrated cache status starts stale.
6. A durable Printer without a usable Connection receives `setupIncomplete` status even though no adapter task starts.
7. `stop` publishes removal and deletes the map entry when a Printer is deleted; clearing only the Connection replaces it with `setupIncomplete` status.
8. The `status` inside a changed event is the same generated `PrinterStatus`
   shape returned in a backfill row.

Use a fake connection and injected clock rather than wall-clock sleeps:

```rust
manager.seed("prn-1", cached_status(215.0));
manager.apply_connection_error("prn-1", "unreachable", now);
let status = manager.statuses()["prn-1"].clone();
assert_eq!(status.telemetry.nozzle_temp_c, Some(215.0));
assert_eq!(status.freshness, TelemetryFreshness::Stale);
assert_eq!(status.operational_state, OperationalState::Error);
```

- [ ] **Step 2: Refactor the adapter channel into observations**

Change the trait channel from complete replacement statuses to normalized observations:

```rust
pub enum ConnectionObservation {
    Telemetry(PrinterTelemetry),
    Health { state: ConnectionState, observed_at: String },
}

async fn subscribe(
    &self,
    tx: Sender<ConnectionObservation>,
) -> Result<(), ConnectionError>;
```

Moonraker merges partial frames in `StatusSnapshot`, normalizes host activity, and uses `tokio::select!` between socket input and a 10-second interval. A liveness tick sends health only after the socket has been established; socket close returns to the supervisor.

- [ ] **Step 3: Make the supervisor merge instead of replace**

Maintain a per-Printer retained status. Apply setup facts, observations, Connection transitions, freshness expiration, and policy evaluation through one function before publishing. On each state change:

```rust
let next = merge_status(previous.as_ref(), observation, durable_setup, now);
status_repository.save_if_due(&next.telemetry_snapshot(), write_kind)?;
publish_changed(&app, &statuses, &id, next);
```

Do not allow persistence errors to overwrite `next` with an error status. Ensure the forwarding task drains accepted observations before shutdown or coordinate cancellation so an accepted final update is not discarded.

- [ ] **Step 4: Hydrate before restoring supervisors and publish removals**

Construct the repository from `Storage`, load snapshots into `StatusMap` as stale, then reconcile every durable Printer against catalog/Profile resolution and Connection configuration. Seed setup-incomplete status for Printers without usable Connections before starting configured supervisors. Update create/edit/import/clear/delete command paths so setup facts stay current: clearing a Connection deletes retained telemetry/cache and replaces status with telemetry-unavailable setup-incomplete, while deleting a Printer publishes removal and lets the foreign-key cascade delete its cache row.

- [ ] **Step 5: Consolidate duplicate DTOs and regenerate contracts**

Remove the stale alternate `contracts::domain::PrinterStatus` family or alias it to canonical runtime types. Register and regenerate the final event/status types:

```sh
source "$HOME/.cargo/env" && just gen-contracts
```

- [ ] **Step 6: Extend the Tauri-path restart tracer**

In `f0_tauri_path.rs`, persist a snapshot, destroy the first runtime, reopen storage, build a new manager, and assert:

```rust
assert_eq!(backfill[0]["status"]["freshness"], "stale");
assert_eq!(backfill[0]["status"]["telemetry"]["nozzleTempC"], 215.0);
// After a live observation:
assert_eq!(event["payload"]["freshness"], "fresh");
```

Also prove a racing event remains exactly-once and removal no longer leaves a backfill row.

- [ ] **Step 7: Run focused and broad checks**

```sh
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml connections
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml --test f0_tauri_path
source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml --test export_contracts
source "$HOME/.cargo/env" && just test-rust
```

Expected: PASS.

- [ ] **Step 8: Commit the backend vertical slice**

```sh
git add src-tauri/src src-tauri/tests src/generated/contracts
git commit -m "feat: retain and normalize Printer status"
```

### Task 5: Frontend status reconciliation and runtime model

**Owner:** Wiring engineer  
**Prerequisites:** Task 4 contracts.  
**Handoff:** Tasks 6, 8, 9, and 10 consume the runtime model.

**Files:**
- Modify: `src/printers/types.ts`
- Modify: `src/printers/printer-status-store.ts`
- Modify: `src/printers/printer-status-store.test.ts`
- Modify: `src/printers/printer-store.ts`
- Modify: `src/printers/printer-store.test.ts`

**Interfaces:**
- `ResolvedPrinter.runtimeStatus?: PrinterStatus` remains presentation-only.
- `StatusEvent = EventEnvelope<PrinterStatusEventType, PrinterStatusEventPayload>`; changed payloads upsert status and removed payloads delete it.
- Status store exposes `syncState(): "syncing" | "current" | "uncertain"` separately from backend `status.freshness`.
- Status removal deletes runtime state and calls `onStatusRemoved(printerId)`.
- `startStatusListener()` surfaces startup failure and always returns/disposes safely, including late listener registration.

- [ ] **Step 1: Write failing reconciliation tests**

Extend existing listener/backfill tests with:

```ts
expect(store.syncState()).toBe("uncertain");
await store.start();
expect(store.syncState()).toBe("current");

receive(statusRemovedEvent("prn-1"));
expect(store.statuses()["prn-1"]).toBeUndefined();
expect(onStatusRemoved).toHaveBeenCalledWith("prn-1");
```

Also test that a failed backfill keeps stale cached status visible, import preserves status only for surviving IDs, and dispose-before-listen-resolution invokes the eventual unlisten function exactly once.

Run:

```sh
npm test -- src/printers/printer-status-store.test.ts src/printers/printer-store.test.ts
```

Expected: FAIL on missing removal and sync-state behavior.

- [ ] **Step 2: Apply generated status without frontend policy**

Export generated types from `types.ts`; do not introduce TypeScript mappings from raw host state to operational state. Update event handling to branch on the generated event discriminator and delete status on removal.

- [ ] **Step 3: Expose synchronization uncertainty through printer-store**

Add accessors such as:

```ts
export const printerStatusSyncState = () => statusStore?.syncState() ?? "syncing";
```

Keep this separate from `runtimeStatus.freshness`. Ensure durable mutations retain runtime status for the same ID and imports prune statuses for absent IDs.

- [ ] **Step 4: Make startup/disposal errors explicit**

Have `startStatusListener` reject with a user-safe store error after listener/backfill startup failure. In `App`, Task 11 will use a cancellation guard so a disposer returned after unmount is immediately called.

- [ ] **Step 5: Run focused and broad frontend tests**

```sh
npm test -- src/printers/printer-status-store.test.ts src/printers/printer-store.test.ts
just test
```

Expected: PASS.

- [ ] **Step 6: Commit frontend status wiring**

```sh
git add src/printers
git commit -m "feat: reconcile normalized Printer status"
```

### Task 6: Monitor presentation store

**Owner:** Frontend engineer  
**Prerequisites:** Tasks 1 and 5.  
**Can run in parallel with:** Task 7.  
**Handoff:** Tasks 8–10 consume `MonitorPrinterView` and `MonitorSectionView`.

**Files:**
- Create: `src/monitor/monitor-store.ts`
- Create: `src/monitor/monitor-store.test.ts`

**Interfaces:**
- Produces `MonitorFilter = "all" | "attention" | "printing" | "ready" | "offline" | "setupIncomplete"`.
- Produces `MonitorPrinterView`, the single card/row/roster view model.
- Produces `MonitorSectionView { key, label, printers }`.
- Owns ephemeral search/filter/selection and persists only section/density through an injected preference writer.

- [ ] **Step 1: Write failing pure-store tests**

Use generated-status fixtures to prove:

- Search is case-insensitive over Printer name, vendor/model, and host activity name.
- Every filter uses normalized operational/readiness/severity fields.
- Stale last-known printing does not match Printing.
- Attention includes warning/fatal current conditions.
- Model grouping preserves `(vendor, model)` identity and trailing Unlinked behavior.
- Operational grouping uses the generated state.
- None produces one unlabelled section.
- Location is unavailable in P1 and falls back to Printer Model without rewriting an explicit saved preference.
- Filtered-empty differs from zero Printers.
- Rosters use the exact filtered membership, sort by Printer name, and expose first eight plus remaining count.

Example assertion:

```ts
store.setFilter("printing");
expect(store.visiblePrinters().map((printer) => printer.id)).toEqual(["fresh-print"]);
expect(store.visiblePrinters().some((printer) => printer.id === "stale-print")).toBe(false);
```

- [ ] **Step 2: Implement dependency-injected state and pure derivation**

Use this constructor boundary:

```ts
export interface MonitorStoreDependencies {
  printers: () => ResolvedPrinter[];
  initialSection: MonitorSection;
  initialDensity: MonitorDensity;
  persistPreferences: (next: {
    monitorSection: MonitorSection;
    monitorDensity: MonitorDensity;
  }) => Promise<void>;
}

export function createMonitorStore(dependencies: MonitorStoreDependencies): MonitorStore;
```

Build labels/icons later in components; the view model carries generated state, severity kind, exact readings, age timestamps, and accessible summary text.

- [ ] **Step 3: Serialize preference writes without revision races**

Queue preference writes so a quick section-then-density change saves the latest combined state after the prior `updateSettings` resolves. On save failure, keep the current in-memory choice and expose a recoverable preference error.

- [ ] **Step 4: Run focused tests**

```sh
npm test -- src/monitor/monitor-store.test.ts
```

Expected: PASS.

- [ ] **Step 5: Commit the Monitor model**

```sh
git add src/monitor
git commit -m "feat: derive Monitor presentation state"
```

### Task 7: Severity marker and bounded Printer roster

**Owner:** Frontend/design-system engineer  
**Prerequisites:** DESIGN.md and Task 5 generated status vocabulary.  
**Can run in parallel with:** Task 6.  
**Handoff:** Tasks 8–10 reuse both components.

**Files:**
- Create: `src/design-system/components/SeverityMarker.tsx`
- Create: `src/design-system/components/SeverityMarker.module.css`
- Create: `src/design-system/components/PrinterRoster.tsx`
- Create: `src/design-system/components/PrinterRoster.module.css`
- Modify: `src/design-system/components/index.ts`
- Modify: `src/design-system/components/components.test.tsx`
- Modify: `src/design-system/Showcase.tsx`
- Modify: `src/design-system/Showcase.module.css`

**Interfaces:**
- `SeverityMarkerProps { severity: "fatal" | "warning" | "info" | "resolved"; label: string }`.
- `PrinterRosterProps { label: string; count: number; printers: readonly { id; name; stateLabel }[]; onViewAll?: () => void }`.
- Roster caps rows internally at eight and exposes View all only when `count > 8`.

- [ ] **Step 1: Inspect the installed Kobalte Popover/HoverCard APIs**

Read `node_modules/@kobalte/core/src/popover/` and, if available, `hover-card/`. Select the primitive that supports interactive content, keyboard focus, outside dismissal, and focus return. Do not infer prop names.

- [ ] **Step 2: Write failing component tests**

Prove icon + visible text + severity class, roster keyboard opening, pointer opening, eight-row cap, View all, Escape dismissal, and focus return. Query by role/name rather than CSS classes:

```ts
expect(screen.getByText("Connection error")).toBeVisible();
expect(screen.getByLabelText("3 offline Printers")).toHaveAttribute("aria-expanded", "false");
```

- [ ] **Step 3: Implement tokenized components**

Use Tabler warning/stop/info/check icons and design tokens only. If hover and focus must be coordinated around a controlled Popover, centralize the open/close delay in `PrinterRoster`; do not create an unbounded Tooltip.

- [ ] **Step 4: Export and showcase every state**

Add all severity variants and roster examples with 0, 3, and 10 Printers to Showcase.

- [ ] **Step 5: Run component tests and build**

```sh
npm test -- src/design-system/components/components.test.tsx
just build
```

Expected: PASS.

- [ ] **Step 6: Commit shared primitives**

```sh
git add src/design-system
git commit -m "feat: add Monitor status primitives"
```

### Task 8: Expand the Shell

**Owner:** Frontend engineer  
**Prerequisites:** Tasks 5–7.  
**Can run in parallel with:** Task 10 after Task 7.  
**Handoff:** Task 11 integrates real App state.

**Files:**
- Modify: `src/screens/ActivityBar.tsx`
- Modify: `src/screens/ActivityBar.module.css`
- Create: `src/screens/ActivityBar.test.tsx`
- Modify: `src/screens/AppShell.tsx`
- Modify: `src/screens/AppShell.module.css`
- Create: `src/screens/AppShell.test.tsx`

**Interfaces:**
- Rename the visible rail destination from Printers to Monitor.
- `AppShellProps` accepts total/operational roster models, adapter health label/severity, and `lastLiveEventAt`; it does not calculate domain state.
- Keep only Monitor, existing Library, and existing Settings behavior available.

- [ ] **Step 1: Write failing Shell and activity-rail tests**

Assert accessible Monitor/Library/Settings controls, active state, top-bar roster opening by keyboard, omission of Job/Attention counts, adapter-health status, relative last-event age, and source order `nav → main → footer`.

- [ ] **Step 2: Implement Shell facts and rosters**

Replace `statusSummary: string` with structured props:

```ts
interface AppShellProps {
  active: ScreenId;
  title: string;
  printerRoster: PrinterRosterModel;
  operationalRosters: readonly PrinterRosterModel[];
  adapterHealth: { severity: Severity; label: string };
  lastLiveEventAt?: string;
  // existing navigation and children props
}
```

Format relative age from backend timestamps without reclassifying freshness. Update age text on a low-frequency timer and clean it up on unmount.

- [ ] **Step 3: Apply responsive CSS**

Keep the dense editor shell, tokenized typography/colors/radii, and reachable rail at 1024 × 700. Add `prefers-reduced-motion` overrides for Shell transitions.

- [ ] **Step 4: Run focused tests**

```sh
npm test -- src/screens/ActivityBar.test.tsx src/screens/AppShell.test.tsx
```

Expected: PASS.

- [ ] **Step 5: Commit the Shell slice**

```sh
git add src/screens/ActivityBar* src/screens/AppShell*
git commit -m "feat: expand the Monitor shell"
```

### Task 9: Monitor controls, cards, compact rows, and required states

**Owner:** Frontend engineer  
**Prerequisites:** Tasks 5–7.  
**Handoff:** Task 10 embeds the dock; Task 11 integrates startup and acceptance.

**Files:**
- Create: `src/screens/MonitorToolbar.tsx`
- Create: `src/screens/MonitorToolbar.module.css`
- Create: `src/screens/MonitorToolbar.test.tsx`
- Create: `src/screens/PrinterCard.tsx`
- Create: `src/screens/PrinterCard.module.css`
- Create: `src/screens/PrinterCard.test.tsx`
- Create: `src/screens/PrinterCompactRow.tsx`
- Create: `src/screens/PrinterCompactRow.module.css`
- Create: `src/screens/PrinterCompactRow.test.tsx`
- Modify: `src/screens/PrinterDashboard.tsx`
- Modify: `src/screens/PrinterDashboard.module.css`
- Modify: `src/screens/PrinterDashboard.test.tsx`

**Interfaces:**
- Toolbar consumes `MonitorStore` setters/accessors and current preference-save error.
- Card and row both consume `MonitorPrinterView` and emit `onSelect(id)`.
- Dashboard consumes a Monitor store plus existing add/import/export callbacks; it does not derive operational state.

- [ ] **Step 1: Write failing toolbar tests**

Cover search, all six filter controls, available section options (`printerModel`, `operationalState`, `none`), disabled/omitted location, density changes, Add Printer, and preference-save error. Use `pointerdown`/`pointerup` for Kobalte Select interactions.

- [ ] **Step 2: Implement the toolbar**

Use `TextField` for search, `Chip` controls for filters, and `Select` for section/density. Keep Import/Export visually secondary to Add Printer. Do not add the P2 batch-add workflow.

- [ ] **Step 3: Write failing shared-information tests for card and row**

For each density, assert:

- Name and normalized state.
- Host activity or readiness reason.
- Progress only when fresh printing progress exists.
- Current and target temperatures when reported.
- Stale/unavailable age/status.
- Warning/fatal marker with visible label.
- Missing fields display an em dash and never `0 °C`.
- Click and keyboard activation select the Printer.

- [ ] **Step 4: Implement card and compact row from one view model**

Do not duplicate status interpretation. Cards and rows choose layout only. Put Profile drift, overrides, adapter kind, and setup details in the dock rather than badges on every card.

- [ ] **Step 5: Refactor PrinterDashboard into Monitor composition**

Replace internal model grouping/filtering with `monitor.sections()`. Render section headings and `PrinterRoster` counts from section membership. Implement explicit states:

```tsx
<Switch>
  <Match when={loadingWithNoPrinters()}>…loading persisted Printers…</Match>
  <Match when={isFirstRun()}>…Add Printer…</Match>
  <Match when={isFilteredEmpty()}>…Clear search and filters…</Match>
  <Match when={hasPrinters()}>…sections/cards-or-rows…</Match>
</Switch>
```

Preserve active filters in filtered-empty state. Show synchronization uncertainty without replacing content with a spinner.

- [ ] **Step 6: Add responsive card-to-row behavior**

At constrained workspace width, render compact rows before operational fields would be clipped. An explicit user-selected compact density always renders rows. Comfortable remains the persisted default.

- [ ] **Step 7: Run focused tests and build**

```sh
npm test -- src/screens/MonitorToolbar.test.tsx src/screens/PrinterCard.test.tsx src/screens/PrinterCompactRow.test.tsx src/screens/PrinterDashboard.test.tsx
just build
```

Expected: PASS.

- [ ] **Step 8: Commit the Monitor workspace**

```sh
git add src/screens/MonitorToolbar* src/screens/PrinterCard* src/screens/PrinterCompactRow* src/screens/PrinterDashboard*
git commit -m "feat: build Monitor controls and Printer views"
```

### Task 10: Reusable responsive Printer detail dock

**Owner:** Frontend engineer  
**Prerequisites:** Tasks 5 and 7.  
**Can run in parallel with:** Task 8.  
**Handoff:** Task 11 connects navigation/deep links.

**Files:**
- Create: `src/screens/PrinterDetailDock.tsx`
- Create: `src/screens/PrinterDetailDock.module.css`
- Create: `src/screens/PrinterDetailDock.test.tsx`
- Modify: `src/screens/PrinterStatusPanel.tsx`
- Modify: `src/screens/PrinterStatusPanel.module.css`
- Modify: `src/screens/PrinterStatusPanel.test.tsx`
- Modify: `src/screens/PrinterDashboard.tsx`
- Modify: `src/screens/PrinterDashboard.module.css`
- Reuse: `src/screens/PrinterProfilePanel.tsx`
- Reuse: `src/screens/PrinterConnectionPanel.tsx`

**Interfaces:**
- `PrinterDetailDockProps { printer?: ResolvedPrinter; mode: "inline" | "overlay"; onClose(); onRemove(id); }`.
- Dock tabs in P1 are `status` and `setup`; Job and Camera are absent.
- Dashboard determines overlay mode from a container-width breakpoint implementing the 44rem workspace / 15rem comfortable-card rule.

- [ ] **Step 1: Write failing dock tests**

Assert card selection opens detail, close returns focus to the selected card, Status and Setup tabs are keyboard-operable, Job/Camera tabs are absent, overlay has dialog semantics and Escape dismissal, inline mode remains complementary content, and unknown selection renders no dock.

- [ ] **Step 2: Refactor operational Status content**

`PrinterStatusPanel` shows generated operational state, readiness reason, Connection health, host activity, progress/readings, freshness, age, and reconciliation uncertainty. It does not show catalog editing.

- [ ] **Step 3: Compose Setup from existing surfaces**

Move identity/name/notes, Profile, and Connection content under Setup without changing their persistence behavior. Preserve debounce cancellation when selected Printer identity changes.

- [ ] **Step 4: Implement inline/overlay behavior**

Prefer CSS container queries if supported by the current browser target; otherwise use a scoped `ResizeObserver` on the Monitor workspace. At 1024 × 700 the dock must be an overlay; at 1440 × 900 it must remain inline. If the resize handle remains, add keyboard ArrowLeft/ArrowRight support and ARIA value bounds; otherwise remove resizing rather than keep pointer-only behavior. Do not persist width.

- [ ] **Step 5: Run focused tests**

```sh
npm test -- src/screens/PrinterDetailDock.test.tsx src/screens/PrinterStatusPanel.test.tsx src/screens/PrinterDashboard.test.tsx
```

Expected: PASS.

- [ ] **Step 6: Commit the dock slice**

```sh
git add src/screens/PrinterDetailDock* src/screens/PrinterStatusPanel* src/screens/PrinterDashboard* src/screens/PrinterProfilePanel* src/screens/PrinterConnectionPanel*
git commit -m "feat: add responsive Printer detail dock"
```

### Task 11: App integration and end-to-end acceptance

**Owner:** Wiring engineer, then quality engineer  
**Prerequisites:** Tasks 4–10.  
**Handoff:** Code reviewer receives command output and manual verification notes.

**Files:**
- Modify: `src/App.tsx`
- Modify: `src/App.module.css`
- Create: `src/App.test.tsx`
- Modify: `src/screens/PrinterDashboard.test.tsx`
- Modify: `src-tauri/tests/f0_tauri_path.rs` if integration gaps remain
- Create: `docs/verification/2026-09-18-p1-shell-monitor.md`

**Interfaces:**
- App constructs one Monitor store from `printers()` and loaded Settings.
- App passes the same derived snapshot to Shell aggregates and Monitor content.
- Printer deep links select/open the dock after durable Printers load.
- Listener startup/disposal uses an explicit cancellation guard.

- [ ] **Step 1: Write failing App orchestration tests**

Mock the stores and prove this order and behavior:

```ts
expect(callOrder).toEqual([
  "loadSettings",
  "loadPrinters",
  "listen",
  "backfill",
]);
```

Also prove persisted Printers render while status is syncing, a deep-linked Printer opens the dock after load, deleted/unknown selection closes detail but stays on Monitor, listener failure appears as a recoverable banner, and unmount calls a disposer that resolves late.

- [ ] **Step 2: Integrate Monitor, Shell, and navigation**

Rename the title to Monitor. Keep available production destinations to Monitor and Library plus the existing Settings menu. Compute all Shell counts/rosters from `MonitorStore` rather than from a second summary function. Remove `summarizePrinters` once no caller remains.

Use a late-resolution-safe cleanup pattern:

```ts
let disposed = false;
let unlisten: (() => void) | undefined;
void startStatusListener().then((dispose) => {
  if (disposed) dispose();
  else unlisten = dispose;
}).catch(reportStatusStartupError);
onCleanup(() => {
  disposed = true;
  unlisten?.();
});
```

- [ ] **Step 3: Run the complete automated suite**

```sh
just build
just test
source "$HOME/.cargo/env" && just test-rust
```

Expected: all commands exit 0. Record command, date, revision, and result in the verification note.

- [ ] **Step 4: Verify the wide tracer at 1440 × 900**

Run:

```sh
source "$HOME/.cargo/env" && just dev
```

With fixture/test Printers representing ready, printing, offline/stale, setup-incomplete, warning, fatal, and missing fields, verify:

- Rail, Monitor canvas, and inline dock are simultaneously visible.
- Search, every filter, density, and section selector work.
- Counts open bounded rosters by pointer and keyboard.
- Selecting a card/row reuses one dock.
- Stale values retain readings and show age.
- No missing reading appears as zero.

- [ ] **Step 5: Verify the compact tracer at 1024 × 700**

Verify the dock overlays instead of squeezing the workspace, cards become rows before fields clip, primary actions remain reachable, overlays dismiss with Escape, and no control is clipped.

- [ ] **Step 6: Complete keyboard and reduced-motion verification**

Using keyboard only, traverse activity rail → toolbar → Printer content → dock → status bar. Open/close rosters, change filters/selects, select a Printer, switch dock tabs, close the dock, and confirm visible focus. Enable reduced motion and confirm nonessential transitions/indeterminate animation stop.

- [ ] **Step 7: Verify restart and stale-to-live behavior through Tauri**

Observe live telemetry, close the app without relying on a graceful final write, restart, and verify cached values appear stale before the Connection refresh. Confirm the first authoritative observation changes them to fresh without losing the active filter, section, density, or selected Printer.

- [ ] **Step 8: Record acceptance evidence**

In `docs/verification/2026-09-18-p1-shell-monitor.md`, record:

- Git revision and platform.
- Exact automated commands/results.
- Both viewport results.
- Keyboard/reduced-motion results.
- Restart/stale-to-live result.
- Any unavailable display/hardware dependency and the exact unverified criterion.

- [ ] **Step 9: Commit integration and verification evidence**

```sh
git add src/App* src/screens src-tauri/tests/f0_tauri_path.rs docs/verification/2026-09-18-p1-shell-monitor.md
git commit -m "feat: complete P1 Monitor tracer"
```

## Delivery order and parallel work

```text
Task 1 Settings/schema ─┐
                        ├─> Task 3 telemetry repository ─> Task 4 backend vertical slice ─> Task 5 frontend wiring
Task 2 policy ──────────┘                                                        │
                                                                                 ├─> Task 6 Monitor store ─┐
                                                                                 └─> Task 7 UI primitives ─┼─> Task 8 Shell ───────┐
                                                                                                       ├─> Task 9 Monitor UI ──┼─> Task 11 integration
                                                                                                       └─> Task 10 dock ───────┘
```

- Tasks 1 and 2 can run in parallel if they coordinate generated contract registration.
- Tasks 6 and 7 can run in parallel after Task 5.
- Tasks 8 and 10 can run in parallel after Task 7; Task 9 also needs Task 6.
- Task 11 is the integration and quality-engineering gate and begins only after all prior slices merge.

## External dependencies and blockers

- **Moonraker contract:** Existing WebSocket/JSON-RPC behavior is the only implemented adapter. No external team is required, but liveness behavior must be verified against Moonraker protocol behavior and, when available, a real host.
- **Display:** Manual 1440 × 900 and 1024 × 700 verification requires a graphical environment. If unavailable, automated checks may complete but P1 remains partially verified.
- **Printer/host:** A real Moonraker host improves restart/live-transition confidence. The deterministic fake adapter and Tauri integration test are mandatory regardless of hardware availability.
- **P2 handoff:** P2 owns durable location and location editing. P1 must leave the `location` enum value contract-compatible but unavailable in controls until real data exists.
- **P7/P8 handoff:** P7 consumes `PrinterReadiness`; P7/P8 later add authoritative Job/Queue/Attention counts and the Queue dock preview. P1 must not reserve those with misleading zeroes.

## Expected deliverables

1. Approved spec and this implementation plan.
2. Schema-v2 migration for Monitor preferences and telemetry cache.
3. Pure Rust operational/readiness/freshness policy with generated contracts.
4. Restart-safe retained telemetry over the existing event/backfill protocol.
5. Monitor presentation store and shared status/roster primitives.
6. Responsive Shell, cards/rows, controls, required states, and detail dock.
7. Automated Rust/frontend/Tauri-path coverage.
8. Recorded responsive, keyboard, reduced-motion, and stale-to-live verification evidence.
