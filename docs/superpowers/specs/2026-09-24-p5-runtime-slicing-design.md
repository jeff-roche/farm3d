# P5 Runtime Slicing and Slice Revisions Design

## Status

Approved focused design for GitHub issue #15. The user approved it on
2026-09-24 and answered its open questions, and this document treats those
answers as fixed. It narrows the
complete-v1 interaction design
(`2026-09-16-complete-v1-ui-workflows-design.md`: "Plate preparation",
"Operational slicing controls", "Slice workflow", and "Input behavior") to
the work P5 owns. It also resolves the P5 decision gate in the phase plan
(`2026-09-16-complete-v1-implementation-approach.md`, P5 section): the
geometry renderer and the OrcaSlicer runtime contract.

It rests on the approved runtime spike,
`docs/superpowers/baselines/2026-09-24-p5-orca-runtime-spike.md`. Every
claim below about OrcaSlicer behaviour cites a gate from that report
(Gate A … Gate J).

The user decided these questions on 2026-09-24:

1. **OrcaSlicer is user-installed, not bundled.** The user downloaded
   v2.4.2 and did not ask for bundling. Open question 1 confirms this
   reading.
2. **Accepted versions are every 2.x release and nightly or dev build.**
   Anything else is refused.
3. **The renderer is three.js**, with meshes parsed in Rust.
4. **External G-code facts are never pre-filled.** Each fact has its own
   **Use the file's value** action, and a fact left empty is recorded as
   absent.
5. **Engine and preset source are separate (spike option B).** A slicer
   runtime is an engine (any accepted OrcaSlicer) plus a JSON-bearing
   preset source. The preset source defaults to the engine itself.
6. **The spike is approved.**

### Open questions (answered 2026-09-24)

The user confirmed question 1 and chose the recommendation for questions 2
and 3. The text below records each question as answered.

1. **No bundling (D2). Confirmed.** farm3d finds or is pointed at an
   OrcaSlicer the user installed. It never ships one.
2. **Deleting Slice Revisions (D14). Chosen:** a Slice Revision
   can't be changed, but it can be deleted on purpose, after a
   confirmation, while nothing references it. Nothing can reference one
   before P7; P7 adds Queue Entries and Jobs as blockers. A Model with
   Slice Revisions can't be deleted until they are deleted.
   - *Rejected alternative:* Slice Revisions can never be deleted in v1.
3. **Where preparation happens (D19). Chosen:** inside the Library
   workspace. **Prepare…** turns the centre pane into the plate viewport
   and the right dock into the preparation panel, with **Back to Library**.
   - *Rejected alternative:* a separate "Prepare" destination in the
     activity bar.

## Goal

A user can:

- Open a Library Model (STL or 3MF) for preparation. They can:
  - arrange its objects across one or more build plates in a real 3D
    viewport, with numeric and keyboard equivalents for every tool;
  - choose a target Printer Profile, a material, a quality, and a small set
    of overrides.
- Slice each plate with a supported OrcaSlicer as a cancellable background
  operation. The operation shows progress, keeps its log collapsed, and
  expands the log on failure.
- Get one **immutable Slice Revision per plate**, each linked to exactly
  one Model Source Revision and one plate. The revision carries estimates
  and compatibility facts.
- Turn an imported G-code Model into an **external Slice Revision**. Every
  compatibility fact on it is either operator-confirmed or explicitly
  absent, and never inferred.
- Restart farm3d at any point and find every published revision unchanged.
  An interrupted slice is reported as interrupted, and its temporary files
  are gone.

P5 ends at an inspectable Slice Revision. It creates no Queue data and has
no Dispatch action.

## Scope

### In scope

- **Runtime management:** discovering the slicer runtime, checking its
  version, a preset source (including extracting an AppImage's profiles),
  and runtime settings.
- **Presets:** flattening presets from the preset source, listing
  compatible options, and mapping farm3d settings to OrcaSlicer keys.
- **Preparations:** persisted drafts, plates, object instances,
  transforms, staleness against the source revision, and reload.
- **Geometry and input:** geometry and mesh transfer from Rust, and the
  per-plate 3MF writer.
- **Running slices:**
  - process supervision (progress, cancel, timeout, orphan protection);
  - the operation log;
  - restart recovery.
- **Publishing:** output validation, estimate extraction, and publishing to
  the content store.
- **Slice Revisions:**
  - farm3d revisions and external revisions;
  - facts with provenance;
  - listing and review;
  - deletion under open question 2.
- **The `slicing` event stream** and its backfill.
- **Frontend:**
  - the three.js viewport and the preparation tools;
  - plate tabs and the preparation panel;
  - operation progress, cancel, and logs;
  - revision review, the G-code fact dialog, and the Slicer section in
    Settings.
- **The queue-handoff intent:** a disabled **Add to Queue…** that states
  its reason.
- **Test infrastructure:** fake-orca, deterministic invocation fixtures,
  real-OrcaSlicer tests (`just test-orca`), and the tracer.

### Non-goals

- Queue Entries, Jobs, dispatch, reservations, and material deduction (P7).
- Bundling OrcaSlicer, reading its binary `.opc` preset cache, or the
  Flatpak runtime (spike Gate I was not run).
- Editing OrcaSlicer's full settings surface. Only the allowlist in D4 is
  exposed.
- Multi-material or multi-extruder slicing. One filament per plate.
- Painting, supports editing, modifiers, per-object settings, cutting, and
  mesh repair.
- G-code preview (toolpath rendering). Review shows facts, estimates, and
  the log.
- Binary G-code, OBJ, STEP, and AMF (unchanged from P4).
- Windows and macOS runtime proof. The process layer compiles for them but
  makes no support claim, and they stay "Candidate, unverified".
- Pruning or disk-usage management (P9).

## Product vocabulary

These terms are added to or refined in `CONTEXT.md`:

- **Slice** — refined. The act of converting one prepared build plate of a
  Model into printable G-code for a target Printer Profile, done by a
  user-installed OrcaSlicer run as a separate process (ADR-0003).
- **Slice Revision** — refined. An immutable result made of the G-code
  bytes plus the facts needed to judge where it may be printed. A **farm3d
  Slice Revision** comes from one plate of one Model Source Revision. An
  **external Slice Revision** wraps an imported G-code Model Source
  Revision with operator-confirmed facts.
  _Avoid_: G-code version, export.
- **Preparation** — a Model's editable, persisted draft. It holds the
  plates, the object placement, the target, and the slicing choices, and
  is pinned to one Model Source Revision. It is not a Slice Revision.
  _Avoid_: Project (that means a Library grouping), job setup.
- **Plate** — one build plate within a Preparation. It has a stable
  identity (`plateKey`), a name, and an order. Each farm3d Slice Revision
  records the plate it came from.
  _Avoid_: bed (the physical surface), tray.
- **Slicer runtime** — the OrcaSlicer engine farm3d runs, plus the preset
  source it takes presets from.
- **Preset source** — an OrcaSlicer installation whose
  `resources/profiles` holds JSON presets. It defaults to the engine.
- **Confirmed fact** — a compatibility fact on an external Slice Revision
  that the operator entered or explicitly accepted. Its opposite is an
  **absent fact**.

## Decisions

### D1. Slice Revision kinds and identity

- **Two kinds**, `farm3d` and `external`, stored in one table (D14).
- **A farm3d revision** has:
  - exactly one `source_revision_id` (a Model Source Revision of an STL or
    3MF Model);
  - exactly one plate identity: `plate_key`, `plate_index`, `plate_name`;
  - one invocation manifest.
- **An external revision** has:
  - exactly one `source_revision_id` (a G-code Model Source Revision);
  - **no** plate identity and **no** invocation. Plate identity is never
    fabricated.
- **Slicing all plates** of a Preparation runs one operation per plate and
  publishes one revision per successful plate. There is no multi-plate
  revision.
- **Ids** use the prefixes `slr-` (Slice Revision), `prp-` (Preparation),
  and `sop-` (slice operation), made by `library::new_id`.

### D2. Slicer runtime: engine and preset source

- **Runtime configuration** is a single-row table,
  `slicer_runtime_config`. It is kept out of the exported Settings record
  because the paths belong to this machine. It holds:
  - `engine_path`: nullable, meaning auto-discover.
  - `preset_source_path`: nullable, meaning use the engine.
  - `revision`.
- **Paths never come from the frontend** (P4 D7). The paths arrive only
  through Rust-owned native pickers:
  - `pick_slicer_engine` (an executable or AppImage);
  - `pick_preset_source` (an executable, AppImage, or directory that
    contains `resources/profiles`).

  `reset_slicer_runtime` clears either or both.
- **Engine discovery** stops at the first candidate that probes
  successfully:
  1. `engine_path`, if set.
  2. `orca-slicer` on `PATH`.
  3. On Linux, every `~/Applications/*OrcaSlicer*.AppImage`,
     `~/.local/bin/*OrcaSlicer*.AppImage`, and
     `~/Downloads/*OrcaSlicer*.AppImage`. When several match, the newest
     parsed version wins, and a release beats a prerelease of the same
     version. Native-package install paths (AUR, `.deb`) are added only
     when there is evidence of them. Until then, those users rely on
     `PATH` or **Choose engine…**.

  Discovery never scans the whole disk. The Settings section shows which
  candidate was chosen and why.
- **The version probe** runs `<engine> --help` with:
  - the working directory set to a fresh temporary directory, because even
    `--help` writes `result.json` into the working directory (Gate A);
  - the D8 environment;
  - a 10 s timeout.

  It reads the first **stdout** line only. stderr carries the harmless
  `Error: unable to open display` (Gate A). The line must match:

  ```text
  ^OrcaSlicer-(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?:$
  ```

  Major version 2 is accepted. A suffix makes the channel `prerelease`,
  otherwise it is `release`. Any other major version gives
  `unsupportedVersion`, and an unparseable line gives `probeFailed`.
- **AppImage engines** run directly through FUSE: `--help` took 0.19 s
  (Gate A). The fallback is `--appimage-extract-and-run`, used only when
  the direct probe fails with the AppImage runtime's FUSE error. That
  fallback costs 0.86 s cold and leaves a `/tmp/appimage_extracted_*`
  directory, and the Settings section says so.
- **Preset source resolution** finds a directory that contains
  `profiles/<Vendor>.json`:
  - A directory is searched at `.`, `resources/`, and
    `share/OrcaSlicer/resources/`.
  - An installed executable is resolved through symlinks, then its
    `../resources`, `../share/OrcaSlicer/resources`, and
    `../../resources` are searched.
  - An AppImage has its profiles extracted with
    `<appimage> --appimage-extract 'resources/profiles/*'` (0.35 s, 78 MB;
    spike follow-up). The extraction runs in a temporary directory, then
    non-`.json` files are deleted, and the result is atomically renamed to
    `<app_cache_dir>/orca-profiles/<appimage-sha256>/`. That cache is
    reused while the AppImage's hash is unchanged. Stale cache entries are
    deleted at startup.
  - If the directory contains only `<Vendor>.opc` files (a cache-only
    build, spike "Open decision"), the source is `presetsUnreadable`.
- **Runtime state** (`SlicerRuntimeStatus`) has four parts:
  - `engine`: one of `available { version, channel, source: "configured" |
    "path" | "wellKnown", executableName }`, `notFound`,
    `unsupportedVersion { version }`, or `probeFailed { reason }`.
  - `presetSource`: one of `available { version, channel, origin: "engine"
    | "configured", vendorCount }`, `notConfigured`, `presetsUnreadable`,
    or `unavailable { reason }`.
  - `canSlice`: true when both parts are `available`.
  - `versionsDiffer`: true when the engine and preset-source versions
    differ.

  Only basenames cross to the UI, following P4's path rule, except in the
  Settings section, which shows full paths to help the user.

  When the engine has no readable presets and no preset source is
  configured, the state is `presetSource: presetsUnreadable`. The Settings
  section then offers **Choose preset source…**, with text naming the
  2.5.0-dev nightly case.
- **The status is re-probed**:
  - at startup (in the background, after the command gate opens);
  - when the configuration changes;
  - when the user presses **Check again**;
  - before each slice operation. That probe is cheap and cached for 60 s,
    keyed by the engine file's size and mtime.

### D3. Presets

- **A resolver** (`slicing/presets.rs`) indexes every `<Vendor>.json`
  bundle in the preset source by `(kind, name)` over `machine_list`,
  `process_list`, and `filament_list`. The first entry for a name wins, in
  sorted vendor order, so `OrcaFilamentLibrary` parents resolve across
  bundles (Gate B).
- **Flattening** follows `inherits` to a depth of 20; a cycle is
  `PRESET_INVALID`. Parent then child keys are merged, `inherits` is
  removed, and `name` and `from: "system"` are kept.
  - `from` must stay `"system"`: with `"User"`, v2.4.2 rejects the process
    preset as incompatible (Gate B).
  - The generator's `resolve_machine_preset` in `catalog/ingest/inherits.rs`
    is generalized to do this, and the catalog snapshot test must still
    pass byte for byte.
- **The machine preset** is the target's catalog `variant`, which is the
  OrcaSlicer machine preset name (for example "Elegoo Centauri Carbon 0.4
  nozzle"). If the preset source has no preset by that name, the result is
  `PRESET_NOT_FOUND`, naming the preset and the preset-source version.
- **Offered process presets** are instantiable (`instantiation != "false"`)
  presets whose flattened `compatible_printers` contains the machine preset
  name. A preset whose `compatible_printers` is empty and that has a
  non-empty `compatible_printers_condition` is **not offered** in P5. The
  condition language is not evaluated. The UI states this limit.
- **Offered filament presets** follow the same rule. Their compatibility is
  checked by farm3d at validation **and** again at start, because
  OrcaSlicer silently slices with an incompatible filament (Gate F). A
  mismatch is `FILAMENT_INCOMPATIBLE`.
- **Defaults** are chosen deterministically:
  - Quality: the offered process whose name contains "Standard", and
    otherwise the first offered process in name order.
  - Material: the first offered filament whose `filament_type` matches the
    target's loaded Spool family, if the target is a Printer. Otherwise the
    first "Generic PLA" match, and otherwise the first in name order.
- **Cost.** The index is built once per preset-source identity and cached
  in memory. It covers about 12,000 files, and the build runs on
  `spawn_blocking` and is cancellable. The flattened presets for one
  operation go into its work directory (D8) and into content blobs (D13).
  None of it is committed, bundled, or copied into the catalog, so ADR-0007
  is unchanged. This is the user's local installation, used locally
  (ADR-0004).

### D4. Mapping farm3d settings to OrcaSlicer keys

`slicing/mapping.rs` holds two explicit tables, each verified by a test
against the v2.4.2 flat presets.

**Profile overrides** go into the machine preset. Each
`PrinterProfile` field is either mapped or listed as not applicable, so a
new field fails the test:

| `PrinterProfile` field | OrcaSlicer key | Encoding |
|---|---|---|
| `bedShape` | `printable_area` | `["0x0","Wx0","WxD","0xD"]` offset by the origin; a polygon lists its points |
| `printableHeightMm` | `printable_height` | decimal string |
| `bedExcludeAreas` | `bed_exclude_area` | point list |
| `nozzleDiameterMm` | `nozzle_diameter` | one string per extruder; P5 requires exactly one |
| `nozzleType` | `nozzle_type` | as stored |
| `gcodeFlavor` | `gcode_flavor` | as stored |
| `defaultBedType` | `default_bed_type` (machine) | Encoding verified in plan Task 4 against the catalog's stored form. v2.4.2 stores `"4"` for the Centauri Carbon. If it can't be verified, the field is treated as unmapped. |
| `hasAuxiliaryFan`, `supportsAirFiltration`, `supportsMultiFilament`, `suggestedHostType` | — | not applicable (not slicing inputs) |

An override that has no row in the table is `UNMAPPED_PROFILE_OVERRIDE`.
It never happens silently.

**Slicing controls** (the only settings exposed in P5):

| Control | Section | OrcaSlicer key(s) | Allowed values |
|---|---|---|---|
| Layer height | Quality | `layer_height` | 0.05 mm up to 80% of the nozzle diameter |
| Walls | Strength | `wall_loops` | 1–20 |
| Top / bottom shells | Strength | `top_shell_layers`, `bottom_shell_layers` | 0–50 |
| Infill density | Strength | `sparse_infill_density` | 0–100 % |
| Infill pattern | Strength | `sparse_infill_pattern` | `rectilinear`, `grid`, `line`, `cubic`, `gyroid`, `honeycomb`, `lightning` (OrcaSlicer enum names) |
| Supports | Support | `enable_support`, `support_type` | off (`enable_support` = `0`), `normal(auto)`, `tree(auto)` |
| Support overhang angle | Support | `support_threshold_angle` | 0–90° |
| Adhesion | Support | `brim_type`, `brim_width`, `skirt_loops` | brim `no_brim`, `outer_only`, or `auto_brim`; width 0–20 mm; skirt 0–10 |

- **Unset controls** take the chosen process preset's value, and the
  panel shows which value that is.
- **Key check.** Before every run, each mapped key must be *known* to the
  preset source. A key is known when it appears in at least one preset of
  the same kind in the index (D3).
  - A single flat preset is not a reliable test, because presets leave out
    keys that keep their default value. For example, v2.4.2's
    `0.20mm Standard @Elegoo CC 0.4 nozzle` has no `brim_type`, yet
    writing it works (spike Gate B).
  - An unknown key fails with `UNSUPPORTED_SETTING_FOR_RUNTIME { key,
    presetSourceVersion }`.
  - The engine-side guard is D11 check 6, plus the explicit output checks.

  Together these are the guard for accepting any 2.x runtime (user
  decision 2).
- **No search.** Advanced Overrides is not a separate section in P5,
  because the list is under 12 fields (umbrella rule).

### D5. Preparations

- **One Preparation per Model.** Different targets are handled by changing
  the target and slicing again. Each result is its own revision.
- **Persisted** in `slice_preparations` (D14 SQL), with optimistic
  `revision`. Every edit is `update_preparation` with the whole next
  document and `expectedRevision`. A mismatch gives `CONFLICT`, and the
  frontend reloads.
- **Document** (`PreparationDocument`, JSON):
  - `plates: Plate[]`, holding 1 to 36 entries. 36 is OrcaSlicer's
    `MAX_PLATE_COUNT`. Each plate has `plateKey` (UUID), `name?`, and
    `instances: Instance[]`.
  - Each instance has `instanceKey` (UUID), `objectKey`, and
    `transform: { translateMm: [x, y], rotateDeg: [x, y, z],
    scale: [x, y, z] }`.
  - `target`: `{ kind: "printer", printerId }` or
    `{ kind: "profile", catalogRef }`.
  - `processPreset`, `filamentPreset`, and `controls`, where each field of
    `controls` is optional (D4).
- **Transforms.** The object's local mesh is scaled, rotated as
  X, then Y, then Z (extrinsic), and translated on XY. Z is derived: every
  instance rests on the bed, so its minimum world Z is 0. There is no
  free-floating placement, and the Z field is read-only.
  - Scale is 0.01–100 per axis.
  - Translation is plate-local millimetres from the bed origin of the
    target profile.
- **Seeding** happens when the Preparation is created from the Model's
  current revision:
  - **STL:** one plate, with one instance centred on the target bed.
  - **3MF with `plates`** (P4 `ThreeMfInspection.plates`): one plate per
    Orca plate, in index order, keeping names. Each plate's instances keep
    their relative XY layout from the build transforms, and the group is
    centred on the target bed.

    This avoids depending on the source printer's bed and on OrcaSlicer's
    plate grid: the stride is bed width × 1.2 and the columns are
    ⌈√count⌉ (spike Gate D and OrcaSlicer's `PartPlate.cpp`). Objects that
    don't fit are left in place and flagged by validation.
  - **3MF without plates:** one plate holding every build item, laid out
    the same way.
  - Build items that the 3MF marks unprintable are skipped and listed.
- **Staleness is derived**, never stored. A Preparation is stale when its
  `source_revision_id` is not the Model's current revision.
  - `reload_preparation` re-bases it onto the current revision. Instances
    whose `objectKey` still exists keep their transforms, new objects are
    added to plate 1 and arranged, and missing objects are removed and
    listed in the result.
  - `start_slice` on a stale Preparation returns `PREPARATION_STALE`
    unless the request carries `continueWithSourceRevision` equal to the
    pinned revision. That makes continuing a deliberate choice (umbrella
    "Slice workflow").
- **G-code Models have no Preparation.** `create_preparation` rejects them
  with `VALIDATION`.

### D6. Geometry and mesh transfer

- **`get_revision_geometry(revisionId)`** returns JSON:
  - `objects: [{ objectKey, name?, triangleCount, boundsMm, layFlatFaces:
    [{ normal: [x, y, z], areaMm2 }] }]`. `layFlatFaces` holds up to 8
    convex-hull faces, largest first.
  - `buildItems: [{ objectKey, transform: number[12], plateIndex?,
    printable }]`.
  - Keys: an STL has a single object, `objectKey: 1`. A 3MF uses its
    object ids, with components flattened into the object frame by the P4
    reader.
- **`get_revision_mesh(revisionId, objectKey)`** returns binary through
  `tauri::ipc::Response`. The layout is little-endian:
  - the magic `F3DM`;
  - `u32` version (1);
  - `u32` vertex count and `u32` index count;
  - `f32` positions (x, y, z), then `u32` indices.

  STL vertices are welded exactly by their bit patterns. The buffer is
  capped by P4's size limits. A 1 M-triangle mesh (18 MB) transferred in
  42 ms and 50 MiB in 160 ms (Gate H).
- **Cache.** Geometry is computed on `spawn_blocking` and cached in memory,
  keyed by `content_sha256`, with an LRU of 4 entries. Convex hulls use a
  farm3d-owned quickhull in `slicing/hull.rs`, tested on the fixtures,
  unless the plan's review picks a crate.

### D7. Plate input writer

`slicing/plate3mf.rs` writes one **core-only 3MF 2015/02** per plate
operation:

- One `<object>` per distinct `objectKey` used on the plate, with the mesh
  copied from the source revision (after component flattening).
- One `<item>` per instance, with the composed 3×4 transform (D5), Z
  already dropped to the bed.
- No Orca metadata, no Production extension, and no settings. OrcaSlicer
  places objects exactly by the build transforms with `--arrange 0
  --orient 0` (Gate C), so the Orca `model_settings.config` fallback is
  not needed.
- **Deterministic bytes:** ZIP entries use the date 1980-01-01 and mode
  0644, in the fixed order `[Content_Types].xml`, `_rels/.rels`,
  `3D/3dmodel.model`, with coordinates formatted to 6 decimal places.
- **An empty plate** is refused before spawning with
  `PREPARATION_INVALID { plateKey, reason: "empty" }`. OrcaSlicer would
  report code −6 (Gate F).

### D8. Invocation

**Work directory.** Each operation gets
`<content_root>/slicing-work/<sop-id>/`:

```text
input/plate.3mf
input/machine.json
input/process.json
input/filament.json
datadir/                 # empty, per operation; never the user's OrcaSlicer data dir
out/
progress.fifo            # Linux only
```

**Arguments** (spike Gates B–D), with every path absolute inside the work
directory:

```text
<engine>
  --datadir <work>/datadir
  --outputdir <work>/out
  --load-settings "<work>/input/machine.json;<work>/input/process.json"
  --load-filaments "<work>/input/filament.json"
  --arrange 0 --orient 0
  --slice 1
  [--pipe <work>/progress.fifo]          # Linux
  <work>/input/plate.3mf
```

These flags are never passed:

- `--allow-newer-file`;
- `--mstpp`, because it is not enforced (Gate E);
- `--no-check`;
- `--debug`.

**Working directory.** `<work>` itself, so any stray `result.json` stays
inside it.

**Environment.** An explicit allowlist, not inherited:

- `HOME`, `USER`, `LANG`, `LC_ALL`, `TMPDIR`, and `XDG_RUNTIME_DIR` when
  set.
- `PATH` set to `/usr/local/bin:/usr/bin:/bin` on Unix.

Everything else is dropped, including `DISPLAY`, `WAYLAND_DISPLAY`,
`LD_LIBRARY_PATH`, `LD_PRELOAD`, `APPDIR`, `APPIMAGE`, `ARGV0`, `OWD`,
`GDK_BACKEND`, `GTK_*`, `GIO_*`, `GDK_PIXBUF_*`, `GSETTINGS_SCHEMA_DIR`,
and `XDG_DATA_DIRS`. farm3d's own AppImage leaks these (Gate J).

**The invocation manifest** (JSON, stored as a blob, D13) records:

- the engine version and channel, and the engine file's SHA-256;
- the preset-source version and origin;
- the argument vector, with the work directory replaced by `<work>`;
- the flat preset hashes;
- the plate 3MF hash;
- the controls, and the profile overrides applied.

### D9. Process supervision

**Before spawning** (Linux), farm3d creates the FIFO and opens its read end
non-blocking. OrcaSlicer's writer gives up after about 1 s without a
reader, and then slices without progress (Gate E).

**Spawn** uses `tokio::process::Command` (the tokio `process` feature is
added). Its Unix `pre_exec`:

- `setsid()`, so the process gets its own process group;
- `prctl(PR_SET_PDEATHSIG, SIGTERM)` on Linux, so a crashed farm3d never
  leaves OrcaSlicer running (Gate E).

On Windows, farm3d assigns a Job Object with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. That is compiled and unit-tested, and
has no runtime claim.

**Progress.** Each FIFO line is parsed as `{ message, plate_index,
plate_count, plate_percent, total_percent, warning? }`:

- Malformed lines are ignored.
- `total_percent` is clamped to be monotonic.
- Updates are throttled to 250 ms as `slicing.operation.progress` events.
- The `message` is always shown, because the percentage sits at 1% for the
  whole load phase (6.6 s of 9.3 s on a 1 M-triangle mesh, Gate E).
- Off Linux, progress is indeterminate, with `message` = "Slicing…".

**Log.** stdout and stderr are merged into one ring buffer of at most
4 MiB, keeping the head (64 KiB) and the tail.

- **Redaction:** before storage or emission, the work directory is
  rewritten to `<work>`, the engine directory to `<engine>`, and `$HOME` to
  `~`.
- **Known noise** (`Error: unable to open display`) is kept but tagged, so
  the UI can dim it.

**Cancel** (`cancel_slice_operation`):

- A queued operation becomes `cancelled` without spawning.
- A running operation gets SIGTERM to the process group, then a 5 s grace
  period, then SIGKILL to the group. SIGTERM exits in about 31 ms (Gate E).
  SIGKILL is only the fallback, because it can leave a stale AppImage FUSE
  mount (Gate E).
- After a SIGKILL, farm3d checks `/proc/self/mountinfo` for a
  `.mount_*` whose AppImage matches the engine. If one is found, it runs
  `fusermount -u`, then `-uz`, and logs the result.

**Timeout.** Each plate has a wall-clock limit of **30 min**, which gives
`failed { code: "timeout" }` through the same SIGTERM escalation.
`--mstpp` is not used (Gate E).

**Concurrency.** At most one OrcaSlicer process runs at a time. Queued
operations start in FIFO order within the process lifetime.

### D10. Operation state machine and restart recovery

```text
queued ──start──> running ──ok──> succeeded
   │                 ├──error──> failed
   │                 ├──cancel─> cancelled
   └──cancel──> cancelled
(any non-terminal) ──app restart──> interrupted
```

- **Durable rows.** `slice_operations` rows are written:
  - at `queued`;
  - at the `running` transition, with `pid` and `pid_started_at` (the
    Linux `/proc/<pid>/stat` field 22);
  - at the terminal state.

  Progress is not persisted.
- **`start_slice`** is idempotent by `operationId`, using P3's `operations`
  ledger with a new `OperationKind::StartSlice`. A retry with the same id
  returns the same operation ids.
- **Startup recovery** runs in `build_runtime_services` after
  `startup_sweep` and before commands are served:
  1. Every `queued` or `running` row becomes `interrupted`, with
     `finished_at = now`.
  2. For each such `running` row whose `pid` is alive with the same start
     time and whose executable matches the engine basename: SIGTERM to its
     process group, 5 s, then SIGKILL, followed by the stale-mount check.
     This normally never happens, thanks to PDEATHSIG.
  3. `<content_root>/slicing-work/*` is removed entirely.
  4. Published revisions and their blobs are never touched. The sweep only
     removes work directories.
- **Retry.** An `interrupted`, `failed`, or `cancelled` operation is never
  resumed. **Slice again** starts a new operation.

### D11. Output validation and failure mapping

**Exit status.** `return_code` is the signed code from `result.json` when
it is present (Linux). Otherwise, the exit status is interpreted as `i8`,
because the exit code is `return_code` mod 256 (Gate F). A signal exit is
`cancelled` if farm3d sent it, and `engineCrashed` otherwise.

**Success requires every one of these:**

1. `return_code == 0`.
2. `out/plate_1.gcode` exists. OrcaSlicer can report "Success." without
   writing it (Gate F).
3. The file is regular, not a symlink, and at most 1 GiB.
4. The P4 G-code inspector accepts it with `producer.name ==
   "OrcaSlicer"` and `commandCount > 0`.
5. `observedBoundsMm`, if present, lies within the target printable area
   and height, with 2 mm of XY tolerance.
6. The G-code's own `printer_settings_id` and `filament_settings_id`
   claims, which are P4 allowlisted keys, equal the flat preset names that
   were passed in, after stripping surrounding quotes. The G-code writes
   `"Elegoo PLA @ECC"` quoted (spike Gate B).

**Failure codes** (`SliceFailureCode`), each mapped to user-facing text:

| Code | When | Text |
|---|---|---|
| `objectsOutsidePlate` | −50 | "An object is outside the printable area." |
| `presetInvalid` | −5 | "OrcaSlicer couldn't read the presets farm3d prepared." |
| `inputMissing` | −3 | "OrcaSlicer couldn't find its input." |
| `inputInvalid` | −6 | "OrcaSlicer couldn't read the prepared plate." |
| `presetIncompatible` | −17 | "The quality preset isn't compatible with this printer." |
| `engineError { returnCode }` | any other nonzero | the `error_string` from the table in spike Gate F or `result.json` |
| `outputMissing` | success with no G-code | "OrcaSlicer reported success but wrote no G-code." |
| `outputInvalid { reason }` | validation 3–6 fails | the reason |
| `timeout` | D9 | "Slicing took longer than 30 minutes." |
| `engineCrashed { signal }` | unexpected signal | "OrcaSlicer stopped unexpectedly." |
| `spawnFailed` | exec error | "farm3d couldn't start OrcaSlicer." |

A failed operation keeps its log blob and publishes nothing else. Its log
expands automatically in the UI.

### D12. Estimates

These are parsed from the produced G-code's claims, using the P4 claim
keys:

- `estimated printing time (normal mode)`, for example `1h 2m 3s`, becomes
  `printSeconds`.
- `filament used [g]` becomes `filamentGrams`, and `filament used [mm]`
  becomes `filamentMm`.
- `total layer number` becomes `layerCount`.
- `max_z_height` becomes `maxZMm`.

Each estimate is `null` when its claim is missing. The record is
`{ …, source: "farm3dSlice" }`.

External revisions keep the file's own values separately, as
`claimedEstimates { …, source: "fileClaim", trusted: false }`. They are
shown under "What the file says (not verified)" and are never copied into
`estimates`.

### D13. Publishing and storage

- **Staging.** The run stages these under the operation's content-store
  staging key, using P4 `stage_from_path` and `stage_bytes`:
  - `gcode`, the output;
  - `plate3mf`, `machinePreset`, `processPreset`, `filamentPreset`;
  - `manifest`, `log`.
- **Commit.** One `place_and_commit` does three things: inserts the
  blobs, inserts the `slice_revisions` row plus its `slice_revision_blobs`
  rows, and moves the operation to `succeeded` with `slice_revision_id`.
- **Crash safety** is P4's rule: a blob file may exist without a row, but
  never a row without a file. A crash between placement and commit leaves
  no revision (tested with `ContentFailurePoint`).
- **Failed and cancelled operations** stage and commit only the `log`
  blob, referenced from `slice_operations.log_sha256`.
- **Blob references.** `mark_unreferenced_blobs` and `startup_sweep` also
  count references from `slice_revision_blobs` and
  `slice_operations.log_sha256`.

### D14. Persistence

Migration `0006_p5_slicing.sql` (numbered at rebase, with
`CURRENT_SCHEMA_VERSION` moving with it):

```sql
CREATE TABLE slicer_runtime_config (
  singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
  revision INTEGER NOT NULL CHECK (revision >= 1),
  engine_path TEXT CHECK (engine_path IS NULL OR length(engine_path) > 0),
  preset_source_path TEXT CHECK (preset_source_path IS NULL OR length(preset_source_path) > 0),
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE slice_preparations (
  id TEXT PRIMARY KEY CHECK (id GLOB 'prp-*' AND length(id) BETWEEN 5 AND 64),
  model_id TEXT NOT NULL UNIQUE REFERENCES library_models(id) ON DELETE CASCADE,
  source_revision_id TEXT NOT NULL REFERENCES model_source_revisions(id) ON DELETE RESTRICT,
  revision INTEGER NOT NULL CHECK (revision >= 1),
  document_json TEXT NOT NULL CHECK (json_valid(document_json) AND json_type(document_json) = 'object'),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE slice_operations (
  id TEXT PRIMARY KEY CHECK (id GLOB 'sop-*' AND length(id) BETWEEN 5 AND 64),
  preparation_id TEXT NOT NULL REFERENCES slice_preparations(id) ON DELETE CASCADE,
  source_revision_id TEXT NOT NULL REFERENCES model_source_revisions(id) ON DELETE RESTRICT,
  plate_key TEXT NOT NULL,
  plate_snapshot_json TEXT NOT NULL CHECK (json_valid(plate_snapshot_json)),
  state TEXT NOT NULL CHECK (state IN ('queued','running','succeeded','failed','cancelled','interrupted')),
  failure_json TEXT CHECK (failure_json IS NULL OR json_valid(failure_json)),
  pid INTEGER,
  pid_started_at INTEGER,
  log_sha256 TEXT REFERENCES content_blobs(sha256),
  slice_revision_id TEXT REFERENCES slice_revisions(id) ON DELETE SET NULL,
  queued_at TEXT NOT NULL,
  started_at TEXT,
  finished_at TEXT,
  CHECK ((state = 'failed') = (failure_json IS NOT NULL)),
  CHECK (state <> 'succeeded' OR slice_revision_id IS NOT NULL OR finished_at IS NOT NULL)
) STRICT;
CREATE INDEX slice_operations_active ON slice_operations(state) WHERE state IN ('queued','running');

CREATE TABLE slice_revisions (
  id TEXT PRIMARY KEY CHECK (id GLOB 'slr-*' AND length(id) BETWEEN 5 AND 64),
  kind TEXT NOT NULL CHECK (kind IN ('farm3d','external')),
  model_id TEXT NOT NULL REFERENCES library_models(id) ON DELETE RESTRICT,
  source_revision_id TEXT NOT NULL REFERENCES model_source_revisions(id) ON DELETE RESTRICT,
  plate_key TEXT,
  plate_index INTEGER,
  plate_name TEXT,
  gcode_sha256 TEXT NOT NULL REFERENCES content_blobs(sha256),
  gcode_size INTEGER NOT NULL CHECK (gcode_size > 0),
  target_json TEXT NOT NULL CHECK (json_valid(target_json)),
  facts_json TEXT NOT NULL CHECK (json_valid(facts_json)),
  requires_manual_printer_selection INTEGER NOT NULL CHECK (requires_manual_printer_selection IN (0,1)),
  estimates_json TEXT NOT NULL CHECK (json_valid(estimates_json)),
  runtime_json TEXT CHECK (runtime_json IS NULL OR json_valid(runtime_json)),
  created_at TEXT NOT NULL,
  CHECK (
    (kind = 'farm3d' AND plate_key IS NOT NULL AND plate_index IS NOT NULL AND plate_index >= 1 AND runtime_json IS NOT NULL)
    OR (kind = 'external' AND plate_key IS NULL AND plate_index IS NULL AND plate_name IS NULL AND runtime_json IS NULL)
  )
) STRICT;
CREATE INDEX slice_revisions_model ON slice_revisions(model_id, created_at);
CREATE INDEX slice_revisions_gcode ON slice_revisions(gcode_sha256);

CREATE TRIGGER slice_revisions_immutable
BEFORE UPDATE ON slice_revisions
BEGIN
  SELECT RAISE(ABORT, 'slice revisions are immutable');
END;

CREATE TABLE slice_revision_blobs (
  revision_id TEXT NOT NULL REFERENCES slice_revisions(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK (role IN ('plate3mf','machinePreset','processPreset','filamentPreset','manifest','log')),
  sha256 TEXT NOT NULL REFERENCES content_blobs(sha256),
  PRIMARY KEY (revision_id, role)
) STRICT, WITHOUT ROWID;
CREATE INDEX slice_revision_blobs_sha ON slice_revision_blobs(sha256);

CREATE TRIGGER slice_revision_blobs_immutable
BEFORE UPDATE ON slice_revision_blobs
BEGIN
  SELECT RAISE(ABORT, 'slice revision blobs are immutable');
END;
```

Notes:

- **Immutability** means no `UPDATE`, ever, which the triggers enforce.
  Deletion follows open question 2 (answered):
  `delete_slice_revision` checks a new `SliceRevisionDeletionBlocker`
  registry. It is empty in P5; P7 registers Queue Entries and Jobs. The
  command runs in one transaction with `mark_unreferenced_blobs`, and emits
  `slicing.revision.removed`.
- **Model deletion.** `library_models → slice_revisions` is `RESTRICT`. P5
  registers a `ModelDeletionBlocker` (P4 D18) with a new
  `LifecycleBlockerCode::SliceRevisionsExist`: "Delete this Model's N
  Slice Revisions first." The Preparation cascades with the Model.
- **Source revisions.** `model_source_revisions` referenced by a
  preparation, an operation, or a revision can't disappear (`RESTRICT`).
  P4 deletes them only by cascading from the Model, and the blocker above
  runs first.
- **Where facts live.** `target_json`, `facts_json`, `estimates_json`, and
  `runtime_json` hold the wire shapes from D12, D15, and D16. The G-code content and the
  input blobs are in the content store.

### D15. Facts and provenance

Every Slice Revision carries `facts: SliceFacts`. Each field is a
`Fact<T> = { value: T, provenance: "farm3dInput" | "operatorConfirmed" } |
{ value: null, provenance: "absent" }`.

| Fact | farm3d revision | External revision |
|---|---|---|
| `printerProfile` (a `ProfileSnapshot`: `catalogRef`, `bedShape`, `printableHeightMm`, `bedExcludeAreas`, `nozzleType`, `gcodeFlavor`) | `farm3dInput`, taken from the target at start | `operatorConfirmed` (the operator picks a Printer or a catalog profile) or `absent` |
| `nozzleDiameterMm` | `farm3dInput`, from the machine preset | `operatorConfirmed` or `absent` |
| `materialFamily` (+ `materialOther?`) | `farm3dInput`, from the filament preset's `filament_type`, mapped to `MaterialFamily` (unknown → `OTHER` with the raw string) | `operatorConfirmed` or `absent` |
| `filamentDiameterMm` | `farm3dInput`, from the filament preset | `operatorConfirmed` or `absent` |

- `requiresManualPrinterSelection` is true exactly when any fact is
  `absent`. P7 reads it.
- **An external revision can never hold `farm3dInput`.** This is enforced
  by the Rust constructor, and by a property test over every G-code
  fixture, which checks that claims never flow into facts.
- `target` (`SliceTarget`, farm3d revisions only) records the Printer id,
  if the target was a Printer, plus the snapshot above, the preset names,
  and the controls.
- **Matching Farm Printer count.** The preparation panel shows the number
  of active Printers whose resolved `PrinterProfile` equals the snapshot on
  `bedShape`, `printableHeightMm`, `nozzleDiameterMm`, `nozzleType`, and
  `gcodeFlavor`. It uses the existing `PrinterRoster` disclosure. This is a
  display count, not a P7 eligibility rule.

### D16. External Slice Revisions

**`create_external_slice_revision`** takes
`{ operationId, sourceRevisionId, facts }`:

- `sourceRevisionId` must be a G-code Model Source Revision.
- `facts` holds, for each fact, either `{ kind: "confirmed", value }` or
  `{ kind: "absent" }`.
- The Printer Profile is confirmed either as `{ printerId }` (resolved to a
  snapshot at creation) or as `{ catalogRef }`.

**The resulting revision:**

- reuses the source revision's `content_sha256` as `gcode_sha256`. There is
  no copy, and `slice_revision_blobs` gets no rows;
- stores `claimedEstimates` (D12) and the producer, from the P4 inspection;
- has no plate and no runtime.

**Idempotency** is by `operationId`, through the P3 ledger with the new
`OperationKind::CreateExternalSliceRevision`. Creating several external
revisions from one source revision is allowed, because the facts may
differ.

**The dialog** (`GcodeFactsDialog`) shows each fact as a row:

- On the left: "What the file says (not verified)". This is the claim
  value with its line number, or "Not in the file".
- On the right: the confirmed value. It starts **empty**, as user decision
  4 requires.
- A per-row **Use the file's value** button parses the claim into the
  field's type, and is disabled when the claim is missing or unparseable.
  The row is then labelled "Confirmed by you".
- A **Clear** action returns the fact to absent.
- A summary shows "N facts not provided: this Slice Revision will need a
  Printer chosen by hand when it is queued."

### D17. Events and backfill

The **`slicing` stream** is its own sequence on `farm3d-event-v1`. Its
types, all emitted after commit:

| Type | Subject | Payload |
|---|---|---|
| `slicing.runtime.changed` | `runtime/local` | `SlicerRuntimeStatus` |
| `slicing.preparation.changed` | `preparation/<id>` | `PreparationRecord` |
| `slicing.preparation.removed` | `preparation/<id>` | `{}` |
| `slicing.operation.changed` | `sliceOperation/<id>` | `SliceOperationRecord` |
| `slicing.operation.progress` | `sliceOperation/<id>` | `SliceProgress { totalPercent?, platePercent?, message, warning? }` (ephemeral, throttled, no sequence gap handling needed) |
| `slicing.revision.created` | `sliceRevision/<id>` | `SliceRevisionSummary` |
| `slicing.revision.removed` | `sliceRevision/<id>` | `{}` |

**Backfill.** `list_slicing` returns `SlicingSnapshot { streamId,
snapshotSequence, runtime, preparations, activeAndRecentOperations,
revisions: SliceRevisionSummary[] }`. "Recent" means the last 50 terminal
operations.

**Frontend.** The frontend listens before backfilling, following
`library-store`. `isSlicingEvent` filters on the `slicing.` prefix, and
the Library and Printer listeners ignore these events.

### D18. Renderer

- **three.js** (plain, pinned in `package.json`; 0.186 was used in the
  spike). It is imperative and owned by `src/slicing/viewport/
  three-renderer.ts`, behind `ViewportRenderer`:

  ```ts
  interface ViewportRenderer {
    mount(canvas: HTMLCanvasElement): void;
    setBuildVolume(v: BuildVolume): void;     // bed polygon, height, exclude areas
    setMeshes(meshes: Map<ObjectKey, MeshBuffer>): void;
    setInstances(i: RenderedInstance[]): void; // transform, selected, outOfBounds
    setCamera(view: CameraView | CameraPose): void;
    setOverlays(o: { measure?: [Vec3, Vec3] }): void;
    pick(x: number, y: number): PickResult | null;
    setTheme(t: ViewportTheme): void;
    dispose(): void;
  }
  ```

- **Tests.** jsdom tests use `FakeViewportRenderer`, which records calls.
  The real renderer is verified by screenshots and by hand.
- **Rendering.** One indexed `BufferGeometry` per object, with instances
  as meshes that share it. Normals are computed once. The build volume is
  drawn as a wireframe box with a bed grid, and exclude areas are hatched.
- **Out-of-bounds instances** get a distinct material plus a text marker
  in the object list, so colour is never the only signal.
- **Colours** come from `--f3d-color-*` tokens, read with
  `getComputedStyle` at mount and on `onThemeChange`. No hex colours are
  hardcoded in components.
- **Camera.** Orbit, pan, and zoom follow the pointer (OrbitControls from
  `three/examples/jsm`). The standard views are Top, Front, Left, Right,
  and Iso, plus Reset. Under `prefers-reduced-motion`, camera moves jump
  instead of tweening.
- **Performance budget:** 1 M triangles at 60 fps on the development host
  (Gate H). Above 2 M triangles per plate, the object list shows "Large
  model: viewport may be slow".
- **WebGL unavailable.** If creating the context fails, the viewport shows
  a panel saying "3D view unavailable (WebGL is not available)". Every
  numeric tool still works.

### D19. Preparation workspace and tools

This follows open question 3 (answered).

- **Opening.** In the Library workspace, **Prepare…** (Model details, STL
  and 3MF only) calls `create_preparation` or loads the existing one. The
  centre pane becomes `PreparationWorkspace`, and the right dock becomes
  `PreparationPanel`. **Back to Library** returns. The navigation target
  stays `library/model/<id>`. Preparation mode is workspace state and is
  not deep-linked in P5.
- **Layout:**
  - plate tabs (Kobalte `Tabs`) at the top, with **+ Plate**, then a tab
    `DropdownMenu` with Rename, Move left/right, and Delete (the last plate
    can't be deleted);
  - the viewport, with a toolbar;
  - an object list (`DataTable`) with numeric fields for the selected
    instance.
- **Tools.** Each tool has a toolbar button and a keyboard command, and the
  numeric equivalent is always visible:

  | Tool | Keyboard | Numeric |
  |---|---|---|
  | Select next / previous | Tab in the object list; `[` / `]` in the viewport | list selection |
  | Move | arrows ±1 mm, Shift ±10 mm | X, Y (mm) |
  | Rotate Z | `R` / Shift+`R` ±15° | X, Y, Z (°) |
  | Scale | `+` / `-` ±5 % uniform | X, Y, Z (%) + Uniform checkbox |
  | Lay flat | `F` cycles through `layFlatFaces` (largest first) | face list with area |
  | Arrange plate | `A` | spacing (mm, default 5) |
  | Measure | `M` toggles; pointer picks two surface points | "Distance between selected objects" (centre-to-centre and gap) as the keyboard alternative |
  | Move to plate | `Shift+1`…`9` | plate `Select` |
  | Duplicate / Delete instance | `Ctrl+D` / `Delete` | buttons |
  | Views | `1` Top, `2` Front, `3` Left, `4` Right, `5` Iso, `0` Reset | view `SegmentedControl` |

- **Where the logic lives.** Pure modules hold all of it, fully unit-tested:
  `transforms.ts` (composition, Z drop, bounds), `layflat.ts`,
  `arrange.ts`, `bounds.ts` (inside/outside for polygon beds and exclude
  areas), and `validation.ts`. The viewport only renders the result.
- **Arrange** is a deterministic skyline packer. It works over instance
  footprints (axis-aligned after rotation), sorted by area and then
  `instanceKey`, with the given spacing, inside the largest axis-aligned
  rectangle of the printable area that avoids exclude areas. What doesn't
  fit stays where it is and is flagged.
- **Saving.** Edits apply locally and save through `update_preparation`,
  debounced 500 ms. A `CONFLICT` reloads, with a notice.

### D20. Preparation panel, validation, and slicing UI

- **Sections:**
  - **Target:** a `Select` of Printers and catalog profiles, grouped, with
    the matching count (D15).
  - **Material:** a filament `Select`, with the family shown.
  - **Quality:** a process `Select`, plus layer height.
  - **Strength** and **Support**, compact, with the D4 controls.

  Each control shows the preset's value as a placeholder when unset.
- **Validation** runs continuously on the frontend and again
  authoritatively in `start_slice`, which returns `PREPARATION_INVALID
  { issues }`. It reports these issues:
  - `emptyPlate`;
  - `outOfBounds { instanceKey }`;
  - `inExcludeArea { instanceKey }`;
  - `tooTall { instanceKey }`;
  - `nozzleMismatch` (the machine preset nozzle ≠ the profile nozzle);
  - `filamentIncompatible`;
  - `presetNotFound`;
  - `runtimeUnavailable`;
  - `stale`;
  - `unsupportedSetting { key }`.

  Each issue is a list row that moves focus to its object or field.
- **Actions:** **Slice plate** (the current tab) and **Slice all plates**.
  Both are disabled with a visible reason while issues exist.
- **`SliceOperationPanel`**, one row per plate operation:
  - state, message, and a `Progress` bar (determinate on Linux);
  - **Cancel**;
  - **Show log**, collapsed by default. On failure it expands and receives
    focus, and the failure text is shown above it;
  - **Copy log**.
- **On success,** the row links to the new revision's review.
- **Runtime missing.** The panel replaces the Slice actions with the
  runtime state and **Open Slicer settings**.

### D21. Slice Revision review and Library integration

- **Model details** gain a **Slice Revisions** section: a `Timeline` of
  revisions, newest first. Each entry shows the plate or "External", the
  target, time, filament, a prerelease badge if any, and a
  "Needs manual Printer selection" marker.
- **`SliceRevisionReview`** (dock view) shows:
  - identity: kind, plate, the source revision sequence, created time;
  - the target snapshot;
  - estimates, and for external revisions the claimed estimates, labelled;
  - facts with provenance badges that use icon, text, and colour:
    "From farm3d settings", "Confirmed by you", "Not provided";
  - runtime: "Engine 2.5.0-dev (prerelease) · presets 2.4.2", with a
    notice when `versionsDiffer`;
  - a read-only log (farm3d revisions);
  - **Add to Queue…**, **disabled**, with the visible reason "The Queue
    arrives in a later version." It creates no data and no navigation
    target. This is the queue-handoff intent;
  - **Delete…** (open question 2).
- **G-code Models.** Their details replace P4's note with **Create Slice
  Revision…**, which opens `GcodeFactsDialog`. The claims list stays
  as it is.
- **`BuildPlate`** (the P4 static placeholder) is deleted. For STL and
  3MF, Model details show a small read-only `PlateViewport` of the current
  revision, which is the same component without tools. A G-code Model shows
  its thumbnail.

### D22. Settings: Slicer section

A new section in the existing Settings surface shows:

- **Engine:** state, version and channel, executable name and full path,
  source. Actions: **Choose engine…** and **Use automatic discovery**.
- **Preset source:** state, version, origin, vendor count. Actions:
  **Choose preset source…** and **Use the engine's presets**. For
  `presetsUnreadable`, it adds: "This OrcaSlicer build stores its presets
  in a format farm3d can't read. Choose an OrcaSlicer 2.4 install or
  AppImage as the preset source."
- **Notices:** when the versions differ, a note that the presets come from
  another version; when the extract-and-run fallback is in use, a note
  about its `/tmp` cache.
- **Check again.**

### D23. Test infrastructure and fixtures

- **`fake-orca`** is a test-only binary target (`src-tauri/src/bin/
  fake-orca.rs`, built only under `cfg(test)` or feature `test-support`,
  and excluded from packages by `assert-package-contents.sh`). It
  reproduces the spike's observed contract:
  - `--help` prints `OrcaSlicer-<FAKE_ORCA_VERSION>:` to stdout and
    writes `result.json` into the working directory.
  - It honours `--outputdir`, `--pipe`, and `--slice`.
  - Its FIFO opener retries for about 1 s.
  - It writes `result.json` in the observed shape, and exits with
    `return_code & 0xff`.
  - Behaviour is selected by `FAKE_ORCA_SCENARIO`, one of: `success` (copies
    `FAKE_ORCA_GCODE`), `fail:<code>`, `successNoOutput`, `hang`,
    `hangWithGrandchild`, `garbageProgress`, `oversizedLog`, or
    `wrongPresetNames`.
  - SIGTERM exits within 100 ms.
- **Deterministic invocation fixtures.** `just gen-slicing-fixtures` writes
  `src-tauri/tests/fixtures/slicing/` for the `TestVendor` profile fixture,
  extended with process and filament presets:
  - the expected flat presets;
  - the plate 3MF bytes for a two-plate cube Preparation;
  - the argument vectors, with `<work>` placeholders.

  A test regenerates them in memory and compares.
- **Real-OrcaSlicer tests** are `#[ignore]` and gated by `FARM3D_ORCA`
  (the engine) and `FARM3D_ORCA_PRESETS` (optional). They run through
  `just test-orca` and cover:
  - success;
  - cancel;
  - two-plate identity;
  - determinism, with line 2 normalized;
  - the failure mapping for −50, −5, and success-without-output.

  The default is the v2.4.2 AppImage. The nightly runs with the v2.4.2
  presets.
- **Web fixtures** (`src/slicing/web-fixtures.ts`):
  - the runtime state `available 2.4.2`;
  - a two-plate Preparation for `mdl-web-enclosure` (its fixture gains two
    `plates`);
  - one succeeded operation and one failed operation, with log text;
  - a farm3d revision, and an external revision on `mdl-web-cube-gcode`
    with `materialFamily` absent;
  - generated mesh fixtures.

  In web mode, Slice, Cancel, Create external, and the pickers throw
  `needsDesktop`. Preparation edits are local.

### D24. Platform scope

Linux x86_64 is the only supported platform (F0). The exit criterion
"packaged/installed OrcaSlicer executes" is met there, by the installed
farm3d package (or its AppImage, see spike Gate J) slicing the tracer with
the installed v2.4.2 AppImage.

Windows and macOS builds must compile the process layer: a Job Object on
Windows, and on macOS a `setsid` process group with no PDEATHSIG, relying
on startup recovery. They make no runtime claim. The Linux-only FIFO path
is behind `cfg(target_os = "linux")`, and the other platforms report
indeterminate progress.

## Backend model

### Module layout

`src-tauri/src/slicing/`:

| File | Content |
|---|---|
| `mod.rs` | `SlicingServices<R>` (runtime, preset index cache, scheduler, stream, geometry cache) and the domain types |
| `runtime.rs` | D2: discovery, probe, preset-source resolution, AppImage extraction cache |
| `presets.rs` | D3: index, flattening, offered options, compatibility |
| `mapping.rs` | D4 tables |
| `preparation.rs` | D5: document validation, seeding, reload, staleness |
| `geometry.rs`, `hull.rs` | D6 |
| `plate3mf.rs` | D7 |
| `invocation.rs` | D8: work directory, arguments, environment, manifest |
| `process.rs` | D9: spawn, FIFO, log ring, cancel, timeout, mount cleanup |
| `operations.rs` | D10: state machine, scheduler, recovery |
| `publish.rs` | D11–D13 |
| `external.rs` | D16 |
| `facts.rs` | D15 |
| `repository.rs` | SQL for D14 |
| `events.rs` | D17 |
| `blockers.rs` | `SliceRevisionDeletionBlocker` registry and the P4 `ModelDeletionBlocker` impl |
| `commands.rs` | Tauri commands |

`RuntimeServices` gains `slicing: Arc<SlicingServices<R>>`, built after
`library`, with recovery (D10) run before the bootstrap gate opens.
`catalog/ingest/inherits.rs` is generalized (D3). `library/content.rs`
reference counting is extended (D13). `library/blockers.rs` registers the
slicing blocker (D14).

### Crates

- `tokio` gains the `process` feature.
- `rustix` is already a direct dependency (1.x, feature `fs`). It gains
  the `process` feature for `setsid`, PDEATHSIG, and `kill_process_group`,
  and uses `fs` for `mkfifo`.
- `windows-sys` Job Object features, only under `cfg(windows)`.
- `zip` already exists. Its `deflate` writer feature is enabled for D7.
- `uuid` (already present, `v4`) for `plateKey` and `instanceKey`.
- No new Tauri plugin.

### Commands

These are final names, and every one is registered per the P4 global
constraints. Requests carry `contractVersion` implicitly, and mutations
carry an `expectedRevision` or an `operationId`.

| Command | Request | Result |
|---|---|---|
| `get_slicer_runtime` | — | `SlicerRuntimeStatus` |
| `check_slicer_runtime` | — | `SlicerRuntimeStatus` (forces a probe) |
| `pick_slicer_engine` | `{ expectedRevision }` | `SlicerRuntimeStatus` or `cancelled` |
| `pick_preset_source` | `{ expectedRevision }` | same |
| `reset_slicer_runtime` | `{ expectedRevision, engine: bool, presetSource: bool }` | `SlicerRuntimeStatus` |
| `list_slice_options` | `{ target: SliceTarget }` | `{ machinePreset, processPresets[], filamentPresets[], defaults, profileSnapshot, matchingPrinterIds[] }` |
| `get_revision_geometry` | `{ revisionId }` | `RevisionGeometry` |
| `get_revision_mesh` | `{ revisionId, objectKey }` | binary (D6) |
| `list_slicing` | — | `SlicingSnapshot` |
| `create_preparation` | `{ modelId, target? }` | `PreparationRecord` (returns the existing one if present) |
| `update_preparation` | `{ preparationId, expectedRevision, document }` | `PreparationRecord` |
| `reload_preparation` | `{ preparationId, expectedRevision }` | `{ preparation, removedObjectKeys[], addedObjectKeys[] }` |
| `delete_preparation` | `{ preparationId, expectedRevision }` | `{}` |
| `start_slice` | `{ operationId, preparationId, expectedRevision, plateKeys[], continueWithSourceRevision? }` | `{ operations: SliceOperationRecord[] }` |
| `cancel_slice_operation` | `{ sliceOperationId }` | `SliceOperationRecord` |
| `get_slice_operation_log` | `{ sliceOperationId }` | `{ text, truncated }` |
| `list_slice_revisions` | `{ modelId }` | `SliceRevisionSummary[]` |
| `get_slice_revision` | `{ sliceRevisionId }` | `SliceRevisionRecord` |
| `create_external_slice_revision` | `{ operationId, sourceRevisionId, facts }` | `SliceRevisionRecord` |
| `delete_slice_revision` | `{ sliceRevisionId }` | `{}` or `LIFECYCLE_BLOCKED` |

That is 20 commands. The count assertions are updated by adding 20 to
`main`'s total at rebase.

### Error codes

`ErrorCode` gains:

- `SlicerUnavailable`
- `PresetSourceUnavailable`
- `PresetNotFound`
- `PresetInvalid`
- `FilamentIncompatible`
- `UnmappedProfileOverride`
- `UnsupportedSettingForRuntime`
- `PreparationStale`
- `PreparationInvalid`
- `OperationNotCancellable`

`RecoveryCode` gains:

- `OpenSlicerSettings`
- `ReloadPreparation`
- `EditPreparation`

Slice failures are not command errors. They are
`SliceOperationRecord.failure: SliceFailure` (D11).

### Wire types (ts-rs, `domain/`)

- **Runtime:** `SlicerRuntimeStatus`, `EngineState`, `PresetSourceState`,
  `RuntimeChannel`.
- **Preparation:** `PreparationRecord { id, modelId, sourceRevisionId,
  revision, stale, document, createdAt, updatedAt }`, plus
  `PreparationDocument`, `PlateDoc`, `InstanceDoc`, `InstanceTransform`,
  `SliceTarget`, `SliceControls`.
- **Geometry:** `RevisionGeometry`, `GeometryObject`, `GeometryBuildItem`,
  `LayFlatFace`.
- **Operations:** `SliceOperationRecord { id, preparationId,
  sourceRevisionId, plateKey, plateName?, plateIndex, state, failure?,
  sliceRevisionId?, queuedAt, startedAt?, finishedAt? }`, plus
  `SliceOperationState`, `SliceFailure`, `SliceFailureCode`, and
  `SliceProgress`.
- **Revisions:**
  - `SliceRevisionSummary { id, kind, modelId, sourceRevisionId,
    sourceRevisionSequence, plate?, targetLabel, estimates, facts,
    requiresManualPrinterSelection, runtime?, createdAt }`.
  - `SliceRevisionRecord` adds `target`, `claimedEstimates?`, `producer?`,
    and `blobs: { role, sizeBytes }[]`.
  - Supporting types: `SliceFacts`, `Fact<T>`, `FactProvenance`,
    `ProfileSnapshot`, `SliceEstimates`, `ClaimedEstimates`,
    `SliceRuntimeInfo { engineVersion, engineChannel, presetSourceVersion,
    presetSourceChannel }`.
- **Snapshot and events:** `SlicingSnapshot`, `SlicingEventType`, and
  `SlicingEventPayload`.
- **Existing types:** `LifecycleBlockerCode` gains `SliceRevisionsExist`,
  and `OperationKind` gains `StartSlice` and `CreateExternalSliceRevision`.

## Frontend architecture

### State

- **`src/slicing/types.ts`** re-exports the generated types.
- **`src/slicing/slicing-store.ts`** is the only owner of slicing data:
  - It loads with listen-before-backfill (`createSequencedStream`) over
    `slicing.*`.
  - It holds `runtime`, `preparations` (keyed by model), `operations`,
    `revisionsByModel`, `progress` (ephemeral), and `syncState`.
  - Its actions settle from command results by `revision` or state.
  - `desktopAvailable()` branches to web fixtures.
- **`src/slicing/geometry-cache.ts`** is an LRU of `RevisionGeometry` and
  `MeshBuffer` keyed by revision and object. It is dropped when leaving
  preparation mode.
- **Pure modules:** `transforms.ts`, `layflat.ts`, `arrange.ts`,
  `bounds.ts`, `validation.ts`, and `fact-parsing.ts` (claim string to
  typed value, used by **Use the file's value**).
- **`library-store`** is unchanged, except that `deleteModel` surfaces the
  new blocker text.

### Components

These are screens. No design-system primitive is expected, and the plan
adds one only if two screens need the same thing.

- `PlateViewport.tsx`: the canvas, the toolbar, and the live text
  description. It is used in both modes: tools, and read-only.
- `PreparationWorkspace.tsx`: plate tabs, the viewport, the object list,
  and the numeric fields.
- `PreparationPanel.tsx`: the D20 sections, the validation list, and the
  Slice actions.
- `SliceOperationPanel.tsx`: the operation rows and logs.
- `SliceRevisionReview.tsx`, `SliceRevisionList.tsx`.
- `GcodeFactsDialog.tsx`.
- `DeleteSliceRevisionDialog.tsx`.
- `SlicerSettingsSection.tsx`: the D22 Settings section.

Modified: `ModelDetailsPanel.tsx`, `LibraryWorkspace.tsx`, `App.tsx`
(`startSlicing()` in the startup order, after `startLibrary`), and the
Settings surface. `BuildPlate.tsx` and its CSS are deleted.

## Errors and recovery

| Situation | Behavior |
|---|---|
| No engine found | Settings and the panel show `notFound`, with **Choose engine…**. Slice is disabled with its reason. |
| Engine is a 3.x or unparseable build | `unsupportedVersion` or `probeFailed`, with the version or reason shown. |
| Engine is a cache-only nightly with no preset source | `presetsUnreadable`, with **Choose preset source…** and the D22 text. |
| Target preset missing from the preset source | `PRESET_NOT_FOUND`, naming the preset and the source version. |
| Incompatible filament chosen | Validation shows `filamentIncompatible`, and `start_slice` refuses. |
| Profile override without a mapping | `UNMAPPED_PROFILE_OVERRIDE`, naming the field, with a link to the Printer profile. |
| Mapped key missing in this runtime's presets | `UNSUPPORTED_SETTING_FOR_RUNTIME`, naming the key. |
| Object outside the bed | Validation shows it before slicing. If OrcaSlicer still reports −50, the result is `objectsOutsidePlate`. |
| Source changed (linked Model) | The Preparation is marked stale, with **Reload onto revision N** or **Continue with revision M**. |
| Cancel | The operation becomes `cancelled`, with no revision and no leftover process, work directory, or mount. |
| OrcaSlicer fails | The operation becomes `failed`, with its code and text, and the log expands and receives focus. |
| OrcaSlicer "succeeds" with no output | `outputMissing`. |
| Slow or hung slice | `timeout` after 30 min. |
| farm3d crash or restart mid-slice | OrcaSlicer exits (PDEATHSIG). At startup the operation becomes `interrupted` and its work directory is removed. Revisions are unchanged. |
| Crash between placement and commit | There is no revision. Orphan blobs are swept at startup. |
| Model delete with revisions | `LIFECYCLE_BLOCKED` (`sliceRevisionsExist`). |
| Revision conflict on a Preparation | `CONFLICT`, then reload and a notice. |
| WebGL unavailable | A text panel. Every numeric tool still works. |
| Slicing stream uncertain | Content is kept, marked stale, and backfilled with backoff (P4 behaviour). |

## Accessibility and adaptation

- **Pointer is never required.** Every viewport operation has a toolbar
  button with a shortcut hint, a keyboard command, and a numeric field
  (D19).
- **The canvas** has `role="img"`, with `aria-describedby` pointing to a
  visually hidden live region. The region lists the plate name, the object
  count, the selected object and its position, rotation, and scale, and any
  out-of-bounds objects. It updates `aria-live="polite"` with a 500 ms
  debounce.
- **Keyboard focus.** Shortcuts act only while the viewport or the object
  list has focus, so they never capture typing in fields. **Escape**
  returns focus from the viewport to the toolbar.
- **Focus order:** activity rail, then Library toolbar, then plate tabs,
  then viewport toolbar, then viewport, then object list and fields, then
  preparation panel, then status bar.
- **Progress** uses `Progress` with a text value and the stage message.
  Failure moves focus to the failure summary.
- **Provenance and validation** always use icon, text, and colour.
  Disabled actions show their reason as visible text, not only a tooltip.
- **Reduced motion.** Camera moves jump, and progress bars don't animate.
- **Window sizes.** At 1440 × 900, the Library sidebar collapses while
  preparing, and the viewport and the panel dock side by side. At
  1024 × 700, the preparation panel becomes an overlay toggled by
  **Settings panel** (the umbrella's explicit toggle rule), and the object
  list collapses under the viewport with a toggle.

## Acceptance criteria

1. **Migration.** The migration applies, is ledgered, and survives
   crash-boundary injection. Existing Library, Spool, and Printer data is
   unchanged. `UPDATE` on `slice_revisions` or `slice_revision_blobs`
   aborts. The external/farm3d CHECK rejects an external revision with a
   plate or a runtime.
2. **Runtime.**
   - Discovery and the probe classify fake-orca versions `2.4.2`,
     `2.5.0-dev`, `3.0.0`, and garbage correctly.
   - The probe's working directory is temporary; there is no stray
     `result.json`.
   - An AppImage preset source is extracted once per hash.
   - A cache-only source is `presetsUnreadable`.
3. **Presets.**
   - Flattening the `TestVendor` fixtures matches the golden output.
   - The catalog snapshot is byte-identical after the resolver
     generalization.
   - The offered-option filters, the filament compatibility check, and
     `PRESET_NOT_FOUND` all hold.
   - Every mapped key is known to the v2.4.2 preset source, and writing
     each one changes the G-code header (a real-Orca test).
4. **Mapping.** Every `PrinterProfile` field is mapped or marked not
   applicable. An unmapped override blocks, and `UNSUPPORTED_SETTING_FOR_RUNTIME`
   fires when a key is unknown to the preset source.
5. **Deterministic invocation fixtures.** Plate 3MF bytes, flat presets,
   and argument vectors equal the committed fixtures.
6. **Two-plate identity.** A two-plate Preparation gives two revisions with
   distinct `plateKey`s, the same `sourceRevisionId`, and G-code whose
   observed extents match each plate's layout (real-Orca, and fake-orca for
   CI).
7. **Cancellation and restart.**
   - Cancel stops fake-orca and its grandchild within 6 s, and leaves no
     revision or work directory.
   - A restart mid-run marks the operation `interrupted` and removes the
     work directory.
   - A PDEATHSIG test shows OrcaSlicer exits when its parent is killed.
   - Every prior revision is byte-identical.
8. **Failure.** Each D11 row is produced by fake-orca and mapped. The log
   is stored and redacted, with no absolute work, engine, or home path.
   Success-without-output fails.
9. **Stale source.** A linked-source change marks the Preparation stale.
   `start_slice` refuses without `continueWithSourceRevision`, and succeeds
   with it. Reload keeps the transforms of surviving objects.
10. **Immutable artifacts.** A published revision's G-code verifies through
    `open_verified`. Content changes to the source, the Preparation, or the
    settings never change an existing revision. Model deletion is blocked
    while revisions exist.
11. **External provenance and missing facts.**
    - External revisions hold only `operatorConfirmed` or `absent` facts
      (a property test over every G-code fixture).
    - `requiresManualPrinterSelection` equals "any absent".
    - The G-code blob is reused, not copied.
    - Claims appear only as `claimedEstimates` with `trusted: false`.
    - The dialog starts empty.
12. **Events.** Events are emitted after commit only. Backfill ordering
    holds. `slicing.*` events are ignored by the Library and Printer
    listeners, and vice versa.
13. **Frontend tests** cover:
    - the store: backfill, progress, settling, web mode;
    - the pure modules: transforms, lay-flat, arrange, bounds, validation,
      fact parsing;
    - `PlateViewport` with the fake renderer: toolbar, shortcuts, live
      description;
    - the preparation workspace and panel: tabs, numeric fields, the
      validation list, stale reload;
    - the operation panel: progress, cancel, log expand-on-failure;
    - the review: provenance text, the disabled queue intent announced with
      its reason;
    - `GcodeFactsDialog`: an empty start, use-the-file's-value, clear;
    - the Slicer settings states.
14. **Accessible viewport and keyboard verification.** In `just dev` at
    1440 × 900 and 1024 × 700, the whole tracer is completed keyboard-only,
    with screenshots `docs/screenshots/p5-*.png` and reduced motion
    checked.
15. **Packaged OrcaSlicer execution.** On Linux x86_64, the packaged farm3d
    (installed `.deb`, or its AppImage if root isn't available, recorded as
    such) slices the tracer with the installed v2.4.2 AppImage.
16. **The tracer** completes through the Tauri path:
    1. Import `orca-two-plates.3mf` and `orca-cube.gcode`.
    2. Prepare two plates and slice both into two revisions.
    3. Create an external revision from the G-code with the nozzle
       confirmed and the Printer and material absent.
    4. Restart.
    5. Every revision record and G-code hash is unchanged, and no
       operation is `running`.

    It runs with fake-orca in CI and with real v2.4.2 under
    `just test-orca`.

## Delivery strategy

Contract-first vertical slices, each landing with red/green tests. The
task-level plan is
`docs/superpowers/plans/2026-09-24-p5-runtime-slicing-slice-revisions.md`
(Tasks 3–16):

1. Schema, domain types, and blockers.
2. Runtime and preset source.
3. Presets and mapping.
4. Geometry and the plate writer.
5. The process supervisor and fake-orca.
6. Publishing.
7. Operations, stream, commands, and recovery.
8. External revisions.
9. Contracts and the store.
10. The viewport.
11. Tools.
12. The panel and the operation UI.
13. Review and the facts dialog.
14. Settings.
15. The tracer and verification.
