# P2 Printer lifecycle and batch setup verification

**Date:** 2026-09-22
**Platform:** Linux
**Validated source:** `97b27d9` (`fix: keep the batch dialog's footer
reachable at compact viewports`), the last code commit on
`feature/p2-printer-lifecycle` before this doc, itself committed as
`docs: record P2 verification evidence` alongside the tracer test
(`src-tauri/tests/p2_tracer.rs`) and the `CONTEXT.md`/known-unknowns
updates. Covers Tasks 1–12 (P2 in full).

## Automated evidence

| Command | Result |
| --- | --- |
| `just build` | Passed: TypeScript type check and Vite production build completed successfully. |
| `just test` | Passed: 32 files, 311 tests. The runner emitted five existing jsdom `Window.scrollTo()` notices; it exited 0. |
| `source "$HOME/.cargo/env" && just test-rust` | Passed: 255 library tests (1 ignored); 28 export-contract tests (1 ignored); plus 5 `f0_tauri_path`, 3 `f1_contract_path`, 11 `f1_import_export`, 5 `f1_migration`, 7 `f1_repositories`, 12 `f1_residual_acceptance`, 15 `p2_batch`, 12 `p2_contract_path`, 7 `p2_lifecycle`, 5 `p2_migration`, **1 `p2_tracer`**, and 3 `snapshot` tests. Two existing `ts-rs` transparent/`double_option`-serde-attribute warnings were emitted, as before. |
| `source "$HOME/.cargo/env" && just gen-contracts` then `git diff --exit-code src/generated` | Passed: regeneration ran clean and the diff against the committed `src/generated` tree was empty (exit 0) — the frontend's generated contracts already match the Rust side. |

### The tracer (spec acceptance criterion 16)

`src-tauri/tests/p2_tracer.rs`,
`the_tracer_creates_a_batch_archives_one_printer_and_survives_a_restart`,
drives the real `tauri::test` IPC path in one test:

1. `create_printer` with no `connection` → a Profile-only Printer
   (`setupGaps: ["missingConnection"]`).
2. `create_printer` with a `connection` to the fake factory's `voron.local`
   → a connected Printer (`setupGaps: []`).
3. `create_printers_batch`, 4 rows across `"Bay A"` and `"Bay B"`:
   - `ok.local`, probed successfully → `created`.
   - `auth.local` → `createdSetupIncomplete` with `AUTHENTICATION_FAILED`,
     Connection stripped from the stored Printer, as required.
   - no Connection given → `createdSetupIncomplete`.
   - `unreachable.local`, probe fails → `createdSetupIncomplete`.
4. `archive_printer` on the batch's `ok.local` row (the one Printer under
   active supervision at archive time).
5. A simulated restart: a brand-new `ConnectionManager`/`CredentialStore`
   driven through the same `restore_persisted_connections` function
   `lib.rs` uses at real startup, over the *same* `Storage` and the *same*
   credentials directory the original services wrote the connected
   Printer's secret to (production reopens the same on-disk credential
   store at restart, not a fresh one) — mirroring `p2_lifecycle.rs`'s
   existing restart test.
6. After restart: `list_printers` still returns all 6 Printers. The
   archived one keeps its id and name, has `archivedAt` set, and is absent
   from the restarted manager's `statuses()` map (no status). The other 5
   are present with `archivedAt: null` and the correct `setupGaps`
   (`[]` for the connected Printer, `["missingConnection"]` for the four
   Profile-only/incomplete ones). Resupervision of the connected Printer is
   proven two ways, not merely inferred from status presence (see "Fix
   round 1" below for why that distinction matters): its status has
   `connectionState: "connecting"` — which only `ConnectionManager::start`
   ever sets, never the reconcile path a Printer with no usable Connection
   takes — and the restart-scoped fake connection factory was actually
   invoked, exactly once, for host `voron.local` and no other host (every
   other Printer either has no Connection at all, or, for the archived
   one, is stopped rather than started).

### Acceptance criteria 1–17, mapped to automated evidence

| # | Criterion | Evidence |
| --- | --- | --- |
| 1 | Migration v2→v3: ledgered, crash-boundary-safe, backfills host identity, archives pre-existing duplicate hosts with a warning | `p2_migration.rs`: `fresh_database_reaches_v3_with_a_matching_ledger_row`, `upgrading_v2_archives_every_duplicate_host_printer_but_the_oldest`, `after_upgrade_a_third_active_printer_on_the_same_host_conflicts`, `a_crash_before_commit_leaves_the_database_unchanged_at_v2`, `location_and_start_safety_check_constraints_reject_invalid_values` |
| 2 | Profile-only Printer creates, persists across restart, shows Setup incomplete not Offline | `p2_contract_path.rs`: `create_printer_profile_only_trims_location_and_reconciles_to_setup_incomplete`; `p2_tracer.rs` (restart) |
| 3 | Connected Printer created in one step; credential under its own ref; supervision starts only after commit | `p2_contract_path.rs`: `create_printer_with_connection_and_credential_starts_supervision_after_commit` |
| 4 | `probe_connection` leaves storage byte-identical, no supervisor task/status event | `p2_contract_path.rs`: `probe_connection_has_no_side_effects`, `probe_connection_to_auth_local_is_authentication_failed_and_never_leaks_the_secret` |
| 5 | Batch: one failing probe still creates the row (`createdSetupIncomplete`); invalid-identity row `rejected`, unpersisted, input retained | `p2_batch.rs`: `a_partial_batch_creates_valid_rows_and_rejects_only_invalid_identity`; `p2_tracer.rs` |
| 6 | Retrying a `createdSetupIncomplete` row attaches a Connection to the same Printer, never a duplicate | `p2_contract_path.rs`: `set_printer_connection_probes_before_replacing_a_working_connection`, `first_time_set_printer_connection_never_probes` (backend half); `src/screens/PrinterBatchDialog.test.tsx` (frontend retry flow, see AC15) |
| 7 | A seeded shared secret appears in no batch result, error, warning, log, SQLite file, or generated contract | `p2_batch.rs`: `batch_secrets_never_leave_the_credential_store`, `batch_output_streams_never_contain_the_secrets` |
| 8 | Probes never exceed 4 concurrent; cancellation leaves uncommitted rows `cancelled`, unpersisted | `p2_batch.rs`: `at_most_four_probes_are_in_flight`, `cancelling_keeps_committed_rows_and_cancels_the_rest` |
| 9 | Duplicate hosts rejected against DB, within a batch, and on unarchive; archived Printers don't reserve hosts | `p2_batch.rs`: `a_later_row_duplicating_an_earlier_rows_host_is_created_without_a_connection`, `a_row_duplicating_an_active_printers_host_is_created_without_a_connection`; `p2_contract_path.rs`: `create_printer_with_a_duplicate_active_host_persists_nothing`; `p2_lifecycle.rs`: `unarchiving_into_a_reclaimed_host_fails_with_duplicate_host`; `p2_migration.rs`: `after_upgrade_a_third_active_printer_on_the_same_host_conflicts` |
| 10 | Shared bed type copied as an independent per-Printer override | `p2_batch.rs`: `each_created_printer_gets_its_own_default_bed_type_override` |
| 11 | Archive stops supervision, survives restart unsupervised, keeps identity; unarchive restores supervision | `p2_lifecycle.rs`: `archiving_a_connected_printer_stops_supervision_and_preserves_the_connection_and_credential`, `after_archiving_a_restart_never_supervises_the_archived_printer_but_keeps_it_listed`, `unarchiving_restarts_supervision`; `p2_tracer.rs` |
| 12 | Deleting a non-archived Printer fails `LIFECYCLE_BLOCKED`; deleting an archived Printer keeps the delete/credential-cleanup ordering | `p2_lifecycle.rs`: `delete_of_an_active_printer_is_lifecycle_blocked_and_leaves_the_row_untouched`, `deleting_an_archived_printer_succeeds_and_removes_its_credential` |
| 13 | Replacing a working Connection with a failing one is rejected unless `acceptUnverified` | `p2_contract_path.rs`: `set_printer_connection_probes_before_replacing_a_working_connection` |
| 14 | Location, start safety, and archive state round-trip through export v2; v1 imports still load | `f1_import_export.rs`: `printers_export_writes_schema_version_2_with_lifecycle_fields`, `printers_import_defaults_lifecycle_fields_for_a_v1_document` |
| 15 | Frontend tests cover the wizard, batch intake, generation, mapping, retry, Setup tab guards, Monitor location/archive views | `src/printers/batch-intake.test.ts`, `src/printers/host-identity.test.ts`, `src/printers/printer-store.test.ts`, `src/screens/PrinterSetupWizard.test.tsx`, `src/screens/PrinterBatchDialog.test.tsx`, `src/screens/BatchRowsTable.test.tsx`, `src/screens/PrinterSetupPanel.test.tsx`, `src/screens/MonitorToolbar.test.tsx`, `src/monitor/monitor-store.test.ts` — 132 tests across these 9 files, all passing (subset of the 311-test `just test` run above) |
| 16 | The tracer completes through the Tauri path | `p2_tracer.rs` (new; see above) |
| 17 | Keyboard and viewport checks pass at 1440 × 900 and 1024 × 700 | **Unavailable** — see "Manual verification limitations" below |

## Manual verification limitations

Per Ruling R5, this non-interactive session cannot drive the native Tauri
window, so the plan was to check layout and keyboard operation against
`just web` (frontend-only, browser at `http://localhost:1420`) using the
Playwright browser tools.

`just web` itself started cleanly (`vite v6.4.3 ready`, listening on
`http://localhost:1420/`), confirming the web-mode dev server is healthy.
However, every attempt to drive a browser against it failed at the browser
*launch* step, before any page load: `mcp__playwright__browser_navigate`
timed out after 180 s three times in a row, each time with Chrome launched
(`--remote-debugging-pipe ... about:blank`) but never completing its CDP
handshake (no renderer/GPU subprocess ever spawned). To rule out an
MCP-specific problem, the same launch was reproduced directly from the
shell — `chrome --remote-debugging-pipe --user-data-dir=... about:blank`
with file descriptors 3/4 provided — and it hung identically (`timeout`
killed it at 15 s with no CDP output on fd 4), while the same Chrome binary
launched fine with a TCP `--remote-debugging-port` instead. This points to
a pipe-transport-specific incompatibility in this sandboxed environment,
not a flaky one-off; retrying further would not have changed the outcome.

Because of this, the following are recorded as **unavailable, not passed**,
exactly as the P1 doc records its equivalent gaps:

- Wide 1440 × 900 and compact 1024 × 700 walkthroughs of the setup wizard,
  the "Add Printers…" batch dialog (row generation, Bay A/Bay B, create),
  the Monitor's Location section, a Printer's Setup tab archive action and
  the Archived filter.
- The batch dialog footer's reachability and the batch grid's horizontal
  (not page) scroll at 1024 × 700, in an actual rendered browser.
- The keyboard-only pass (Tab/Enter/Space/Escape) through the wizard.
- `docs/screenshots/p2-*.png` — none were captured; no browser session ever
  reached a page to screenshot.

No real Moonraker host or Printer hardware was available, so live
connected-Printer probing/supervision also remains **unavailable**, as in
P1 — every probe and supervision path above is covered by the injected
fake connection factory instead.

The native desktop window (`just dev`) was not attempted this round, for
the same reason P1 recorded it as unavailable: a non-interactive session
cannot operate a launched Tauri window once open.

## Deviations and findings

- **Monitor default section is not switched to Location automatically.**
  Per the design's planning clarification, the persisted `monitorSection`
  preference defaults to `printerModel`, and the app cannot distinguish
  "never chosen" from "chose Printer Model" — so P2 does not auto-switch
  the section to Location once locations exist, departing from the
  umbrella rule that Location is the default once locations exist. This is
  by design, not a defect.
- **A missing credential is not a `setupGap`.** Per the design's plan
  clarification 1, `derive_setup_facts`'s `setupGaps` covers
  `missingConnection`, `unsupportedAdapter`, and `unresolvedProfile` only.
  A Connection whose credential cannot be read at runtime is *not* a setup
  gap; it surfaces as a runtime authentication error at supervision/probe
  time (`ConnectionError::Auth` → `AUTHENTICATION_FAILED`), the same as
  today. This is by design, not a defect.
- **Batch dialog footer: found and fixed a real layout defect, via static
  CSS review (visual confirmation was unavailable — see above).** A prior
  review flagged that the batch dialog's Create/Cancel/Retry footer could
  scroll below the fold at 1024 × 700. Reading the CSS confirmed the
  mechanism: `Dialog.module.css`'s `.content` is the *only* scroll
  container (`max-height: 85vh; overflow: auto;`), and
  `PrinterBatchDialog.module.css`'s `.footer` sat in normal flow inside
  it — at 700 px tall (85vh ≈ 595 px) with a full rows grid or results
  list, the footer could indeed end up below the visible area, requiring
  the user to scroll the whole dialog to find the action buttons. Fixed
  minimally, token-only, in `src/screens/PrinterBatchDialog.module.css`:
  `.footer` is now `position: sticky; bottom: 0;` with
  `background-color: var(--f3d-color-surface-raised)` (the same token
  `Dialog.module.css` already uses for the dialog surface), so it stays
  pinned to the bottom of the scroll container and is always reachable.
  `BatchRowsTable.module.css`'s `.scroller` already had
  `overflow-x: auto`, so the row grid's own horizontal scroll (the other
  half of this concern) was already correctly contained inside the
  dialog, not the page — confirmed by reading the CSS, not by rendering.
  `just build`, `just test`, and the batch dialog/table test files
  (`PrinterBatchDialog.test.tsx`, `BatchRowsTable.test.tsx`, 35 tests) were
  re-run after the change and still pass; jsdom does not compute CSS
  `position: sticky`, so this fix's actual on-screen effect could not be
  asserted by an automated test and remains part of the unavailable
  manual-viewport verification above, not confirmed visually.

## Fix round 1: the restart-resupervision assertion was a false positive

Review found a Critical issue in the tracer's restart step: `restart_credentials`
was built from a brand-new, empty temp directory rather than the original
`credentials_dir` the connected Printer's secret had been written to. On
restart, `supervise_printer` therefore always took its `CredentialRequired`
branch for the connected Printer — which still calls `reconcile_printer`
and still inserts a status. The old assertion,
`restart_manager.statuses().contains_key(&connected_id)`, is true for
*every* non-archived Printer regardless of whether supervision actually
started, so it proved nothing beyond "this Printer exists and isn't
archived."

**Fix**, in `src-tauri/tests/p2_tracer.rs`:

1. `restart_credentials` now reopens the *same* `credentials_dir` the
   original `common::runtime` call used, exactly as production reopens the
   same on-disk credential store at restart rather than a fresh one.
2. The restart `ConnectionManager` is now built with a recording factory
   (`recording_factory`, new in this file, mirroring the pattern already
   used in `p2_lifecycle.rs`/`p2_contract_path.rs`) instead of the plain
   `factory`, so the test can observe which hosts actually reach it.
3. The assertion is now two discriminating checks instead of one
   non-discriminating one:
   - `restart_manager.statuses()[&connected_id].connection_state ==
     ConnectionState::Connecting` — only `ConnectionManager::start` ever
     sets `Connecting`, and it does so synchronously before spawning the
     task that calls the factory, so this is available immediately with
     no timing race. The reconcile path this bug took instead leaves
     `Offline` (a fresh manager's default) with `operationalState:
     SetupIncomplete`.
   - The restart factory is asserted (via a 2 s `recv_timeout`, since the
     actual factory call happens inside the asynchronously spawned task)
     to have been called exactly once, for host `voron.local` — proving
     the factory was reached at all — and never again
     (`expect_no_more_calls`), which also rules out the Profile-only
     Printer, the three `createdSetupIncomplete` rows (none of which have
     a Connection to supervise), and the archived Printer (which is
     stopped, not started) ever reaching it.

**RED/GREEN proof that the fix is discriminating:** I temporarily
reverted step 1 alone (restart credentials pointed back at a fresh empty
`tempfile::tempdir()`, everything else unchanged) and re-ran `cargo test
--test p2_tracer`:

```
thread '...' panicked at tests/p2_tracer.rs:303:5:
assertion `left == right` failed: the connected Printer must be
resupervised (Connecting), not just reconciled: PrinterStatus {
connection_state: Offline, ... operational_state: SetupIncomplete,
readiness: PrinterReadiness { state: NotReady, reason:
Some(SetupIncomplete) }, ... }
  left: Offline
 right: Connecting
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

RED, as expected — the new assertion catches exactly the bug review
found. I then restored the fix (diffed clean against the pre-revert file)
and re-ran:

```
test the_tracer_creates_a_batch_archives_one_printer_and_survives_a_restart ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

GREEN. `source "$HOME/.cargo/env" && just test-rust` was then re-run in
full: same counts as the original verification pass (255 library tests, 1
ignored; 28 export-contract, 1 ignored; 5/3/11/5/7/12/15/12/7/5/**1**/3
across `f0_tauri_path`/`f1_contract_path`/`f1_import_export`/
`f1_migration`/`f1_repositories`/`f1_residual_acceptance`/`p2_batch`/
`p2_contract_path`/`p2_lifecycle`/`p2_migration`/**`p2_tracer`**/
`snapshot`), all passing, no regressions.

The "Automated evidence" and "The tracer" sections above already reflect
the fixed test's actual, corrected behavior — no stale wording was left
describing the disproven claim.

## Files changed in this task

- `src-tauri/tests/p2_tracer.rs` (new, then fixed in review round 1): the
  tracer test.
- `CONTEXT.md`: added Location, Profile-only Printer, Setup incomplete,
  Start-safety rule, and Archive.
- `docs/superpowers/plans/2026-09-16-complete-v1-implementation-approach.md`:
  marked the `group`-to-location-migration and Batch-CSV/host-matching
  known unknowns resolved.
- `src/screens/PrinterBatchDialog.module.css`: sticky footer fix (see
  above).
- `docs/verification/2026-09-22-p2-printer-lifecycle.md` (this file).
