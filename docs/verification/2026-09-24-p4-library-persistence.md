# P4 Library Persistence and Projects verification

**Date:** 2026-09-24
**Platform:** Linux x86_64 (Linux 7.2.6-1-cachyos, btrfs `$HOME`)
**Validated source:** the commit that adds this document
(`docs: record P4 verification evidence`) on
`feature/p4-library-persistence`, directly on top of `8e5e2d8` (`fix: scope
DataTable keys to its rows and hold Library dialogs open mid-request`).
Besides this document, that commit adds the tracer
(`src-tauri/tests/p4_tracer.rs`), the `CONTEXT.md` vocabulary, the umbrella
doc wording, and the `docs/screenshots/p4-*.png` captures. It changes no
application source.
Covers Tasks 1–14 (P4 in full), for GitHub issue #14.

**Final counts after the rebase onto P3:** `COMMAND_NAMES` holds **58**
commands (41 before P4, plus P4's 17), matching the `COMMAND_CONTRACTS`
array length. `CURRENT_SCHEMA_VERSION` is **5**, applied by migration
`0005_p4_library.sql`.

## Automated evidence

| Command | Result |
| --- | --- |
| `just build` | Passed: TypeScript type check and Vite production build. The main chunk is `index-*.js` at 493.74 kB (150.85 kB gzip), with no Vite chunk-size warning. The Import, Locate, Delete Model, and Project dialogs are separate lazy chunks. |
| `just test` | Passed: 64 files, 772 tests. The runner printed the same pre-existing jsdom `Window.scrollTo()` notices as P3, and exited 0. |
| `source "$HOME/.cargo/env" && just test-rust` | Passed: 395 library tests (1 ignored), 28 export-contract tests (1 ignored), and 3 `library_fixtures` tests (1 ignored, the `gen-library-fixtures` generator). The P4 integration files ran 27 `p4_content`, 10 `p4_contract_path`, 27 `p4_import`, 8 `p4_links`, 8 `p4_migration`, and **1 `p4_tracer`**. Every F0–P3 file passed unchanged: 5 `f0_tauri_path`, 3 `f1_contract_path`, 12 `f1_import_export`, 5 `f1_migration`, 7 `f1_repositories`, 12 `f1_residual_acceptance`, 15 `p2_batch`, 14 `p2_contract_path`, 10 `p2_lifecycle`, 6 `p2_migration`, 1 `p2_tracer`, 16 `p3_contract_path`, 11 `p3_ledger`, 20 `p3_lifecycle`, 8 `p3_migration`, 12 `p3_movement`, 10 `p3_reservations`, 13 `p3_setup`, 1 `p3_tracer`, and 3 `snapshot`. In all, 691 tests passed, 0 failed, and 3 were ignored. The build printed the existing `ts-rs` "failed to parse serde attribute" warnings (`transparent`, `double_option`), as before. |
| `source "$HOME/.cargo/env" && cargo fmt --manifest-path src-tauri/Cargo.toml --check` | Passed (exit 0). |
| `source "$HOME/.cargo/env" && just gen-contracts`, then `git diff --exit-code src/generated` | Passed: the regeneration ran clean under `--locked`, and the diff was empty. |
| `source "$HOME/.cargo/env" && just package` | Passed: a release build, then three bundles: `farm3d_0.1.0_amd64.deb` (8.4 MB), `farm3d-0.1.0-1.x86_64.rpm` (8.4 MB), and `farm3d_0.1.0_amd64.AppImage` (111.5 MB). `scripts/assert-package-contents.sh` passed on all three: each carries `usr/bin/farm3d` and `printer-catalog.json`, and none carries source, tests, fixtures, build output, or metadata. Installing and exercising a bundle is **unavailable** (see below). |
| `p4_tracer` alone, 20 consecutive runs | 20 of 20 passed, about 1.9 s each. |

### The tracer (spec acceptance criteria 4, 5, 6, 7, and 15)

`src-tauri/tests/p4_tracer.rs`,
`the_tracer_imports_restarts_follows_a_linked_source_and_deletes_a_project`,
drives the real `tauri::test` IPC path in one test. Its source files live in
a temp directory outside farm3d's own trees, and its linked source uses
real native `notify` watches.

1. `create_project` for "Brackets" and for "Calibration".
2. One import selection is registered exactly as the drop handler registers
   one (clarification 2). It holds `cube-binary.stl` and a temp-dir file
   named `orca-two-plates.3mf`, which is the in-test stand-in (see AC 2).
   `inspect_import_selection` reports both files `ready`. The 3MF has
   `plateCount: 2`, and one unsupported entry,
   `Metadata/project_settings.config`. `import_models` imports the STL as
   **managed** into both Projects, and the 3MF as **linked** into
   "Brackets" with `acknowledgeUnsupported: true`. The STL's `projectIds`
   are `[Brackets, Calibration]`, and the linked Model's `link.path` is the
   selected path.
3. Both revision-1 hashes are recorded. Each equals the SHA-256 of its
   source bytes.
4. **Restart.** The first app's `LinkSupervisor` is shut down explicitly
   (P19: the AppHandle → services → supervisor cycle means dropping them
   does not stop it), and the test drops its handles to the first app. A
   new `Storage` is then opened over the same metadata and content roots,
   with a new `RuntimeServices` and IPC app. The P3 tracer reused one
   `Storage` across its restart; this one reopens the database. `start_library_runtime` runs as
   `build_runtime_services` runs it. After the startup pass, `list_library`
   returns both Models with the same ids, hashes, and `projectIds`. The
   linked Model is `ok`, `watching`, with one revision.
5. The linked file is overwritten with `core-two-objects.3mf` through a
   temp file and a rename. `library.revision.created` arrives with
   `sequence: 2`, `origin: "linkedChange"`, and the new hash. No polling was
   needed.
6. The linked file is deleted, and the Model's source state becomes
   `missing`.
7. The revision-2 bytes are copied to a new directory. A `locate` selection
   is registered, and `locate_linked_source` is called with
   `acceptDifferentContent: false`. The Model is `ok` at the new path, with
   `revisionCount: 2` and no second `library.revision.created`.
8. Every revision of both Models (1 + 2) opens through `open_verified` and
   reads to the end. Each blob's SHA-256 equals its revision's hash.
   Revision 1 of the linked Model still has the recorded hash, and its
   bytes equal the original two-plate package.
9. `delete_project "Calibration"` lists the STL in `affectedModelIds`, with
   `nowUnfiledModelIds: []`. Only "Brackets" is left. The STL's `projectIds`
   are `[Brackets]`, the linked Model's are unchanged, both Models remain
   with 1 and 2 revisions, and all three blobs still verify.

**The tracer fails when it should.** Three deliberate mutations, each
reverted afterwards:

- `delete_project` changed to strip every membership of the affected
  Models. The tracer failed at step 9: `nowUnfiledModelIds` held the STL.
- The restarted app changed to poll only, every hour. The tracer failed at
  step 4 (`watchMode` was `polling`, not `watching`).
- The first supervisor leaked instead of shut down (`std::mem::forget`).
  The tracer failed at step 5 in 3 of 3 runs. The leaked supervisor took the
  save, so the restarted app never saw `library.revision.created`. The P19
  shutdown is therefore load-bearing.

The tracer passed on its first run. P4's behavior was already implemented
by Tasks 2–13, so there was no red phase to capture. The mutations above
stand in for it.

### Acceptance criteria 1–16, mapped to evidence

| # | Criterion | Evidence |
| --- | --- | --- |
| 1 | The migration applies, is ledgered, survives crash-boundary injection, and leaves Printer and P3 data unchanged. Revisions reject `UPDATE`. | `p4_migration.rs`: `fresh_database_records_the_v5_ledger_row_with_a_matching_checksum`, `upgrading_v4_to_v5_leaves_printers_and_spools_unchanged`, `a_crash_before_commit_leaves_the_database_unchanged_at_v4`, `revisions_are_immutable_and_deleting_a_model_cascades_everything`, plus the CHECK, unique-index, and `RESTRICT` tests. |
| 2 | **Partial.** Every D20 fixture's inspection equals its `*.expected.json`, and every rejection fixture returns its code. | `library_fixtures.rs`: `every_fixture_matches_its_expected_inspection` covers all 17 committed fixtures, including `prusa-project.3mf` and `prusa-cube.gcode`. There are also `core_two_objects_yields_its_embedded_thumbnail` and `prusa_project_dangling_thumbnail_relationship_is_no_thumbnail`. `orca-two-plates.3mf` and `orca-cube.gcode` are **unavailable** (see "Deviations and interpretations"). Plates are covered by the in-test stand-ins: the `threemf.rs` unit test `reads_orca_production_parts_plates_and_unsupported_settings`, `p4_import.rs`'s `rich_3mf`/`orca_style_gcode`, and the tracer's two-plate package. |
| 3 | Content store: identical content is stored once; an orphan blob is swept at startup; no row exists without its file; cleanup after `delete_model` removes only unreferenced blobs and survives a failed unlink. | `p4_content.rs`: `placement_creates_a_read_only_blob_and_commits_the_closure_rows`, `a_crash_between_placement_and_commit_leaves_an_orphan_the_sweep_removes`, `unreferenced_blobs_move_to_pending_cleanup_and_are_released`, `mark_unreferenced_blobs_keeps_blobs_a_revision_still_references`, `a_failed_unlink_stays_pending_until_the_startup_sweep`, `a_blob_reimported_before_release_is_kept`, `verified_reads_return_intact_bytes_and_reject_damaged_ones`. `p4_contract_path.rs`: `deleting_a_managed_model_releases_only_the_blobs_nothing_else_holds`. |
| 4 | Managed and linked imports survive restart with identical hashes, and linked Models resume watching. | `p4_tracer.rs`, step 4. `p4_links.rs`: `a_linked_model_survives_a_restart_and_an_edit_while_stopped_is_captured`. |
| 5 | Editing a linked file creates revision 2, and revision 1's bytes still verify. Deleting it sets `missing`; restoring it sets `ok` with no new revision. A mid-write change sets `changing`, then settles. | `p4_tracer.rs`, steps 5, 6, and 8. `p4_links.rs`: `an_atomic_save_adds_exactly_one_revision_and_keeps_revision_1_readable`, `a_deleted_source_goes_missing_and_recovers_when_restored`, `a_removed_parent_directory_is_followed_through_its_ancestor`. `library/links.rs` unit tests: `a_source_changing_during_the_copy_is_changing`, `a_changing_source_is_retried_at_2_4_and_8_seconds_then_left`, `retries_stop_once_the_source_settles`. |
| 6 | Locating a moved file with the same content relinks with no revision. A different file needs `acceptDifferentContent`, then adds a `relocate` revision. Convert to managed works while `missing`. | `p4_tracer.rs`, step 7. `p4_links.rs`: `locate_relinks_same_content_and_needs_consent_for_different_content`, `converting_a_missing_model_to_managed_stops_following_its_source`. |
| 7 | Project membership: two Projects give `projectIds` of length 2; the last removal makes it Unfiled; a repeated add is a no-op with no event; deleting a Project deletes no Model, revision, or blob, and lists the newly Unfiled; import with two `projectIds` creates both; `useExisting` only adds. | `p4_tracer.rs`, steps 2 and 9. `p4_contract_path.rs`: `set_model_projects_edits_membership_as_a_set`, `deleting_a_project_keeps_its_models_and_reports_the_newly_unfiled`. `p4_import.rs`: `a_managed_stl_imports_into_two_projects_with_its_first_revision`, `an_unfiled_import_has_no_membership_and_repeated_project_ids_collapse`, `duplicate_resolution_is_explicit_for_every_action`. `p4_migration.rs`: `membership_is_many_to_many_and_deleting_a_project_removes_only_its_own_row`. |
| 8 | A duplicate without a decision is rejected; **Use existing**, **Add as another Model** (one blob), and **Add as a new revision** each behave as in D14. | `p4_import.rs`: `duplicate_resolution_is_explicit_for_every_action`, `inspection_reports_every_model_holding_the_same_bytes`. `src/library/import-flow.test.ts` and `src/screens/ImportDialog.test.tsx` cover the dialog side, including the commit-time re-check. |
| 9 | Unsupported 3MF entries are reported before import, an unacknowledged row is rejected, and the stored bytes equal the source. | `p4_import.rs`: `an_unacknowledged_rich_3mf_is_rejected_and_writes_nothing`, `a_linked_3mf_records_the_selected_path_its_stat_and_its_thumbnail`. `p4_tracer.rs`, steps 2 and 8. |
| 10 | G-code is inspected, its claims are stored verbatim with `trusted: false`, and its bytes are kept exactly. No Slice Revision, Queue, or dispatch control appears. Failure and cancellation at each injection point leave nothing behind. | `p4_import.rs`: `gcode_is_retained_byte_for_byte_with_its_claims_untrusted`, `a_gcode_inspection_failure_writes_nothing_and_a_fresh_selection_imports`, `a_crash_between_placement_and_commit_leaves_only_an_orphan_and_a_retry_imports`, `the_startup_sweep_removes_the_orphan_a_failed_commit_leaves`, `cancelling_between_staging_and_placement_writes_nothing`, `cancelling_during_inspection_cancels_unfinished_items_and_discards_the_selection`. `src/screens/ModelDetailsPanel.test.tsx` shows claims as untrusted, with no slicing or dispatch control. |
| 11 | No command accepts a raw path. An expired selection returns `SELECTION_EXPIRED`. A drop registers a selection and emits `library.selection.dropped`. | `p4_import.rs`: `no_command_request_carries_a_filesystem_path`, `a_selection_expires_thirty_minutes_after_its_last_use`, `a_window_drop_registers_the_files_and_emits_a_path_free_summary`, `farm3d_trees_are_refused_and_a_symlink_elsewhere_keeps_its_own_name`. |
| 12 | Library events go out after commit only, and backfill ordering holds. A `library.*` event causes no Printer status backfill, and the Library store ignores `printer.status.*`. | `p4_contract_path.rs`: `the_snapshot_sequence_precedes_the_next_change`, plus the event assertions in each command test. `src/printers/printer-store.test.ts` (the `library.model.changed` case) and `src/library/library-store.test.ts` (backfill, replay, the revision guard, and the `printer.status.*` filter). |
| 13 | The watcher spike is recorded with its pass/fail result on Linux x86_64, and the chosen mode is implemented. | `docs/superpowers/baselines/2026-09-24-p4-format-watcher-spike.md`: Gates A–C pass, and Gate D is unavailable. Native `notify` with a 750 ms debouncer is implemented in `library/links.rs`. `p4_links.rs`: `a_failed_watch_falls_back_to_polling_with_a_warning`, `check_linked_sources_returns_only_changed_models_without_a_watcher`. |
| 14 | Frontend tests cover the store, saved views, the import-flow reducer, the import dialog, the sidebar, grid and list, the details panel, membership editing, locate and convert, the project dialogs, and deep links. | `src/library/library-store.test.ts`, `saved-views.test.ts`, `import-flow.test.ts`, `types.test.ts`, `web-fixtures.test.ts`; `src/screens/ImportDialog.test.tsx`, `LibrarySidebar.test.tsx`, `LibraryWorkspace.test.tsx`, `ModelGrid.test.tsx`, `ModelList.test.tsx`, `ModelDetailsPanel.test.tsx`, `LocateSourceDialog.test.tsx`, `ProjectDialogs.test.tsx`, `DeleteModelDialog.test.tsx`; `src/App.test.tsx` (deep links); and the `FileDropSurface`, `SegmentedControl`, `Combobox`, and `Chip` component tests. All are part of the 772-test `just test` run above. |
| 15 | The tracer completes through the Tauri path. | `p4_tracer.rs` (new; see above). |
| 16 | **Partial.** Keyboard and window-size checks pass at 1440 × 900 and 1024 × 700, and `just package` succeeds on Linux, with the installed bundle picking and dropping files. | `just package` **passed**. The keyboard and window-size checks **passed in web mode** (below). The native window, the installed bundle, and Gate D (drop and picker) are **unavailable** and need a human (below). |

## Visual and keyboard verification

These checks ran in headless Chrome (`chrome-headless-shell` through
`playwright-core`) against `vite` in web mode, at 1440 × 900 and
1024 × 700. `just web` has no Rust backend, so every state is the web-mode
fixture: five Models and the Projects "Brackets" and "Calibration". Import
and Check sources are disabled there with "needs the desktop app". No
console errors were logged.

Screenshots, in `docs/screenshots/`:

- `p4-library-grid-1440x900.png`: three panes. The sidebar shows the five
  saved views with counts, then the Projects. The grid shows "Source
  missing" on Cable clip, and the details panel shows "Corner bracket" with
  its **Brackets** and **Calibration** membership chips.
- `p4-library-list-1440x900.png`: list mode, with Name, Projects, Format,
  Storage, Source, Revisions, Added, and Actions columns. A Model in two
  Projects lists both.
- `p4-missing-source-1440x900.png` and `p4-locate-dialog-1440x900.png`: the
  missing linked Model shows its path, both revisions, **Locate source…**,
  and **Convert to managed**. The Locate dialog names the missing file and
  its last path, and says locating needs the desktop app.
- `p4-library-1024x700.png` and `p4-details-overlay-1024x700.png`: at 1024
  wide, the details panel is hidden, and **Details** appears in the
  toolbar. The panel then opens as an overlay.
- `p4-keyboard-chip-focus-1440x900.png`: the focus ring on the first
  membership chip's remove control, reached by Tab alone (37 presses from
  page load).
- `p4-keyboard-membership-1024x700.png`: the same Model after its
  "Calibration" membership was removed by keyboard alone. The Calibration
  count drops from 3 to 2.

Keyboard-only checks, all passed:

- **SegmentedControl arrows.** Tab reaches the Grid option. ArrowRight
  selects List, and the table replaces the grid. ArrowLeft selects Grid
  again. This is the browser check that the T9 ruling deferred here: jsdom
  can't simulate native radio arrow keys.
- **Sidebar.** Tab reaches "Calibration", and Enter opens that Project's
  view (`#nav=v1/library/project/prj-web-calibration`).
- **Details overlay and membership chips (1024 × 700).** Tab reaches
  **Details**, and Enter opens the overlay with focus on **Close details**.
  Tab cycles inside the overlay: Name, then each chip's toggle and its
  remove control, Add to Project…, Delete…, and back to Close. Enter on
  Calibration's remove control removes the membership.

**Finding:** after a chip is removed by keyboard, focus falls to `<body>`
rather than moving to the next chip or the Add to Project… field. It is
listed under Known follow-ups, with the similar delete finding from
Task 13.

Still **unavailable**, and needing a human. The desktop is a shared, live
session, so this pass didn't run `just dev`, install a bundle, or drive or
screenshot the desktop.

- **The native window (`just dev`)** at 1440 × 900 and 1024 × 700. Steps:
  1. Run `just dev` in the worktree.
  2. Walk the tracer by hand. Create "Brackets" and "Calibration". Import
     `src-tauri/tests/fixtures/library/cube-binary.stl` (managed) into
     both, and a copy of `core-two-objects.3mf` in a scratch directory
     (linked) into "Brackets".
  3. Quit and relaunch. Both Models, their chips, and the linked Model's
     `ok` state are unchanged.
  4. Overwrite the scratch 3MF with other bytes. Revision 2 appears in the
     details panel's history within about 2 s.
  5. Delete the scratch file, and check that the Model shows "Source
     missing". Move a copy of the revision-2 bytes elsewhere, then use
     **Locate source…** and pick it. The Model is `ok` with no new
     revision.
  6. Delete "Calibration" from the sidebar menu. The STL keeps "Brackets".
  7. Repeat steps 2–6 by keyboard alone, covering the sidebar, grid/list,
     the import dialog (two Projects on one row), the membership chips,
     Locate, and delete. Repeat at both window sizes.
- **Gate D, drop and picker.** Follow the manual steps in the spike
  report's "Gate D: drop and picker" section. Now that P4 ships the real
  picker and drop handler, the temporary code there is no longer needed.
  1. In `just dev`, click **Import…** and Ctrl-select two fixture files
     in the native dialog. Check that the dialog shows the "3D models and
     G-code" filter, and that the import dialog lists both files.
  2. Drag one file from the file manager onto the Library. The drop
     surface highlights, the import dialog opens with that file, and no
     capability or permission error appears in the terminal.
  3. If either step reports a capability error, add exactly the named
     permission to `src-tauri/capabilities/default.json` and record it.
     Today that file holds `core:default`, `dialog:allow-open`, and
     `dialog:allow-save`.
- **The installed bundle.** The development host is Arch-based (CachyOS),
  so neither the `.deb` nor the `.rpm` installs natively there. Run
  `src-tauri/target/release/bundle/appimage/farm3d_0.1.0_amd64.AppImage`
  directly, or install the `.deb` on a Debian or Ubuntu machine with `sudo
  apt install ./farm3d_0.1.0_amd64.deb`. Repeat the Gate D picker and drop
  steps in the bundled app. Then import one file as linked, edit it in
  place, and confirm that revision 2 appears within 2 s.
- **Windows and macOS** watcher and drop behavior stays unverified, as the
  supported-platforms baseline requires.

## Spike

`docs/superpowers/baselines/2026-09-24-p4-format-watcher-spike.md` (Task 1)
records:

- **Gate A:** the 3MF reader passed with a substitution, since the Orca
  fixture is unavailable.
- **Gate B:** SHA-256 ran at about 1.5 GB/s.
- **Gate C:** native watches gave one debounced batch within 2 s, in 20 of
  20 repetitions for each of six scenarios. The watch limit fails with
  `MaxFilesWatch`, as the fallback expects.
- **Gate D:** unavailable.

No spike fallback was taken:

- D10 keeps `zip` + `quick-xml` with the Production extension, and
  `lib3mf` was never needed.
- D15 keeps native `notify` watches, with `PollWatcher` only as the
  per-directory fallback.
- D3 keeps SHA-256.

## Umbrella documents and vocabulary

User decision 4 (spec §Status) makes Projects many-to-many and flat, and
keeps the name "Project". That is a recorded user decision, so **no new ADR
is needed**. This task updates the documents to match:

- `CONTEXT.md`:
  - **Project** now reads "An organizational grouping of Models in the
    Library. A Model may belong to any number of Projects, or none
    (Unfiled)…", and its `_Avoid_` line is kept.
  - **Model** now includes pre-sliced G-code.
  - **Unfiled**, **Managed Model**, **Linked Model**, **Source state**, and
    **Import selection** are added, as worded in spec §Product
    vocabulary. Unfiled means a Model in no Project.
- `docs/superpowers/specs/2026-09-16-complete-v1-ui-workflows-design.md`:
  - "Organizational Project folders" becomes "Organizational Project
    groupings".
  - "Projects are folders, not orders" becomes "Projects are organizational
    groupings, not orders". That sentence wraps across two lines in the
    source.
  - "Projects are organizational folders only" becomes "Projects are
    organizational groupings only. A Model may belong to any number of
    Projects, or none (Unfiled)".
- `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`:
  - "Projects are organizational groupings (many-to-many with Models), not
    orders".
  - The known-unknowns rows "Parser and watcher libraries" (spec D9, D10,
    D11, and D15, plus the spike report) and "Managed-content layout and
    hashing" (spec D3 and D4) are marked **Resolved (P4)**.

## Deviations and interpretations

Summarized from the controller ledger's rulings
(`.superpowers/sdd/2026-09-23-p4-library-persistence-projects/progress.md`).
Each one changes or narrows documented behavior, or corrects the spec's
evidence.

- **Clarification 4: Convert to managed while missing (intentional
  deviation for the product owner).** The umbrella design says "after the
  source is located, **Convert to managed** copies it". P4's D5 is
  deliberately broader: every revision's bytes are already stored, so
  conversion copies nothing and works in any source state, including
  `missing`. Evidence:
  `p4_links.rs::converting_a_missing_model_to_managed_stops_following_its_source`.
  The product owner should confirm this or narrow it.
- **D2 of the preflight: `project_models` is written `WITHOUT ROWID,
  STRICT`.** The spec's §Migration text has `) STRICT, WITHOUT ROWID;`.
  SQLite table options are an unordered list, so the two forms mean the
  same thing. The reordering keeps the existing schema guard (the stored SQL
  ends in ` STRICT`) intact instead of weakening it.
- **D9's evidence line is partly wrong; the decision stands.** The spike
  found that `stl_io` 0.11 **does** parse a binary STL whose header starts
  with `solid `, *unless* every byte up to the first newline is valid UTF-8.
  Its ASCII probe calls `read_line`, which fails on the first invalid UTF-8
  byte and falls back to the binary reader. D9's decision still holds for
  the other reasons it gives:
  - A bare `solid` ASCII file fails.
  - Detection depends on the data.
  - NaN and zero-triangle files are accepted.
  - The format-specific readers are private.

  Read D9's evidence sentence as "can fail to parse", not "fail to parse".
- **D6: `ModelSourceRevisionRecord.sourcePath` returns the full path.** The
  spec's §Domain types names `sourcePath` on the revision record, and the
  revision history shows it. The plan's constraint that `ModelRecord.link.path`
  is the *only* full path crossing to the UI was stricter than the spec.
  Errors, warnings, and events still carry basenames only.
- **D14: the same-name pre-fill suggests only managed Models of the same
  format.** An `addRevision` target must be a managed Model of the same
  format, so the UI suggests only Models that can actually accept the
  revision. Separately, `import_models` **re-checks duplicates at commit
  time**. A row with no `duplicateAction`, whose hash is now in the Library
  (an intra-selection twin, or a racing import), is rejected with
  `DUPLICATE_DECISION_REQUIRED`, so resolution is never silent.
- **D15: link-check retries run at 2, 6, and 14 s after the first
  `changing`.** The spec says "retries after 2, 4, and 8 s". The
  implementation sleeps 2, then 4, then 8 s between retries, so they land
  at 2, 6, and 14 s after the first `changing`.
- **D15: `TOO_LARGE` has no carrier on `ModelRecord.link` (a spec gap).**
  A linked source over 1 GiB becomes `unreadable`, as D15 says, but
  `ModelLink` has no field for the `TOO_LARGE` reason. The UI therefore
  shows it as plain `unreadable`. A later phase needs a reason field, or a
  warning, to surface it.
- **AC 2 is partial: the Orca fixtures are unavailable.**
  - `orca-two-plates.3mf` and `orca-cube.gcode` were not produced. No
    OrcaSlicer flatpak is installed; the only Orca binary on the host is an
    unapproved nightly AppImage, and running it is the user's call.
  - Plates and Orca's package layout are covered by in-test stand-ins
    (listed in the AC table).
  - As a real-world check, the Task 4 review ran the inspector over 35
    `.3mf` files already on the host, read-only. 33 were accepted. One was
    not a ZIP. One was a BambuStudio project with **no objects**, which D10
    rejects with "This 3MF contains no objects."
  - **Note for the product owner:** BambuStudio can save such an empty
    project, and farm3d will refuse it.
- **Other rulings that shape behavior:**
  - A 3MF's `triangleCount` counts triangles as placed: each build item
    or component placement counts its mesh, consistent with per-placement
    `boundsMm`.
  - A dangling 3MF thumbnail relationship, as in `prusa-project.3mf`, is
    "no thumbnail", not an error.
  - G-code claims are a per-key map: the first value wins, and each value
    is at most 1 KiB. Thumbnail and tool lists are capped at 64.
  - `DUPLICATE_NAME` is a warning, returned by `update_model` and on
    `import_models` item results.
  - Web-mode refusals use `PERSISTENCE_UNAVAILABLE` with "needs the desktop
    app", not a dedicated code.
  - The details panel becomes an overlay below 1280 px. The spec fixes only
    1440 (three panes) and 1024 (overlay).
- **P9 note (preflight §7): pre-import snapshots now include Library rows
  but no blobs.** The Printers and Settings import snapshots copy the
  SQLite database, so after P4 they carry the Library tables but none of
  the content blobs under `content_root`. No app code restores snapshots
  today, so nothing is broken now. Any P9 restore must treat content blobs
  as part of the snapshot, or re-verify revisions against the blob store.

## Known follow-ups

Deferred minors from the task reviews, grouped and deduplicated. None
blocks an acceptance criterion, and none needs a migration or a wire-shape
change unless marked.

**Content store**

- `place()` checks containment with `starts_with`, so a `..` path would
  pass. `StagedFile` fields are `pub`.
- The re-stat reads the path's metadata before opening it. A FIFO swapped
  in blocks, so prefer `file.metadata()` with dev and inode.
- A release racing a re-import can leak a blob until the next startup.
  Stray files directly under `blobs/sha256/` are never swept.
- A poisoned placement lock breaks the store until restart.
- A missing or unreadable blob reads as `INTERNAL`, arguably
  `CORRUPT_DATA`; flag this for P5.
  `get_revision_thumbnail` has the same issue, and reads up to 1 MiB on
  the command thread.

**Format inspectors**

- STL: `facet` is matched as a substring. A multi-solid ASCII file stops at
  the first `endsolid`.
- G-code: an unclosed thumbnail block swallows later comments. Compact
  moves such as `G1X10` are not recognized.
- ZIP:
  - The ratio check trusts the declared compressed size.
  - A ZIP64 end record naming a directory past EOF reports `Io`, not
    `INVALID_CONTENT`.
  - A corrupt ZIP's EOF reports `SOURCE_UNREADABLE`.
  - The hidden-signature scan can falsely reject, at about 2⁻³¹ per byte.
- A failing thumbnail fails the inspection instead of warning
  `THUMBNAIL_SKIPPED`.
- Cancellation isn't checked inside very long newline-free lines,
  whitespace runs, or `ZipArchive::new`.

**Selection, import, and links**

- The drop handler calls `bootstrap.ready()`, which can trigger a
  bootstrap retry; `is_ready()` avoids that.
- `remove_dir_all` runs off `spawn_blocking`. Expired selections are
  reclaimed lazily; the supervisor tick could sweep them.
- Under cancellation, any failure is reported as cancelled. A concurrent
  inspect after a cancel returns `CANCELLED`, not `SELECTION_EXPIRED`.
- Once every ready item has committed, staging is discarded, so a late
  re-commit gets `PERSISTENCE_UNAVAILABLE`.
- `has_same_name_model` scans every Model (O(items × models)).
- `project_write_error` maps every name `VALIDATION` to "already exists".
- The supervisor's gate map grows with any `modelIds`, which are uncapped.
- A failing `PollWatcher` gives no `WATCH_UNAVAILABLE`.
- Re-establishing or moving a watch changes `watchMode` without a
  `library.model.changed`.
- Locate inspects the file only after consent, so a corrupt file first
  reports `SOURCE_CONTENT_DIFFERS`.

**Frontend**

- A stale `startLibrary` listen error isn't guarded, and
  `refreshContentInfo` responses aren't ordered.
- A targeted `checkSources` shares the 30 s throttle.
- The web thumbnail cache can pin `null` before `startLibrary`, and a
  thumbnail load failure falls back silently.
- A whitespace-only name round-trips to the backend.
- `SELECTION_EXPIRED` leaves Import enabled.
- **Focus falls to `<body>`** after a Model delete (Task 13) and after a
  membership chip is removed by keyboard (this pass).
- The "Checked just now" live region is created with its text, so it may
  not announce.
- Two separate web-mode toolbar notes, where one would do.
- The `requestClose`/busy gate is hand-rolled in four dialogs.
- The main chunk has about 6 kB of headroom under Vite's 500 kB advisory.
- Design-system polish:
  - FileDropSurface's disabled opacity compounds the disabled text
    contrast.
  - The SegmentedControl focus ring can bleed 2 px into an adjacent
    selected segment.

**Test gaps**

- `useExisting` with a stale `targetExpectedRevision`.
- Cancel while importing, and close during inspection.
- A commit-time duplicate that matches only an older revision.
- The `LibraryWorkspace` forwarding of `dropRefused`.
- One backfill after a gap, and a re-entrant `startLibrary` during listen.
- A `status: error` with a pending deep link.
- `rename_project`'s duplicate path through IPC.
- Some integration tests rely on an mtime tick or a short sleep.
- The test hooks are `#[doc(hidden)] pub`.

**Pre-existing, outside P4:** the design-system RadioGroup and Checkbox
compose their `focusRing` on a Control element that never takes focus, and
DropdownMenu triggers have no focus ring. Their keyboard focus rings likely
never render. SegmentedControl, new in P4, was fixed in Task 9.

## Files changed in this task

- `src-tauri/tests/p4_tracer.rs` (new): the tracer (spec acceptance
  criterion 15).
- `CONTEXT.md`: the Project redefinition, the refined Model entry, and the
  new Unfiled, Managed Model, Linked Model, Source state, and Import
  selection entries.
- `docs/superpowers/specs/2026-09-16-complete-v1-ui-workflows-design.md`:
  "folders" becomes "groupings", in three places.
- `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`:
  the Projects wording, and two known-unknowns rows marked resolved.
- `docs/screenshots/p4-*.png` (new, 8 files): the web-mode visual and
  keyboard captures listed above.
- `docs/verification/2026-09-24-p4-library-persistence.md` (this file).
