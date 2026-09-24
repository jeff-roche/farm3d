# P4 Format, Hash, and Watcher Spike

## Decision

| Question | Decision | Evidence |
|---|---|---|
| 3MF reader (D10) | Keep `zip` 8.6 + `quick-xml` 0.42 with D10's full supported set, including the Production extension. D10's fallback (narrowing to core-only) is **not needed**; `lib3mf` 0.1.6 was not tried. | Gate A |
| Content hash (D3, Task 3) | Keep **SHA-256** through `sha2` 0.10: `blobs/sha256/`, `content_sha256`. | Gate B: 1,512–1,520 MB/s against a 300 MB/s pass line |
| Watcher mode (D15, Task 8) | Keep **native `notify` watches** with `notify-debouncer-full` 0.7 (750 ms), one `NonRecursive` watch per parent directory. `PollWatcher` stays only the per-directory fallback when registration fails. | Gate C |
| Drop and picker (D7) | **Unavailable**: needs a human at the desktop. Handed to the user and re-run in Task 14. No capability change is made. | Gate D |

The spike ran on the Linux x86_64 development host only. It makes no claim
about Windows or macOS.

## Gates

| Gate | Result | Evidence summary |
|---|---|---|
| A: 3MF parsers | PASS | Every committed 3MF fixture read or was rejected as D10 requires. The two-plate Production-extension check first ran against a spike-only stand-in with OrcaSlicer's package layout and 19 real Bambu-format project files on the host. After the final review it also ran against the real `orca-two-plates.3mf` (OrcaSlicer 2.5.0-dev nightly): 2 objects, 2 plates, 24 triangles, and `requiredExtensions: ["p"]`, with both object parts reached through `3D/_rels/3dmodel.model.rels`. A 60.9 MB object part was inspected in 0.29 s with a 3.5 MB peak RSS. |
| A: `stl_io` evidence for D9 | Partly reproduced | `cube-ascii-bare-solid.stl` fails in `stl_io` 0.11. `cube-binary-solid-header.stl` **parses**: `stl_io` falls back to binary when the first line is not valid UTF-8. It fails only when the bytes up to the first newline are valid UTF-8. |
| B: hash throughput | PASS | SHA-256, 512 MiB, 1 MiB chunks, release build: 1,512.0 / 1,519.5 / 1,513.0 MB/s. |
| C: watcher | PASS | Each of the six scenarios that should fire produced exactly 1 debounced batch within 2 s in 20 of 20 repetitions, and no late batch; the first batch arrived at 825–826 ms. The symlink whose target is in another directory produced 0 batches in 20 of 20 repetitions, as expected. |
| C: watch limit | PASS | In an unprivileged user namespace with `user.max_inotify_watches` lowered to 16, the 17th `watch()` returned `ErrorKind::MaxFilesWatch`. |
| D: drop and picker | Unavailable | Needs a human to drag files from the file manager on the live desktop. |

## Host and versions

| Item | Value |
|---|---|
| CPU | AMD Ryzen 9 5900XT 16-Core Processor (32 threads, `sha_ni` present) |
| Kernel | Linux 7.2.6-1-cachyos x86_64 |
| Filesystem for watcher and hash runs | btrfs (`$HOME`) |
| Toolchain | rustc 1.98.0 (88d9e12ae 2026-08-18), cargo 1.98.0 |
| Crates | `zip` 8.6.0 (`deflate-flate2-zlib-rs`, over `flate2` 1.1.10 and `zlib-rs` 0.6.8), `quick-xml` 0.42.0, `sha2` 0.10.9, `notify` 8.2.0, `notify-debouncer-full` 0.7.0, `file-id` 0.2.3, `stl_io` 0.11.0 |
| PrusaSlicer | 2.9.6 (flatpak `com.prusa3d.PrusaSlicer`, system install), bundled vendor profile `PrusaResearch.ini` `config_version = 2.4.14` |
| OrcaSlicer | 2.5.0-dev nightly AppImage (`OrcaSlicer_Linux_AppImage_Ubuntu2404_nightly.AppImage`, SHA-256 `23e42968dee9b6905491b801fb0da62c73d271a42c39689374b5c67856035de4`, binary dated 2026-09-20). It writes `Application` = `BambuStudio-02.08.01.55` and `OrcaSlicer` = `2.5.0-dev` into 3MF files. Printer, process and filament presets come from the bundled `Custom` and `OrcaFilamentLibrary` vendor profiles, version 02.04.00.03; see "OrcaSlicer exports". |
| `fs.inotify.max_user_watches` | 524288 |

Gates A to C ran in a throwaway crate outside the repository. `stl_io` and
`lib3mf` never entered `src-tauri/Cargo.toml` or `Cargo.lock`. The only
dependency this task adds to the app is `zip`, which the fixture generator
needs.

## Fixtures

### Generated

`just gen-library-fixtures` writes these files into
`src-tauri/tests/fixtures/library/`. Two consecutive runs produce identical
bytes: ZIP entries carry the fixed date 1980-01-01 00:00 and mode 0644, and
nothing reads the clock. The generator never writes or deletes
`*.expected.json`; Task 4 writes those by hand.

The cube runs from (0,0,0) to (10,10,10) mm: 12 triangles, 8 distinct
vertices, and outward counter-clockwise winding.

| Fixture | Bytes | Construction |
|---|---|---|
| `cube-ascii.stl` | 2,499 | ASCII, first line `solid farm3d-cube`, 12 facets, `endsolid farm3d-cube` |
| `cube-ascii-bare-solid.stl` | 2,475 | ASCII, first line exactly `solid`, 12 facets, `endsolid` |
| `cube-binary.stl` | 684 | Binary, header `farm3d binary cube` zero-padded, count 12, 12 triangles |
| `cube-binary-solid-header.stl` | 684 | Binary, header `solid farm3d-binary cube with an ASCII-looking header` zero-padded, count 12. The first byte that is not valid UTF-8 is at offset 94, and the file contains no `0x0A` byte. |
| `cube-for-slicers.stl` | 684 | Binary, header `farm3d cube for slicer exports`, the same cube |
| `truncated-binary.stl` | 234 | Binary, declared count 12, 3 triangles present (84 + 3 × 50 bytes) |
| `nan.stl` | 684 | Binary cube, count 12. Triangle index 4 (0-based), vertex 1, x is NaN. |
| `empty.stl` | 84 | Binary, header `farm3d empty`, count 0 |
| `core-two-objects.3mf` | 1,406 | See below |
| `required-beam-lattice.3mf` | 1,116 | Core cube (object 1, one build item) with `xmlns:b` = Beam Lattice 2017/02 and `requiredextensions="b"` |
| `zip-slip.3mf` | 1,313 | `requiredextensions="p"`. Object 2 has one component `p:path="/../../etc/passwd" objectid="1"`. `3D/_rels/3dmodel.model.rels` also targets `/../../etc/passwd`. |
| `no-objects.3mf` | 879 | Empty `<resources>` and `<build/>` |
| `cura-style.gcode` | 432 | `;FLAVOR:Marlin`, `;TIME:1234`, `;Filament used: 1.2m`, `;Layer height: 0.2`, `;Generated with Cura_SteamEngine 5.8.0`, then 20 `G1 X.. Y.. F3000` moves |
| `plain.gcode` | 174 | `G28`, `G90`, then 10 `G1` moves, no comments |
| `binary.bgcode` | 16 | `GCDE` followed by 12 zero bytes |

`core-two-objects.3mf` holds these entries:

- `[Content_Types].xml`
- `_rels/.rels`, with the model and a thumbnail relationship
- `3D/3dmodel.model`
- `Metadata/thumbnail.png`, a 77-byte 2×2 RGB PNG, stored uncompressed

Its model has these properties:

- Unit `millimeter`, and metadata `Title` = `farm3d core two objects`.
- Object 1, `Cube A`, is the mesh cube.
- Object 2, `Cube B`, has one component: object 1 with transform
  `1 0 0 0 1 0 0 0 1 20 0 0`.
- The build has two items:
  - Object 1 with transform `1 0 0 0 1 0 0 0 1 5 5 0`.
  - Object 2 with no transform.
- Built geometry:
  - The build expands to 24 triangles; the model holds 12 mesh triangles.
  - Two objects, two build items.
  - Axis-aligned bounds from (5,0,0) to (30,15,10) mm.

### Slicer exports

| Fixture | Status | Producer and command |
|---|---|---|
| `prusa-project.3mf` | Committed | PrusaSlicer 2.9.6 CLI, see below |
| `prusa-cube.gcode` | Committed | PrusaSlicer 2.9.6 CLI, see below |
| `orca-two-plates.3mf` | Committed | OrcaSlicer 2.5.0-dev nightly CLI, see "OrcaSlicer exports" |
| `orca-cube.gcode` | Committed | OrcaSlicer 2.5.0-dev nightly CLI, see "OrcaSlicer exports" |

The PrusaSlicer GUI was not driven: the desktop is shared and live. Both
PrusaSlicer files come from the headless CLI, with an isolated `--datadir`
so the user's own configuration was never read or written.

The CLI's `--printer-profile`/`--print-profile`/`--material-profile`
selection did not apply the printer preset. The G-code came out with
`printer_settings_id = - default FFF -` and an empty `printer_model`, and
`--printer-profile` on its own crashed with exit status 139. The spike
therefore flattened the three bundled presets' `inherits` chains from
`PrusaResearch.ini` into one file, `mk4s.ini`, and passed it with `--load`.
The presets are:

- Printer: `Original Prusa MK4S 0.4 nozzle`
- Print: `0.20mm SPEED @MK4S 0.4`
- Filament: `Prusament PLA @MK4S`

Flattening follows PrusaSlicer's own rule: parents are applied left to
right, then the preset's own keys. Two changes were made:

- The preset-selection keys were dropped: `inherits`, `compatible_*`,
  `renamed_from`, `alias`, `default_*_profile` and `visible`.
- The three `*_settings_id` keys were set to the preset names, as the GUI
  records them.

The MK4S profile sets `binary_gcode = 1`, so the command overrides it with
`--no-binary-gcode` to get the ASCII fixture D20 asks for.

```sh
D=$PWD/prusa-data   # empty except vendor/PrusaResearch.ini
P="flatpak run --command=prusa-slicer com.prusa3d.PrusaSlicer --datadir $D"
$P --load mk4s.ini --no-binary-gcode --export-gcode -o prusa-cube.gcode cube-for-slicers.stl
$P --load mk4s.ini --no-binary-gcode --export-3mf  -o prusa-project.3mf cube-for-slicers.stl
```

| File | SHA-256 |
|---|---|
| `prusa-cube.gcode` | `afb1f3f61709c791caa386a08fa5392aba4d9f32cdfd3900f5fa1f3bee79226c` |
| `prusa-project.3mf` | `75f2244d7cf205e5fc7d3dfeecda37ede7f797ec2b429a249496ef1290dd6e5d` |
| `mk4s.ini` (not committed) | `b13439d579422c6b1f6fa7fa8fa4e4de41c776ae300f73be4c3a94a6b6956aa0` |

`prusa-cube.gcode` has these facts:

- 93,983 bytes. The first line is
  `; generated by PrusaSlicer 2.9.6 on 2026-09-24 at 12:47:26 UTC`.
- Settings recorded:
  - `printer_model = MK4S`
  - `printer_settings_id = Original Prusa MK4S 0.4 nozzle`
  - `print_settings_id = 0.20mm SPEED @MK4S 0.4`
  - `filament_settings_id = "Prusament PLA @MK4S"`
  - `filament_type = PLA`
  - `layer_height = 0.2`
  - `nozzle_diameter = 0.4`
  - `binary_gcode = 0`
- Results recorded:
  - `filament used [mm] = 246.29`
  - `filament used [g] = 0.73`
  - `estimated printing time (normal mode) = 5m 9s`
  - `max_layer_z = 10`
  - 50 `;Z:` layer markers
- The `; prusaslicer_config = begin` block runs from line 3817 to line 4170.
- No embedded thumbnail. The CLI does not render thumbnails without the GUI,
  although the profile lists `thumbnails = 16x16/QOI, 313x173/QOI,
  480x240/QOI, 380x285/PNG`.

`prusa-project.3mf` has these facts:

- 2,108 bytes, with these entries:
  - `[Content_Types].xml`
  - `_rels/.rels`
  - `3D/3dmodel.model`
  - `Metadata/Prusa_Slicer_wipe_tower_information.xml`
  - `Metadata/Slic3r_PE_model.config`
- It has no `Metadata/Slic3r_PE.config`: the CLI export does not embed the
  print configuration, where a GUI project save does.
  `Slic3r_PE_model.config` still matches D10's `Metadata/Slic3r_PE*.config`
  unsupported pattern.
- `_rels/.rels` declares a thumbnail relationship to
  `/Metadata/thumbnail.png`, but **that entry is absent**. Task 4's reader
  must treat a dangling thumbnail relationship as "no thumbnail", not as an
  error.
- The model is core only: no `requiredextensions`, and the `slic3rpe`
  namespace.
- Metadata: `Application` = `PrusaSlicer-2.9.6`, `Title` = `prusa-project`.
- One object (id 1) with 12 triangles. One build item with an identity
  transform.
- Built bounds from (0,0,0) to (10,10,10) mm.

The ZIP entry dates are the export time, so the file is committed as
produced and is not regenerated.

### OrcaSlicer exports

Both Orca files were added after the final review, from the user-approved
nightly AppImage, through the headless CLI only. The AppImage was unpacked
with `--appimage-extract` into a scratch directory. Every run cleared
`DISPLAY` and `WAYLAND_DISPLAY` and pointed `HOME` and `XDG_CONFIG_HOME` at
a scratch directory, so no window opened and the user's configuration was
neither read nor written. Orca prints `Error: unable to open display` and
then carries on in CLI mode. Each run exited 0, and `result.json` reported
`"return_code": 0`.

**Profiles.** The nightly ships its vendor profiles only as packed `.opc`
files. Its CLI loader reads `<datadir>/system/<Vendor>.json` trees and
cannot read an `.opc` in an empty `--datadir`: it fails with
`Failed loading configuration file …/Custom.json` whether the `.opc` comes
from the AppImage or from the GUI's cache. The runs therefore used a
scratch `--datadir` whose `system/` holds a read-only copy of the JSON
`Custom` and `OrcaFilamentLibrary` vendor trees, version 02.04.00.03. An
earlier OrcaSlicer flatpak install had unpacked them under
`~/.var/app/com.orcaslicer.OrcaSlicer/config/OrcaSlicer/system/`. The
presets were passed as the system preset files themselves:

- Printer: `MyKlipper 0.4 nozzle` (printer model `Generic Klipper Printer`)
- Process: `0.20mm Standard @MyKlipper`
- Filament: `Generic PLA @System`

**Two plates.** The CLI has no option that puts an object on a given
plate. `--arrange 1` fills plate 1 first, and two 10 mm cubes always fit
there. The second plate therefore comes from a generated input 3MF that
Orca loads and saves again:

1. Orca exports both cubes onto plate 1 (`one-plate.3mf`).
2. A script rewrites two parts of that package. In
   `Metadata/model_settings.config` it moves object 4's `<model_instance>`
   into a new `<plate>` with `plater_id` 2. In `3D/3dmodel.model` it adds
   300 mm to object 4's build-item X translation, which is one 250 mm bed
   width plus Orca's 20 % plate gap. Every other entry is copied unchanged.
3. Orca loads that package with `--arrange 0` and exports
   `orca-two-plates.3mf`. Orca writes every byte of the committed file. It
   recomputed the transforms: each object part now holds the cube centred on
   its origin, and the build items became `125 131 5` and `425 119 5`.
4. Orca slices plate 1 of the committed project, which gives
   `orca-cube.gcode`.

```sh
S=$SCRATCH/squashfs-root/AppRun            # from --appimage-extract
D=$SCRATCH/data                            # system/{Custom,OrcaFilamentLibrary}{,.json}
P=$D/system
ORCA="env -u DISPLAY -u WAYLAND_DISPLAY HOME=$SCRATCH/home XDG_CONFIG_HOME=$SCRATCH/home/.config $S --datadir $D"
SETTINGS="$P/Custom/machine/MyKlipper 0.4 nozzle.json;$P/Custom/process/0.20mm Standard @MyKlipper.json"
FILAMENT="$P/OrcaFilamentLibrary/filament/Generic PLA @System.json"

# 1. Both cubes, one plate
$ORCA --load-settings "$SETTINGS" --load-filaments "$FILAMENT" --arrange 1 \
  --outputdir step1 --export-3mf one-plate.3mf cube-for-slicers.stl cube-for-slicers.stl
# 2. Split the plates (script above) -> step2/two-plate-input.3mf
# 3. Re-save through Orca
$ORCA --arrange 0 --outputdir step3 --export-3mf orca-two-plates.3mf step2/two-plate-input.3mf
# 4. Slice plate 1 (writes step4/plate_1.gcode, committed as orca-cube.gcode)
$ORCA --arrange 0 --slice 1 --outputdir step4 step3/orca-two-plates.3mf
```

| File | SHA-256 |
|---|---|
| `orca-two-plates.3mf` | `7b925aa66e2f1968ae1aef9cc3135dc606e48c529a454376b7c8efceaf388666` |
| `orca-cube.gcode` | `2afa256122996660fc0940f9ed47bb5c07c4578e8454d22e9dc42c503b42a750` |

`orca-two-plates.3mf` has these facts:

- 10 entries:
  - `[Content_Types].xml`
  - `_rels/.rels`
  - `3D/3dmodel.model`
  - `3D/_rels/3dmodel.model.rels`
  - `3D/Objects/cube-for-slicers.stl_1.model`
  - `3D/Objects/cube-for-slicers.stl_2.model`
  - `Metadata/project_settings.config`
  - `Metadata/model_settings.config`
  - `Metadata/slice_info.config`
  - `Metadata/filament_sequence.json`
- The start part declares `xmlns:p` and `requiredextensions="p"`. So does
  each object part.
- `Application` = `BambuStudio-02.08.01.55`. `Title` is empty.
- Objects 2 and 4 each have one `p:path` component with an identity
  transform. Those components point at object 1 in
  `cube-for-slicers.stl_1.model` and object 3 in
  `cube-for-slicers.stl_2.model`. `3D/_rels/3dmodel.model.rels` declares
  both parts.
- Each object part holds one 12-triangle cube from (−5,−5,−5) to (5,5,5).
- The build items are object 2 at `1 0 0 0 1 0 0 0 1 125 131 5` and object
  4 at `… 425 119 5`. The built geometry is 24 triangles, with bounds from
  (120,114,0) to (430,136,10) mm.
- `model_settings.config` has two plates: `plater_id` 1 holds object 2, and
  `plater_id` 2 holds object 4. Both `plater_name` values are empty. Each
  object also carries per-object metadata: `extruder`, and on its part
  `matrix`, `source_file`, `source_object_id`, `source_volume_id` and
  `source_offset_x/y/z`.
- **No thumbnails.** Headless Orca renders no plate images. Even so,
  `_rels/.rels` and the `Thumbnail_Middle` metadata both name
  `/Metadata/plate_1.png`, and that entry is absent. As with
  `prusa-project.3mf`, this dangling reference means "no thumbnail".

`orca-cube.gcode` has these facts:

- 132,594 bytes and 5,064 lines. Line endings are LF.
- Line 2 is `; generated by OrcaSlicer 2.5.0-dev on 2026-09-24 at 16:42:43`.
- The header block records `total layer number: 50` and
  `max_z_height: 10.00`.
- The trailer records the settings and results:
  - `printer_model = Generic Klipper Printer`
  - `printer_settings_id = MyKlipper 0.4 nozzle`
  - `filament_settings_id = "Generic PLA @System"`
  - `filament_type = PLA`
  - `layer_height = 0.2`
  - `nozzle_diameter = 0.4`
  - `filament used [mm] = 245.37`
  - `filament used [g] = 0.73`
  - `estimated printing time (normal mode) = 3m 42s`
- There is no `bed_temperature` key. Orca writes
  `first_layer_bed_temperature = 35` instead.
- `M83` is set, and there is no `G91` and no tool change.
- Only plate 1's cube is present:
  `EXCLUDE_OBJECT_DEFINE … CENTER=125,131`.
- No thumbnail blocks, although the profile lists
  `thumbnails = 48x48/PNG, 300x300/PNG`.

## Gate A: 3MF parsers

The spike reader streams each part with `quick-xml` 0.42 (the
`Reader::read_event_into` API over `BufReader<ZipFile>`) and keeps counters
and bounds only. It implements D10's supported set:

- OPC:
  - `[Content_Types].xml` presence.
  - The `_rels/.rels` start part.
  - Part relationships from `3D/_rels/3dmodel.model.rels`. A `p:path` part
    must be declared there.
- Core:
  - `unit`, `<metadata>`, objects, and mesh vertices and triangles.
  - Components and build items with transforms.
  - Bounds use the row-vector convention: the 8 corners of each object box,
    transformed.
- Production: `p:path` resolved within the package. Any `..` that escapes
  the root is `INVALID_CONTENT`.
- Required extensions: prefixes are resolved through the `xmlns:` declarations.
  `p` and `m` are accepted; any other is `UNSUPPORTED_FORMAT`.
- Plates from `Metadata/model_settings.config`: `plater_id`, `plater_name`,
  and `model_instance` `object_id`.
- Unsupported listing: `Metadata/*.config|.gcode|.txt|.xml|.json` other than
  `model_settings.config`; per-object keys in `model_settings.config`; paint
  attributes; and material groups.
- Limits: 10,000 entries, 4 GiB total, 2 GiB per entry, and a 1,000:1
  per-entry read limit.

| Input | Result |
|---|---|
| `core-two-objects.3mf` | OK. 2 objects, 2 build items, 24 triangles, bounds (5,0,0)–(30,15,10). Title `farm3d core two objects`. Build transforms: object 1 `… 5 5 0`, object 2 identity. Component transform: object 2 → object 1 `… 20 0 0`. Thumbnail `Metadata/thumbnail.png`. |
| `zip-slip.3mf` | Rejected: `INVALID_CONTENT: part path escapes the package: /../../etc/passwd` |
| `no-objects.3mf` | Rejected: `INVALID_CONTENT: This 3MF contains no objects.` |
| `required-beam-lattice.3mf` | Rejected: `UNSUPPORTED_FORMAT`, extensions `["http://schemas.microsoft.com/3dmanufacturing/beamlattice/2017/02"]` |
| `prusa-project.3mf` | OK. 1 object, 12 triangles, bounds (0,0,0)–(10,10,10). Producer `PrusaSlicer-2.9.6`. Unsupported: `Metadata/Prusa_Slicer_wipe_tower_information.xml`, `Metadata/Slic3r_PE_model.config`. Dangling thumbnail relationship `Metadata/thumbnail.png`. |
| `orca-two-plates.3mf` (added after the final review, read by the shipped Task 4 inspector) | OK. 2 objects, 2 build items, 2 plates (`1` → object `2`, `2` → object `4`), 24 triangles, bounds (120,114,0)–(430,136,10), `requiredExtensions: ["p"]`. Both object parts are reached through `3D/_rels/3dmodel.model.rels`. Unsupported: `Metadata/filament_sequence.json`, per-object settings, `Metadata/project_settings.config`, `Metadata/slice_info.config`. Dangling `Thumbnail_Middle` `/Metadata/plate_1.png`. |
| Orca-layout stand-in (spike only, not committed) | OK. 2 objects, 2 plates (`1` → object `2`, `2` "Second" → object `4`), 24 triangles, `requiredExtensions: ["p"]`. Parts through `3D/_rels/3dmodel.model.rels`: `3D/Objects/object_1.model`, `3D/Objects/object_2.model`. Unsupported: `perObjectSettings` (`extruder`, `part`), `Metadata/project_settings.config`. |
| 22 real project files on the host (read-only, not committed, names not recorded) | 19 Bambu-format packages with `requiredextensions="p"` parsed OK in 76–201 ms, one of them with 2 plates. 2 PrusaSlicer projects parsed OK. 1 Orca filament-profile export was rejected as `This 3MF contains no objects.`, as D10 requires. 2 matches were directories, not files. |
| Synthetic Production-extension package with a 60,915,354-byte object part (588,232 triangles; 4.85 MB compressed) | OK in 284–287 ms over 3 runs. |

Peak RSS was read from `VmHWM` in `/proc/self/status` at process exit,
because `/usr/bin/time` is not installed on the host:

| Run | Peak RSS |
|---|---|
| Baseline: `core-two-objects.3mf` | 3,512–3,528 kB |
| 60.9 MB part | 3,524–3,536 kB |

The 60.9 MB part added less than 0.1 MB above baseline, against a 64 MB
limit. It was inspected in 0.29 s, against a 5 s limit. **PASS.**

`lib3mf` 0.1.6 was not tried: the fallback applies only when the first
reader fails.

### `stl_io` 0.11 (evidence for D9)

| Input | `stl_io::read_stl` |
|---|---|
| `cube-ascii-bare-solid.stl` | ERROR `failed to fill whole buffer`: the ASCII probe rejects `solid` without a space, and the binary reader then misreads the text |
| `cube-binary-solid-header.stl` | **OK, 12 faces** |
| Spike-only variant: the same bytes with header `solid farm3d-binary\n` padded with spaces | ERROR `stream did not contain valid UTF-8` |
| `cube-binary.stl`, `cube-ascii.stl` | OK, 12 faces |
| `truncated-binary.stl` | ERROR `failed to fill whole buffer` |
| `nan.stl` | **OK, 12 faces**: no finiteness check |
| `empty.stl` | **OK, 0 faces**: no zero-triangle rejection |

`AsciiStlReader::probe` calls `read_line`. On a binary file with a
`solid `-prefixed header, `read_line` usually fails on the first byte that is
not valid UTF-8, and `create_stl_reader` then falls back to the binary
reader. Misdetection happens only when every byte up to the first `0x0A` is
valid UTF-8. That is data dependent, for example a header that ends in a
newline.

D9's evidence sentence therefore overstates the failure: a binary STL with a
`solid ` header does not always fail. D9's decision still holds for four
reasons:

- A bare `solid` ASCII file fails.
- Detection depends on the data.
- NaN and zero-triangle files are accepted.
- The format-specific readers are private.

The controller or spec owner should decide whether to reword D9's evidence
line.

## Gate B: hash throughput

This gate ran `sha2` 0.10.9 `Sha256` over a 512 MiB file from `/dev/urandom`,
in `read()` calls of 1 MiB, in a release build. The file had just been
written, so the page cache was warm. The gate therefore measures the hash,
not the disk.

| Run | Seconds | MB/s | MiB/s |
|---|---|---|---|
| 1 | 0.355 | 1,512.0 | 1,441.9 |
| 2 | 0.353 | 1,519.5 | 1,449.2 |
| 3 | 0.355 | 1,513.0 | 1,442.9 |

All three digests were
`bd45cf7eabe3fa917eb28c1755038a942af47675028ac9efb2838c5d6f7bdf8c`, which
matches `sha256sum`. The CPU is an AMD Ryzen 9 5900XT with `sha_ni`.
**PASS** at about 5× the 300 MB/s line. Task 3 keeps `blobs/sha256/` and
`content_sha256`.

## Gate C: watcher

This gate used `notify-debouncer-full` 0.7 `new_debouncer(750 ms, None,
mpsc::Sender)` with one `NonRecursive` watch. Each scenario ran 20
repetitions, and each repetition built its own setup:

1. It made a fresh `watched/` and `other/` directory under `$HOME` (btrfs)
   and wrote the setup files.
2. It created a new debouncer and watched `watched/`.
3. It waited 300 ms and drained the channel.
4. It performed the action and collected batches for 3.5 s.

A batch counts when any of its events names `watched/` itself or a direct
child of it. Batches up to 2,000 ms after the action count as "within 2 s";
later ones are counted separately.

| Scenario | Reps with ≥1 batch within 2 s | Batches within 2 s, per rep | Batches after 2 s | First batch (ms) | Result |
|---|---|---|---|---|---|
| In-place overwrite (`fs::write` on the target) | 20/20 | 1 ×20 | 0 ×20 | 825–826 | PASS |
| Write `.model.stl.tmp`, rename over target | 20/20 | 1 ×20 | 0 ×20 | 825–826 | PASS |
| Delete | 20/20 | 1 ×20 | 0 ×20 | 825–826 | PASS |
| Rename away to `other/`, 50 ms, rename back | 20/20 | 1 ×20 | 0 ×20 | 825–826 | PASS |
| `remove_dir_all(watched)`, `create_dir(watched)`, write the file | 20/20 | 1 ×20 | 0 ×20 | 825–826 | PASS |
| Symlink `link.stl` → `watched/target.stl`; overwrite the target | 20/20 | 1 ×20 | 0 ×20 | 825–826 | PASS |
| Symlink `link.stl` → `other/target.stl`; overwrite the target | 0/20 (expected) | 0 ×20 | 0 ×20 | none | Recorded: no event, as D15 expects |

No repetition reported a watcher error. The full per-repetition output is
below.

```
in-place overwrite: reps_with_batch_within_2s=20/20
  batches_within_2s=[1 ×20]  batches_after_2s=[0 ×20]
  first_batch_ms=[826, 826, 826, 826, 825, 826, 825, 825, 825, 826, 826, 825, 826, 825, 825, 825, 826, 825, 825, 825]
temp write + rename over: reps_with_batch_within_2s=20/20
  batches_within_2s=[1 ×20]  batches_after_2s=[0 ×20]
  first_batch_ms=[825, 826, 826, 825, 826, 825, 825, 825, 825, 825, 825, 825, 825, 825, 825, 825, 825, 825, 825, 825]
delete: reps_with_batch_within_2s=20/20
  batches_within_2s=[1 ×20]  batches_after_2s=[0 ×20]
  first_batch_ms=[826, 826, 825, 825, 826, 826, 825, 825, 826, 825, 826, 826, 826, 826, 825, 825, 825, 826, 825, 826]
rename away then back: reps_with_batch_within_2s=20/20
  batches_within_2s=[1 ×20]  batches_after_2s=[0 ×20]
  first_batch_ms=[826, 826, 826, 826, 826, 826, 826, 825, 826, 826, 825, 825, 825, 826, 825, 826, 826, 825, 825, 826]
remove + recreate parent, recreate file: reps_with_batch_within_2s=20/20
  batches_within_2s=[1 ×20]  batches_after_2s=[0 ×20]
  first_batch_ms=[826, 826, 825, 825, 826, 826, 826, 825, 825, 826, 826, 826, 825, 825, 826, 826, 825, 825, 825, 825]
symlink, same-dir target overwritten: reps_with_batch_within_2s=20/20
  batches_within_2s=[1 ×20]  batches_after_2s=[0 ×20]
  first_batch_ms=[826, 826, 825, 826, 825, 825, 826, 825, 825, 826, 825, 826, 825, 826, 826, 825, 826, 826, 826, 826]
symlink, target in another dir overwritten: reps_with_batch_within_2s=0/20 (expected none)
  batches_within_2s=[0 ×20]  batches_after_2s=[0 ×20]
GATE C PASS
```

**Reading of D15.** The spec asks for "exactly one resulting check within
2 s". A debounced batch is not a check: the pass line here is at least one
batch within 2 s in 20 of 20 repetitions, and every batch count is recorded.
On this host every scenario happened to produce exactly one batch. "Exactly
one check" is a property of `LinkSupervisor`, which Task 8 enforces with
in-flight coalescing and asserts in `p4_links` test 2.

**Parent directory removal.** The one batch comes from the removal. The
inotify watch on the removed directory does not survive, so later changes
in the recreated directory are not seen by that watch. This is D15's
"missing parent" case: the supervisor must re-watch the nearest existing
ancestor, then re-add the parent's watch when it reappears. Task 8 must
implement and test that; the debouncer does not do it.

### Watch-limit behaviour

The host has `fs.inotify.max_user_watches = 524288` and
`user.max_inotify_watches = 524288`. The spike never changed a host sysctl.

It ran in a throwaway unprivileged user namespace (`unshare -Ur`):

1. It lowered that namespace's `/proc/sys/user/max_inotify_watches` to 16.
2. It created directories and called `RecommendedWatcher::watch(dir,
   NonRecursive)` on each until one failed.

```
watch #17 failed: kind=MaxFilesWatch display=OS file watch limit reached. about ["…/limit/d16"]
```

The host values read 524288 again afterwards. Outside the namespace, 100
watches registered without error. `watch()` therefore returns
`notify::ErrorKind::MaxFilesWatch` at the limit, which is the error Task 8's
`PollWatcher` fallback keys on.

## Gate D: drop and picker

**Unavailable (needs a human; handed to the user and re-run in Task 14).**
The desktop is shared and live, so the spike did not run `just dev`, add the
temporary handler, or drag files. `capabilities/default.json` is unchanged:
`core:default`, `dialog:allow-open`, `dialog:allow-save`.

Manual steps for a human:

1. In `src-tauri/src/lib.rs`, add this temporary handler to the
   `tauri::Builder` chain (next to `.setup(…)`). It logs the count only,
   never the paths:

   ```rust
   .on_webview_event(|_webview, event| {
       if let tauri::WebviewEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
           eprintln!("p4-spike drop count: {}", paths.len());
       }
   })
   ```

2. Add a temporary command. Register it in `generate_handler!` only; do not
   add it to `COMMAND_NAMES` or the contracts:

   ```rust
   #[tauri::command]
   fn p4_spike_pick(app: tauri::AppHandle) -> usize {
       use tauri_plugin_dialog::DialogExt;
       app.dialog()
           .file()
           .add_filter("3D models and G-code", &["stl", "3mf", "gcode", "gco", "g"])
           .blocking_pick_files()
           .map(|files| files.len())
           .unwrap_or(0)
   }
   ```

3. Run `just dev`. Open the webview devtools (right-click, **Inspect**), and
   in the console run:

   ```js
   const { getCurrentWebview } = await import('@tauri-apps/api/webview');
   await getCurrentWebview().onDragDropEvent(e => console.log('dnd', e.payload.type));
   ```

4. From the file manager, select two files (for example `cube-ascii.stl` and
   `plain.gcode` from `src-tauri/tests/fixtures/library/`) and drag them
   onto the window.

   - **Pass:** the terminal prints `p4-spike drop count: 2`, the console
     shows `dnd enter`, `dnd over` and `dnd drop`, and no capability or
     permission error appears.

5. In the console, run:

   ```js
   await window.__TAURI_INTERNALS__.invoke('p4_spike_pick')
   ```

   In the dialog, confirm the filter is shown and that you can Ctrl-select
   two files.

   - **Pass:** the call returns `2`.

6. If step 4 or step 5 reports a capability error, add exactly the named
   permission to `src-tauri/capabilities/default.json` and record it here.
7. Revert the temporary code, then confirm
   `git diff --exit-code src-tauri/src` is clean.

## Consequences for later tasks

- **Task 3:** SHA-256 (`blobs/sha256/<hh>/<hex>`, `content_sha256`).
- **Task 4:**
  - Uses the committed generated fixtures, `prusa-project.3mf` and
    `prusa-cube.gcode`.
  - `orca-two-plates.3mf` and `orca-cube.gcode` were unavailable during
    Task 4. They were produced after the final review, and their
    hand-written oracles are now in the fixture loop.
  - The reader must tolerate a dangling thumbnail relationship, as in
    `prusa-project.3mf`.
  - The `triangleCount` of `core-two-objects.3mf` depends on whether
    component instances are expanded: 24 expanded, 12 mesh triangles. Task 4
    must pick one and write it into the hand-written expectation.
- **Task 8:**
  - Native watches with the 750 ms debouncer.
  - `PollWatcher` (10 s) only for a directory whose registration fails with
    `MaxFilesWatch` or any other error.
  - D15's "exactly one resulting check" is a property of the supervisor, not
    of the debouncer: a batch is not a check. Task 8 enforces it with
    in-flight coalescing and asserts it in `p4_links` test 2.
- **Task 14:** re-run Gate D with a human at the desktop.
