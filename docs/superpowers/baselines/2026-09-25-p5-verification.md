# P5 Runtime Slicing and Slice Revisions Verification

Independent verification of P5 (GitHub issue #15), Task 16b of
[`docs/superpowers/plans/2026-09-24-p5-runtime-slicing-slice-revisions.md`](../plans/2026-09-24-p5-runtime-slicing-slice-revisions.md).
The acceptance criteria are in
[`docs/superpowers/specs/2026-09-24-p5-runtime-slicing-design.md`](../specs/2026-09-24-p5-runtime-slicing-design.md).

Verified on 2026-09-25 on branch `feature/p5-runtime-slicing` at `da962da`.
This file records what was observed. Where a check could not be run, it
says so.

**Overall:** all automated gates pass. The tracer passes against fake-orca
and against the real OrcaSlicer v2.4.2. The packaged farm3d sliced the
two-plate fixture with the installed v2.4.2 AppImage. Two defects were found
in the full user workflow (D1, D2 below), and some criteria are only
partially met.

## Environment

- Linux 7.2.6-1-cachyos, x86_64. KDE Plasma on Wayland, with Xwayland 24.1.13.
- Node.js 24.19.0 (nvm). Rust via rustup (`source "$HOME/.cargo/env"`).
- OrcaSlicer v2.4.2 AppImage at
  `~/Downloads/OrcaSlicer_Linux_AppImage_Ubuntu2404_V2.4.2.AppImage`, with
  SHA-256 `d12fb8c8eac1aecd2dfb6377acd48f994f8fa439ed5292fa532dd82880f029fd`
  (the spike's hash).

## Automated verification matrix

Each command below was run in one sequence at `da962da`, with
`source "$HOME/.cargo/env"`.

| Command | Exit | Result |
|---|---|---|
| `just build` | 0 | `tsc` and the Vite build pass. |
| `just test` | 0 | 89 files, 1142 tests passed. |
| `just test-rust` | 0 | 959 passed, 0 failed, 13 ignored, across 39 test binaries. No flake this run. |
| `just gen-contracts`, then `git diff --exit-code src/generated` | 0 | No diff. |
| `just gen-slicing-fixtures`, then `git diff --exit-code` | 0 | No diff. |
| `FARM3D_ORCA=~/Downloads/OrcaSlicer_Linux_AppImage_Ubuntu2404_V2.4.2.AppImage just test-orca` | 0 | All 8 `real_orca*` tests pass (list below). |
| `just package` | 0 | deb, rpm, and AppImage bundles. `scripts/assert-package-contents.sh` passed. |

The `real_orca*` tests that passed under `just test-orca`:

- `real_orca_places_a_written_plate_exactly` (4.4 s)
- `real::real_orca_cancel_stops_the_group_quickly`
- `real::real_orca_slices_under_supervision` (these two: 18.3 s)
- `real::real_orca_an_off_bed_purge_in_the_start_gcode_still_publishes` (6.9 s)
- `real_orca_engine_probes_as_a_supported_version`
- `real_orca_preset_source_knows_every_mapped_key` (these two: 7.3 s)
- `real_orca_slices_a_cube_through_the_commands` (3.3 s)
- `real_orca_tracer_runs_against_a_real_orcaslicer` (31.0 s)

No FUSE mount was left after `just test-orca` (`mount | grep -c /tmp/.mount_`
= 0).

### Package artifacts (this run)

| Artifact | Bytes | SHA-256 |
|---|---|---|
| `bundle/deb/farm3d_0.1.0_amd64.deb` | 9,481,748 | `4e09b0424f3bed28be66e79ee8d1d9f147329b7413a85e1b491aebf102489a0a` |
| `bundle/rpm/farm3d-0.1.0-1.x86_64.rpm` | 9,481,094 | `8c3513f4b7b89626333c24722b3268662406a82e606439de093f4499cb8e402c` |
| `bundle/appimage/farm3d_0.1.0_amd64.AppImage` | 112,499,192 | `68dfb449fa6bfaa8db50f2ec8c27cd21f5db976e2e0ae7b9d077e05833d67377` |
| `usr/bin/farm3d` inside the .deb | – | `d67f6d0e479313d630dfcf395f017a51085727a60b6affbbd2c8de6ea03c9c38` |

The .deb payload holds the binary, `usr/lib/farm3d/resources/printer-catalog.json`,
the desktop file, and three icons. `strings usr/bin/farm3d | grep -c
'fake-orca\|FAKE_ORCA_SCENARIO'` = 0.

## How the native app was driven

**Display.** `just dev` launched and rendered on the real display `:0`
(`GDK_BACKEND=x11`, 1440 × 900). XTest input did not reach the window there:
KWin's rootless Xwayland dropped both synthetic keys and clicks. So the
manual pass ran on a **rootful nested Xwayland** (`Xwayland :9 -geometry
1440x900`). That is a window on the same KWin desktop, with the same GPU,
and the same `just dev` build and webview. The native app itself was
checked; the only difference is which X server delivered the keys. No web
mode or fixtures were used for any result below.

**Tools.** Everything already on the machine; nothing was installed:

- Keys: python-xlib XTest.
- Screenshots: X `GetImage`, then `magick`.
- Focus and state: AT-SPI (`gi.repository.Atspi`). An AT-SPI focus listener
  reported each focus change.
- Occasionally, `Atspi.Component.grab_focus` jumped to a region during
  setup, after a keyboard path had already been shown once. It was never
  used for the checks themselves.

**Data.** The app ran with a scratch `HOME` and XDG directories, so the
user's real farm3d data was not touched. The scratch `~/Downloads` held only
a symlink to the v2.4.2 AppImage. Every slice used that real engine; farm3d
discovered and probed it, and extracted its presets.

## Manual pass (native `just dev`, real v2.4.2)

The screenshots are in `docs/screenshots/`, the path AC14 names and earlier
phases used.

| Check | Result | Evidence |
|---|---|---|
| Import `orca-two-plates.3mf`, `orca-cube.gcode`, and a 998,000-triangle STL sphere through the native GTK file chooser, by keyboard (Ctrl+L, path, Enter) | Done. The unsupported-contents acknowledgement was checked with Space. | – |
| Prepare… opens the workspace, and focus moves to "Preparing orca-two-plates" | Pass. The Elegoo Centauri Carbon presets from v2.4.2 resolved. The Library sidebar is collapsed at 1440 × 900. | `p5-1440-workspace.png` |
| Focus order | Pass up to the panel: plate tabs, then viewport toolbar, then viewport (`role=img`), then object list, then fields, then panel. See **D1** for the panel. | AT-SPI focus log |
| Select (`]`), move by arrows (→→ gives X +2 mm; Shift+↑ gives Y +10 mm), rotate (`R`, 15°) | Pass. The live description read "Selected: cube-for-slicers.stl, at X 130.0 mm, Y 138.0 mm; rotated 0°, 0°, 15°…". | `p5-1440-keyboard-move-rotate.png` |
| Lay flat (`F`), top view (`1`), arrange (`A`) | Pass. The description showed a −90° Y rotation, and the status showed "Arranged 1 object." | – |
| Measure (`M`), duplicate (Ctrl+D), then choose the second object in the measure select | Pass: "Centre to centre: 17.2 mm. Gap: 5.0 mm.", with an overlay line. | `p5-1440-measure.png` |
| Move by number: X field set to 60 | Pass. The description showed X 60.0 mm. | – |
| Move to plate (Shift+2), then the Plate 2 tab (→ in the tab list), then Delete | Pass: "Moved cube-for-slicers.stl 2 to Plate 2." and "Removed from the plate." | `p5-1440-plate2-tab.png` |
| Slice all plates | Pass. Two revisions, both Sliced. | `p5-1440-sliced-two-plates.png` |
| Revision review | Pass. Provenance shown as icon and text; queue disabled with a visible reason; "Engine 2.4.2 · presets 2.4.2"; estimates. | `p5-1440-revision-review.png`, `p5-1440-revision-runtime.png` |
| Progress | Pass. "Optimizing toolpath 70%"; "Already slicing Plate 1." shown as text. | `p5-1440-slice-progress.png` |
| Cancel (keyboard: Tab to Cancel, Enter) on a 29 s real slice | Pass. The engine was gone 0.26 s after Enter. State `cancelled`, no revision, a log blob stored, work dir removed, 0 FUSE mounts. | `p5-1440-interrupted-failed-cancelled.png` |
| Failure log: `kill -SEGV` on the running `orca-slicer` | Pass. "OrcaSlicer stopped unexpectedly (signal 11).", `engineCrashed { signal: 11 }`, no revision, no work dir, 0 mounts. The log expanded and took focus. | `p5-1440-failure-log.png` |
| G-code facts dialog on `orca-cube.gcode` | Pass. It started empty (4 × "Not provided"). "Use the file's value" filled the nozzle (0.4). The revision was created with `requiresManualPrinterSelection = 1`, only `operatorConfirmed`/`absent` facts, and `gcode_sha256` equal to the source's `content_sha256` (reused, not copied). | `p5-1440-facts-dialog.png`, `p5-1440-external-review.png` |
| Hard restart (SIGKILL of the app, idle) | Pass. Every `slice_revisions` and `slice_revision_blobs` row, and every blob's SHA-256, were byte-identical (`cmp` of before and after dumps). No operation was running. | – |
| Crash mid-slice: SIGKILL the app while the engine (whose parent was the app) was slicing | Pass. The engine exited within 0.2 s of the SIGKILL, with 0 stale mounts (PDEATHSIG). After relaunch: operation `interrupted`, work dir removed, revisions byte-identical. | `p5-1440-interrupted-failed-cancelled.png` |
| 1024 × 700 | Pass. The panel folds behind **Settings panel** and opens as an overlay. The object list folds under the viewport behind **Show objects**. Model details sit behind **Details**. | `p5-1024-panel-overlay.png`, `p5-1024-objects.png` |
| Reduced motion (`gtk-enable-animations=false`, which WebKitGTK maps to `prefers-reduced-motion`) | Pass for the camera. With reduced motion, the viewport 34 ms after pressing `1` or `5` is pixel-identical to the final pose. With animations on, the 35/93/162 ms frames differ (250 ms tween). The progress-bar CSS (`Progress.module.css`, `@media (prefers-reduced-motion)`) was not measured natively. | `p5-reduced-motion.png` |

## Packaged app (Linux x86_64)

The package was not installed system-wide (that needs root).

**1. The .deb binary.** The fresh `.deb` was extracted to a scratch
directory. Its `usr/bin/farm3d` was run under `env -i`, with a fresh scratch
`HOME` and the display `:9`. The app was driven the same way:

1. Add Printer (Elegoo Centauri Carbon, Profile-only).
2. Import `orca-two-plates.3mf` (managed).
3. Prepare…, then Slice all plates.

Result: two `succeeded` operations and two revisions. The plate keys are
distinct (`1c3ee936…`, `6d04affe…`), with plate indexes 1 and 2. Both share
one source revision (`msr-460f8803…`). The runtime is `{"engineVersion":
"2.4.2","engineChannel":"release","presetSourceVersion":"2.4.2",…}`.

Screenshot: `p5-packaged-deb-sliced.png`.

The two G-code blobs are identical, and that is expected. The fixture's two
plates each hold the same cube at the same place, so the two plate 3MFs are
byte-identical (`e12a4f22…`). They were sliced in the same second, and the
header timestamp is the only line that varies.

**2. farm3d's AppImage.** Spec D24 and AC15 allow this when root isn't
available. `farm3d_0.1.0_amd64.AppImage` was run the same way, with its own
scratch `HOME`. It also gave two `succeeded` operations and two revisions
(`slr-72366361…` plate 1, `slr-cbd6f9e3…` plate 2), from one source revision,
with engine 2.4.2. Its FUSE mount was released on exit.

**Not done: slicing through an *installed* .deb or Arch package.** It needs
root, so it remains a user action:

```sh
just package          # already run; produces the .deb
just package-arch     # repackages the .deb into packaging/arch/*.pkg.tar.zst
sudo pacman -U packaging/arch/farm3d-bin-0.1.0-1-x86_64.pkg.tar.zst
farm3d                # then: Add Printer (Elegoo Centauri Carbon), import
                      # src-tauri/tests/fixtures/library/orca-two-plates.3mf,
                      # Prepare…, Slice all plates; expect two revisions
                      # with engine 2.4.2.
```

## Acceptance criteria: issue #15

| # | Criterion | Status | Evidence |
|---|---|---|---|
| 1 | The runtime spike is approved, and the deterministic invocation fixtures pass | **met** | The spike is approved (`2026-09-24-p5-orca-runtime-spike.md`, "Approval"). `the_deterministic_invocation_fixtures_match_the_committed_ones` passes. `just gen-slicing-fixtures` then `git diff --exit-code` exits 0. |
| 2 | The cancellation/restart, stale-source, two-plate identity, external provenance/missing-fact, and immutable-artifact tests pass | **met** | All pass in `just test-rust` (names under spec AC6, 7, 9, 10, and 11 below). One sub-item has no automated test (PDEATHSIG, spec AC7); it is verified by hand. |
| 3 | Accessible viewport and keyboard verification pass | **partially met** | The whole flow was done by keyboard in the native app, and reduced motion was checked. But **D1** breaks linear Tab order in the preparation panel, and the pass ran on a nested Xwayland, not `:0`. |
| 4 | A packaged or installed OrcaSlicer executes on Linux x86_64 | **met** | The packaged farm3d (the .deb's binary, and farm3d's AppImage) sliced the two-plate fixture with the installed v2.4.2 AppImage (see above). An installed-package run remains a user action. |
| 5 | The tracer completes with all revisions unchanged after restart | **met** | `tracer_runs_against_fake_orca` (CI) and `real_orca_tracer_runs_against_a_real_orcaslicer` (31.0 s) both pass. The native manual restarts also showed byte-identical revisions. |

## Acceptance criteria: spec

| # | Criterion | Status | Evidence |
|---|---|---|---|
| 1 | Migration | **met** | `p5_migration`: `fresh_database_records_the_v6_ledger_row_with_a_matching_checksum`, `upgrading_v5_to_v6_keeps_every_existing_row_and_survives_a_restart`, `a_crash_before_commit_leaves_the_database_unchanged_at_v5`, `revisions_and_their_blobs_reject_updates_but_allow_a_guarded_delete`, and `revision_checks_enforce_plate_and_runtime_exclusivity`. |
| 2 | Runtime | **met** | `slicing::runtime` tests: `version_lines_parse_as_d2_specifies`, `garbage_output_is_a_probe_failure`, `a_release_ranks_above_a_prerelease_of_the_same_version`, `the_probe_runs_in_a_scratch_directory_with_the_allowlisted_environment`, `appimage_profiles_are_extracted_once_and_keep_only_json`, `a_cache_only_appimage_is_unreadable`, and `an_engine_without_readable_presets_is_presets_unreadable`. Also `real_orca_engine_probes_as_a_supported_version`. |
| 3 | Presets | **partially met** | Met: `flattening_merges_the_chain_and_keeps_system_identity`, the committed `flat-presets.json` fixture, the offered and compatibility tests, `a_missing_preset_is_preset_not_found`, and `real_orca_preset_source_knows_every_mapped_key`. Not met: no real-Orca test shows that **writing each mapped key changes the G-code header**. The real test checks that the keys are known and that overrides reach the preset JSON; it doesn't slice and compare headers. |
| 4 | Mapping | **met** | `every_profile_field_is_mapped_or_not_applicable`, `an_unmapped_override_blocks_slicing`, `an_unknown_override_key_blocks_slicing_for_a_printer`, and `every_mapped_key_is_known_to_the_fixture_preset_source`. |
| 5 | Deterministic invocation fixtures | **met** | `the_deterministic_invocation_fixtures_match_the_committed_ones` (plate 3MF, argument vectors, flat presets). Regenerating gives no diff. |
| 6 | Two-plate identity | **met** | `a_two_plate_preparation_slices_into_two_revisions_of_one_source` (fake-orca), `real_orca_places_a_written_plate_exactly`, and both tracers. Native: two revisions with distinct plate keys and one source, in dev, the packaged .deb binary, and the AppImage. |
| 7 | Cancellation and restart | **partially met** | Met: `cancel_mid_run_stops_the_grandchild_within_six_seconds`, `cancelling_a_running_slice_stops_it_and_a_queued_one_never_starts`, `real::real_orca_cancel_stops_the_group_quickly`, `a_restart_mid_slice_interrupts_it_and_keeps_earlier_revisions`, and the tracer's step 6. Native: cancel in 0.26 s, and an interrupted slice after a crash. Gap: **no automated PDEATHSIG test exists** in the repo (none in `tests/p5_process.rs` or `process_group.rs`). PDEATHSIG is shown only by spike Gate E and by today's manual run (the engine exited within 0.2 s of the app's SIGKILL). |
| 8 | Failure | **met** | `every_gate_f_return_code_maps_to_its_d11_failure`, `missing_and_unreadable_inputs_fail_like_orca`, `an_unexpected_signal_is_an_engine_crash`, `an_engine_that_cannot_start_is_spawn_failed_without_its_path`, `a_hung_slice_times_out_through_the_stop_escalation`, `a_slice_that_cannot_be_stored_fails_with_its_log`, `a_worker_panic_fails_the_slice_with_internal_error_and_the_queue_continues`, `success_without_gcode_is_output_missing`, and the redaction tests. Native: `engineCrashed` with the real engine. |
| 9 | Stale source | **met (automated only)** | `a_stale_source_is_refused_then_continued_and_reload_keeps_transforms`. Not exercised by hand. |
| 10 | Immutable artifacts | **met** | `revisions_are_immutable_and_deleting_a_model_cascades_everything`, `revisions_and_their_blobs_reject_updates_but_allow_a_guarded_delete`, `a_model_with_slice_revisions_is_blocked_by_the_registered_source`, `a_model_with_an_external_revision_blocks_delete_model`, and the tracer's `open_verified` hashes. Native: byte-identical across two restarts (an idle SIGKILL, and a SIGKILL mid-slice). |
| 11 | External provenance and missing facts | **met** | `external_facts_never_pick_up_a_files_own_claims_for_every_gcode_fixture`, `an_external_revision_takes_only_confirmed_or_absent_facts_and_reuses_the_source_blob`, `claimed_estimates_parse_from_the_orca_cube_claims_but_are_never_trusted`, and the `GcodeFactsDialog` tests. Native: the dialog started empty; the facts, the manual-selection flag, and blob reuse were checked in the DB. |
| 12 | Events | **met** | `slicing::events` tests, `events_are_ordered_and_the_backfill_covers_what_came_before`, "ignores every event on the shared channel that is not slicing.*" (`slicing-store.test.ts`), and "4. ignores every event on the shared channel that is not library.*" (`library-store.test.ts`). |
| 13 | Frontend tests | **met** | 1142 tests pass. Store: `slicing-store.test.ts` (84). Pure modules: transforms (13), layflat (5), arrange (13), bounds (16), validation (7), fact-parsing (8). `PlateViewport.test.tsx` (20). `PreparationWorkspace.test.tsx` (29). `PreparationPanel.test.tsx` (41; it also covers the operation panel's progress, cancel, and log expand-on-failure; there is no separate `SliceOperationPanel.test.tsx`). `SliceRevisionReview.test.tsx` (13). `GcodeFactsDialog.test.tsx` (14). `SlicerSettingsDialog.test.tsx` (25). |
| 14 | Accessible viewport and keyboard verification | **partially met** | Native, keyboard-only, at 1440 × 900 and 1024 × 700, with reduced motion checked; screenshots `docs/screenshots/p5-*.png`. Reasons for partial: **D1** (the panel's Tab order is broken by the matching-Printers popover), and the keyboard ran through a nested Xwayland (see "How the native app was driven"). |
| 15 | Packaged OrcaSlicer execution | **met, via the AppImage/unpacked path** | See "Packaged app". The installed-`.deb` variant needs root and is left as a user action with the commands above. |
| 16 | The tracer | **met** | `tracer_runs_against_fake_orca` and `real_orca_tracer_runs_against_a_real_orcaslicer`. |

## Defects found

### D1: The matching-Printers popover splits the preparation panel's Tab order (keyboard, moderate)

The Target section's "N matching Printers" chip reuses `PrinterRoster`
(`src/design-system/components/PrinterRoster.tsx`). The chip opens its
popover **on focus**, and focus then moves into the popover. The popover is
portalled to the end of `<body>`, so:

- Tab from **Slice for** enters the popover, and the next Tab wraps to the
  app header's roster chips.
- Shift+Tab from **Filament preset** enters the popover, and the next
  Shift+Tab wraps to the end of the page.

So a keyboard user moving linearly can't reach Material, Quality, the
controls, or **Slice** from the Target select, or go back the other way.
Pressing **Escape** at the popover and then Tab again does cross it, but
nothing on screen says so.

The header's roster chips (P2) behave the same way: each needs Escape, then
Tab.

- **Steps:** `just dev`, with 1 Printer. Library, then a 3MF, then
  Prepare…. Tab to **Slice for**. Press Tab: focus goes to a `dialog` (the
  popover). Press Tab again: focus goes to the header's "N Printers" chip.
- **Expected:** Tab from **Slice for** reaches the chip, then Filament
  preset.
- **Actual:** focus wraps out of the panel.

### D2: With no Printers, Prepare… is a dead end (feature completion, moderate)

With an empty Farm, **Prepare…** shows "Add a Printer, or choose a printer
profile, to prepare for." and only a **Back to Library** button. There is no
way to choose a printer profile from there.

- **Cause:** `PreparationMode` calls `create_preparation` without a target.
  `slicing/preparation.rs::default_target` then returns `VALIDATION` when
  there is no active Printer.
- The panel's **Other printer profile…** path is unit-tested "with no
  Printers at all" (`PreparationPanel.test.tsx:200`). In the real app,
  though, that panel is never reached without a Printer.
- **Steps:** a fresh profile with no Printers. Import any STL or 3MF, then
  Prepare….
- **Expected:** the message's "choose a printer profile" is offered. For
  example, the workspace opens on a catalog profile, or a profile picker is
  shown.
- **Actual:** the user must first go to Monitor and add a Printer.

### Other observations (not blocking)

- **Heap abort at one exit.** Once, the dev binary printed `free():
  corrupted unsorted chunks` on SIGTERM. That was 1 of 5 exits; AT-SPI
  inspection was active and WebKit was tearing down. Two more SIGTERMs
  exited cleanly, so it wasn't reproduced. It is unconfirmed and could be in
  WebKitGTK or ATK rather than farm3d.
- **Empty logs from v2.4.2.** Real v2.4.2 writes **nothing** to stdout or
  stderr on a successful slice. Re-running the stored inputs by hand gave 0
  and 0 bytes with rc 0. So every successful revision's `log` blob is empty
  (`e3b0c442…`), and **Show log** says "This slice left no log." The same
  was true for the SIGSEGV failure. The log feature only shows content when
  the engine prints something.
- **The 1024 × 700 overlay.** Opening **Settings panel** leaves focus on the
  toggle, and the next Tab goes to the plate tabs, not into the overlay.
  Escape closes it only when focus is inside it. While open, it covers the
  right half of the object fields.
- **Focus after the import dialog.** After **Done** in the import dialog
  (P4), focus returns to the start of the document, not to **Import…**.
- **Lay flat on a sphere.** The UV sphere's lay-flat list shows eight faces
  of "0 mm²".
- **The two-plate fixture.** Both plates hold identical content, so the
  fixture can't show different per-plate G-code on its own. Placement
  extents are covered separately by `real_orca_places_a_written_plate_exactly`.

## Known limitations and deferred items

**From the Task 16a report:**

- **Recovery can miss a real AppImage engine** during its `/bin/sh`
  `orca-slicer-env` wrapper phase. PDEATHSIG is the primary guard, and the
  gap is documented in D10 and ADR-0009.
- **When the slicing worker panics after its engine ran,** the run's log is
  lost (`fail_operation(…, None, …)`).
- **`SchedulerPoint::Exited`** is a `#[doc(hidden)]` test seam in
  production code.
- **Spec D20's wording** differs from the Task 12 ledger ruling.

**Platform scope.** Windows and macOS compile and are unit-tested only.
They make no runtime claim (D24), and CI stays on fake-orca.

**Deferred minors in the SDD ledger** (`progress.md`, "minor (deferred)"):

- **Task 3:**
  - One full-suite `cargo test` failure could not be reproduced. It didn't
    recur this run.
  - `insert_preparation` ignores the source format. D5's G-code rejection is
    enforced in `create_preparation`.
  - `library::model_content_hashes` reads slicing tables directly.
  - `slicing/repository.rs` repeats prepare/query_map/collect blocks, and is
    about 2,000 lines.
  - The test `…allow_a_guarded_delete` should be renamed.
- **Task 4:**
  - Float noise in the layer-height error text and in the bed-shape
    `printable_area`.
  - `PresetIndex::find` rescans vendors on every lookup.
  - A weak scratch-dir assertion; a presets module doc that overstates
    laziness; `storage_error` is duplicated.
  - 8 ts-rs "failed to parse serde attribute" warnings, still printed in
    this run.
  - Non-Unix `wait_or_stop` waits the full grace period, and the Printer
    branch calls `resolve_catalog_ref` twice.
  - Clippy `result_large_err` warnings on `CommandError`.
- **Task 5:**
  - Concurrent geometry loads are not deduplicated.
  - The STL weld keeps −0.0 and 0.0 as distinct vertices.
  - The golden plate bytes depend on the zip crate version.
- **Task 6:** `p4_links::a_failed_watch_falls_back_to_polling_with_a_warning`
  flaked once under full-suite load. It didn't recur this run.
- **Task 7:** an in-place unretract extends the printed bounds.
- **Task 8:**
  - `run_job` treats a storage read error as "not queued".
  - `abandon_deleted_jobs` has a redundant check.
  - A cancel of an unknown id waits behind an in-flight `start_slice`.
- **Task 11:**
  - The loaded inspector is taller than its placeholder.
  - Flat shading.
  - The theme preview doesn't recolour the viewport.
- **Task 12:**
  - A burst of rotate or scale on a 1M-vertex mesh blocks for about 170 ms.
  - The disposed-guard style differs between files.
- **Task 13:** the PrinterBatchDialog `Suspense` has no fallback.

## Checks that were unavailable or not done

- **Keyboard on `:0`.** Synthetic keyboard and pointer input on the real
  display `:0` did not work: KWin's rootless Xwayland ignored XTest. The app
  launched and rendered there, but the keyboard pass ran on a nested rootful
  Xwayland on the same desktop. There is no ydotool, xdotool, wtype, or
  tauri-driver on this machine.
- **Installed package.** Slicing through an **installed** `.deb` or Arch
  package needs root. It is a user action (commands above).
- **Progress-bar reduced motion.** Its reduced-motion behaviour was not
  measured natively; only its CSS rule was read.
- **Stale source by hand.** Stale-source reload (AC9) was not exercised
  manually; the automated test covers it.

## Cleanup

- Every `just dev`, packaged, and AppImage instance was stopped, and so were
  the nested Xwayland `:9` and the AT-SPI helper.
- No `orca-slicer` processes remain, and no `/tmp/.mount_*` FUSE mounts
  remain. None needed `fusermount -u`.
- The scratch data is under the job's `tmp/`, not the user's farm3d
  profile.
