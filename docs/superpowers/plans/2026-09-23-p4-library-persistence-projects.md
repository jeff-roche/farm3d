# P4 Library Persistence and Projects Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Library durable. Deliver Projects; managed and linked
Models; immutable Model Source Revisions with retained bytes; STL, 3MF, and
G-code inspection; explicit duplicate resolution; linked-source watching;
missing-link recovery; and the Library workspace. Imported G-code is
retained and inspectable, but not dispatchable.

**Architecture:** Rust owns persisted truth.

- **Schema.** A migration adds Projects, Models, revisions, thumbnails,
  content blobs, and a blob-cleanup queue.
- **Content store.** A content-addressed SHA-256 store under
  `content_root` holds every revision's exact bytes, whether the Model is
  managed or linked.
- **Imports** enter only through a Rust-owned native picker or a
  Rust-observed window drop, held in an in-memory selection registry, and
  are staged, hashed, inspected, and committed per file.
- **Inspection** uses farm3d-owned STL and G-code readers and a `zip` +
  `quick-xml` 3MF reader.
- **Linked sources** are followed by `notify` parent-directory watches.
  Authority rests with a stat-then-hash check that also runs at startup and
  on demand.
- **Events.** Library events go on their own stream over `farm3d-event-v1`,
  with listen-before-backfill.
- **Frontend.** SolidJS adds a `library-store`, pure import-flow and
  saved-view modules, two design-system primitives (`FileDropSurface` and
  `SegmentedControl`), and the workspace, import, and recovery screens.

**Tech Stack:** Rust, Tauri 2.11, rusqlite/SQLite, tokio, sha2, zip 8.6,
quick-xml 0.42, notify 8.2, notify-debouncer-full 0.7, base64 0.22, ts-rs,
tauri-plugin-dialog 2.7 (already present), SolidJS, TypeScript, Kobalte, CSS
Modules, Vitest, and Rust unit and integration tests.

**Spec:** `docs/superpowers/specs/2026-09-23-p4-library-persistence-projects-design.md`
(decisions D1–D20 are cited below by number). The spec is a draft awaiting
answers to Q1–Q6. This plan assumes each question's recommended option. If
the user chooses otherwise, revise the tasks the spec names next to that
question before starting them.

## Global Constraints

**Ordering relative to P3**

P3 (Spools and Material Slots) is expected to merge first. Before Task 2
starts, rebase onto `main`. Then:

- **Migration number.** Name the migration after the highest existing one
  (`0005_p4_library.sql` if `0004_p3_spools_material_slots.sql` exists),
  and set `CURRENT_SCHEMA_VERSION` to match. Every test reads
  `persistence::CURRENT_SCHEMA_VERSION` rather than a literal version.
- **Command-count assertions** (`COMMAND_NAMES` length,
  `COMMAND_CONTRACTS` array length, `f1_contract_path.rs` counts) are
  updated by **adding 16** to whatever `main` has. Never hard-code a total
  from this plan.
- **`RuntimeServices` / `for_test`.** Add the `library` field following
  whatever construction pattern P3 left. If P3 added a builder or extra
  constructor argument, extend it the same way rather than inventing a
  second one.
- **P3's `DataTable` and `Timeline`** are reused in Tasks 11 and 13. If P3
  has not merged when those tasks start, use the named screen-local
  fallback and leave a `TODO(P3-merge)` comment naming the swap.

**Data and ownership**

- Rust is persisted truth. Import rows, the active saved view, grid/list
  mode, search, and viewport state are frontend display state.
- Every Model Source Revision's bytes live in
  `<content_root>/blobs/sha256/<hh>/<hex>` (D2, D4). Revisions are immutable,
  enforced by a trigger. Only `delete_model` removes them.
- No command accepts a raw filesystem path from the frontend (D7).
- Errors, warnings, and events carry basenames and ids, never full paths.
  `ModelRecord.link.path` is the one place a full path crosses to the UI
  (D6).
- G-code metadata is stored verbatim as untrusted claims. No Printer,
  nozzle, or material fact is inferred (D11). No Slice Revision, Queue, or
  dispatch control is created.
- Duplicate resolution is never silent (D14).

**Events**

Events are emitted after commit only, on the `library` stream. The Printer
status listener and the Library listener each filter by type prefix (D17).

**Out of scope**

Rendering, plate preparation, slicing, Slice Revisions, Queue, binary
G-code, OBJ/STEP, managed-to-linked conversion, and pruning. Also out of
scope is any claim about Windows or macOS watcher behavior.

**Repo conventions**

- Use Kobalte primitives, CSS Modules, and `--f3d-*` color, type, and radius
  tokens only, in the editor aesthetic (no elevation or ripple).
- New design-system components go in `components/index.ts`,
  `components.test.tsx`, and `Showcase.tsx`.
- Kobalte `Select`, `DropdownMenu`, and `Combobox` tests use
  `fireEvent.pointerDown` and `fireEvent.pointerUp`.
- Never hand-edit `src/generated/contracts/**`. Run `just gen-contracts`.
- Every new command goes in:
  - `lib.rs` `COMMAND_NAMES` and `generate_handler!`.
  - `contracts/inventory.rs` `COMMAND_CONTRACTS` (and its array length) and
    the `CommandContracts` declaration and visitor.
  - `tests/export_contracts.rs`.
  - `src/ipc/client.ts` `CommandMap`.
- A new `just` recipe (`gen-library-fixtures`) accompanies the new
  regeneration entry point.
- The worktree needs `npm install` once before the frontend gates run.

**Validation**

- Frontend is done when `just build` and `just test` pass.
- Rust is done when `source "$HOME/.cargo/env" && just test-rust` passes and
  `just gen-contracts` leaves no diff.

## Known spec clarifications found while planning

1. **Shared event channel.** `src/printers/printer-store.ts` passes every
   `farm3d-event-v1` payload to `createPrinterStatusStore`. P3's inventory
   stream already sends foreign-stream events down that path, which forces
   needless status backfills. Task 10 adds type-prefix filtering (D17).
   After rebasing, check whether P3 already filters. If it does, keep one
   filter.
2. **Selection ids in tests.** The Tauri path can't open a real dialog or
   drop in `tauri::test`. The selection registry therefore exposes
   `register(purpose, paths) -> SelectionId` as `pub` (not `#[cfg(test)]`),
   which the drop handler also uses. Integration tests call it through
   `RuntimeServices` exactly as the drop handler does. That is still not a
   frontend path: no command wraps it.
3. **`ModelRecord.link.watchMode`** is runtime state from `LinkSupervisor`,
   merged into records when they are read. Repository reads alone return
   `notWatched`. Commands and events always go through
   `LibraryServices::record_for`, which merges the supervisor state.
4. **Convert to managed while missing** is deliberately broader than the
   umbrella text (spec D5). The verification doc records this as an
   intentional deviation for the product owner.

## File and module map

### Backend

- **Create** `src-tauri/migrations/0005_p4_library.sql` (the number is set
  at rebase).
- **Modify** `src-tauri/src/persistence/migrations.rs`: the migration entry
  and schema version.
- **Modify** `src-tauri/src/persistence/database.rs`: `Storage::paths()`.
- **Create** the following in `src-tauri/src/library/`:
  - `mod.rs`
  - `repository.rs`
  - `content.rs`
  - `selection.rs`
  - `import.rs`
  - `links.rs`
  - `events.rs`
  - `blockers.rs`
  - `commands.rs`
  - `formats/mod.rs`
  - `formats/stl.rs`
  - `formats/threemf.rs`
  - `formats/gcode.rs`
  - `formats/png.rs`
- **Modify** `src-tauri/src/lib.rs`:
  - The module, `RuntimeServices.library`, and startup.
  - The content sweep and `LinkSupervisor` start.
  - `on_webview_event` drop handling.
  - Command registration.
- **Modify** `src-tauri/src/contracts/command.rs`: the new `ErrorCode`s and
  constructors.
- **Modify** `src-tauri/src/contracts/inventory.rs`: command contracts.
- **Modify** `src-tauri/Cargo.toml` (and `Cargo.lock`): `notify`,
  `notify-debouncer-full`, `zip`, `quick-xml`, and `base64`.
- **Create** `src-tauri/tests/library_fixtures.rs`: the ignored
  `regenerate_library_fixtures` test plus the fixture assertions.
- **Create** `src-tauri/tests/fixtures/library/**`: the fixtures and their
  `*.expected.json` files (D20).
- **Create** these test files:
  - `src-tauri/tests/p4_migration.rs`
  - `src-tauri/tests/p4_content.rs`
  - `src-tauri/tests/p4_import.rs`
  - `src-tauri/tests/p4_links.rs`
  - `src-tauri/tests/p4_contract_path.rs`
  - `src-tauri/tests/p4_tracer.rs`
- **Modify** `src-tauri/tests/common/mod.rs`: the library fakes
  (`FakeModelFileIo`) and a `runtime_with_content_root` helper.
- **Modify** `src-tauri/tests/export_contracts.rs` and
  `src-tauri/tests/f1_contract_path.rs`: registry entries and counts.
- **Modify** `justfile`: the `gen-library-fixtures` recipe.

### Frontend

- **Generated:** `src/generated/contracts/**`, via `just gen-contracts`
  only.
- **Create** the following in `src/design-system/components/`, and update
  `index.ts`, `components.test.tsx`, and `Showcase.tsx`:
  - `FileDropSurface.{tsx,module.css}`
  - `SegmentedControl.{tsx,module.css}`
- **Create** the following in `src/library/`, each with a test:
  - `types.ts`
  - `library-store.ts`
  - `saved-views.ts`
  - `import-flow.ts`
  - `web-fixtures.ts`
- **Modify** `src/ipc/client.ts`: the 16 `CommandMap` entries.
- **Modify** `src/printers/printer-store.ts` and its test: the event-type
  filter.
- **Create** the following in `src/screens/`, each with `.module.css` and
  `.test.tsx`:
  - `LibraryWorkspace`
  - `LibrarySidebar`
  - `ModelGrid`
  - `ModelList`
  - `ModelDetailsPanel`
  - `ImportDialog`
  - `LocateSourceDialog`
  - `ProjectDialogs`
  - `DeleteModelDialog`
- **Delete** `src/screens/ModelLibrary.{tsx,module.css}`.
- **Modify** `src/screens/BuildPlate.tsx`: the `model` label prop only.
- **Modify** `src/App.tsx` and `src/App.test.tsx`:
  - Remove `MODELS` and the `Model` import.
  - Mount `LibraryWorkspace`.
  - Library ids in the navigation context.
  - The drop listener.
  - `library-store` startup.

### Docs

- **Create**
  `docs/superpowers/baselines/2026-09-2x-p4-format-watcher-spike.md` (Task 1).
- **Modify** `CONTEXT.md`: Model (refined), Unfiled, Managed Model, Linked
  Model, Source state, and Import selection.
- **Modify**
  `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`:
  close the "Parser and watcher libraries" and "Managed-content layout and
  hashing" rows.
- **Create** `docs/verification/2026-09-2x-p4-library-persistence.md`.

---

### Task 1: Spike — fixtures, parsers, watcher, and drop

**Owner:** Backend (parsers and watcher). Wiring (drop). The Protocol owner
consults on format evidence.
**Prerequisites:** Approved spec.
**Handoff:**

- The committed fixtures and their `*.expected.json` feed Task 4.
- The spike report fixes three things:
  - The watcher mode used by Task 8.
  - The hash algorithm used by Task 3.
  - Whether D10's fallback is needed.

**Files:**
- Create: `src-tauri/tests/library_fixtures.rs` (the generator part only)
- Create: `src-tauri/tests/fixtures/library/*`
- Create: `docs/superpowers/baselines/2026-09-2x-p4-format-watcher-spike.md`
- Modify: `justfile` (`gen-library-fixtures`)
- Scratch (not committed): `src-tauri/examples/p4_spike_*.rs`, deleted
  before the commit

**Interfaces:**
- Produces:
  - `just gen-library-fixtures`, which runs
    `cargo test --manifest-path src-tauri/Cargo.toml --test library_fixtures regenerate_library_fixtures -- --ignored --exact`.
  - The spike report, with one row per gate below: PASS or FAIL, evidence,
    and the decision taken.

- [ ] **Step 1: Write the fixture generator**

`regenerate_library_fixtures` writes these generated fixtures from D20
deterministically, with no timestamps:

- A 10 mm cube as `cube-ascii.stl`, `cube-binary.stl`,
  `cube-binary-solid-header.stl` (80-byte header starting
  `solid farm3d-binary`), and `cube-ascii-bare-solid.stl`.
- `truncated-binary.stl` (count 12, 3 triangles present), `nan.stl`, and
  `empty.stl` (count 0).
- `core-two-objects.3mf`: two cubes, one as a component of the other with a
  translate transform, `Title` metadata, and a 2×2 PNG at
  `Metadata/thumbnail.png`.
- `required-beam-lattice.3mf`: `requiredextensions="b"` with the Beam
  Lattice namespace.
- `zip-slip.3mf`: a `p:path="/../../etc/passwd"` component.
- `no-objects.3mf`: empty `<resources>` and `<build/>`.
- `cura-style.gcode`: `;FLAVOR:Marlin`, `;TIME:1234`,
  `;Filament used: 1.2m`, `;Layer height: 0.2`, and
  `;Generated with Cura_SteamEngine 5.8.0`, followed by 20 `G1` moves.
- `plain.gcode`: `G28`, `G90`, and 10 `G1` moves, with no comments.
- `binary.bgcode`: `GCDE` plus 12 zero bytes.

It also writes `cube-for-slicers.stl`, a copy of the cube used for the
slicer exports.

Run `just gen-library-fixtures` twice. Expected: the second run produces no
`git diff`.

- [ ] **Step 2: Produce the slicer fixtures**

With the flatpak slicers already installed on the development host:

- **OrcaSlicer** (`flatpak run com.orcaslicer.OrcaSlicer`): import
  `cube-for-slicers.stl` twice, move the second copy to plate 2, and save
  the project as `orca-two-plates.3mf`. Slice plate 1 with any bundled
  Printer profile and export `orca-cube.gcode`.
- **PrusaSlicer 2.9.6** (`flatpak run com.prusa3d.PrusaSlicer`): import the
  cube, save `prusa-project.3mf`, slice, and export `prusa-cube.gcode`.

Record each slicer's exact version and the profile used in the spike report.
These four files are committed as produced. If a slicer can't run (no
display), mark that fixture **unavailable** in the report. Task 4 then uses
the generated fixtures only, and acceptance criterion 2 is recorded as
partial until the fixtures exist.

- [ ] **Step 3: Gate A — parsers**

In a scratch example, read every 3MF fixture with `zip` 8.6
(`default-features = false`, `deflate-flate2-zlib-rs`) and a `quick-xml`
0.42 streaming reader that implements D10's supported set.

Pass when all of these hold:

- `orca-two-plates.3mf` yields 2 objects, 2 plates, and 24 triangles, with
  `requiredExtensions: ["p"]` resolved through `3D/_rels/3dmodel.model.rels`.
- `core-two-objects.3mf` yields its transforms.
- `zip-slip.3mf` is rejected.
- A synthetic 50 MB object part (generated in the example, not committed) is
  inspected in under 5 s, with peak RSS under 64 MB above baseline, measured
  with `/usr/bin/time -v`.

Fail means: try `lib3mf` 0.1.6 against the same checks. If that also fails,
stop and ask the user to approve D10's narrowing.

Also run `stl_io` 0.11 against `cube-binary-solid-header.stl` and
`cube-ascii-bare-solid.stl`, and record that both fail. That is the evidence
for D9.

- [ ] **Step 4: Gate B — hash throughput**

Hash a 512 MiB file of random bytes with `sha2` 0.10 in 1 MiB chunks, in a
release build.

- **Pass:** at least 300 MB/s. Record MB/s and CPU model.
- **Fail:** use `blake3`. Task 3 changes the directory name to
  `blobs/blake3/` and the columns to `content_blake3`.

- [ ] **Step 5: Gate C — watcher (Linux x86_64)**

In a scratch example, use `notify-debouncer-full` 0.7 (750 ms) with one
`NonRecursive` watch on a temp directory. For each D15 scenario, run 20
repetitions and count resulting debounced batches that mention the
directory:

- An in-place overwrite.
- Write to a temp file, then rename over the target.
- Delete.
- Rename away, then rename back.
- Remove and recreate the parent directory, then recreate the file.
- A symlink in the watched directory whose same-directory target is
  overwritten.
- A symlink whose target is in another directory (expected: no event;
  record it).

Pass when every scenario except the last yields at least one batch within
2 s in all 20 repetitions.

Also record the host's `/proc/sys/fs/inotify/max_user_watches`, and prove
that exceeding a lowered limit makes `watch()` return an error. Use a
`ulimit`-style test by creating watches until error, in a throwaway user
namespace if available; otherwise record it as "not exercised."

Fail means Task 8 uses `PollWatcher` (5 s) for every linked directory.

- [ ] **Step 6: Gate D — drop and picker in `just dev`**

Add a temporary `on_webview_event` handler that logs the dropped path count
(never the paths) with `eprintln!`. Run `just dev` and drop two files from
the file manager.

- **Pass:** the handler logs `2`, and the frontend
  `getCurrentWebview().onDragDropEvent` fires `enter`, `over`, and `drop`
  with no capability error in the devtools console.
- Also call `blocking_pick_files()` with the D7 filter from a temporary
  command and confirm multi-select works.
- **Fail** on a capability error: add exactly the named permission to
  `capabilities/default.json` and record it.

Remove the temporary code afterwards. If there is no display, mark Gate D
**unavailable**; Task 14 re-runs it.

- [ ] **Step 7: Write the spike report**

Mirror the evidence-table style of
`docs/superpowers/baselines/2026-09-16-supported-platforms.md`. Give the
gates, the results, the versions (rustc, crates, slicers, kernel), the
decisions taken, and the unavailable items.

- [ ] **Step 8: Commit**

Message: `test: add P4 library format fixtures and spike evidence`.

---

### Task 2: Schema, domain types, and the Project repository

**Owner:** Backend
**Prerequisites:** Task 1. Rebase onto `main` after P3 has merged.
**Handoff:** Tasks 3, 6, 7, and 8 use the tables, `ModelFormat`,
`StorageMode`, `SourceState`, and `LibraryRepository`.

**Files:**
- Create: `src-tauri/migrations/0005_p4_library.sql` (spec §Migration,
  verbatim)
- Create: `src-tauri/src/library/{mod.rs,repository.rs}`
- Create: `src-tauri/tests/p4_migration.rs`
- Modify: `src-tauri/src/persistence/{migrations.rs,database.rs}`
- Modify: `src-tauri/src/lib.rs` (`pub mod library;`)

**Interfaces:**
- Produces, in `library/mod.rs`:
  - `ModelFormat { Stl, ThreeMf, Gcode }`, with serde names `stl`, `3mf`,
    and `gcode`.
  - `StorageMode { Managed, Linked }`.
  - `SourceState { Ok, Missing, Unreadable, NotAFile, InvalidContent, Changing }`.
  - `RevisionOrigin { Import, LinkedChange, Relocate, AddedRevision }`.
  - `ProjectRecord`, `StoredModel`, and `StoredRevision`.
  - `fn validate_project_name(&str) -> Result<String, CommandError>` (trim,
    1–128 characters).
  - `fn validate_model_name` (1–255 characters).
  - `new_id(prefix: &str) -> String`, producing `"{prefix}-{uuid-v4}"`.
- Produces `LibraryRepository` methods:
  - `list_projects(&Connection) -> Result<Vec<ProjectRecord>, StorageError>`,
    with `model_count`.
  - `insert_project` and `rename_project`, each taking
    `(tx, id, expected_revision, name)`.
  - `project_name_taken(tx, name, excluding)`: a Unicode `to_lowercase`
    comparison in Rust, with the index as a backstop.
- Produces `Storage::paths(&self) -> &StoragePaths`.

- [ ] **Step 1: Write the failing migration tests**

In `tests/p4_migration.rs`, following `p2_migration.rs` and using
`persistence::test_support::apply_through`:

1. A fresh database reaches `CURRENT_SCHEMA_VERSION`, and the ledger has
   the P4 row with its checksum.
2. A database at `CURRENT_SCHEMA_VERSION - 1` with one Printer (and one
   Spool if P3 has merged) upgrades. Those rows compare equal before and
   after.
3. Inserting a Project named `' x'`, or a second Project named `Brackets`
   when `brackets` exists, fails.
4. A `managed` Model with a `linked_path` fails the CHECK. A `linked` Model
   with a NULL `link_state` fails.
5. `UPDATE model_source_revisions SET source_file_name='x'` fails with
   `model source revisions are immutable`. `DELETE FROM library_models`
   cascades to revisions and thumbnails.
6. `DELETE FROM content_blobs` for a referenced hash fails (FK).
7. **Crash boundary:** `apply_through_failing_before_commit` at the new
   version leaves the database at the previous version and unchanged.

Run
`source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml --test p4_migration`.
Expected: FAIL (the schema version is one short).

- [ ] **Step 2: Add the migration**

Add the SQL file and a `Migration { version, name: "0005_p4_library", sql,
post: None }` entry. Bump `CURRENT_SCHEMA_VERSION` and the array length.
Update the literal schema-version assertions in `persistence/mod.rs` tests
to use `CURRENT_SCHEMA_VERSION`. Run the tests. Expected: PASS.

- [ ] **Step 3: Write failing repository unit tests**

In `library/repository.rs`, using `crate::test_storage()`:

- `insert_project` then `list_projects` returns `modelCount: 0`.
- `rename_project` with a stale revision gives `CONFLICT`.
- A Unicode case-fold duplicate (`Ärger` vs `ärger`) is rejected by
  `project_name_taken`.

Expected: FAIL.

- [ ] **Step 4: Implement the types and project SQL**

Keep the SQL in `repository.rs` and the types in `mod.rs`, with one `const
MODEL_COLUMNS: &str` shared by every model `SELECT`. Serde and ts-rs
derives follow `printers/mod.rs`, exporting to `domain/`. Run the tests.
Expected: PASS.

- [ ] **Step 5: Commit**

Message: `feat: add the P4 library schema and project repository`.

---

### Task 3: The content store

**Owner:** Backend
**Prerequisites:** Task 2.
**Handoff:**

- Task 5 uses `stage_from_path`.
- Tasks 6 and 8 use `place_and_commit`.
- Task 7 uses `release_unreferenced` and `info`.
- P5 uses `open_verified`.

**Files:**
- Create: `src-tauri/src/library/content.rs`
- Create: `src-tauri/tests/p4_content.rs`
- Modify: `src-tauri/src/lib.rs` (the startup sweep in
  `build_runtime_services`, before `restore_persisted_connections`)

**Interfaces:**

```rust
pub struct ContentStore { root: PathBuf, placement: Mutex<()>, /* test hooks */ }
pub struct StagedFile { pub path: PathBuf, pub sha256: String, pub size: u64 }
pub enum ContentError { TooLarge, NotAFile, Unreadable(std::io::ErrorKind), ChangedDuringRead,
                        Cancelled, HashMismatch, Io, Storage(StorageError) }
impl ContentStore {
    pub fn open(content_root: &Path) -> Result<Self, StorageError>;       // creates blobs/sha256, staging
    pub fn startup_sweep(&self, storage: &Storage) -> Result<SweepReport, StorageError>;
    pub fn stage_from_path(&self, source: &Path, staging_key: &str, file_index: usize,
                           cancel: &CancelFlag, progress: &mut dyn FnMut(u64, u64))
        -> Result<StagedFile, ContentError>;                              // D4 steps 1–2
    pub fn stage_bytes(&self, bytes: &[u8], staging_key: &str, name: &str) -> Result<StagedFile, ContentError>; // thumbnails
    pub fn place_and_commit<T>(&self, storage: &Storage, staged: &[&StagedFile],
        commit: impl FnOnce(&Transaction<'_>) -> Result<T, StorageError>) -> Result<T, ContentError>; // D4 steps 4–5
    pub fn release_unreferenced(&self, storage: &Storage) -> Result<usize, StorageError>; // post-commit unlink under the lock
    pub fn open_verified(&self, sha256: &str) -> Result<VerifiedReader, ContentError>;
    pub fn discard_staging(&self, staging_key: &str);
    pub fn info(&self, storage: &Storage) -> Result<ContentInfo, StorageError>;
    #[doc(hidden)] pub fn inject_failure_once(&self, point: ContentFailurePoint); // AfterPlacementBeforeCommit, BeforeUnlink
}
pub fn mark_unreferenced_blobs(tx: &Transaction<'_>, candidates: &[String]) -> Result<(), StorageError>; // in-transaction half of D4 cleanup
```

`CancelFlag` wraps a `tokio::sync::watch::Receiver<bool>` and has a
`is_cancelled()` check.

- [ ] **Step 1: Write the failing tests** in `tests/p4_content.rs`

1. `stage_from_path` on a 3 MiB file returns the SHA-256 that `sha2`
   computes independently. The staged file is under
   `staging/<key>/0.part`.
2. **Changed during read.** A test hook appends to the source between the
   copy and the re-stat. The result is `ChangedDuringRead`, and staging for
   that key is empty.
3. **Too large.** With a test-only lower limit
   (`ContentStore::with_max_bytes(1024)`), a 2 KiB file gives `TooLarge`
   without reading. A directory gives `NotAFile`.
4. **Cancellation.** Cancelling after the first 1 MiB chunk gives
   `Cancelled`, and nothing remains in staging.
5. **Placement.** `place_and_commit` for a new hash creates
   `blobs/sha256/<hh>/<hex>` (mode `0400` on Unix) and commits the row the
   closure inserts. A second placement of the same bytes leaves one file and
   deletes the second staged file.
6. **Crash between placement and commit.**
   `inject_failure_once(AfterPlacementBeforeCommit)` returns an error. The
   blob file exists with no row. `startup_sweep` deletes it and reports
   `orphans_removed: 1`.
7. **Existing-blob size mismatch.** Truncate an existing blob, then place
   the same hash. The result is `ContentError::HashMismatch`, mapped to
   `CORRUPT_DATA`.
8. **Cleanup.**
   - Insert a blob row, call `mark_unreferenced_blobs` in a transaction, and
     commit. A `pending_blob_cleanup` row exists and the `content_blobs` row
     is gone.
   - `release_unreferenced` unlinks the file and deletes the pending row.
   - With `inject_failure_once(BeforeUnlink)`, the pending row survives and
     `startup_sweep` finishes it.
9. **Race guard.** A blob pending cleanup is re-placed by an import before
   `release_unreferenced` runs. `release_unreferenced` sees the new
   `content_blobs` row, keeps the file, and drops the pending row.
10. **Verified reads.** `open_verified` on an intact blob reads all bytes. On
    a blob with one byte flipped, reading to the end returns `HashMismatch`.
11. `startup_sweep` deletes everything under `staging/`.

Expected: FAIL (the module is missing).

- [ ] **Step 2: Implement `content.rs`**

- Directories use the same contained-directory checks as
  `persistence/database.rs`. Expose `create_contained_directory` as
  `pub(crate)`; don't copy it.
- Copy in 1 MiB chunks, updating a `Sha256`, calling `progress`, and
  checking `cancel` per chunk.
- Blob permissions: `0400` on Unix; `set_readonly(true)` elsewhere.
- Place with `fs::rename` from staging to the blob path (both under
  `content_root`, so on one filesystem), then `fsync` the parent directory
  on Unix. Don't use `document_io::atomic_replace`: a blob is never
  replaced, only created.

Run the tests. Expected: PASS.

- [ ] **Step 3: Wire the startup sweep**

In `build_runtime_services`, after `migrate_legacy`, open
`ContentStore::open(storage.paths().content_root())` and call
`startup_sweep`. A sweep error maps through `startup_error`:

- `Filesystem` becomes `PERSISTENCE_UNAVAILABLE` (retryable).
- A blob-directory symlink becomes `PathCollision`.

Keep the store in a local for Task 7 to put into `LibraryServices`.

- [ ] **Step 4: Commit**

Message: `feat: add the content-addressed library content store`.

---

### Task 4: Format inspectors

**Owner:** Backend (the Protocol owner reviews the claim allowlist)
**Prerequisites:** Task 1. This can run in parallel with Tasks 2 and 3.
**Handoff:** Tasks 5 and 8 call `formats::detect` and `formats::inspect`.

**Files:**
- Create: `src-tauri/src/library/formats/{mod.rs,stl.rs,threemf.rs,gcode.rs,png.rs}`
- Modify: `src-tauri/tests/library_fixtures.rs` (the fixture assertions)
- Modify: `src-tauri/Cargo.toml` (`zip`, `quick-xml`, and `base64`)

**Interfaces:**

```rust
pub const INSPECTOR_VERSION: i64 = 1;
pub fn detect(path: &Path) -> Result<Detected, InspectError>;       // D8; Detected { format, stl_encoding?, warnings }
pub fn inspect(path: &Path, detected: &Detected, cancel: &CancelFlag) -> Result<InspectOutcome, InspectError>;
pub struct InspectOutcome { pub inspection: Inspection, pub summary: InspectionSummary,
                            pub thumbnail: Option<ThumbnailBytes>, pub warnings: Vec<ImportWarning> }
pub enum InspectError { UnsupportedFormat { reason: String, extensions: Vec<String> },
                        InvalidContent(String), Cancelled, Io }
#[serde(tag = "format")] pub enum Inspection { Stl(StlInspection), #[serde(rename = "3mf")] ThreeMf(ThreeMfInspection), Gcode(GcodeInspection) }
pub struct GcodeClaim { pub key: String, pub value: String, pub line: u64 }
pub struct GcodeInspection { producer: Option<Producer>, claims: Vec<GcodeClaim>, #[ts(type = "false")] trusted: bool,
    line_count: u64, command_count: u64, tools_used: Vec<u32>, relative_positioning_seen: bool,
    relative_extrusion_seen: bool, observed_bounds_mm: Option<BoundsMm>, thumbnails: Vec<ThumbnailInfo> }
```

`inspect` always reads the *staged* path it is given.

- [ ] **Step 1: Write the failing fixture tests**

In `tests/library_fixtures.rs`, add `#[test] fn every_fixture_matches_its_expected_inspection`.

- For each file in `fixtures/library/` that has a sibling
  `*.expected.json`, run `detect` + `inspect` and compare
  `serde_json::to_value(outcome.inspection)` with the file.
  - Compare floats within 1e-4.
  - Rejection fixtures hold `{ "error": "<CODE>", "reasonContains": "…" }`.
- Write each `*.expected.json` by hand from the fixture's known
  construction (Task 1 records slicer-fixture facts). Never generate one by
  running the code under test.

Add unit tests beside each reader:

- **STL** (`stl.rs`):
  - The detection-rule table from D9, using in-memory byte cases.
  - A trailing-bytes warning.
  - Non-finite coordinates are rejected.
- **G-code** (`gcode.rs`):
  - Claim extraction from header, trailer, and Cura forms.
  - A `; thumbnail begin 16x16 N` block is decoded to PNG bytes, and its
    dimensions match `png.rs`.
  - `G91` sets `relative_positioning_seen` and omits
    `observed_bounds_mm`.
  - A 70 KiB line gives a `LONG_LINE` warning.
  - Latin-1 input is accepted.
- **3MF** (`threemf.rs`):
  - Path normalisation rejects `..` escapes.
  - The Materials extension (`m`) required gives geometry plus an
    unsupported `MATERIALS` entry.
  - The Beam Lattice extension (`b`) required gives `UnsupportedFormat`
    with `extensions: ["b"]`.
  - The entry-count, ratio, and total-size limits are each hit, via
    test-only limit parameters.
- **PNG** (`png.rs`): the IHDR width and height are read; a non-PNG is
  rejected; a PNG over 1024 px is skipped with `THUMBNAIL_SKIPPED`.

Expected: FAIL.

- [ ] **Step 2: Implement the readers**

- **STL.** Hand-written, following D9. The ASCII tokenizer is
  whitespace-split and streamed through a `BufReader` over the staged file.
- **3MF.** `zip::ZipArchive` over the staged `File`.
  - Parse `[Content_Types].xml` and `_rels/.rels` for the
    `http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel` target.
  - Stream-parse the start part and each referenced part with
    `quick_xml::Reader::from_reader`, reading through a size- and
    ratio-limited `Read` adapter.
  - Accumulate vertices per object only into a per-object bounds plus a
    triangle count. Never store vertex arrays.
  - Components and build items combine object bounds through 3×4
    transforms (the corner method; D10).
  - Detect plates by parsing `Metadata/model_settings.config`. Build the
    unsupported list from the part names and attribute scan in D10.
  - Thumbnail order follows D12.
- **G-code.** A single-pass line reader over bytes, decoding per line as
  UTF-8 with a Latin-1 fallback.
  - The claim allowlist is exactly D11's list. Any other key is ignored and
    not stored.
- **`Cargo.toml`:** add the three crates with the exact features in the
  spec.

Run
`source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml --test library_fixtures && cargo test --manifest-path src-tauri/Cargo.toml --lib library::formats`.
Expected: PASS.

- [ ] **Step 3: Commit**

Message: `feat: inspect STL, 3MF, and G-code sources`.

---

### Task 5: Selection registry, native picker, drop, and inspection

**Owner:** Wiring (primary), Backend
**Prerequisites:** Tasks 3 and 4.
**Handoff:** Task 6 consumes `SelectionEntry` inspection results. Task 12
calls `pick_model_files` and `inspect_import_selection` and listens for
`library.selection.dropped` and `library.import.progress`.

**Files:**
- Create: `src-tauri/src/library/{selection.rs,events.rs}` (the events are
  minimal here and completed in Task 7)
- Create: `src-tauri/src/library/commands.rs` (the first commands)
- Create: `src-tauri/tests/p4_import.rs` (the selection and inspection
  part)
- Modify: `src-tauri/src/lib.rs` (`LibraryServices`, `on_webview_event`,
  and registration)
- Modify: `src-tauri/src/contracts/{command.rs,inventory.rs}`
- Modify: `src-tauri/tests/common/mod.rs` (`FakeModelFileIo`, and
  `runtime()` building `LibraryServices` over the test storage)

**Interfaces:**

```rust
pub trait ModelFileIo: Send + Sync {
    fn pick_files(&self, purpose: SelectionPurpose) -> Result<Option<Vec<PathBuf>>, CommandError>;
}
pub struct SelectionRegistry { /* Mutex<HashMap<SelectionId, SelectionEntry>>, ttl */ }
impl SelectionRegistry {
    pub fn register(&self, purpose: SelectionPurpose, paths: Vec<PathBuf>) -> ImportSelectionSummary; // ≤100 paths
    pub fn get(&self, id: &str) -> Result<Arc<SelectionEntry>, CommandError>;                         // SELECTION_EXPIRED
    pub fn discard(&self, id: &str);
    pub fn sweep_expired(&self, now: Instant);
}
pub struct LibraryServices<R: Runtime> { pub content: Arc<ContentStore>, pub stream: Arc<LibraryStream>,
    pub selections: Arc<SelectionRegistry>, pub file_io: Arc<dyn ModelFileIo>, pub links: Arc<LinkSupervisor<R>> /* Task 8; Option until then */, app: AppHandle<R> }
```

- Commands: `pick_model_files`, `inspect_import_selection`, and
  `cancel_import_selection`.
- Contract types: `ImportSelectionSummary`, `ImportInspection`,
  `ImportCandidate` (tagged `status: "ready" | "rejected"`),
  `DuplicateMatch`, `ImportWarning`, and `ImportItemErrorCode`.
- Events: `library.selection.dropped` and `library.import.progress`.
- `ErrorCode` gains `SelectionExpired`, `SourceContentDiffers`,
  `SourceUnavailable`, and `UnsupportedFormat`, with constructors
  following `duplicate_host`.

- [ ] **Step 1: Write the failing tests** in `tests/p4_import.rs`

Use `common::runtime()` with `FakeModelFileIo` returning fixture paths
copied into a temp directory:

1. **Picker.** `pick_model_files { purpose: "import" }` returns a summary
   with 3 files, their basenames, and sizes. With the fake returning `None`,
   it returns `null` and registers nothing.
2. **Inspection** of [cube-binary.stl, orca-two-plates.3mf, binary.bgcode]:
   - Items 0 and 1 are `ready` with the fixture hashes.
   - Item 2 is `rejected` with `UNSUPPORTED_FORMAT`.
   - `staging/<selectionId>/` holds exactly two `.part` files.
   - Calling it a second time returns an identical result without
     restaging (the staged files' mtimes are unchanged).
3. **Path policy.** A path inside `content_root`, and a symlink to a file
   inside `metadata_root`, are both `rejected` with `PATH_NOT_ALLOWED`. A
   symlink to a regular file elsewhere is `ready`, and its summary
   `fileName` is the symlink's basename.
4. **Directories.** A directory in a drop registration is `rejected` with
   `NOT_A_FILE`.
5. **Progress events.** Inspecting a 5 MiB STL emits at least one
   `library.import.progress` event on the library stream before the command
   resolves.
6. **Cancellation.** `cancel_import_selection` during inspection makes it
   resolve with the unfinished items `rejected: CANCELLED`, and staging is
   empty. A later `inspect_import_selection` returns `SELECTION_EXPIRED`.
7. **Expiry.** `sweep_expired` with the clock 31 min ahead makes `get`
   return `SELECTION_EXPIRED`.
8. **Drop.** Calling the drop handler function
   (`library::selection::handle_drop(&services, paths)`, which is what
   `on_webview_event` calls) emits `library.selection.dropped` with a
   summary equal to `register`'s. No full path appears in the serialized
   event (assert that the temp-dir prefix is absent).
9. **No raw paths.** No command's generated request type has a field whose
   name contains `path` or `Path`. Scan `CommandContracts.ts` in the test.

Expected: FAIL.

- [ ] **Step 2: Implement the selection, inspection, and path policy**

- **Path policy (D6):** check `is_absolute` and lexical normalisation, then
  `fs::metadata` (following links), a regular-file check, and a
  `canonicalize` check against the `StoragePaths` trees.
- **Inspection:** runs in `tauri::async_runtime::spawn_blocking` with
  `buffer_unordered(2)`, calling `content.stage_from_path` then
  `formats::inspect` on the staged file. The thumbnail is staged through
  `stage_bytes`.
- **Duplicate lookup:**
  `SELECT … FROM model_source_revisions JOIN library_models … WHERE content_sha256 = ?`.
- **Storage:** results are stored on the `SelectionEntry`.
- **`NativeModelFileIo`:** uses the D7 filters.
- **Drop:** `run()` gains
  `.on_webview_event(|webview, event| if let WebviewEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event { … handle_drop … })`.
  It resolves `LibraryServices` through `BootstrapState` and does nothing
  if bootstrap isn't ready.
- **Registration:** register the three commands in every registry listed
  under Global Constraints, then run `just gen-contracts`.

Run the tests. Expected: PASS.

- [ ] **Step 3: Commit**

Message: `feat: select, stage, and inspect Library import files`.

---

### Task 6: Import commit, duplicates, and G-code retention

**Owner:** Backend (the Wiring owner reviews idempotency and cancellation)
**Prerequisites:** Task 5.
**Handoff:** Task 12 calls `import_models`. Task 8 reuses
`insert_revision`. P5 reads G-code revisions through `open_verified`.

**Files:**
- Create: `src-tauri/src/library/import.rs`
- Modify: `src-tauri/src/library/{repository.rs,commands.rs}`
- Modify: `src-tauri/tests/p4_import.rs`

**Interfaces:**
- `import_models { selectionId, operationId, items: ImportItemRequest[] }`
  → `ImportModelsResult { items: ImportItemResult[] }`.
- `ImportItemResult { fileIndex, outcome: "imported" | "revisionAdded" |
  "reusedExisting" | "rejected" | "cancelled", model?: ModelRecord,
  revision?: ModelSourceRevisionSummary, errors: ImportItemError[], warnings }`.
- `pub(crate) fn insert_revision(tx, model_id, staged: &StagedFile,
  outcome: &InspectOutcome, origin, source_path, source_mtime) ->
  Result<StoredRevision, StorageError>` computes
  `sequence = max(sequence) + 1`.

- [ ] **Step 1: Write the failing tests** (in `tests/p4_import.rs`)

1. **Managed STL** into Project P:
   - The result is `imported`.
   - `library_models` has one row with `storage_mode='managed'`.
   - Revision 1 has `origin='import'` and the fixture hash.
   - The blob exists and staging is cleared for that item.
   - Events `library.model.changed` and `library.revision.created` follow,
     in that order and with consecutive sequences.
2. **Linked 3MF:**
   - The row has `linked_path` equal to the selected (non-canonical) path,
     `link_state='ok'`, and the observed size and mtime set.
   - The thumbnail row exists with `source='embedded'`.
   - `inspection_json` lists the Orca settings under `unsupported`.
3. **Unacknowledged rich 3MF:** the same 3MF with
   `acknowledgeUnsupported: false` is `rejected` with
   `UNSUPPORTED_NOT_ACKNOWLEDGED`, and no rows are written.
4. **Duplicates:** re-inspect the same STL in a new selection. Its
   `duplicates[0].modelId` is the first Model.
   - No `duplicateAction`: `DUPLICATE_DECISION_REQUIRED`.
   - `useExisting`: `reusedExisting`, with no new rows.
   - `addAnother`: a new Model, and still exactly one `content_blobs` row
     for the hash.
   - `addRevision` targeting the first Model: `reusedExisting`, because the
     bytes are unchanged.
   - `addRevision` with different bytes (`cube-ascii.stl` into the
     binary-STL Model): `revisionAdded`, sequence 2,
     `origin='addedRevision'`.
   - `addRevision` targeting a linked Model: `VALIDATION`.
   - `addRevision` with a format mismatch: `VALIDATION`.
5. **Idempotency:** calling `import_models` twice with the same
   `operationId` returns identical outcomes and creates no extra rows. A
   different `operationId` while the first is still running gives
   `CONFLICT`.
6. **G-code retention:**
   - After import, `open_verified` bytes equal the source file bytes
     exactly (including CRLF in a CRLF variant of `plain.gcode`).
   - The inspection has `trusted: false` and the `printer_model` claim
     verbatim.
   - The database has no table whose name contains `slice` or `queue`
     (asserted through `sqlite_schema`).
7. **G-code failure and cancellation.** Import `orca-cube.gcode` with
   injected failures at each point:
   - (a) inspection, via a truncated staged copy;
   - (b) `ContentFailurePoint::AfterPlacementBeforeCommit`;
   - (c) cancel between staging and placement.

   Each leaves zero `library_models`, `model_source_revisions`, and
   `content_blobs` rows for the hash. For (b), the orphan file is removed
   by `startup_sweep`. The source file's hash is unchanged. A following
   import of the same selection (for (a) and (c), after re-inspection)
   succeeds.
8. **Partial success:** a 3-item import where item 1 has a
   missing-project `projectId` commits items 0 and 2 and returns item 1
   `rejected: NOT_FOUND`.

Expected: FAIL.

- [ ] **Step 2: Implement `import.rs`**

- Process items in request order.
- Check the cancel flag before each item's placement.
- Each item calls `content.place_and_commit` with a closure that:
  1. Revalidates the Project and the target Model revision.
  2. Inserts the blob row if it is absent.
  3. Inserts the Model (unless the action is `addRevision`), the revision,
     and the thumbnail blob and row.
- After commit, record the outcome under `(selectionId, operationId,
  fileIndex)`, then publish events.
- For linked items, call the Task 8 hook `links.register(model_id)`. It is
  a no-op until Task 8 lands, through an `Option`.
- When every ready item has an outcome, discard the selection's staging.

Run the tests. Expected: PASS.

- [ ] **Step 3: Commit**

Message: `feat: import managed and linked Models with explicit duplicate handling`.

---

### Task 7: Library stream, backfill, and Project and Model commands

**Owner:** Backend (the Wiring owner reviews ordering)
**Prerequisites:** Task 6.
**Handoff:** Task 10's store consumes `list_library` and the events.

**Files:**
- Modify: `src-tauri/src/library/{events.rs,commands.rs,repository.rs}`
- Create: `src-tauri/src/library/blockers.rs`
- Create: `src-tauri/tests/p4_contract_path.rs`
- Modify: the registries (see Global Constraints); `f1_contract_path.rs`
  counts +16 overall by the end of Task 8

**Interfaces:**
- `LibraryStream` follows P3's `InventoryStream`: `stream_id`, an
  `AtomicU64` sequence, an emit mutex, `snapshot_sequence()`, and
  `publish(app, events: Vec<LibraryEvent>)`.
- Commands:
  - `list_library`
  - `create_project`, `rename_project`, `delete_project`
  - `update_model`, `delete_model`
  - `list_model_revisions`
  - `get_revision_thumbnail`
  - `library_content_info`
- `pub trait ModelDeletionBlocker: Send + Sync { fn blockers(&self, model:
  &StoredModel, tx: &Transaction) -> Result<Vec<LifecycleBlocker>,
  StorageError>; }`, with `blockers::registry() -> Vec<Box<dyn
  ModelDeletionBlocker>>` (empty in P4).

- [ ] **Step 1: Write the failing tests** in `tests/p4_contract_path.rs`

Use `common::runtime` with a recording event listener:

1. **Backfill ordering.** `list_library` returns `snapshotSequence = s`.
   After `create_project`, exactly one `library.project.changed` event
   arrives with `sequence = s + 1` and `subject { kind: "project", id }`.
2. **Project validation.** `create_project` with a duplicate name (case
   difference) gives `VALIDATION` with `fieldPath: "name"`.
   `rename_project` with a stale revision gives `CONFLICT` in P2's shape.
3. **`delete_project`** with two Models:
   - Both Models have `projectId: null` and bumped revisions.
   - The events are two `library.model.changed` then one
     `library.project.removed`.
   - The result lists both moved ids.
4. **`update_model`** moves a Model between Projects and renames it. A
   same-name sibling produces a `DUPLICATE_NAME` warning, not an error.
5. **`delete_model`** for a linked Model whose blob is shared with another
   Model:
   - The shared blob stays. The unshared thumbnail blob is removed.
   - `library.model.removed` is emitted.
   - The linked source file still exists with its original bytes.
6. **Blockers.** A test-registered blocker source makes `delete_model`
   return `LIFECYCLE_BLOCKED` with the blockers, and nothing changes. Use a
   `#[cfg(test)]`-injectable registry, following P2's lifecycle registry.
7. **Revisions.** `list_model_revisions` returns revisions newest first
   with full `inspection`. `get_revision_thumbnail` returns base64 that
   decodes to the fixture PNG, or `null` for an STL revision.
8. **No events on failure.** A failed mutation (for example `CONFLICT`)
   emits no event.
9. **Contracts.** The generated contracts include every new type, and
   `COMMAND_NAMES` grows by the number of commands registered so far,
   counted relative to the pre-P4 value captured in a constant at the top
   of the test.

Expected: FAIL.

- [ ] **Step 2: Implement**

- Each command does `contract_version.validate()?`, then
  `bootstrap.ready()?`, then one `storage.write`, then post-commit work
  (unlink and unwatch), then `publish`.
- `list_library` reads `snapshot_sequence()` before its read transaction.
- `delete_model`:
  1. Collect the Model's revision and thumbnail hashes.
  2. Delete the Model.
  3. Call `mark_unreferenced_blobs` for hashes no longer referenced by any
     revision or thumbnail.
  4. Commit.
  5. Call `content.release_unreferenced`.

Register the commands and run `just gen-contracts`. Run the tests.
Expected: PASS.

- [ ] **Step 3: Commit**

Message: `feat: add Library projects, model management, and the library event stream`.

---

### Task 8: Linked-source watching, reconciliation, and recovery

**Owner:** Backend (the Wiring owner reviews event ordering and restart)
**Prerequisites:** Tasks 1 (Gate C result) and 7.
**Handoff:** Task 13 calls `check_linked_sources`, `locate_linked_source`,
and `convert_model_to_managed`. `ModelRecord.link.watchMode` becomes live.

**Files:**
- Create: `src-tauri/src/library/links.rs`
- Create: `src-tauri/tests/p4_links.rs`
- Modify: `src-tauri/src/library/{commands.rs,import.rs,mod.rs}`
- Modify: `src-tauri/src/lib.rs` (start `LinkSupervisor` after services are
  ready; initial reconciliation pass)
- Modify: `src-tauri/Cargo.toml` (`notify` and `notify-debouncer-full`)

**Interfaces:**

```rust
pub struct LinkSupervisor<R: Runtime> { /* dir watches: HashMap<PathBuf, WatchEntry{ refcount, mode, handle }>, retry timers */ }
impl<R: Runtime> LinkSupervisor<R> {
    pub fn start(services: Weak<LibraryServices<R>>, storage: Arc<Storage>, mode: WatchPolicy) -> Arc<Self>;
    pub fn register(&self, model_id: &str, linked_path: &Path);
    pub fn unregister(&self, model_id: &str);
    pub fn watch_mode(&self, model_id: &str) -> WatchMode;
    pub async fn check(&self, model_ids: Option<Vec<String>>) -> Result<Vec<ModelRecord>, CommandError>;
    pub async fn reconcile_all(&self);                     // startup pass
}
pub enum WatchPolicy { Native { debounce: Duration }, PollOnly { interval: Duration } } // from Task 1 Gate C
pub fn check_linked_source(...) -> Result<CheckOutcome, CommandError>; // D15 algorithm; pure over an injected Fs trait for unit tests
```

- Commands: `check_linked_sources`, `locate_linked_source`, and
  `convert_model_to_managed`.

- [ ] **Step 1: Write failing unit tests** for `check_linked_source` in
  `links.rs`

Use an in-memory `Fs` fake that supplies stat and bytes:

1. Unchanged stat with state `ok` does no hashing (the fake counts reads).
2. A changed mtime with the same hash updates only the observed stat.
   There is no new revision, but the model revision is bumped (and the
   event is emitted by the caller).
3. Changed content creates revision 2 with `origin='linkedChange'`, and
   revision 1 is untouched.
4. `NotFound`, `PermissionDenied`, and a directory map to `missing`,
   `unreadable`, and `notAFile`.
5. Changed content that fails inspection gives `invalidContent`. The
   current revision is unchanged and no blob is left behind.
6. Content that changes during the copy gives `changing` and schedules
   retries at 2, 4, and 8 s (a fake clock), then stops.

Expected: FAIL.

- [ ] **Step 2: Write failing integration tests** in `tests/p4_links.rs`

Use a real filesystem under a temp directory and real `notify`. Each
wait uses a 10 s deadline and polls the recorded events:

1. **Import linked, then restart.**
   - Import `cube-binary.stl` as linked.
   - Rebuild `RuntimeServices` over the same storage and content root.
   - The startup reconciliation leaves `ok` and no new revision.
   - `watchMode` is `watching`, or `polling` when Gate C failed.
2. **Atomic save.** Write new STL bytes to `x.tmp` and rename it over the
   source. Within the deadline, `library.revision.created` arrives with
   sequence 2, and `open_verified` on revision 1 still yields the original
   bytes.
3. **Delete, then restore.**
   - Deleting the file leads to `library.model.changed` with state
     `missing`.
   - Restoring identical bytes leads to `ok` with `revisionCount`
     unchanged.
4. **Parent directory.** Delete the parent directory and recreate it with
   the file; the state goes `missing`, then `ok` (the ancestor watch from
   D15).
5. **`locate_linked_source`:**
   - Moving the file elsewhere and locating it by the same content relinks
     with no revision.
   - Locating different content with `acceptDifferentContent: false`
     returns `SOURCE_CONTENT_DIFFERS` with both hashes.
   - With `true`, a revision is added with `origin='relocate'`.
   - Locating a 3MF for an STL Model gives `VALIDATION`.
6. **`convert_model_to_managed`** while `missing`: `storageMode: managed`,
   `link: null`, the watch is released (the refcount reaches 0), and later
   filesystem changes emit nothing.
7. **Manual check.** `check_linked_sources {}` returns only the changed
   Models. With the watcher disabled (`WatchPolicy::PollOnly` with a 1 h
   interval), an edit is still picked up by this call.
8. **Watch registration failure.** A test hook makes `watch()` fail, which
   gives `watchMode: polling` for that directory and a
   `WATCH_UNAVAILABLE` warning on the import result.

Expected: FAIL.

- [ ] **Step 3: Implement `links.rs`**

- **Watcher threads.** The debouncer runs on its own thread and forwards
  directory paths over a `tokio::mpsc` channel to one async task. That task
  maps a directory to its linked Model ids with one indexed query, then
  calls `check` for them.
- **Checks** run one at a time per Model, using a `HashSet` of Models in
  flight so repeated events coalesce.
- **Commits** go through `content.place_and_commit` and
  `insert_revision`. Events are published after commit.
- **Records.** `LibraryServices::record_for` merges `watch_mode` into every
  `ModelRecord` (clarification 3).
- **Startup.** `lib.rs` spawns `reconcile_all` after `app.manage(...)`.
  Nothing blocks setup.

Run both test sets. Expected: PASS. If the watcher tests are flaky on the
CI host, do not add retries. Record the failure against Gate C and switch
`WatchPolicy` per the spike's fallback.

- [ ] **Step 4: Commit**

Message: `feat: watch linked Model sources and recover missing links`.

---

### Task 9: `FileDropSurface` and `SegmentedControl` primitives

**Owner:** Frontend
**Prerequisites:** None. This can run in parallel with Tasks 2–8.
**Handoff:** Tasks 11 and 12.

**Files:**
- Create: `src/design-system/components/{FileDropSurface,SegmentedControl}.{tsx,module.css}`
- Modify: `src/design-system/components/{index.ts,components.test.tsx}`,
  `src/design-system/Showcase.tsx`

**Interfaces:**

```ts
export interface FileDropSurfaceProps { active: boolean; disabled?: boolean; disabledReason?: string;
  label: string; hint?: string; chooseLabel?: string; onChoose: () => void; }
export interface SegmentedControlProps<T extends string> { label: string; value: T;
  options: { value: T; label: string; icon?: JSX.Element }[]; onChange: (value: T) => void; }
```

- [ ] **Step 1: Write the failing component tests**

- **`FileDropSurface`:**
  - It renders a `region` with `aria-label={label}`.
  - The **Choose files…** button calls `onChoose` on click and on Enter.
  - `active` sets `data-active`.
  - `disabled` disables the button and shows `disabledReason` as visible
    text (not only a tooltip).
- **`SegmentedControl`:**
  - It renders a Kobalte `SegmentedControl` (check
    `node_modules/@kobalte/core/src/segmented-control/` for the item and
    indicator part names).
  - Arrow keys move the selection and call `onChange`.
  - The selected option has `data-checked`.
  - Icon-only use is impossible: `label` is always rendered, visually
    hidden when an icon is present.

Run `npx vitest run src/design-system`. Expected: FAIL.

- [ ] **Step 2: Implement both, and add them to `index.ts` and
  `Showcase.tsx`**

- Use tokens only.
- The drop surface's active state uses `--f3d-color-accent-muted` with a
  `--f3d-color-accent` dashed border.
- There is no transition under `prefers-reduced-motion`.

Run `just test` and `just build`. Expected: PASS.

- [ ] **Step 3: Commit**

Message: `feat: add FileDropSurface and SegmentedControl components`.

---

### Task 10: Library store, shared-channel filter, and web fixtures

**Owner:** Frontend (the Wiring owner reviews backfill and the filter)
**Prerequisites:** Tasks 5–8 (generated contracts).
**Handoff:** Tasks 11–13.

**Files:**
- Create: the following in `src/library/`, each with a test:
  - `types.ts`
  - `library-store.ts`
  - `saved-views.ts`
  - `import-flow.ts`
  - `web-fixtures.ts`
- Modify: `src/ipc/client.ts` (16 `CommandMap` entries)
- Modify: `src/printers/printer-store.ts` and its test

**Interfaces:**

```ts
// library-store.ts
export const library: { projects(): ProjectRecord[]; models(): ModelRecord[]; status(): "idle"|"loading"|"ready"|"error";
  syncState(): "syncing"|"current"|"uncertain"; error(): string | null; contentInfo(): LibraryContentInfo | null };
export async function startLibrary(): Promise<() => void>;        // listen-before-backfill
export function onSelectionDropped(handler: (s: ImportSelectionSummary) => void): () => void;
export function onImportProgress(handler: (selectionId: string, p: ImportProgress) => void): () => void;
export async function createProject(name: string): Promise<ProjectRecord>;  // rejects for inline field errors
export async function renameProject(id: string, name: string): Promise<void>;
export async function deleteProject(id: string): Promise<void>;
export async function updateModel(id: string, patch: ModelPatch): Promise<void>;
export async function deleteModel(id: string): Promise<void>;
export async function loadRevisions(modelId: string): Promise<ModelSourceRevisionRecord[]>;
export async function loadThumbnail(revisionId: string): Promise<string | null>;  // data: URL, cached
export async function pickFiles(purpose: "import" | "locate"): Promise<ImportSelectionSummary | null>;
export async function inspectSelection(selectionId: string): Promise<ImportInspection>;
export async function importModels(selectionId: string, items: ImportItemRequest[], operationId?: string): Promise<ImportModelsResult>;
export async function cancelSelection(selectionId: string): Promise<void>;
export async function checkSources(modelIds?: string[]): Promise<void>;          // throttled 30 s unless forced
export async function locateSource(modelId: string, selectionId: string, fileIndex: number, acceptDifferentContent: boolean): Promise<ModelRecord>;
export async function convertToManaged(modelId: string): Promise<void>;
// saved-views.ts
export type SavedViewId = "all" | "unfiled" | "recent" | "attention" | "gcode";
export function modelsFor(view: { kind: "view"; id: SavedViewId } | { kind: "project"; id: string }, models: ModelRecord[], now: Date): ModelRecord[];
export function searchModels(models: ModelRecord[], projects: ProjectRecord[], query: string): ModelRecord[];
export function sortModels(models: ModelRecord[], by: "name" | "recent"): ModelRecord[];
export function viewCounts(models: ModelRecord[], now: Date): Record<SavedViewId, number>;
// import-flow.ts
export type ImportRow = { fileIndex: number; candidate: ImportCandidate; name: string; projectId: string | null;
  storageMode: "managed" | "linked"; duplicateAction?: "useExisting"|"addAnother"|"addRevision";
  targetModelId?: string; acknowledgeUnsupported: boolean; result?: ImportItemResult };
export function rowsFromInspection(inspection: ImportInspection, ctx: { projectId: string | null; models: ModelRecord[] }): ImportRow[];
export function rowBlockers(row: ImportRow): ("duplicateDecision"|"acknowledgeUnsupported"|"name"|"rejected")[];
export function buildRequest(rows: ImportRow[], models: ModelRecord[]): ImportItemRequest[];  // only unblocked, not-yet-succeeded rows
export function mergeResults(rows: ImportRow[], result: ImportModelsResult): ImportRow[];
```

- [ ] **Step 1: Write the failing tests**

- **`printer-store.test.ts`:** a `library.model.changed` envelope with a
  foreign `streamId` reaches the listener and does **not** call
  `printer_statuses` again. A `printer.status.changed` envelope still
  applies.
- **`library-store.test.ts`** (mock `@tauri-apps/api/core` and `event`):
  1. The listener registers before `list_library` is invoked. Buffered
     events at or below `snapshotSequence` are dropped, and later ones are
     applied in order.
  2. A `library.model.changed` with a `revision` not greater than the held
     one is ignored.
  3. A gap in the sequence triggers a new backfill and marks the store
     `uncertain` until it settles.
  4. `printer.status.*` events are ignored.
  5. `library.selection.dropped` calls `onSelectionDropped` handlers.
  6. `createProject` settles from the command result and rejects with the
     `CommandError` on `VALIDATION`.
  7. `checkSources` throttles to one call per 30 s unless forced.
  8. **Web mode** (`isTauri` false):
     - The fixtures load with `status: "ready"`.
     - `createProject` works locally.
     - `pickFiles` rejects with "Importing Models needs the desktop app."
- **`saved-views.test.ts`:**
  - Every view's membership is checked, including `recent` at exactly
    14 days (inclusive) and `attention` excluding `ok` and managed Models.
  - Search matches the Project name and the linked file basename.
  - Counts equal the lengths of the filtered lists.
- **`import-flow.test.ts`:**
  - Rows default to Managed and the current Project.
  - A duplicate row is blocked until an action is chosen.
  - A same-name Model in the target Project pre-fills `targetModelId`
    without choosing `addRevision`.
  - Rich 3MF is blocked until acknowledged.
  - Rejected rows are excluded from `buildRequest`.
  - `mergeResults` keeps failed rows' choices, and a second `buildRequest`
    includes only rows without a successful outcome.

Expected: FAIL.

- [ ] **Step 2: Implement the store and modules**

Model the stream handling on `printer-status-store.ts`: buffering, the
generation counter, and backoff at 1, 2, 4, 8, 16, then 30 s. The web
fixtures follow the spec. Wrap `listen("farm3d-event-v1", …)` in both
stores with a type-prefix check before the handler.

Run `just test` and `just build`. Expected: PASS.

- [ ] **Step 3: Commit**

Message: `feat: add the Library store and filter shared-channel events by type`.

---

### Task 11: Library workspace, navigation, grid and list, and details

**Owner:** Frontend
**Prerequisites:** Tasks 9 and 10.
**Handoff:** Tasks 12 and 13 add dialogs to the workspace.

**Files:**
- Create: the following in `src/screens/`, each with `.module.css` and
  `.test.tsx`:
  - `LibraryWorkspace`
  - `LibrarySidebar`
  - `ModelGrid`
  - `ModelList`
  - `ModelDetailsPanel`
- Delete: `src/screens/ModelLibrary.{tsx,module.css}`
- Modify: `src/screens/BuildPlate.tsx`, `src/App.tsx`, `src/App.test.tsx`

- [ ] **Step 1: Write the failing tests**

1. **`App.test.tsx`:**
   - `MODELS` is gone.
   - Navigating to `#nav=v1/library/model/<fixture id>` selects that Model
     and its Project.
   - An unknown model id shows "The requested item is no longer
     available."
   - The Library is available in the destination list.
2. **`LibrarySidebar`:**
   - The five views then the Projects alphabetically, each with a count.
   - The active item has `aria-current="page"`.
   - The Project row's `DropdownMenu` (pointer events) offers Rename… and
     Delete….
3. **`ModelGrid`:**
   - Cards show the thumbnail `<img alt="">` when `hasThumbnail` (after
     `loadThumbnail` resolves), otherwise the format icon with text.
   - A `missing` linked Model shows the `SeverityMarker` "Source missing".
   - Enter selects a card.
4. **`ModelList`:** columns Name, Project, Format, Storage, Source,
   Revisions, and Added. It scrolls horizontally inside its pane.
   - With `DataTable`, keyboard row selection works.
   - With the fallback, each row is a button.
5. **`LibraryWorkspace`:**
   - The `SegmentedControl` switches grid and list and persists to
     `localStorage` (`farm3d:library-mode`), inside `try/catch`.
   - Search filters.
   - An empty Library shows "Import a Model" with the `FileDropSurface`.
   - A filtered-empty view shows "No Models in Unfiled" and a **Show all
     Models** button.
   - In web mode, the drop surface is disabled with the reason text.
   - A stale sync shows "Library may be out of date" and **Refresh**.
6. **`ModelDetailsPanel`:**
   - Name edits call `updateModel` after blur or Enter.
   - The Project `Select` (pointer events) moves the Model.
   - A G-code Model shows "What the file says (not verified)" with its
     claims and no Slice, Queue, or Dispatch buttons.
   - A 3MF shows its "Not used by farm3d" list.
   - The revision history lists sequence, origin label, capture time, and
     short hash.
   - A linked `missing` Model shows **Locate source…** and **Convert to
     managed**.
7. **Window size.** At 1024 wide (set via `window.innerWidth` and a resize
   event), the details panel becomes an overlay toggled by **Details**.

Expected: FAIL.

- [ ] **Step 2: Implement the screens**

`App.tsx`:

- Starts `startLibrary()` after `loadPrinters()` in the startup sequence,
  and disposes it on cleanup.
- Extends `navigationContext()`: when on `library`, `availableIds` is the
  Library's project and model ids.
- Keeps `BuildPlate` as a placeholder that receives only
  `{ modelName, approximateSize }`.
- Removes the Slice, Dispatch, and Target printer controls.

Run `just test` and `just build`. Expected: PASS.

- [ ] **Step 3: Commit**

Message: `feat: replace the placeholder Library with the persisted Library workspace`.

---

### Task 12: Import dialog, drop, and duplicate resolution

**Owner:** Frontend (the Wiring owner reviews correlation, cancel, and
retry)
**Prerequisites:** Task 11.
**Handoff:** Task 14 exercises it in the tracer.

**Files:**
- Create: `src/screens/ImportDialog.{tsx,module.css,test.tsx}`
- Modify: `src/screens/LibraryWorkspace.tsx`, `src/App.tsx` (drop
  listener and the dropped-selection routing), and their tests

- [ ] **Step 1: Write the failing tests** (`ImportDialog.test.tsx`)

Mock the store functions:

1. **Import… flow.** **Import…** calls `pickFiles("import")`. A `null`
   result keeps the dialog closed. A selection opens it on step **Files**
   and calls `inspectSelection`. Progress events update a `Progress` with
   text.
2. **Cancel during inspection** calls `cancelSelection` and closes the
   dialog.
3. **Review step:**
   - Rows show name, Project `Select` (default: the viewed Project),
     and the Managed/Linked radio (Managed checked).
   - A rejected row shows its reason and has no controls.
   - A duplicate row shows the three actions, none selected. **Import** is
     disabled with "Choose what to do with 1 duplicate file."
   - Choosing **Add as a new revision of…** reveals the Model `Combobox`,
     pre-filled with the same-name Model.
4. **Rich 3MF.** The "Unsupported contents" disclosure lists each entry
   with "Kept in the stored file. farm3d won't use these when slicing." The
   acknowledgment `Checkbox` gates **Import**.
5. **G-code rows** say "Stored as pre-sliced G-code. It won't be sliced."
6. **Results step:**
   - `importModels` results render per row with icon and text.
   - A failed row keeps its choices and offers **Retry**, which calls
     `importModels` again with only that row and a new `operationId`.
   - **Done** closes the dialog and selects the first imported Model.
7. **Expired selection.** A `SELECTION_EXPIRED` rejection shows **Choose
   files again**.
8. **Drop.** `App`: a `library.selection.dropped` handler navigates to
   Library and opens `ImportDialog` at inspection. Enter and leave drag
   events toggle `FileDropSurface` `active` (mock
   `@tauri-apps/api/webview`).

Expected: FAIL.

- [ ] **Step 2: Implement**

The dialog uses `Dialog`, `Stepper`, `RadioGroup`, `Select`, `Combobox`,
`Checkbox`, and `Progress`. Rows live in a component-local `createStore`
driven by `import-flow.ts`. Nothing about rows goes into `library-store`.

Run `just test` and `just build`. Expected: PASS.

- [ ] **Step 3: Commit**

Message: `feat: import Models with managed or linked storage and explicit duplicate choices`.

---

### Task 13: Recovery, Project management, and Model deletion

**Owner:** Frontend
**Prerequisites:** Task 12.
**Handoff:** Task 14.

**Files:**
- Create: the following in `src/screens/`, each with `.module.css` and
  `.test.tsx`:
  - `LocateSourceDialog`
  - `ProjectDialogs`
  - `DeleteModelDialog`
- Modify: `src/screens/{ModelDetailsPanel,LibrarySidebar,LibraryWorkspace}.tsx`
  and their tests

- [ ] **Step 1: Write the failing tests**

1. **`LocateSourceDialog`:**
   - It names the missing file.
   - **Choose file…** calls `pickFiles("locate")`, then
     `locateSource(..., false)`.
   - On `SOURCE_CONTENT_DIFFERS`, it shows "This file is different from the
     last imported version." and **Relink and import as a new revision**,
     which calls `locateSource(..., true)`.
   - On success, the dialog closes and the panel shows the state `ok`.
2. **Convert to managed** shows a confirmation ("farm3d will stop following
   <file name>. Your existing revisions are kept.") and calls
   `convertToManaged`.
3. **Project dialogs:**
   - Create and rename show `VALIDATION` inline on the name field.
   - Delete states "3 Models will move to Unfiled." and calls
     `deleteProject`.
   - When the viewed Project is deleted, the view becomes Unfiled.
4. **`DeleteModelDialog`:**
   - It shows the D18 text.
   - `LIFECYCLE_BLOCKED` shows the blockers list.
   - On success, the selection clears.
5. **Check sources.**
   - The workspace calls `checkSources()` on mount, on `window` `focus`,
     and from **Check sources**.
   - **Check sources** is forced (not throttled) and shows "Checked just
     now".
6. **Updated from source.** A `library.revision.created` for the selected
   Model with `origin: "linkedChange"` shows the inline notice "Updated from
   source · revision N", and the revision history refreshes.

Expected: FAIL.

- [ ] **Step 2: Implement**

Use `Dialog`, `Button`, `TextField`, and the `Timeline` refresh. Kobalte
`AlertDialog` isn't wrapped in the design system, so use the existing
`Dialog` with `role="alertdialog"` as P2's `DeletePrinterDialog` does.

Run `just test` and `just build`. Expected: PASS.

- [ ] **Step 3: Commit**

Message: `feat: recover missing links and manage Projects and Models`.

---

### Task 14: Tracer, docs, packaging, and verification evidence

**Owner:** Wiring owner (the Verification owner performs a fresh pass)
**Prerequisites:** Tasks 1–13.

**Files:**
- Create: `src-tauri/tests/p4_tracer.rs`
- Modify: `CONTEXT.md`
- Modify: `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`
- Create: `docs/verification/2026-09-2x-p4-library-persistence.md`

- [ ] **Step 1: Write the automated tracer**

It runs through `tauri::test` IPC in one test:

1. `create_project "Brackets"`.
2. Register a selection with `cube-binary.stl` (managed) and a temp-dir copy
   of `orca-two-plates.3mf` (linked). Inspect it, then import both into
   "Brackets", acknowledging the unsupported entries.
3. Record both revision-1 hashes.
4. Rebuild `RuntimeServices` over the same metadata and content roots
   (restart). `list_library` returns both Models with the same ids and
   hashes, and the linked Model is `ok`.
5. Overwrite the linked 3MF with `core-two-objects.3mf` bytes through
   temp-file plus rename. Wait for `library.revision.created` (sequence 2),
   or call `check_linked_sources` if polling.
6. Delete the linked file. The state becomes `missing`.
7. Copy the modified bytes to a new directory, register a `locate`
   selection, and call `locate_linked_source` with `false`. The state
   becomes `ok` with no new revision (the content equals revision 2).
8. For every revision of both Models, `open_verified` succeeds, and
   revision 1 of the linked Model still hashes to the recorded value.

- [ ] **Step 2: Update `CONTEXT.md`**

Add or refine these terms as worded in spec §Product vocabulary:

- Model (G-code included)
- Unfiled
- Managed Model
- Linked Model
- Source state
- Import selection

- [ ] **Step 3: Close the known-unknowns rows**

In the approach doc, mark "Parser and watcher libraries" resolved (spec D9,
D10, D11, and D15, plus the spike report) and "Managed-content layout and
hashing" resolved (spec D3 and D4).

- [ ] **Step 4: Run the full verification**

Run each of these:

- `just build`
- `just test`
- `source "$HOME/.cargo/env" && just test-rust`
- `just gen-contracts`, then `git diff --exit-code src/generated`
- `source "$HOME/.cargo/env" && just package`

After packaging, install the `.deb` (or run the AppImage) on the Linux host.
Then:

- Pick two files through the native dialog.
- Drop one file from the file manager.
- Confirm that linked-file edits appear within 2 s.

- [ ] **Step 5: Run the manual checks with a display**

Run `just dev`:

- Walk the tracer by hand at 1440 × 900 and 1024 × 700.
- Complete the whole flow keyboard-only: sidebar, grid/list, import dialog,
  locate, and delete.
- Save screenshots to `docs/screenshots/p4-*.png`.

If there's no display, record these checks, Gate D, and the installed-bundle
checks as **unavailable**, not passed.

- [ ] **Step 6: Write the verification doc**

Mirror `docs/verification/2026-09-22-p2-printer-lifecycle.md`. Include:

- The automated evidence mapped to spec acceptance criteria 1–15.
- The spike report link.
- The manual evidence and any unavailable checks.
- The recorded deviations: clarification 4 (convert while missing), and any
  spike fallback taken.
- The final `COMMAND_NAMES` count and schema version after the rebase.

- [ ] **Step 7: Commit, push, and open a draft PR**

Commit with `docs: record P4 verification evidence`. Push and open a draft
PR referencing #14. Do this only when the delivery lead authorizes it.

---

## Delivery order and parallel work

```text
Spike:     T1
Backend:   T2 → T3 → (T4 may start right after T1) → T5 → T6 → T7 → T8
Frontend:  T9 (parallel with T2–T8) → T10 (needs T5–T8 contracts) → T11 → T12 → T13
Wiring:    reviews T5 path policy + drop, T6 idempotency/cancel, T7 ordering, T10 filter → T14
```

With a single implementer, run them strictly in numeric order. Task 2 waits
for P3 to merge, or for an explicit decision to take migration number
`0004`.

## External dependencies and blockers

- **P3 merge.** It sets the migration number, the command-count baseline,
  and the availability of `DataTable` and `Timeline`. The fallbacks are
  named in Tasks 11 and 13.
- **Slicer fixtures.** OrcaSlicer (flatpak `com.orcaslicer.OrcaSlicer`) and
  PrusaSlicer 2.9.6 (flatpak) are installed on the development host.
  Producing the fixtures needs a display. Without one, those fixtures are
  **unavailable** and acceptance criterion 2 is partial.
- **Network access** for `cargo fetch` of `notify`, `notify-debouncer-full`,
  and `zip` (`quick-xml` and `base64` are already vendored in the lockfile).
- **No Windows or macOS host.** Watcher and drop behavior there stays
  unverified, as the supported-platforms baseline requires.
- **User answers to Q1–Q6.** A different answer revises the tasks named in
  the spec before they start.

## Expected deliverables

- A new schema version with `library_projects`, `library_models`,
  `model_source_revisions` (immutable), `model_revision_thumbnails`,
  `content_blobs`, and `pending_blob_cleanup`.
- A content-addressed store under `farm3d-content/v1/blobs/sha256/` with
  crash-safe placement, startup sweep, and deferred cleanup.
- STL, 3MF, and G-code inspectors with a committed fixture suite.
- 16 new commands:
  - `list_library`
  - `create_project`, `rename_project`, `delete_project`
  - `update_model`, `delete_model`
  - `list_model_revisions`
  - `get_revision_thumbnail`
  - `pick_model_files`
  - `inspect_import_selection`
  - `import_models`
  - `cancel_import_selection`
  - `check_linked_sources`
  - `locate_linked_source`
  - `convert_model_to_managed`
  - `library_content_info`
- The `library` event stream, and type-filtered listeners for both
  streams.
- The Library workspace, the import dialog, recovery and management
  dialogs, and the `FileDropSurface` and `SegmentedControl` primitives.
- The spike report, the `CONTEXT.md` updates, and the P4 verification doc.
