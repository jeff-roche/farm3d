# P1 Shell and Monitor verification

**Date:** 2026-09-18
**Platform:** Linux
**Validated source:** Task 11 fix-round-1 working tree at `93579f9b757cfd1e331e579a25d26b0bc738bda0` plus the uncommitted fix-round-1 changes recorded in this note.

## Automated evidence

| Command | Result |
| --- | --- |
| `npm test -- src/App.test.tsx` | Passed: 1 file, 9 tests. Covers settings → durable Printer → listener/backfill ordering, sync-state transitions, valid/unknown Printer deep links, import reconciliation, recoverable startup failures, retry orchestration, and late listener disposal. |
| `just build` | Passed: TypeScript type check and Vite production build completed successfully. |
| `just test` | Passed: 26 files, 199 tests. The runner emitted three existing jsdom notices: `Not implemented: Window's scrollTo() method`; it exited 0. |
| `source "$HOME/.cargo/env" && just test-rust` | Passed: 229 library tests, 28 export-contract tests, 5 `f0_tauri_path` tests, 3 `f1_contract_path` tests, 8 `f1_import_export` tests, 5 `f1_migration` tests, 4 `f1_repositories` tests, 12 `f1_residual_acceptance` tests, and 3 snapshot tests. Three existing `ts-rs` warnings report that serde `transparent` cannot be parsed (two before tests and one during doc-tests). |

The existing `status_runtime_hydrates_stale_then_publishes_live_and_removal_once` Tauri tracer already covers restart hydration, stale-to-live publication, a racing backfill whose status matches the emitted event status and stream/sequence, exactly-once status emission, and removal. Task 11 found no genuine Tauri integration gap, so `src-tauri/tests/f0_tauri_path.rs` was not changed.

## Desktop launch attempt

`WAYLAND_DISPLAY=wayland-0` was available. `source "$HOME/.cargo/env" && just dev` started Vite at `http://localhost:1420/`, compiled the Tauri development binary, and ran `target/debug/farm3d`. The command was terminated by the 30-second non-interactive command timeout (signal 15).

## Manual verification limitations

The display was detected, but this non-interactive session could not inspect or operate the launched desktop window. These checks are **unavailable**, not passed:

- Wide 1440 × 900 tracer: inline dock, filters, section/density controls, rosters, stale readings, and missing-reading presentation.
- Compact 1024 × 700 tracer: overlay dock, card-to-row transition, reachable primary actions, Escape dismissal, and unclipped controls.
- Keyboard and reduced-motion traversal: rail, toolbar, Printer content, dock, status bar, focus visibility, and animation suppression.

No real Moonraker host or Printer hardware was available. The restart/stale-to-live behavior was verified through the deterministic Rust Tauri tracer, but live hardware telemetry, connection loss, and restart against a real host remain **unavailable**.
