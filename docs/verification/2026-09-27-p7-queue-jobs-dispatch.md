# P7 Queue Entries, Jobs, and dispatch verification

**Date:** 2026-09-27
**Platform:** Linux x86_64 (Linux 7.2.8-1-cachyos), podman for the
simulators.
**Validated source:** branch `feature/p7-queue-jobs-dispatch`, on top of
`deaba0e` (Task 16's tracer fix round, "docs(p7): record the simulator
run for the tightened tracer"), plus this task's spec-text fixes
(`5e0a2cb`). Every gate in "Automated evidence" ran on that tree (the
sim-gate manifest's `repoCommit` is `5e0a2cb`).
**Covers:** Tasks 1–17 of
[`docs/superpowers/plans/2026-09-26-p7-queue-entries-jobs-dispatch.md`](../superpowers/plans/2026-09-26-p7-queue-entries-jobs-dispatch.md),
for GitHub issue #17. The binding spec is
[`docs/superpowers/specs/2026-09-26-p7-queue-entries-jobs-dispatch-design.md`](../superpowers/specs/2026-09-26-p7-queue-entries-jobs-dispatch-design.md),
with ADR-0013 (Queue and Job state machines).

**Overall:** every automated gate passes: `just build`, `just test`,
`just test-rust`, `just gen-contracts` (no diff), and the full simulator
suite including the P7 live-host tracer. `just check-hosts` checks
nothing in this environment (no denylist configured). `just package` was
not run: P7 changes no bundling configuration
(`git diff main -- src-tauri/tauri.conf.json` is empty). Screenshots are
in `docs/screenshots/p7-*` (Task 15's web-mode captures, plus the manual
`just dev` launch screenshot from Task 16 Step 3).

`CURRENT_SCHEMA_VERSION` is **8**, applied by `0008_p7_queue_jobs.sql`.
`COMMAND_NAMES` grew from P6's 89 to **107** (P7 added 18 commands
across Tasks 6, 7, 8, and 9 — the count assertions in
`f1_contract_path.rs`, `p3_contract_path.rs`, and
`f1_residual_acceptance.rs` read `58 + 21 + 2 + 8 + 11 + 4 + 1 + 2`).

## Automated evidence

Each command ran with `source "$HOME/.cargo/env"` where it needs cargo.

| Command | Exit | Result |
| --- | --- | --- |
| `just build` | 0 | `tsc` and the Vite build pass. The main chunk is `index-CTxutor1.js` at 545.37 kB (165.93 kB gzip). |
| `just test` | 0 | 117 files, 1514 tests passed. |
| `just test-rust` | 0 | 60 test binaries (including the lib and the doc-tests): 1605 passed, 0 failed, 58 ignored (the simulator and real-hardware tests, which need `just test-sim` or a live host env var). The lib has 904 passed, 2 ignored. The P7 binaries: `p7_evaluator` 15, `p7_guards` 11, `p7_host_ops_seams` 10, `p7_jobs` 60, `p7_migration` 11, `p7_queue` 11, `p7_restart_matrix` 25, `p7_settlement` 15, `p7_tracer` 2 (1 ignored, simulator). |
| `just gen-contracts`, then `git diff --exit-code src/generated` | 0 | No diff. |
| `just sim-up && FARM3D_SIM_REQUIRED=1 just test-sim && just sim-down` | 0 | 43 of 43: `p6_tracer` 3, `p7_tracer` 3, `sim_elegoolink` 7, `sim_moonraker` 23 (261.58 s; includes `p7_a_cancelled_print_gives_the_tracker_a_cancelled_verdict` and `p7_an_emergency_stop_gives_the_tracker_a_failed_verdict`), and `sim_octoprint` 7. The run recorded `sim-runs/20260927T173757Z`, committed as [`2026-09-27-p7-sim-manifest-20260927T173757Z.json`](../superpowers/baselines/2026-09-27-p7-sim-manifest-20260927T173757Z.json). `sim-down` exited 0 and stopped every simulator container. |
| `just check-hosts` | 0 | `check-hosts: no denylist (set FARM3D_PRIVATE_HOSTS or create .private-hosts); nothing checked` — no denylist is configured in this environment. |
| `just package` | — | **Not required.** P7 changes no bundling config: `git diff main -- src-tauri/tauri.conf.json` is empty. |

### The simulator manifests

The harness writes each run's `manifest.json` and `test.log` under the
untracked `src-tauri/target/sim-runs/<UTC>/`. Each cited run is committed
as a copy under `docs/superpowers/baselines/`:

| Run | Copy | What it is |
| --- | --- | --- |
| `20260927T165142Z` | [`2026-09-27-p7-sim-manifest-20260927T165142Z.json`](../superpowers/baselines/2026-09-27-p7-sim-manifest-20260927T165142Z.json) | Task 16's first evidence run, before the tracer was tightened; `repoCommit` is the branch's base commit at that point. **Superseded** — its tracer round found the stale-progress bug (fixed in `f25f58e`) and R22 was revised afterward. |
| `20260927T170707Z` | [`2026-09-27-p7-sim-manifest-20260927T170707Z.json`](../superpowers/baselines/2026-09-27-p7-sim-manifest-20260927T170707Z.json) | Task 16's fix-round run, `repoCommit` `0dc0f17` — the tightened tracer and the revised R22 rule. **Superseded** by this task's fresh run below; the code didn't change between `0dc0f17` and this task's gate run except this task's own spec-text edits. |
| `20260927T173757Z` | [`2026-09-27-p7-sim-manifest-20260927T173757Z.json`](../superpowers/baselines/2026-09-27-p7-sim-manifest-20260927T173757Z.json) | **This task's gate run** (`repoCommit` `5e0a2cb`), the authoritative citation for this record. All 43 simulator-backed tests pass, including both P7 tracer entry points and the two P6 tracer entries. |

Each copy holds the manifest verbatim under `manifest`: the engine, the
image digests, the prind and Klipper pins, the reported Moonraker and
Klipper versions, and the config hashes. None of those holds a home path,
a hostname, or a non-loopback address. The `test.log` is **not**
committed (its lines carry a local home path in compiler diagnostics); a
`results` summary is transcribed into the "Automated evidence" table
above instead, with every P7-relevant test's name and outcome.

## Acceptance criteria: issue #17

| # | Criterion | Evidence |
| --- | --- | --- |
| 1 | State-machine model, transaction/concurrency, and restart-matrix tests pass. | Migration and schema: `p7_migration.rs` (11 tests, e.g. `at_most_one_active_job_per_printer`, `job_checks_reject_invalid_rows`, `queue_entry_checks_reject_invalid_rows`, `a_crash_before_commit_leaves_the_database_unchanged_at_v7`). Transaction/concurrency: `p7_jobs.rs` (60), `p7_queue.rs` (11, e.g. `add_to_queue_replay_returns_the_same_entries_and_publishes_nothing`), `p7_host_ops_seams.rs` (10). The restart matrix: `p7_restart_matrix.rs`, 25 tests naming every crash boundary in the spec's transition table — `restart_inside_assign_rolls_back_everything`, `restart_after_assign_stages_once`, `restart_before_upload_send_returns_the_job_to_assigned`, `restart_during_upload_reconciles_then_awaits_start`, `restart_after_upload_success_applies_the_outcome`, `restart_before_start_send_returns_to_awaiting_start`, `restart_after_start_reply_lost_reconciles_without_a_second_start`, `restart_after_start_success_applies_the_outcome`, `restart_inside_release_rolls_back`, `restart_after_release_keeps_the_replacement_in_place`, `restart_after_abandoned_start_is_outcome_unknown`, `restart_after_pause_success_applies_the_outcome`, `restart_after_assign_stages_once_when_the_printer_connects_late`, `restart_before_unattended_start_send_never_starts_again`, `restart_after_unattended_start_reply_lost_never_starts_again`, `restart_during_printing_keeps_tracking`, `restart_after_host_completed_completes_once`, `restart_after_host_failed_opens_one_requirement`, `restart_after_terminal_keeps_settlement_pending`, `restart_inside_settle_rolls_back`, `restart_after_defer_keeps_the_amount_unavailable`, `restart_after_cancel_reply_lost_ends_cancelled_once`, `restart_after_declare_is_stable`, `restart_keeps_host_unreachable_since_and_allows_declare_after_30_minutes`, `endpoint_change_during_printing_never_pins_a_job_on_the_new_host`. |
| 2 | Three-copy lineage remains linked but independently actionable. | `p7_queue.rs` `add_to_queue_with_quantity_three_creates_three_linked_entries`; `p7_jobs.rs` `three_copies_are_independently_assignable_releasable_and_retryable` and `removing_an_assigned_entry_points_at_its_job`; end to end on both backends, `p7_tracer.rs` `queue_tracer_runs_against_fake_moonraker` (CI) and `p7_queue_tracer_runs_against_the_simulator` (simulator, steps 2–3, 9, 11: three linked entries share `lineageId`/`copyCount`, copy 2 is blocked `JOB_ACTIVE` once copy 1 is assigned, and a retry of copy 3 joins the same lineage with the old history unchanged — asserted `lineage.len() == 4`). |
| 3 | Successful, failed, cancelled, estimated, measured, and deferred reconciliation each settle exactly once. | `p7_settlement.rs` (15 tests): `completion_consumes_the_estimate_in_the_terminal_transaction`, `completion_correction_records_one_measurement_marked_as_correction`, `second_correction_is_rejected`, `failed_job_marks_the_reservation_unresolved_and_opens_a_pending_requirement`, `estimated_settlement_uses_the_progress_scaled_estimate`, `settle_measured_rejects_a_non_measured_or_out_of_range_entry`, `correct_rejects_a_non_measured_or_out_of_range_entry`, `measured_settlement_consumes_once_and_resolves_the_requirement`, `defer_keeps_the_amount_unavailable_and_blocks_over_reservation`, `deferred_then_measured_settles_exactly_once`, `settle_replay_returns_the_same_result_and_writes_no_second_ledger_row`, `cancelled_before_start_needs_no_settlement_and_releases_the_reservation`, `outcome_unknown_declared_failed_then_settled`, `every_settlement_path_publishes_job_spool_and_requirement_changes_once`. Exactly-once end to end on both backends: `p7_tracer.rs` runs all three outcomes in one lineage — copy 1 completes (estimate consumed, then one measured correction), copy 2 is cancelled mid-print (deferred, then settled measured), copy 3 fails (`klippy_shutdown`, settled by estimate, then retried) — with `t.assert_writes` pinning the host writes at each step so nothing double-sends. |
| 4 | Archive/delete reference tests and frontend workflow tests pass. | `p7_guards.rs` (11): `archive_is_blocked_by_an_active_job_and_allowed_after_terminal`, `archive_keeps_job_identity_and_printer_snapshot`, `delete_is_blocked_by_job_history`, `delete_is_blocked_by_a_pinned_queued_entry`, `slice_revision_delete_is_blocked_by_any_entry_or_job`, `model_delete_stays_blocked_transitively`, `printer_import_is_rejected_whole_while_any_job_exists`, `printer_import_is_rejected_whole_while_a_queue_entry_is_pinned_with_no_job`, `endpoint_change_is_blocked_only_by_an_unresolved_host_operation`, `spool_archive_is_blocked_by_an_unresolved_job_reservation`, `no_row_is_orphaned_after_every_allowed_lifecycle_action`. The tracer's step 12 exercises archive/delete live: "Archive the Printer: blocked while a Job is active, allowed after. Delete stays blocked (`JOB_HISTORY_EXISTS`)." Frontend workflow tests: `src/queue/queue-store.test.ts` (30), `src/screens/QueueScreen.test.tsx` (24), `src/screens/QueueEntryDetail.test.tsx` (11), `src/screens/JobPanel.test.tsx` (12), `src/screens/PrinterJobPanel.test.tsx` (23), `src/screens/AddToQueueDialog.test.tsx` (6), `src/screens/QueuePreview.test.tsx` (6), and `src/screens/HostOperationAlert.test.tsx` ("HostOperationAlert: OPEN_JOB" — the archive/delete blocker's Job link). |
| 5 | The live-host tracer completes. | `p7_tracer.rs` `p7_queue_tracer_runs_against_the_simulator` (`#[ignore]`, driven by `just test-sim`): passed in the committed `20260927T173757Z` manifest's run (and in the two earlier runs). It runs every step in the module doc comment against the real Moonraker simulator: three linked entries from one Add to Queue, `explain_queue_entry` eligibility, assign/reserve, restart at every transition, stage, bed-clear-gated start, a completed print with exactly-once settlement (plus a measured correction), a cancelled print settled deferred-then-measured, a failed print (`emergency_stop` → `klippy_shutdown`) settled by estimate and then retried into the same lineage, and the archive/delete guard. |
| 6 | Automatic dispatch is not enabled behind an unproven adapter. | `p7_evaluator.rs` `no_automatic_assignment_behind_an_unproven_adapter`; `src-tauri/src/queue/eligibility.rs` `readonly_hardware_evidence_never_qualifies_for_automatic`. |

### The plan's exit gate

- **State-machine model, transaction/concurrency, restart-matrix tests
  pass:** met (row 1).
- **Three-copy lineage stays linked but independently actionable:** met
  (row 2).
- **Successful, failed, cancelled, estimated, measured, deferred
  reconciliation each settle exactly once:** met (row 3).
- **Archive/delete reference tests and frontend workflow tests pass:**
  met (row 4).
- **The live-host tracer completes, manifest committed:** met (row 5;
  `2026-09-27-p7-sim-manifest-20260927T173757Z.json`).
- **Automatic dispatch not enabled behind an unproven adapter:** met
  (row 6).

## Visual verification

The screenshots are in `docs/screenshots/p7-*.png` (one size each,
`-1440.png`/`-1024.png`, taken in Task 15's `just web` pass): `p7-queue-*`
(the Queue screen's ordered rows and filters), `p7-queue-detail-*` (a
row's detail and blocker explanation), `p7-queue-add-*` (Add to Queue),
`p7-assign-*` (candidates and policy), `p7-start-*` (the bed-clear
confirmation), `p7-job-*` (the Job panel), `p7-settle-*` (the
estimated/measured/deferred reconciliation choice), and `p7-preview-*`
(the Monitor's Queue preview). `just web` has no Rust backend, so these
show fixture states (`src/host-ops/web-fixtures.ts`), not live simulator
writes — the live writes are `p7_tracer.rs` above.

`p7-desktop-launch.png` is the Task 16 Step 3 manual pass (below), copied
from `.superpowers/sdd/2026-09-26-p7-queue-entries-jobs-dispatch/`.

### Manual `just dev` pass (Task 16 Step 3, performed by the controller)

`just dev` was launched with isolated `XDG_CONFIG_HOME` and
`XDG_DATA_HOME` (pointed at a scratch directory), so the owner's real
`~/.config/farm3d` database was never touched — it would otherwise have
been irreversibly migrated to schema v8 from an unmerged branch. The
desktop build compiled, migrated a fresh database through `0008`, and
rendered the Monitor screen with the new Queue activity button, the
Active Jobs count, and the Queue preview (`p7-desktop-launch.png`).

The interactive click-through (stage, start, settle) was **not**
performed in this pass: there is no input automation on this KDE Wayland
session (no `xdotool`/`ydotool`), so it is recorded under "Unavailable
checks" below rather than attempted. That workflow is covered instead by
`p7_tracer.rs` on the live simulator (row 5 above) plus the web-mode
screen tests (`QueueScreen.test.tsx`, `QueueEntryDetail.test.tsx`,
`JobPanel.test.tsx`, `PrinterJobPanel.test.tsx`) and their screenshots.

## Unavailable checks

- **Interactive click-through of the native `just dev` build.**
  **Unavailable:** no input automation (`xdotool`/`ydotool`) exists on
  this KDE Wayland session, and no human was driving it during the
  controller's manual pass. The flow it would have exercised (stage,
  bed-clear start, settle) is proven instead by `p7_tracer.rs` against
  the live Moonraker simulator and by the frontend screen tests against
  fixtures.
- **The installed-package staging pass.** Not attempted: `just package`
  itself was not run (P7 changes no bundling config), so there was
  nothing to install.

## Known limitations

- **Progress after an ended status doesn't ratchet, even once the Job is
  pinned (revised ruling R22).** The driver pins a Job from history
  before it reads the tracker's cached status, so a pinned Job can still
  see a stale printing/paused frame or an ended one from a previous run
  of the same file. Progress therefore counts only while the host
  reports the Job's own file printing or paused, never from an ended
  status. **Accepted residual risk:** a print that ends while farm3d is
  disconnected settles a `settlementPreview` estimate from the last
  progress observed live, undercharging the Spool. The operator sees the
  preview and can choose "Enter measured remaining weight" instead. See
  the spec's D7 Progress bullet and decision list (Task 16's fix round
  1, commit `0dc0f17`).
- **The `list_queue` cache-or-compute rule can show a stale summary for
  an edited entry.** Once the evaluator has run, `list_queue` reuses the
  last run's `EligibilitySummary` for a `queued` entry matched by id
  only — an entry edited since the last run (a policy or manual-Printer
  change) keeps its last-run summary until the evaluator's next run
  replaces it. The edit's `QueueChanged` trigger pokes that run, but
  `list_queue` doesn't wait for it. See the spec's D6 (this task's text
  fix) and `queue/commands.rs:194-227`.
- **A no-Printer Automatic entry's `EligibilitySummary.topBlocker` is
  `null`, not `SETUP_INCOMPLETE`.** The explanation lives in
  `nextAutomaticAction.blocker` instead, which does carry
  `SETUP_INCOMPLETE`. A caller reading only a summary's `topBlocker` in
  isolation would see nothing. See the spec's new decision 26 and
  `queue/evaluator.rs:260-268`.
- **Simulator timing.** The live tracer's Moonraker print takes about
  10 s (a no-motion, ~120 MB G-code fixture), and the queued-start and
  history-poll waits in `sim_moonraker.rs` depend on host speed; both
  fail loudly (timeout) rather than silently on a slow host.
- **Moonraker history pagination.** The tracker reads the newest 50 jobs;
  a Printer that runs more than 50 other jobs during one farm3d Job would
  lose the pinned job from the page and end `outcomeUnknown` (spec
  residual risk, unchanged from Task 1).

## Follow-ups

- **Pre-existing P6 flake (out of P7 scope).**
  `a_host_unreachable_throughout_stays_uncertain_and_abandon_lifts_the_guards`
  (`p6_host_ops.rs`) has flaked intermittently under load since P6,
  likely from freed-port reuse in `seed_unreachable_uncertain`
  (`p6_host_ops.rs:1147-1148`); see Task 10f's ledger entry. It **passed**
  in this task's `just test-rust` gate run. A fix (hold the listener, or
  use an RFC 5737 address) is a follow-up, not a P7 blocker.
- **OctoPrint and ElegooLink dispatch.** P7's automatic and manual
  dispatch both refuse an adapter without simulator-proven capabilities
  (`no_automatic_assignment_behind_an_unproven_adapter`,
  `readonly_hardware_evidence_never_qualifies_for_automatic`). OctoPrint
  and ElegooLink command capabilities remain P6's open follow-up.
- A native, interactive `just dev` click-through by the owner (see
  "Unavailable checks").

## Documentation updated in this task

- **The spec.** D7's Staging bullet now matches decision 25/ruling R13
  (unreachable, timeout, and host-operation-pending defer; other
  refusals record `lastFailure`), D6 documents the `list_queue`
  cache-or-compute rule for entries edited since the evaluator's last
  run, D7's Progress bullet states the revised ruling R22 rule with its
  accepted residual risk, and two new decisions (26, 27) record the
  no-Printer `topBlocker` fallback and the operation-id-derived lineage
  id.
- **The v1 approach doc's known-unknowns register.** "Queue/Job state
  machine" is marked **Resolved (P7)**, linking ADR-0013, the P7 spec,
  `p7_restart_matrix.rs`, `p7_tracer.rs`, and this record.
- **`CONTEXT.md`.** Reviewed for drift from Task 1's write; none found —
  its Queue/Job/Settlement/Reconciliation Requirement vocabulary already
  matches this record's evidence.
- **`README.md`.** No new recipe: P7 added `p7_tracer` to the existing
  `test-sim` recipe's test list only (no user-facing recipe change), so
  no README update was needed.
- **Screenshots.** `docs/screenshots/p7-desktop-launch.png` (new, this
  task, copied from the controller's manual pass). The other
  `docs/screenshots/p7-*.png` files were already in place from Task 15.

## Files changed in this task

- `docs/superpowers/specs/2026-09-26-p7-queue-entries-jobs-dispatch-design.md`:
  D6, D7, and two new decisions (see above).
- `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`:
  the known-unknowns register row.
- `docs/superpowers/baselines/2026-09-27-p7-sim-manifest-20260927T173757Z.json`
  (new, this task's gate run).
- `docs/screenshots/p7-desktop-launch.png` (new).
- `docs/verification/2026-09-27-p7-queue-jobs-dispatch.md` (this file).
