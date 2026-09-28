# P8 Attention Events, Incidents, cameras, and notifications verification

**Date:** 2026-09-28
**Platform:** Linux x86_64 (Linux 7.2.8-1-cachyos), KDE Plasma 6.7.5 on
Wayland, podman for the simulators.
**Validated source:** branch `feature/p8-attention-incidents-notifications`.
`just build`, `just test`, `just test-rust`, and `just gen-contracts`
ran on `1a0e13a` (Task 16's fix round, "docs(p8): record the simulator
run for the fix-round tracer"). The simulator suite, `just check-hosts`,
and `just package` ran on `16b5388` ("docs(spec): align the P8 spec with
the implementation"), which is `1a0e13a` plus documentation only (the
spec, ADR-0015, `CONTEXT.md`, `README.md`, and the v1 approach doc). The
sim-gate manifest's `repoCommit` is `16b5388`. This record's own commit
adds one more spec wording fix (decision 39's lag behavior) and no code.
**Covers:** Tasks 1–17 of the P8 plan
(`docs/superpowers/plans/2026-09-27-p8-attention-incidents-cameras-notifications.md`,
kept uncommitted by the owner's ruling), for GitHub issue #18. The
binding spec is
[`docs/superpowers/specs/2026-09-27-p8-attention-incidents-cameras-notifications-design.md`](../superpowers/specs/2026-09-27-p8-attention-incidents-cameras-notifications-design.md),
with ADR-0014 (Attention is a projection of Conditions) and ADR-0015
(desktop notifications over D-Bus).

**Overall:** every automated gate passes: `just build`, `just test`,
`just test-rust`, `just gen-contracts` (no diff), the full simulator suite
including the P8 live-host tracer, and `just package`. `just check-hosts`
checks nothing in this environment (no denylist configured); a manual
scan of this task's diff found no real network details. **The
installed-bundle pass (Task 17 Steps 2 and 4) is pending the owner and
has not been performed.** Its exit-gate item stays open, and so does issue
#18's packaging/installed-bundle criterion. The runnable checklist is in
"Installed-bundle pass" below.

`CURRENT_SCHEMA_VERSION` is **9**, applied by `0009_p8_attention.sql`.
`COMMAND_NAMES` grew from P7's 107 to **129** (P8 added 22 commands, spec
decision 18; the count assertions in `f1_contract_path.rs`,
`p3_contract_path.rs`, and `f1_residual_acceptance.rs` read
`58 + 21 + 2 + 8 + 11 + 4 + 1 + 2 + 7 + 6 + 5 + 4`).

## Final-review fix wave

After the final whole-branch review, one fix wave (on top of `b64fb57`)
changed behavior the rest of this record describes:

- **Focus is seeded at launch** (spec decision 41, ADR-0015 amended).
  `setup` seeds `Focus` once from the shown main window's `is_focused()`
  (`false` if that errors); `WindowEvent::Focused` is the source after
  that. Before, Focus started `true`, so a window the compositor opened
  unfocused stayed silent until the operator focused and left it once.
  The installed pass below gains a check for this.
- **Unchanged amendments persist at most once a minute per Event** (spec
  decision 40). `observationCount` now counts persisted observations and
  `lastObservedAt` can trail by up to a minute. The pass also reads the
  latest resolved Events only for keys it may insert, and filters failed
  Jobs by the epoch and "no Event yet" in SQL. The tracer's step 3 now
  checks that twenty passes inside the minute persist nothing and one a
  minute later persists exactly one observation. **The simulator run
  above predates this change to `p8_tracer.rs`**; the fakes run passes
  in `just test-rust` below.
- **The UI no longer shows `lastObservedAt` or `observationCount`**
  (spec decision 42): the Event detail keeps "First observed", and an open
  Attention center row is dated by `firstObservedAt`.
- Also: a rate-limited projector failure log, a chained (non-overlapping)
  camera preview poll, a cleared stale snapshot-image error, stale and
  out-of-order Incident detail fetches dropped, the P7 rig's no-adapter
  factory restored as the default (the P8 tracer opts into the real
  adapters), and `p8_setup.rs`'s unused `Rig::err` removed.

Gates on the fix wave (before this record's update): `just build` 0 (the
603 kB chunk warning remains), `just test` 0 (135 files, 1,760 tests),
`just test-rust` 0 (1,962 passed, 0 failed, 61 ignored, over 73 test
binaries; no warning beyond ts-rs's serde-attribute notes), and `just
gen-contracts` with no diff under `src/generated`. The simulators were
not re-run.

## Automated evidence

Each command ran with `source "$HOME/.cargo/env"` where it needs cargo.

| Command | Exit | Result |
| --- | --- | --- |
| `just build` | 0 | `tsc` and the Vite build pass. The main chunk is `index-BVEvONZo.js` at 603.00 kB (183.53 kB gzip), which now trips Vite's 600 kB chunk-size warning (P7's was 545.37 kB). A warning, not a failure; see "Follow-ups". |
| `just test` | 0 | 134 files, 1752 tests passed. |
| `just test-rust` | 0 | 73 test-result lines (the test binaries, the lib, and the doc-tests): 1956 passed, 0 failed, 61 ignored (the simulator and real-hardware tests, which need `just test-sim` or a live host env var). The lib has 973 passed, 2 ignored. The P8 binaries: `p8_attention` 14, `p8_backfill` 2, `p8_cameras` 31, `p8_guards` 7, `p8_incidents` 4, `p8_media` 21, `p8_migration` 12, `p8_notifications` 30, `p8_observe_plan` 144, `p8_setup` 11, `p8_tracer` 2 (1 ignored, simulator). The lib's P8 unit tests (`attention::*`, `incidents::*`, `notifications::*`, `printers::alerts`) are 58 more. Compiler output: the pre-existing ts-rs "failed to parse serde attribute" warnings, and one new dead-code warning (`p8_setup.rs`'s unused `Rig::err`, see "Follow-ups"). |
| `just gen-contracts`, then `git diff --exit-code src/generated` | 0 | No diff. |
| `just sim-up && FARM3D_SIM_REQUIRED=1 just test-sim; just sim-down` | 0 | `sim-up` ready after 4 s (images already built). 47 of 47: `p6_tracer` 3, `p7_tracer` 3, `p8_tracer` 3 (42.78 s; includes `p8_attention_tracer_runs_against_the_simulator` and `attention_tracer_runs_against_the_fakes`), `sim_elegoolink` 7 (each prints `PENDING`: the ElegooLink fake waits for #8's real-hardware captures), `sim_moonraker` 24 (273.07 s; includes `p8_a_host_webcam_resolves_through_the_simulator_and_fetches_the_test_pattern`), and `sim_octoprint` 7. The run recorded `sim-runs/20260928T154804Z`, committed as [`2026-09-28-p8-sim-manifest-20260928T154804Z.json`](../superpowers/baselines/2026-09-28-p8-sim-manifest-20260928T154804Z.json). `sim-down` exited 0, and `podman ps` lists no container afterwards. |
| `just check-hosts` | 0 | `check-hosts: no denylist (set FARM3D_PRIVATE_HOSTS or create .private-hosts); nothing checked`. No denylist is configured in this environment. Instead, `git diff 1a0e13a` (this task's whole diff, including the new manifest) was scanned for private IPv4 addresses in 192.168/16 and 10/8 and for mDNS (`local`-suffix) hostnames, plus local home paths: no match. |
| `just package` | 0 | Required: P8 adds a Linux dependency (`zbus`), a Tauri event, and a window-event hook. Built the deb (12,045,836 bytes), rpm (12,049,465 bytes), and AppImage (114,797,048 bytes), and `scripts/assert-package-contents.sh` passed (each bundle has the binary and the printer catalog, and no source, tests, credential files, or developer tooling such as `gen-catalog` or `fake-orca`). Hashes under "Packaging" below. Not installed (pending owner). |

### The simulator manifests

The harness writes each run's `manifest.json` and `test.log` under the
untracked `src-tauri/target/sim-runs/<UTC>/`. Each cited run is committed
as a verbatim copy of its `manifest.json` under
`docs/superpowers/baselines/`:

| Run | Copy | What it is |
| --- | --- | --- |
| `20260928T150610Z` | (deleted in `1a0e13a`) | Task 16's first evidence run, `repoCommit` `4ac3344`. **Superseded** by the fix round; its copy was replaced in the same commit that added the next one. |
| `20260928T152754Z` | [`2026-09-28-p8-sim-manifest-20260928T152754Z.json`](../superpowers/baselines/2026-09-28-p8-sim-manifest-20260928T152754Z.json) | Task 16's fix-round run, `repoCommit` `dd3f25e`: the tracer with the secret corpus pushed through a real capture and the archive-with-an-open-Event step. `src-tauri/src/connections/adapters.rs` cites it as the Moonraker `camera` capability's P8 evidence, so it stays committed. **Superseded as this record's citation** by the gate run below; between `dd3f25e` and `16b5388` only documentation and one code comment (the `adapters.rs` evidence note) changed. |
| `20260928T154804Z` | [`2026-09-28-p8-sim-manifest-20260928T154804Z.json`](../superpowers/baselines/2026-09-28-p8-sim-manifest-20260928T154804Z.json) | **This task's gate run** (`repoCommit` `16b5388`), the authoritative citation for this record. All 47 simulator-backed tests pass, including both P8 tracer entry points and the P8 sim-camera test. Apart from `recordedAt` and `repoCommit`, it is identical to the `152754Z` copy (same images, pins, reported versions, and config hashes). |
| `20260928T164915Z` | [`2026-09-28-p8-sim-manifest-20260928T164915Z.json`](../superpowers/baselines/2026-09-28-p8-sim-manifest-20260928T164915Z.json) | The re-run after the final-review fix wave (`repoCommit` `9421414`), which changed the projector (scoped reads, the 60 s amend throttle) and the tracer's step 3. All 47 simulator-backed tests pass again, including `p8_attention_tracer_runs_against_the_simulator`. `sim-down` exited 0 and no container is left. |

Each copy holds the engine, the image digests (including the sim camera's
nginx), the prind and Klipper pins, the reported Moonraker and Klipper
versions, and the config hashes (including `camera/*`). None of those
holds a home path, a hostname, or a non-loopback address; this task
checked the new copy before committing it. The `test.log` is **not**
committed (its compiler diagnostics carry a local home path); the
results are transcribed into the table above instead.

## Read-only camera evidence (real hardware)

P8 owner decision 14: the owner's Snapmaker U1 (Moonraker) was probed
**once**, read-only, during Task 16 with `just moonraker-live camera`
(`live_camera` in `src-tauri/tests/a0_moonraker_live.rs`). The host came
only from `FARM3D_MOONRAKER_HOST`, set in that one command's environment;
it is not recorded here or anywhere in the repository. The probe sends
`GET`s only (the webcam list and one snapshot) and writes no file. This
task did not contact the printer.

| Item | Result |
| --- | --- |
| Webcams listed | 2 |
| Webcam 1 | service `webrtc-camerastreamer`, relative snapshot URL |
| Webcam 2 | service `mjpegstreamer-adaptive`, absolute snapshot URL |
| Snapshot fetched | one, from the absolute-URL webcam, through `FrameFetcher` |
| Frame size | 84866 bytes |
| Content type | `image/jpeg` |
| SHA-256 | `a64e22eda0b5914df32ed7e6a4198019d711cb93a8e65e3ede3bf311a3355654` |

The frame stayed in memory. Nothing but `GET`s was sent. Only the U1's
webcam layout is evidenced (spec residual risks).

## Packaging and the installed-bundle pass (Task 17)

### Step 1: `just package`

Task 17's controller run (2026-09-28, at `1a0e13a`) exited 0, built the
deb, rpm, and AppImage, and passed `scripts/assert-package-contents.sh`.
This task ran `just package` again at `16b5388`; those bundles' hashes
are the current ones:

| Bundle | SHA-256 (this task, `16b5388`) | SHA-256 (Task 17, `1a0e13a`) |
| --- | --- | --- |
| `.deb` | `2f1cf4e0336efacdf0820004aec57a4c4c4565a853765503af8afd00feb1e1d6` | `5ef66f55eee670b4deee2291b5a940ba29fc23f033666d1e6d9befebdf0375c2` |
| `.rpm` | `d836f24d680283fd44c2ec1b67301de85be1b3a2df77fdb9546ba0661c8d3ddc` | `8cf3d1b0bbdde8da840add5840cd91ac61393a6af0a8fc856c2e494889c3bc06` |
| AppImage | `1aaf3683b727825adb0420be3ab6b8c9e34709566f367b0e3a2cfefd9eebdb91` | `40e45ec24b11bdca9273b78fa6529bd1bf21561887706a8592c5bbab5646a9ca` |

The hashes differ from Task 17's even though the source differs only in
documentation, so the bundles are not byte-reproducible between builds.
Neither build was installed: installing a system package is the owner's
step (on this Arch-based host, `just package-arch` and then
`sudo pacman -U packaging/arch/farm3d-bin-*.pkg.tar.zst`).

### Step 3: platform facts

| Item | Value |
| --- | --- |
| Desktop | KDE Plasma 6.7.5, KWin 6.7.5 (Wayland) |
| Notification server | `GetServerInformation` = `("Plasma", "KDE", "6.7.5", "1.2")`; implements `ActivationToken` (spike report) |
| GNOME | **Unavailable**: not installed on this host |
| Windows, macOS | **Unverified** (decision 15: F0 supports Linux x86_64 only; other platforms get the null sink) |

### Steps 2 and 4: installed-bundle pass — **PENDING OWNER**

**Not performed.** It needs a person at the desktop to install the
package, switch focus, click notifications, and look at them. There is no
input automation on this KDE Wayland session. Nothing below has been
checked yet; the owner runs it and records the results here.

Setup:

1. `source "$HOME/.cargo/env" && just package && just package-arch`, then
   `sudo pacman -U packaging/arch/farm3d-bin-*.pkg.tar.zst` (or install
   the `.deb` on a Debian-family host).
2. `just sim-up`. Launch the **installed** farm3d (not `just dev`).
3. Add a Moonraker Printer at `127.0.0.1`, port `27125` (the sim
   Moonraker, through the fault proxy), with the host webcam `farm3d-sim`
   as its camera, **Offline alert** "1 minute", and **Notifications**
   "Follow the notification settings". In the notification settings,
   turn the **Connectivity** class on (it is off by default).

Checks (tracer steps 2–7, by hand):

- [ ] **No notification while focused.** With farm3d focused, run
      `just sim fault host-down moonraker`, wait past the 1-minute grace:
      one `printer.offline` Event appears in the Attention center and no
      desktop notification is shown. Then `just sim fault host-up
      moonraker` and let the Event resolve.
- [ ] **One notification while another app is focused.** Focus another
      application, cut the host again, and wait past the grace: exactly
      one notification. Leaving it cut for several more minutes shows no
      second one.
- [ ] **Unfocused from launch** (decision 41's seed). Quit farm3d. Launch
      it and switch away before it takes focus (or launch it in the
      background), without ever focusing it. Trigger a failure (cut the
      host and wait past the grace): confirm a notification arrives.
- [ ] **Attribution.** The notification shows as farm3d, with farm3d's
      icon.
- [ ] **Click raises and opens the Printer.** Clicking the notification
      raises farm3d and opens that Printer on Monitor. Repeat with farm3d
      **minimized** first: the click still raises it (the Wayland
      hide → show → set_focus ruling). If the window only flashes in the
      taskbar, record that (spec residual risk: the raise depends on the
      daemon sending `ActivationToken`); the Printer must still open.
- [ ] **Acknowledge, then restore.** Acknowledge the Event: it stays
      open. `just sim fault host-up moonraker`: it resolves
      `conditionCleared`, still read and acknowledged.
- [ ] **Send test notification** (in the notification settings) shows
      one notification.
- [ ] **Do Not Disturb.** With Plasma's Do Not Disturb on, cut the host
      again while unfocused. Whether a popup shows is the daemon's
      business (ADR-0015); the Event must still appear in the Attention
      center.
- [ ] **Screenshots** saved as `docs/screenshots/p8-installed-*.png`: the
      notification popup, the raised window on the Printer, and the
      Attention center under Do Not Disturb.
- [ ] `just sim-down`.

## Acceptance criteria: issue #18

| # | Criterion | Evidence |
| --- | --- | --- |
| 1 | Event lifecycle, deduplication, recurrence, and startup-backfill tests pass. | **Met.** Lifecycle: every cell of D2's lifecycle table is `attention::lifecycle::tests::spec_table_every_state_and_op_cell`, with `acknowledge_follows_the_table`, `mark_read_follows_the_table`, `resolve_system_reasons_follow_the_table_for_every_mode`, `resolve_operator_resolved_follows_the_table_for_a_manual_event`, and `resolve_operator_resolved_is_not_manual_for_every_other_mode`; the schema side is `p8_migration.rs` `attention_events_one_open_per_dedup_key` and `attention_event_read_implication_checks_reject_unread_acknowledged_or_resolved`. Deduplication and recurrence: `p8_observe_plan.rs` (144 tests) runs every Condition fixture verbatim — the 90 catalogue fixtures `fixture_c1_a_n` … `fixture_c10_u_r`, the 29 supplementary `fixture_s1` … `fixture_s29` (s22: planned, applied, planned again gives only `Amend=`; s24: a recurrence links `recurrenceOf` to the newest resolution), the 5 deadline fixtures `fixture_d1` … `fixture_d5`, and `plan_is_a_fixed_point_for_every_fixture`. On the projector: `p8_attention.rs` `offline_opens_once_amends_quietly_resolves_and_recurs`, `a_lagged_receiver_forces_a_full_pass_without_duplicates`, `a_hundred_concurrent_wakes_open_exactly_one_event`, and `a_protocol_error_opens_a_connection_error_and_a_live_status_clears_it`. Startup backfill: `p8_backfill.rs` `backfill_projects_every_requirement_once_and_resolving_one_keeps_both_histories` and `backfill_leaves_printer_events_alone`, and `p8_observe_plan.rs` `fixture_s2`, `fixture_x1_backfill_archived_printer_resolves_its_printer_events`, `fixture_x2_backfill_setup_incomplete_resolves_connection_error`, `fixture_x3_backfill_offline_alerts_off_resolves_offline`, `fixture_x4_backfill_job_completed_absent_by_durable_facts_but_unknown_by_status`, `fixture_x5_backfill_still_projects_durable_families`, and `fixture_x11_no_supervisors_started_at_is_unknown_even_with_a_status`. |
| 2 | Retention/pinning, deep-link, and archive/delete reference tests pass. | **Met.** Retention and pinning: `p8_media.rs` (21), including the `plan_prune` tables `rows_older_than_the_retention_period_are_pruned_for_age`, `over_the_cap_the_oldest_unpinned_rows_go_for_the_disk_cap_until_usage_fits`, `age_goes_first_and_counts_toward_the_cap`, `pinned_rows_are_never_pruned`, and `when_only_pinned_rows_remain_the_disk_cap_actions_are_dropped_and_age_stays`; crash repair `a_crash_on_either_side_of_the_rename_leaves_what_the_sweep_repairs`, `the_sweep_deletes_an_orphan_from_a_crash_after_the_rename`, `the_sweep_deletes_a_pruned_file_whose_unlink_never_ran_and_the_row_stays_pruned`, and `a_missing_file_becomes_pruned_missing_file_even_when_pinned`; concurrency `twenty_concurrent_captures_against_a_pruning_pass_keep_usage_under_the_cap`; and `a_full_cap_of_pinned_evidence_refuses_a_capture`, `the_snapshot_commands_capture_pin_serve_and_count`, and `a_janitor_poke_prunes_under_the_stored_retention`. Deep links: `src/attention/deep-link.test.ts` (`sourceTarget`, `eventTarget`, `incidentTarget`, `targetForSource`, `openTargetFor`, including "an archived Printer -- still present, just archived -- is still a valid target" and "falls back to the Event when the source no longer exists"); `src/App.test.tsx` "routes a farm3d-navigate-v1 payload through navigate, changing the hash", "passes an Attention Event deep link to the dashboard as attentionEventId, …", and "is available for a closed Incident referenced only by a resolved Event's incidentId …"; backend `p8_notifications.rs` `a_click_raises_the_window_navigates_to_the_target_and_marks_the_event_read` and `a_click_on_a_deleted_source_opens_the_event_itself`. Archive/delete: `p8_guards.rs` (7) `a_printer_with_an_incident_can_be_archived_but_not_deleted` (the archived Printer's target still resolves), `deleting_a_printer_with_only_attention_events_resolves_them_source_removed` (the deleted source's Event deep-links to itself), `pinned_unattached_evidence_blocks_delete_and_the_rest_goes_with_the_printer`, `an_import_over_an_incident_fails_evidence_exists_and_writes_nothing`, `an_import_over_camera_evidence_fails_evidence_exists_and_writes_nothing`, `an_import_resolves_the_replaced_printers_open_events_source_removed`, and `a_repository_delete_removes_unattached_rows_and_resolves_open_events`. The tracer's step 12 does it live: delete refused `INCIDENT_HISTORY_EXISTS`, archive resolves the open Event, and the Printer and Incident targets still resolve. |
| 3 | Single/batch camera and alert setup preserves per-instance endpoints and evidence. | **Met.** Backend `p8_setup.rs` (11): `a_single_create_persists_its_camera_and_alert_defaults`, `a_snapshot_url_template_builds_each_rows_url_from_its_own_host` (every Printer gets its own copy of the alert defaults, and no health beyond a fresh `unknown`, no test result, and no `camera_snapshots` row), `a_host_webcam_template_resolves_against_each_rows_own_connection` (each Printer's health moves on its own fetch, never copied), `a_bad_camera_template_fails_the_batch_and_a_bad_override_rejects_its_row`, `an_invalid_camera_or_alert_default_rejects_the_whole_create_with_a_field_path`, `export_schema_4_round_trips_cameras_and_alert_defaults`, `schemas_2_and_3_still_import_with_no_camera_and_the_default_alerts`, and `the_seeded_corpus_stays_in_the_camera_column_and_the_export_file`. Batch dialog: `src/screens/PrinterBatchDialog.test.tsx` "never asks for a host, for either template kind", "shows a per-row Camera host override column only for a snapshotUrl template, never copying one row's value to another", "sends each row's own camera host override, never another row's", and "Test camera shows each row's own result, never crossing rows"; `src/screens/BatchRowsTable.test.tsx` "editing one row's camera host override never touches another row's" and "offers a per-row Test camera button, and shows that row's own result"; `src/printers/batch-intake.test.ts` "applying the same shared template to two rows never copies one row's own host override to the other" and "ignores a cameraHostOverride column: no row gets a silent host mapping". |
| 4 | The failure-to-notification-to-resolution tracer completes without duplicate Events. | **Met.** `p8_tracer.rs` `attention_tracer_runs_against_the_fakes` (CI, in `just test-rust`) and `p8_attention_tracer_runs_against_the_simulator` (in `just test-sim`; passed in this task's gate run `20260928T154804Z` and in Task 16's two runs). One core function runs both: cut the Printer, one `printer.offline` Event and one notification after the grace (twenty more observations add neither), the click navigates and marks it read, a restart backfills with no new Event or notification, acknowledge keeps it open, restore resolves it `conditionCleared` with its history intact, a second cut recurs with `recurrenceOf`, a failed print opens one `job.failed` Event, one Incident with one camera snapshot, and one linked material Event, deferral acknowledges, a restart adds nothing, settling and resolving close the Incident, pinning survives retention, and the archive/delete guard holds. After every step it asserts no dedup key has two open Events and that none of the seeded-secret corpus or the camera endpoint appears in any emitted event, navigation, notification, log line, or command response. |
| 5 | Packaging and installed-bundle notification/capability verification pass on every F0-supported platform. | **Open (pending owner).** `just package` passes (above). F0 supports Linux x86_64 only. The installed-deb notification, focus, and click-to-source checks have **not** been run; see "Steps 2 and 4". The automated parts are covered: `p8_notifications.rs` `decide_notifies_only_a_live_insert_of_an_enabled_unmuted_class_while_unfocused`, `nothing_is_shown_while_farm3d_has_focus_and_it_starts_focused`, the launch seed `the_seed_takes_is_focused_and_an_error_seeds_unfocused` (decision 41), `unfocused_a_live_insert_is_shown_once_with_its_target_and_marked_notified`, `a_click_raises_the_window_navigates_to_the_target_and_marks_the_event_read`, `without_focus_after_the_raise_the_window_is_remapped_then_flagged`, `a_focus_gained_after_the_raise_stops_the_fallbacks`, `an_unavailable_notifier_never_panics_and_the_commands_say_so`, and `the_null_sink_reports_unsupported`, plus the D-Bus spike on KDE Plasma 6.7.5 ([`2026-09-27-p8-notification-spike.md`](../superpowers/baselines/2026-09-27-p8-notification-spike.md)). Windows and macOS are unverified. |

### The plan's exit gate

- [x] **The event lifecycle, deduplication, recurrence, and
  startup-backfill tests pass** (`p8_observe_plan.rs`, `p8_attention.rs`,
  `p8_backfill.rs`): met (row 1).
- [x] **The retention/pinning, deep-link, and archive/delete reference
  tests pass** (`p8_media.rs`, `p8_guards.rs`, `deep-link.test.ts`): met
  (row 2).
- [x] **Single and batch camera/alert setup keeps endpoints and evidence
  per instance** (`p8_setup.rs` and the batch dialog tests): met (row 3).
- [x] **The failure-to-notification-to-resolution tracer completes
  without duplicate Events on the fakes (CI) and the simulator, with the
  manifest committed:** met (row 4;
  `2026-09-28-p8-sim-manifest-20260928T154804Z.json`).
- [ ] **`just package` passes, and the installed deb's notification,
  focus, and click-to-source checks pass on Linux x86_64, with Windows
  and macOS recorded as unverified:** **open.** `just package` passes and
  Windows/macOS are recorded as unverified, but the installed-deb checks
  are pending the owner (row 5).
- [x] **A camera never blocks monitoring, slicing, assignment, or Job
  control** (Task 8's optionality tests): met. `p8_cameras.rs`
  `printer_statuses_and_the_projector_never_wait_for_a_hanging_camera`,
  `a_slice_starts_while_a_camera_hangs`,
  `assign_queue_entry_succeeds_while_a_camera_hangs`, and
  `start_job_succeeds_while_a_camera_hangs`; and a broken media store
  never blocks startup (`p8_media.rs`
  `a_failed_media_sweep_degrades_capture_instead_of_blocking_startup`).

### The spec's acceptance criteria

The spec's list (its "Acceptance criteria" 1–15) maps onto the rows
above; the ones the issue doesn't name separately:

- **1. Migration:** `p8_migration.rs` (12), including
  `upgrading_v8_to_v9_keeps_every_existing_row_and_survives_a_restart`,
  `a_crash_before_commit_leaves_the_database_unchanged_at_v8`,
  `attention_events_one_open_per_dedup_key`,
  `incident_events_reject_update_delete_and_invalid_rows`,
  `attention_event_read_implication_checks_reject_unread_acknowledged_or_resolved`,
  and `no_new_p8_column_can_hold_a_credential`.
- **5. Incidents:** `p8_incidents.rs`
  `a_jobs_incident_opens_links_closes_and_reopens_with_the_right_revision_math`,
  `a_printer_host_failed_incident_has_no_job_and_never_reuses_an_earlier_one`,
  and `append_entry_sequence_is_monotonic`; `p8_attention.rs`
  `a_failed_job_opens_one_incident_that_the_commands_settle_and_close`.
- **6. Cameras:** `p8_cameras.rs` (31).
- **9. Notifications:** `p8_notifications.rs` (30), including
  `the_same_key_notifies_at_most_once_in_ten_minutes`,
  `a_fourth_notification_within_ten_seconds_becomes_one_summary_that_opens_the_attention_center`,
  and `the_body_is_the_severity_word_and_the_event_summary_verbatim`.
- **12. Secrets:** `the_seeded_secret_never_appears_in_attention_events_json_columns`,
  `the_seeded_secret_never_appears_in_incident_json_columns`,
  `the_seeded_secret_never_reaches_a_notification_navigation_response_or_log_line`,
  `the_seeded_corpus_stays_in_the_camera_column_and_the_export_file`, and
  the tracer's per-step scan.
- **13. Frontend:** the Task 12–15 tests pass in `just test`, and the
  screenshots exist at both viewports (below), except the Printer Status
  tab's open-Events list, which has a 1440×900 capture only.
- **15. Installed bundle:** open, as row 5.

## Visual verification

The screenshots are in `docs/screenshots/p8-*.png`, taken in the `just
web` passes of Tasks 13–15 at 1440×900 and 1024×700:
`p8-attention-center-*`, `p8-attention-event-detail-*`,
`p8-incident-detail-*`, `p8-snapshot-viewer-*`,
`p8-printer-camera-tab-*`, `p8-setup-camera-*`,
`p8-batch-camera-template-*`, `p8-notification-settings-*`, and
`p8-printer-status-events-1440x900.png` (1440×900 only). `just web` has
no Rust backend, so they show fixture states (the web fixtures, such as
`src/attention/web-fixtures.ts`), not live simulator data; the live path
is `p8_tracer.rs`. No `p8-installed-*` screenshot exists yet (pending
owner).

## Unavailable checks

- **The installed-bundle pass** (Task 17 Steps 2 and 4). **Pending
  owner:** it needs a person at the desktop, and there is no input
  automation (`xdotool`/`ydotool`) on this KDE Wayland session. The
  checklist is above.
- **Whether KWin sends `ActivationToken` before `ActionInvoked`, and
  whether it raises the window.** The spike couldn't click; it is part of
  the installed pass.
- **GNOME.** Not installed on this host.
- **Windows and macOS.** Unverified by decision 15; they get the null
  sink, and the in-app Attention center still works.
- **An interactive `just dev` click-through.** Not performed; the
  installed pass supersedes it.

## Known limitations

- **The Wayland raise depends on the notification daemon.** If KWin or
  Mutter doesn't send `ActivationToken`, a click may only flash the
  taskbar entry. `request_user_attention` is the fallback, and navigation
  still happens.
- **A GTK file dialog counts as unfocused,** so a notification can fire
  while the operator is in farm3d's own dialog.
- **Relative webcam URLs assume the web frontend is on port 80;**
  `webPort` makes this configurable per camera. Only the U1's layout is
  evidenced.
- **Listing host webcams needs the Printer online.** The host-facts
  camera count stays in memory and `list_host_webcams` queries live, so an
  offline Printer can't list webcams in Setup; the saved name still works
  once it's back.
- **A transient protocol error opens `printer.connectionError` with no
  grace** (spec residual risks), and the next live status resolves it.
- **The notification receiver drops what it missed when it lags.** A
  missed notification is never shown late; the Event is always in the
  Attention center.
- **Settings export v3 and Printers export v4 are schema bumps** that
  P9's backup format must absorb (a P9 input).
- ~~**Batch create never pokes the queue evaluator.**~~ Pre-existing, and
  fixed after this gate run in `617f8d8`: a batch that creates any Printer
  now sends `Trigger::PrinterChanged`, like every single-Printer command
  (`p2_batch.rs` `a_batch_that_creates_a_printer_wakes_the_evaluator`).

## Follow-ups

- **The installed-bundle pass** (owner): run the checklist above, save
  `docs/screenshots/p8-installed-*`, record the results here, and then
  tick the exit gate's packaging item and issue #18's last criterion.
- ~~**Test-rig scope.**~~ Resolved in the final-review fix wave: the P7
  dispatch rig keeps its no-adapter factory by default, and only the P8
  tracer opts into `build_connection` (`AttentionBoot::real_adapters`).
- **Deferred minors from the task reviews:**
  - the frontend `DEFAULT_SETTINGS` duplicates the Rust notification
    defaults;
  - replaying `set_printer_alert_defaults` after the Printer is deleted
    returns the defaults;
  - the Attention center's `knownSourceIds` duplicates `App`'s
    `navigationContext` id set;
  - Acknowledge and Resolve share one pending flag;
  - the `aria-live` region keeps only the last of several announcements
    made in the same tick.
- ~~**New in this gate run:** the main JS chunk (603 kB) now trips Vite's
  600 kB warning.~~ Fixed after this gate run in `bfab050`: the Attention
  center, the Event and Incident dock detail, and the Notifications and
  retention dialog load lazily, and the main chunk is 583 kB with no
  warning (545 kB before P8). (`p8_setup.rs`'s unused `Rig::err` was
  removed in the final-review fix wave.)

## Documentation updated in this task

- **The spec** (commit `16b5388`, plus decision 39's lag wording in this
  record's commit). "Startup backfill" says a durable `Absent` beats the
  backfill's `Unknown` (fixtures x1–x4 and x11). D3 says a new Incident
  starts at `revision` 1, and a close or reopen costs one bump.
  `EvidenceSkipReason` gains `storage` in D3's timeline table, D4's
  recorded outcomes, and the wire types. `NotifyCandidate` is
  `{ event, change: EventChange }`. The candidate channel is a
  `broadcast` of 64 committed passes, not an `mpsc` of 256. New decisions
  35–39 record each change, and a new residual risk covers transient
  protocol errors.
- **ADR-0015.** It no longer cites the notification spec as version 1.3:
  it records that Plasma 6.7.5 reports version 1.2 and still implements
  every signal farm3d uses, including `ActivationToken`, and that farm3d
  doesn't gate on the reported version.
- **`CONTEXT.md`.** Attention Event: only Printer, low-Spool, and
  start-confirmation Conditions recur; a Job's end and a Reconciliation
  Requirement get one Event each. The other P8 terms (Incident,
  Condition, Snapshot, Camera Source, Alert defaults) match the code.
- **`README.md`.** A `just moonraker-live <mode>` row (with the read-only
  `camera` mode), and the sim camera in `just sim-up` and "Printer
  simulators".
- **The v1 approach doc's known-unknowns register.** "Attention
  deduplication/recurrence" and "Notification plugin/OS behavior" are
  **Resolved (P8)**, linking ADR-0014/ADR-0015, the P8 spec, the spike
  report, and this record. The notification row notes that the
  installed-bundle checks are still pending the owner.

## Files changed in this task

- `docs/superpowers/specs/2026-09-27-p8-attention-incidents-cameras-notifications-design.md`
- `docs/adr/0015-desktop-notifications-over-dbus.md`
- `CONTEXT.md`
- `README.md`
- `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`
- `docs/superpowers/baselines/2026-09-28-p8-sim-manifest-20260928T154804Z.json`
  (new, this task's gate run)
- `docs/verification/2026-09-28-p8-attention-incidents.md` (this file)
