# Supported Platforms

## Decision

No operating system is promoted to `Supported` by configuration intent alone.
The F0 run was performed only on a Linux x86_64 CachyOS development host. Its
automated suites, exact full package command, and release-executable smoke
launch passed after the package recipe disabled linuxdeploy's incompatible
legacy strip step. Linux x86_64 is therefore `Supported`, and F0 acceptance is
met.

`src-tauri/tauri.conf.json:25-35` sets bundle `"targets": "all"`. In Tauri this
means all bundle types available on the current build host; it does not mean all
operating systems or CPU architectures.

## Evidence Levels

| Level | Meaning | F0 use |
|---|---|---|
| Source/configuration | Code and configuration inspection establishes intended paths and target settings. | Available for the shared codebase; not runtime platform proof. |
| Automated tests | Type checking, frontend unit/component tests, Rust unit/integration tests, and mock-runtime IPC execute repeatable behavior. | Passed on the Linux x86_64 host; mock IPC is below real-WebView E2E. |
| Package build | The native packaging command completes and emits the expected host bundles. | Passed on Linux; `.deb`, `.rpm`, and AppImage completed. |
| Packaged/installed launch | A packaged or release executable starts on the native platform; installed launch is stronger than a build-tree executable. | Build-tree release executable survived 10 seconds on Linux; no bundle was installed. |
| Live external-system exercise | A bounded run uses a representative authorized external service or device. | Not run; no authorized Moonraker endpoint was present. |

Evidence accumulates. A higher row does not erase a failed lower prerequisite,
and source/configuration or tests alone cannot establish a platform support
claim.

## Matrix

| Platform | v1 release status | F0 evidence | Required before platform claim |
|---|---|---|---|
| Linux x86_64 | Supported | On CachyOS x86_64: clean frontend build; 130 frontend and 114 top-level Cargo tests passed; the parent IPC test separately executed the outer harness's one ignored child target successfully. Exact `just package` exited 0, verified the dev-only generator was absent from all three bundle inventories, and produced a 5,456,930-byte `.deb`, 5,456,567-byte `.rpm`, 108,878,328-byte unstripped AppImage, and 18,754,752-byte release executable. With display available, that newly built executable stayed alive for 10 seconds and was terminated by SIGTERM. No install or visual acceptance. | Clean build/test/package and packaged-executable launch met in F0; later feature-specific installed-bundle checks remain required |
| Windows x86_64 | Candidate, unverified | Not run in F0 | Native clean build/test/package; installed launch; credential-store and later permission/notification/process checks |
| Windows arm64 | Candidate, unverified | Not run in F0 | Native clean build/test/package; installed launch; credential-store and later permission/notification/process checks |
| macOS x86_64 | Candidate, unverified | Not run in F0 | Native clean build/test/package; signed/notarized installation decision; launch; Keychain and later permission/notification/process checks |
| macOS arm64 | Candidate, unverified | Not run in F0 | Native clean build/test/package; signed/notarized installation decision; launch; Keychain and later permission/notification/process checks |

## Promotion Requirements

Linux x86_64 met the F0 promotion threshold through a clean native rerun of
`just build`, `just test`, `just test-rust`, and the full `just package`, followed
by a build-tree release-executable launch. Later phases must add
feature-specific installed-bundle checks and representative external-system
evidence where those features depend on OS integration or hardware. F0 does not
claim installation or visual acceptance.

Each Windows and macOS row requires native execution on that exact architecture.
Cross-platform Rust compilation, broad Tauri bundle configuration, or another
platform's test results are insufficient. Credential-store behavior and later
permission, notification, and process-management paths must be exercised on the
native OS. macOS additionally requires an explicit signing/notarization and
installation decision before release support is claimed.

## F0 Evidence

| Evidence | UTC start | Result | Artifact/log |
|---|---|---|---|
| Frontend build | 2026-09-17T14:25:02Z | Exit 0; TypeScript/Vite passed | `/tmp/farm3d-f0/final/just-build.log` |
| Frontend tests | 2026-09-17T14:25:13Z | Exit 0; 14 files, 130 tests passed | `/tmp/farm3d-f0/final/just-test.log` |
| Rust tests | 2026-09-17T14:25:40Z | Exit 0; 110 library + 1 parent IPC integration + 3 snapshot tests passed; 1 child target ignored by the outer harness and passed when invoked by the parent; disabled generator target absent | `/tmp/farm3d-f0/final/just-test-rust.log` |
| Frontend-only server | 2026-09-17T12:18:54Z | Root and catalog resource returned HTTP 200; not Tauri evidence | `/tmp/farm3d-f0/just-web-result.log` |
| Original full package red | 2026-09-17T12:19:00Z | Exit 1; bundled strip rejected modern `.relr.dyn` sections | `/tmp/farm3d-f0/just-package.log` and `package-verbose.log` |
| AppImage-only hypothesis | 2026-09-17T13:03:47Z | Exit 0 with `NO_STRIP=1`; AppImage completed | `/tmp/farm3d-f0/no-strip-appimage-hypothesis.log` |
| Final exact full package | 2026-09-17T14:25:55Z | Exit 0; clean `.deb`, `.rpm`, and AppImage completed; all archive inventories omitted `gen-catalog` | `/tmp/farm3d-f0/final/just-package.log` and `usr-bin-inventory.log` |
| Release executable launch | 2026-09-17T14:27:45Z | Rebuilt executable (mtime `2026-09-17T14:26:32Z`) alive for 10 seconds with display available; then SIGTERM, wait status 143 | `/tmp/farm3d-f0/final/launch-result.log` |
| Live Moonraker | 2026-09-17T14:28:50Z | Not run; configured endpoint variable absent, no probe attempted | `/tmp/farm3d-f0/final/moonraker-presence.log` |

The package recipe sets `NO_STRIP=1` because linuxdeploy's bundled legacy
`strip` cannot parse this host's modern ELF `.relr.dyn` sections. Upstream
linuxdeploy's `executeDeferredOperations()` explicitly clears deferred strip
operations when that variable is present. The compatibility tradeoff is a
larger, unstripped 108,878,328-byte AppImage.

The baseline details, limitations, source citations, command inventory and
artifact sizes are recorded in
`docs/superpowers/baselines/2026-09-16-f0-baseline.md`.
