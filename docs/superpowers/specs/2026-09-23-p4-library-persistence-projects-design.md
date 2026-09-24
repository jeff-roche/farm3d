# P4 Library Persistence and Projects Design

## Status

Approved focused design for GitHub issue #14. The user answered the open
questions on 2026-09-23, and this document treats those answers as fixed.
It narrows the approved complete-v1 interaction design
(`2026-09-16-complete-v1-ui-workflows-design.md`, "Library, Projects, and
slicing") to the work P4 owns. It also resolves the P4 decision gate in the
phase plan (`2026-09-16-complete-v1-implementation-approach.md`, P4
section): parser libraries, content hash, managed storage layout, watcher
behavior, path/symlink policy, duplicate semantics, thumbnail ownership,
rich-3MF preservation boundaries, and native file selection and drop.

The user decided these questions on 2026-09-23:

1. **Managed is pre-selected** in the import dialog. Linked is one click
   away (D5).
2. **Duplicates** offer three explicit actions, and none is chosen for the
   user (D14):
   - **Add as a new revision of** a chosen Model, pre-filled from a
     same-name match.
   - **Use existing**.
   - **Add as another Model**.

   The old revision is always kept.
3. **Deleting a Model is permanent after a confirmation.** farm3d's stored
   copies are removed once nothing references them. A linked source file is
   never touched, and later phases can block the delete (D18).
4. **Projects are many-to-many.** A Model can belong to any number of
   Projects, or none (Unfiled). Projects are flat, with no nesting, and the
   entity keeps the name "Project" (D1).
5. **Deleting a Project removes only its memberships.** Its Models are never
   deleted. A Model left in no Project becomes Unfiled (D18).
6. **Saved views are five fixed built-ins:** All Models, Unfiled, Recently
   added, Needs attention, and Pre-sliced G-code (D19).

Decision 4 changes the umbrella design's "Projects are organizational
folders" wording and the `CONTEXT.md` **Project** entry. The plan's docs
task updates those documents to "organizational grouping."

Two decisions depend on a spike (Task 1 of the plan) and name their
fallback: D10 (3MF reader) and D15 (watcher behavior). The spike can change
an implementation detail; it cannot change the product behavior described
here.

## Goal

Make the Library durable. A user can:

- Create Projects and add each Model to any number of them, or leave it
  Unfiled.
- Import STL, 3MF, and pre-sliced G-code from a native file picker or by
  dropping files on the window, choosing managed or linked storage per file.
- Resolve duplicate content deliberately.
- See every Model's immutable source history.
- Keep working when a linked file changes, disappears, or comes back, and
  recover a missing link or convert it to managed storage.

Every import and every linked-source content change creates an immutable
Model Source Revision whose exact bytes farm3d keeps, so later changes to the
source can never alter prior work. Imported G-code is inspected and retained
with its provenance, but it is not dispatchable until P5 publishes it as an
external Slice Revision.

## Scope

### In scope

- Projects: create, rename, delete. Models: rename, add to and remove from
  Projects (many-to-many), delete.
- Model Source Revisions with exact retained bytes, provenance, and inspection
  results.
- A content-addressed managed store under the existing `content_root`.
- Native multi-file selection and window drop, with a Rust-owned selection
  registry.
- Format inspection for STL, supported 3MF, and G-code, with fixtures.
- Duplicate detection by content hash, with explicit resolution.
- Rich-3MF reporting: unsupported parts are listed before import and require
  acknowledgment.
- Embedded thumbnail extraction from 3MF and G-code.
- Linked-source watching, reconciliation at startup and on demand, missing,
  unreadable, and invalid-content states, locate, and convert to managed.
- The Library event stream with listen-before-backfill.
- The Library workspace: Project and saved-view navigation, grid and list
  modes, a Model details panel, import, duplicate resolution, and recovery.
- Deterministic `just web` fixtures that do not pretend import works.

### Non-goals

- Rendering geometry, plate preparation, transforms, or rendered thumbnails
  (P5 selects the renderer).
- Slicing, Slice Revisions, or the G-code fact-confirmation flow (P5).
- Queue Entries, **Create Queue Entry**, or dispatch (P7).
- Inferring or storing Printer, nozzle, or material facts from G-code or 3MF
  metadata. P4 stores what the file claims, marked untrusted.
- Writing or re-exporting 3MF files.
- Binary G-code (`.bgcode`), OBJ, STEP, and AMF.
- Backup and restore of managed content, disk-usage management, and pruning
  old revisions (P9).
- Converting a managed Model to linked.
- Proving watcher behavior on Windows or macOS. They remain "Candidate,
  unverified" in `docs/superpowers/baselines/2026-09-16-supported-platforms.md`.

## Product vocabulary

These terms are added to or refined in `CONTEXT.md`:

- **Model** — refined: a 3D file (STL/3MF) *or a pre-sliced G-code file*
  kept in the Library. A G-code Model is inspectable and retained, but it is
  never sliced, and it becomes dispatchable work only as an external Slice
  Revision (P5).
- **Project** — refined: an organizational grouping of Models in the
  Library. A Model may belong to any number of Projects, or none (Unfiled).
  A Project does not carry production quantities, deadlines, or fulfillment
  state.
- **Unfiled** — the state of a Model that belongs to no Project. It is not a
  Project.
- **Managed Model** — a Model whose source is farm3d's own stored copy. It
  changes only when the user adds a revision.
- **Linked Model** — a Model that follows a file path outside farm3d. farm3d
  watches it and captures a new Model Source Revision whenever its content
  changes.
- **Source state** — a Linked Model's current relationship to its file: `ok`,
  `missing`, `unreadable`, `notAFile`, `invalidContent`, or `changing`. It
  never affects existing revisions.
- **Import selection** — a short-lived, Rust-held set of files the user picked
  or dropped. It is not persisted.

## Decisions

### D1. Library entities

- **Project**: `{ id, revision, name }`. The name is trimmed, 1–128
  characters, and unique case-insensitively. Projects are flat: no Project
  contains another.
- **Model**: `{ id, revision, name, projectIds, format, storageMode }`, plus
  link fields when linked. `format` is `stl`, `3mf`, or `gcode`, fixed at
  creation. The name is trimmed and 1–255 characters. Names need not be
  unique; a same-name Model in a Project it shares is a UI warning, as in
  P2.
- **Project membership** is many-to-many (decision 4), stored as
  `project_models(project_id, model_id, added_at)` rows.
  - Membership is a set: adding a Model to a Project it already belongs to
    is a successful no-op, not an error.
  - **Unfiled is derived**, never stored: a Model with no membership row is
    Unfiled. There is no "Unfiled" Project row.
  - Adding or removing membership changes *organization* only. It bumps the
    Model's `revision` (so the frontend's revision guard settles
    correctly), but it creates no Model Source Revision.
  - The ordering of `projectIds` in `ModelRecord` is by Project name
    (case-insensitive), then id, so the UI and tests see a stable order.
- **Model Source Revision**: an immutable row with a per-Model `sequence`
  starting at 1, the content hash, size, provenance, and inspection results.
  The Model's current revision is the one with the highest `sequence`.
- **G-code is a Model format**, not a separate entity. One import pipeline,
  one duplicate rule, one linking rule, and Project membership apply to all
  three formats. P5 creates an external Slice Revision that refers to a
  G-code Model Source Revision.

Alternative considered: a separate "G-code import" entity. It would duplicate
storage, linking, duplicate handling, and Project membership for no product
gain. The Model record makes the difference visible through `format`.

### D2. Model Source Revisions retain exact bytes in both storage modes

Every revision, managed or linked, stores its exact bytes in the managed
content store (D4). A linked source is followed for *new* revisions, but no
revision depends on the linked file remaining.

- This is what lets "prior Model Source Revisions remain valid when linked
  sources change or disappear" hold. Keeping only a hash for linked revisions
  would make a revision unreadable the moment the file changed.
- The cost is disk space: a linked file uses space in farm3d once per
  distinct content version. Identical content is stored once (D4). P9 owns
  disk management and pruning.
- Rows are immutable. A SQLite trigger aborts any `UPDATE` on
  `model_source_revisions`. Rows are deleted only by deleting their Model.
- **Provenance** on every revision: `origin` (`import`, `linkedChange`,
  `relocate`, or `addedRevision`), the source file's basename, the absolute
  path read, the source's modification time, `captured_at`, and the
  inspector version that produced `inspection_json`.
- A Rust API gives P5 verified read access:
  `library::content::open_verified(&ContentStore, &RevisionRef) ->
  Result<VerifiedReader, ContentError>`. It streams the blob and fails with
  `ContentError::HashMismatch` (mapped to `CORRUPT_DATA`) if the bytes no
  longer hash to the recorded value.

### D3. Content hash: SHA-256

Use SHA-256 through the `sha2 = "0.10"` crate, which is already a direct
dependency. It is used today for migration checksums and legacy-import
fingerprints.

- **Evidence.** `sha2` 0.10 detects SHA-NI and ARMv8 SHA instructions at
  runtime through `cpufeatures`. The spike (Task 1) records throughput on the
  Linux x86_64 host over a 512 MiB file. The pass line is ≥ 300 MB/s, which
  keeps a 1 GiB import's hash under about 4 s of the copy it overlaps with.
- **Alternatives.** BLAKE3 (`blake3` 1.8.7) is several times faster but adds
  a dependency and a second hash vocabulary to the codebase. xxHash is not
  collision-resistant and cannot serve as a content identity.
- **Fallback.** If SHA-256 misses the pass line, switch to BLAKE3. The
  on-disk layout names the algorithm (`blobs/sha256/`), so the algorithm can
  change without migrating existing blobs.
- The hash is computed in the same streaming pass that copies the source into
  staging, never in a second read of the live file.

### D4. Managed content layout and write protocol

F1 reserved `content_root = app_data_dir()/farm3d-content/v1/`. P4 uses it
as follows:

```text
<content_root>/
  blobs/sha256/<first 2 hex>/<64 hex>     immutable content, one file per hash
  staging/<selection-id>/<file-index>.part temporary copies being hashed or inspected
```

- **Content-addressed and deduplicated.** Identical bytes are stored once,
  whichever Models or revisions refer to them. Thumbnails are blobs too.
- **Permissions.** Directories are `0700` and blob files `0400` on Unix. On
  Windows, blob files get the read-only attribute. Paths are created through
  the existing `create_contained_directory` checks, so a symlinked
  `blobs` or `staging` directory is rejected.
- **Write protocol (per file):**
  1. Stream the source into `staging/…/<n>.part` while hashing. `fsync` the
     file.
  2. Re-stat the source. If size or modification time changed during the
     copy, discard the staged file and report `SOURCE_CHANGED_DURING_READ`.
  3. Inspect the *staged* bytes (D8–D11), never the live file.
  4. Take the store's **placement lock**. Rename the staged file to its blob
     path if the blob is absent (then `fsync` the directory). If the blob
     already exists and its size matches, delete the staged file. A size
     mismatch on an existing blob is `CORRUPT_DATA`.
  5. In the same critical section, commit the SQLite transaction that inserts
     the `content_blobs` row (if new) and the referencing rows. Release the
     lock.
- **Crash safety.** A blob file may exist without a row (crash between steps
  4 and 5). A row never exists without its file. At startup, before commands
  are served, the store:
  1. Deletes everything under `staging/`.
  2. Retries `pending_blob_cleanup`.
  3. Deletes blob files that have no `content_blobs` row.

  No import can be in flight at startup, because the metadata-root lease is
  held by one process.
- **Cleanup.** When a deletion leaves a `content_blobs` row unreferenced, the
  same transaction deletes that row and inserts a `pending_blob_cleanup` row.
  After commit, the file is unlinked under the placement lock, and the
  cleanup row is removed. Holding the placement lock prevents a concurrent
  import from committing a new row for the same hash between the check and
  the unlink. Failures stay pending and are retried at startup, following the
  `pending_credential_cleanup` pattern.
- **Limits.** A single source file may be at most 1 GiB
  (`MAX_SOURCE_BYTES`). Larger files are rejected with `TOO_LARGE` before
  copying.
- **Backup.** Blobs live outside SQLite, so SQLite snapshots do not include
  them. P9 backs up referenced blobs. The layout lets it copy exactly the
  hashes that `content_blobs` lists.

### D5. Storage modes

- **Managed.** The Model has no path. New revisions come only from **Add as
  a new revision** (D14).
- **Linked.** The Model stores `linked_path` and follows it (D15). Its
  current revision stays current until a content change is captured.
- The mode is chosen per file at import. Managed is pre-selected
  (decision 1).
- **Convert to managed** (`convert_model_to_managed`) clears the link fields
  and stops watching. Because every revision's bytes are already stored (D2),
  conversion copies nothing and works in any source state, including
  `missing`. This is broader than the umbrella wording ("after the source is
  located, Convert to managed copies it"), and nothing is lost by allowing it
  earlier.
- Converting managed to linked is not offered in v1.

### D6. Path and symlink policy

- **What is stored.** The absolute, lexically normalized path the user
  selected (`.` and `..` resolved, no symlink resolution). The Model follows
  that name, so a symlink the user chose keeps working if its target is
  swapped.
- **Reading.** Reads follow symlinks. The target must be a regular file;
  directories, FIFOs, sockets, and devices get `notAFile` (import) or source
  state `notAFile` (linked).
- **Farm3d's own trees are refused.** A path whose canonical form is inside
  `metadata_root` or `content_root` is rejected with `PATH_NOT_ALLOWED`.
- **Identity for change detection.** farm3d records size, modification time
  (nanoseconds), and, on Unix, `(dev, ino)` of the resolved file. A mismatch
  on any of them triggers a hash comparison; the hash decides whether content
  changed. On Windows, the file ID is omitted until that platform is
  verified.
- **Where paths appear.** `ModelRecord.link.path` carries the full path,
  because the user needs it to recognise and recover the source. Errors,
  warnings, and events carry basenames and ids only, following F1's rule.
- **Hard links and network filesystems** are allowed. Network filesystems may
  not deliver watch events; D15's reconciliation covers them.

### D7. Native file selection and drop

No command accepts a raw filesystem path from the frontend. Files enter only
through a Rust-owned native dialog or a Rust-observed window drop. This
continues the `document_io.rs` rule for JSON import/export, so a compromised
webview cannot ask the backend to read arbitrary files.

- **Picker.** `tauri-plugin-dialog` 2.7.3 is already a dependency with
  `dialog:allow-open` granted. A new `ModelFileIo` trait (production:
  `NativeModelFileIo`, tests: deterministic fakes) calls
  `blocking_pick_files()` with the filter `3D models and G-code: stl, 3mf,
  gcode, gco, g` for imports, and `blocking_pick_file()` for Locate.
- **Drop.** Tauri 2.11's `Builder::on_webview_event` delivers
  `WebviewEvent::DragDrop(DragDropEvent::Drop { paths, .. })` to Rust. The
  handler registers the paths as an import selection and emits
  `library.selection.dropped` (D17). The webview's own drag events
  (`getCurrentWebview().onDragDropEvent`) are used only for the hover
  highlight, never for paths.
- **Selection registry.** A selection is
  `{ selectionId: "sel-<uuid>", purpose: import | locate, files: [path] }`,
  held in memory for 30 minutes after its last use. It is discarded on
  cancel, after a complete import, on expiry, and at restart. Commands take
  `selectionId` plus `fileIndex`. An unknown or expired id returns
  `SELECTION_EXPIRED`. A drop holds at most 100 paths; directories in a drop
  are reported as `notAFile`, not walked.
- **Plugins not used.** `tauri-plugin-fs` (in the lockfile only as the dialog
  plugin's dependency) is not added to the app: Rust reads files with
  `std::fs`, so no fs scope or capability is needed. `tauri-plugin-opener`
  is not a picker. Kobalte's `FileField` yields browser `File` objects with
  no path, so it cannot create a linked Model.
- **No capability changes are expected.** Drop visuals use the event API that
  `core:default` already grants. The spike (Task 1, part C) confirms this in
  `just dev`. If a permission turns out to be missing, the plan adds exactly
  that one permission and records why.
- **Web mode (`just web`).** **Import…** and the drop surface are shown
  disabled with "Importing Models needs the desktop app." The Library shows
  deterministic sample Projects and Models, labelled "Sample data (browser
  preview)."

### D8. Format detection and limits

Detection uses content, not the extension:

| Check, in order | Result |
|---|---|
| Starts with `PK\x03\x04` and the ZIP contains `[Content_Types].xml` | 3MF (D10) |
| Starts with `GCDE` | Rejected, `UNSUPPORTED_FORMAT` ("Binary G-code isn't supported yet.") |
| ASCII STL rule (D9) | STL, ASCII |
| Binary STL size rule (D9) | STL, binary |
| The first 64 KiB is valid UTF-8 or Latin-1 text, and at least one line is a G-code command (`G`/`M`/`T` + digits) or a `;` comment | G-code (D11) |
| Otherwise | Rejected, `UNSUPPORTED_FORMAT` |

An extension that disagrees with the detected format adds an
`EXTENSION_MISMATCH` warning; it does not reject the file.

### D9. STL: a farm3d-owned reader

farm3d implements its own STL reader in `library/formats/stl.rs`.

- **Evidence.** `stl_io` 0.11.0 (the most-used STL crate) probes ASCII by
  checking whether the first line starts with `"solid "` and then commits to
  the ASCII parser (`src/lib.rs`, `create_stl_reader`). Binary STLs whose
  80-byte header begins with `solid ` (common from CAD exporters) therefore
  fail to parse. An ASCII file whose first line is exactly `solid` (no name)
  is rejected. The binary reader trusts the triangle count without checking
  the file length. The readers that would let farm3d choose the format itself
  are private modules. `nom_stl` 0.2.2 was last released in 2021.
- **Detection rule.** Binary if `file_len >= 84` and
  `84 + 50 × count <= file_len`, and either the file does not start with
  `solid` or the ASCII rule fails. ASCII if the file starts with `solid`
  after optional whitespace and the first `facet` keyword appears within the
  first 4 KiB. Trailing bytes after the last binary triangle produce a
  `TRAILING_BYTES` warning. A binary file shorter than its declared count is
  `INVALID_CONTENT` (truncated).
- **Output.** `StlInspection { encoding: ascii | binary, solidName?,
  triangleCount, boundsMm: { min: [x,y,z], max: [x,y,z] }, unitsAssumed: true
  }`. STL has no units; millimetres are assumed and marked as such.
- **Rejected content.** Zero triangles, and any non-finite coordinate, are
  `INVALID_CONTENT`.
- **Memory.** The reader streams triangles and keeps only counters and bounds.

### D10. 3MF: `zip` + `quick-xml`, with a defined support boundary

- **Crates.** `zip` 8.6 (`default-features = false`, feature
  `deflate-flate2-zlib-rs`; read-only, no compressor) and `quick-xml` 0.42
  (already in `Cargo.lock` as a transitive dependency; it becomes direct).
- **Alternatives rejected, with evidence.**
  - `threemf` 0.8.0 is write-oriented; its documentation says only "the most
    basic features" exist and reading is not implemented in a usable form.
  - `lib3mf` (pure Rust, 0.1.6) has about 900 downloads and no track record.
  - The C++ lib3mf would add a native build dependency.

  The spike tries `lib3mf` 0.1.6 only as the fallback described below.
- **Evidence for the Production extension requirement.** Real Bambu Studio
  and OrcaSlicer project files on the development host declare
  `requiredextensions="p"` in `3D/3dmodel.model` and keep each object's mesh
  in a separate part (`3D/Objects/*.model`, one of them 19 MB uncompressed)
  that is referenced through `p:path` components and
  `3D/_rels/3dmodel.model.rels`. A root-model-only reader would therefore see
  no geometry in most slicer-produced 3MF files.
- **What the reader supports (the "supported 3MF"):**
  - The OPC package: `[Content_Types].xml`, `_rels/.rels` to find the start
    part, and part relationships.
  - Core 2015/02: `unit`, `<metadata>`, `<object type>`, `<mesh>`
    (vertices and triangles), `<components>`, and `<build>` items with
    transforms.
  - The Production extension (`p`), read-only: `p:path` component references
    to other model parts and `p:UUID` attributes. Parts are resolved within
    the package only; a path that escapes the package root is
    `INVALID_CONTENT`.
  - The Materials extension (`m`) when it is required: geometry is read, and
    colour and material assignments are reported as unsupported.
  - Orca/Bambu plate grouping from `Metadata/model_settings.config`
    (`<plate>` with `plater_id`, `plater_name`, and `<model_instance>` object
    ids), returned as `plates: [{ index, name?, objectIds }]` for P5.
  - Embedded thumbnails (D12).
- **Any other required extension** (for example Beam Lattice or Slice) makes
  inspection fail with `UNSUPPORTED_FORMAT` and `details.extensions`, as the
  3MF core specification requires of a consumer that doesn't implement a
  required extension.
- **Rich-3MF reporting.** Anything outside the supported set is listed in
  `unsupported: [{ code, part, detail }]` before import. Examples:
  - Slicer settings: `Metadata/project_settings.config`,
    `Metadata/Slic3r_PE*.config`.
  - Per-object settings in `model_settings.config` (anything other than plate
    grouping).
  - Layer-height profiles, paint and MMU segmentation attributes
    (`paint_color`, `slic3rpe:mmu_segmentation`).
  - Material and colour groups.
  - Embedded sliced G-code (`Metadata/plate_*.gcode`).

  The exact bytes are always kept (D2), so nothing is lost from the stored
  revision. What is lost is only farm3d *using* those fields: P5 will slice
  from geometry and plates plus farm3d's own settings. The import row says
  exactly that: "Kept in the stored file. farm3d won't use these when
  slicing." Importing a file with any unsupported entry requires
  `acknowledgeUnsupported: true` on its row (D13).
- **Output.** `ThreeMfInspection { unit, producer?, title?, objectCount,
  buildItemCount, triangleCount, boundsMm, plates, requiredExtensions,
  unsupported, thumbnails }`. `boundsMm` is the axis-aligned union of each
  build item's transformed object box, so it is conservative for rotated
  items, and the UI labels it "approximate size".
- **Safety limits.** At most 10,000 ZIP entries; at most 4 GiB decompressed
  in total and 2 GiB per entry; decompressed reads stop at a 1,000:1 ratio
  per entry; XML is streamed (no DOM), and entity expansion is disabled
  (`quick-xml` does not expand external entities). Nothing is extracted to
  disk except the chosen thumbnail blob.
- **Zero objects.** A 3MF with no printable objects (for example an exported
  filament profile) is `INVALID_CONTENT` with "This 3MF contains no objects."
- **Spike gate.** Task 1 must read every 3MF fixture (D20). If the
  Production-extension reader cannot be made to pass, the fallback is to try
  `lib3mf` 0.1.6 against the same fixtures. If that also fails, P4 narrows
  "supported 3MF" to core-only packages and imports Production-extension
  packages with `objectCount: null` plus an `unsupported` entry. The user
  must then approve that narrowing before the plan continues.

### D11. G-code inspection and retention

- **Reader.** A farm3d-owned streaming line scanner in
  `library/formats/gcode.rs`. The `gcode` crate (0.7.0) parses motion
  commands for no-std targets and does not read slicer comment metadata, which
  is what P4 needs.
- **What is extracted:**
  - `producer`: from `; generated by <name> <version>` (OrcaSlicer, Bambu
    Studio, PrusaSlicer, SuperSlicer) or `;Generated with Cura_SteamEngine
    <version>` (Cura).
  - `claims`: an allowlisted map of `key = value` and `key: value` comment
    pairs from the header block, the trailing config block, or Cura's `;KEY:`
    header lines:
    - `printer_model`
    - `printer_settings_id`
    - `nozzle_diameter`
    - `filament_type`
    - `filament_settings_id`
    - `layer_height`
    - `filament used [g]`
    - `filament used [mm]`
    - `estimated printing time (normal mode)`
    - `total layer number`
    - `max_z_height`
    - `bed_temperature`
    - Cura's `FLAVOR`, `TIME`, `Filament used`, and `Layer height`

    Each claim is stored verbatim as a string with its line number. Nothing
    is converted into a Printer Profile, nozzle, or material fact. The record
    is typed `claims: { key, value, line }[]` with a fixed `trusted: false`.
  - Structure: line count, command count, tool numbers used (`T<n>`), whether
    relative extrusion or relative positioning is ever active, and the
    observed extents of `G0`/`G1` X/Y/Z in absolute mode (`observedBoundsMm`,
    omitted if relative positioning makes them ambiguous).
  - Thumbnails from `; thumbnail begin WxH N` … `; thumbnail end` blocks
    (base64 PNG; D12). QOI and JPG thumbnail variants are listed but not
    decoded.
- **Evidence.** A real OrcaSlicer 2.5.0-dev file on the development host has
  `; HEADER_BLOCK_START`, `; generated by OrcaSlicer 2.5.0-dev`, a 640×480
  thumbnail block at line 709, and the trailing config block with
  `printer_model`, `nozzle_diameter`, `filament_type`, `filament used [g]`,
  and `estimated printing time (normal mode)` near line 53,000 of 54,079. The
  scanner must therefore read the whole file, which it does in the same pass
  as hashing.
- **Invalid content.** A file that fails UTF-8 and Latin-1 decoding in its
  first 64 KiB, or has no command lines at all, is `INVALID_CONTENT`. Lines
  over 64 KiB are counted and skipped with a `LONG_LINE` warning.
- **Retention.** A G-code Model's revisions hold the exact bytes (D2,
  including line endings) and the inspection. Nothing in P4 creates a Slice
  Revision, a Queue Entry, or a dispatch control. The Model details panel says
  "Pre-sliced G-code. It can be sent to a Printer once G-code handoff is
  available." and shows the claims under "What the file says (not verified)".
- **Failure and cancellation.** A failed or cancelled inspection or import
  leaves no Model row, no revision row, no blob row, and no staged file
  (D13). The source file is never modified. After a successful import, the
  revision and its bytes are all P5 needs to retry external Slice Revision
  creation any number of times.

### D12. Thumbnails

- **Owner.** Rust extracts and stores thumbnails. The frontend never decodes
  model files.
- **Sources in P4.** Embedded images only:
  - 3MF: the part named by `<metadata name="Thumbnail_Middle">`, then
    `Metadata/thumbnail.png`, then `Metadata/plate_1.png`, in that order.
  - G-code: the largest PNG thumbnail block no bigger than 1024×1024.

  STL has none; the UI shows a format icon.
- **Storage.** One preferred PNG per revision, at most 1 MiB and 1024×1024,
  stored as a blob with a `model_revision_thumbnails` row. The PNG header is
  checked for dimensions; the pixels are not decoded.
- **Delivery.** `get_revision_thumbnail { revisionId }` returns
  `{ mediaType: "image/png", width, height, dataBase64 } | null`. The frontend
  renders it as a `data:` URL. No asset protocol scope or capability change is
  needed.
- **Rendered thumbnails** are P5's, after it selects a renderer. The table
  has a `source` column (`embedded` in P4) so P5 can add `rendered` rows.

### D13. Import pipeline

Import is three commands over one selection:

1. **Select.** `pick_model_files { purpose }` or a window drop creates the
   selection and returns (or emits) `ImportSelectionSummary { selectionId,
   purpose, files: [{ fileIndex, fileName, sizeBytes | null }] }`. Picker
   cancellation returns `null`.
2. **Inspect.** `inspect_import_selection { selectionId }` stages, hashes,
   and inspects every file with at most 2 files in flight. It emits throttled
   `library.import.progress` events (at most one every 250 ms per file) and
   returns `ImportInspection { selectionId, items: ImportCandidate[] }`, where
   each item is either `ready` (format, size, hash, inspection summary,
   unsupported entries, warnings, duplicate matches) or `rejected` (error
   code, message). Calling it again for the same selection returns the stored
   result without re-reading files.
3. **Commit.** `import_models { selectionId, operationId, items:
   ImportItemRequest[] }` commits each requested `ready` item in its own
   transaction (D4 steps 4–5). It returns per-item outcomes:
   - `imported`
   - `revisionAdded`
   - `reusedExisting`
   - `rejected`
   - `cancelled`

   Items commit as they finish, so earlier items stay committed if a later
   one fails.

`ImportItemRequest`:

```text
{ fileIndex, name, projectIds: string[],   // [] = Unfiled; duplicates ignored
  storageMode: "managed" | "linked",
  duplicateAction?: "useExisting" | "addAnother" | "addRevision",
  targetModelId?: string,           // required for useExisting and addRevision
  targetExpectedRevision?: number,  // required for addRevision
  acknowledgeUnsupported: boolean }
```

- **Row validation.** Each item is checked before commit:
  - A `ready` item with content duplicates and no `duplicateAction` is
    rejected with `DUPLICATE_DECISION_REQUIRED` (D14).
  - An item with unsupported 3MF entries and
    `acknowledgeUnsupported: false` is rejected with
    `UNSUPPORTED_NOT_ACKNOWLEDGED`.
  - Any unknown id in `projectIds` is `NOT_FOUND` for that item, and none
    of its memberships are written. More than 64 `projectIds` is
    `VALIDATION`.
  - For `useExisting` and `addRevision`, the row's `projectIds` are *added*
    to the target Model's memberships in the same transaction, never
    removed. That way "import into Brackets, use existing" places the
    existing Model in Brackets too.
  - An empty name is `VALIDATION`.
- **Idempotency.** The registry records each committed item's outcome under
  `(selectionId, operationId, fileIndex)`. Repeating `import_models` with the
  same `operationId` returns the recorded outcomes and commits only items
  that have none. A different `operationId` on a selection that is still
  importing returns `CONFLICT`. The registry is in memory; after a restart,
  the selection is gone and a re-import meets D14's duplicate check instead
  of creating a silent copy.
- **Cancellation.** `cancel_import_selection { selectionId }` signals a
  `watch` channel:
  - An in-flight copy, inspection, or commit stops at its next check. Checks
    happen between 1 MiB read chunks and before the placement lock.
  - Uncommitted items come back as `cancelled`. Committed items stay.
  - Staging for the selection is deleted, and the selection is discarded.
  - Cancelling an unknown selection is a no-op success.
- **Linked items** additionally record the observed stat and start watching
  after commit (D15).
- **Events.** Events go out after each commit: `library.model.changed` and
  `library.revision.created`. Nothing is emitted for a rejected or cancelled
  item. Each commit also emits
  `library.project.changed` for every Project that gained a member.

### D14. Duplicate semantics

- **Identity.** Two files are duplicates when their SHA-256 hashes are equal.
  Names and paths never make content a duplicate.
- **Detection.** Inspection returns `duplicates: [{ modelId, modelName,
  projectIds, revisionId, sequence, isCurrent }]`: every Model with *any*
  revision of the same hash, marking whether it is the current one.
- **Resolution (never silent, decision 2):**
  - **Use existing:** no new rows. The outcome `reusedExisting` names the
    Model, and the UI selects it.
  - **Add as another Model:** a new Model whose first revision refers to the
    same blob. No bytes are stored twice.
  - **Add as a new revision of…** (`addRevision`): appends a revision to the
    target Model (`origin: "addedRevision"`). This is available for any
    `ready` item, not only duplicates. The target must be a managed Model of
    the same format, checked against `targetExpectedRevision`. If the new
    bytes equal the target's current revision, the outcome is
    `reusedExisting` and no revision is added.
- **Same-name suggestion.** When a Model with the same name
  (case-insensitive) belongs to any of the row's chosen Projects, or is
  Unfiled when the row has no Projects, the UI pre-fills it as the
  `addRevision` target. If several match, it pre-fills the most recently
  updated. It never selects the action.
- **Linked Models** receive revisions only from their own source (D15) or
  from Locate (D16). An `addRevision` that targets a linked Model is
  `VALIDATION`.

### D15. Linked-source watching and reconciliation

The watcher is a hint. The authority is a stat-then-hash check,
`check_linked_source(model)`, which runs from four triggers:

1. **Startup.** A background pass over every linked Model after
   `RuntimeServices` is ready. It does not block startup.
2. **A watch event** for the Model's parent directory (debounced).
3. **`check_linked_sources { modelIds? }`.** The frontend calls it when the
   Library becomes visible and when the window regains focus, throttled to
   once per 30 s. The user can also run it from **Check sources**.
4. **A retry timer** for the `changing` state.

**`check_linked_source` algorithm:**

1. `metadata(path)` (following symlinks):
   - `NotFound` → state `missing`.
   - `PermissionDenied` → `unreadable`.
   - Not a regular file → `notAFile`.
   - Larger than 1 GiB → `unreadable`, with `TOO_LARGE`.
2. If the state was `ok` and size, mtime, and file id match the observed
   values, stop. This is the common case and costs one `stat`.
3. Otherwise, stage a copy while hashing (D4 steps 1–2).
   - If the source changed during the copy, set `changing` and schedule
     retries after 2, 4, and 8 s. After the last retry, stay `changing` until
     the next trigger.
4. If the hash equals the current revision's, update the observed stat and
   set `ok`. No revision is created.
5. Otherwise, inspect the staged bytes as the Model's format.
   - If inspection fails, set `invalidContent` and discard the staged copy.
     The current revision stays current.
6. Otherwise, place the blob and, in one transaction, insert the revision
   (`origin: "linkedChange"`), update the observed stat, and set `ok`.
   Emit `library.model.changed` and `library.revision.created`.

State changes that create no revision update only `link_state`,
`link_checked_at`, and the observed stat, bump the Model's `revision`, and
emit `library.model.changed`. A file that reappears at its path returns to
`ok` on the next trigger, with a new revision if its content differs. No user
action is needed.

**Watcher.** `notify` 8.2 with `notify-debouncer-full` 0.7:

- **Parent-directory watches.** One `NonRecursive` watch per distinct parent
  directory of a linked path, shared and reference-counted across Models.
  Watching the parent (not the file) survives the temp-file-plus-rename saves
  that editors and slicers use, which replace the inode that a file watch
  would follow. `notify-debouncer-full` 0.7 also stitches rename pairs by
  file id.
- **Debounce.** A 750 ms debounce. Any event in a watched directory triggers
  `check_linked_source` for every linked Model in that directory, rather than
  trusting event kinds or path matching. The check is cheap in the unchanged
  case (step 2).
- **Missing parent directory.** If the parent directory does not exist, the
  Model is `missing`. The supervisor watches the nearest existing ancestor,
  non-recursively, until the parent reappears.
- **Watch registration failure.** If a watch cannot be registered (for
  example, the inotify `max_user_watches` limit), that directory falls back
  to a `notify::PollWatcher` at 10 s intervals.
- **Surfacing the watch mode.** `ModelRecord.link.watchMode` reports
  `watching`, `polling`, or `notWatched` (runtime only, not persisted). For
  `polling`, the UI shows "Changes are checked every N seconds", with N
  from the active poll interval.
- **Network filesystems.** Event delivery is not assumed. Triggers 1 and 3
  still apply.

**Spike gate (Task 1, part B), on Linux x86_64 only** (the only Supported
platform). Each scenario must produce exactly one resulting check within
2 s, over 20 repetitions:

- An in-place overwrite.
- A write to a temp file then rename over the target (the Orca and editor
  save pattern).
- Delete.
- Rename away, then rename back.
- Removing and recreating the parent directory.
- A symlinked path whose target file changes in the same directory.

A symlink target in *another* directory is expected not to fire; it is
covered by the triggers 1 and 3 and documented. If any other scenario fails,
the fallback is `PollWatcher` for all linked sources at 5 s, and
`watchMode` reports `polling`. The spike result is recorded in the plan's
evidence file either way. Windows and macOS compile with notify's native
backends but make no behavior claim.

### D16. Missing-link recovery

- **Automatic recovery.** A file restored at the same path recovers on its
  own (D15).
- **Locate source…** (`locate_linked_source { modelId, expectedRevision,
  selectionId, fileIndex, acceptDifferentContent }`). The selection comes
  from `pick_model_files { purpose: "locate" }`. The located file is staged,
  hashed, and inspected.
  - **Different format:** `VALIDATION`.
  - **Same hash as the current revision:** update `linked_path` and the
    observed stat, set `ok`, and restart watching. No revision is created.
  - **Different hash and `acceptDifferentContent: false`:**
    `SOURCE_CONTENT_DIFFERS`, with `details { currentSha256, locatedSha256,
    locatedFileName }`. The UI explains that the located file differs and
    offers **Relink and import as a new revision**.
  - **Different hash and `acceptDifferentContent: true`:** relink and add a
    revision with `origin: "relocate"`.
- **Convert to managed** (D5), available in every source state.
- **No Model state invalidates revisions.** Prior revisions remain readable
  and hash-verified in every source state.

### D17. Events, backfill, and the shared channel

Library events use the existing `farm3d-event-v1` channel and
`EventEnvelope`, on their own `library` stream: a per-process `streamId`
and a sequence, following P3's `InventoryStream` pattern.

| Type | Subject | Payload | In backfill? |
|---|---|---|---|
| `library.project.changed` | `project/<id>` | `ProjectRecord` | yes |
| `library.project.removed` | `project/<id>` | `{}` | yes (absence) |
| `library.model.changed` | `model/<id>` | `ModelRecord` (includes `projectIds`) | yes |
| `library.model.removed` | `model/<id>` | `{}` | yes (absence) |
| `library.revision.created` | `model/<id>` | `ModelSourceRevisionSummary` | via `ModelRecord.currentRevision` |
| `library.selection.dropped` | `selection/<id>` | `ImportSelectionSummary` | no (ephemeral) |
| `library.import.progress` | `selection/<id>` | `{ fileIndex, bytesDone, bytesTotal }` | no (ephemeral) |

- **Backfill.** `list_library` returns `LibrarySnapshot { streamId,
  snapshotSequence, projects, models }`. It reads the sequence *before* it
  reads rows, so a change the snapshot misses always has a larger sequence.
  The frontend listens before backfilling, buffers, applies the snapshot, and
  replays buffered events with a larger sequence. It ignores a
  `library.model.changed` whose `revision` is not newer than the one it
  holds.
- **When events go out.** Events are emitted after commit only, never inside
  a transaction and never for a replay. Ephemeral events consume sequence
  numbers, so gap detection still works.
- **Shared-channel demultiplexing (cross-phase fix).** Today
  `printer-store.ts` passes *every* `farm3d-event-v1` payload to the Printer
  status store. The status store treats a foreign `streamId` as a stream
  change and forces a status backfill. P3's inventory stream already has this
  problem, and P4 would add to it.
  - P4 wraps each subscription with a type-prefix filter: Printer status
    accepts `printer.status.*`, and the Library store accepts `library.*`.
  - Unit tests prove that a foreign-stream event causes no backfill.
  - If P3 has merged its own filter, P4 keeps whichever is stricter and does
    not add a second filter.

### D18. Deletion and cleanup

- **`delete_model { id, expectedRevision }`**, following decision 3:
  1. Evaluate `ModelDeletionBlocker` sources inside the transaction. P4
     registers none; P5 and P7 add "referenced by a Slice Revision, Queue
     Entry, or Job." A blocked delete returns `LIFECYCLE_BLOCKED` with the
     blockers.
  2. Delete the Model. Its revisions, thumbnail rows, and `project_models`
     rows cascade.
  3. Unreferenced blobs follow D4's cleanup.
  4. After commit, stop watching and emit `library.model.removed`, then
     `library.project.changed` for each Project whose `modelCount` dropped.
  5. The linked source file is never touched.

  The UI confirms with "Delete <name>? Its imported revisions are deleted.
  The original file on disk is not." There is no typed-name confirmation;
  that is reserved for Printers.
- **`delete_project { id, expectedRevision }`**, following decision 5. In
  one transaction:
  1. Collect the ids of Models with a membership in the Project.
  2. Bump each of those Models' `revision` and `updated_at`.
  3. Delete the Project. Its `project_models` rows cascade.

  After commit, emit `library.model.changed` for each affected Model (its
  `projectIds` no longer lists the Project; it is Unfiled if that was its
  only Project), then `library.project.removed`. The result is
  `{ deletedId, affectedModelIds, nowUnfiledModelIds }`. No Model, revision,
  or blob is deleted. The confirmation reads "Delete <Project>? Its 3 Models
  stay in the Library. 1 of them will become Unfiled."
- **Startup reconciliation** of content is D4's startup sweep.

### D19. Workspace, saved views, and navigation

- **Layout.** This follows the umbrella layout: navigation on the left, then
  the Model grid or list, then a Model details panel on the right.
  - The right panel shows details only. P4 removes the placeholder **Slice**
    and **Dispatch** buttons and the Target Printer select; P5 adds the
    preparation panel.
  - `BuildPlate` stays a static placeholder showing the selected Model's
    name and approximate size.
  - Viewport state stays inside viewport components and is never part of
    `library-store`.
- **Saved views** (decision 6) are frontend filters over the store:
  - All Models.
  - Unfiled: `projectIds` is empty.
  - Recently added: current revision captured within 14 days.
  - Needs attention: linked and not `ok`.
  - Pre-sliced G-code.

  The sidebar lists the views, then the Projects alphabetically, each with a
  count. A Project's view shows every Model whose `projectIds` contains it,
  so one Model appears under each of its Projects. View counts therefore
  don't sum to the All Models count, and the UI never shows such a sum. The
  active view is display state and is remembered in `localStorage`.
- **Grid and list.**
  - **Grid:** cards with the thumbnail (or format icon), name, format, and a
    storage/source-state badge.
  - **List:** `DataTable` (from P3) with these columns: Name, Projects
    (comma-separated names, truncated with a `+N` count), Format, Storage,
    Source, Revisions, Added.
  - If P3's `DataTable` has not merged, a screen-local table following
    `BatchRowsTable` is used and swapped later. The mode is remembered in
    `localStorage`.
- **Sort.** Name, or Recently added. Search matches Model name, Project name,
  and linked file name.
- **Membership editing.** The details panel lists the Model's Projects as
  removable chips, plus an **Add to Project…** `Combobox` (with a **New
  Project…** entry). Grid cards and list rows have a `DropdownMenu` with
  **Add to Project** and **Remove from <current Project>**; the second
  appears only inside a Project view.
- **Deep links.** `#nav=v1/library/project/<id>` opens that Project.
  `#nav=v1/library/model/<id>` selects that Model. The current view is kept
  if it contains the Model; otherwise the view switches to All Models,
  because a Model may be in several Projects and none is "its" Project. `App.tsx`'s navigation context adds Library ids when the
  destination is `library`. An unknown id shows the existing "no longer
  available" banner.

### D20. Fixtures

Committed under `src-tauri/tests/fixtures/library/`. Every fixture is either
generated by the ignored Rust test `regenerate_library_fixtures` in
`src-tauri/tests/library_fixtures.rs` (run through a new
`just gen-library-fixtures` recipe), or exported by an installed slicer from
the generated cube. No personal or third-party model is committed.

| Fixture | Source | Proves |
|---|---|---|
| `cube-ascii.stl`, `cube-binary.stl` | generated | both encodings, 12 triangles, 10 mm bounds |
| `cube-binary-solid-header.stl` | generated | a binary STL whose header starts with `solid ` |
| `cube-ascii-bare-solid.stl` | generated | ASCII whose first line is `solid` |
| `truncated-binary.stl`, `nan.stl`, `empty.stl` | generated | `INVALID_CONTENT` |
| `core-two-objects.3mf` | generated | core mesh, components, build transforms, metadata, thumbnail |
| `orca-two-plates.3mf` | OrcaSlicer (flatpak `com.orcaslicer.OrcaSlicer`) export of two generated cubes on two plates | Production-extension parts, plates, unsupported settings list |
| `prusa-project.3mf` | PrusaSlicer 2.9.6 (flatpak) export | `Slic3r_PE` config reported unsupported |
| `required-beam-lattice.3mf` | generated | `UNSUPPORTED_FORMAT` for a required extension |
| `zip-slip.3mf`, `no-objects.3mf` | generated | a `p:path` escaping the package; zero objects |
| `orca-cube.gcode` | OrcaSlicer slice of the generated cube | header, thumbnail, trailing config claims |
| `prusa-cube.gcode` | PrusaSlicer slice | PrusaSlicer claims |
| `cura-style.gcode` | generated in Cura's documented header style | Cura claims |
| `plain.gcode` | generated | no metadata; empty claims |
| `binary.bgcode` | generated `GCDE` header | `UNSUPPORTED_FORMAT` |

Each fixture has a sibling `*.expected.json` holding its expected
inspection. The fixture tests compare against it exactly (bounds within
1e-4 mm).

## Backend model

### Module layout

`src-tauri/src/library/`:

- `mod.rs`: domain types (`ProjectRecord`, `ModelRecord`, `ModelFormat`,
  `StorageMode`, `SourceState`, and the revision types) and validation.
- `repository.rs`: SQL for projects, models, revisions, and thumbnails.
- `content.rs`: `ContentStore` (D4), `open_verified`, and the startup sweep.
- `formats/mod.rs`: detection (D8) and `inspect(bytes_path, format) ->
  Result<Inspection, InspectError>`.
- `formats/stl.rs`, `formats/threemf.rs`, `formats/gcode.rs`, and
  `formats/png.rs` (header-only dimension check).
- `selection.rs`: the `ModelFileIo` trait, `NativeModelFileIo`, and the
  selection registry.
- `import.rs`: D13 and D14.
- `links.rs`: `LinkSupervisor` and `check_linked_source` (D15, D16).
- `events.rs`: `LibraryStream` and `publish` (D17).
- `blockers.rs`: the `ModelDeletionBlocker` registry (D18).
- `commands.rs`: Tauri commands.

`RuntimeServices` gains `library: Arc<LibraryServices<R>>`, which holds
`content`, `stream`, `selections`, `links`, and `file_io`. `Storage` gains
`pub fn paths(&self) -> &StoragePaths` so the library can reach
`content_root`.

### Crates

`src-tauri/Cargo.toml` gains:

```toml
notify = "8.2"
notify-debouncer-full = "0.7"
zip = { version = "8.6", default-features = false, features = ["deflate-flate2-zlib-rs"] }
quick-xml = "0.42"
base64 = "0.22"
```

- `quick-xml` 0.42.0 and `base64` 0.22.1 are already in `Cargo.lock`
  transitively.
- `zip` 8.6 needs Rust 1.88 and `notify-debouncer-full` 0.7 needs 1.85. The
  development host has rustc 1.98.0.
- No new Tauri plugin is added.

### Migration `0005_p4_library.sql`

The number is set at rebase time: `0005` if P3's `0004` merges first (the
expected order), otherwise `0004`. `CURRENT_SCHEMA_VERSION` moves with it.
This migration has no Rust post-step.

```sql
CREATE TABLE library_projects (
  id TEXT PRIMARY KEY CHECK (id GLOB 'prj-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL CHECK (revision >= 1),
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 128 AND name = trim(name)),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX library_projects_name ON library_projects(lower(name));

CREATE TABLE content_blobs (
  sha256 TEXT PRIMARY KEY CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
  size_bytes INTEGER NOT NULL CHECK (size_bytes BETWEEN 0 AND 4294967296),
  created_at TEXT NOT NULL
) STRICT;

CREATE TABLE library_models (
  id TEXT PRIMARY KEY CHECK (id GLOB 'mdl-*' AND length(id) BETWEEN 5 AND 64),
  revision INTEGER NOT NULL CHECK (revision >= 1),
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 255 AND name = trim(name)),
  format TEXT NOT NULL CHECK (format IN ('stl', '3mf', 'gcode')),
  storage_mode TEXT NOT NULL CHECK (storage_mode IN ('managed', 'linked')),
  linked_path TEXT,
  link_state TEXT CHECK (link_state IS NULL OR link_state IN
    ('ok', 'missing', 'unreadable', 'notAFile', 'invalidContent', 'changing')),
  link_checked_at TEXT,
  link_observed_size INTEGER,
  link_observed_mtime_ns INTEGER,
  link_observed_file_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK (
    (storage_mode = 'managed' AND linked_path IS NULL AND link_state IS NULL)
    OR (storage_mode = 'linked' AND linked_path IS NOT NULL AND length(linked_path) > 0
        AND link_state IS NOT NULL)
  )
) STRICT;
CREATE INDEX library_models_linked ON library_models(storage_mode) WHERE storage_mode = 'linked';

CREATE TABLE project_models (
  project_id TEXT NOT NULL REFERENCES library_projects(id) ON DELETE CASCADE,
  model_id TEXT NOT NULL REFERENCES library_models(id) ON DELETE CASCADE,
  added_at TEXT NOT NULL,
  PRIMARY KEY (project_id, model_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX project_models_model ON project_models(model_id);

CREATE TABLE model_source_revisions (
  id TEXT PRIMARY KEY CHECK (id GLOB 'msr-*' AND length(id) BETWEEN 5 AND 64),
  model_id TEXT NOT NULL REFERENCES library_models(id) ON DELETE CASCADE,
  sequence INTEGER NOT NULL CHECK (sequence >= 1),
  content_sha256 TEXT NOT NULL REFERENCES content_blobs(sha256),
  size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
  format TEXT NOT NULL CHECK (format IN ('stl', '3mf', 'gcode')),
  origin TEXT NOT NULL CHECK (origin IN ('import', 'linkedChange', 'relocate', 'addedRevision')),
  source_file_name TEXT NOT NULL CHECK (length(source_file_name) BETWEEN 1 AND 1024),
  source_path TEXT NOT NULL,
  source_mtime TEXT,
  captured_at TEXT NOT NULL,
  inspector_version INTEGER NOT NULL CHECK (inspector_version >= 1),
  inspection_json TEXT NOT NULL CHECK (json_valid(inspection_json)
                                       AND json_type(inspection_json) = 'object'),
  UNIQUE (model_id, sequence)
) STRICT;
CREATE INDEX model_source_revisions_content ON model_source_revisions(content_sha256);

CREATE TRIGGER model_source_revisions_immutable
BEFORE UPDATE ON model_source_revisions
BEGIN
  SELECT RAISE(ABORT, 'model source revisions are immutable');
END;

CREATE TABLE model_revision_thumbnails (
  revision_id TEXT PRIMARY KEY REFERENCES model_source_revisions(id) ON DELETE CASCADE,
  source TEXT NOT NULL CHECK (source IN ('embedded')),
  origin_part TEXT NOT NULL,
  media_type TEXT NOT NULL CHECK (media_type = 'image/png'),
  width INTEGER NOT NULL CHECK (width BETWEEN 1 AND 1024),
  height INTEGER NOT NULL CHECK (height BETWEEN 1 AND 1024),
  content_sha256 TEXT NOT NULL REFERENCES content_blobs(sha256)
) STRICT;
CREATE INDEX model_revision_thumbnails_content ON model_revision_thumbnails(content_sha256);

CREATE TABLE pending_blob_cleanup (
  sha256 TEXT PRIMARY KEY CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
  attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
  last_error_code TEXT,
  created_at TEXT NOT NULL,
  last_attempt_at TEXT
) STRICT;
```

Notes:

- `project_models` is the whole of Project membership (D1). It cascades from
  both sides: deleting a Project removes only its membership rows (decision
  5), and deleting a Model removes its own. The composite primary key makes
  a duplicate membership impossible. Rust uses `INSERT OR IGNORE` for the
  set-add semantics.
- `content_blobs` has no `ON DELETE` from its referrers. Cleanup deletes its
  rows explicitly (D4), and the default `RESTRICT` guards against deleting a
  blob that is still referenced.
- The immutability trigger blocks `UPDATE` but not the cascade `DELETE` from
  `library_models`, which is the only permitted way revisions disappear. P5
  and P7 foreign keys to `model_source_revisions` use `RESTRICT`, and their
  deletion blockers (D18) report the reason before SQLite would.
- Migration tests cover the following, following `p2_migration.rs`:
  - A fresh database at the new version, with its ledger row.
  - An upgrade from the previous version with Printers (and, after the
    rebase, Spools) unchanged.
  - Crash-boundary rollback.
  - The CHECK and trigger constraints.
  - A duplicate `project_models` row is rejected, and both cascades hold.

### Domain and wire types

All types are ts-rs exported under `domain/` or `command/`:

```text
ProjectRecord { id, revision, name, modelCount, createdAt, updatedAt }   // modelCount = membership rows

ModelRecord {
  id, revision, name, projectIds: string[],   // [] = Unfiled; ordered by Project name
  format: "stl" | "3mf" | "gcode",
  storageMode: "managed" | "linked",
  link: { path, state: SourceState, checkedAt: string | null,
          watchMode: "watching" | "polling" | "notWatched" } | null,
  currentRevision: ModelSourceRevisionSummary,
  revisionCount, createdAt, updatedAt }

ModelSourceRevisionSummary {
  id, modelId, sequence, sha256, sizeBytes, format, origin,
  sourceFileName, capturedAt, hasThumbnail,
  summary: InspectionSummary }

InspectionSummary =
  | { format: "stl", triangleCount, boundsMm, unitsAssumed: true }
  | { format: "3mf", objectCount, plateCount, triangleCount, boundsMm, unsupportedCount }
  | { format: "gcode", producer: { name, version } | null, lineCount,
      claimedPrinterModel: string | null, claimedEstimatedTime: string | null }

ModelSourceRevisionRecord = ModelSourceRevisionSummary & {
  sourcePath, sourceMtime, inspectorVersion, inspection: Inspection, warnings }

Inspection = StlInspection | ThreeMfInspection | GcodeInspection   // tagged by "format"
```

`claimedPrinterModel` and `claimedEstimatedTime` are verbatim claim values,
for list display only. The UI labels them "claimed".

### Commands

Each command is added to `lib.rs` `COMMAND_NAMES` and `generate_handler!`,
to `contracts/inventory.rs` `COMMAND_CONTRACTS` and the `CommandContracts`
declaration and visitor, and to `src/ipc/client.ts` `CommandMap`. Counts are
asserted as "the previous count plus P4's 17", not as a hard-coded total,
because P3 merges first.

| Command | Args → Result |
|---|---|
| `list_library` | `{}` → `LibrarySnapshot` |
| `create_project` | `{ name }` → `ProjectMutationResult { project }` |
| `rename_project` | `{ id, expectedRevision, name }` → `ProjectMutationResult` |
| `delete_project` | `{ id, expectedRevision }` → `DeleteProjectResult { deletedId, affectedModelIds, nowUnfiledModelIds }` |
| `update_model` | `{ id, expectedRevision, patch: { name? } }` → `ModelMutationResult { model, warnings }` |
| `set_model_projects` | `{ modelId, expectedRevision, add: string[], remove: string[] }` → `ModelMutationResult` |
| `delete_model` | `{ id, expectedRevision }` → `DeleteModelResult { deletedId, warnings }` |
| `list_model_revisions` | `{ modelId }` → `ModelSourceRevisionRecord[]` (newest first) |
| `get_revision_thumbnail` | `{ revisionId }` → `RevisionThumbnail \| null` |
| `pick_model_files` | `{ purpose: "import" \| "locate" }` → `ImportSelectionSummary \| null` |
| `inspect_import_selection` | `{ selectionId }` → `ImportInspection` |
| `import_models` | `{ selectionId, operationId, items }` → `ImportModelsResult { items: ImportItemResult[] }` |
| `cancel_import_selection` | `{ selectionId }` → `CancelImportSelectionData {}` |
| `check_linked_sources` | `{ modelIds?: string[] }` → `ModelRecord[]` (only those that changed) |
| `locate_linked_source` | `{ modelId, expectedRevision, selectionId, fileIndex, acceptDifferentContent }` → `ModelMutationResult` |
| `convert_model_to_managed` | `{ modelId, expectedRevision }` → `ModelMutationResult` |
| `library_content_info` | `{}` → `{ blobCount, totalBytes, pendingCleanupCount }` |

`set_model_projects` is the one membership command:

- In one transaction, it inserts the `add` memberships (`INSERT OR IGNORE`)
  and deletes the `remove` memberships.
- It checks `expectedRevision` against the Model and bumps its revision
  only if the membership set actually changed. A call that changes nothing
  returns the unchanged Model and emits no event.
- An id in both `add` and `remove` is `VALIDATION`. An unknown Project id is
  `NOT_FOUND`, and nothing is written.
- Removing the last membership makes the Model Unfiled; that is not an
  error.
- It emits one `library.model.changed` after commit, plus
  `library.project.changed` for each Project whose `modelCount` changed.

Multi-Project membership at import goes through `ImportItemRequest.projectIds`
(D13). Grid and list menus call `set_model_projects` for one Model at a
time. Bulk multi-select editing is not in P4.

`library_content_info` feeds the Library status line ("Stored copies:
1.2 GB") and is P9's starting point for disk management.

- **`ErrorCode` gains:**
  - `SELECTION_EXPIRED` (retryable false, recovery `[Reload]`).
  - `SOURCE_CONTENT_DIFFERS` (details as in D16).
  - `SOURCE_UNAVAILABLE` (Locate, when the chosen file cannot be read).
  - `UNSUPPORTED_FORMAT` (details `{ reason, extensions? }`).
- **`ImportItemErrorCode`:**
  - `VALIDATION`
  - `NOT_FOUND`
  - `CONFLICT`
  - `UNSUPPORTED_FORMAT`
  - `INVALID_CONTENT`
  - `TOO_LARGE`
  - `NOT_A_FILE`
  - `PATH_NOT_ALLOWED`
  - `SOURCE_UNREADABLE`
  - `SOURCE_CHANGED_DURING_READ`
  - `DUPLICATE_DECISION_REQUIRED`
  - `UNSUPPORTED_NOT_ACKNOWLEDGED`
  - `PERSISTENCE_UNAVAILABLE`
  - `CANCELLED`
- **`ImportWarningCode`:**
  - `EXTENSION_MISMATCH`
  - `TRAILING_BYTES`
  - `LONG_LINE`
  - `DUPLICATE_NAME`
  - `WATCH_UNAVAILABLE`
  - `THUMBNAIL_SKIPPED`

  Error messages and warnings carry basenames only.
- **Revision conflicts** use P2's `CONFLICT` shape (`classify_entity_write`).
- **Model deletion blocks** use the existing `LIFECYCLE_BLOCKED`.

## Frontend architecture

### State

- **`src/library/types.ts`:** re-exports of generated types, plus
  `SavedViewId`.
- **`src/library/library-store.ts`:** the only owner of Library data.
  - Loads with listen-before-backfill on `library.*` events.
  - Holds `projects`, `models`, `syncState`, `revisionsByModel` (lazy), and
    `thumbnails` (a lazy cache keyed by revision id).
  - Actions settle from command results:
    - `createProject`, `renameProject`, `deleteProject`
    - `updateModel`, `setModelProjects`, `deleteModel`
    - `loadRevisions`, `loadThumbnail`
    - `checkSources`, `locateSource`, `convertToManaged`
  - Mutations report into a store error signal shown in a Library banner,
    following the printer-store pattern. Import and locate calls reject
    instead, because their dialogs render errors inline.
- **`src/library/import-flow.ts`:** a pure reducer for the import dialog's
  rows. It covers selection → inspection → per-row choices → request
  building → merging results by `fileIndex`, including the same-name
  pre-fill (D14) and the acknowledgment gating (D10). The dialog keeps its
  rows as component-local state.
- **`src/library/saved-views.ts`:** pure view filtering, search, sort, and
  counts (D19).
- **`src/library/web-fixtures.ts`:** web mode data. Two Projects ("Brackets",
  "Calibration"), five Models (managed STL in both Projects, linked 3MF
  `ok`, linked STL `missing`, G-code with claims, and one Unfiled), and
  embedded 1×1 thumbnails.
  - In web mode, Project and Model edits change local state.
  - `pickFiles`, `inspect`, `import`, `locate`, and `checkSources` throw
    "needs the desktop app".
- **`src/printers/printer-store.ts`:** its listener filters to
  `printer.status.` types (D17).

### Components

**Design-system additions** (added to `components/index.ts`,
`components.test.tsx`, and `Showcase.tsx`):

- **`FileDropSurface`**: the umbrella's "file drop/import surface" pattern.
  - Props: `{ active: boolean; disabled?: boolean; disabledReason?: string;
    onChoose: () => void; label; hint }`.
  - It renders the drop affordance and a real **Choose files…** `Button`, so
    it is keyboard-operable without dragging.
  - The caller supplies `active` from the Tauri drag events. The surface
    never reads browser `File` objects.
  - Reuse: Settings restore (P9) and G-code import.
- **`SegmentedControl`**: wraps Kobalte `segmented-control` for the grid/list
  toggle, with icon-plus-label options. Reuse: Monitor density (P1's control
  can migrate later) and P5 plate tabs.

**Screens** (`src/screens/`, replacing `ModelLibrary.tsx`):

- **`LibraryWorkspace.tsx`**: the layout, the toolbar, and wiring to the
  navigation store and `library-store`.
  - **Toolbar:** **Import…**, **New Project**, search, sort, the grid/list
    `SegmentedControl`, **Check sources**, and the stored-copies size.
- **`LibrarySidebar.tsx`**: the saved views, then the Projects with counts.
  - Each Project row has a `DropdownMenu` with Rename… and Delete….
  - It is a `nav` of buttons with `aria-current`.
- **`ModelGrid.tsx`** and **`ModelList.tsx`**: the two modes (D19). Both
  support multi-select-free single selection by click, Enter, or Space.
- **`ModelDetailsPanel.tsx`**, showing:
  - Name (editable), and Projects as removable chips with **Add to
    Project…** (D19). "Unfiled" is shown when there are none.
  - Format.
  - Storage, with the source state shown as a `SeverityMarker`, icon, text,
    and colour, plus the path and the polling notice when polling.
  - The current revision summary.
  - For 3MF, the "Not used by farm3d" list; for G-code, the claims list.
  - The revision history (`Timeline` from P3, or a screen-local ordered list
    until it merges).
  - Actions: **Locate source…**, **Convert to managed**, and **Delete…**.
- **`ImportDialog.tsx`**, with three steps shown with the existing `Stepper`:
  1. **Files:** the selection summary, with per-file progress while
     inspecting and **Cancel**.
  2. **Review:** one row per file. It shows the detection result, size, and
     any warnings, and offers:
     - A name field and a Projects multi-select `Combobox` (Kobalte
       `Combobox` with `multiple`). It defaults to the Project currently
       being viewed, or none when a saved view is active. None means
       Unfiled. An **Apply Projects to all rows** action copies one row's
       Projects to every row.
     - A Managed/Linked `RadioGroup` (Managed pre-selected, decision 1).
     - Duplicate resolution: a `RadioGroup` with **Use existing** / **Add as
       another Model** / **Add as a new revision of…** plus a Model
       `Combobox`. Nothing is pre-selected, and the row can't be imported
       until one is chosen.
     - For rich 3MF, an "Unsupported contents" disclosure with the
       acknowledgment `Checkbox`.
     - For G-code, the line "Stored as pre-sliced G-code. It won't be
       sliced."

     Rejected rows show their reason and are excluded.
  3. **Results:** the per-row outcome. Failed rows are kept with
     **Retry**, which re-submits them with a new `operationId` against the
     same selection while it lives. **Done** selects the first imported
     Model.
- **`LocateSourceDialog.tsx`**: explains the missing source, calls
  `pick_model_files({ purpose: "locate" })`, then `locate_linked_source`. It
  handles `SOURCE_CONTENT_DIFFERS` with **Relink and import as a new
  revision**.
- **`ProjectDialogs.tsx`**: create, rename, and delete (the delete states
  the D18 wording: the Models stay, and N become Unfiled).
- **`DeleteModelDialog.tsx`**: the D18 wording.
- **Window drop:** `App.tsx` registers the webview drag listener once. Enter
  and leave toggle a global `dropActive` signal. On
  `library.selection.dropped`, the app navigates to Library and opens
  `ImportDialog` with that selection.

## Errors and recovery

| Situation | Behavior |
|---|---|
| Picker cancelled | Nothing happens; no selection is created. |
| Unsupported or invalid file in a selection | Its row is `rejected` with the reason; the other rows continue. |
| Content duplicate, no choice made | The row can't be imported; the backend rejects with `DUPLICATE_DECISION_REQUIRED` if it is forced. |
| Rich 3MF not acknowledged | The row can't be imported; the backend rejects with `UNSUPPORTED_NOT_ACKNOWLEDGED`. |
| Source changes while it is being read | The row is rejected with `SOURCE_CHANGED_DURING_READ`, with **Retry**. |
| Cancel during inspection or import | Committed rows stay; the rest are `cancelled`; staging is removed. |
| Crash during import | At startup, staging and orphan blobs are swept; committed Models are intact. |
| Selection expired (e.g. dialog left open 30 min) | `SELECTION_EXPIRED`; the dialog offers **Choose files again**. |
| Linked source edited | A new revision is captured automatically; the details panel shows "Updated from source". |
| Linked source deleted or moved | Source state `missing` (warning marker); revisions still open; **Locate source…** or **Convert to managed**. |
| Linked source mid-write | `changing`, retried; the current revision stays current. |
| Linked source now invalid | `invalidContent`; the current revision stays current; fixing the file recovers it. |
| Located file differs | `SOURCE_CONTENT_DIFFERS`; **Relink and import as a new revision** or cancel. |
| Blob hash mismatch on read | `CORRUPT_DATA`; the Model shows "Stored copy is damaged". P4 offers no repair; Locate for linked Models imports a fresh revision. |
| Revision conflict | Existing `CONFLICT` handling: reload and retry. |
| Watch limit reached | Polling fallback; `watchMode: polling` shown. |
| Library stream uncertain | Keep content, mark it stale, backfill with backoff as the status store does. |

## Accessibility and adaptation

- **Keyboard access.** Every action is keyboard-operable. Import is always
  reachable through **Import…** and the `FileDropSurface` button; dragging is
  never required.
- **Focus order:** activity rail, then toolbar, then sidebar, then
  grid/list, then details panel.
- **Grid** items are buttons in a list with `aria-selected` via
  `aria-pressed`. **List** mode uses `DataTable`'s row selection.
- **Source state** always uses the `SeverityMarker` icon, text, and colour.
- **Import dialog.** It uses `Stepper` with `aria-current="step"`. Per-row
  progress uses `Progress` with a text value. Outcomes use icon and text.
- **Motion.** The drop highlight has no animation under reduced motion.
- **Window sizes.** At 1440 × 900, all three panes are shown. At 1024 × 700,
  the details panel becomes an overlay toggled by **Details**, following the
  umbrella's dock rule. The import dialog's rows scroll inside the dialog,
  and list mode scrolls horizontally inside its pane.

## Acceptance criteria

1. The migration applies, is ledgered, survives crash-boundary injection, and
   leaves Printer (and P3) data unchanged. Revisions reject `UPDATE`.
2. **The fixture suite passes:** every D20 fixture's inspection equals its
   `*.expected.json`, and every rejection fixture returns its code.
3. **Content store:**
   - Identical content is stored once.
   - A blob without a row is swept at startup.
   - A row never exists without its file (crash injection between placement
     and commit).
   - Cleanup after `delete_model` removes only unreferenced blobs and
     survives a failed unlink through `pending_blob_cleanup`.
4. **Managed and linked imports survive restart** with identical hashes, and
   linked Models resume watching.
5. **Source changes.** Editing a linked file creates revision 2, and
   revision 1's bytes still verify.
   - Deleting the file sets `missing`; restoring it sets `ok` without a new
     revision when the content is unchanged.
   - A mid-write change sets `changing` and then settles.
6. **Recovery.**
   - Locating a moved file with the same content relinks without a revision.
   - Locating a different file requires `acceptDifferentContent`, then adds
     a `relocate` revision.
   - Convert to managed works while `missing`.
7. **Project membership.**
   - A Model added to two Projects appears in both Project views and has
     `projectIds` of length 2.
   - Removing its last membership makes it Unfiled.
   - Adding an existing membership is a no-op with no event.
   - Deleting a Project deletes no Model, revision, or blob. Models that
     were only in that Project become Unfiled, and the result lists them.
   - Import with two `projectIds` creates both memberships. `useExisting`
     adds the row's Projects to the existing Model without removing any.
8. **Duplicates.** A content duplicate without a decision is rejected, and
   **Use existing**, **Add as another Model** (one blob), and **Add as a new
   revision** each behave as in D14.
9. **Rich 3MF.** Unsupported entries are reported before import, an
   unacknowledged row is rejected, and the stored bytes equal the source
   bytes.
10. **G-code.**
   - It is inspected, and its claims are stored verbatim with
     `trusted: false`.
   - The bytes are retained exactly.
   - No Slice Revision, Queue, or dispatch control appears.
   - Failure and cancellation at each injection point leave no Model,
     revision, blob row, or staged file.
11. **Selection.** No command accepts a raw path. An expired selection
    returns `SELECTION_EXPIRED`. A drop registers a selection and emits
    `library.selection.dropped`.
12. **Events.** Library events are emitted after commit only, and backfill
    ordering holds. A `library.*` event causes no Printer status backfill,
    and a `printer.status.*` event is ignored by the Library store.
13. **Watcher spike.** It is recorded with its pass/fail result on Linux
    x86_64, and the chosen mode is implemented.
14. **Frontend tests** cover the store (backfill, replay, revision guard, web
    mode), saved views, the import-flow reducer, the import dialog
    (duplicates, acknowledgment, cancel, retry), the sidebar, grid/list,
    the details panel, membership editing, locate and convert, project
    dialogs, and deep links.
15. **The tracer completes through the Tauri path.** One managed and one
    linked Model are imported, one of them into two Projects. After a
    restart, the memberships are unchanged, and the linked
    source is modified, then removed. The Model is recovered through Locate,
    and every prior revision still verifies.
16. **Keyboard and window-size checks** pass at 1440 × 900 and 1024 × 700,
    and `just package` succeeds on Linux, with the installed bundle picking
    and dropping files.

## Delivery strategy

Build contract-first vertical slices:

1. Spike: fixtures, the parser, the watcher, and drop.
2. Schema and domain types.
3. Content store.
4. Format inspectors.
5. Selection and inspection.
6. Import commit and duplicates.
7. Project and Model commands with events and backfill.
8. Linked sources.
9. Design-system primitives.
10. Store and shared-channel demultiplexing.
11. The workspace.
12. The import dialog.
13. Recovery and management dialogs.
14. The tracer and verification.

Each slice lands with its red/green tests. The task-level plan is
`docs/superpowers/plans/2026-09-23-p4-library-persistence-projects.md`.
