# P5 Runtime Slicing and Slice Revisions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Stages A and B approved on 2026-09-24; Stage C is ready to start. GitHub issue #15. It was written on
2026-09-24 against `main` at `7b9d82d`, with P4 merged.

The approach doc's P5 blocking gate says a runtime spike must be approved
before the focused implementation plan. This document therefore has three
stages, and each one ends with an approval stop:

| Stage | Output | Stop |
|---|---|---|
| A. Runtime spike (Task 1) | `docs/superpowers/baselines/2026-09-2x-p5-orca-runtime-spike.md` | The user approves the spike report |
| B. Focused spec (Task 2) | `docs/superpowers/specs/2026-09-2x-p5-runtime-slicing-design.md` | The user answers the spec's open questions |
| C. Implementation (Tasks 3–16) | Code, tests, docs, verification record | Exit gate below |

The decisions in §Proposed decisions are this plan's recommendations. The
spike may change an implementation detail. A change in product behavior
needs the user.

**Stage A ran on 2026-09-24.** The report is
`docs/superpowers/baselines/2026-09-24-p5-orca-runtime-spike.md`. The user
approved it on 2026-09-24 and chose option B: a runtime is an engine plus a
JSON-bearing preset source, so cache-only nightlies work. The
spike's changes to P3–P6 are listed in its "Consequences for the plan"
section, and Task 2 folds them into the spec.

**User answers (2026-09-24).** These are fixed:

1. **P1 (obtaining OrcaSlicer):** the user downloaded OrcaSlicer v2.4.2 to
   `~/Downloads/` and did not ask for bundling. The plan therefore keeps the
   recommendation to discover a user-installed OrcaSlicer, with no
   bundling. Task 2 confirms this reading with the user.
2. **P2 (versions):** allow every **2.x.y** release **and nightly or dev
   builds** of 2.x, such as `2.5.0-dev`. Block anything outside major
   version 2.
3. **P9 (renderer):** use three.js with Rust-parsed meshes, as recommended.
4. **P8 (external G-code facts):** nothing is pre-filled. Each fact has a
   **Use the file's value** button, and a fact left empty is recorded as
   `absent`, as recommended.

**Goal:** Turn a Library Model into immutable, inspectable Slice Revisions:

- Prepare one or more build plates in a real 3D viewport.
- Run a supported OrcaSlicer as a cancellable background process, one plate
  per revision.
- Publish the result as an immutable Slice Revision linked to one Model
  Source Revision and one plate.
- Wrap imported G-code as an *external* Slice Revision. Compatibility facts
  must be operator-confirmed and are never inferred.

P5 ends at an inspectable Slice Revision. It creates no Queue data and has no
Dispatch action.

**Architecture:** Rust owns persisted truth and the OrcaSlicer process.

- **Runtime.** A new `src-tauri/src/slicing/` module discovers or accepts a
  user-chosen OrcaSlicer executable, checks its version against an
  allowlist, and resolves flat presets from *that installation's own*
  `resources/profiles`.
- **Input.** For each plate it writes a per-operation work directory with a
  farm3d-authored geometry-only 3MF and the flat machine, process, and
  filament JSON.
- **Process.** Rust spawns OrcaSlicer in its own process group. It reads
  progress from a FIFO, captures a bounded log, and validates the output
  with the P4 G-code inspector.
- **Publishing.** The G-code goes into the P4 content store. The immutable
  `slice_revisions` row is committed in the same transaction.
- **Events.** A `slicing` event stream uses the same listen-before-backfill
  contract as `library`.
- **Frontend.** A new `src/slicing/` holds the store, pure transform,
  arrange, and lay-flat modules, and a three.js viewport behind a small
  renderer interface. The preparation workspace replaces `BuildPlate`.

**Tech stack:** Rust, Tauri 2.11, rusqlite/SQLite, tokio (the `process`
feature is added), `nix` or `rustix` for process groups on Unix,
`zip` 8.6 (a writer is added for plate 3MFs), `quick-xml`, `sha2`, ts-rs,
SolidJS, TypeScript, Kobalte, CSS Modules, three.js, Vitest,
playwright-core (ad hoc, as in P4), and Rust unit and integration tests.

## Evidence gathered while planning

These facts were checked on 2026-09-24. They are recorded here so the spike
starts from evidence instead of assumptions. They are not the spike itself.

**OrcaSlicer releases.** `v2.4.2` is the latest stable release
(2026-07-07). It matches the catalog's pinned tag (`gen-catalog
tag="v2.4.2"`). Release assets:

- Linux: an AppImage and a Flatpak for x86_64 and aarch64.
- macOS: a universal `.dmg`.
- Windows: an installer and a portable zip for x64 and arm64.

**CLI surface at `v2.4.2`** (`src/libslic3r/PrintConfig.cpp`,
`CLIActionsConfigDef`, `CLITransformConfigDef`, and `CLIMiscConfigDef`):

- **Slicing and output:**
  - `--slice <n>`: 0 means all plates, *i* means plate *i*.
  - `--outputdir`, `--datadir`.
  - `--export-3mf`, `--min-save`.
- **Presets:**
  - `--load-settings "machine.json;process.json"`.
  - `--load-filaments "f1.json;..."`.
- **Placement:** `--arrange 0|1`, `--orient 0|1`, `--ensure-on-bed`.
- **Progress:** `--pipe <name>`.
- **Limits:** `--mstpp` (maximum slicing seconds per plate) and `--mtcpp`
  (maximum triangles per plate).
- **Checks:** `--no-check`, `--normative-check`.
- **Logging:** `--debug <0-5>`.

**Presets are not resolved by the CLI.** `--load-settings` and
`--load-filaments` go through `load_from_json` as flat configs. The CLI
reads the `inherits` key only as a system-preset *name*; it does not walk
the chain (`src/OrcaSlicer.cpp` around lines 1965–2201). farm3d must pass
fully flattened presets. The bundled catalog (ADR-0007) is not enough to
build them, as the approach doc already warns.

**Progress and result are Linux-only.** Both `cli_callback_mgr_t` (the
`--pipe` writer) and `record_exit_reson` (which writes
`<outputdir>/result.json`) sit inside `#if defined(__linux__)`. On Windows
and macOS the only signals are:

- the process exit code, which is the `CLI_*` code;
- stdout and stderr;
- the output files.

**Linux probe.** The run used the 2.5.0-dev nightly AppImage already on the
host, headless, with `DISPLAY` and `WAYLAND_DISPLAY` unset:

```sh
OrcaSlicer.AppImage --datadir <empty> --outputdir <out> --pipe <fifo> \
  --slice 0 src-tauri/tests/fixtures/library/orca-two-plates.3mf
```

The results:

- The run exited 0 in 0.33 s. It wrote `plate_1.gcode`, `plate_2.gcode`, and
  `result.json`. `result.json` held `return_code`, `error_string`,
  `sliced_plates[].id`, and `warning_message`.
- The FIFO received one JSON object per line, for example:
  `{"message":"Generating walls","plate_count":2,"plate_index":1,"plate_percent":15,"total_percent":9}`.
  `total_percent` is monotonic.
- The log contained `Error: unable to open display`, which did not affect
  the result.
- `--slice 2` on its own wrote only `plate_2.gcode`.
- A re-run was byte-identical except for line 2:
  `; generated by OrcaSlicer 2.5.0-dev on <date> at <time>`.
- The output G-code contains `; filament used [g] = 0.73`,
  `; estimated printing time (normal mode) = 3m 42s`, and
  `; nozzle_diameter = 0.4`, which the P4 claim allowlist already
  extracts.

The probe used the 3MF's own embedded settings. It did **not** test
external presets, a farm3d-authored 3MF, v2.4.2 itself, cancellation, or
the Flatpak. Those are spike gates.

**F0 platform scope.** `2026-09-16-supported-platforms.md` declares only
**Linux x86_64** as Supported. The exit gate "packaged/installed OrcaSlicer
executes on every F0-declared supported platform" therefore means Linux
x86_64. Windows and macOS stay "Candidate, unverified". The process layer
must still compile on all targets, and the Linux-only progress path must
degrade cleanly.

## Proposed decisions

The Task 2 spec makes these final. **(User)** marks a decision that is the
user's to make; the recommendation comes first.

### P1. How farm3d obtains OrcaSlicer (answered: discover, no bundling — Task 2 confirms)

**Recommendation:** farm3d uses an OrcaSlicer the user has installed, and
does not bundle it in v1.

- **Discovery order:**
  1. An executable the user picked in Settings. It is stored as a setting
     and chosen through a Rust-owned native picker, so no raw path comes
     from the frontend (P4 D7).
  2. `orca-slicer` on `PATH`.
  3. Well-known install locations per platform (the spike finalizes these).
  4. On Linux, the Flatpak — only if the Flatpak gate passes.
- **Version check:** read from the first `--help` line
  (`OrcaSlicer-<version>:`).
- **Why not bundle:**
  - The AppImage adds about 150 MB or more to every package.
  - Each platform needs its own asset.
  - Shipping AGPL binaries brings source-offer obligations that the
    separate-process design (ADR-0003) otherwise keeps out of farm3d's
    distribution.
- **Alternative:** bundle a pinned OrcaSlicer as a Tauri `externalBin`
  sidecar. That is more predictable, but it enlarges packaging and needs a
  licensing decision and ADR.

### P2. Supported versions (decided by the user: 2.x and nightlies)

**Accepted:** any version whose `--help` first line parses as
`OrcaSlicer-2.<minor>.<patch>[-<suffix>]:`. Stable releases and nightly or
dev builds such as `2.5.0-dev` are both accepted.

**Refused:** anything with a major version other than 2, or a version that
fails to parse. The state is `unsupportedVersion` and slicing is blocked
with an explanation.

**Reference versions.** v2.4.2 is the reference release:

- The deterministic invocation fixtures and `just test-orca` default to it.
- The spike and `just test-orca` also run against the host's 2.5.0-dev
  nightly.
- Other 2.x versions are accepted without per-version evidence.

**Recording.** Every farm3d Slice Revision records the exact
`runtimeVersion` string and a `runtimeChannel` of `release` or `prerelease`
(prerelease when the version has a suffix). The review shows the channel,
so a slice made with a nightly is visible as one.

**Consequence.** Because accepted versions have no individual evidence,
preset-key compatibility is checked on every run, not assumed:

- The flat-preset resolver reads the *installed* profiles.
- A key in the mapping table that the installed presets don't know fails
  with `UNSUPPORTED_SETTING_FOR_RUNTIME` and names the key.
- Output validation (P6) is the final guard.

**Verified evidence (2026-09-24).**
`~/Downloads/OrcaSlicer_Linux_AppImage_Ubuntu2404_V2.4.2.AppImage`:

- 137,759,224 bytes.
- SHA-256 `d12fb8c8eac1aecd2dfb6377acd48f994f8fa439ed5292fa532dd82880f029fd`,
  which matches the GitHub release asset digest.
- Headless `--help` exits 0 and prints `OrcaSlicer-2.4.2:` on stdout. The
  line `Error: unable to open display` goes to **stderr**, so the version
  probe must read stdout only.

### P3. Where presets come from

- **Machine preset.** farm3d resolves it from the discovered installation's
  `resources/profiles/<Vendor>/machine/*.json`. The existing ingest resolver
  (`catalog/ingest/inherits.rs`, `resolve_machine_preset`) is generalized to
  resolve the `inherits` chains of machine, process, and filament presets.
- **Selection.**
  - Machine: `CatalogRef.printer_variant` selects the preset.
  - Process ("quality") presets: offered only when their
    `compatible_printers` names that machine.
  - Filament presets: offered by `filament_type`, mapped from `MaterialFamily`.
- **Where the flat JSON goes.** Only into the per-operation work directory,
  and as content blobs in the user's local store (see P6). It is never
  committed, bundled, or copied into the catalog, so ADR-0007 is unchanged.
  This is local use of the user's own installation (ADR-0004), not
  distribution.
- **Printer overrides.** A Printer Profile override (for example, bed size
  or nozzle) is applied to the flat machine JSON only through an explicit
  key map. `bed_shape` maps to `printable_area`, `printable_height_mm` to
  `printable_height`, `nozzle_diameter_mm` to `nozzle_diameter`, and so on.
  An override with no mapping blocks slicing with `UNMAPPED_PROFILE_OVERRIDE`.
  farm3d never silently drops an override.

### P4. Invocation shape

**One process per plate.** Each process slices one plate, which makes plate
identity and cancellation trivial.

**Input.** A farm3d-authored geometry-only 3MF holding just that plate's
objects. Build-item transforms carry the prepared placement. The 3MF uses
core 2015/02 with one object per mesh, and farm3d writes no Orca metadata
unless the spike shows it is required.

**Arguments:**

```text
--datadir <work>/datadir        (empty, per operation; never the user's Orca profile)
--outputdir <work>/out
--load-settings "<work>/machine.json;<work>/process.json"
--load-filaments "<work>/filament.json"
--arrange 0 --orient 0          (farm3d owns placement)
--slice 1
--pipe <work>/progress.fifo     (Linux only)
<work>/plate.3mf
```

**Environment.** `DISPLAY` and `WAYLAND_DISPLAY` are removed on Linux. The
working directory is `<work>`, and the environment is minimal.

**Fallback.** If Gate C fails, farm3d re-emits the plate with an Orca-style
`model_settings.config` plate layout, modeled on `orca-two-plates.3mf`,
instead of a bare core 3MF.

### P5. Supervision, progress, and cancellation

- **Unix.** The process is spawned with `setsid` (its own process group).
  Cancelling sends SIGTERM to the group, waits up to 5 s, then sends
  SIGKILL. Because an AppImage re-executes through a runtime wrapper,
  signalling the group is required.
- **Windows.** A Job Object with kill-on-close. This compiles and is
  unit-tested, but runtime proof is out of scope because the platform is
  unverified.
- **Progress.**
  - Linux: the FIFO's JSON lines are parsed and throttled to 250 ms, like
    P4's `ProgressThrottle`.
  - Other platforms: indeterminate progress plus stage messages from the
    log.
- **Log.** A bounded ring of 4 MiB. Absolute paths under the work
  directory are rewritten to `<work>/…` before the log is stored or
  emitted.
- **Concurrency.** One OrcaSlicer process at a time. Other plates wait in a
  FIFO order that lasts only while the app runs.
- **Restart.** The operation row stores the child's pid and start time. At
  startup:
  - Any `running` or `queued` operation becomes `interrupted`.
  - A surviving child whose pid and start time still match is killed.
  - Every `<content_root>/slicing-work/*` directory is removed.
  - Published revisions and their blobs are never touched.

### P6. Output validation and publishing

**Success requires all of the following:**

- The exit code is 0, and on Linux `result.json` has `return_code` 0.
- Exactly `out/plate_1.gcode` exists and is 1 GiB or smaller.
- The P4 G-code inspector accepts it with `producer.name == "OrcaSlicer"`
  and `command_count > 0`.
- `observedBoundsMm`, when present, lies within the target build volume
  plus a 0.5 mm tolerance.

**Estimates.** Time, filament grams and millimetres, and layer count are
parsed from our own output into typed values. The revision labels them
`source: "farm3dSlice"`.

**Publishing.** The G-code, the plate 3MF, the flat presets, the
invocation manifest (JSON), and the log are staged. `place_and_commit` then
inserts the blobs and the `slice_revisions` row in one transaction.

**Failure.** A failed or cancelled operation publishes nothing except its
log blob, which the operation row references.

**Deterministic fixtures.** They normalize only the second line,
`; generated by … on <date> at <time>`, before comparing hashes.

### P7. Persistence (migration `0006_p5_slicing.sql`)

**`slice_preparations`: mutable draft state.** Columns:

- `id` (`prp-*`), `model_id`, `source_revision_id`, `revision`.
- `state_json`: the plates with a stable `plateKey` (UUID), name, and
  order, and the object instances with transforms.
- `target_json`: the Printer Profile ref, machine, process, and filament
  preset ids, and the overrides.
- `created_at`, `updated_at`.

A preparation is **stale** when `source_revision_id` is not the Model's
current revision. That is derived, never stored.

**`slice_operations`: durable operation log.** Columns:

- `id` (`sop-*`), `preparation_id`, `plate_key`, and a `plate_snapshot_json`
  that freezes the plate at start.
- `state`: `queued`, `running`, `succeeded`, `failed`, `cancelled`, or
  `interrupted`.
- `pid`, `pid_started_at`, `started_at`, `finished_at`.
- `error_code`, `error_detail`, `log_sha256`, `slice_revision_id`.

**`slice_revisions`: immutable.** Both a `BEFORE UPDATE` and a
`BEFORE DELETE` trigger raise errors, following P3's append-only
precedent. Columns:

- `id` (`slr-*`) and `kind`, which is `'farm3d'` or `'external'`.
- `model_id`: `REFERENCES library_models ON DELETE RESTRICT`.
- `source_revision_id`: `REFERENCES model_source_revisions ON DELETE RESTRICT`.
- Plate identity: `plate_key`, `plate_index`, and `plate_name`. They are
  required when `kind='farm3d'` and must be NULL when `kind='external'`,
  enforced by a CHECK.
- `gcode_sha256`, `gcode_size`.
- `target_json`: the snapshot of the Printer Profile, machine, process, and
  filament names, and the overrides.
- `facts_json`: per fact, `{ value | null, provenance: "farm3dInput" |
  "operatorConfirmed" | "absent" }` for `printerProfile`,
  `nozzleDiameterMm`, and `materialFamily`.
- `requires_manual_printer_selection`: generated, true when any fact is
  `absent`.
- `estimates_json`.
- `invocation_sha256`, `runtime_version`: NULL for external revisions.
- `created_at`.

**Blob references.** `mark_unreferenced_blobs` and `startup_sweep` must
count references from `slice_revisions`, and from the invocation, preset,
log, and plate blobs through a `slice_revision_blobs(revision_id, role,
sha256)` table.

**Model deletion.** `delete_model` is refused with
`REFERENCED_BY_SLICE_REVISION` while any revision exists. The P4
`DeleteModelDialog` copy already anticipates this. Deleting revisions is
P9's pruning work.

### P8. External Slice Revisions from imported G-code

`create_external_slice_revision` takes:

- `sourceRevisionId`: a G-code Model Source Revision.
- `operationId`.
- `facts`, where each fact is either `{ kind: "confirmed", value }` or
  `{ kind: "absent" }`.

The revision rules:

- It reuses the source revision's blob (the same `gcode_sha256`) and stores
  no copy.
- It gets no plate identity and no invocation.
- It keeps the file's claims only as `claimedEstimates` with
  `trusted: false`.

**Confirmation UI.** The dialog shows each claim beside the empty confirmed
field. A per-fact **Use the file's value** button fills the field and
labels it "Confirmed by you". Nothing is pre-filled. Leaving a fact empty
records `absent`, and the review shows **Needs manual Printer selection**.

**Repeat creation.** Creating another external revision from the same
source revision is allowed, because facts may differ. Each one has its own
`operationId`, so a retry is idempotent.

### P9. Renderer (decided by the user: three.js)

- **Choice.** Plain `three`, imperative, owned by one Solid component. No
  `solid-three` or react-style binding.
- **Mesh data.** Geometry comes from Rust through `get_revision_mesh` as a
  binary `tauri::ipc::Response` of positions and indices per object. That
  way the viewport and the slicer see the same parse from the P4 readers,
  and no JS STL or 3MF loader is needed. For web mode, mesh fixtures are
  generated from the library fixtures.
- **The renderer interface** (`src/slicing/viewport/renderer.ts`) is an
  interface that jsdom tests replace with a fake:
  - `mount`, `setScene`, `setCamera`, `pick`, `dispose`.
  - `setOverlays`: build volume, out-of-bounds tint, exclude areas, and
    measure line.
- **Visual checks.** Real rendering is verified by playwright-core
  screenshots in `just web` and by hand in `just dev`.
- **Accessibility.**
  - The canvas has `role="img"` with a live text description: object
    count, selection, dimensions, and out-of-bounds objects.
  - Every tool is a toolbar button with a keyboard shortcut and numeric
    fields.
  - Pointer manipulation is never required.
  - `prefers-reduced-motion` disables camera tweening.

### P10. Plate preparation tools and placement

These are the umbrella spec's tools. They are implemented as pure,
unit-tested TypeScript over preparation state:

- **Camera:** orbit, pan, zoom, reset, and the standard views (top, front,
  side, iso).
- **Select, move, rotate, scale:** numeric fields, arrow keys with a
  1 mm or 1° step, Shift for a 10 mm or 15° step, and uniform or per-axis
  scale.
- **Lay-flat:** the face is picked by pointer, or by keyboard from a list of
  the largest convex-hull faces.
- **Arrange:** deterministic footprint packing within the bed shape, minus
  exclude areas.
- **Measure:** point to point, plus the bounding dimensions.
- **Plates:** plate tabs to add, rename, reorder, and delete a plate, and
  a **Move to plate** command.

**Placement stays in farm3d.** OrcaSlicer runs with `--arrange 0
--orient 0`, so the revision matches what the user saw.

**Where plates come from.** 3MF plates from `ThreeMfInspection.plates`
seed the preparation. STL starts with one plate.

### P11. Slicing controls (the explicit v1 mapping)

**Controls:**

- Target Printer Profile, with the number of matching Farm Printers.
- Material (filament preset).
- Quality (process preset).

**Overrides.** Only this allowlist, each mapped to one Orca key, which the
spike verifies:

| Control | Orca key |
|---|---|
| Infill density | `sparse_infill_density` |
| Infill pattern | `sparse_infill_pattern` |
| Walls | `wall_loops` |
| Top and bottom shells | `top_shell_layers`, `bottom_shell_layers` |
| Supports | `enable_support`, `support_type` |
| Support threshold angle | `support_threshold_angle` |
| Adhesion | `brim_type`, `brim_width`, `skirt_loops` |
| Layer height | `layer_height` |

Advanced Overrides stays under 12 fields, so it has no search in P5.

**Validation runs before a slice starts:**

- Geometry is present.
- All objects are inside the build volume and clear of exclude areas.
- The preset nozzle equals the profile nozzle.
- The filament is compatible with the machine.
- The runtime is available and verified.
- The preparation is not stale.

### P12. Queue handoff intent

The Slice Revision review shows **Add to Queue…** as a disabled button with
the visible reason "The Queue arrives in a later version." It creates no
Queue data, and no navigation target is added. This follows the approach
doc's "does not ship a fake Dispatch action".

## Global constraints

**Carried over from P4:**

- **Contract registration.** Every new command goes in:
  - `lib.rs` `COMMAND_NAMES` and `generate_handler!`.
  - `contracts/inventory.rs` (`COMMAND_CONTRACTS`, its array length, and
    the `CommandContracts` declaration and visitor).
  - `tests/export_contracts.rs`.
  - `src/ipc/client.ts` `CommandMap`.

  Count assertions are updated by adding P5's command count to whatever
  `main` has; never hard-code a total.
- **Schema version.** The migration is `0006_p5_slicing.sql` if 0005 is
  still the highest, and `CURRENT_SCHEMA_VERSION` matches it. Tests read the
  constant.
- **Generated contracts.** Never hand-edit `src/generated/contracts/**`. Run
  `just gen-contracts`.
- **Paths.** No command accepts a raw filesystem path from the frontend.
  Errors, events, and logs carry basenames and ids.
- **Frontend conventions.** Kobalte primitives, CSS Modules, and `--f3d-*`
  tokens only, in the editor aesthetic. Kobalte Select and Menu tests use
  `pointerDown` and `pointerUp`.
- **Events.** Emitted after commit only. Slicing events are on the
  `slicing` stream (type prefix `slicing.`), filtered like `library.`.

**P5-specific:**

- A Slice Revision is never updated or deleted. Tests assert that the
  triggers fire.
- No Queue, Job, or dispatch data, table, or command exists.
- An external revision never gets a fact whose provenance is not
  `operatorConfirmed` or `absent`. A property test asserts that
  `farm3dInput` is impossible for `kind='external'`.
- **Real-OrcaSlicer tests** are `#[ignore]` and run when
  `FARM3D_ORCA=<path>` is set. CI runs the **fake-orca** harness (Task 6)
  instead. Every behavior test that CI relies on must pass against
  fake-orca.
- **New `just` recipes:**
  - `test-orca`: runs the ignored real-Orca tests with `FARM3D_ORCA`.
  - `gen-slicing-fixtures`: regenerates the deterministic invocation
    fixtures.

## File and module map

### Backend (`src-tauri/src/slicing/`)

| File | Responsibility |
|---|---|
| `mod.rs` | `SlicingServices<R>` (runtime, operation queue, stream), wired into `RuntimeServices` |
| `runtime.rs` | Discovery, the `--help` version probe, the version allowlist, and the `SlicerRuntime` status |
| `presets.rs` | Resolving flat machine, process, and filament presets from the install's `resources/profiles`; listing compatible options |
| `mapping.rs` | Printer Profile override → Orca keys, and slicing controls → Orca keys (explicit tables) |
| `plate3mf.rs` | Writes a deterministic geometry-only 3MF for one plate from a source revision and transforms |
| `mesh.rs` | Mesh extraction for `get_revision_mesh` (reuses the P4 STL and 3MF readers) |
| `process.rs` | Spawn, process group or Job Object, FIFO progress, log ring, cancel, timeout, exit-code map |
| `publish.rs` | Output validation, estimates, staging, and `place_and_commit` of the revision |
| `external.rs` | External Slice Revision creation |
| `operations.rs` | Operation state machine, FIFO scheduler, restart recovery, work-dir sweep |
| `repository.rs` | SQL for preparations, operations, and revisions |
| `events.rs` | `SlicingStream`, event types, and backfill |
| `commands.rs` | Tauri commands |
| `bin/fake-orca.rs` (test-only) | A CLI double that honours the real flags, writes the FIFO and `result.json`, and fails, hangs, or produces bad output on command |

Also modified: `catalog/ingest/inherits.rs` (generalized),
`library/content.rs` (reference counting), `library/commands.rs`
(`delete_model` guard), `settings/` (the slicer executable setting),
`lib.rs`, `contracts/inventory.rs`, and `Cargo.toml` (tokio `process`,
`nix`/`rustix`, `windows-sys` Job Object features, zip writer features).

### Frontend (`src/slicing/`, `src/screens/`)

| File | Responsibility |
|---|---|
| `src/slicing/slicing-store.ts` (+ `-mock.ts`, `web-fixtures.ts`) | Preparations, operations, revisions, and runtime status; sequenced stream |
| `src/slicing/transforms.ts`, `arrange.ts`, `layflat.ts`, `bounds.ts`, `validation.ts` | Pure logic with unit tests |
| `src/slicing/viewport/renderer.ts`, `three-renderer.ts`, `fake-renderer.ts` | The renderer interface, the three.js implementation, and the test double |
| `src/screens/PlateViewport.tsx` | Replaces `BuildPlate`; hosts the canvas, toolbar, and text description |
| `src/screens/PreparationWorkspace.tsx` | Plate tabs, viewport, and the object list with numeric transform fields |
| `src/screens/PreparationPanel.tsx` | Target, material, and quality; Strength, Support, and Advanced sections; validation; Slice |
| `src/screens/SliceOperationPanel.tsx` | Progress, Cancel, collapsed log that expands on failure |
| `src/screens/SliceRevisionReview.tsx` | Estimates, facts with provenance, the disabled queue-handoff intent |
| `src/screens/GcodeFactsDialog.tsx` | The external-revision fact confirmation |
| `src/screens/SlicerRuntimeSettings.tsx` | Runtime status, Choose executable…, unsupported-version explanation |

Also modified: `ModelDetailsPanel.tsx` (it adds **Prepare…**, the
**Slice Revisions** section, and **Create Slice Revision…** for G-code, and
removes `BuildPlate`), `LibraryWorkspace.tsx`, `App.tsx` (startup), and
`src/ipc/client.ts`. `BuildPlate.tsx` and its CSS are deleted.

### Docs

- ADR-0009, "OrcaSlicer runtime contract": discovery, version policy,
  preset source, invocation, and the Linux-only progress path.
- ADR-0010, "three.js renderer with Rust-parsed meshes".
- `CONTEXT.md`: refine **Slice** (no longer "not yet implemented") and
  **Slice Revision**. Add **Plate**, **Preparation**, **External Slice
  Revision**, **Confirmed fact**, and **Slicer runtime**.
- The approach doc: close the "Geometry renderer" and "OrcaSlicer runtime
  contract" known-unknowns rows.
- `docs/verification/2026-09-2x-p5-runtime-slicing.md` and
  `docs/screenshots/p5-*.png`.

## Tasks

### Task 1: Runtime spike (Stage A)

**Owner:** Protocol/Backend (OrcaSlicer). Frontend handles Gate H.
**Prerequisites:** none.
**Output:** the spike report, with one row per gate (PASS, FAIL, or
Unavailable), its evidence, and the decision taken. Scratch code goes in a
throwaway crate outside the repo, as in P4.

Gates B to G run against **both** the v2.4.2 AppImage
(`~/Downloads/OrcaSlicer_Linux_AppImage_Ubuntu2404_V2.4.2.AppImage`) and the
2.5.0-dev nightly (`~/.local/bin/OrcaSlicer_Linux_AppImage_Ubuntu2404_nightly.AppImage`).
The report notes every difference between the two, since P2 accepts both.

- [ ] **Gate A — Version and headless start.** The v2.4.2 AppImage is
  downloaded and its hash verified (see P2).
  - Confirm the version parses for both AppImages, including the `-dev`
    suffix.
  - Confirm the parser refuses a synthetic `OrcaSlicer-3.0.0:` and a
    garbage line.
  - Repeat with `--appimage-extract-and-run` and with FUSE unavailable.
  - Record the start time and the display-related stderr noise.
- [ ] **Gate B — Flat presets.** Use a generalized resolver to flatten one
  machine, process, and filament preset from the v2.4.2 install's
  `resources/profiles`. Do this for the Elegoo Centauri Carbon 0.4 and the
  Prusa MK4 0.4 (the web-fixture Printers).
  - Slice `cube-for-slicers.stl` with `--load-settings` and
    `--load-filaments`.
  - The output header must show the chosen machine, process, and filament
    names and the nozzle.
  - Record which keys (`from`, `inherits`, `compatible_printers`,
    `version`) the CLI requires or rejects.
- [ ] **Gate C — farm3d-authored plate 3MF.** Hand-write a core 3MF with
  two objects and non-identity build transforms. Slice it with
  `--arrange 0 --orient 0 --slice 1`.
  - Confirm the G-code places the objects where the transforms put them.
    Compare `observedBoundsMm` with the expected footprint.
  - If this fails, try the Orca `model_settings.config` layout (the P4
    fallback).
- [ ] **Gate D — Two-plate identity.** Run the per-plate invocation twice,
  once per plate of `orca-two-plates.3mf` re-emitted by the Gate C writer.
  Each run must give exactly one `plate_1.gcode`. Compare against
  `--slice 0` on the original.
- [ ] **Gate E — Progress, cancel, and orphans.**
  - FIFO behaviour when farm3d opens the read end first, and when the
    reader is slow.
  - SIGTERM to the process group mid-slice on a large mesh (generate a
    subdivided sphere with about 2 M triangles). Measure the time to exit
    and whether any child survives. Repeat with SIGKILL.
  - Kill the parent farm3d-like process and observe whether OrcaSlicer is
    orphaned.
  - Measure what `--mstpp` does.
- [ ] **Gate F — Failure mapping.** Force a representative set of failures
  and record the exit code, the `result.json` contents, and the stderr
  lines:
  - an object partly outside the bed (`CLI_OBJECTS_PARTLY_INSIDE`);
  - an invalid preset JSON;
  - a missing input;
  - an empty plate;
  - a filament preset incompatible with the machine.
- [ ] **Gate G — Determinism.** Two runs with identical inputs give G-code
  that is identical apart from line 2. Identify any other variable content,
  such as thumbnails or config dumps that embed paths.
- [ ] **Gate H — Renderer in WebKitGTK.** In `just dev`, a throwaway
  three.js canvas renders a 1 M-triangle mesh. Record whether WebGL2 is
  available, the frame time, and memory. Also test `get_revision_mesh`-style
  binary IPC throughput with a 50 MB buffer.
- [ ] **Gate I — Flatpak (optional).** Test whether `flatpak run
  --filesystem=<work>` can read the inputs and write the outputs. If not,
  the Flatpak is recorded as unsupported.
- [ ] **Gate J — Installed package.** From an installed farm3d `.deb` or
  Arch package, spawn the v2.4.2 AppImage from inside the app's environment,
  so that `APPIMAGE` and `LD_LIBRARY_PATH` leakage is checked. If farm3d
  itself is run as an AppImage, its environment variables must be scrubbed
  before OrcaSlicer is spawned.
- [ ] **Write the report** and stop for approval.

### Task 2: Focused spec (Stage B)

**Written 2026-09-24:**
`docs/superpowers/specs/2026-09-24-p5-runtime-slicing-design.md`. The user
approved it on 2026-09-24 and chose the recommendations: no bundling,
deletable-when-unreferenced Slice Revisions, and preparation inside the
Library workspace. Stage C (Tasks 3–16) may start.

**Where this plan and the spec disagree, the spec wins.** The spec changes
Tasks 3–16 in these ways:

- **Task 3** also creates `slicer_runtime_config` and
  `slice_revision_blobs`. It uses the spec's D14 SQL: an UPDATE-only
  trigger, with deletion guarded by a blocker registry (open question 2).
  It registers P4's `ModelDeletionBlocker` with
  `LifecycleBlockerCode::SliceRevisionsExist`.
- **Task 4** covers spec D2–D4:
  - engine discovery;
  - the preset source, including AppImage profile extraction cached by
    hash;
  - `presetsUnreadable` for cache-only builds;
  - the known-key check across the preset index;
  - the filament compatibility check.
- **Task 5** adds `get_revision_geometry` with lay-flat faces (D6), and
  `hull.rs`.
- **Task 6:**
  - explicit environment allowlist (D8);
  - FIFO opened before spawn;
  - PDEATHSIG and `setsid`;
  - SIGTERM, then 5 s, then SIGKILL, plus the stale-mount check;
  - 30-minute timeout, and no `--mstpp`;
  - fake-orca scenarios as in D23.
- **Task 7:** validation adds success-without-output and the preset-name
  claim check (D11).
- **Task 8** implements the spec's 20 commands and the D17 events.
- **Task 15** implements the D22 Settings section: engine plus preset
  source.

**Owner:** Wiring. **Prerequisites:** Task 1 approved.

- [ ] Write the P5 design spec in the form of the P4 spec: status, goal,
  scope and non-goals, vocabulary, decisions D1…Dn (finalizing P1–P12 with
  the spike outcomes), backend model, wire types, frontend architecture,
  errors, accessibility, acceptance criteria, and delivery.
- [ ] Record the user answers above in the spec, confirm the P1 reading with the user,
  and record the answers in §Status.
- [ ] Update this plan's tasks if the spike changed an interface. Stop for
  approval.

### Task 3: Schema, domain types, and repository

**Owner:** Backend. **Prerequisites:** Task 2.

- [ ] Add the P7 migration and bump `CURRENT_SCHEMA_VERSION`.
- [ ] Add the ts-rs domain types:
  - `SlicePreparation`, `PlateState`, `ObjectInstance`, `Transform`
  - `SliceOperation`, `SliceOperationState`
  - `SliceRevisionSummary`, `SliceRevisionRecord`
  - `SliceFacts`, `FactProvenance`
  - `SliceEstimates`, `SliceTarget`
- [ ] Add the repository functions, each with tests:
  - create, update (`expectedRevision`), and reload a preparation;
  - insert an operation and change its state;
  - insert and list revisions.
- [ ] **Tests:**
  - An UPDATE or DELETE on `slice_revisions` raises an error.
  - An external revision with a plate is rejected by the CHECK.
  - `delete_model` returns `REFERENCED_BY_SLICE_REVISION`.
  - Blob cleanup does not release a blob a revision references.
  - Migration over a P4-shaped database, and restart.

### Task 4: Runtime discovery, presets, and mapping

**Owner:** Protocol/Backend. **Prerequisites:** Task 3.

- [ ] **Runtime.**
  - `runtime.rs` handles discovery (the P1 order), the version probe with a
    10 s timeout, and the allowlist.
  - `SlicerRuntime { state: "available" | "notFound" | "unsupportedVersion"
    | "probeFailed", version?, executableName, source }`.
  - `pick_slicer_engine` and `pick_preset_source` use the Rust-owned dialog and persist the
    choice in settings.
- [ ] **Presets.**
  - Generalize `resolve_machine_preset` into a preset-kind-agnostic
    resolver. Keep the catalog generator's behaviour byte-identical (the
    snapshot test must still pass).
  - `presets.rs` lists compatible process and filament presets for a
    machine preset.
- [ ] **Mapping.** `mapping.rs` holds the P3 override table and the P11
  control table.
- [ ] **Tests:**
  - Resolution against `tests/fixtures/profiles/TestVendor`, extended with
    process and filament presets.
  - A mapping table test for every allowlisted key.
  - Every `PrinterProfile` field is either mapped or listed as not
    applicable, so a new field fails the test.
  - An unmapped override blocks slicing.
  - Discovery with a fake executable on `PATH`.

### Task 5: Plate 3MF writer and mesh extraction

**Owner:** Backend. **Prerequisites:** Task 3.

- [ ] `plate3mf.rs` writes deterministic bytes: fixed ZIP timestamps, sorted
  entries, and no clock.
- [ ] `mesh.rs` plus the `get_revision_mesh(revisionId)` command return the
  binary buffer format defined in the spec. It is capped at the P4 size
  limits.
- [ ] **Tests:**
  - A round trip: the written 3MF, read back through the P4 3MF inspector,
    gives the expected object count, transforms, and bounds.
  - Golden bytes.
  - Meshes for every STL and 3MF fixture.

### Task 6: Process supervisor and fake-orca

**Owner:** Protocol/Backend. **Prerequisites:** Task 1 (Gates E and F).

- [ ] **`fake-orca`**, a test binary built only for tests, is driven by
  arguments and environment:
  - It emits FIFO progress lines and writes `result.json` plus
    `plate_1.gcode` copied from a fixture.
  - It exits with a chosen `CLI_*` code.
  - It can hang until killed and spawn a grandchild, which tests
    process-group cleanup.
  - It can write malformed or oversized output, or garbage to the FIFO.
- [ ] **`process.rs`:** spawn in a process group, FIFO reader, log ring with
  path redaction, cancel escalation (SIGTERM first), PDEATHSIG, the 30-minute wall-clock timeout,
  and the exit-code-to-`SliceErrorCode` map from Gate F.
- [ ] **Tests against fake-orca:**
  - Success.
  - Each mapped failure.
  - Cancel mid-run: the grandchild is gone within 6 s.
  - Garbage progress lines are ignored.
  - The log is truncated at the cap.
  - No absolute path appears in stored logs.
- [ ] Add a real-Orca `#[ignore]` twin for success and cancel.

### Task 7: Validation, estimates, and publishing

**Owner:** Backend. **Prerequisites:** Tasks 3, 5, and 6.

- [ ] `publish.rs` implements the P6 checks, typed estimates, staging, and
  one `place_and_commit` for the revision, its blobs, and the operation's
  success.
- [ ] **Tests:**
  - Each validation failure yields `failed` with no revision row and no
    orphan blob row.
  - A crash between placement and commit, using `ContentFailurePoint`,
    leaves no revision, and the sweep cleans up.
  - Estimates parse from the `orca-cube.gcode` claims.

### Task 8: Operations, stream, commands, and restart recovery

**Owner:** Wiring. **Prerequisites:** Tasks 4–7.

- [ ] **`operations.rs`:** a FIFO scheduler with one active process,
  `start_slice(preparationId, plateKeys, operationId)` (idempotent), and
  `cancel_slice_operation`. A queued operation cancels without spawning.
- [ ] **Staleness:** a linked-source change marks preparations stale in
  their derived state. `start_slice` on a stale preparation returns
  `PREPARATION_STALE` unless `continueWithSourceRevision` is sent
  explicitly. `reload_preparation` re-bases the preparation onto the
  current revision and keeps the transforms of objects that still exist.
- [ ] **Recovery:** the P5 restart steps run in `build_runtime_services`
  after `startup_sweep` and before commands are served.
- [ ] **`SlicingStream` events:**
  - `slicing.preparation.changed`, `slicing.preparation.removed`
  - `slicing.operation.changed`
  - `slicing.operation.progress` (ephemeral, throttled)
  - `slicing.revision.created`
  - `slicing.runtime.changed`

  `list_slicing` is the backfill, returning `snapshotSequence`.
- [ ] **Commands** (final names come from the spec):
  - `get_slicer_runtime`, `check_slicer_runtime`, `pick_slicer_engine`,
    `pick_preset_source`, `reset_slicer_runtime`, `list_slice_options`
  - `get_revision_mesh`
  - `create_preparation`, `update_preparation`, `reload_preparation`,
    `delete_preparation`
  - `start_slice`, `cancel_slice_operation`
  - `list_slicing`, `list_slice_revisions`, `get_slice_revision`,
    `get_slice_operation_log`
  - `create_external_slice_revision`
- [ ] **Tests** (fake-orca, through `tauri::test` IPC):
  - Two plates give two revisions with distinct `plate_key` values and the
    same `source_revision_id`.
  - Cancel.
  - Failure: the log expands, and no revision is created.
  - Restart mid-run: the operation becomes `interrupted`, the work dir is
    removed, and prior revisions are byte-identical.
  - A stale source is refused, then continued explicitly.
  - Event ordering and backfill.

### Task 9: External Slice Revisions

**Owner:** Backend. **Prerequisites:** Task 8.

- [ ] `external.rs` plus its command implement P8.
- [ ] **Tests:**
  - All three facts absent: `requiresManualPrinterSelection` is true.
  - Each fact confirmed.
  - The claims are never copied into facts. A property test covers every
    G-code fixture.
  - The revision reuses the source blob.
  - A non-G-code source is rejected.
  - An idempotent retry.
  - The revision is unchanged after restart.

### Task 10: Contracts and the frontend slicing store

**Owner:** Wiring. **Prerequisites:** Task 8 (types stable).

- [ ] Run `just gen-contracts`, then update `CommandMap`.
- [ ] Add `slicing-store.ts`, following the library-store pattern:
  sequenced stream, `isSlicingEvent`, settle by revision, and ephemeral
  progress.
- [ ] Add `slicing-store-mock.ts` and web fixtures. The fixtures give:
  - the runtime state `available 2.4.2`;
  - a preparation for `mdl-web-enclosure` with two plates;
  - one succeeded operation and one failed operation, with a log;
  - one farm3d revision and one external revision, for
    `mdl-web-cube-gcode`, with the `materialFamily` fact absent;
  - mesh fixtures.

  In web mode, Slice and Create return `needsDesktop`.
- [ ] Add store tests covering backfill, gaps, progress, and settling.

### Task 11: Renderer and PlateViewport

**Owner:** Frontend. **Prerequisites:** Tasks 1 (Gate H), 2, and 10.

- [ ] Add the `three` dependency and its types.
- [ ] Write the renderer interface and the three.js implementation:
  - camera controls and standard views;
  - build volume from `BedShape` and `printableHeightMm`, with exclude
    areas;
  - out-of-bounds tint;
  - selection outline;
  - picking.

  Colours come from `--f3d-color-*` tokens, read at mount and on theme
  change.
- [ ] **`PlateViewport`:** the canvas, a toolbar with shortcut hints, and
  the text description. Replace `BuildPlate` in `ModelDetailsPanel` as a
  read-only inspector, and delete `BuildPlate`.
- [ ] **Tests:** with the fake renderer, check the toolbar and keyboard
  commands, the description text, and theme updates. Verify the real
  renderer by screenshot.

### Task 12: Preparation tools

**Owner:** Frontend. **Prerequisites:** Task 11.

- [ ] Write `transforms.ts`, `layflat.ts`, `arrange.ts`, `bounds.ts`, and
  `validation.ts` as pure modules with thorough unit tests:
  - Transform composition.
  - Lay-flat on the cube and a tilted fixture.
  - Arrange is deterministic, stays inside the bed, and avoids exclude
    areas.
  - Out-of-bounds detection on a polygon bed.
- [ ] Add the `PreparationWorkspace` pieces:
  - plate tabs (Kobalte `Tabs`, plus add, rename, reorder, and delete);
  - the object list;
  - numeric transform fields (`NumberField`);
  - keyboard commands;
  - measure;
  - Move to plate.

  Edits debounce through `update_preparation` with `expectedRevision`, and
  a `CONFLICT` resyncs.
- [ ] Show a stale-preparation banner with **Reload onto revision N** and
  **Continue with revision M**.

### Task 13: Preparation panel and operation UI

**Owner:** Frontend. **Prerequisites:** Task 12.

- [ ] **Controls:**
  - Target Printer Profile, using a `PrinterRoster` count.
  - Material and quality Selects fed by `list_slice_options`.
  - Strength and Support sections.
  - A collapsed Advanced section.
- [ ] **Validation:** a list whose rows each link to the offending object
  or field.
- [ ] **Slice controls:** **Slice plate** and **Slice all plates**.
- [ ] **`SliceOperationPanel`:**
  - A `Progress` bar that is determinate on Linux and indeterminate
    elsewhere.
  - **Cancel**.
  - A log that stays collapsed on success, expands and receives focus on
    failure, and has a copy button.
- [ ] **Runtime missing:** show the reason in the panel and link to
  `SlicerRuntimeSettings`.

### Task 14: Revision review, G-code facts, and queue intent

**Owner:** Frontend. **Prerequisites:** Tasks 9 and 13.

- [ ] **`SliceRevisionReview`:**
  - Plate, source revision (sequence), target snapshot, and estimates.
  - Facts with provenance badges. The badges use shape and label as well as
    colour: "From farm3d settings", "Confirmed by you", "Not provided".
  - The disabled **Add to Queue…** from P12.
  - A read-only log.
- [ ] **`GcodeFactsDialog`** implements P8, with claims shown under "What
  the file says (not verified)".
- [ ] **`ModelDetailsPanel`:**
  - A **Slice Revisions** list, shown as a Timeline.
  - **Prepare…** for STL and 3MF.
  - **Create Slice Revision…** for G-code. Replace the P4 note "It can be
    sent to a Printer once G-code handoff is available."
- [ ] **Tests:**
  - Absent and confirmed facts render differently in text, not only in
    colour.
  - The disabled queue intent is announced with its reason.
  - A keyboard-only run through the dialog.

### Task 15: Settings integration

**Owner:** Frontend and Backend. **Prerequisites:** Task 4.

- [ ] Add a **Slicer** section in Settings: status, version, source, the
  **Choose executable…** button, a **Check again** button, and help text
  for an unsupported version, and a "prerelease" label for nightly or dev builds. Emit `slicing.runtime.changed` when it
  changes.

### Task 16: Tracer, docs, packaging, and verification

**Owner:** Wiring. A Verification owner then does a fresh pass.
**Prerequisites:** all.

- [ ] **Automated tracer** (`src-tauri/tests/p5_tracer.rs`), run once
  against fake-orca in CI and once against real v2.4.2 under `just
  test-orca`:
  1. Import `orca-two-plates.3mf` (managed) and `orca-cube.gcode`.
  2. Create a preparation, which is seeded with 2 plates. Pick a target.
     Slice all plates. Assert two revisions with distinct plate keys and
     the same source revision.
  3. Create an external revision from the G-code with the nozzle
     confirmed and the Printer and material absent. Assert that
     `requiresManualPrinterSelection` is true.
  4. Record every revision's record JSON and `open_verified` hashes.
  5. Restart (rebuild `RuntimeServices` over the same roots). Everything is
     byte-identical, and there are no operations in the `running` state.
  6. Start a slice, then restart mid-run. Assert the operation is
     `interrupted`, the work dir is gone, and the step-4 records are
     unchanged.
- [ ] **Deterministic invocation fixtures:** `just gen-slicing-fixtures`
  writes the expected plate 3MF bytes, the argument vectors, and the flat
  preset JSON for the test vendor. A test compares against them.
- [ ] **Docs:** ADR-0009, ADR-0010, `CONTEXT.md`, and closing the
  approach-doc rows.
- [ ] **Full verification:**
  - `just build`, `just test`, `just test-rust`.
  - `just gen-contracts` plus `git diff --exit-code src/generated`.
  - `just test-orca`.
  - `just package`. Then install the `.deb` or Arch package on Linux x86_64
    and slice the two-plate fixture through the installed app with the
    installed v2.4.2. This is the F0-platform exit criterion.
- [ ] **Manual pass in `just dev`**, at 1440 × 900 and 1024 × 700:
  - A keyboard-only preparation: select, move by number, rotate, lay-flat,
    arrange, a plate tab, measure.
  - Slice, cancel, and a failure log.
  - The fact dialog.
  - Reduced motion.
  - Screenshots `p5-*.png`. Any check that could not be run is recorded as
    **unavailable**, not passed.
- [ ] **Verification doc:** map the evidence to each acceptance criterion
  of issue #15.

## Delivery order and parallel work

```text
Task 1 spike ──> Task 2 spec ──> Task 3 schema ──┬─> Task 4 runtime/presets ──┐
                                                  ├─> Task 5 3MF/mesh ─────────┤
                     (Gate E/F) ──────────────────┴─> Task 6 process ──────────┴─> Task 7 publish ─> Task 8 ops/commands ─┬─> Task 9 external
                                                                                                                          └─> Task 10 store ─> 11 viewport ─> 12 tools ─> 13 panel ─> 14 review
Task 4 ─> Task 15 settings                                                                              all ─> Task 16 tracer/verify
```

- Tasks 4, 5, and 6 run in parallel after Task 3.
- The renderer part of Task 11 (with the fake store) can start after Task 2,
  because artifact identity is agreed there. This is the overlap with P6
  that the approach doc allows. It wires to the real store after Task 10.
- A0 continues in parallel. P6 Moonraker research may run now. P6
  implementation needs P5's revision identity (Task 3).

## External dependencies and blockers

- **OrcaSlicer binaries: resolved.** The v2.4.2 AppImage is at
  `~/Downloads/OrcaSlicer_Linux_AppImage_Ubuntu2404_V2.4.2.AppImage` and its
  hash is verified. The 2.5.0-dev nightly is at
  `~/.local/bin/OrcaSlicer_Linux_AppImage_Ubuntu2404_nightly.AppImage`.
  `just test-orca` takes the path from `FARM3D_ORCA`.
- **A display.** Gate H, Task 16's manual pass, and the installed-app check
  need one. Without a display they are recorded as unavailable.
- **Windows and macOS.** No runner or hardware exists. Those builds compile
  and are unit-tested, but they make no support claim. The platforms stay
  "Candidate, unverified".
- **CI** has no OrcaSlicer. CI stays on fake-orca, and real-Orca evidence
  lives in the verification doc.

## Exit gate (from issue #15)

- [ ] The runtime spike is approved, and the deterministic invocation
  fixtures pass (Tasks 1 and 16).
- [ ] These tests pass (Tasks 3, 8, and 9):
  - cancellation and restart;
  - stale source;
  - two-plate identity;
  - external provenance and missing facts;
  - immutable artifacts.
- [ ] Accessible viewport and keyboard verification pass (Tasks 11–14 and
  16).
- [ ] A packaged or installed OrcaSlicer executes on Linux x86_64, the only
  platform F0 declares supported (Task 16).
- [ ] The tracer completes, with all revisions unchanged after restart
  (Task 16).
