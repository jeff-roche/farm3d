# F1 Foundation Verification

The authoritative implementation plan is
[`docs/superpowers/plans/2026-09-17-f1-foundation.md`](../plans/2026-09-17-f1-foundation.md).
F1 was planned; this baseline must not be read as evidence that implementation
started without a plan.

Verified on 2026-09-17 in the `feature/f1-foundation` Linux worktree. This file records observed evidence, not inferred platform support.

## Environment

- Linux 7.2.4-3-cachyos, x86_64
- Node.js 24.19.0 and npm 11.17.0
- rustc/cargo 1.98.0
- Tauri CLI 2.11.4

## Automated evidence

- `just gen-contracts`: generated bindings matched the committed contract tree.
- `just build`: TypeScript and the Vite production build passed.
- `just test`: 163 frontend tests passed, including listener-before-backfill, event-during-backfill, duplicate, gap, stream restart, failed-backfill retry, removed-Printer filtering, late listener disposal, applied import settlement, exact UTF-8 revision-precondition ordering, Connection credential-field release after save settlement, the top-level `create_printer` request, and discovery-error propagation.
- `source "$HOME/.cargo/env" && just test-rust`: 200 library tests and 65 integration tests passed; one subprocess helper and the opt-in contract generator were ignored as designed. Focused regressions cover shared failed-bootstrap retries, startup Printer decode failure, post-commit Printers import settlement, discovery worker and hard-timeout mappings, SQLite corruption mapping, and generated contract shapes.
- `source "$HOME/.cargo/env" && just package`: produced DEB, RPM, and AppImage bundles. `scripts/assert-package-contents.sh` inspected each inventory and extracted payload, then passed with no source, test, metadata, credential, legacy-data, developer-tool, or test-secret sentinel payload.
- `git diff --check`: passed.

The migration tests use the canonical F0 `settings.json`, `printers.json`, and `credentials.json` fixtures. They verify idempotent migration, rollback on corrupt input, preserved IDs/Profile/Connection data, restart persistence, and exclusion of fixture and submitted-secret sentinels from durable metadata and generated contracts. The complete mock-runtime tracer migrates F0 data, invokes versioned Printer and Settings commands, performs native-document-boundary imports and exports, checks the status event/backfill boundary without duplication, drops the app and database handle, and verifies imported state after reopening Storage. Separate coverage confirms all 23 registered handlers return the captured nonretryable bootstrap failure. Contract generation also verifies top-level `create_printer` fields, exact `SettingsRecord` naming, and generated `unsupported`/`desktopRequired` outcomes without frontend `Promise<unknown>` result compensation.

## Package evidence

The package gate produced:

- `src-tauri/target/release/bundle/deb/farm3d_0.1.0_amd64.deb`
- `src-tauri/target/release/bundle/rpm/farm3d-0.1.0-1.x86_64.rpm`
- `src-tauri/target/release/bundle/appimage/farm3d_0.1.0_amd64.AppImage`

The package-content assertion listed and extracted the DEB with `ar`/`tar`, the
RPM with `bsdtar`, and the AppImage with `7z`.

## Desktop evidence

`timeout 20s just dev` started Vite, compiled the Tauri development binary, and reached `Running target/debug/farm3d` without a startup error. The command was then terminated intentionally by the timeout.

No human-controlled visual session was available to this agent. The required interaction checks at 1440 × 900 and 1024 × 700—Printer CRUD, Connection save/probe/clear, discovery failure, status display, and native Settings/Printers import/export dialogs—remain manual visual checks. This baseline does not claim that those workflows were visually inspected.
