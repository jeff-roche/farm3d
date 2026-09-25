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

**Update, Task 16c (after this pass):** D1 and D2 are fixed, along with the
1024 × 700 overlay focus and the lay-flat "0 mm²" observations. The
evidence is automated tests plus a headless-Chrome run of `just web`
(details under each defect). The native re-check was **not** redone, so the
native evidence in this record still describes the app before the fixes.

**Update, final fix round (after the whole-branch review):** spec AC3 and
AC7 are now fully met by automated tests. AC3 has a real-Orca test that
slices once per mapped control and checks the G-code header, and AC7 has a
PDEATHSIG test. A race in the SIGTERM-grace test is fixed, and five minor
review findings are fixed. The details are in
[Final fix round](#final-fix-round) below.

**Update, cleanup round (after the final fix round):** the known issues
left over are fixed. Unpublished operation logs are now pruned, a worker
panic after the engine ran keeps the run's log, the process-test guards
no longer risk signalling a reused pid, and the 7 Printer Profile override
keys have a real-Orca header test. The details are in
[Cleanup round](#cleanup-round) below.

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
| 2 | The cancellation/restart, stale-source, two-plate identity, external provenance/missing-fact, and immutable-artifact tests pass | **met** | All pass in `just test-rust` (names under spec AC6, 7, 9, 10, and 11 below). PDEATHSIG (spec AC7) was verified by hand in this pass, and has an automated test since the final fix round. |
| 3 | Accessible viewport and keyboard verification pass | **partially met** | The whole flow was done by keyboard in the native app, and reduced motion was checked. **D1** broke linear Tab order in the preparation panel; it is fixed in Task 16c, with automated and `just web` evidence, but the native keyboard pass was not redone. The pass also ran on a nested Xwayland, not `:0`. |
| 4 | A packaged or installed OrcaSlicer executes on Linux x86_64 | **met** | The packaged farm3d (the .deb's binary, and farm3d's AppImage) sliced the two-plate fixture with the installed v2.4.2 AppImage (see above). An installed-package run remains a user action. |
| 5 | The tracer completes with all revisions unchanged after restart | **met** | `tracer_runs_against_fake_orca` (CI) and `real_orca_tracer_runs_against_a_real_orcaslicer` (31.0 s) both pass. The native manual restarts also showed byte-identical revisions. |

## Acceptance criteria: spec

| # | Criterion | Status | Evidence |
|---|---|---|---|
| 1 | Migration | **met** | `p5_migration`: `fresh_database_records_the_v6_ledger_row_with_a_matching_checksum`, `upgrading_v5_to_v6_keeps_every_existing_row_and_survives_a_restart`, `a_crash_before_commit_leaves_the_database_unchanged_at_v5`, `revisions_and_their_blobs_reject_updates_but_allow_a_guarded_delete`, and `revision_checks_enforce_plate_and_runtime_exclusivity`. |
| 2 | Runtime | **met** | `slicing::runtime` tests: `version_lines_parse_as_d2_specifies`, `garbage_output_is_a_probe_failure`, `a_release_ranks_above_a_prerelease_of_the_same_version`, `the_probe_runs_in_a_scratch_directory_with_the_allowlisted_environment`, `appimage_profiles_are_extracted_once_and_keep_only_json`, `a_cache_only_appimage_is_unreadable`, and `an_engine_without_readable_presets_is_presets_unreadable`. Also `real_orca_engine_probes_as_a_supported_version`. |
| 3 | Presets | **met** (final fix round) | `flattening_merges_the_chain_and_keeps_system_identity`, the committed `flat-presets.json` fixture, the offered and compatibility tests, `a_missing_preset_is_preset_not_found`, and `real_orca_preset_source_knows_every_mapped_key`. Writing each mapped control changes the G-code header: `real_orca_each_mapped_control_changes_its_gcode_header_claim` passes on v2.4.2 for all 11 controls (12 keys; results under [Final fix round](#final-fix-round)). This pass had found no such test. Each of the 7 Printer Profile override keys changes its header too: `real_orca_each_profile_override_changes_its_gcode_header_claim` passes on v2.4.2 (cleanup round, `05e4bb8`; results under [Cleanup round](#cleanup-round)). |
| 4 | Mapping | **met** | `every_profile_field_is_mapped_or_not_applicable`, `an_unmapped_override_blocks_slicing`, `an_unknown_override_key_blocks_slicing_for_a_printer`, and `every_mapped_key_is_known_to_the_fixture_preset_source`. |
| 5 | Deterministic invocation fixtures | **met** | `the_deterministic_invocation_fixtures_match_the_committed_ones` (plate 3MF, argument vectors, flat presets). Regenerating gives no diff. |
| 6 | Two-plate identity | **met** | `a_two_plate_preparation_slices_into_two_revisions_of_one_source` (fake-orca), `real_orca_places_a_written_plate_exactly`, and both tracers. Native: two revisions with distinct plate keys and one source, in dev, the packaged .deb binary, and the AppImage. |
| 7 | Cancellation and restart | **met** (final fix round) | `cancel_mid_run_stops_the_grandchild_within_six_seconds`, `cancelling_a_running_slice_stops_it_and_a_queued_one_never_starts`, `real::real_orca_cancel_stops_the_group_quickly`, `a_restart_mid_slice_interrupts_it_and_keeps_earlier_revisions`, and the tracer's step 6. Native: cancel in 0.26 s, and an interrupted slice after a crash. PDEATHSIG: `the_engine_exits_when_its_parent_is_killed` and `the_engine_exits_when_the_thread_that_spawned_it_exits` (`tests/p5_process.rs`, final fix round). This pass had found no automated PDEATHSIG test. The manual run also showed it: the engine exited within 0.2 s of the app's SIGKILL. |
| 8 | Failure | **met** | `every_gate_f_return_code_maps_to_its_d11_failure`, `missing_and_unreadable_inputs_fail_like_orca`, `an_unexpected_signal_is_an_engine_crash`, `an_engine_that_cannot_start_is_spawn_failed_without_its_path`, `a_hung_slice_times_out_through_the_stop_escalation`, `a_slice_that_cannot_be_stored_fails_with_its_log`, `a_worker_panic_fails_the_slice_with_internal_error_and_the_queue_continues`, `a_worker_panic_after_the_engine_ran_keeps_its_log` (cleanup round), `success_without_gcode_is_output_missing`, and the redaction tests. Native: `engineCrashed` with the real engine. |
| 9 | Stale source | **met (automated only)** | `a_stale_source_is_refused_then_continued_and_reload_keeps_transforms`. Not exercised by hand. |
| 10 | Immutable artifacts | **met** | `revisions_are_immutable_and_deleting_a_model_cascades_everything`, `revisions_and_their_blobs_reject_updates_but_allow_a_guarded_delete`, `a_model_with_slice_revisions_is_blocked_by_the_registered_source`, `a_model_with_an_external_revision_blocks_delete_model`, and the tracer's `open_verified` hashes. Native: byte-identical across two restarts (an idle SIGKILL, and a SIGKILL mid-slice). |
| 11 | External provenance and missing facts | **met** | `external_facts_never_pick_up_a_files_own_claims_for_every_gcode_fixture`, `an_external_revision_takes_only_confirmed_or_absent_facts_and_reuses_the_source_blob`, `claimed_estimates_parse_from_the_orca_cube_claims_but_are_never_trusted`, and the `GcodeFactsDialog` tests. Native: the dialog started empty; the facts, the manual-selection flag, and blob reuse were checked in the DB. |
| 12 | Events | **met** | `slicing::events` tests, `events_are_ordered_and_the_backfill_covers_what_came_before`, "ignores every event on the shared channel that is not slicing.*" (`slicing-store.test.ts`), and "4. ignores every event on the shared channel that is not library.*" (`library-store.test.ts`). |
| 13 | Frontend tests | **met** | 1142 tests pass (1154 after Task 16c, 1156 after the final fix round, 1160 after the cleanup round). Per-file counts, recounted from the vitest JSON reporter after the cleanup round: store: `slicing-store.test.ts` (44). Pure modules: transforms (27), layflat (10), arrange (13), bounds (16), validation (7), fact-parsing (8). `PlateViewport.test.tsx` (20). `PreparationWorkspace.test.tsx` (29). `PreparationPanel.test.tsx` (45; it also covers the operation panel's progress, cancel, log expand-on-failure, and empty-log note; there is no separate `SliceOperationPanel.test.tsx`). `SliceRevisionReview.test.tsx` (13). `GcodeFactsDialog.test.tsx` (13). `SlicerSettingsDialog.test.tsx` (25). |
| 14 | Accessible viewport and keyboard verification | **partially met** | Native, keyboard-only, at 1440 × 900 and 1024 × 700, with reduced motion checked; screenshots `docs/screenshots/p5-*.png`. Reasons for partial: the keyboard ran through a nested Xwayland (see "How the native app was driven"), and the native pass predates the Task 16c fixes. **D1** (the matching-Printers popover broke the panel's Tab order) is fixed in Task 16c, with automated and `just web` evidence only; the native re-check was not redone. |
| 15 | Packaged OrcaSlicer execution | **met, via the AppImage/unpacked path** | See "Packaged app". The installed-`.deb` variant needs root and is left as a user action with the commands above. |
| 16 | The tracer | **met** | `tracer_runs_against_fake_orca` and `real_orca_tracer_runs_against_a_real_orcaslicer`. |

## Final fix round

The fixes from the whole-branch review, on top of `fdc8cc6`:

| Commit | Change |
|---|---|
| `777ce90` | Fixes the race in the SIGTERM-grace test |
| `6162579` | Adds the AC7 PDEATHSIG tests |
| `9e5e53e`, `2bebd7a` | Adds the AC3 real-Orca header test |
| `058d345` | Makes the preset field paths consistent |
| `bb9b535` | Refuses a filament preset without its type or diameter |
| `55873a7` | Corrects the `finish_run` doc comment |
| `3206384` | Unlinks a deleted revision from its operation |
| `d71cccc` | Updates the README for P5 |

### AC7: PDEATHSIG tests

Both tests are in `src-tauri/tests/p5_process.rs` and are Linux-only. Each
starts a `hang` fake-orca through the production
`slicing::process_group::spawn_group`. Each polls `/proc`, counts a zombie
as gone, and has no fixed sleep.

- **`the_engine_exits_when_its_parent_is_killed`.** The test re-executes
  its own binary as an intermediate parent (the ignored helper
  `pdeathsig_intermediate_parent`, which does nothing without its
  environment variable). The intermediate spawns the engine and writes its
  pid (write, then rename). It then parks the spawning thread. The test
  checks that the engine's parent is the intermediate, SIGKILLs the
  intermediate, and requires the engine to be gone within 1 s.
- **`the_engine_exits_when_the_thread_that_spawned_it_exits`.** This test
  pins the rule in `operations.rs`. PDEATHSIG follows the thread that
  spawned the engine, not the process. When the spawning thread exits, the
  engine is gone within 1 s, and its exit status is SIGTERM (15), although
  farm3d is still running. That is why the scheduler spawns from its own
  long-lived thread.
- **Red check.** With the `prctl` changed to `set_parent_process_death_signal(None)`,
  both tests fail with "the engine outlived … by 1 s". No fake-orca was
  left behind, because a guard SIGKILLs it on drop.

### AC3: real-Orca header test

`real_orca_each_mapped_control_changes_its_gcode_header_claim` is in
`tests/p5_slicing.rs`. It is `#[ignore]`d, runs under `just test-orca`, and
panics when `FARM3D_ORCA` is unset. It uses `Farm::with_real_orca`, so the
engine is the real OrcaSlicer and the preset source is the TestVendor
fixtures.

It first slices a cube with no controls. It then slices once for each of
D4's 11 controls, each with a value the preset doesn't have. For each key
the control maps to, it requires that the `CONFIG_BLOCK` claim equals the
value written and differs from the baseline. It also requires a case for
every entry in `CONTROL_MAPPINGS`.

On v2.4.2 (the AppImage above), every key changed as expected:

| Control | Value | Key | Baseline → header |
|---|---|---|---|
| `layerHeightMm` | 0.12 | `layer_height` | 0.2 → 0.12 |
| `wallLoops` | 5 | `wall_loops` | 3 → 5 |
| `topShellLayers` | 6 | `top_shell_layers` | 4 → 6 |
| `bottomShellLayers` | 5 | `bottom_shell_layers` | 3 → 5 |
| `infillDensityPercent` | 35 | `sparse_infill_density` | 15% → 35% |
| `infillPattern` | gyroid | `sparse_infill_pattern` | grid → gyroid |
| `supports` | tree(auto) | `enable_support` | 0 → 1 |
| `supports` | tree(auto) | `support_type` | normal(auto) → tree(auto) |
| `supportThresholdAngleDeg` | 45 | `support_threshold_angle` | 30 → 45 |
| `brimType` | outer_only | `brim_type` | auto_brim → outer_only |
| `brimWidthMm` | 3 | `brim_width` | 5 → 3 |
| `skirtLoops` | 2 | `skirt_loops` | 0 → 2 |

The test takes about 8 s for 12 slices. The Printer Profile override keys
(`printable_area`, `nozzle_diameter`, and the others) have their own test,
added in the cleanup round; see [Cleanup round](#cleanup-round).

### The SIGTERM-grace race

`a_process_that_ignores_sigterm_is_killed_after_the_grace` cancelled 300 ms
after the spawn, and assumed that fake-orca's shell had set its
`trap '' TERM` by then. Under load, the cancel could come first. SIGTERM
would then stop the engine, and `run.killed` would be false.

- **The fix.** The `hangIgnoringTerm` shell now runs `trap '' TERM; : >
  term-ignored; …`. The test cancels only once `<work>/term-ignored`
  exists, and then asserts that the file does exist.
- **Other fixed sleeps.** Every `p5_*` test was checked. The only other
  fixed sleep used as a readiness assumption was a 100 ms pause in
  `cancel_mid_run_stops_the_grandchild_within_six_seconds`, after the
  grandchild's pid file appeared. The pid file is already the signal, so
  the pause was removed. The other sleeps are poll intervals inside
  deadline loops.
- **Stability.** The built `p5_process` binary was run 20 times, 4 at a
  time, and then 24 times, 8 at a time. All 44 runs passed, 17 of 17 tests
  each time. No fake-orca was left afterwards. `CARGO_PKG_NAME` was set as
  cargo sets it, because `the_child_gets_only_the_allowlisted_environment`
  requires it.

### Minor fixes

- **A stale revision link.** When a revision is deleted, the store's
  `dropRevision` now clears `sliceRevisionId` on any held operation that
  made it, as the backend's `ON DELETE SET NULL` does. It does this for the
  `slicing.revision.removed` event and for the delete result. The operation
  panel shows "Its Slice Revision was deleted." for a succeeded operation
  with no revision, in place of "Saved as a Slice Revision" and a button
  that led to NOT_FOUND. Tests: "a removed revision is unlinked from the
  operation that made it, whether the event or the delete result says so"
  (`slicing-store.test.ts`) and "says so when a succeeded operation's
  Slice Revision was deleted" (`PreparationPanel.test.tsx`).
- **Field paths.** The backend now emits `processPreset` and
  `filamentPreset`, not `document.…`, as the other preset errors do, so
  `fieldAt` links them to Quality and Material. Tests:
  `a_slice_without_a_chosen_preset_names_the_preset_field` (`p5_slicing`)
  and `sliceErrorView` in `slice-presentation.test.ts`.
- **Filament facts.** A resolved filament preset without `filament_type`
  or a usable `filament_diameter` now refuses the slice with
  `PRESET_INVALID` (kind `filament`, reason "it has no …"), before anything
  is queued. Before, a missing type was recorded as OTHER, and a missing
  diameter as 1.75. Tests:
  `a_filament_preset_without_its_type_or_diameter_is_preset_invalid` (unit)
  and `a_filament_preset_without_a_diameter_refuses_the_slice` (IPC).
- **A doc comment.** `publish.rs`'s `finish_run` now says that a
  content-store failure fails the operation at once with `storageFailed`.
- **README.** It now covers the user-installed OrcaSlicer 2.x, where
  farm3d looks for it, and **Settings** → **Slicer...**. It says a nightly
  also needs a 2.4 preset source. It adds `just test-orca` and `just
  gen-slicing-fixtures` to the command table, notes that `just test-rust`
  builds with `--features test-support`, and says `just package` also
  rejects `fake-orca`.

### Gates (final fix round)

Run at `2bebd7a`, with `source "$HOME/.cargo/env"`.

| Command | Exit | Result |
|---|---|---|
| `just build` | 0 | `tsc` and the Vite build pass. |
| `just test` | 0 | 89 files, 1156 tests passed. |
| `cd src-tauri && cargo test --features test-support` | 0 | 965 passed, 0 failed, 15 ignored, across 39 test binaries. |
| `cargo clippy --all-targets --features test-support -- -A clippy::result_large_err` | 0 | Only warnings that were there before this round; none in the files it changed. |
| `cargo fmt --check` | 0 | Clean. |
| `just gen-contracts`, then `git diff --exit-code src/generated` | 0 | No diff. |
| `FARM3D_ORCA=~/Downloads/OrcaSlicer_Linux_AppImage_Ubuntu2404_V2.4.2.AppImage just test-orca` | 0 | All 9 `real_orca*` tests pass, including the new AC3 test. No `/tmp/.mount_*` or `orca-slicer` process was left. |

The native app was not re-checked in this round. The fixes change no
layout, except that a succeeded operation can now say its revision was
deleted.

### Follow-ups

- **Operation-log retention.** Resolved in the cleanup round (`368a518`):
  each Preparation keeps the logs of only its 5 most recent failed or
  cancelled operations. See [Cleanup round](#cleanup-round).
- **Focus after the P4 import dialog.** Resolved (`70e1724`): the
  design-system `Dialog`'s `returnFocus` prop is now threaded through
  `ImportDialog` to the **Import…** button, and through
  `CreateProjectDialog` to **New Project** (see "Other observations"
  below).

## Cleanup round

Fixes for the known issues left after the final fix round, on branch
`feature/p5-runtime-slicing`.

| Commit | What it does |
|---|---|
| `368a518` | Prunes unpublished operation logs past 5 per Preparation |
| `f5c4962` | Keeps the run's log when the worker panics after the engine ran |
| `41463e9` | Disarms the PDEATHSIG tests' process guards, and dedupes a wait loop |
| `05e4bb8` | Adds the real-Orca header test for the Printer Profile override keys |
| `32353ba` | Counts only operations that have a log toward the 5 |
| `cebd45b` | Makes the tie-break test real and tests a log hash two operations share |

### Operation-log retention

For each Preparation, only the 5 most recent failed or cancelled
operations that have a log (by end time, then id) keep it
(`KEPT_UNPUBLISHED_LOGS` in `slicing/repository.rs`). Older ones keep
their rows, with `log_sha256` set to NULL. Their blobs are marked through
`mark_unreferenced_blobs` and unlinked after commit by
`release_unreferenced`. The pruning runs in the same transaction as every
move to `failed` or `cancelled`, including `record_unpublished`. Startup
recovery runs it once over every Preparation, and startup then releases
the freed blobs. Succeeded, queued, running, and interrupted operations
are never touched, and neither are Slice Revision logs.

A failed or cancelled operation with no log (for example, one cancelled
before it spawned) doesn't count toward the 5, so it never evicts a real
log (`an_operation_without_a_log_never_evicts_one`). The schema
can't tell a pruned log from one that was never written, so the operation
panel's empty-log note now reads "No log is kept for this attempt." for
both.

Tests: `a_sixth_unpublished_log_drops_the_oldest_and_marks_its_blob`,
`a_tie_in_ending_time_prunes_the_lower_id_first`,
`an_operation_without_a_log_never_evicts_one`,
`a_log_a_kept_operation_shares_is_not_released`, and
`pruning_leaves_succeeded_active_and_revision_logs_alone`
(`slicing::repository`); `a_sixth_failure_prunes_the_oldest_log_and_releases_its_blob`
and `startup_recovery_prunes_a_database_seeded_with_more_than_five_logs`
(`slicing::publish`); and "says no log is kept for a finished attempt
whose log is empty" (`PreparationPanel.test.tsx`).

### A worker panic after the engine ran

`run_job` now catches a panic in the stage after the engine exits (the
`Exited` hook, the last progress event, and `finish_run`) and fails the
operation with `internalError` and the run's log. `run_worker`'s guard
still covers a panic before or during the run. The existing panic test now
injects at `Spawning`, where there is no log. A new test,
`a_worker_panic_after_the_engine_ran_keeps_its_log`, injects at `Exited`
and finds fake-orca's output in the failed operation's log. It fails
without the fix.

### Test hygiene

The PDEATHSIG tests disarm their `KillOnDrop` guard once `gone_within`
confirms the engine is gone, so a reaped and reused pid is never
signalled. The intermediate parent process has a `ReapOnDrop` guard, so a
failed assertion never leaves it running.
`real_orca_slices_a_cube_through_the_commands` now calls
`wait_for_real_success` instead of repeating its wait loop.

### AC3: the Printer Profile override keys

`real_orca_each_profile_override_changes_its_gcode_header_claim` is in
`tests/p5_process.rs`, beside the other real-Orca process tests. It uses
the engine's own Elegoo Centauri Carbon presets. It first slices the
golden plate with the machine preset as it is. It then writes each mapped
override through D4's `apply_profile_overrides` and slices again. It
requires that the `CONFIG_BLOCK` claim equals the value written and
differs from the baseline, and that every mapped field in
`PROFILE_FIELD_MAPPINGS` has a case.

It drives the mapping directly, not the commands. A Printer can override
only `bedShape`, `printableHeightMm`, `bedExcludeAreas`, and
`defaultBedType` (`PrinterProfileOverrides`), so the commands can never
write `nozzle_diameter`, `nozzle_type`, or `gcode_flavor`.

On v2.4.2, every key changed as expected, and every slice was valid:

| Field | Key | Baseline → header |
|---|---|---|
| `bedShape` | `printable_area` | 0x0,256x0,256x256,0x256 → 0x0,300x0,300x280,0x280 |
| `printableHeightMm` | `printable_height` | 256 → 200 |
| `bedExcludeAreas` | `bed_exclude_area` | 246x0,256x0,256x20,246x20 → 0x0,20x0,20x20,0x20 |
| `nozzleDiameterMm` | `nozzle_diameter` | 0.4 → 0.6 |
| `nozzleType` | `nozzle_type` | hardened_steel → stainless_steel |
| `gcodeFlavor` | `gcode_flavor` | klipper → marlin2 |
| `defaultBedType` | `default_bed_type` | 4 → Cool Plate |

The test takes about 12 s for 8 slices.

### Gates (cleanup round)

Final counts, run at `cebd45b` (after the import-dialog focus fix,
`70e1724`), with `source "$HOME/.cargo/env"`. `just test-orca` was last
run at `05e4bb8`. Later commits change only the retention query, unit
tests, the import dialog's focus, and docs.

| Command | Exit | Result |
|---|---|---|
| `just build` | 0 | `tsc` and the Vite build pass. |
| `just test` | 0 | 89 files, 1160 tests passed. |
| `cd src-tauri && cargo test --features test-support` | 0 | 973 passed, 0 failed, 16 ignored, across 39 test binaries. |
| `cargo clippy --all-targets --features test-support` | 0 | No new warnings in the files this round changed. |
| `cargo fmt --check` | 0 | Clean. |
| `just gen-contracts`, then `git status` | 0 | No diff. |
| `FARM3D_ORCA=~/Downloads/OrcaSlicer_Linux_AppImage_Ubuntu2404_V2.4.2.AppImage just test-orca` | 0 | All 10 `real_orca*` tests pass, including the new override test. No `/tmp/.mount_*` or `orca-slicer` process was left. |

The native app was not re-checked in this round. The only visible change
is the wording of the empty-log note.

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

**Fixed in Task 16c** (`f532e6b`). The fix is in the design-system
`PrinterRoster`, so it covers the header's chips too:

- Focusing a chip never opens it. A hover shows the roster without taking
  focus. A press (click, Enter or Space) opens it and moves focus in.
- Escape closes it and returns focus to the chip.
- Tab and Shift+Tab at the roster's edges close it and continue as if it
  sat right after its chip. Tab goes to whatever follows the chip, and
  Shift+Tab goes back to the chip.

Tests (`@testing-library/user-event` isn't installed, so a test can't
press a real Tab; they check that focusing a chip leaves focus on it, and
drive the roster's own Tab handling with key events):

- `components.test.tsx`, `PrinterRoster`: "keeps focus on the chip when
  it's focused, without opening"; "opens on press and returns focus to the
  chip after Escape"; "continues the Tab order after the chip, either way,
  from the open roster"; "moves on from a roster with nothing to press,
  when tabbed"; "opens on hover without taking focus"; and "stays open when
  the hovered chip is clicked".
- `PreparationPanel.test.tsx`: "keeps the Tab order through the matching
  Printers chip, both ways" (from the roster, Tab reaches **Filament
  preset** and Shift+Tab returns to the chip).
- `AppShell.test.tsx`: "keeps the header's Tab order through its roster
  chips".

In `just web`, headless Chrome sent real Tab keys over CDP:

- **Forward from Slice for:** "2 matching Printers", then Filament preset,
  then Process preset.
- **Shift+Tab back:** Filament preset, then the chip, then Slice for.
- **On the chip:** no dialog opened. Enter opened the roster with focus
  inside it. Tab from there went to Filament preset. Shift+Tab went back to
  the chip, and Escape also returned focus to the chip.
- **Header, forward:** 3 Printers, then 0 Ready, 0 Printing, 0 Offline, and
  0 Setup incomplete. Shift+Tab went back through them in reverse.

The screenshots are in
`.superpowers/sdd/2026-09-24-p5-runtime-slicing-slice-revisions/task-16c-screens/`
(`light-d1-*.png`). This run is not in git. The native re-check was not
redone.

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

**Fixed in Task 16c** (`9d2fd61`). The backend already accepted a target
at create time, so no Rust change was needed. When `create_preparation`
refuses with `VALIDATION` at `target`, the message now offers **Choose a
printer profile…**. It opens the catalog picker (`TargetProfileDialog`,
the same one as **Other printer profile…**), then creates the Preparation
for `{ kind: "profile", catalogRef }`. Web mode now refuses the same way
once its Farm has no active Printer.

Tests:

- `PreparationPanel.test.tsx`: "opens on a chosen printer profile when
  there are no Printers to prepare for". With no Printers, it chooses Prusa
  MK4 0.4, and the workspace opens with that target.
- `slicing-store.test.ts`: "a local create needs a target once the Farm
  has no active Printer, as the backend does".
- `p5_slicing.rs`: `with_no_printers_a_preparation_needs_a_profile_target`.
  With no Printers and no target, the error is `VALIDATION` with
  `fieldPath` `target`. The same request with a profile target succeeds.

In `just web`, headless Chrome was served a printer store with no Printers.
Prepare… on the Corner bracket showed the message and the new button. The
dialog chose Prusa and then MK4, and the workspace opened on "Prusa MK4 0.4
nozzle" (`dark-d2-*.png`). In web mode, the fixture's slice options still
report the Centauri presets and "2 matching Printers" for any target. That
comes from the web fixture, not from this fix. The native re-check was not
redone.

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
  right half of the object fields. *The focus part is fixed in Task 16c
  (`bfd1817`):* opening the overlay now focuses its first control, so
  Escape works straight away. The test is "moves focus into the overlay
  when Settings panel opens it" (`PreparationPanel.test.tsx`). In `just web`
  at 1024 × 700, focus landed on **Slice for** inside the overlay. The
  overlay still covers the object fields.
- **Focus after the import dialog.** After **Done** in the import dialog
  (P4), focus returns to the start of the document, not to **Import…**.
  *Fixed (`70e1724`):* `ImportDialog` and `CreateProjectDialog` now pass
  the design-system `Dialog`'s `returnFocus` prop, pointing back at
  LibraryWorkspace's **Import…** and **New Project** buttons. The tests
  are "returns focus to Import… after Done/Escape closes the import
  dialog" and "returns focus to New Project after Cancel closes its
  dialog" (`LibraryWorkspace.test.tsx`). Not re-checked natively.
- **Lay flat on a sphere.** The UV sphere's lay-flat list shows eight faces
  of "0 mm²". *Fixed in Task 16c (`cb834c8`):* areas below 10 mm² now keep
  two significant figures (for example "0.042 mm²"), and larger areas are
  still whole mm². The test is `formatFaceArea` in
  `slice-presentation.test.ts`. The sphere wasn't re-checked natively.
- **The two-plate fixture.** Both plates hold identical content, so the
  fixture can't show different per-plate G-code on its own. Placement
  extents are covered separately by `real_orca_places_a_written_plate_exactly`.

## Known limitations and deferred items

**From the Task 16a report:**

- **Recovery can miss a real AppImage engine** during its `/bin/sh`
  `orca-slicer-env` wrapper phase. PDEATHSIG is the primary guard, and the
  gap is documented in D10 and ADR-0009.
- **When the slicing worker panics after its engine ran,** the run's log
  used to be lost (`fail_operation(…, None, …)`). Resolved in the cleanup
  round (`f5c4962`): the operation fails with `internalError` and keeps the
  log.
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
