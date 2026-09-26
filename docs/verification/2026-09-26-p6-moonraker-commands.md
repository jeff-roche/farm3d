# P6 Moonraker command capabilities verification

**Date:** 2026-09-26
**Platform:** Linux x86_64 (Linux 7.2.6-1-cachyos), podman for the
simulators.
**Validated source:** branch `feature/p6-connection-command-capabilities`,
on top of `8102951` (`test(host-ops): task 13 review fixes (round 1)`),
with this task's changes: the committed sim-manifest copies, the evidence
`source` repointed at them, and the docs below. Every gate in
"Automated evidence" ran on that tree.
**Covers:** Tasks 1–14 of
[`docs/superpowers/plans/2026-09-25-p6-connection-command-capabilities.md`](../superpowers/plans/2026-09-25-p6-connection-command-capabilities.md),
for GitHub issue #16, **Moonraker only**. The binding spec is
[`docs/superpowers/specs/2026-09-25-p6-connection-command-capabilities-design.md`](../superpowers/specs/2026-09-25-p6-connection-command-capabilities-design.md),
with ADR-0011 (composable capabilities) and ADR-0012 (the container
simulators are live evidence).

**Overall:** every automated gate passes, including the simulator suite
and the tracer on the simulator. Every Moonraker capability is `supported`
with `tier: sim` evidence that cites a committed manifest copy. A
read-only pass against the owner's Snapmaker U1 (host withheld) passed
with zero writes. The installed-package staging pass and the native
`just dev` pass are **unavailable** (see below). The screenshots come from
`just web`, so they show fixture states, not live simulator writes.

`CURRENT_SCHEMA_VERSION` is **7**, applied by
`0007_p6_host_operations.sql`.

## Automated evidence

Each command ran with `source "$HOME/.cargo/env"` where it needs cargo.

| Command | Exit | Result |
| --- | --- | --- |
| `just build` | 0 | `tsc` and the Vite build pass. The main chunk is `index-*.js` at 480.05 kB (146.90 kB gzip). |
| `just test` | 0 | 102 files, 1323 tests passed. |
| `just test-rust` | 0 | 51 test binaries: 1305 passed, 0 failed, 55 ignored (the simulator and real-hardware tests, which need `just test-sim` or `FARM3D_MOONRAKER_HOST`). The lib has 784 passed and 2 ignored. The P6 binaries: `p6_capabilities` 4, `p6_guards` 18, `p6_host_ops` 52, `p6_migration` 9, `p6_moonraker_adapter` 39, `p6_moonraker_readonly` 5 (6 ignored, real host), and `p6_tracer` 2 (1 ignored, simulator). |
| `just gen-contracts`, then `git diff --exit-code src/generated` | 0 | No diff once this task's regenerated `CapabilityEvidence.ts` (a doc comment on `source`) is committed. |
| `just sim-up && FARM3D_SIM_REQUIRED=1 just test-sim && just sim-down` | 0 | 38 of 38: `p6_tracer` 3, `sim_elegoolink` 7, `sim_moonraker` 21 (229.78 s), and `sim_octoprint` 7. The run recorded `sim-runs/20260926T051050Z`, committed as [`2026-09-26-p6-sim-manifest-20260926T051050Z.json`](../superpowers/baselines/2026-09-26-p6-sim-manifest-20260926T051050Z.json). `sim-down` exited 0. The unrelated `farm3d-a02-octoprint` container was left running. |
| `FARM3D_PRIVATE_HOSTS="<owner denylist, 4 strings>" just check-hosts` | 0 | `check-hosts: ok (4 denylisted strings, none found)`, before each commit. The staged diff had no home-directory path. |
| `just package` | 0 | A release build and three bundles: `.deb`, `.rpm`, and `.AppImage`. `scripts/assert-package-contents.sh` passed. The installed-package pass is unavailable (see [Packaging](#packaging)). |

### The simulator manifests (ruling R23)

The harness writes each run's `manifest.json` and `test.log` under the
untracked `src-tauri/target/sim-runs/<UTC>/`. Each cited run is committed
as a copy under `docs/superpowers/baselines/`:

| Run | Copy | What it is |
| --- | --- | --- |
| `20260926T033455Z` | [`2026-09-26-p6-sim-manifest-20260926T033455Z.json`](../superpowers/baselines/2026-09-26-p6-sim-manifest-20260926T033455Z.json) | Task 12's evidence run, `repoCommit` `6ef279d`. The Moonraker evidence rows cite it. |
| `20260926T042333Z` | [`2026-09-26-p6-sim-manifest-20260926T042333Z.json`](../superpowers/baselines/2026-09-26-p6-sim-manifest-20260926T042333Z.json) | Task 13's tracer run, `repoCommit` `3280cbd`. It is the first run with `p6_tracer`. |
| `20260926T051050Z` | [`2026-09-26-p6-sim-manifest-20260926T051050Z.json`](../superpowers/baselines/2026-09-26-p6-sim-manifest-20260926T051050Z.json) | This task's gate run, `repoCommit` `8102951` plus this task's uncommitted changes. |

Each copy holds the manifest verbatim under `manifest`: the engine, the
image digests, the prind and Klipper pins, the reported Moonraker and
Klipper versions, and the config hashes. None of those holds a home path,
a hostname, or a non-loopback address. The `test.log` is **not**
committed, because its compiler lines carry a local home path. A `results`
list is transcribed from it instead, with every test's name and outcome
and each binary's summary line. All three runs report the same image
digests and pins: Moonraker `v0.11.0-1-g1cfb0c4-prind`, Klipper
`v0.13.0-770-gce7002bed-prind`, OctoPrint 1.11.8, and Toxiproxy 2.12.0.

`MOONRAKER_SIM_MANIFEST` in `src-tauri/src/connections/adapters.rs` now
names the committed copy (a repository-relative path) instead of
`sim-runs/…`. Two tests assert the path:

- `adapters.rs`
  `moonraker_has_sim_evidence_for_every_capability_from_the_p6_run`
  reads the committed file and checks its `manifest` block.
- `p6_capabilities.rs`
  `the_adapter_matrix_lists_moonraker_supported_and_octoprint_not_verified_over_ipc`
  checks that every capability's `source` is a committed file.

The `CapabilityEvidence.source` doc comment and the spec's D6 comment now
describe the committed path.

## Acceptance criteria: issue #16 (Moonraker)

| # | Criterion | Evidence |
| --- | --- | --- |
| 1 | An evidence-backed capability row records supported and unsupported results. | `adapters.rs`: one `tier: sim` row per capability (`upload`, `start`, `pause`, `resume`, `cancel`, `hostState`, `artifactIdentity`, `camera`). Each cites the committed manifest copy and `Moonraker v0.11.0-1-g1cfb0c4-prind API 1.5.0`. The read-only real-host version `Moonraker 1.5.2 API 1.4.0 (read-only hardware)` is added to `hostState`, `artifactIdentity`, and `camera` only (`moonraker_has_sim_evidence_for_every_capability_from_the_p6_run`). Unsupported results: `capabilities.rs` (the OctoPrint `notVerified` test, the camera host rule when `cameraCount == 0`, and the matrix test); `p6_capabilities.rs` `the_adapter_matrix_lists_moonraker_supported_and_octoprint_not_verified_over_ipc` and `a_printer_with_no_connection_is_unsupported_everywhere_over_ipc`. Detection on every simulator variant: `sim_moonraker.rs` `p6_capability_detection_on_every_simulator_variant` (1 tool; 4 tools; no bed; apikey). |
| 2 | Real-host command tests cover uncertainty, interruption, restart, and reconciliation. | **Writes run on the simulators only** (owner decision 8). `sim_moonraker.rs`, all passing in all three runs: `p6_a_lost_upload_response_reconciles_after_a_restart_to_one_file`, `p6_an_upload_cut_mid_body_settles_then_fails_not_applied`, `p6_a_start_whose_response_is_cut_reconciles_to_succeeded`, `p6_a_queued_start_stays_uncertain_until_it_runs`, `p6_a_klipper_restart_keeps_a_started_row_and_an_uncertain_start_no_longer_pending`, `p6_a_host_that_stays_away_keeps_the_row_uncertain_until_abandoned`, `p6_pause_resume_and_cancel_a_running_print`, `p6_a_second_start_from_finished_needs_prior_state_finished`, and `p6_history_list_honours_order_desc_and_since`. **Real host, read-only:** see [Read-only real-host results](#read-only-real-host-results). |
| 3 | Archive/delete and Connection/credential mutations cannot orphan active or uncertain operations. | `p6_guards.rs` (18 tests), for example `archive_is_blocked_while_a_host_operation_is_unresolved`, `delete_is_blocked_while_a_host_operation_is_unresolved`, `import_is_rejected_whole_while_any_host_operation_is_unresolved`, `an_endpoint_change_is_refused_before_any_probe_or_secret_write`, `clearing_the_connection_is_connection_in_use_while_unresolved`, `clearing_the_credential_is_connection_in_use_while_unresolved`, `replacing_the_credential_on_the_same_endpoint_is_allowed_and_probed_while_unresolved`, `credential_cleanup_never_deletes_a_credential_of_a_printer_with_an_unresolved_row`, and `a_concurrent_insert_and_delete_leave_no_orphan_row`. Each block has an "allowed after a terminal Host Operation" twin. Unblocking after abandonment: `p6_host_ops.rs` `a_host_unreachable_throughout_stays_uncertain_and_abandon_lifts_the_guards`; `p6_tracer.rs`, scenarios A (resolved) and B (abandoned), against `FakeMoonraker` and the simulator. |
| 4 | Capability-aware frontend behaviour distinguishes unsupported from failed operations. | `src/screens/CapabilityList.test.tsx`, `PrinterJobPanel.test.tsx` (unsupported controls aren't rendered; Start… is disabled with the capability reason; failed rows show their failure text), `StartStagedDialog.test.tsx` (one test per Start-table row under both StartSafety values; Failed → Ready enables Start without a reload), `StageOnPrinterDialog.test.tsx` (distinct unsupported, Offline, and pending reasons), `AbandonReconciliationDialog.test.tsx`, and `src/host-ops/*.test.ts` (`startOffer`, `controlOffer`, `capabilityRefusalText`, and the stores). The screenshots below show it rendered. |
| 5 | The tracer completes without duplicate upload or accidental start. | `p6_tracer.rs`: `reconciliation_tracer_runs_against_fake_moonraker` (CI) and `p6_reconciliation_tracer_runs_against_the_simulator` (passed in `20260926T042333Z` and `20260926T051050Z`). Each checks that exactly one upload was sent (on the simulator, by the file listing), that `print_stats` stays `standby`, and that no new history job appears. The read-only variant is `p6_moonraker_readonly.rs` `readonly_reconciliation_tracer`: it seeds an old `uncertain` upload for a never-sent path, restarts, reconciles to `failed{notApplied}`, and makes no upload. |

### The plan's exit gate

- **Capability row with `tier: sim` and a manifest citation:** met (row 1).
  The citation is now a committed file.
- **Simulator command tests; read-only real-host results with zero
  writes:** met (row 2).
- **No orphaning; unblocking only after resolution or abandonment:** met
  (row 3).
- **Unsupported vs failed; the Start table:** met (row 4).
- **Tracer with no duplicate upload and no accidental start; read-only
  variant with no upload:** met (row 5).

### D5 rows: simulator and fake-only evidence (ruling R24)

"Every D5 row observed on the simulator" is met **per row** (upload,
start, and pause/resume/cancel):

- **Upload.** A definitive 201, then `Matches`, in every staging. A lost
  response reconciles to `succeeded`. A cut mid-body settles, then
  `failed{notApplied}`. An unreachable host stays `uncertain`.
- **Start.** 200 `ok` gives `startAccepted`. A lost response reconciles
  from `print_stats`. A queued start reconciles once it runs (42 attempts
  in the gate run). `503 Klippy Disconnected` gives `noLongerPending`.
- **Pause, resume, and cancel.** 200 `ok` with the effect observed.

These **failure cells are covered only by the in-process fakes**, not by
the simulator:

| Cell | Fake-only evidence |
| --- | --- |
| Upload 401 (auth), 403 (file loaded), 422 (checksum), 400 | `p6_moonraker_adapter.rs` `upload_definitive_rejections`, `upload_with_a_wrong_checksum_is_rejected_by_the_fake_like_moonraker`, `upload_over_the_loaded_file_is_file_loaded` |
| Start `SD busy`, `fileMissing` | `p6_moonraker_adapter.rs` `start_definitive_rejections_from_the_host_state`, `scripted_start_rejections_are_definitive`; `p6_host_ops.rs` `a_definitively_rejected_start_fails_with_its_code_and_is_not_reconciled`. Through `host_ops`, steps 7 and 9 refuse these before dispatch (`start_is_refused_with_no_row_when_the_staged_file_is_absent_or_changed`). |
| 503 on a control or a read | `p6_moonraker_adapter.rs` `control_503s_and_400s_classify_like_start`, `a_klippy_503_on_a_read_is_host_not_ready`; `p6_host_ops.rs` `klippy_disconnected_is_uncertain_and_no_longer_pending_never_failed` |
| `effectNotObserved` | `p6_host_ops.rs` `a_control_ok_with_no_state_change_is_effect_not_observed`; `p6_moonraker_adapter.rs` `control_ok_without_an_effect_is_still_ok_at_dispatch` |
| A start proven through history | `p6_host_ops.rs` `a_start_applied_with_only_a_history_job_left_reconciles_from_history`, `only_complete_with_no_job_is_never_proof_of_a_start`, `an_old_job_for_the_same_file_never_proves_a_start`. On the simulator, the cut start was proved from `print_stats` in every run. |

## Read-only real-host results

The owner's Snapmaker U1 (host withheld) was reached through
`FARM3D_MOONRAKER_HOST`, which was given in the dispatch only and never
written to a file. Tasks 12 and 13 ran `just p6-readonly` through the
recording gate in `p6_moonraker_readonly.rs`. The gate forwards only
listed read `GET`s and read RPCs, and refuses anything else with a local
403.

| Test | Result |
| --- | --- |
| `readonly_probe_subscribe_and_capability_detection` | Passed. Moonraker 1.5.2, API 1.4.0; klippy ready; 4 tools; bed; `virtual_sdcard`; `pause_resume`; history; 2 cameras. 13 reads, 0 writes. |
| `readonly_host_job_state_reports_every_tool` | Passed. Klippy Ready, print `Complete`, tool indices 0 to 3. |
| `readonly_locate_of_a_never_sent_file_is_absent` | Passed: `Absent`. |
| `readonly_an_old_uncertain_upload_fails_not_applied_and_a_new_one_stays_uncertain` | Passed. The old row became `failed{notApplied}`; the new row stayed `uncertain` (`uploadSettling`). 2 reads, 0 writes. |
| `readonly_a_blocked_port_keeps_the_row_uncertain_and_abandon_is_local_only` | Passed. `uncertain` (`hostUnreachable`); abandon opened no connection. |
| `readonly_reconciliation_tracer` | Passed. `failed{notApplied}` after a restart. 1 read, 0 writes. |

The last run (Task 13's fix round) passed 6 of 6. **No upload, start,
pause, resume, cancel, G-code, `POST`, or `DELETE` reached the real
host.** The gate recorded no refused request, so none was attempted. The
host value appeared nowhere in the output. This task did not re-run the
suite, because it changed no read-only code.

## Visual verification

### How the screenshots were taken

The Playwright MCP launch timed out after 180 s, as it did in Task 11.
The chrome-devtools MCP failed with `Target.setDiscoverTargets timed out`.
Each was tried once. The screenshots were then taken with a small
`playwright-core` script driving `chrome-headless-shell` (the P4
approach), against `just web` on `http://localhost:1420`. Each view was
loaded fresh, with no console errors, and the PNGs are exactly
1440 × 900 and 1024 × 700. `just web` was stopped afterwards.

`just web` has no Rust backend. Every Printer, capability, and Host
Operation on screen is a web fixture (`src/host-ops/web-fixtures.ts`),
with Connections at `192.0.2.x`. **Web mode refuses every write** ("…
needs the desktop app."). A screenshot of a write action therefore shows
the fixture state and that refusal, not a live simulator write. The live
writes are the simulator tests above; every one targets a loopback
simulator.

Two views need a backend guard that web mode doesn't have: the blocked
archive and the blocked Connection edit. For those, the headless page
served `printer-store.ts` with a patch in the browser only, as P2 did with
its in-page IPC mock:

- the web-mode `lifecycleEligibility` for the uncertain Printer returns
  the blockers `HostOperationBlockers` produces;
- the web-mode `setConnection` for that Printer throws the exact
  `CONNECTION_IN_USE` `CommandError` that `CommandError::connection_in_use`
  builds.

No repository file was changed for this. Those two screenshots verify
rendering, not the backend guard, which `p6_guards.rs` covers.

### Screenshots

All are in `docs/screenshots/`, one per size (`-1440x900.png`,
`-1024x700.png`).

| View | Files | What it shows |
| --- | --- | --- |
| Job tab | `p6-job-tab-*` | The four-tool Snapmaker U1 fixture, printing. File, progress, T0–T3 and bed, Pause and Cancel print… enabled, Resume disabled with its reason, nothing staged, no operations. |
| Stage | `p6-stage-*` | Library → Enclosure lid → Plate 1: Lid → **Stage on Printer…**, with the Printer select open. Printers with no Connection are disabled with "No Connection"; OctoPrint is disabled (its `upload` is `notVerified`); Moonraker — Bay 4 and Four-tool — Bay 5 are selectable. |
| An uncertain row | `p6-uncertain-row-*` | "Upload uncertain" for `farm3d/enclosure-lid.gcode`, "The printer's answer was lost.", Check again and Abandon check…, and every control disabled with "A printer operation is pending. You can still pause or cancel on the printer itself." |
| Check again | `p6-check-again-*` | The same row after **Check again**: the web-mode refusal "Checking a printer needs the desktop app." |
| Abandon | `p6-abandon-*` | The "Stop checking this operation?" dialog: the operation, file, and Printer; the unknown-state warning; the required acknowledgement; the 500-character note; and Stop checking disabled until ticked. |
| Start from Finished | `p6-start-finished-*` | The Finished Printer's staged file → **Start…**: "The previous print finished. The bed is clear." unticked, and Start print disabled with "Tick the confirmation to start." |
| Start after Failed | `p6-start-failed-*` | The Failed Printer's Job tab: every control disabled with "the last print failed", and the failed start row ("Klipper isn't running on the printer."). See the limitation below. |
| The blocked Connection edit | `p6-blocked-connection-*` | Port changed and Save pressed: "Finish or abandon the pending printer operation before changing this Connection." with **Open the Job tab**. |
| Blocked archive | `p6-blocked-archive-*` | Setup → Archive disabled: "Finish or abandon the pending printer operation before archiving." and the delete blocker, each with **Open the Job tab**. |
| Capabilities | `p6-capabilities-*` | The OctoPrint Printer's Setup tab: upload, start, pause, resume, and cancel "Not verified yet" with their details; read print state, verify staged files, and camera "Supported" (read-only hardware). The uncertain Printer's list in `p6-blocked-connection-1440x900.png` shows all eight "Supported" (Simulator). |

### Observations (not blocking)

- **Start after Failed.** No web fixture stages a file on the Failed
  Printer, so the disabled **Start…** button itself isn't in the
  screenshot. `StartStagedDialog.test.tsx` and `PrinterJobPanel.test.tsx`
  cover it: disabled with "Clear the error on the printer first.", and
  enabled again once the status leaves Failed.
- **Capability details wrap narrowly.** In the dock at 1440 × 900, a long
  unsupported detail ("OctoPrint's start has no verified evidence yet.")
  wraps to a column a few words wide. It is readable, but a wider detail
  column would be tidier.
- **The Stage select scrolls.** At both sizes the Printer list is taller
  than the space under the trigger, so Bays 7–9 and the OctoPrint reason
  are below the fold until you scroll.
- **The 1024 × 700 Model details overlay isn't dimmed** behind the Stage
  dialog. The dialog is on top and takes the pointer; only the scrim
  misses the overlay.

## Unavailable checks (ruling R5)

- **The installed-package staging pass** (install the `.deb`, then stage
  one revision to the simulator from it). **Unavailable:** installing a
  `.deb` needs root, which this environment doesn't have.
- **The native `just dev` pass.** **Unavailable:** there is no automation
  for the native WebKitGTK window here, and no human was driving it. The
  owner should do a manual pass: stage, Check again, abandon, and start
  from Finished against `just sim-up`'s Moonraker.

### Packaging

`just package` (a bundle build only; nothing was installed):
exit 0. A release build, then `farm3d_0.1.0_amd64.deb`,
`farm3d-0.1.0-1.x86_64.rpm`, and `farm3d_0.1.0_amd64.AppImage`.
`scripts/assert-package-contents.sh` passed on all three. The build
printed the existing `ts-rs` attribute warnings and the pre-existing
unused `MOONRAKER_KIND` import warning in `supervisor.rs`.

## Known limitations

- **Import deletes Host Operation history (ruling R16).** Printer import
  (`replace_all`) replaces every Printer. Once the import guard passes,
  it deletes the terminal Host Operation rows of every Printer it
  replaces, in the same transaction. Their history is lost on re-import,
  even for a Printer the file brings back under the same id. An
  unresolved row anywhere still rejects the whole import with
  `HOST_OPERATION_PENDING`, and nothing is written. The spec's D7 and
  `CONTEXT.md` ("Host Operation") now say so.
- **The import `HOST_OPERATION_PENDING` banner** renders through
  `HostOperationAlert`, with one Job-tab link per named Printer. No
  end-to-end check has run it against a real import: `just web` can't
  import, and there was no native pass. `App.test.tsx` covers it.
- **`HostOperationAlert`'s fallback label.** When a Printer named in an
  error's details is missing from the Printer store, the link falls back
  to the raw `printerId`. This is cosmetic (parked in Task 11's
  re-review).
- **`observedAt` is always `None`/`null`.** `PrinterCapabilities.observedAt`
  isn't set from a real host-facts read time yet (Tasks 5 and 8 deferred
  it; it needs a host-facts cache).
- **The simulator tests feed Printer status by hand.** In
  `sim_moonraker.rs` and the simulator tracer, the supervisor doesn't
  observe the simulator. `observe_host` feeds the status from the real
  `print_stats`, so the Online hook fires once, at a known moment. The
  supervisor-to-host-ops path is covered in process
  (`going_online_triggers_an_attempt` and its neighbours in
  `p6_host_ops.rs`).
- **A pre-existing `p6_host_ops.rs` flake.**
  `abandon_cancels_the_pending_retry_timers` failed intermittently (1 run
  in 3) during the Task 13 review, and it reproduces on the base commit
  `3280cbd`. It passed in this task's `just test-rust` run. It is parked
  for the final review.
- **A start that finishes very quickly** can stay `uncertain` until it is
  abandoned: a file that ends within one status batch leaves no history
  job (spike Gate F), so there's nothing to prove it ran.
- **The simulator's single-upload check** uses the file listing, not a
  request count. A second upload of identical bytes to the same path
  wouldn't be caught on the simulator; the `FakeMoonraker` tracer counts
  requests and would catch it.

## Follow-ups

- **OctoPrint** stays `notVerified` on every capability. Its command
  capabilities need their own research and evidence (with #10).
- **ElegooLink** is out of P6's scope: #8 closed without a go decision.
- A native and installed-package pass by the owner (above).
- A web fixture that stages a file on the Failed Printer, so the disabled
  Start… is visible in web mode.
- Set `observedAt` from a real host-facts read.

## Documentation updated in this task

- **The spec (ruling R14).** D7 and the exported-types note spell the
  blocker code `HOST_OPERATION_UNRESOLVED`, matching the SCREAMING_SNAKE
  `LifecycleBlockerCode` enum on the wire. The P6 plan's guard table is
  fixed the same way. D7 also records R16's import behaviour, and D6's
  `source` comment names the committed manifest copy.
- **The v1 approach doc.** The "Adapter command/camera capabilities" row
  in the known-unknowns register is resolved **for Moonraker only**, and
  it lists OctoPrint and ElegooLink as still open.
- **`CONTEXT.md`.** Adds **Reconciliation** (the `reconciling` state, its
  triggers, reads only). "Host Operation" gains its six states, the
  import block, and what happens to terminal rows on delete and import.
  "Connection" lists all eight capabilities.

## Files changed in this task

- `src-tauri/src/connections/adapters.rs`: `MOONRAKER_SIM_MANIFEST` names
  the committed copy; its test reads the file.
- `src-tauri/src/connections/capabilities.rs`: the `source` doc comment.
- `src/generated/contracts/domain/CapabilityEvidence.ts`: regenerated
  (the doc comment only).
- `src-tauri/tests/p6_capabilities.rs`: the `source` assertion.
- `docs/superpowers/baselines/2026-09-26-p6-sim-manifest-20260926T033455Z.json`,
  `…-20260926T042333Z.json`, `…-20260926T051050Z.json` (new).
- `docs/screenshots/p6-*.png` (20 new).
- `docs/verification/2026-09-26-p6-moonraker-commands.md` (this file).
- `docs/superpowers/specs/2026-09-25-p6-connection-command-capabilities-design.md`,
  `docs/superpowers/plans/2026-09-25-p6-connection-command-capabilities.md`,
  `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`,
  and `CONTEXT.md`.
